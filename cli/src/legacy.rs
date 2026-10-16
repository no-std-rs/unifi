//! Community-derived NAT and firewall group convenience payloads.
use crate::{
    compat::{self, Flags},
    flows::{self, Remote},
    lookup, payload,
};
use heapless::String;
use unifi::{Call, Json, json::ObjectWriter, session_api};
fn text(value: Json<'_>, key: &str) -> Result<Option<String<8192>>, &'static str> {
    value
        .optional(key)
        .map_err(|_| "invalid legacy field")?
        .map(|v| {
            v.string()
                .and_then(|s| s.decode())
                .map_err(|_| "invalid or oversized legacy string")
        })
        .transpose()
}
fn filter<'a>(
    base: Json<'_>,
    address: Option<&str>,
    port: Option<&str>,
    out: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let mut bytes = [0; 16384];
    let mut w = ObjectWriter::new(&mut bytes).unwrap();
    if address == Some("") || port == Some("") {
        return Err("empty NAT filter; use a JSON body to clear fields explicitly");
    }
    let old_address = text(base, "address")?;
    let old_port = text(base, "port")?;
    w.field(
        "filter_type",
        &if address.or(old_address.as_deref()).is_some() || port.or(old_port.as_deref()).is_some() {
            "ADDRESS_AND_PORT"
        } else {
            "NONE"
        },
    )
    .unwrap();
    if let Some(address) = address {
        w.field("address", &address)
            .map_err(|_| "NAT filter capacity")?;
    }
    if let Some(port) = port {
        w.field("port", &port).map_err(|_| "NAT filter capacity")?;
    }
    let patch = Json::parse(w.finish().map_err(|_| "NAT filter capacity")?).unwrap();
    payload::merge_objects(base, Some(patch), &[], out)
}
fn interface(
    remote: &mut Remote<'_, '_>,
    site: &str,
    selector: &str,
) -> Result<String<512>, &'static str> {
    if selector.len() == 24 && selector.bytes().all(|b| b.is_ascii_hexdigit()) {
        return String::try_from(selector).map_err(|_| "interface ID capacity");
    }
    let records = remote.execute(&session_api::list_network_conf(site).unwrap(), None)?;
    if lookup::uuid(selector) {
        for row in records
            .items()
            .map_err(|_| "invalid network configuration list")?
        {
            if row
                .get("external_id")
                .ok()
                .flatten()
                .and_then(|v| v.string().ok())
                .is_some_and(|s| {
                    s.chars()
                        .map(|c| c.to_ascii_lowercase())
                        .eq(selector.chars().map(|c| c.to_ascii_lowercase()))
                })
            {
                return flows::owned(row, "_id");
            }
        }
        Err("NAT interface UUID has no session network mapping")
    } else {
        flows::network_id(records, selector)
    }
}
#[allow(clippy::too_many_arguments)]
pub fn prepare<'a>(
    remote: &mut Remote<'_, '_>,
    call: &Call,
    args: &[&str],
    incoming: Option<&[u8]>,
    flags: &mut Flags<'_>,
    out: &'a mut [u8],
) -> Result<&'a [u8], &'static str> {
    let options = *remote.options;
    let site = options.site.unwrap_or("default");
    let create = matches!(call.operation.id, "createNatRule" | "createFirewallGroup");
    let nat = args.first() == Some(&"nat");
    if !create && incoming.is_none() && !flags.remaining() && options.kind.is_none() {
        return Err("no legacy update changes supplied");
    }
    let mut previous = [0; 131072];
    let mut next_index = 1;
    let current = if create {
        if nat {
            let list = remote.execute(&session_api::list_nat_rules(site).unwrap(), None)?;
            let max = list
                .items()
                .map_err(|_| "invalid NAT rule list")?
                .filter_map(|v| {
                    v.get("rule_index")
                        .ok()
                        .flatten()
                        .and_then(|v| v.unsigned().ok())
                })
                .max()
                .unwrap_or(0);
            next_index = max.checked_add(1).ok_or("NAT rule index overflow")?;
        }
        Json::parse(if nat {br#"{"enabled":true,"type":"DNAT","ip_version":"IPV4","is_predefined":false,"setting_preference":"manual","logging":false,"exclude":false,"pppoe_use_base_interface":false,"source_filter":{"filter_type":"NONE","firewall_group_ids":[],"invert_address":false,"invert_port":false},"destination_filter":{"filter_type":"NONE","firewall_group_ids":[],"invert_address":false,"invert_port":false}}"#} else {br#"{"group_type":"port-group","group_members":[]}"#}).unwrap()
    } else {
        let id = args.get(3).ok_or("legacy update needs an ID")?;
        let call = if nat {
            session_api::list_nat_rules(site)
        } else {
            session_api::list_firewall_groups(site)
        }
        .unwrap();
        flows::copy(
            payload::select(remote.execute(&call, None)?, "id", id)?,
            &mut previous,
        )?
    };
    let incoming = incoming
        .map(Json::parse)
        .transpose()
        .map_err(|_| "invalid legacy request JSON")?;
    let mut merged = [0; 131072];
    let base = payload::merge_objects(current, incoming, &[], &mut merged)?;
    let mut bytes = [0; 65536];
    let mut w = ObjectWriter::new(&mut bytes).unwrap();
    if !nat {
        if let Some(kind) = options.kind {
            if !["port-group", "address-group", "ipv6-address-group"].contains(&kind) {
                return Err("group type must be port-group, address-group or ipv6-address-group");
            }
            w.field("group_type", &kind).unwrap();
        }
        if let Some(members) = flags.take("members") {
            w.field("group_members", &compat::csv(members)?)
                .map_err(|_| "group members exceed capacity")?;
        }
    } else {
        if create && incoming.is_none_or(|v| v.get("rule_index").ok().flatten().is_none()) {
            w.field("rule_index", &next_index).unwrap();
        }
        let label = text(base, "name")?;
        let description = text(base, "desc")?;
        if label.is_some() && description.is_some() {
            return Err("NAT --name and --description select the same label; supply one");
        }
        if let Some(label) = label.as_deref().or(description.as_deref()) {
            w.field("description", &label)
                .map_err(|_| "NAT label exceeds capacity")?;
        }
        let old_type = text(base, "type")?;
        let mut kind = String::<32>::new();
        for c in options
            .kind
            .or(old_type.as_deref())
            .unwrap_or("DNAT")
            .chars()
        {
            kind.push(c.to_ascii_uppercase())
                .map_err(|_| "NAT type capacity")?;
        }
        let kind = match kind.as_str() {
            "DESTINATION" | "DESTINATION_NAT" | "DNAT" => "DNAT",
            "SOURCE" | "SOURCE_NAT" | "SNAT" => "SNAT",
            "MASQUERADE" => "MASQUERADE",
            _ => return Err("NAT type must be dnat, snat or masquerade"),
        };
        w.field("type", &kind).unwrap();
        if let Some(protocol) = flags.take("protocol") {
            w.field(
                "protocol",
                &match protocol {
                    "tcp" | "TCP" => "tcp",
                    "udp" | "UDP" => "udp",
                    "tcp_udp" | "tcp-and-udp" | "tcp_and_udp" => "tcp_udp",
                    "all" | "ALL" => "all",
                    _ => return Err("NAT protocol must be tcp, udp, tcp_udp or all"),
                },
            )
            .unwrap();
        }
        for (flag, field) in [
            ("translated-address", "ip_address"),
            ("translated-port", "port"),
        ] {
            if let Some(value) = flags.take(flag) {
                w.field(field, &value)
                    .map_err(|_| "NAT translation exceeds capacity")?;
            }
        }
        let interface_flag = flags.take("interface-id");
        let resolved = interface_flag
            .map(|value| interface(remote, site, value))
            .transpose()?;
        let old_interface = text(
            base,
            if old_type.as_deref() == Some("DNAT") {
                "in_interface"
            } else {
                "out_interface"
            },
        )?;
        if let Some(value) = resolved.as_deref().or(old_interface.as_deref()) {
            w.field(
                if kind == "DNAT" {
                    "in_interface"
                } else {
                    "out_interface"
                },
                &value,
            )
            .map_err(|_| "NAT interface exceeds capacity")?;
        }
        let mut filter_buffer = [0; 16384];
        for (address_flag, port_flag, field) in [
            ("src-address", "src-port", "source_filter"),
            ("dst-address", "dst-port", "destination_filter"),
        ] {
            let address = flags.take(address_flag);
            let port = flags.take(port_flag);
            if address.is_some() || port.is_some() {
                let current = base
                    .get(field)
                    .map_err(|_| "invalid NAT filter")?
                    .unwrap_or(
                    Json::parse(
                        br#"{"firewall_group_ids":[],"invert_address":false,"invert_port":false}"#,
                    )
                    .unwrap(),
                );
                w.raw_field(field, filter(current, address, port, &mut filter_buffer)?)
                    .map_err(|_| "NAT filters exceed capacity")?;
            }
        }
        let patch = Json::parse(w.finish().map_err(|_| "legacy payload capacity")?).unwrap();
        let result = payload::merge_objects(
            base,
            Some(patch),
            if kind == "DNAT" {
                &["name", "desc", "out_interface"]
            } else {
                &["name", "desc", "in_interface"]
            },
            out,
        )?;
        if create && text(result, "description")?.is_none_or(|s| s.is_empty()) {
            return Err("NAT create needs --name or a JSON description");
        }
        return Ok(result.raw().as_bytes());
    }
    let result = payload::merge_objects(
        base,
        Some(Json::parse(w.finish().map_err(|_| "legacy payload capacity")?).unwrap()),
        &[],
        out,
    )?;
    if create && text(result, "name")?.is_none_or(|s| s.is_empty()) {
        return Err("group create needs --name");
    }
    Ok(result.raw().as_bytes())
}
