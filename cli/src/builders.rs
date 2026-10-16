//! Convenience payloads use the pinned wire schemas; raw `call` remains explicit.
use crate::{
    Options,
    buffer::Buffer,
    compat::{self, Flags},
    flows::{self, Remote},
    payload,
};
use core::fmt::Write;
use heapless::String;
use unifi::{Call, Json, Method, json::ObjectWriter, network::models};
fn field_text<const N: usize>(
    value: Json<'_>,
    key: &str,
) -> Result<Option<String<N>>, &'static str> {
    value
        .optional(key)
        .map_err(|_| "invalid request field")?
        .map(|v| {
            v.string()
                .and_then(|s| s.decode())
                .map_err(|_| "request string exceeds capacity or has the wrong type")
        })
        .transpose()
}
fn upper<const N: usize>(value: &str) -> Result<String<N>, &'static str> {
    let mut out = String::new();
    for c in value.chars() {
        out.push(if c == '-' {
            '_'
        } else {
            c.to_ascii_uppercase()
        })
        .map_err(|_| "option exceeds capacity")?;
    }
    Ok(out)
}
fn checked(
    json: Json<'_>,
    validate: fn(Json<'_>) -> Result<(), unifi::json::JsonError>,
) -> Result<(), &'static str> {
    validate(json).map_err(
        |_| "request does not match the pinned API schema; inspect required fields and ranges",
    )
}

