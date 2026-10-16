//! Community session workflows, preserving unknown fields and existing secrets.
use crate::{
    buffer::Buffer,
    flows::{self, Remote},
    payload, settings,
};
use core::fmt::Write;
use unifi::{Call, Json, Method, json::ObjectWriter, session_api};

fn required_string<'a>(
    value: Json<'a>,
    field: &str,
) -> Result<unifi::json::JsonString<'a>, &'static str> {
    let text = value
        .required(field)
        .and_then(Json::string)
        .map_err(|_| "VPN body is missing a required string field")?;
    if text.chars().next().is_none() {
        return Err("VPN required fields cannot be empty");
    }
    Ok(text)
}
fn merge<'a>(old: Json<'_>, input: Json<'_>, out: &'a mut [u8]) -> Result<Json<'a>, &'static str> {
    payload::merge_objects(old, Some(input), &["id", "_id", "site_id"], out)
}
fn changes(input: Json<'_>) -> Result<(), &'static str> {
    if !input
        .fields()
        .map_err(|_| "VPN request must be an object")?
        .any(|(k, _)| !k.equals("_id"))
    {
        return Err("no VPN changes supplied");
    }
    Ok(())
}
pub fn family_matches(record: Json<'_>, family: &str) -> bool {
    let purpose = match family {
        "site-to-site" => "site-vpn",
        "remote-access" => "remote-user-vpn",
        "clients" => "vpn-client",
        _ => return false,
    };
    match record.get("purpose").ok().flatten() {
        Some(value) => value.string().is_ok_and(|v| v.equals(purpose)),
        None => {
            family == "clients"
                && record
                    .get("vpn_type")
                    .ok()
                    .flatten()
                    .and_then(|v| v.string().ok())
                    .is_some_and(|v| v.as_str().is_some_and(|v| v.ends_with("-client")))
        }
    }
}
pub fn validate_delete(remote: &mut Remote<'_, '_>, args: &[&str]) -> Result<(), &'static str> {
    let rows = remote.execute(
        &session_api::list_network_conf(remote.options.site.unwrap_or("default")).unwrap(),
        None,
    )?;
    let record = payload::select(rows, "id", args.last().ok_or("VPN ID missing")?)?;
    if !family_matches(record, args[1]) {
        return Err("VPN belongs to a different family");
    }
    Ok(())
}
pub fn prepare<'a>(
    remote: &mut Remote<'_, '_>,
    call: &Call,
    args: &[&str],
    incoming: Option<&[u8]>,
    out: &'a mut [u8],
) -> Result<&'a [u8], &'static str> {
    let input = Json::parse(incoming.ok_or("VPN command needs --from-file FILE or --data JSON")?)
        .map_err(|_| "invalid VPN JSON")?;
    input
        .object()
        .map_err(|_| "VPN convenience commands accept one object; use call for batch bodies")?;
    let site = remote.options.site.unwrap_or("default");
    if args.get(1) != Some(&"peers") {
        changes(input)?;
        let purpose = match args.get(1) {
            Some(&"site-to-site") => "site-vpn",
            Some(&"remote-access") => "remote-user-vpn",
            Some(&"clients") => "vpn-client",
            _ => return Err("unknown VPN family"),
        };
        let mut current = [0; 131072];
        let old = if call.operation.method == Method::PUT {
            let records = remote.execute(&session_api::list_network_conf(site).unwrap(), None)?;
            let record = payload::select(records, "id", args.last().ok_or("VPN ID missing")?)?;
            if !family_matches(record, args[1]) {
                return Err("VPN belongs to a different family");
            }
            flows::copy(record, &mut current)?
        } else {
            Json::parse(br#"{"enabled":true}"#).unwrap()
        };
        let mut combined = [0; 131072];
        let combined = merge(old, input, &mut combined)?;
        required_string(combined, "name")?;
        required_string(combined, "vpn_type")?;
        if let Some(actual) = combined.get("purpose").map_err(|_| "invalid VPN purpose")?
            && !actual.string().is_ok_and(|v| v.equals(purpose))
        {
            return Err("VPN purpose does not match command family");
        }
        if let Some(enabled) = combined.get("enabled").map_err(|_| "invalid VPN body")? {
            enabled
                .boolean()
                .map_err(|_| "VPN enabled must be boolean")?;
        }
        let mut patch = [0; 128];
        let mut w = ObjectWriter::new(&mut patch).unwrap();
        w.field("purpose", &purpose).unwrap();
        let body = payload::merge_objects(
            combined,
            Some(Json::parse(w.finish().unwrap()).unwrap()),
            &[],
            out,
        )?;
        if !remote.options.allow_secret_removal {
            settings::check_credentials(old, Some(body), false)?;
        }
        return Ok(body.raw().as_bytes());
    }
    if call.operation.id == "deleteWireguardPeers" {
        let id = required_string(input, "_id")?;
        let mut writer = Buffer::new(out);
        writer.write_char('[').map_err(|_| "VPN body capacity")?;
        crate::output::quoted(&mut writer, id).map_err(|_| "VPN body capacity")?;
        writer.write_char(']').map_err(|_| "VPN body capacity")?;
        return Ok(writer.finish());
    }
    changes(input)?;
    let server = *args.get(3).ok_or("WireGuard server ID missing")?;
    let mut current = [0; 131072];
    let old = if call.operation.id == "updateWireguardPeers" {
        // Read by command selectors; the requested identity is inserted below.
        let peer = *args.get(4).ok_or("WireGuard peer ID missing")?;
        let records = remote.execute(
            &session_api::list_wireguard_peers(site, server)
                .unwrap()
                .query("networkId", server)
                .unwrap(),
            None,
        )?;
        flows::copy(payload::select(records, "id", peer)?, &mut current)?
    } else {
        Json::parse(br#"{"preshared_key":"","allowed_ips":[]}"#).unwrap()
    };
    let mut combined = [0; 131072];
    let body = merge(old, input, &mut combined)?;
    for field in ["name", "public_key", "interface_ip"] {
        required_string(body, field)?;
    }
    for field in ["allowed_ips"] {
        if let Some(value) = body.get(field).map_err(|_| "invalid WireGuard body")? {
            for item in value
                .items()
                .map_err(|_| "WireGuard allowed_ips must be an array")?
            {
                item.string()
                    .map_err(|_| "WireGuard allowed_ips entries must be strings")?;
            }
        }
    }
    if !remote.options.allow_secret_removal {
        settings::check_credentials(old, Some(body), false)?;
    }
    let mut peer = [0; 131072];
    let mut writer = ObjectWriter::new(&mut peer).unwrap();
    for (name, value) in body.fields().unwrap() {
        let name: heapless::String<512> =
            name.decode().map_err(|_| "WireGuard field name capacity")?;
        writer
            .raw_field(&name, value)
            .map_err(|_| "WireGuard body capacity")?;
    }
    if call.operation.id == "updateWireguardPeers" {
        writer
            .field("_id", args.get(4).ok_or("WireGuard peer ID missing")?)
            .map_err(|_| "WireGuard peer ID capacity")?;
    }
    let bytes = writer.finish().map_err(|_| "WireGuard body capacity")?;
    if bytes.len() + 2 > out.len() {
        return Err("WireGuard batch exceeds capacity");
    }
    out[0] = b'[';
    out[1..bytes.len() + 1].copy_from_slice(bytes);
    out[bytes.len() + 1] = b']';
    Ok(&out[..bytes.len() + 2])
}
