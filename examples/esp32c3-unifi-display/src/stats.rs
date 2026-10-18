//! Official Network integration snapshots, independent of the board and clock.
use crate::api::Error;
use alloc::{string::String, vec::Vec};
use unifi::Json;

pub fn text(value: Json<'_>, key: &str) -> Option<String> {
    Some(
        value
            .optional(key)
            .ok()??
            .string()
            .ok()?
            .chars()
            .take(128)
            .collect(),
    )
}
fn number(value: Json<'_>, key: &str) -> Option<f64> {
    value
        .optional(key)
        .ok()??
        .number()
        .ok()
        .filter(|n| n.is_finite() && *n >= 0.0)
}
fn unsigned(value: Json<'_>, key: &str) -> Option<u64> {
    value.optional(key).ok()??.unsigned().ok()
}
fn pct(value: Json<'_>, key: &str) -> Option<f64> {
    number(value, key).filter(|n| *n <= 100.0)
}

#[derive(Clone, Default)]
pub struct Device {
    pub name: Option<String>,
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub ip: Option<String>,
    pub state: Option<String>,
}
impl Device {
    pub fn parse(json: Json<'_>) -> Result<Self, Error> {
        json.object().map_err(|_| Error::InvalidResponse)?;
        Ok(Self {
            name: text(json, "name"),
            model: text(json, "model"),
            firmware: text(json, "firmwareVersion"),
            ip: text(json, "ipAddress"),
            state: text(json, "state"),
        })
    }
}
#[derive(Clone, Default)]
pub struct System {
    pub cpu: Option<f64>,
    pub memory: Option<f64>,
    pub load: [Option<f64>; 3],
    pub uptime: Option<u64>,
}
#[derive(Clone, Default)]
pub struct Radio {
    pub frequency: Option<f64>,
    pub retries: Option<f64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rates {
    pub rx: u64,
    pub tx: u64,
}
#[derive(Clone)]
pub struct Dashboard {
    pub network: Option<crate::connection::NetworkState>,
    pub device: Device,
    pub system: System,
    pub radios: Vec<Radio>,
    pub rates: Option<Rates>,
    pub history: [Option<Rates>; 60],
    pub devices: Option<u64>,
    pub clients: Option<u64>,
    pub last_sample_ms: Option<u64>,
    pub error: Option<Error>,
    pub heartbeat: Option<String>,
    pub heartbeat_changed_ms: Option<u64>,
}
impl Default for Dashboard {
    fn default() -> Self {
        Self {
            network: None,
            device: Device::default(),
            system: System::default(),
            radios: Vec::new(),
            rates: None,
            history: [None; 60],
            devices: None,
            clients: None,
            last_sample_ms: None,
            error: None,
            heartbeat: None,
            heartbeat_changed_ms: None,
        }
    }
}
impl Dashboard {
    /// Record one disconnect transition, independent of readiness-poll frequency.
    pub fn network_unavailable(&mut self) -> bool {
        if self.last_sample_ms.is_none() || self.error == Some(Error::Transport) {
            return false;
        }
        self.error = Some(Error::Transport);
        self.push_rate(None);
        true
    }

