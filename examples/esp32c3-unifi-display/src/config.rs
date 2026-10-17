//! Settings written locally over USB by the web flasher, never by the publisher.
use alloc::string::String;
use serde::{Deserialize, Serialize};

pub const FLASH_OFFSET: u32 = 0x3f0000;
pub const SECTOR_SIZE: usize = 4096;
pub const MAX_JSON: usize = 2048;
pub const MAGIC: &[u8; 8] = b"UNIFICF1";

/// Private, link-local and shared-address IPv4 ranges used by local controllers.
pub fn local_ipv4(ip: core::net::Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    a == 10
        || a == 172 && (16..=31).contains(&b)
        || a == 192 && b == 168
        || a == 169 && b == 254
        || a == 100 && (64..=127).contains(&b)
}

// Deliberately no Debug: settings contain the user's passwords.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub controller_url: String,
    pub api_key: String,
    pub certificate_sha256: String,
    pub site_id: String,
    pub device_id: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Missing,
    Corrupt,
    Invalid(&'static str),
}

impl DeviceConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bounded = |s: &str, min, max| s.len() >= min && s.len() <= max && !s.contains('\0');
        if !bounded(&self.wifi_ssid, 1, 32) {
            return Err(ConfigError::Invalid("Wi-Fi name"));
        }
        let password = &self.wifi_password;
        if !(password.is_empty()
            || bounded(password, 8, 63)
            || password.len() == 64 && password.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(ConfigError::Invalid("Wi-Fi password"));
        }
        Endpoint::parse(&self.controller_url)?;
        if !bounded(&self.api_key, 1, 512) || !self.api_key.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(ConfigError::Invalid("API key"));
        }
        if self.certificate_sha256.len() != 64
            || !self
                .certificate_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err(ConfigError::Invalid("Certificate fingerprint"));
        }
        for id in [&self.site_id, &self.device_id] {
            if !id.is_empty() && !uuid(id) {
                return Err(ConfigError::Invalid("Site or device UUID"));
            }
        }
        Ok(())
    }
}

/// CRC-32/ISO-HDLC, also implemented by the browser config writer.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

pub fn decode_sector(bytes: &[u8]) -> Result<DeviceConfig, ConfigError> {
    if bytes.get(..8) != Some(MAGIC.as_slice()) {
        return Err(ConfigError::Missing);
    }
    let header = bytes.get(..16).ok_or(ConfigError::Corrupt)?;
    let len = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
    if len == 0 || len > MAX_JSON {
        return Err(ConfigError::Corrupt);
    }
    let payload = bytes.get(16..16 + len).ok_or(ConfigError::Corrupt)?;
    let checksum = u32::from_le_bytes(header[12..16].try_into().unwrap());
    if crc32(payload) != checksum {
        return Err(ConfigError::Corrupt);
    }
    let config: DeviceConfig = serde_json::from_slice(payload).map_err(|_| ConfigError::Corrupt)?;
    config.validate()?;
    Ok(config)
}

fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

/// HTTPS origin plus integration base. IPv4 and DNS names match this board's network stack.
pub struct Endpoint<'a> {
    pub authority: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub base: &'a str,
}
impl<'a> Endpoint<'a> {
    pub fn parse(url: &'a str) -> Result<Self, ConfigError> {
        let invalid = || ConfigError::Invalid("Controller HTTPS URL");
        if url.len() > 256 || !url.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(invalid());
        }
        let rest = url.strip_prefix("https://").ok_or_else(invalid)?;
        let end = rest.find('/').unwrap_or(rest.len());
        let authority = &rest[..end];
        let base = rest[end..].trim_end_matches('/');
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (
                host,
                port.parse::<u16>()
                    .ok()
                    .filter(|p| *p != 0)
                    .ok_or_else(invalid)?,
            ),
            None => (authority, 443),
        };
        if host.is_empty()
            || host.len() > 253
            || host.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
            || base.contains(['?', '#'])
            || base.split('/').any(|part| part == "." || part == "..")
        {
            return Err(invalid());
        }
        unifi::Client::new(authority, base, "preflight").map_err(|_| invalid())?;
        if host.eq_ignore_ascii_case("api.ui.com")
            || base.contains("/v1/connector/")
            || host
                .parse::<core::net::Ipv4Addr>()
                .is_ok_and(|ip| !local_ipv4(ip))
        {
            return Err(ConfigError::Invalid("Use a direct LAN controller URL"));
        }
        Ok(Self {
            authority,
            host,
            port,
            base,
        })
    }
}