const WIFI_DEFAULTS:&[u8]=br#"{"enabled":true,"type":"STANDARD","hideName":false,"clientIsolationEnabled":false,"multicastToUnicastConversionEnabled":false,"uapsdEnabled":true,"channel2gLockedTo6":false,"dtimPeriod2gLockedTo3":false,"broadcastingFrequenciesGHz":[2.4,5],"mloEnabled":false,"bandSteeringEnabled":false,"arpProxyEnabled":false,"bssTransitionEnabled":false,"advertiseDeviceName":false,"securityConfiguration":{"type":"WPA2_PERSONAL"}}"#;
fn wifi<'a>(
    base: Json<'_>,
    flags: &mut Flags<'_>,
    options: &Options<'_>,
    out: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let mut bytes = [0; 16384];
    let mut patch = ObjectWriter::new(&mut bytes).unwrap();
    let kind = flags.take("broadcast-type").map(upper::<64>).transpose()?;
    let current = field_text::<64>(base, "type")?;
    let kind = kind.as_deref().or(current.as_deref()).unwrap_or("STANDARD");
    if !["STANDARD", "IOT_OPTIMIZED"].contains(&kind) {
        return Err("broadcast type must be standard or iot-optimized");
    }
    let mut standard_buffer = [0; 131072];
    let base = if kind == "STANDARD" {
        payload::merge_objects(
            Json::parse(WIFI_DEFAULTS).unwrap(),
            Some(base),
            &[],
            &mut standard_buffer,
        )?
    } else {
        base
    };
    patch.field("type", &kind).unwrap();
    let band = flags.boolean("band-steering")?;
    if let Some(value) = band {
        patch.field("bandSteeringEnabled", &value).unwrap();
    }
    if let Some(value) = flags.boolean("hidden")? {
        patch.field("hideName", &value).unwrap();
    }
    let fast = flags.boolean("fast-roaming")?;
    if kind == "IOT_OPTIMIZED" && (band == Some(true) || fast == Some(true)) {
        return Err("IoT broadcasts do not support fast roaming or band steering");
    }
    if let Some(fast) = fast.filter(|_| kind == "STANDARD") {
        patch.field("bssTransitionEnabled", &fast).unwrap();
    }
    if let Some(frequencies) = flags.take("frequencies") {
        if kind != "STANDARD" {
            return Err("frequency selection requires a standard Wi-Fi broadcast");
        }
        let mut frequencies_out = [0; 512];
        let mut w = Buffer::new(&mut frequencies_out);
        w.write_char('[').unwrap();
        for (i, value) in compat::csv(frequencies)?.iter().enumerate() {
            if !["2.4", "5", "5.0", "6", "6.0"].contains(value) {
                return Err("frequencies must be 2.4, 5 or 6");
            }
            if i > 0 {
                w.write_char(',').unwrap();
            }
            w.write_str(value).unwrap();
        }
        w.write_char(']').unwrap();
        patch
            .raw_field(
                "broadcastingFrequenciesGHz",
                Json::parse(w.finish()).unwrap(),
            )
            .unwrap();
    }
    if let Some(network) = options.network {
        let mut value = [0; 1024];
        let mut w = ObjectWriter::new(&mut value).unwrap();
        w.field("type", &"SPECIFIC").unwrap();
        w.field("networkId", &network)
            .map_err(|_| "network ID exceeds capacity")?;
        patch
            .raw_field(
                "network",
                Json::parse(w.finish().map_err(|_| "payload exceeds capacity")?).unwrap(),
            )
            .unwrap();
    }
    let security = flags.take("security").map(upper::<64>).transpose()?;
    let passphrase = flags.take("passphrase");
    let original = base
        .get("securityConfiguration")
        .map_err(|_| "invalid security configuration")?
        .unwrap_or(Json::parse(br#"{"type":"WPA2_PERSONAL"}"#).unwrap());
    let old_type = field_text::<64>(original, "type")?;
    let mode = security
        .as_deref()
        .or(old_type.as_deref())
        .unwrap_or("WPA2_PERSONAL");
    if security.is_some()
        || passphrase.is_some()
        || fast.is_some()
        || kind == "IOT_OPTIMIZED" && original.get("fastRoamingEnabled").ok().flatten().is_some()
    {
        if ![
            "OPEN",
            "WPA2_PERSONAL",
            "WPA3_PERSONAL",
            "WPA2_WPA3_PERSONAL",
            "WPA2_ENTERPRISE",
            "WPA3_ENTERPRISE",
            "WPA2_WPA3_ENTERPRISE",
        ]
        .contains(&mode)
        {
            return Err("unknown Wi-Fi security mode");
        }
        let mut changes = [0; 8192];
        let mut w = ObjectWriter::new(&mut changes).unwrap();
        w.field("type", &mode).unwrap();
        if let Some(password) = passphrase {
            if mode == "OPEN" {
                return Err("open Wi-Fi cannot have a passphrase");
            }
            w.field("passphrase", &password)
                .map_err(|_| "passphrase exceeds capacity")?;
        }
        if let Some(fast) = fast.filter(|_| mode != "OPEN" && kind == "STANDARD") {
            w.field("fastRoamingEnabled", &fast).unwrap();
        }
        if mode.contains("WPA3")
            && mode.ends_with("PERSONAL")
            && original.get("saeConfiguration").ok().flatten().is_none()
        {
            w.raw_field(
                "saeConfiguration",
                Json::parse(br#"{"anticloggingThresholdSeconds":5,"syncTimeSeconds":5}"#).unwrap(),
            )
            .unwrap();
        }
        let changes = Json::parse(w.finish().map_err(|_| "payload exceeds capacity")?).unwrap();
        let mut configuration = [0; 8192];
        let existing = if security.is_some() && old_type.as_deref() != Some(mode) {
            Json::parse(b"{}").unwrap()
        } else {
            original
        };
        let configuration = payload::merge_objects(
            existing,
            Some(changes),
            if kind == "IOT_OPTIMIZED" {
                &["fastRoamingEnabled"][..]
            } else {
                &[]
            },
            &mut configuration,
        )?;
        patch
            .raw_field("securityConfiguration", configuration)
            .map_err(|_| "security configuration exceeds capacity")?;
    }
    let patch = Json::parse(
        patch
            .finish()
            .map_err(|_| "Wi-Fi payload exceeds capacity")?,
    )
    .unwrap();
    let excluded = if kind == "IOT_OPTIMIZED" {
        &[
            "broadcastingFrequenciesGHz",
            "mloEnabled",
            "bandSteeringEnabled",
            "arpProxyEnabled",
            "bssTransitionEnabled",
            "advertiseDeviceName",
        ][..]
    } else {
        &[]
    };
    let result = payload::merge_objects(base, Some(patch), excluded, out)?;
    checked(result, models::WifiBroadcastCreateOrUpdate::validate)?;
    Ok(result)
}

fn network<'a>(
    base: Json<'_>,
    flags: &mut Flags<'_>,
    out: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let mut patch_buffer = [0; 8192];
    let mut w = ObjectWriter::new(&mut patch_buffer).unwrap();
    let host = flags.take("ipv4-host");
    let dhcp = flags.boolean("dhcp")?;
    let management = flags.take("management").map(upper::<32>).transpose()?;
    let old = field_text::<32>(base, "management")?;
    let management = management.as_deref().or(old.as_deref()).unwrap_or(
        if host.is_some() || dhcp == Some(true) {
            "GATEWAY"
        } else {
            "UNMANAGED"
        },
    );
    if !["GATEWAY", "SWITCH", "UNMANAGED"].contains(&management) {
        return Err("management must be gateway, switch or unmanaged");
    }
    w.field("management", &management).unwrap();
    if let Some(vlan) = flags.take("vlan") {
        w.field("vlanId", &vlan.parse::<u16>().map_err(|_| "invalid VLAN")?)
            .unwrap();
    }
    if let Some(zone) = flags.take("zone") {
        w.field("zoneId", &zone)
            .map_err(|_| "zone ID exceeds capacity")?;
    }
    if management != "UNMANAGED" {
        for (flag, field, default) in [
            ("isolated", "isolationEnabled", false),
            ("internet", "internetAccessEnabled", true),
        ] {
            if let Some(value) = flags
                .boolean(flag)?
                .or_else(|| base.get(field).ok().flatten().is_none().then_some(default))
            {
                w.field(field, &value).unwrap();
            }
        }
        if base.get("cellularBackupEnabled").ok().flatten().is_none() {
            w.field("cellularBackupEnabled", &false).unwrap();
        }
        if let Some(host) = host {
            let (ip, prefix) = host.split_once('/').ok_or("ipv4-host requires IP/PREFIX")?;
            ip.parse::<core::net::Ipv4Addr>()
                .map_err(|_| "invalid IPv4 host")?;
            let prefix = prefix.parse::<u8>().map_err(|_| "invalid IPv4 prefix")?;
            let mut ipv4 = [0; 4096];
            let mut ip_w = ObjectWriter::new(&mut ipv4).unwrap();
            ip_w.field("hostIpAddress", &ip).unwrap();
            ip_w.field("prefixLength", &prefix).unwrap();
            ip_w.field("autoScaleEnabled", &false).unwrap();
            if dhcp == Some(true) {
                let mut config = [0; 2048];
                let mut d = ObjectWriter::new(&mut config).unwrap();
                d.field("mode", &"SERVER").unwrap();
                let lease = flags
                    .take("dhcp-lease")
                    .unwrap_or("86400")
                    .parse::<u32>()
                    .map_err(|_| "invalid DHCP lease")?;
                d.field("leaseTimeSeconds", &lease).unwrap();
                d.field("pingConflictDetectionEnabled", &false).unwrap();
                let start = flags
                    .take("dhcp-start")
                    .ok_or("DHCP requires --dhcp-start and --dhcp-stop")?;
                let stop = flags
                    .take("dhcp-stop")
                    .ok_or("DHCP requires --dhcp-start and --dhcp-stop")?;
                let a = start
                    .parse::<core::net::Ipv4Addr>()
                    .map_err(|_| "invalid DHCP range")?;
                let b = stop
                    .parse::<core::net::Ipv4Addr>()
                    .map_err(|_| "invalid DHCP range")?;
                if a > b {
                    return Err("DHCP range start exceeds stop");
                }
                let mut range = [0; 256];
                let mut r = ObjectWriter::new(&mut range).unwrap();
                r.field("start", &start).unwrap();
                r.field("stop", &stop).unwrap();
                d.raw_field(
                    "ipAddressRange",
                    Json::parse(r.finish().map_err(|_| "payload exceeds capacity")?).unwrap(),
                )
                .unwrap();
                if let Some(dns) = flags.take("dns") {
                    let list = compat::csv(dns)?;
                    for address in &list {
                        address
                            .parse::<core::net::IpAddr>()
                            .map_err(|_| "invalid DNS server address")?;
                    }
                    d.field("dnsServerIpAddressesOverride", &list)
                        .map_err(|_| "DNS servers exceed capacity")?;
                }
                ip_w.raw_field(
                    "dhcpConfiguration",
                    Json::parse(d.finish().map_err(|_| "DHCP payload exceeds capacity")?).unwrap(),
                )
                .map_err(|_| "IPv4 payload exceeds capacity")?;
            } else if dhcp == Some(false) { /* Omitted DHCP configuration disables it in the pinned schema. */
            }
            w.raw_field(
                "ipv4Configuration",
                Json::parse(ip_w.finish().map_err(|_| "payload exceeds capacity")?).unwrap(),
            )
            .unwrap();
        } else if dhcp.is_some() {
            return Err("DHCP flags require --ipv4-host");
        }
    } else if host.is_some() || dhcp.is_some() {
        return Err("IPv4/DHCP flags require a managed network");
    }
    let patch = Json::parse(w.finish().map_err(|_| "payload exceeds capacity")?).unwrap();
    let result = payload::merge_objects(base, Some(patch), &[], out)?;
    checked(result, models::CreateOrUpdateNetwork::validate)?;
    Ok(result)
}

fn dns<'a>(
    base: Json<'_>,
    flags: &mut Flags<'_>,
    out: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let kind = flags.take("record-type").map(upper::<32>).transpose()?;
    let old = field_text::<32>(base, "type")?;
    let kind = kind.as_deref().or(old.as_deref()).unwrap_or("A_RECORD");
    let (kind, field) = match kind {
        "A" | "A_RECORD" => ("A_RECORD", "ipv4Address"),
        "AAAA" | "AAAA_RECORD" => ("AAAA_RECORD", "ipv6Address"),
        "CNAME" | "CNAME_RECORD" => ("CNAME_RECORD", "targetDomain"),
        "MX" | "MX_RECORD" => ("MX_RECORD", "mailServerDomain"),
        "TXT" | "TXT_RECORD" => ("TXT_RECORD", "text"),
        "SRV" | "SRV_RECORD" => ("SRV_RECORD", "serverDomain"),
        "FORWARD" | "FORWARD_DOMAIN" => ("FORWARD_DOMAIN", "ipAddress"),
        _ => return Err("unknown DNS record type"),
    };
    let mut bytes = [0; 8192];
    let mut w = ObjectWriter::new(&mut bytes).unwrap();
    w.field("type", &kind).unwrap();
    if let Some(domain) = flags.take("domain") {
        w.field("domain", &domain)
            .map_err(|_| "DNS domain exceeds capacity")?;
    }
    if let Some(value) = flags.take("value") {
        if field == "ipv4Address" {
            value
                .parse::<core::net::Ipv4Addr>()
                .map_err(|_| "invalid IPv4 DNS address")?;
        }
        if field == "ipv6Address" {
            value
                .parse::<core::net::Ipv6Addr>()
                .map_err(|_| "invalid IPv6 DNS address")?;
        }
        w.field(field, &value)
            .map_err(|_| "DNS value exceeds capacity")?;
    }
    if let Some(ttl) = flags.take("ttl") {
        w.field(
            "ttlSeconds",
            &ttl.parse::<u32>().map_err(|_| "invalid DNS TTL")?,
        )
        .unwrap();
    } else if ["A_RECORD", "AAAA_RECORD", "CNAME_RECORD"].contains(&kind)
        && base.get("ttlSeconds").ok().flatten().is_none()
    {
        w.field("ttlSeconds", &3600u32).unwrap();
    }
    if let Some(priority) = flags.take("priority") {
        w.field(
            "priority",
            &priority
                .parse::<u16>()
                .map_err(|_| "invalid DNS priority")?,
        )
        .unwrap();
    }
    let result = payload::merge_objects(
        base,
        Some(Json::parse(w.finish().map_err(|_| "DNS payload exceeds capacity")?).unwrap()),
        &[],
        out,
    )?;
    checked(result, models::CreateOrUpdateDnsPolicy::validate)?;
    Ok(result)
}

