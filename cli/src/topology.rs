//! Snapshot topology using paginated official reads and adopted-device uplinks.
use crate::{
    buffer::Buffer,
    flows::{self, Remote},
    pages,
};
use core::fmt::{self, Write};
use heapless::String;
use unifi::{Json, network};

fn id<'a>(value: Json<'a>, field: &str) -> Option<unifi::JsonString<'a>> {
    value.get(field).ok().flatten()?.string().ok()
}
fn parent(value: Json<'_>) -> Option<unifi::JsonString<'_>> {
    id(value.get("uplink").ok().flatten()?, "deviceId")
}
pub fn build<'a>(
    remote: &mut Remote<'_, '_>,
    site: &str,
    output: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let mut device_buffer = [0; 131072];
    let mut client_buffer = [0; 262144];
    let mut network_buffer = [0; 131072];
    let devices = pages::collect(
        remote.url,
        remote.base,
        remote.key,
        &network::get_adopted_device_overview_page(site).unwrap(),
        remote.options,
        remote.response,
        &mut device_buffer,
    )?;
    let devices = Json::parse(devices).unwrap();
    let clients = pages::collect(
        remote.url,
        remote.base,
        remote.key,
        &network::get_connected_client_overview_page(site).unwrap(),
        remote.options,
        remote.response,
        &mut client_buffer,
    )?;
    let networks = pages::collect(
        remote.url,
        remote.base,
        remote.key,
        &network::get_networks_overview_page(site).unwrap(),
        remote.options,
        remote.response,
        &mut network_buffer,
    )?;
    let mut out = Buffer::new(output);
    out.write_str("{\"devices\":[")
        .map_err(|_| "topology capacity")?;
    for (i, device) in devices.items().unwrap().enumerate() {
        let id: String<512> = flows::owned(device, "id")?;
        let details = remote.execute(
            &network::get_adopted_device_details(site, &id).unwrap(),
            None,
        )?;
        if i > 0 {
            out.write_char(',').map_err(|_| "topology capacity")?;
        }
        // Detail responses contain the same identity fields plus uplink/interfaces/features.
        out.write_str(details.raw())
            .map_err(|_| "topology capacity")?;
    }
    write!(
        out,
        "],\"clients\":{},\"networks\":{}}}",
        core::str::from_utf8(clients).unwrap(),
        core::str::from_utf8(networks).unwrap()
    )
    .map_err(|_| "topology capacity")?;
    Ok(Json::parse(out.finish()).unwrap())
}
fn display(out: &mut dyn Write, value: Json<'_>, keys: &[&str], demo: bool) -> fmt::Result {
    if demo {
        return out.write_str("[redacted]");
    }
    for key in keys {
        if let Some(value) = id(value, key) {
            for c in value.chars() {
                out.write_char(if c.is_control() { '?' } else { c })?;
            }
            return Ok(());
        }
    }
    out.write_char('-')
}
fn client_line(
    out: &mut dyn Write,
    client: Json<'_>,
    depth: usize,
    demo: bool,
    networks: Json<'_>,
) -> fmt::Result {
    for _ in 0..depth {
        out.write_str("  ")?;
    }
    out.write_str("└─ ")?;
    display(out, client, &["name", "hostname", "macAddress", "id"], demo)?;
    out.write_str(" · ")?;
    display(out, client, &["ipAddress"], demo)?;
    out.write_str(" · ")?;
    display(out, client, &["type"], false)?;
    if let Some(network) = id(client, "networkId") {
        for row in networks.items().map_err(|_| fmt::Error)? {
            if id(row, "id").is_some_and(|v| v.chars().eq(network.chars())) {
                out.write_str(" · ")?;
                display(out, row, &["name"], demo)?;
                break;
            }
        }
    }
    out.write_char('\n')
}
pub fn render(out: &mut dyn Write, topology: Json<'_>, demo: bool) -> fmt::Result {
    let devices = topology.required("devices").map_err(|_| fmt::Error)?;
    let clients = topology.required("clients").map_err(|_| fmt::Error)?;
    let networks = topology.required("networks").map_err(|_| fmt::Error)?;
    let mut rows = heapless::Vec::<Json<'_>, 1024>::new();
    for row in devices.items().map_err(|_| fmt::Error)? {
        rows.push(row).map_err(|_| fmt::Error)?;
    }
    let mut seen = [false; 1024];
    let mut stack = heapless::Vec::<(usize, usize), 1024>::new();
    // Roots first, then any orphan or cycle. Each device is emitted at most once.
    for roots in [true, false] {
        for root in 0..rows.len() {
            if seen[root]
                || roots
                    && parent(rows[root]).is_some_and(|p| {
                        rows.iter()
                            .any(|r| id(*r, "id").is_some_and(|v| v.chars().eq(p.chars())))
                    })
            {
                continue;
            }
            stack.push((root, 0)).map_err(|_| fmt::Error)?;
            while let Some((n, depth)) = stack.pop() {
                if seen[n] {
                    continue;
                }
                seen[n] = true;
                let row = rows[n];
                let node = id(row, "id").ok_or(fmt::Error)?;
                for _ in 0..depth {
                    out.write_str("  ")?;
                }
                out.write_str("├─ ")?;
                display(out, row, &["name", "model", "id"], demo)?;
                out.write_str(" · ")?;
                display(out, row, &["ipAddress"], demo)?;
                out.write_str(" · ")?;
                display(out, row, &["state"], false)?;
                out.write_char('\n')?;
                for client in clients.items().map_err(|_| fmt::Error)? {
                    if id(client, "uplinkDeviceId").is_some_and(|v| v.chars().eq(node.chars())) {
                        client_line(out, client, depth + 1, demo, networks)?;
                    }
                }
                if depth >= 63 {
                    continue;
                }
                for child in (0..rows.len()).rev() {
                    if !seen[child]
                        && parent(rows[child]).is_some_and(|p| p.chars().eq(node.chars()))
                    {
                        stack.push((child, depth + 1)).map_err(|_| fmt::Error)?;
                    }
                }
            }
        }
    }
    let mut unlinked = false;
    for client in clients.items().map_err(|_| fmt::Error)? {
        if id(client, "uplinkDeviceId").is_none_or(|u| {
            !rows
                .iter()
                .any(|r| id(*r, "id").is_some_and(|v| v.chars().eq(u.chars())))
        }) {
            if !unlinked {
                out.write_str("└─ no known uplink\n")?;
                unlinked = true;
            }
            client_line(out, client, 1, demo, networks)?;
        }
    }
    Ok(())
}
