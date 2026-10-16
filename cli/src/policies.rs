//! Current Network firewall payloads and compatibility shorthand resolution.
use crate::{
    builders,
    compat::{self, Flags},
    flows::Remote,
    lookup, payload,
};
use heapless::{String, Vec};
use unifi::{
    Json,
    json::ObjectWriter,
    network::{self, models},
};
fn text<const N: usize>(value: Json<'_>, key: &str) -> Result<Option<String<N>>, &'static str> {
    value
        .optional(key)
        .map_err(|_| "invalid policy field")?
        .map(|v| {
            v.string()
                .and_then(|s| s.decode())
                .map_err(|_| "invalid or oversized policy string")
        })
        .transpose()
}
fn upper<const N: usize>(value: &str) -> Result<String<N>, &'static str> {
    let mut out = String::new();
    for c in value.chars() {
        out.push(c.to_ascii_uppercase())
            .map_err(|_| "policy option exceeds capacity")?;
    }
    Ok(out)
}
pub(crate) fn resolve(
    remote: &mut Remote<'_, '_>,
    site: &str,
    selector: &str,
    zone: bool,
) -> Result<String<512>, &'static str> {
    if lookup::uuid(selector) {
        return String::try_from(selector).map_err(|_| "selector exceeds capacity");
    }
    lookup::resolve(
        remote.url,
        remote.base,
        remote.key,
        if zone {
            network::get_firewall_zones(site)
        } else {
            network::get_networks_overview_page(site)
        }
        .unwrap(),
        selector,
        "id",
        remote.options,
        remote.response,
    )
}
fn networks(
    remote: &mut Remote<'_, '_>,
    site: &str,
    values: &str,
) -> Result<Vec<String<512>, 256>, &'static str> {
    let mut ids = Vec::new();
    for name in compat::csv(values)? {
        ids.push(resolve(remote, site, name, false)?)
            .map_err(|_| "too many network selectors")?;
    }
    Ok(ids)
}
fn matching_list(
    remote: &mut Remote<'_, '_>,
    site: &str,
    selector: &str,
    ports: bool,
) -> Result<String<512>, &'static str> {
    let id = if lookup::uuid(selector) {
        String::try_from(selector).map_err(|_| "list ID exceeds capacity")?
    } else {
        lookup::resolve(
            remote.url,
            remote.base,
            remote.key,
            network::get_traffic_matching_lists(site).unwrap(),
            selector,
            "id",
            remote.options,
            remote.response,
        )?
    };
    let list = remote.execute(
        &network::get_traffic_matching_list(site, &id).unwrap(),
        None,
    )?;
    let kind = text::<32>(list, "type")?.ok_or("matching list has no type")?;
    if ports && kind != "PORTS"
        || !ports && !["IPV4_ADDRESSES", "IPV6_ADDRESSES"].contains(&kind.as_str())
    {
        return Err("firewall group has the wrong matching-list type");
    }
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
fn endpoint<'a>(
    remote: &mut Remote<'_, '_>,
    site: &str,
    base: Json<'_>,
    zone: Option<&str>,
    nets: Option<&str>,
    ips: Option<&str>,
    ports: Option<&str>,
    address_group: Option<&str>,
    port_group: Option<&str>,
    out: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    if usize::from(nets.is_some())
        + usize::from(ips.is_some())
        + usize::from(address_group.is_some())
        > 1
    {
        return Err("network, IP and address-group filters are mutually exclusive");
    }
    if ports.is_some() && port_group.is_some() {
        return Err("port and port-group filters are mutually exclusive");
    }
    let mut patch_buffer = [0; 65536];
    let mut patch = ObjectWriter::new(&mut patch_buffer).unwrap();
    if let Some(zone) = zone {
        patch
            .field("zoneId", &resolve(remote, site, zone, true)?.as_str())
            .map_err(|_| "zone selector exceeds capacity")?;
    }
    if nets.is_some()
        || ips.is_some()
        || ports.is_some()
        || address_group.is_some()
        || port_group.is_some()
    {
        let previous = base
            .optional("trafficFilter")
            .map_err(|_| "invalid traffic filter")?
            .unwrap_or(Json::parse(b"{}").unwrap());
        let mut filter_buffer = [0; 49152];
        let mut filter = ObjectWriter::new(&mut filter_buffer).unwrap();
        let new_address = nets.is_some() || ips.is_some() || address_group.is_some();
        let old_type = text::<64>(previous, "type")?;
        let kind = if nets.is_some() {
            "NETWORK"
        } else if ips.is_some() || address_group.is_some() {
            "IP_ADDRESS"
        } else {
            old_type.as_deref().unwrap_or("PORT")
        };
        filter.field("type", &kind).unwrap();
        if let Some(nets) = nets {
            let ids = networks(remote, site, nets)?;
            let mut bytes = [0; 16384];
            let mut w = ObjectWriter::new(&mut bytes).unwrap();
            w.field("matchOpposite", &false).unwrap();
            w.field("networkIds", &ids)
                .map_err(|_| "network filters exceed capacity")?;
            filter
                .raw_field(
                    "networkFilter",
                    Json::parse(w.finish().map_err(|_| "payload exceeds capacity")?).unwrap(),
                )
                .map_err(|_| "traffic filter exceeds capacity")?;
        }
        let mut bytes = [0; 32768];
        if ips.is_some() || address_group.is_some() {
            let mut w = ObjectWriter::new(&mut bytes).unwrap();
            w.field("matchOpposite", &false).unwrap();
            if let Some(group) = address_group {
                w.field("type", &"TRAFFIC_MATCHING_LIST").unwrap();
                w.field(
                    "trafficMatchingListId",
                    &matching_list(remote, site, group, false)?.as_str(),
                )
                .map_err(|_| "matching list ID exceeds capacity")?;
            } else {
                let mut items = [0; 24576];
                w.field("type", &"IP_ADDRESSES").unwrap();
                w.raw_field(
                    "items",
                    builders::matching_items(ips.unwrap(), "IP_ADDRESSES", &mut items)?,
                )
                .map_err(|_| "IP filter exceeds capacity")?;
            }
            filter
                .raw_field(
                    "ipAddressFilter",
                    Json::parse(w.finish().map_err(|_| "payload exceeds capacity")?).unwrap(),
                )
                .map_err(|_| "traffic filter exceeds capacity")?;
        }
        if ports.is_some() || port_group.is_some() {
            let mut w = ObjectWriter::new(&mut bytes).unwrap();
            w.field("matchOpposite", &false).unwrap();
            if let Some(group) = port_group {
                w.field("type", &"TRAFFIC_MATCHING_LIST").unwrap();
                w.field(
                    "trafficMatchingListId",
                    &matching_list(remote, site, group, true)?.as_str(),
                )
                .map_err(|_| "matching list ID exceeds capacity")?;
            } else {
                let mut items = [0; 24576];
                w.field("type", &"PORTS").unwrap();
                w.raw_field(
                    "items",
                    builders::matching_items(ports.unwrap(), "PORTS", &mut items)?,
                )
                .map_err(|_| "port filter exceeds capacity")?;
            }
            filter
                .raw_field(
                    "portFilter",
                    Json::parse(w.finish().map_err(|_| "payload exceeds capacity")?).unwrap(),
                )
                .map_err(|_| "traffic filter exceeds capacity")?;
        }
        let changes = Json::parse(
            filter
                .finish()
                .map_err(|_| "traffic filter exceeds capacity")?,
        )
        .unwrap();
        let mut merged = [0; 49152];
        let excluded = if new_address {
            if kind == "NETWORK" {
                &["ipAddressFilter"][..]
            } else {
                &["networkFilter"][..]
            }
        } else {
            &[]
        };
        patch
            .raw_field(
                "trafficFilter",
                payload::merge_objects(previous, Some(changes), excluded, &mut merged)?,
            )
            .map_err(|_| "endpoint exceeds capacity")?;
    }
    payload::merge_objects(
        base,
        Some(Json::parse(patch.finish().map_err(|_| "endpoint exceeds capacity")?).unwrap()),
        &[],
        out,
    )
}

