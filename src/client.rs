use crate::{BuildError, Call, Json, JsonError, Method};
use core::fmt;
use embedded_io_async::{Read, Write};
use heapless::String;
use reqwless::{
    request::{Request, RequestBuilder},
    response,
};

/// Credentials are borrowed and deliberately redacted from debug output.
#[derive(Clone, Copy)]
pub enum Authentication<'a> {
    ApiKey(&'a str),
    Session {
        cookie: &'a str,
        csrf: Option<&'a str>,
    },
    /// For explicit login requests. Never selected as a fallback.
    None,
}
impl fmt::Debug for Authentication<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ApiKey(_) => "ApiKey([REDACTED])",
            Self::Session { .. } => "Session([REDACTED])",
            Self::None => "None",
        })
    }
}

/// A complete request body in caller-owned memory.
#[derive(Clone, Copy)]
pub enum Body<'a> {
    Json(&'a [u8]),
    Bytes {
        bytes: &'a [u8],
        content_type: &'a str,
    },
}
impl<'a> Body<'a> {
    pub fn bytes(self) -> &'a [u8] {
        match self {
            Self::Json(bytes) | Self::Bytes { bytes, .. } => bytes,
        }
    }
    pub fn content_type(self) -> &'a str {
        match self {
            Self::Json(_) => "application/json",
            Self::Bytes { content_type, .. } => content_type,
        }
    }
}

fn header_value(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| (32..127).contains(&b))
}

trait HeaderObserver {
    fn header(&mut self, name: &str, value: &[u8]) -> Result<(), BuildError>;
    fn complete(self);
}

struct HeaderCallback<F>(F);
impl<F: FnMut(&str, &[u8]) -> Result<(), BuildError>> HeaderObserver for HeaderCallback<F> {
    fn header(&mut self, name: &str, value: &[u8]) -> Result<(), BuildError> {
        (self.0)(name, value)
    }
    fn complete(self) {}
}

struct SessionHeaders<'a, const C: usize, const T: usize> {
    session: &'a mut crate::session::Session<C, T>,
    update: crate::session::ResponseUpdate<C, T>,
}
impl<const C: usize, const T: usize> HeaderObserver for SessionHeaders<'_, C, T> {
    fn header(&mut self, name: &str, value: &[u8]) -> Result<(), BuildError> {
        self.update.observe(name, value)
    }
    fn complete(self) {
        *self.session = self.update.finish();
    }
}

#[derive(Debug)]
pub enum Error {
    Build(BuildError),
    Transport(reqwless::Error),
    Json(JsonError),
    UnsupportedEncoding,
    InvalidResponse,
}
impl From<BuildError> for Error {
    fn from(value: BuildError) -> Self {
        Self::Build(value)
    }
}
impl From<reqwless::Error> for Error {
    fn from(value: reqwless::Error) -> Self {
        Self::Transport(value)
    }
}
impl From<JsonError> for Error {
    fn from(value: JsonError) -> Self {
        Self::Json(value)
    }
}

/// HTTP errors preserve the original body, including non-JSON proxy errors.
#[derive(Clone, Debug)]
pub struct HttpError<'a> {
    pub status: u16,
    pub body: &'a [u8],
    /// Preserved when `Response::success` converts a 429/503 into an error.
    pub retry_after: Option<String<128>>,
}
#[derive(Debug)]
pub struct Response<'a> {
    pub status: u16,
    pub body: &'a [u8],
    /// Retry-After is preserved as either seconds or an HTTP date. No automatic retries occur.
    pub retry_after: Option<String<128>>,
}
impl<'a> Response<'a> {
    // Error metadata lives inline because this crate has no allocator.
    #[allow(clippy::result_large_err)]
    pub fn success(self) -> Result<Self, HttpError<'a>> {
        if (200..300).contains(&self.status) {
            Ok(self)
        } else {
            Err(HttpError {
                status: self.status,
                body: self.body,
                retry_after: self.retry_after,
            })
        }
    }
    pub fn json(&self) -> Result<Json<'a>, JsonError> {
        Json::parse(self.body)
    }
}

