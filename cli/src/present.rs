use crate::{Options, Output, output, payload};
use unifi::Json;

pub const VPN_KEYS: &[&str] = &[
    "teleport",
    "magic_site_to_site_vpn",
    "openvpn",
    "peer_to_peer",
];
pub fn vpn_key(value: &str) -> Result<&str, &'static str> {
    let key = match value {
        "magic-site-to-site-vpn" => "magic_site_to_site_vpn",
        "peer-to-peer" => "peer_to_peer",
        other => other,
    };
    VPN_KEYS
        .contains(&key)
        .then_some(key)
        .ok_or("unknown VPN setting section")
}
fn text(value: Json<'_>, key: &str, expected: &str) -> bool {
    value
        .get(key)
        .ok()
        .flatten()
        .and_then(|v| v.string().ok())
        .is_some_and(|v| v.equals(expected))
}
pub fn show(
    json: Json<'_>,
    selector: Option<(&str, &str)>,
    args: &[&str],
    options: &Options<'_>,
    filtered: &mut [u8],
    sanitized: &mut [u8],
) -> Result<i32, &'static str> {
    let mut json = json;
    let compatibility_page =
        matches!(args, ["network", _, "list", ..]) && !options.hide_page && !options.id_only;
    let page = compatibility_page
        .then_some(json)
        .filter(|v| v.get("data").ok().flatten().is_some());
    let keep_page = page.is_some()
        && options.format.is_some_and(|f| f != output::Format::Table)
        && options.local_filter.is_none()
        && options.local_limit.is_none()
        && options.local_offset == 0;
    if !keep_page && args.first() != Some(&"call") && args.first() != Some(&"api") {
        for field in ["data", "connections"] {
            if let Some(list) = json.get(field).ok().flatten().filter(|v| v.items().is_ok()) {
                json = list;
                break;
            }
        }
    }
    let local_filter = options
        .local_filter
        .map(crate::filter::Expression::parse)
        .transpose()?;
    let vpn_list = args.first() == Some(&"vpn") && matches!(args.get(2), Some(&"list" | &"get"));
    let needs_filter = selector.is_some_and(|(key, _)| key == "search")
        || args.starts_with(&["clients", "reservations"])
        || vpn_list
        || options.unarchived
        || options.local_filter.is_some()
        || options.local_limit.is_some()
        || options.local_offset != 0
        || args.starts_with(&["firewall", "groups", "list"]) && options.kind.is_some();
    if needs_filter {
        let mut matched = 0usize;
        json = payload::filter(
            json,
            |v| {
                let mut keep = true;
                if let Some(("search", query)) = selector {
                    keep &= payload::contains(v, query);
                }
                if args.starts_with(&["clients", "reservations"]) {
                    keep &= v
                        .get("use_fixedip")
                        .ok()
                        .flatten()
                        .and_then(|v| v.boolean().ok())
                        == Some(true);
                }
                if options.unarchived {
                    keep &= v
                        .get("archived")
                        .ok()
                        .flatten()
                        .and_then(|v| v.boolean().ok())
                        != Some(true);
                }
                if args.starts_with(&["firewall", "groups", "list"])
                    && let Some(kind) = options.kind
                {
                    keep &= text(v, "group_type", kind);
                }
                if vpn_list && let Some(family) = args.get(1) {
                    if *family == "settings" {
                        keep &= VPN_KEYS.iter().any(|key| text(v, "key", key));
                    } else if ["site-to-site", "remote-access", "clients"].contains(family) {
                        keep &= crate::vpn::family_matches(v, family);
                    }
                }
                if let Some(filter) = &local_filter {
                    keep &= filter.matches(v);
                }
                if !keep {
                    return false;
                }
                let index = matched;
                matched = matched.saturating_add(1);
                index >= options.local_offset
                    && (options.all
                        || options
                            .local_limit
                            .is_none_or(|limit| index - options.local_offset < limit))
            },
            filtered,
        )?;
    }
    if let Some((field, wanted)) = selector.filter(|(field, _)| *field != "search") {
        json = payload::select(json, field, wanted)?;
    }
    if args.starts_with(&["dpi", "status"]) {
        json = payload::select(json, "key", "dpi")?
            .required("enabled")
            .map_err(|_| "DPI setting has no enabled flag")?;
    }
    if args.starts_with(&["vpn", "health"]) {
        json = payload::select(json, "subsystem", "vpn")?;
    }
    json = payload::sanitize(json, options.demo, options.with_secrets, sanitized)?;
    let mut ids = [0; 131072];
    if options.id_only {
        use core::fmt::Write;
        let mut out = crate::buffer::Buffer::new(&mut ids);
        out.write_char('[').map_err(|_| "ID output capacity")?;
        for (i, item) in json
            .items()
            .map_err(|_| "--id-only requires a list")?
            .enumerate()
        {
            if i > 0 {
                out.write_char(',').map_err(|_| "ID output capacity")?;
            }
            let value = ["id", "_id", "siteId", "hostId"]
                .iter()
                .find_map(|key| item.get(key).ok().flatten().and_then(|v| v.string().ok()))
                .ok_or("list item has no ID")?;
            crate::output::quoted(&mut out, value).map_err(|_| "ID output capacity")?;
        }
        out.write_char(']').map_err(|_| "ID output capacity")?;
        json = Json::parse(out.finish()).unwrap();
    }
    if let Some(file) = options.save {
        crate::save_file(file, json.raw().as_bytes())?;
        return Ok(0);
    }
    if options.quiet {
        return Ok(0);
    }
    let default = if args.first() == Some(&"call") || args.first() == Some(&"api") {
        output::Format::Compact
    } else if options.id_only {
        output::Format::Plain
    } else if args.starts_with(&["settings", "export"]) {
        output::Format::Json
    } else {
        output::Format::Table
    };
    let format = options.format.unwrap_or(default);
    if format == output::Format::Table && crate::color::enabled(options) {
        render(
            &mut crate::color::Header {
                out: &mut Output(false),
                accent: crate::color::accent(options.theme),
                first: true,
            },
            json,
            format,
            args,
        )
        .map_err(|_| "output failed")?;
    } else {
        render(&mut Output(false), json, format, args).map_err(|_| "output failed")?;
    }
    if format == output::Format::Table
        && let Some(page) = page
    {
        let count = page
            .get("count")
            .ok()
            .flatten()
            .and_then(|v| v.integer().ok());
        let total = page
            .get("totalCount")
            .ok()
            .flatten()
            .and_then(|v| v.integer().ok());
        if let (Some(count), Some(total)) = (count, total) {
            use core::fmt::Write;
            writeln!(Output(false), "{count} of {total} items").map_err(|_| "output failed")?;
        }
    }

    Ok(0)
}