pub fn firewall<'a>(
    remote: &mut Remote<'_, '_>,
    site: &str,
    base: Json<'_>,
    flags: &mut Flags<'_>,
    patch_only: bool,
    out: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let mut bytes = [0; 131072];
    let mut w = ObjectWriter::new(&mut bytes).unwrap();
    if let Some(logging) = flags.boolean("logging")? {
        w.field("loggingEnabled", &logging).unwrap();
    }
    if let Some(description) = text::<8192>(base, "desc")? {
        w.field("description", &description.as_str())
            .map_err(|_| "policy description exceeds capacity")?;
    }
    if !patch_only {
        let action = flags.take("action").map(upper::<16>).transpose()?;
        let old = base
            .optional("action")
            .map_err(|_| "invalid policy action")?
            .unwrap_or(Json::parse(b"{}").unwrap());
        let old_action = text::<16>(old, "type")?;
        if let Some(action) = action.as_deref().or(old_action.as_deref()) {
            if !["ALLOW", "BLOCK", "REJECT"].contains(&action) {
                return Err("firewall action must be allow, block or reject");
            }
            let returning = flags.boolean("allow-return-traffic")?;
            if returning.is_some() && action != "ALLOW" {
                return Err("return traffic applies only to Allow policies");
            }
            let mut action_bytes = [0; 1024];
            let mut a = ObjectWriter::new(&mut action_bytes).unwrap();
            a.field("type", &action).unwrap();
            if action == "ALLOW" {
                a.field(
                    "allowReturnTraffic",
                    &returning
                        .or_else(|| {
                            old.get("allowReturnTraffic")
                                .ok()
                                .flatten()
                                .and_then(|v| v.boolean().ok())
                        })
                        .unwrap_or(true),
                )
                .unwrap();
            }
            w.raw_field(
                "action",
                Json::parse(a.finish().map_err(|_| "payload exceeds capacity")?).unwrap(),
            )
            .unwrap();
        }
        if let Some(version) = flags.take("ip-version") {
            let version = upper::<32>(version)?;
            let version = match version.as_str() {
                "IPV4" | "IPV4_ONLY" => "IPV4",
                "IPV6" | "IPV6_ONLY" => "IPV6",
                "IPV4_AND_IPV6" => "IPV4_AND_IPV6",
                _ => return Err("IP version must be IPV4, IPV6 or IPV4_AND_IPV6"),
            };
            let mut scope = [0; 2048];
            let mut s = ObjectWriter::new(&mut scope).unwrap();
            s.field("ipVersion", &version).unwrap();
            if let Some(protocol) = base
                .get("ipProtocolScope")
                .ok()
                .flatten()
                .and_then(|v| v.get("protocolFilter").ok().flatten())
            {
                s.raw_field("protocolFilter", protocol)
                    .map_err(|_| "protocol scope exceeds capacity")?;
            }
            w.raw_field(
                "ipProtocolScope",
                Json::parse(s.finish().map_err(|_| "payload exceeds capacity")?).unwrap(),
            )
            .unwrap();
        }
        if let Some(states) = flags.take("states") {
            let mut items = Vec::<String<16>, 32>::new();
            for state in compat::csv(states)? {
                let state = upper(state)?;
                if !["NEW", "ESTABLISHED", "RELATED", "INVALID"].contains(&state.as_str()) {
                    return Err("invalid connection state");
                }
                items
                    .push(state)
                    .map_err(|_| "too many connection states")?;
            }
            w.field("connectionStateFilter", &items)
                .map_err(|_| "connection states exceed capacity")?;
        }
        let mut endpoint_buffer = [0; 65536];
        for (field, zone, nets, ips, ports, addresses, port_group) in [
            (
                "source",
                "source-zone",
                "src-network",
                "src-ip",
                "src-port",
                "src-address-group",
                "src-port-group",
            ),
            (
                "destination",
                "dest-zone",
                "dst-network",
                "dst-ip",
                "dst-port",
                "dst-address-group",
                "dst-port-group",
            ),
        ] {
            let current = base
                .optional(field)
                .map_err(|_| "invalid policy endpoint")?
                .unwrap_or(Json::parse(b"{}").unwrap());
            w.raw_field(
                field,
                endpoint(
                    remote,
                    site,
                    current,
                    flags.take(zone),
                    flags.take(nets),
                    flags.take(ips),
                    flags.take(ports),
                    flags.take(addresses),
                    flags.take(port_group),
                    &mut endpoint_buffer,
                )?,
            )
            .map_err(|_| "policy endpoints exceed capacity")?;
        }
    }
    let result = payload::merge_objects(
        base,
        Some(Json::parse(w.finish().map_err(|_| "policy exceeds capacity")?).unwrap()),
        &["desc"],
        out,
    )?;
    let validate = if patch_only {
        models::PatchFirewallPolicy::validate
    } else {
        models::CreateOrUpdateFirewallPolicy::validate
    };
    validate(result).map_err(|_| "firewall policy does not match the pinned API schema")?;
    Ok(result)
}
pub fn zone<'a>(
    remote: &mut Remote<'_, '_>,
    site: &str,
    base: Json<'_>,
    flags: &mut Flags<'_>,
    out: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let mut bytes = [0; 16384];
    let mut w = ObjectWriter::new(&mut bytes).unwrap();
    if let Some(values) = flags.take("networks") {
        w.field("networkIds", &networks(remote, site, values)?)
            .map_err(|_| "zone networks exceed capacity")?;
    }
    let result = payload::merge_objects(
        base,
        Some(Json::parse(w.finish().map_err(|_| "payload exceeds capacity")?).unwrap()),
        &[],
        out,
    )?;
    models::CreateOrUpdateFirewallZone::validate(result)
        .map_err(|_| "zone needs a name and network IDs")?;
    Ok(result)
}