    pub fn push_rate(&mut self, rate: Option<Rates>) {
        self.rates = rate;
        self.history.rotate_left(1);
        self.history[59] = rate;
    }
    pub fn apply_statistics(&mut self, json: Json<'_>, now_ms: u64) -> Result<(), Error> {
        json.object().map_err(|_| Error::InvalidResponse)?;
        let interfaces = json
            .required("interfaces")
            .and_then(Json::object)
            .map_err(|_| Error::InvalidResponse)?;
        let heartbeat = text(json, "lastHeartbeatAt");
        if self.heartbeat != heartbeat || self.heartbeat_changed_ms.is_none() {
            self.heartbeat_changed_ms = Some(now_ms);
        }
        self.heartbeat = heartbeat;
        let uptime = unsigned(json, "uptimeSec");
        if self
            .system
            .uptime
            .zip(uptime)
            .is_some_and(|(old, new)| new < old)
        {
            self.push_rate(None);
        }
        self.system = System {
            cpu: pct(json, "cpuUtilizationPct"),
            memory: pct(json, "memoryUtilizationPct"),
            load: [
                number(json, "loadAverage1Min"),
                number(json, "loadAverage5Min"),
                number(json, "loadAverage15Min"),
            ],
            uptime,
        };
        self.radios.clear();
        if let Some(radios) = interfaces
            .optional("radios")
            .map_err(|_| Error::InvalidResponse)?
        {
            for radio in radios.items().map_err(|_| Error::InvalidResponse)?.take(4) {
                self.radios.push(Radio {
                    frequency: number(radio, "frequencyGHz"),
                    retries: pct(radio, "txRetriesPct"),
                });
            }
        }
        let rate = json.optional("uplink").ok().flatten().and_then(|u| {
            Some(Rates {
                rx: unsigned(u, "rxRateBps")?,
                tx: unsigned(u, "txRateBps")?,
            })
        });
        self.push_rate(rate);
        self.last_sample_ms = Some(now_ms);
        Ok(())
    }
    pub fn stale(&self, now_ms: u64) -> bool {
        self.error.is_some()
            || self
                .last_sample_ms
                .is_none_or(|t| now_ms.saturating_sub(t) > 15000)
            || self.heartbeat.is_some()
                && self
                    .heartbeat_changed_ms
                    .is_some_and(|t| now_ms.saturating_sub(t) > 45000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_readiness_checks_preserve_traffic_history_during_an_outage() {
        let mut s = Dashboard::default();
        assert!(!s.network_unavailable());
        s.last_sample_ms = Some(0);
        let rate = Some(Rates { rx: 1, tx: 2 });
        s.history.fill(rate);
        assert!(s.network_unavailable());
        for _ in 0..60 {
            assert!(!s.network_unavailable());
        }
        assert_eq!(s.history.iter().filter(|v| v.is_some()).count(), 59);
        assert_eq!(s.history[59], None);
        assert_eq!(s.error, Some(Error::Transport));
        // A successful sample allows a later disconnection to add its own gap.
        s.error = None;
        s.push_rate(rate);
        assert!(s.network_unavailable());
        assert_eq!(s.history[59], None);
    }
    #[test]
    fn decodes_official_statistics_without_reinterpreting_uplink_as_wan() {
        let mut s = Dashboard::default();
        let body=br#"{"cpuUtilizationPct":5.8,"memoryUtilizationPct":77.2,"uptimeSec":100,"lastHeartbeatAt":"sample1","loadAverage1Min":0.25,"uplink":{"rxRateBps":24860000,"txRateBps":3420000},"interfaces":{"radios":[{"frequencyGHz":5,"txRetriesPct":1.2}]}}"#;
        s.apply_statistics(Json::parse(body).unwrap(), 1000)
            .unwrap();
        assert_eq!(s.system.cpu, Some(5.8));
        assert_eq!(s.system.load, [Some(0.25), None, None]);
        assert_eq!(
            s.rates,
            Some(Rates {
                rx: 24860000,
                tx: 3420000
            })
        );
        assert_eq!(s.radios[0].frequency, Some(5.0));
        s.apply_statistics(Json::parse(body).unwrap(), 47000)
            .unwrap();
        assert!(s.stale(47000));
    }
    #[test]
    fn missing_nullable_and_invalid_metrics_stay_missing_and_failures_make_gaps() {
        let mut s = Dashboard::default();
        s.apply_statistics(Json::parse(br#"{"cpuUtilizationPct":101,"memoryUtilizationPct":null,"uplink":{"rxRateBps":-1,"txRateBps":1},"interfaces":{}}"#).unwrap(),0).unwrap();
        assert_eq!(s.system.cpu, None);
        assert_eq!(s.system.memory, None);
        assert_eq!(s.rates, None);
        assert!(s.radios.is_empty());
        assert!(!s.stale(15000));
        assert!(s.stale(15001));
        s.push_rate(Some(Rates { rx: 1, tx: 2 }));
        s.push_rate(None);
        assert_eq!(s.history[58], Some(Rates { rx: 1, tx: 2 }));
        assert_eq!(s.history[59], None);
        s.error = Some(Error::Timeout);
        assert!(s.stale(0));
        assert!(s.apply_statistics(Json::parse(b"{}").unwrap(), 0).is_err());
        assert_eq!(
            Device::parse(Json::parse(br#"{"name":"AP \u65e5"}"#).unwrap())
                .unwrap()
                .name
                .as_deref(),
            Some("AP 日")
        );
    }
}