pub fn matching_items<'a>(
    items: &str,
    kind: &str,
    out: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let mut rows = Buffer::new(out);
    rows.write_char('[').unwrap();
    for (i, value) in compat::csv(items)?.iter().enumerate() {
        if i > 0 {
            rows.write_char(',')
                .map_err(|_| "traffic items exceed capacity")?;
        }
        let mut item = [0; 1024];
        let mut v = ObjectWriter::new(&mut item).unwrap();
        if kind == "PORTS" {
            if let Some((start, stop)) = value.split_once('-') {
                let start = start.parse::<u16>().map_err(|_| "invalid port")?;
                let stop = stop.parse::<u16>().map_err(|_| "invalid port")?;
                if start > stop {
                    return Err("port range start exceeds stop");
                }
                v.field("type", &"PORT_NUMBER_RANGE").unwrap();
                v.field("start", &start).unwrap();
                v.field("stop", &stop).unwrap();
            } else {
                v.field("type", &"PORT_NUMBER").unwrap();
                v.field("value", &value.parse::<u16>().map_err(|_| "invalid port")?)
                    .unwrap();
            }
        } else if let Some((start, stop)) = value.split_once('-') {
            if kind == "IPV6_ADDRESSES" {
                return Err(
                    "IPv6 lists accept addresses/subnets; ranges are not in the pinned schema",
                );
            }
            let a = start
                .parse::<core::net::IpAddr>()
                .map_err(|_| "invalid IP range")?;
            let b = stop
                .parse::<core::net::IpAddr>()
                .map_err(|_| "invalid IP range")?;
            if a.is_ipv4() != b.is_ipv4()
                || a > b
                || kind != "IP_ADDRESSES" && a.is_ipv4() != (kind == "IPV4_ADDRESSES")
            {
                return Err("invalid IP range");
            }
            v.field("type", &"IP_ADDRESS_RANGE").unwrap();
            v.field("start", &start).unwrap();
            v.field("stop", &stop).unwrap();
        } else {
            let address = value.split_once('/').map_or(*value, |(address, _)| address);
            let ip = address
                .parse::<core::net::IpAddr>()
                .map_err(|_| "invalid traffic address")?;
            if kind != "IP_ADDRESSES" && ip.is_ipv4() != (kind == "IPV4_ADDRESSES") {
                return Err("traffic address has the wrong IP version");
            }
            if let Some((_, prefix)) = value.split_once('/') {
                let prefix = prefix.parse::<u8>().map_err(|_| "invalid subnet prefix")?;
                if prefix > if ip.is_ipv4() { 32 } else { 128 } {
                    return Err("invalid subnet prefix");
                }
            }
            v.field(
                "type",
                &if value.contains('/') {
                    "SUBNET"
                } else {
                    "IP_ADDRESS"
                },
            )
            .unwrap();
            v.field("value", value)
                .map_err(|_| "traffic item exceeds capacity")?;
        }
        rows.write_str(
            core::str::from_utf8(v.finish().map_err(|_| "payload exceeds capacity")?).unwrap(),
        )
        .map_err(|_| "traffic items exceed capacity")?;
    }
    rows.write_char(']')
        .map_err(|_| "traffic items exceed capacity")?;

    Ok(Json::parse(rows.finish()).unwrap())
}