fn zone_networks(
    remote: &mut Remote<'_, '_>,
    site: &str,
    selector: &str,
) -> Result<Vec<String<512>, 256>, &'static str> {
    let id = resolve(remote, site, selector, true)?;
    let zone = remote.execute(&network::get_firewall_zone(site, &id).unwrap(), None)?;
    let mut ids = Vec::new();
    for value in zone
        .required("networkIds")
        .and_then(Json::items)
        .map_err(|_| "zone has no network list")?
    {
        ids.push(
            value
                .string()
                .and_then(|s| s.decode())
                .map_err(|_| "invalid zone network ID")?,
        )
        .map_err(|_| "too many zone networks")?;
    }
    if ids.is_empty() {
        return Err("an empty firewall zone cannot be represented as an ACL network filter");
    }
    Ok(ids)
}
fn acl_ports(values: &str) -> Result<Vec<u16, 1024>, &'static str> {
    let mut ports = Vec::new();
    for value in compat::csv(values)? {
        let (start, stop) = if let Some((a, b)) = value.split_once('-') {
            (a.parse::<u16>(), b.parse::<u16>())
        } else {
            let n = value.parse::<u16>();
            (n.clone(), n)
        };
        let start = start.map_err(|_| "invalid ACL port")?;
        let stop = stop.map_err(|_| "invalid ACL port")?;
        if start == 0 || start > stop {
            return Err("invalid ACL port range");
        }
        for port in start..=stop {
            if !ports.contains(&port) {
                ports
                    .push(port)
                    .map_err(|_| "ACL port expansion exceeds 1024 entries")?;
            }
        }
    }
    Ok(ports)
}
pub fn acl<'a>(
    remote: &mut Remote<'_, '_>,
    site: &str,
    base: Json<'_>,
    flags: &mut Flags<'_>,
    create: bool,
    out: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let mut bytes = [0; 65536];
    let mut w = ObjectWriter::new(&mut bytes).unwrap();
    let rule_type = flags.take("rule-type").map(upper::<16>).transpose()?;
    let old = text::<16>(base, "type")?;
    let kind = match rule_type.as_deref().or(old.as_deref()).unwrap_or("IPV4") {
        "IP" | "IPV4" => "IPV4",
        "MAC" => "MAC",
        _ => return Err("ACL rule type must be ipv4 or mac"),
    };
    w.field("type", &kind).unwrap();
    if let Some(action) = flags.take("action") {
        let action = upper::<16>(action)?;
        if !["ALLOW", "BLOCK"].contains(&action.as_str()) {
            return Err("ACL action must be allow or block");
        }
        w.field("action", &action.as_str()).unwrap();
    }
    let protocol = flags.take("protocol");
    let clear_protocol = protocol.is_some_and(|v| v.eq_ignore_ascii_case("all"));
    if kind == "MAC" && protocol.is_some() && !clear_protocol {
        return Err("MAC ACL rules do not have an IP protocol filter");
    }
    if kind == "IPV4"
        && let Some(protocol) = protocol.filter(|_| !clear_protocol)
    {
        let mut protocols = Vec::<&str, 2>::new();
        for value in compat::csv(protocol)? {
            match upper::<32>(value)?.as_str() {
                "TCP" => {
                    if !protocols.contains(&"TCP") {
                        protocols
                            .push("TCP")
                            .map_err(|_| "too many ACL protocols")?;
                    }
                }
                "UDP" => {
                    if !protocols.contains(&"UDP") {
                        protocols
                            .push("UDP")
                            .map_err(|_| "too many ACL protocols")?;
                    }
                }
                "TCP_UDP" | "TCP_AND_UDP" => {
                    protocols.clear();
                    protocols.push("TCP").unwrap();
                    protocols.push("UDP").unwrap();
                }
                _ => return Err("ACL protocols must be tcp, udp or all"),
            }
        }
        w.field("protocolFilter", &protocols).unwrap();
    }
    let mut scope: Option<String<512>> = None;
    let mut endpoint_bytes = [0; 32768];
    for (field, zone_flag, port_flag) in [
        ("sourceFilter", "source-zone", "source-port"),
        ("destinationFilter", "dest-zone", "destination-port"),
    ] {
        let zone = flags.take(zone_flag);
        let ports = flags.take(port_flag);
        if kind == "MAC" {
            if ports.is_some() {
                return Err("MAC ACL rules do not have IP port filters");
            }
            if let Some(zone) = zone {
                let ids = zone_networks(remote, site, zone)?;
                if ids.len() != 1 {
                    return Err(
                        "MAC ACL compatibility zones must contain exactly one network; use an explicit networkIdFilter body otherwise",
                    );
                }
                if scope.as_ref().is_some_and(|old| old != &ids[0]) {
                    return Err(
                        "MAC ACL source and destination zones must refer to the same network",
                    );
                }
                scope = Some(ids[0].clone());
            }
            continue;
        }
        if zone.is_some() || ports.is_some() {
            let current = base
                .optional(field)
                .map_err(|_| "invalid ACL endpoint")?
                .unwrap_or(Json::parse(b"{}").unwrap());
            let mut change = [0; 16384];
            let mut e = ObjectWriter::new(&mut change).unwrap();
            if let Some(zone) = zone {
                e.field("type", &"NETWORKS").unwrap();
                e.field("networkIds", &zone_networks(remote, site, zone)?)
                    .map_err(|_| "ACL networks exceed capacity")?;
            } else if current.get("type").ok().flatten().is_none() {
                e.field("type", &"PORTS").unwrap();
            }
            if let Some(ports) = ports {
                e.field("portFilter", &acl_ports(ports)?)
                    .map_err(|_| "ACL ports exceed capacity")?;
            }
            let endpoint = payload::merge_objects(
                current,
                Some(Json::parse(e.finish().map_err(|_| "payload exceeds capacity")?).unwrap()),
                if zone.is_some() {
                    &["ipAddressesOrSubnets"]
                } else {
                    &[]
                },
                &mut endpoint_bytes,
            )?;
            w.raw_field(field, endpoint)
                .map_err(|_| "ACL endpoints exceed capacity")?;
        }
    }
    if let Some(scope) = scope {
        w.field("networkIdFilter", &scope.as_str()).unwrap();
    }
    if let Some(description) = text::<8192>(base, "desc")? {
        w.field("description", &description.as_str())
            .map_err(|_| "ACL description exceeds capacity")?;
    }
    let changes = Json::parse(w.finish().map_err(|_| "ACL payload exceeds capacity")?).unwrap();
    let mut exclusions = Vec::<&str, 8>::new();
    exclusions.push("desc").unwrap();
    if kind == "MAC" || clear_protocol {
        exclusions.push("protocolFilter").unwrap();
    }
    if kind == "IPV4" {
        exclusions.push("networkIdFilter").unwrap();
    }
    if rule_type.is_some() && old.as_deref() != Some(kind) {
        for field in ["sourceFilter", "destinationFilter"] {
            if let Some(endpoint) = base.get(field).map_err(|_| "invalid ACL endpoint")? {
                let validate = if kind == "MAC" {
                    models::MacAclRuleEndpoint::validate
                } else {
                    models::IpAclRuleEndpoint::validate
                };
                if validate(endpoint).is_err() {
                    exclusions.push(field).unwrap();
                }
            }
        }
    }
    let result = payload::merge_objects(base, Some(changes), &exclusions, out)?;
    if create
        && kind == "IPV4"
        && remote.options.body_file.is_none()
        && remote.options.data.is_none()
        && result.optional("sourceFilter").ok().flatten().is_none()
        && result
            .optional("destinationFilter")
            .ok()
            .flatten()
            .is_none()
    {
        return Err("ACL create needs source/destination zones or explicit endpoint filters");
    }
    models::AclRuleUpdate::validate(result)
        .map_err(|_| "ACL rule does not match the pinned API schema")?;
    Ok(result)
}

