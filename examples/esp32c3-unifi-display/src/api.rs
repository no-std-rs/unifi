//! Read-only generated Network API calls, with unambiguous site/device selection.
use crate::stats::{text, Dashboard, Device};
use alloc::{string::String, vec, vec::Vec};
use unifi::{network, Call, Json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Transport,
    LocalAddress,
    Tls,
    Timeout,
    TooLarge,
    InvalidResponse,
    Http(u16),
    RateLimited(u64),
    SelectSite,
    SelectDevice,
}
impl Error {
    pub fn label(self) -> &'static str {
        match self {
            Self::Transport => "NETWORK - RECONNECTING",
            Self::LocalAddress => "USE LOCAL CONTROLLER IP",
            Self::Tls => "TLS CERTIFICATE / HANDSHAKE",
            Self::Timeout => "REQUEST TIMED OUT",
            Self::TooLarge => "RESPONSE TOO LARGE",
            Self::InvalidResponse => "INVALID API RESPONSE",
            Self::Http(401 | 403) => "API KEY / ACCESS DENIED",
            Self::Http(404) => "CHECK API URL / DEVICE",
            Self::Http(_) => "CONTROLLER HTTP ERROR",
            Self::RateLimited(_) => "CONTROLLER RATE LIMIT",
            Self::SelectSite => "SET SITE UUID IN INSTALLER",
            Self::SelectDevice => "SET DEVICE UUID IN INSTALLER",
        }
    }
}
#[allow(async_fn_in_trait)]
pub trait Http {
    async fn get(&mut self, call: &Call, response: &mut [u8]) -> Result<usize, Error>;
}

#[derive(Clone, Copy)]
enum Request {
    Sites,
    DeviceSelection,
    Devices,
    Details,
    Clients,
    Statistics,
}
impl Request {
    // Build the 4 KiB SDK Call in one reusable frame, outside the outer async
    // sample. Separate call temporaries at every await otherwise inflate both
    // the task's static storage and its CPU polling frame on the ESP32-C3.
    #[inline(never)]
    fn build(self, site: &str, device: &str) -> Result<Call, Error> {
        let call = match self {
            Self::Sites => network::get_site_overview_page(),
            Self::DeviceSelection | Self::Devices => {
                network::get_adopted_device_overview_page(site)
            }
            Self::Details => network::get_adopted_device_details(site, device),
            Self::Clients => network::get_connected_client_overview_page(site),
            Self::Statistics => network::get_adopted_device_latest_statistics(site, device),
        }
        .map_err(|_| Error::InvalidResponse)?;
        let limit = match self {
            Self::Sites | Self::DeviceSelection => Some("2"),
            Self::Devices | Self::Clients => Some("1"),
            Self::Details | Self::Statistics => None,
        };
        // Fresh generated calls have no query to replace. Appending directly
        // avoids page()/set_query() copying another 4 KiB path onto the stack.
        if let Some(limit) = limit {
            call.query("offset", "0")
                .and_then(|call| call.query("limit", limit))
                .map_err(|_| Error::InvalidResponse)
        } else {
            Ok(call)
        }
    }
}