fn traffic<'a>(
    base: Json<'_>,
    flags: &mut Flags<'_>,
    out: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let kind = flags.take("list-type").map(upper::<32>).transpose()?;
    let old = field_text::<32>(base, "type")?;
    let kind = match kind
        .as_deref()
        .or(old.as_deref())
        .unwrap_or("IPV4_ADDRESSES")
    {
        "IPV4" | "IPV4_ADDRESSES" => "IPV4_ADDRESSES",
        "IPV6" | "IPV6_ADDRESSES" => "IPV6_ADDRESSES",
        "PORT" | "PORTS" => "PORTS",
        _ => return Err("list type must be ports, ipv4 or ipv6"),
    };
    let mut bytes = [0; 32768];
    let mut w = ObjectWriter::new(&mut bytes).unwrap();
    w.field("type", &kind).unwrap();
    if let Some(items) = flags.take("items") {
        let mut array = [0; 24576];
        w.raw_field("items", matching_items(items, kind, &mut array)?)
            .map_err(|_| "traffic payload exceeds capacity")?;
    }
    let result = payload::merge_objects(
        base,
        Some(Json::parse(w.finish().map_err(|_| "payload exceeds capacity")?).unwrap()),
        &[],
        out,
    )?;
    checked(result, models::CreateOrUpdateTrafficMatchingList::validate)?;
    Ok(result)
}