pub fn after_create(
    remote: &mut Remote<'_, '_>,
    site: &str,
    id: &str,
    source: &str,
    destination: &str,
) -> Result<(), &'static str> {
    if !lookup::uuid(id) {
        return Err("created policy response has no valid UUID");
    }
    let call = network::get_firewall_policy_ordering(site)
        .unwrap()
        .query("sourceFirewallZoneId", source)
        .and_then(|c| c.query("destinationFirewallZoneId", destination))
        .map_err(|_| "ordering selectors exceed capacity")?;
    let current = remote.execute(&call, None)?;
    models::IntegrationFirewallPolicyOrderingDto::validate(current)
        .map_err(|_| "invalid current policy ordering")?;
    let halves = current.required("orderedFirewallPolicyIds").unwrap();
    let keep = |v: Json<'_>| {
        !v.string().is_ok_and(|s| {
            s.chars()
                .map(|c| c.to_ascii_lowercase())
                .eq(id.chars().map(|c| c.to_ascii_lowercase()))
        })
    };
    let mut before = [0; 65536];
    let mut after = [0; 65536];
    let before = payload::filter(
        halves.required("beforeSystemDefined").unwrap(),
        keep,
        &mut before,
    )?;
    let after = payload::filter(
        halves.required("afterSystemDefined").unwrap(),
        keep,
        &mut after,
    )?;
    let mut appended = [0; 65536];
    let mut array = crate::buffer::Buffer::new(&mut appended);
    use core::fmt::Write;
    array
        .write_str(after.raw().strip_suffix(']').unwrap())
        .map_err(|_| "policy ordering exceeds capacity")?;
    if after.items().unwrap().next().is_some() {
        array
            .write_char(',')
            .map_err(|_| "policy ordering exceeds capacity")?;
    }
    let mut encoded = [0; 1024];
    let n =
        unifi::json::encode_to_slice(id, &mut encoded).map_err(|_| "policy ID exceeds capacity")?;
    array
        .write_str(core::str::from_utf8(&encoded[..n]).unwrap())
        .map_err(|_| "policy ordering exceeds capacity")?;
    array
        .write_char(']')
        .map_err(|_| "policy ordering exceeds capacity")?;
    let mut half_bytes = [0; 131072];
    let mut w = ObjectWriter::new(&mut half_bytes).unwrap();
    w.raw_field("beforeSystemDefined", before)
        .map_err(|_| "policy ordering exceeds capacity")?;
    w.raw_field("afterSystemDefined", Json::parse(array.finish()).unwrap())
        .map_err(|_| "policy ordering exceeds capacity")?;
    let mut bytes = [0; 131072];
    let mut body = ObjectWriter::new(&mut bytes).unwrap();
    body.raw_field(
        "orderedFirewallPolicyIds",
        Json::parse(w.finish().map_err(|_| "payload exceeds capacity")?).unwrap(),
    )
    .map_err(|_| "policy ordering exceeds capacity")?;
    let call = network::update_firewall_policy_ordering(site)
        .unwrap()
        .query("sourceFirewallZoneId", source)
        .and_then(|c| c.query("destinationFirewallZoneId", destination))
        .map_err(|_| "ordering selectors exceed capacity")?;
    remote.execute(
        &call,
        Some(
            body.finish()
                .map_err(|_| "policy ordering exceeds capacity")?,
        ),
    )?;
    Ok(())
}