impl DeviceConfig {
    pub fn fingerprint(&self) -> [u8; 32] {
        let mut digest = [0; 32];
        for (out, pair) in digest
            .iter_mut()
            .zip(self.certificate_sha256.as_bytes().chunks_exact(2))
        {
            let hex = |b: u8| {
                if b.is_ascii_digit() {
                    b - b'0'
                } else {
                    b.to_ascii_lowercase() - b'a' + 10
                }
            };
            *out = (hex(pair[0]) << 4) | hex(pair[1]);
        }
        digest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings() -> DeviceConfig {
        DeviceConfig {
            wifi_ssid: "test network".into(),
            wifi_password: "test-password".into(),
            controller_url: "https://unifi.example.com/proxy/network/integration".into(),
            api_key: "fixture-key".into(),
            certificate_sha256: "ab".repeat(32),
            site_id: "".into(),
            device_id: "".into(),
        }
    }
    #[test]
    fn configuration_survives_sector_round_trip_and_rejects_corruption() {
        let config = settings();
        let json = serde_json::to_vec(&config).unwrap();
        let mut sector = [0xff; SECTOR_SIZE];
        sector[..8].copy_from_slice(MAGIC);
        sector[8..12].copy_from_slice(&(json.len() as u32).to_le_bytes());
        sector[12..16].copy_from_slice(&crc32(&json).to_le_bytes());
        sector[16..16 + json.len()].copy_from_slice(&json);
        assert_eq!(decode_sector(&sector).unwrap().api_key, config.api_key);
        sector[20] ^= 1;
        assert!(matches!(decode_sector(&sector), Err(ConfigError::Corrupt)));
        assert!(matches!(
            decode_sector(&[0xff; SECTOR_SIZE]),
            Err(ConfigError::Missing)
        ));
        assert!(matches!(decode_sector(MAGIC), Err(ConfigError::Corrupt)));
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
        assert_eq!(config.fingerprint(), [0xab; 32]);
    }
    #[test]
    fn validates_https_keys_ids_and_utf8_lengths() {
        for url in [
            "http://unifi/",
            "https:///",
            "https://user:pass@unifi/",
            "https://unifi/#fragment",
            "https://unifi/?query",
            "https://unifi:0/",
            "https://unifi:99999/",
            "https://unifi/../api",
            "https://unifi/\r\n",
            "https://[::1]/",
        ] {
            let mut c = settings();
            c.controller_url = url.into();
            assert!(c.validate().is_err(), "{url}");
        }
        let mut c = settings();
        assert!(c.validate().is_ok());
        c.api_key = "key\r\nheader".into();
        assert!(c.validate().is_err());
        c = settings();
        c.site_id = "bad/site".into();
        assert!(c.validate().is_err());
        c = settings();
        c.wifi_ssid = "界".repeat(11);
        assert!(c.validate().is_err());
        c = settings();
        c.certificate_sha256 = "ab".repeat(31);
        assert!(c.validate().is_err());
        c = settings();
        c.wifi_password = "".into();
        assert!(c.validate().is_ok());
        let e = Endpoint::parse("https://192.168.1.1:8443/proxy/network/integration/").unwrap();
        assert_eq!(
            (e.host, e.port, e.base),
            ("192.168.1.1", 8443, "/proxy/network/integration")
        );
        for url in [
            "https://api.ui.com/v1/connector/consoles/fixture%3A123/proxy/network/integration",
            "https://8.8.8.8/integration",
            "https://controller.local/v1/connector/consoles/id/proxy/network/integration",
        ] {
            assert!(Endpoint::parse(url).is_err());
        }
        for ip in [
            "10.1.2.3",
            "172.16.0.1",
            "192.168.6.1",
            "169.254.1.2",
            "100.64.0.1",
        ] {
            assert!(local_ipv4(ip.parse().unwrap()));
        }
        for ip in [
            "8.8.8.8",
            "127.0.0.1",
            "172.32.0.1",
            "100.128.0.1",
            "224.0.0.1",
        ] {
            assert!(!local_ipv4(ip.parse().unwrap()));
        }
    }
}
