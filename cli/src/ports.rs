//! Read/merge/write port overrides, preserving fields and ports outside the patch.
use crate::{
    buffer::Buffer,
    flows::{self, Kind, Remote},
    jsonc, payload, read_file,
};
use core::fmt::Write;
use heapless::{String, Vec};
use unifi::{Json, json::ObjectWriter, session_api};

fn number(value: Json<'_>, field: &str) -> Option<u64> {
    value
        .get(field)
        .ok()
        .flatten()
        .and_then(|v| v.unsigned().ok())
}
fn text<'a>(value: Json<'a>, field: &str) -> Result<Option<String<512>>, &'static str> {
    value
        .optional(field)
        .map_err(|_| "invalid port field")?
        .map(|v| {
            v.string()
                .and_then(|s| s.decode())
                .map_err(|_| "invalid port string")
        })
        .transpose()
}
fn flag(value: Json<'_>, field: &str) -> Result<Option<bool>, &'static str> {
    value
        .optional(field)
        .map_err(|_| "invalid port flag")?
        .map(|v| v.boolean().map_err(|_| "invalid port flag"))
        .transpose()
}
fn index(value: Json<'_>) -> Result<u64, &'static str> {
    number(value, "index")
        .or_else(|| number(value, "port_idx"))
        .filter(|n| (1..=u32::MAX as u64).contains(n))
        .ok_or("port index must be a positive 32-bit integer")
}
fn find(list: Json<'_>, wanted: u64) -> Option<Json<'_>> {
    list.items().ok()?.find(|v| index(*v).ok() == Some(wanted))
}
fn array<'a>(value: Json<'a>, key: &str) -> Result<Json<'a>, &'static str> {
    let value = value
        .optional(key)
        .map_err(|_| "invalid device field")?
        .unwrap_or(Json::parse(b"[]").unwrap());
    value.items().map_err(|_| "invalid device port array")?;
    Ok(value)
}
fn append_summary(summary: &mut heapless::String<512>, records: Json<'_>) {
    for record in records.items().unwrap() {
        let name = ["name", "hostname", "model", "mac", "_id"]
            .iter()
            .find_map(|field| text(record, field).ok().flatten());
        if let Some(name) = name {
            if !summary.is_empty() && summary.push_str(", ").is_err() {
                break;
            }
            if summary.push_str(&name).is_err() {
                let _ = summary.push('…');
                break;
            }
        }
    }
}
fn linked_devices<'a>(
    records: Json<'_>,
    parent: &str,
    out: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let parent = crate::lookup::mac(parent)?;
    let mut out = Buffer::new(out);
    out.write_char('[').unwrap();
    let mut first = true;
    for row in records.items().map_err(|_| "invalid device list")? {
        let Some(uplink) = row.get("uplink").ok().flatten() else {
            continue;
        };
        if text(uplink, "uplink_mac")
            .ok()
            .flatten()
            .and_then(|v| crate::lookup::mac(&v).ok())
            .as_ref()
            != Some(&parent)
        {
            continue;
        }
        let Some(port) = number(uplink, "uplink_remote_port") else {
            continue;
        };
        let mut bytes = [0; 4096];
        let mut w = ObjectWriter::new(&mut bytes).unwrap();
        w.field("port_idx", &port).unwrap();
        for field in ["_id", "name", "mac", "model", "ip", "state"] {
            if let Some(value) = row.get(field).ok().flatten() {
                w.raw_field(field, value)
                    .map_err(|_| "linked device exceeds capacity")?;
            }
        }
        if !first {
            out.write_char(',')
                .map_err(|_| "linked devices exceed capacity")?;
        }
        first = false;
        out.write_str(
            core::str::from_utf8(w.finish().map_err(|_| "linked device exceeds capacity")?)
                .unwrap(),
        )
        .map_err(|_| "linked devices exceed capacity")?;
    }
    out.write_char(']')
        .map_err(|_| "linked devices exceed capacity")?;
    Ok(Json::parse(out.finish()).unwrap())
}
fn port_patch<'a>(
    entry: Json<'_>,
    existing: Json<'_>,
    networks: Json<'_>,
    out: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    for (key, _) in entry.fields().map_err(|_| "port patch must be an object")? {
        let key: String<128> = key.decode().map_err(|_| "port key too long")?;
        if ![
            "index",
            "reset",
            "name",
            "mode",
            "native_network_id",
            "tagged_network_ids",
            "tagged_all",
            "poe",
            "speed",
            "last_seen",
        ]
        .contains(&key.as_str())
        {
            return Err("unknown port patch field");
        }
    }
    let mode = text(entry, "mode")?;
    if mode
        .as_deref()
        .is_some_and(|v| !["access", "trunk", "mirror"].contains(&v))
    {
        return Err("mode must be access, trunk or mirror");
    }
    let tagged = entry
        .optional("tagged_network_ids")
        .map_err(|_| "invalid tagged networks")?;
    let all = flag(entry, "tagged_all")?.unwrap_or(false);
    if all
        && (mode.as_deref().is_some_and(|v| v != "trunk")
            || tagged.is_some_and(|v| v.items().is_ok_and(|mut v| v.next().is_some())))
    {
        return Err("tagged_all conflicts with port mode or tagged networks");
    }
    let mut tags = Vec::<String<512>, 64>::new();
    if let Some(tagged) = tagged {
        for item in tagged
            .items()
            .map_err(|_| "tagged_network_ids must be an array")?
        {
            let name: String<512> = item
                .string()
                .and_then(|v| v.decode())
                .map_err(|_| "invalid tagged network")?;
            tags.push(flows::network_id(networks, &name)?)
                .map_err(|_| "too many tagged networks")?;
        }
    }
    if mode
        .as_deref()
        .is_some_and(|m| m == "access" || m == "mirror")
        && !tags.is_empty()
    {
        return Err("access/mirror mode cannot carry tagged networks");
    }
    let mut patch = [0; 65536];
    let mut writer = ObjectWriter::new(&mut patch).unwrap();
    writer
        .field("port_idx", &index(entry)?)
        .map_err(|_| "port patch capacity")?;
    if let Some(name) = text(entry, "name")? {
        writer
            .field("name", &name.as_str())
            .map_err(|_| "port patch capacity")?;
    }
    let mode = if all { Some("trunk") } else { mode.as_deref() };
    if let Some(mode) = mode {
        writer
            .field(
                "op_mode",
                &if mode == "mirror" { "mirror" } else { "switch" },
            )
            .unwrap();
        if mode == "access" || mode == "mirror" {
            writer.field("tagged_vlan_mgmt", &"block_all").unwrap();
            writer
                .raw_field("tagged_networkconf_ids", Json::parse(b"[]").unwrap())
                .unwrap();
        } else if mode == "trunk" {
            writer
                .field(
                    "tagged_vlan_mgmt",
                    &if tagged.is_some() && !all {
                        "custom"
                    } else {
                        "auto"
                    },
                )
                .unwrap();
        }
    }
    if tagged.is_some() && !all && !matches!(mode, Some("access" | "mirror")) {
        writer
            .field("tagged_networkconf_ids", &tags)
            .map_err(|_| "port patch capacity")?;
        if mode.is_none() {
            writer.field("tagged_vlan_mgmt", &"custom").unwrap();
        }
    }
    if let Some(native) = text(entry, "native_network_id")? {
        writer
            .field(
                "native_networkconf_id",
                &flows::network_id(networks, &native)?.as_str(),
            )
            .map_err(|_| "port patch capacity")?;
    }
    if let Some(poe) = text(entry, "poe")? {
        let poe = match poe.as_str() {
            "on" | "auto" => "auto",
            "off" => "off",
            "pasv24" => "pasv24",
            "passthrough" => "passthrough",
            _ => return Err("invalid PoE mode"),
        };
        writer.field("poe_mode", &poe).unwrap();
    }
    let speed = text(entry, "speed")?;
    if let Some(speed) = &speed {
        if !["auto", "10", "100", "1000", "2500", "5000", "10000"].contains(&speed.as_str()) {
            return Err("invalid port speed");
        }
        writer.field("autoneg", &(speed == "auto")).unwrap();
        if speed != "auto" {
            writer.field("speed", &speed.as_str()).unwrap();
        }
    }
    let patch = Json::parse(writer.finish().map_err(|_| "port patch capacity")?).unwrap();
    payload::merge_objects(
        existing,
        Some(patch),
        if speed.as_deref() == Some("auto") {
            &["speed"]
        } else {
            &[]
        },
        out,
    )
}
fn export_entry<'a>(row: Json<'_>, out: &'a mut [u8]) -> Result<Json<'a>, &'static str> {
    let mut w = ObjectWriter::new(out).unwrap();
    w.field("index", &index(row)?).unwrap();
    for (source, target) in [
        ("name", "name"),
        ("native_networkconf_id", "native_network_id"),
        ("poe_mode", "poe"),
    ] {
        if let Some(value) = row.optional(source).map_err(|_| "invalid port override")? {
            w.raw_field(target, value).map_err(|_| "export capacity")?;
        }
    }
    let op = text(row, "op_mode")?;
    let mgmt = text(row, "tagged_vlan_mgmt")?;
    let mode = if op.as_deref() == Some("mirror") {
        Some("mirror")
    } else {
        match mgmt.as_deref() {
            Some("block_all") => Some("access"),
            Some("auto" | "custom") => Some("trunk"),
            _ => None,
        }
    };
    if let Some(mode) = mode {
        w.field("mode", &mode).unwrap();
    }
    if mgmt.as_deref() == Some("auto") {
        w.field("tagged_all", &true).unwrap();
    } else if mgmt.as_deref() == Some("custom") {
        w.raw_field("tagged_network_ids", array(row, "tagged_networkconf_ids")?)
            .map_err(|_| "export capacity")?;
    }
    if flag(row, "autoneg")? == Some(true) {
        w.field("speed", &"auto").unwrap();
    } else if let Some(speed) = text(row, "speed")? {
        w.field("speed", &speed.as_str()).unwrap();
    }
    Json::parse(w.finish().map_err(|_| "export capacity")?).map_err(|_| "invalid port export")
}

