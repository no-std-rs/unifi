//! Supplied Reqwest blocking backend (`blocking` feature).
use crate::Authentication;
use crate::host::{self, BlockingClient, BlockingTransport, Error, Limits, Options, Request};
use std::{string::ToString, time::Duration};

pub type Client = BlockingClient<Transport>;

/// Adapter for a caller-configured Reqwest client and its reusable connection pool.
/// The supplied client must satisfy [`BlockingTransport`]'s TLS and request policy.
pub struct Transport(reqwest::blocking::Client);
impl Transport {
    pub fn new(http: reqwest::blocking::Client) -> Self {
        Self(http)
    }
    pub fn http_client(&self) -> &reqwest::blocking::Client {
        &self.0
    }
}
impl BlockingTransport for Transport {
    type Body = reqwest::blocking::Response;
    fn send(
        &self,
        request: Request,
        timeout: Duration,
    ) -> Result<http::Response<Self::Body>, Error> {
        let (parts, body) = request.into_parts();
        let reply = self
            .0
            .request(parts.method, parts.uri.to_string())
            .headers(parts.headers)
            .body(body)
            .timeout(timeout)
            .send()?;
        let mut response = http::Response::new(reply);
        *response.status_mut() = response.body().status();
        *response.headers_mut() = response.body().headers().clone();
        Ok(response)
    }
}
impl Client {
    pub fn new(origin: &str, base: &str, key: &str) -> Result<Self, Error> {
        Self::with_authentication(origin, base, Authentication::ApiKey(key))
    }
    pub fn with_authentication(
        origin: &str,
        base: &str,
        auth: Authentication<'_>,
    ) -> Result<Self, Error> {
        Self::with_options(origin, base, auth, Options::default())
    }
    /// Build a verified-TLS client without redirects, retries, decompression or environment proxies.
    pub fn with_options(
        origin: &str,
        base: &str,
        auth: Authentication<'_>,
        options: Options,
    ) -> Result<Self, Error> {
        let limits = options.limits().validate()?;
        let http = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_zstd()
            .no_deflate()
            .tls_certs_merge(host::reqwest_roots(options.root_certificates)?)
            .build()?;
        Self::with_http_client(origin, base, auth, http, limits)
    }
    /// Reuse an application's HTTP client, including its TLS, DNS, proxy and pool
    /// configuration. The caller configures [`BlockingTransport`]'s request policy;
    /// an opaque, prebuilt client cannot be inspected or reconfigured by the SDK.
    pub fn with_http_client(
        origin: &str,
        base: &str,
        auth: Authentication<'_>,
        http: reqwest::blocking::Client,
        limits: Limits,
    ) -> Result<Self, Error> {
        Self::with_transport(origin, base, auth, Transport::new(http), limits)
    }
}