fn reorder<'a>(
    remote: &mut Remote<'_, '_>,
    call: &Call,
    selected: &str,
    after: bool,
    out: &'a mut [u8],
) -> Result<&'a [u8], &'static str> {
    let ids = compat::csv(selected)?;
    for (index, id) in ids.iter().enumerate() {
        if !crate::lookup::uuid(id) {
            return Err("policy ordering requires UUIDs");
        }
        if ids[..index]
            .iter()
            .any(|other| id.eq_ignore_ascii_case(other))
        {
            return Err("duplicate policy in ordering");
        }
    }
    let current = remote.execute(
        &Call::raw(Method::GET, call.path()).map_err(|_| "invalid ordering path")?,
        None,
    )?;
    checked(
        current,
        models::IntegrationFirewallPolicyOrderingDto::validate,
    )?;
    let halves = current
        .required("orderedFirewallPolicyIds")
        .map_err(|_| "invalid current policy ordering")?;
    let mut preserved = [0; 65536];
    let preserved = payload::filter(
        halves
            .required(if after {
                "beforeSystemDefined"
            } else {
                "afterSystemDefined"
            })
            .map_err(|_| "invalid policy ordering")?,
        |v| {
            !ids.iter().any(|id| {
                v.string().is_ok_and(|s| {
                    s.chars()
                        .map(|c| c.to_ascii_lowercase())
                        .eq(id.chars().map(|c| c.to_ascii_lowercase()))
                })
            })
        },
        &mut preserved,
    )?;
    let mut halves = [0; 131072];
    let mut w = ObjectWriter::new(&mut halves).unwrap();
    w.field(
        if after {
            "afterSystemDefined"
        } else {
            "beforeSystemDefined"
        },
        &ids,
    )
    .map_err(|_| "policy ordering exceeds capacity")?;
    w.raw_field(
        if after {
            "beforeSystemDefined"
        } else {
            "afterSystemDefined"
        },
        preserved,
    )
    .map_err(|_| "policy ordering exceeds capacity")?;
    let mut result = ObjectWriter::new(out).unwrap();
    result
        .raw_field(
            "orderedFirewallPolicyIds",
            Json::parse(w.finish().map_err(|_| "policy ordering exceeds capacity")?).unwrap(),
        )
        .map_err(|_| "policy ordering exceeds capacity")?;
    result
        .finish()
        .map_err(|_| "policy ordering exceeds capacity")
}