pub struct Client<H> {
    pub http: H,
    site: String,
    device: String,
    buffer: Vec<u8>,
    polls: u32,
}
impl<H: Http> Client<H> {
    pub fn new(http: H, site: &str, device: &str) -> Self {
        Self {
            http,
            site: site.into(),
            device: device.into(),
            buffer: vec![0; 16384],
            polls: 0,
        }
    }
    async fn fetch(&mut self, request: Request) -> Result<Json<'_>, Error> {
        let call = request.build(&self.site, &self.device)?;
        let len = self.http.get(&call, &mut self.buffer).await?;
        let bytes = self.buffer.get(..len).ok_or(Error::InvalidResponse)?;
        Json::parse(bytes).map_err(|_| Error::InvalidResponse)
    }
    pub async fn sample(
        &mut self,
        state: &mut Dashboard,
        now_ms: impl Fn() -> u64,
    ) -> Result<(), Error> {
        if self.site.is_empty() {
            let json = self.fetch(Request::Sites).await?;
            self.site = single_id(json, Error::SelectSite)?;
        }
        if self.device.is_empty() {
            let json = self.fetch(Request::DeviceSelection).await?;
            self.device = single_id(json, Error::SelectDevice)?;
        }
        if self.polls.is_multiple_of(12) {
            let json = self.fetch(Request::Details).await?;
            state.device = Device::parse(json)?;
            // Optional site counts do not prevent selected-device statistics.
            state.devices = optional_count(self.fetch(Request::Devices).await)?;
            state.clients = optional_count(self.fetch(Request::Clients).await)?;
        }
        let json = self.fetch(Request::Statistics).await?;
        state.apply_statistics(json, now_ms())?;
        self.polls = self.polls.wrapping_add(1);
        Ok(())
    }
}
fn count(json: Json<'_>) -> Option<u64> {
    json.required("totalCount").ok()?.unsigned().ok()
}
fn optional_count(result: Result<Json<'_>, Error>) -> Result<Option<u64>, Error> {
    match result {
        Ok(json) => Ok(count(json)),
        Err(error @ Error::RateLimited(_)) => Err(error),
        Err(_) => Ok(None),
    }
}
fn single_id(json: Json<'_>, ambiguous: Error) -> Result<String, Error> {
    if count(json) != Some(1) {
        return Err(ambiguous);
    }
    let mut data = json
        .required("data")
        .and_then(Json::items)
        .map_err(|_| Error::InvalidResponse)?;
    let id = data
        .next()
        .and_then(|value| text(value, "id"))
        .ok_or(Error::InvalidResponse)?;
    if data.next().is_some() || id.is_empty() {
        return Err(Error::InvalidResponse);
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        paths: Vec<String>,
        ambiguous: bool,
    }
    impl Http for Fixture {
        async fn get(&mut self, call: &Call, response: &mut [u8]) -> Result<usize, Error> {
            self.paths.push(call.path().into());
            let body = if call.path().starts_with("/v1/sites?") {
                if self.ambiguous {
                    r#"{"totalCount":2,"data":[{"id":"site1"},{"id":"site2"}]}"#
                } else {
                    r#"{"totalCount":1,"data":[{"id":"site"}]}"#
                }
            } else if call.path().ends_with("statistics/latest") {
                r#"{"uptimeSec":100,"cpuUtilizationPct":8,"interfaces":{}}"#
            } else if call.path().contains("/devices?") {
                r#"{"totalCount":1,"data":[{"id":"device"}]}"#
            } else if call.path().contains("/clients?") {
                r#"{"totalCount":7,"data":[]}"#
            } else {
                r#"{"id":"device","name":"Living room AP","state":"ONLINE"}"#
            };
            response[..body.len()].copy_from_slice(body.as_bytes());
            Ok(body.len())
        }
    }
    #[test]
    fn sample_state_reuses_request_storage() {
        let mut client = Client::new(
            Fixture {
                paths: Vec::new(),
                ambiguous: false,
            },
            "",
            "",
        );
        let mut state = Dashboard::default();
        let sample = client.sample(&mut state, || 0);
        let bytes = core::mem::size_of_val(&sample);
        assert!(bytes < 12 * 1024, "sample future grew to {bytes} bytes");
    }
    #[tokio::test]
    async fn uses_only_generated_gets_and_reuses_explicit_or_single_selection() {
        let mut c = Client::new(
            Fixture {
                paths: Vec::new(),
                ambiguous: false,
            },
            "",
            "",
        );
        let mut s = Dashboard::default();
        c.sample(&mut s, || 1000).await.unwrap();
        assert_eq!(
            c.http.paths,
            [
                "/v1/sites?offset=0&limit=2",
                "/v1/sites/site/devices?offset=0&limit=2",
                "/v1/sites/site/devices/device",
                "/v1/sites/site/devices?offset=0&limit=1",
                "/v1/sites/site/clients?offset=0&limit=1",
                "/v1/sites/site/devices/device/statistics/latest",
            ]
        );
        assert_eq!(s.device.name.as_deref(), Some("Living room AP"));
        assert_eq!(s.devices, Some(1));
        assert_eq!(s.clients, Some(7));
        assert_eq!(s.system.cpu, Some(8.0));
        let len = c.http.paths.len();
        c.sample(&mut s, || 6000).await.unwrap();
        assert_eq!(c.http.paths.len(), len + 1);
        assert!(c.http.paths.last().unwrap().ends_with("statistics/latest"));
    }
    #[tokio::test]
    async fn never_picks_arbitrary_first_site() {
        let mut c = Client::new(
            Fixture {
                paths: Vec::new(),
                ambiguous: true,
            },
            "",
            "",
        );
        assert_eq!(
            c.sample(&mut Dashboard::default(), || 0).await,
            Err(Error::SelectSite)
        );
        assert_eq!(c.http.paths.len(), 1);
    }
    #[tokio::test]
    async fn rate_limited_optional_count_stops_all_further_requests() {
        struct Limited {
            calls: usize,
        }
        impl Http for Limited {
            async fn get(&mut self, _: &Call, response: &mut [u8]) -> Result<usize, Error> {
                self.calls += 1;
                if self.calls == 2 {
                    return Err(Error::RateLimited(90));
                }
                let body = br#"{"name":"AP","state":"ONLINE"}"#;
                response[..body.len()].copy_from_slice(body);
                Ok(body.len())
            }
        }
        let mut c = Client::new(Limited { calls: 0 }, "site", "device");
        assert_eq!(
            c.sample(&mut Dashboard::default(), || 0).await,
            Err(Error::RateLimited(90))
        );
        assert_eq!(c.http.calls, 2);
    }
}
