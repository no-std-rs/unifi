//! Multi-request workflows. Every mutation is gated before the first request.
use crate::{Options, Url, lookup, payload, request};
use heapless::String;
use unifi::{Body, Call, Json, Operation, network, session_api};

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    SetIp,
    RemoveIp,
    Ports,
    PortsExport,
    PortSet,
    Topology,
}
pub fn kind(args: &[&str]) -> Option<Kind> {
    Some(match args {
        ["clients", "set-ip", ..] => Kind::SetIp,
        ["clients", "remove-ip", ..] => Kind::RemoveIp,
        ["devices", "ports", ..] => Kind::Ports,
        ["devices", "ports-export", ..] => Kind::PortsExport,
        ["devices", "port-set", ..] => Kind::PortSet,
        ["topology", ..] => Kind::Topology,
        _ => return None,
    })
}
impl Kind {
    pub fn operation(self) -> &'static Operation {
        match self {
            Self::SetIp | Self::RemoveIp => &session_api::UPDATE_USER,
            Self::PortSet => &session_api::UPDATE_DEVICE,
            Self::Ports | Self::PortsExport => &session_api::LIST_DEVICES,
            Self::Topology => &network::GET_CONNECTED_CLIENT_OVERVIEW_PAGE,
        }
    }
}
pub struct Remote<'a, 'b> {
    pub url: &'a Url<'b>,
    pub base: &'a str,
    pub key: &'a str,
    pub options: &'a Options<'b>,
    pub response: &'a mut [u8],
}
impl Remote<'_, '_> {
    pub fn execute(&mut self, call: &Call, body: Option<&[u8]>) -> Result<Json<'_>, &'static str> {
        let reply = request(
            self.url,
            self.base,
            self.key,
            call,
            body.map(Body::Json),
            self.options,
            self.response,
        )?;
        if !(200..300).contains(&reply.status) {
            return Err("workflow API request failed");
        }
        if reply.body.is_empty() && call.operation.method != unifi::Method::GET {
            return Ok(Json::parse(b"null").unwrap());
        }
        let json = reply.json().map_err(|_| "invalid workflow response")?;
        if json.get("meta").ok().flatten().is_some() {
            unifi::session::data(json).map_err(|_| "session workflow returned an error")
        } else {
            Ok(json)
        }
    }
}
pub fn owned<const N: usize>(value: Json<'_>, field: &str) -> Result<String<N>, &'static str> {
    value
        .required(field)
        .and_then(Json::string)
        .and_then(|s| s.decode())
        .map_err(|_| "missing or oversized resource field")
}
pub fn copy<'a>(value: Json<'_>, target: &'a mut [u8]) -> Result<Json<'a>, &'static str> {
    let bytes = value.raw().as_bytes();
    target
        .get_mut(..bytes.len())
        .ok_or("workflow resource exceeds capacity")?
        .copy_from_slice(bytes);
    Ok(Json::parse(&target[..bytes.len()]).unwrap())
}
pub fn network_id(records: Json<'_>, selector: &str) -> Result<String<512>, &'static str> {
    for row in records.items().map_err(|_| "invalid network list")? {
        if row
            .get("_id")
            .ok()
            .flatten()
            .and_then(|s| s.string().ok())
            .is_some_and(|s| s.equals(selector))
        {
            return owned(row, "_id");
        }
    }
    owned(payload::select(records, "id", selector)?, "_id")
}
pub fn subnet_contains(network: Json<'_>, ip: core::net::Ipv4Addr) -> bool {
    let subnet: Result<String<64>, _> = owned(network, "ip_subnet");
    let Ok(subnet) = subnet else {
        return false;
    };
    let Some((address, prefix)) = subnet.split_once('/') else {
        return false;
    };
    let Ok(address) = address.parse::<core::net::Ipv4Addr>() else {
        return false;
    };
    let Some(prefix) = prefix.parse::<u32>().ok().filter(|v| *v <= 32) else {
        return false;
    };
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    u32::from(address) & mask == u32::from(ip) & mask
}

pub fn reservation(
    remote: &mut Remote<'_, '_>,
    kind: Kind,
    args: &[&str],
    site: &str,
) -> Result<(), &'static str> {
    let options = *remote.options;
    if args.len() != 3 {
        return Err(
            "usage: clients set-ip MAC --ip IPV4 [--network NAME]; clients remove-ip MAC [--network NAME]",
        );
    }
    let mac = lookup::mac(args[2])?;
    let ip = if kind == Kind::SetIp {
        Some(
            remote
                .options
                .ip
                .ok_or("set-ip needs --ip IPV4")?
                .parse::<core::net::Ipv4Addr>()
                .map_err(|_| "invalid IPv4 reservation")?,
        )
    } else {
        None
    };
    let mut network = None;
    if options.network.is_some() || ip.is_some() {
        let records = remote.execute(&session_api::list_network_conf(site).unwrap(), None)?;
        network = Some(if let Some(selector) = options.network {
            network_id(records, selector)?
        } else {
            let mut matched = None;
            for row in records.items().map_err(|_| "invalid network list")? {
                if subnet_contains(row, ip.unwrap()) {
                    if matched.is_some() {
                        return Err("reservation matches multiple networks; supply --network");
                    }
                    matched = Some(owned::<512>(row, "_id")?);
                }
            }
            matched.ok_or("reservation matches no network; supply --network")?
        });
    }
    let records = remote.execute(&session_api::list_users(site).unwrap(), None)?;
    let mut matches = heapless::Vec::<String<512>, 256>::new();
    let mut mac_count = 0;
    let mut only_mac = None;
    for row in records.items().map_err(|_| "invalid users response")? {
        if owned::<64>(row, "mac")
            .ok()
            .and_then(|s| lookup::mac(&s).ok())
            .as_ref()
            != Some(&mac)
        {
            continue;
        }
        mac_count += 1;
        only_mac = Some(owned::<512>(row, "_id")?);
        if network.as_ref().is_none_or(|n| {
            row.get("network_id")
                .ok()
                .flatten()
                .and_then(|v| v.string().ok())
                .is_some_and(|v| v.equals(n))
        }) {
            matches
                .push(owned(row, "_id")?)
                .map_err(|_| "too many matching reservations")?;
        }
    }
    if kind == Kind::SetIp && matches.is_empty() && mac_count == 1 {
        matches.push(only_mac.unwrap()).unwrap();
    }
    if kind == Kind::SetIp && (matches.len() > 1 || matches.is_empty() && mac_count > 1) {
        return Err("ambiguous reservation; select its network");
    }
    if kind == Kind::RemoveIp && matches.is_empty() {
        return Err("no matching reservation");
    }
    let mut bytes = [0; 2048];
    let mut body = unifi::json::ObjectWriter::new(&mut bytes).unwrap();
    body.field("use_fixedip", &(kind == Kind::SetIp)).unwrap();
    if let Some(ip) = options.ip.filter(|_| kind == Kind::SetIp) {
        body.field("fixed_ip", &ip).unwrap();
        body.field("network_id", &network.as_ref().unwrap().as_str())
            .unwrap();
    }
    if matches.is_empty() {
        body.field("mac", &mac.as_str()).unwrap();
    }
    let body = body.finish().unwrap();
    if matches.is_empty() {
        remote.execute(&session_api::create_user(site).unwrap(), Some(body))?;
    } else {
        for id in matches {
            remote.execute(&session_api::update_user(site, &id).unwrap(), Some(body))?;
        }
    }
    Ok(())
}
