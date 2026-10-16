//! Blocking host client (`std` feature). Do not call on an async executor thread.
use crate::host::{self, Endpoint, Error, Options, Response};
use crate::{Authentication, Body, Call, session::Session};

/// A host client with owned credentials, verified TLS, and bounded owned responses.
/// Redirects, automatic retries, decompression, and environment proxies are disabled.
pub struct Client {
    endpoint: Endpoint,
    http: reqwest::blocking::Client,
    limit: usize,
}
impl core::fmt::Debug for Client {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Client")
            .field("authentication", &"[REDACTED]")
            .finish_non_exhaustive()
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
    pub fn with_options(
        origin: &str,
        base: &str,
        auth: Authentication<'_>,
        options: Options,
    ) -> Result<Self, Error> {
        let endpoint = Endpoint::new(origin, base, auth)?;
        if options.timeout.is_zero() || options.max_response_bytes == usize::MAX {
            return Err(crate::BuildError::InvalidParameter.into());
        }
        let builder = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_zstd()
            .no_deflate()
            .timeout(options.timeout)
            .tls_certs_merge(host::reqwest_roots(options.root_certificates)?);
        Ok(Self {
            endpoint,
            http: builder.build()?,
            limit: options.max_response_bytes,
        })
    }
    /// Execute a generated call with an optional JSON body.
    pub fn execute(&self, call: &Call, body: Option<&[u8]>) -> Result<Response, Error> {
        self.execute_body(call, body.map(Body::Json))
    }
    /// Execute JSON, binary or multipart data. No request is replayed.
    pub fn execute_body(&self, call: &Call, body: Option<Body<'_>>) -> Result<Response, Error> {
        self.execute_inner::<4096, 512>(call, body, None)
    }
    /// Use this origin's jar for outgoing cookies/CSRF (or unauthenticated login
    /// when empty), and atomically rotate it as soon as response headers arrive.
    /// Do not share a jar across origins. No password or login replay is stored.
    pub fn execute_with_session<const C: usize, const T: usize>(
        &self,
        call: &Call,
        body: Option<Body<'_>>,
        session: &mut Session<C, T>,
    ) -> Result<Response, Error> {
        self.execute_inner(call, body, Some(session))
    }
    fn execute_inner<const C: usize, const T: usize>(
        &self,
        call: &Call,
        body: Option<Body<'_>>,
        session: Option<&mut Session<C, T>>,
    ) -> Result<Response, Error> {
        let auth = session
            .as_ref()
            .map(|jar| jar.authentication().unwrap_or(Authentication::None));
        let (url, headers) = self.endpoint.request(call, body, auth)?;
        let method = reqwest::Method::from_bytes(call.operation.method.as_str().as_bytes())
            .map_err(|_| crate::BuildError::InvalidParameter)?;
        let mut request = self.http.request(method, url).headers(headers);
        if let Some(body) = body {
            request = request.body(body.bytes().to_vec());
        }
        let mut reply = request.send()?;
        if let Some(session) = session {
            session.observe_response(
                reply
                    .headers()
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_bytes())),
            )?;
        }
        host::check_encoding(reply.headers())?;
        let mut response = host::metadata(reply.status().as_u16(), reply.headers());
        use std::io::Read;
        (&mut reply)
            .take((self.limit as u64) + 1)
            .read_to_end(&mut response.body)?;
        if response.body.len() > self.limit {
            return Err(Error::ResponseTooLarge);
        }
        Ok(response)
    }
}
