use super::{Endpoint, Error, Limits, Response, check_encoding, metadata};
use crate::{Authentication, Body, Call, session::Session};
use core::future::Future;
use http::{HeaderMap, Method};
use std::{io::Read, time::Duration, vec::Vec};

/// A validated, owned HTTP request. Its headers include the selected authentication.
/// Transport implementations must not log credential headers or request bodies.
pub type Request = http::Request<Vec<u8>>;

/// Application-selected blocking HTTP and TLS backend.
///
/// Return after receiving headers, before buffering the body. Send the request once,
/// without following redirects, retrying it, decompressing it, or adding credentials
/// from an independent cookie jar. Verify the target hostname and certificate for
/// HTTPS. Enforce `timeout` across connection setup, headers and body reads.
pub trait BlockingTransport {
    type Body: Read;
    fn send(
        &self,
        request: Request,
        timeout: Duration,
    ) -> Result<http::Response<Self::Body>, Error>;
}

/// Streaming body returned by an application-selected async HTTP backend.
pub trait AsyncBody: Send {
    type Chunk: AsRef<[u8]> + Send;
    /// Return the next body chunk, or `None` at EOF. Body errors retain already
    /// observed session updates. Dropping this body must cancel its pending I/O.
    fn next_chunk(&mut self) -> impl Future<Output = Result<Option<Self::Chunk>, Error>> + Send;
}

/// Application-selected async HTTP and TLS backend, independent of the executor.
///
/// This has the same origin, TLS, deadline and single-send contract as
/// [`BlockingTransport`]. Return headers before reading the body. Futures are
/// `Send` so an SDK call can be spawned on a multithreaded executor.
pub trait AsyncTransport: Sync {
    type Body: AsyncBody;
    fn send(
        &self,
        request: Request,
        timeout: Duration,
    ) -> impl Future<Output = Result<http::Response<Self::Body>, Error>> + Send;
}

/// Owned UniFi request/session handling over a chosen blocking HTTP backend.
pub struct BlockingClient<T> {
    endpoint: Endpoint,
    transport: T,
    limits: Limits,
}
/// Owned UniFi request/session handling over a chosen async HTTP backend.
pub struct AsyncClient<T> {
    endpoint: Endpoint,
    transport: T,
    limits: Limits,
}

macro_rules! common {
    ($client:ident) => {
        impl<T> $client<T> {
            pub fn with_transport(
                origin: &str,
                base: &str,
                auth: Authentication<'_>,
                transport: T,
                limits: Limits,
            ) -> Result<Self, Error> {
                Ok(Self {
                    endpoint: Endpoint::new(origin, base, auth)?,
                    transport,
                    limits: limits.validate()?,
                })
            }
            pub fn transport(&self) -> &T {
                &self.transport
            }
            pub fn into_transport(self) -> T {
                self.transport
            }
        }
        impl<T> core::fmt::Debug for $client<T> {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.debug_struct(stringify!($client))
                    .field("authentication", &"[REDACTED]")
                    .finish_non_exhaustive()
            }
        }
    };
}
common!(BlockingClient);
common!(AsyncClient);

fn request(
    endpoint: &Endpoint,
    call: &Call,
    body: Option<Body<'_>>,
    auth: Option<Authentication<'_>>,
) -> Result<Request, Error> {
    let (url, headers) = endpoint.request(call, body, auth)?;
    let mut request = Request::new(body.map(|body| body.bytes().to_vec()).unwrap_or_default());
    *request.method_mut() = Method::from_bytes(call.operation.method.as_str().as_bytes())
        .map_err(|_| crate::BuildError::InvalidParameter)?;
    *request.uri_mut() = url
        .as_str()
        .parse()
        .map_err(|_| crate::BuildError::InvalidParameter)?;
    *request.headers_mut() = headers;
    Ok(request)
}

fn headers<const C: usize, const T: usize>(
    status: u16,
    headers: &HeaderMap,
    session: Option<&mut Session<C, T>>,
) -> Result<Response, Error> {
    if let Some(session) = session {
        session.observe_response(
            headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_bytes())),
        )?;
    }
    check_encoding(headers)?;
    Ok(metadata(status, headers))
}

impl<T: BlockingTransport> BlockingClient<T> {
    pub fn execute(&self, call: &Call, body: Option<&[u8]>) -> Result<Response, Error> {
        self.execute_body(call, body.map(Body::Json))
    }
    pub fn execute_body(&self, call: &Call, body: Option<Body<'_>>) -> Result<Response, Error> {
        self.execute_inner::<4096, 512>(call, body, None)
    }
    /// Send the jar's credentials and atomically rotate it when headers arrive,
    /// including when body reading subsequently fails. Keep each jar origin-scoped.
    pub fn execute_with_session<const C: usize, const S: usize>(
        &self,
        call: &Call,
        body: Option<Body<'_>>,
        session: &mut Session<C, S>,
    ) -> Result<Response, Error> {
        self.execute_inner(call, body, Some(session))
    }
    fn execute_inner<const C: usize, const S: usize>(
        &self,
        call: &Call,
        body: Option<Body<'_>>,
        session: Option<&mut Session<C, S>>,
    ) -> Result<Response, Error> {
        let auth = session
            .as_ref()
            .map(|jar| jar.authentication().unwrap_or(Authentication::None));
        let request = request(&self.endpoint, call, body, auth)?;
        let reply = self.transport.send(request, self.limits.timeout)?;
        let (parts, body) = reply.into_parts();
        let mut response = headers(parts.status.as_u16(), &parts.headers, session)?;
        body.take(self.limits.max_response_bytes as u64 + 1)
            .read_to_end(&mut response.body)?;
        if response.body.len() > self.limits.max_response_bytes {
            return Err(Error::ResponseTooLarge);
        }
        Ok(response)
    }
}

impl<T: AsyncTransport> AsyncClient<T> {
    pub async fn execute(&self, call: &Call, body: Option<&[u8]>) -> Result<Response, Error> {
        self.execute_body(call, body.map(Body::Json)).await
    }
    pub async fn execute_body(
        &self,
        call: &Call,
        body: Option<Body<'_>>,
    ) -> Result<Response, Error> {
        self.execute_inner::<4096, 512>(call, body, None).await
    }
    /// Send the jar's credentials and atomically rotate it before polling the
    /// response body. Cancellation after headers retains the rotated session.
    pub async fn execute_with_session<const C: usize, const S: usize>(
        &self,
        call: &Call,
        body: Option<Body<'_>>,
        session: &mut Session<C, S>,
    ) -> Result<Response, Error> {
        self.execute_inner(call, body, Some(session)).await
    }
    async fn execute_inner<const C: usize, const S: usize>(
        &self,
        call: &Call,
        body: Option<Body<'_>>,
        session: Option<&mut Session<C, S>>,
    ) -> Result<Response, Error> {
        let auth = session
            .as_ref()
            .map(|jar| jar.authentication().unwrap_or(Authentication::None));
        let request = request(&self.endpoint, call, body, auth)?;
        let reply = self.transport.send(request, self.limits.timeout).await?;
        let (parts, mut body) = reply.into_parts();
        let mut response = headers(parts.status.as_u16(), &parts.headers, session)?;
        while let Some(chunk) = body.next_chunk().await? {
            let chunk = chunk.as_ref();
            if chunk.len() > self.limits.max_response_bytes - response.body.len() {
                return Err(Error::ResponseTooLarge);
            }
            response.body.extend_from_slice(chunk);
        }
        Ok(response)
    }
}