#[allow(clippy::too_many_arguments)]
pub fn run<'a>(
    remote: &mut Remote<'_, '_>,
    kind: Kind,
    args: &[&str],
    site: &str,
    fields: &[(&str, &str)],
    output: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let options = *remote.options;
    if args.len() < 3 || args.len() > if kind == Kind::PortSet { 4 } else { 3 } {
        return Err(
            "usage: devices ports|ports-export DEVICE; devices port-set DEVICE [PORT] [--from-file FILE]",
        );
    }
    let mut device_buffer = [0; 131072];
    let records = remote.execute(&session_api::list_devices(site).unwrap(), None)?;
    let device = flows::copy(payload::select(records, "id", args[2])?, &mut device_buffer)?;
    let mut linked_buffer = [0; 131072];
    let linked = if options.with_clients {
        Some(linked_devices(
            records,
            &flows::owned::<512>(device, "mac")?,
            &mut linked_buffer,
        )?)
    } else {
        None
    };
    let overrides = array(device, "port_overrides")?;
    let table = array(device, "port_table")?;
    let id: String<512> = flows::owned(device, "_id")?;
    if kind == Kind::PortSet {
        let mut raw = [0; 131072];
        let mut parsed = [0; 131072];
        let mut single = [0; 16384];
        let entries = if let Some(file) = options.body_file {
            if args.len() != 3
                || options.reset
                || options.port_mode.is_some()
                || options.native_vlan.is_some()
                || options.tagged_vlans.is_some()
                || options.poe.is_some()
                || options.speed.is_some()
                || !fields.is_empty()
            {
                return Err("--from-file cannot be combined with a port or port flags");
            }
            jsonc::parse(read_file(file, &mut raw)?, &mut parsed)?
                .required("ports")
                .map_err(|_| "port file needs a ports array")?
        } else {
            if args.len() != 4 {
                return Err("port-set needs PORT or --from-file FILE");
            }
            let idx = args[3]
                .parse::<u32>()
                .ok()
                .filter(|v| *v > 0)
                .ok_or("invalid port index")?;
            let mut entry = [0; 8192];
            let mut w = ObjectWriter::new(&mut entry).unwrap();
            w.field("index", &idx).unwrap();
            if options.reset {
                w.field("reset", &true).unwrap();
            }
            for (name, value) in [
                ("mode", options.port_mode),
                ("native_network_id", options.native_vlan),
                ("poe", options.poe),
                ("speed", options.speed),
            ] {
                if let Some(value) = value {
                    w.field(name, &value).map_err(|_| "port field capacity")?;
                }
            }
            if let Some(tags) = options.tagged_vlans {
                let mut list = Vec::<&str, 64>::new();
                for tag in tags.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    list.push(tag).map_err(|_| "too many tagged networks")?;
                }
                w.field("tagged_network_ids", &list)
                    .map_err(|_| "port field capacity")?;
            }
            for (key, value) in fields {
                if *key != "name" {
                    return Err("unsupported port body flag");
                }
                w.field("name", value).map_err(|_| "port field capacity")?;
            }
            let bytes = w.finish().map_err(|_| "port field capacity")?;
            let mut array = Buffer::new(&mut single);
            write!(array, "[{}]", core::str::from_utf8(bytes).unwrap())
                .map_err(|_| "port field capacity")?;
            Json::parse(array.finish()).unwrap()
        };
        let mut selected = Vec::<Json<'_>, 1024>::new();
        for entry in entries.items().map_err(|_| "ports must be an array")? {
            selected.push(entry).map_err(|_| "too many ports")?;
        }
        let entries = selected;
        if entries.is_empty() {
            return Err("ports array is empty");
        }
        for (i, entry) in entries.iter().enumerate() {
            let idx = index(*entry)?;
            if entries[..i].iter().any(|v| index(*v).ok() == Some(idx)) {
                return Err("duplicate port in update");
            }
            if table.items().unwrap().next().is_some() && find(table, idx).is_none() {
                return Err("port does not exist on this device");
            }
            if flag(*entry, "reset")? == Some(true)
                && entry.fields().unwrap().any(|(key, _)| {
                    !key.equals("index") && !key.equals("reset") && !key.equals("last_seen")
                })
            {
                return Err("reset cannot include port changes");
            }
        }
        let mut network_buffer = [0; 131072];
        let networks = flows::copy(
            remote.execute(&session_api::list_network_conf(site).unwrap(), None)?,
            &mut network_buffer,
        )?;
        let mut out = Buffer::new(output);
        out.write_str("{\"port_overrides\":[")
            .map_err(|_| "port update capacity")?;
        let mut first = true;
        let mut changed = false;
        for existing in overrides.items().unwrap() {
            if entries
                .iter()
                .any(|e| index(*e).ok() == index(existing).ok())
            {
                continue;
            }
            if !first {
                out.write_char(',').map_err(|_| "port update capacity")?;
            }
            first = false;
            out.write_str(existing.raw())
                .map_err(|_| "port update capacity")?;
        }
        let mut patch_buffer = [0; 131072];
        for entry in entries {
            let existing = find(overrides, index(entry)?);
            if flag(entry, "reset")? == Some(true) {
                changed |= existing.is_some();
                continue;
            }
            let noop = entry
                .fields()
                .unwrap()
                .all(|(k, _)| k.equals("index") || k.equals("reset") || k.equals("last_seen"));
            let next = if noop {
                existing
            } else {
                changed = true;
                Some(port_patch(
                    entry,
                    existing.unwrap_or(Json::parse(b"{}").unwrap()),
                    networks,
                    &mut patch_buffer,
                )?)
            };
            if let Some(next) = next {
                if !first {
                    out.write_char(',').map_err(|_| "port update capacity")?;
                }
                first = false;
                out.write_str(next.raw())
                    .map_err(|_| "port update capacity")?;
            }
        }
        out.write_str("]}").map_err(|_| "port update capacity")?;
        let body = out.finish();
        if changed {
            remote.execute(&session_api::update_device(site, &id).unwrap(), Some(body))?;
        }
        return Ok(Json::parse(body).unwrap());
    }
    let mut clients_buffer = [0; 131072];
    let clients = if options.with_clients {
        Some(flows::copy(
            remote.execute(&session_api::list_clients(site).unwrap(), None)?,
            &mut clients_buffer,
        )?)
    } else {
        None
    };
    let mac = crate::lookup::mac(&flows::owned::<512>(device, "mac")?)?;
    let source =
        if kind == Kind::PortsExport && !options.all || table.items().unwrap().next().is_none() {
            overrides
        } else {
            table
        };
    let mut rows = Vec::<Json<'_>, 1024>::new();
    for row in source.items().unwrap() {
        index(row)?;
        rows.push(row).map_err(|_| "too many ports")?;
    }
    rows.sort_unstable_by_key(|r| index(*r).unwrap());
    let mut out = Buffer::new(output);
    out.write_str(if kind == Kind::PortsExport {
        "{\"ports\":["
    } else {
        "["
    })
    .map_err(|_| "port output capacity")?;
    let mut merge_buffer = [0; 65536];
    let mut entry_buffer = [0; 65536];
    let mut client_buffer = [0; 131072];
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.write_char(',').map_err(|_| "port output capacity")?;
        }
        let idx = index(*row)?;
        let override_row = find(overrides, idx);
        let result = if kind == Kind::PortsExport {
            export_entry(override_row.unwrap_or(*row), &mut entry_buffer)?
        } else {
            let merged = payload::merge_objects(*row, override_row, &[], &mut merge_buffer)?;
            let mut w = ObjectWriter::new(&mut entry_buffer).unwrap();
            w.field("index", &idx).unwrap();
            for (name, value) in merged.fields().unwrap() {
                if name.equals("index")
                    || options.with_clients && (name.equals("clients") || name.equals("devices"))
                {
                    continue;
                }
                let name: String<512> = name.decode().map_err(|_| "port key capacity")?;
                w.raw_field(&name, value)
                    .map_err(|_| "port output capacity")?;
            }
            Json::parse(w.finish().map_err(|_| "port output capacity")?).unwrap()
        };
        if let Some(clients) = clients {
            let mut metadata = ObjectWriter::new(&mut merge_buffer).unwrap();
            let mut summary = heapless::String::<512>::new();
            let attached = payload::filter(
                clients,
                |c| {
                    ["sw_mac", "ap_mac"].iter().any(|key| {
                        text(c, key)
                            .ok()
                            .flatten()
                            .and_then(|v| crate::lookup::mac(&v).ok())
                            .as_ref()
                            == Some(&mac)
                    }) && (number(c, "sw_port") == Some(idx) || number(c, "port_idx") == Some(idx))
                },
                &mut client_buffer,
            )?;
            append_summary(&mut summary, attached);
            metadata
                .raw_field("clients", attached)
                .map_err(|_| "port annotations exceed capacity")?;
            let devices = payload::filter(
                linked.unwrap(),
                |v| number(v, "port_idx") == Some(idx),
                &mut client_buffer,
            )?;
            metadata
                .raw_field("devices", devices)
                .map_err(|_| "port annotations exceed capacity")?;
            append_summary(&mut summary, devices);
            metadata
                .field("connected", &summary.as_str())
                .map_err(|_| "port annotations exceed capacity")?;
            metadata
                .field(
                    "observedAtUnix",
                    &rustix::time::clock_gettime(rustix::time::ClockId::Realtime).tv_sec,
                )
                .map_err(|_| "port annotations exceed capacity")?;
            let metadata = core::str::from_utf8(
                metadata
                    .finish()
                    .map_err(|_| "port annotations exceed capacity")?,
            )
            .unwrap();
            if kind == Kind::PortsExport {
                write!(
                    out,
                    "{},\"last_seen\":{metadata}}}",
                    result.raw().strip_suffix('}').unwrap()
                )
                .map_err(|_| "port output capacity")?;
            } else {
                write!(
                    out,
                    "{},{}",
                    result.raw().strip_suffix('}').unwrap(),
                    metadata.strip_prefix('{').unwrap()
                )
                .map_err(|_| "port output capacity")?;
            }
        } else {
            out.write_str(result.raw())
                .map_err(|_| "port output capacity")?;
        }
    }
    out.write_str(if kind == Kind::PortsExport { "]}" } else { "]" })
        .map_err(|_| "port output capacity")?;
    Ok(Json::parse(out.finish()).unwrap())
}
