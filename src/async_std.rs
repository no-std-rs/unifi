//! Native async-std client (`async_std` feature). Uses async-std sockets and
//! futures-rustls with the allocation-free HTTP engine; no Tokio runtime is used.
use crate::host::{Endpoint, Error, Limits, Options, Response};
use crate::{Authentication, Body, Call, session::Session};
use embedded_io_adapters::futures_03::FromFutures;
use embedded_io_async::{Read, Write};
use std::{boxed::Box, sync::Arc, time::Duration, vec};

pub struct Client {
    endpoint: Endpoint,
    tls: futures_rustls::TlsConnector,
    limit: usize,
    timeout: Duration,
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
        let limits = options.limits().validate()?;
        let mut roots = rustls::RootCertStore::empty();
        roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
        for cert in options.root_certificates {
            roots.add(cert.0).map_err(Error::Tls)?;
        }
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Self::with_tls_config(origin, base, auth, Arc::new(config), limits)
    }
    /// Use an application's Rustls configuration, including its trust store,
    /// client certificate, protocol versions and cryptographic provider.
    pub fn with_tls_config(
        origin: &str,
        base: &str,
        auth: Authentication<'_>,
        tls: Arc<rustls::ClientConfig>,
        limits: Limits,
    ) -> Result<Self, Error> {
        let limits = limits.validate()?;
        Ok(Self {
            endpoint: Endpoint::new(origin, base, auth)?,
            tls: tls.into(),
            limit: limits.max_response_bytes,
            timeout: limits.timeout,
        })
    }
    pub async fn execute(&self, call: &Call, body: Option<&[u8]>) -> Result<Response, Error> {
        self.execute_body(call, body.map(Body::Json)).await
    }
    pub async fn execute_body(
        &self,
        call: &Call,
        body: Option<Body<'_>>,
    ) -> Result<Response, Error> {
        self.execute_timed::<4096, 512>(call, body, None).await
    }
    /// Use this origin's jar for outgoing credentials and atomic response-header
    /// rotation. An empty jar authenticates neither with cookies nor an API key.
    pub async fn execute_with_session<const C: usize, const T: usize>(
        &self,
        call: &Call,
        body: Option<Body<'_>>,
        session: &mut Session<C, T>,
    ) -> Result<Response, Error> {
        self.execute_timed(call, body, Some(session)).await
    }

    async fn execute_timed<const C: usize, const T: usize>(
        &self,
        call: &Call,
        body: Option<Body<'_>>,
        session: Option<&mut Session<C, T>>,
    ) -> Result<Response, Error> {
        // Keep nested core/TLS futures off the executor stack, including in
        // unoptimized builds and task::block_on callers.
        ::async_std::future::timeout(
            self.timeout,
            Box::pin(self.execute_inner(call, body, session)),
        )
        .await
        .map_err(|_| Error::Timeout)?
    }
    async fn execute_inner<const C: usize, const T: usize>(
        &self,
        call: &Call,
        body: Option<Body<'_>>,
        session: Option<&mut Session<C, T>>,
    ) -> Result<Response, Error> {
        let auth = session
            .as_ref()
            .map(|jar| jar.authentication().unwrap_or(Authentication::None));
        let (url, headers) = self.endpoint.request(call, body, auth)?;
        let core = self.endpoint.core(&headers)?;
        let host = url.host_str().unwrap().trim_matches(['[', ']']);
        let stream =
            ::async_std::net::TcpStream::connect((host, url.port_or_known_default().unwrap()))
                .await?;
        if url.scheme() == "https" {
            let name = rustls::pki_types::ServerName::try_from(std::string::String::from(host))
                .map_err(|_| Error::InvalidOrigin)?;
            let tls = self.tls.connect(name, stream).await?;
            Box::pin(self.receive(&core, &mut FromFutures::new(tls), call, body, session)).await
        } else {
            Box::pin(self.receive(&core, &mut FromFutures::new(stream), call, body, session)).await
        }
    }
    async fn receive<IO: Read + Write, const C: usize, const T: usize>(
        &self,
        core: &crate::Client<'_>,
        stream: &mut IO,
        call: &Call,
        body: Option<Body<'_>>,
        session: Option<&mut Session<C, T>>,
    ) -> Result<Response, Error> {
        // The core parser requires caller-owned bounded storage. Reserve one
        // extra byte to detect an exact-limit response without replaying it.
        let mut headers = vec![0; 16384];
        let mut bytes = vec![0; self.limit + 1];
        let response = if let Some(session) = session {
            core.execute_with_session(stream, call, body, &mut headers, &mut bytes, session)
                .await
        } else {
            core.execute_with(stream, call, body, &mut headers, &mut bytes, |_, _| Ok(()))
                .await
        };
        let response = response.map_err(|error| match error {
            crate::Error::Transport(reqwless::Error::BufferTooSmall) => Error::ResponseTooLarge,
            crate::Error::UnsupportedEncoding => Error::UnsupportedEncoding,
            error => Error::Core(error),
        })?;
        let len = response.body.len();
        if len > self.limit {
            return Err(Error::ResponseTooLarge);
        }
        let status = response.status;
        let retry_after = response
            .retry_after
            .map(|value| std::string::String::from(value.as_str()));
        bytes.truncate(len);
        bytes.shrink_to_fit();
        Ok(Response {
            status,
            body: bytes,
            retry_after,
        })
    }
}