fn render(
    out: &mut dyn core::fmt::Write,
    json: Json<'_>,
    format: output::Format,
    args: &[&str],
) -> core::fmt::Result {
    let args = args.strip_prefix(&["network"]).unwrap_or(args);
    if format != output::Format::Table {
        return output::render(out, json, format);
    }
    let fields: &[&str] = match args {
        ["devices", "ports", ..] => &["index", "name", "up", "connected", "speed", "poe_mode"],
        ["devices", ..] => &["name", "state", "model", "ipAddress", "id"],
        ["clients", ..] => &["@clientName", "ipAddress", "type", "fixed_ip", "id", "mac"],
        ["wifi", ..] => &[
            "name",
            "enabled",
            "broadcastingFrequenciesGHz",
            "securityConfiguration.type",
            "ssid",
            "signal",
            "channel",
            "id",
        ],
        ["networks", ..] => &["name", "vlanId", "management", "enabled", "id"],
        ["countries", ..] => &["code", "name"],
        ["nat", ..] => &["description", "type", "enabled", "rule_index", "id"],
        ["vpn", ..] => &["name", "purpose", "vpn_type", "enabled", "id", "_id"],
        ["dns", ..] => &["domain", "type", "@dnsValue", "ttlSeconds", "id"],
        ["cloud", "sites", ..] => &["meta.name", "siteId", "hostId", "permission"],
        ["cloud", "hosts", ..] => &["reportedState.name", "reportedState.state", "type", "id"],
        _ => return output::render(out, json, format),
    };
    output::table_columns(out, json, fields)
}
