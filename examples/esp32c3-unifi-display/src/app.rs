//! Bounded, versioned USB setup protocol. No operating system or runtime dependency.
use crate::config::DeviceConfig;
use alloc::{string::String, vec::Vec};
use serde::{Deserialize, Serialize};

pub const PREFIX: &[u8] = b"UNIFI_APP ";
pub const MAX_REQUEST: usize = 4096;
pub const SESSION_MS: u64 = 10_000;

// No Debug: requests can carry credentials.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub v: u8,
    pub id: u32,
    pub session: u32,
    pub command: Command,
}
#[derive(Deserialize)]
#[serde(tag = "name", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Hello {
        nonce: String,
    },
    Ping {},
    Bye {},
    GetConfig {},
    Status {},
    ScanWifi {},
    TestWifi {
        ssid: String,
        password: String,
    },
    ConsoleBegin {
        ssid: String,
        password: String,
        address: String,
        hostname: String,
        now: u64,
    },
    ConsoleLogin {
        username: String,
        password: String,
        token: String,
    },
    ConsoleMfa {
        id: String,
    },
    ConsolePoll {},
    ConsoleProvision {
        site_id: String,
        device_id: String,
        local_key: String,
    },
    ConsoleEnd {},
    SaveConfig {
        config: DeviceConfig,
    },
}
#[derive(Serialize)]
pub struct Reply<T: Serialize> {
    pub v: u8,
    pub id: u32,
    pub session: u32,
    pub ok: bool,
    pub result: T,
    pub error: Option<&'static str>,
}

/// Discards oversized lines in full, then resumes at the next newline.
#[derive(Default)]
pub struct Decoder {
    bytes: Vec<u8>,
    dropping: bool,
}
impl Decoder {
    pub fn push(&mut self, byte: u8) -> Option<Request> {
        if byte == b'\n' {
            let result = if !self.dropping {
                self.bytes
                    .strip_prefix(PREFIX)
                    .and_then(|json| serde_json::from_slice(json).ok())
            } else {
                None
            };
            self.bytes.clear();
            self.dropping = false;
            return result;
        }
        if !self.dropping {
            if self.bytes.len() >= MAX_REQUEST {
                self.bytes.clear();
                self.dropping = true;
            } else {
                self.bytes.push(byte);
            }
        }
        None
    }
}

#[derive(Clone, Copy, Default)]
pub struct Session {
    id: u32,
    seen: u64,
}
impl Session {
    pub const fn default_const() -> Self {
        Self { id: 0, seen: 0 }
    }
    pub fn open(&mut self, id: u32, now: u64) {
        self.id = id.max(1);
        self.seen = now;
    }
    pub fn active(&self, now: u64) -> bool {
        self.id != 0 && now.saturating_sub(self.seen) < SESSION_MS
    }
    pub fn accepts(&self, id: u32, now: u64) -> bool {
        self.id == id && self.active(now)
    }
    pub fn touch(&mut self, id: u32, now: u64) -> bool {
        if !self.accepts(id, now) {
            return false;
        }
        self.seen = now;
        true
    }
    pub fn close(&mut self, id: u32) {
        if self.id == id {
            self.id = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_recovers_from_boot_logs_oversize_and_invalid_json() {
        let mut decoder = Decoder::default();
        for b in b"boot log\nUNIFI_APP {bad}\n" {
            assert!(decoder.push(*b).is_none());
        }
        for _ in 0..MAX_REQUEST + 80 {
            assert!(decoder.push(b'x').is_none());
        }
        assert!(decoder.push(b'\n').is_none());
        let bytes = b"UNIFI_APP {\"v\":1,\"id\":2,\"session\":3,\"command\":{\"name\":\"ping\"}}\n";
        let r = bytes
            .iter()
            .filter_map(|b| decoder.push(*b))
            .next()
            .unwrap();
        assert_eq!((r.v, r.id, r.session), (1, 2, 3));
        assert!(matches!(r.command, Command::Ping {}));
        assert!(serde_json::from_str::<Request>(
            r#"{"v":1,"id":1,"session":0,"command":{"name":"ping","password":"unexpected"}}"#
        )
        .is_err());
    }
    #[test]
    fn expired_or_replaced_sessions_cannot_authorize_writes() {
        let mut s = Session::default();
        assert!(!s.accepts(0, 0));
        s.open(7, 100);
        assert!(s.touch(7, 200));
        assert!(!s.touch(8, 201));
        assert!(!s.touch(7, 10200));
        s.open(8, 10201);
        s.close(7);
        assert!(s.accepts(8, 10202));
        assert!(!s.accepts(7, 10202));
        s.close(8);
        assert!(!s.active(10203));
    }
}