/// Authentication and routing for a single UniFi API surface.
///
/// Pass an established, authenticated TLS stream (or a local test stream) to `execute`.
/// Each call requests `Connection: close`; reconnect before executing another call.
pub struct Client<'a> {
    pub(crate) host: &'a str,
    pub(crate) base: &'a str,
    pub(crate) authentication: Authentication<'a>,
}
impl fmt::Debug for Client<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("host", &self.host)
            .field("base", &self.base)
            .field("authentication", &self.authentication)
            .finish()
    }
}
impl<'a> Client<'a> {
    /// `base` is `/proxy/network/integration` locally, or empty for Site Manager.
    pub fn new(host: &'a str, base: &'a str, key: &'a str) -> Result<Self, BuildError> {
        Self::with_authentication(host, base, Authentication::ApiKey(key))
    }
    pub fn with_authentication(
        host: &'a str,
        base: &'a str,
        authentication: Authentication<'a>,
    ) -> Result<Self, BuildError> {
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| (33..127).contains(&b) && !b"/?#@".contains(&b))
        {
            return Err(BuildError::InvalidHeader);
        }
        match authentication {
            Authentication::ApiKey(key) if !header_value(key) || key.contains(' ') => {
                return Err(BuildError::InvalidHeader);
            }
            Authentication::Session { cookie, csrf }
                if !header_value(cookie) || csrf.is_some_and(|v| !header_value(v)) =>
            {
                return Err(BuildError::InvalidHeader);
            }
            _ => {}
        }
        if (!base.is_empty() && !base.starts_with('/'))
            || base.contains(['?', '#'])
            || base.bytes().any(|b| b <= 32 || b >= 127)
            || base.split('/').any(|s| s == "." || s == "..")
        {
            return Err(BuildError::InvalidBasePath);
        }
        Ok(Self {
            host,
            base: base.trim_end_matches('/'),
            authentication,
        })
    }
    /// Execute without redirects or retries. Required parameters and JSON bodies are checked before I/O.
    pub async fn execute<'b, IO: Read + Write>(
        &self,
        io: &mut IO,
        call: &Call,
        body: Option<&[u8]>,
        headers: &mut [u8],
        output: &'b mut [u8],
    ) -> Result<Response<'b>, Error> {
        self.execute_with(io, call, body.map(Body::Json), headers, output, |_, _| {
            Ok(())
        })
        .await
    }

    /// Execute and atomically apply cookie/CSRF response headers to a session jar.
    /// Borrow outgoing authentication from a separate credential snapshot of the jar.
    /// Validated headers are committed even if reading the response body fails.
    pub async fn execute_with_session<'b, IO: Read + Write, const C: usize, const T: usize>(
        &self,
        io: &mut IO,
        call: &Call,
        body: Option<Body<'_>>,
        headers: &mut [u8],
        output: &'b mut [u8],
        session: &mut crate::session::Session<C, T>,
    ) -> Result<Response<'b>, Error> {
        let update = session.response_update();
        self.execute_observed(
            io,
            call,
            body,
            headers,
            output,
            SessionHeaders { session, update },
        )
        .await
    }

    /// Execute a JSON, binary or multipart request. Observe response headers before reading its body.
    /// The observer can update a host-scoped session cookie jar. No credentials are retried or replayed.
    /// Use `execute_with_session` to commit related cookie/CSRF headers atomically.
    pub async fn execute_with<'b, IO: Read + Write>(
        &self,
        io: &mut IO,
        call: &Call,
        body: Option<Body<'_>>,
        headers: &mut [u8],
        output: &'b mut [u8],
        on_header: impl FnMut(&str, &[u8]) -> Result<(), BuildError>,
    ) -> Result<Response<'b>, Error> {
        self.execute_observed(io, call, body, headers, output, HeaderCallback(on_header))
            .await
    }

    async fn execute_observed<'b, IO: Read + Write>(
        &self,
        io: &mut IO,
        call: &Call,
        body: Option<Body<'_>>,
        headers: &mut [u8],
        output: &'b mut [u8],
        mut observer: impl HeaderObserver,
    ) -> Result<Response<'b>, Error> {
        call.validate(body.map(Body::bytes))?;
        if let Some(Body::Json(bytes)) = body {
            Json::parse(bytes)?;
        }
        if body.is_some_and(|b| !header_value(b.content_type())) {
            return Err(BuildError::InvalidHeader.into());
        }
        let mut path = String::<8192>::new();
        path.push_str(self.base).map_err(|_| BuildError::Capacity)?;
        path.push_str(call.path())
            .map_err(|_| BuildError::Capacity)?;
        let mut extra = heapless::Vec::<(&str, &str), 6>::new();
        match self.authentication {
            Authentication::ApiKey(key) => {
                let _ = extra.push(("X-API-Key", key));
            }
            Authentication::Session { cookie, csrf } => {
                let _ = extra.push(("Cookie", cookie));
                if let Some(value) = csrf {
                    let _ = extra.push(("X-CSRF-Token", value));
                }
            }
            Authentication::None => {}
        }
        let _ = extra.push(("Connection", "close"));
        let _ = extra.push(("Accept-Encoding", "identity"));
        let _ = extra.push(("Accept", "*/*"));
        if let Some(body) = body {
            let _ = extra.push(("Content-Type", body.content_type()));
        }
        let request = Request::new(call.operation.method, &path)
            .host(self.host)
            .headers(&extra);
        if let Some(body) = body {
            let bytes = body.bytes();
            request.body(bytes).build().write_header(io).await?;
            io.write_all(bytes)
                .await
                .map_err(|e| reqwless::Error::Network(embedded_io::Error::kind(&e)))?;
        } else {
            request.build().write_header(io).await?;
        }
        io.flush()
            .await
            .map_err(|e| reqwless::Error::Network(embedded_io::Error::kind(&e)))?;
        let response = response::Response::read(io, call.operation.method, headers).await?;
        let status = response.status.0;
        for (name, value) in response.headers() {
            observer.header(name, value)?;
        }
        observer.complete();
        let mut retry_after = None;
        let mut seen_length = None;
        let mut seen_transfer = false;
        for (name, value) in response.headers() {
            if name.eq_ignore_ascii_case("content-length") {
                let length = core::str::from_utf8(value)
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
                    .ok_or(Error::InvalidResponse)?;
                if seen_length.is_some_and(|previous| previous != length) {
                    return Err(Error::InvalidResponse);
                }
                seen_length = Some(length);
            }
            if name.eq_ignore_ascii_case("transfer-encoding") {
                if seen_transfer || !value.eq_ignore_ascii_case(b"chunked") {
                    return Err(Error::UnsupportedEncoding);
                }
                seen_transfer = true;
            }
            if name.eq_ignore_ascii_case("content-encoding")
                && !value.eq_ignore_ascii_case(b"identity")
            {
                return Err(Error::UnsupportedEncoding);
            }
            if name.eq_ignore_ascii_case("retry-after") {
                retry_after = core::str::from_utf8(value)
                    .ok()
                    .and_then(|v| String::try_from(v).ok());
            }
        }
        if seen_length.is_some() && seen_transfer {
            return Err(Error::InvalidResponse);
        }
        // RFC 9110: 204 and 304 responses never have a message body, even without Content-Length.
        let n = if status == 204 || status == 304 || call.operation.method == Method::HEAD {
            0
        } else {
            response.body().reader().read_to_end(output).await?
        };
        Ok(Response {
            status,
            body: &output[..n],
            retry_after,
        })
    }
    /// Serialize a Rust request model into caller-owned memory.
    pub fn encode<'b, T: crate::json::Encode + ?Sized>(
        value: &T,
        output: &'b mut [u8],
    ) -> Result<&'b [u8], Error> {
        let n = crate::json::encode_to_slice(value, output)?;
        Ok(&output[..n])
    }
}
