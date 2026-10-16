//! Firmware-compatible DPI reads. POST queries retain the CLI's explicit opt-in.
use crate::{
    flows::{self, Remote},
    request,
};
use unifi::{Json, session_api};

pub fn dpi<'a>(
    remote: &mut Remote<'_, '_>,
    site: &str,
    body: &[u8],
    out: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let input = Json::parse(body).map_err(|_| "invalid DPI request JSON")?;
    let group = input
        .required("type")
        .and_then(Json::string)
        .and_then(|v| v.decode::<32>())
        .map_err(|_| "DPI query needs a group type")?;
    if !["by_app", "by_cat"].contains(&group.as_str()) {
        return Err("DPI type must be by_app or by_cat");
    }
    if let Some(macs) = input.get("macs").map_err(|_| "invalid DPI MAC filter")? {
        let items = macs.items().map_err(|_| "DPI macs must be an array")?;
        if items.clone().next().is_none() {
            return Err("DPI macs cannot be empty");
        }
        for mac in items {
            let mac = mac
                .string()
                .and_then(|v| v.decode::<64>())
                .map_err(|_| "invalid DPI MAC address")?;
            crate::lookup::mac(&mac)?;
        }
        return flows::copy(
            remote.execute(&session_api::get_station_dpi(site).unwrap(), Some(body))?,
            out,
        );
    }
    let current = session_api::get_traffic_flow_statistics(site).unwrap();
    if group == "by_app"
        && let Ok(reply) = request(
            remote.url,
            remote.base,
            remote.key,
            &current,
            None,
            remote.options,
            remote.response,
        )
        && (200..300).contains(&reply.status)
        && let Ok(json) = reply.json()
        && json
            .get("top_all_traffic_by_application")
            .ok()
            .flatten()
            .and_then(|v| v.items().ok())
            .is_some_and(|mut v| v.next().is_some())
    {
        let bytes = json.raw().as_bytes();
        if bytes.len() + 2 > out.len() {
            return Err("DPI response exceeds capacity");
        }
        out[0] = b'[';
        out[1..bytes.len() + 1].copy_from_slice(bytes);
        out[bytes.len() + 1] = b']';
        return Ok(Json::parse(&out[..bytes.len() + 2]).unwrap());
    }
    let rows = remote.execute(&session_api::get_site_dpi(site).unwrap(), Some(body))?;
    let items = rows.items().map_err(|_| "invalid site DPI response")?;
    if items.clone().next().is_some()
        && items
            .into_iter()
            .any(|v| v.get(&group).ok().flatten().is_some())
    {
        return flows::copy(rows, out);
    }
    flows::copy(
        remote.execute(&session_api::get_dpi(site).unwrap(), None)?,
        out,
    )
}