#[allow(clippy::too_many_arguments)]
pub fn prepare<'a>(
    remote: &mut Remote<'_, '_>,
    call: &Call,
    args: &[&str],
    site: &str,
    incoming: Option<&'a [u8]>,
    flags: &mut Flags<'_>,
    output: &'a mut [u8],
) -> Result<Option<&'a [u8]>, &'static str> {
    if matches!(args.first(), Some(&"api" | &"call")) {
        return Ok(incoming);
    }
    if args.first() == Some(&"vpn") && call.operation.id == "deleteNetworkConf" {
        crate::vpn::validate_delete(remote, args)?;
        return Ok(incoming);
    }
    if args.first() == Some(&"vpn")
        && matches!(
            call.operation.id,
            "createNetworkConf"
                | "updateNetworkConf"
                | "createWireguardPeers"
                | "updateWireguardPeers"
                | "deleteWireguardPeers"
        )
    {
        return Ok(Some(crate::vpn::prepare(
            remote, call, args, incoming, output,
        )?));
    }
    if call.operation.id == "queryISPMetrics"
        && let Some(sites) = remote.options.metric_sites
    {
        return Ok(Some(crate::cloud::metrics(
            remote, sites, incoming, output,
        )?));
    }
    if matches!(
        call.operation.id,
        "createNatRule" | "updateNatRule" | "createFirewallGroup" | "updateFirewallGroup"
    ) {
        return Ok(Some(crate::legacy::prepare(
            remote, call, args, incoming, flags, output,
        )?));
    }
    if let Some(order) = remote.options.order {
        if incoming.is_some() {
            return Err("--set and explicit JSON body sources are mutually exclusive");
        }
        return Ok(Some(reorder(
            remote,
            call,
            order,
            remote.options.order_after,
            output,
        )?));
    }
    let family = match call.operation.id {
        "createWifiBroadcast" | "updateWifiBroadcast" => "wifi",
        "createNetwork" | "updateNetwork" => "network",
        "createDnsPolicy" | "updateDnsPolicy" => "dns",
        "createTrafficMatchingList" | "updateTrafficMatchingList" => "traffic",
        "createFirewallPolicy" | "updateFirewallPolicy" => "policy",
        "patchFirewallPolicy" => "policy_patch",
        "createFirewallZone" | "updateFirewallZone" => "zone",
        "createAclRule" | "updateAclRule" => "acl",
        _ => return Ok(incoming),
    };
    let options = *remote.options;
    let input = incoming
        .map(Json::parse)
        .transpose()
        .map_err(|_| "invalid request JSON")?
        .unwrap_or(Json::parse(b"{}").unwrap());
    input.object().map_err(|_| "request must be an object")?;
    let mut current_buffer = [0; 131072];
    let current = if call.operation.method == Method::PUT {
        if incoming.is_none() && !flags.remaining() {
            return Err("no update changes supplied");
        }
        flows::copy(
            remote.execute(
                &Call::raw(Method::GET, call.path()).map_err(|_| "invalid update read path")?,
                None,
            )?,
            &mut current_buffer,
        )?
    } else {
        Json::parse(match family {
            "wifi" => WIFI_DEFAULTS,
            "network" => br#"{"enabled":true,"vlanId":1}"#,
            "dns" => br#"{"enabled":true}"#,
            "policy"=>br#"{"enabled":true,"loggingEnabled":false,"ipProtocolScope":{"ipVersion":"IPV4_AND_IPV6"}}"#,
            "zone"=>br#"{"networkIds":[]}"#,
            "acl"=>br#"{"enabled":true,"type":"IPV4"}"#,
            _ => b"{}",
        })
        .unwrap()
    };
    let mut merged = [0; 131072];
    let base = payload::merge_objects(
        current,
        Some(input),
        &["id", "metadata", "origin"],
        &mut merged,
    )?;
    let result = match family {
        "wifi" => wifi(base, flags, &options, output)?,
        "network" => network(base, flags, output)?,
        "dns" => dns(base, flags, output)?,
        "policy" | "policy_patch" => {
            crate::policies::firewall(remote, site, base, flags, family == "policy_patch", output)?
        }
        "zone" => crate::policies::zone(remote, site, base, flags, output)?,
        "acl" => crate::policies::acl(
            remote,
            site,
            base,
            flags,
            call.operation.method == Method::POST,
            output,
        )?,
        _ => traffic(base, flags, output)?,
    };
    Ok(Some(result.raw().as_bytes()))
}
