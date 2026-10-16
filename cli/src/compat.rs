//! Command-specific convenience options, kept separate from wire-level JSON fields.
use heapless::Vec;
pub struct Flags<'a> {
    values: Vec<(&'a str, &'a str), 64>,
    used: u64,
}
impl<'a> Flags<'a> {
    pub fn new() -> Self {
        Self {
            values: Vec::new(),
            used: 0,
        }
    }
    pub fn push(&mut self, name: &'a str, value: &'a str) -> Result<(), &'static str> {
        self.values
            .push((name, value))
            .map_err(|_| "too many command options")
    }
    pub fn take(&mut self, name: &str) -> Option<&'a str> {
        let mut result = None;
        for (i, (key, value)) in self.values.iter().enumerate() {
            if *key == name {
                self.used |= 1 << i;
                result = Some(*value);
            }
        }
        result
    }
    pub fn boolean(&mut self, name: &str) -> Result<Option<bool>, &'static str> {
        self.take(name)
            .map(|v| match v {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => Err("boolean option requires true or false"),
            })
            .transpose()
    }
    pub fn remaining(&self) -> bool {
        (0..self.values.len()).any(|i| self.used & (1 << i) == 0)
    }
    pub fn finish(&self) -> Result<(), &'static str> {
        if self.remaining() {
            Err("one of the supplied options is not valid for this command")
        } else {
            Ok(())
        }
    }
}
pub fn boolean(name: &str) -> bool {
    matches!(
        name,
        "get"
            | "no-descriptions"
            | "force"
            | "after-system"
            | "hidden"
            | "band-steering"
            | "fast-roaming"
            | "dhcp"
            | "isolated"
            | "internet"
            | "logging"
            | "allow-return-traffic"
            | "unarchived"
    )
}
pub fn known(name: &str) -> bool {
    boolean(name)
        || matches!(
            name,
            "action"
                | "broadcast-type"
                | "security"
                | "passphrase"
                | "frequencies"
                | "management"
                | "vlan"
                | "ipv4-host"
                | "dhcp-start"
                | "dhcp-stop"
                | "dhcp-lease"
                | "dns"
                | "zone"
                | "domain"
                | "record-type"
                | "value"
                | "ttl"
                | "priority"
                | "list-type"
                | "items"
                | "members"
                | "networks"
                | "rule-type"
                | "source-zone"
                | "dest-zone"
                | "protocol"
                | "source-port"
                | "destination-port"
                | "ip-version"
                | "states"
                | "src-ip"
                | "dst-ip"
                | "src-network"
                | "dst-network"
                | "src-port"
                | "dst-port"
                | "src-address-group"
                | "dst-address-group"
                | "src-port-group"
                | "dst-port-group"
                | "interface-id"
                | "src-address"
                | "dst-address"
                | "translated-address"
                | "translated-port"
                | "group-by"
                | "types"
                | "set"
                | "path"
        )
}
pub fn csv(value: &str) -> Result<Vec<&str, 256>, &'static str> {
    let mut result = Vec::new();
    for value in value.split(',').map(str::trim).filter(|v| !v.is_empty()) {
        result
            .push(value)
            .map_err(|_| "too many comma-separated values")?;
    }
    Ok(result)
}
