//! Shared options, owned responses and errors for the optional host clients.
use crate::{Authentication, Body, BuildError, Call, Json, JsonError};
use http::header::{HeaderMap, HeaderValue};
use std::{
    format,
    string::{String, ToString},
    time::Duration,
    vec::Vec,
};
use url::Url;

/// Host TLS roots and resource limits. Certificate verification is always enabled.
#[derive(Clone, Debug)]
pub struct Options {
    /// Overall request timeout, including the response body (default 30 seconds).
    pub timeout: Duration,
    /// Maximum buffered response body (default 16 MiB); excess data is an error.
    pub max_response_bytes: usize,
    /// Additional trusted PEM/DER roots, such as a controller's private CA.
    pub root_certificates: Vec<Certificate>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_response_bytes: 16 * 1024 * 1024,
            root_certificates: Vec::new(),
        }
    }
}
/// An additional DER or PEM trust anchor, shared by all host transports.
#[derive(Clone, Debug)]
pub struct Certificate(pub(crate) rustls::pki_types::CertificateDer<'static>);
impl Certificate {
    pub fn from_der(bytes: &[u8]) -> Result<Self, Error> {
        let der = rustls::pki_types::CertificateDer::from(bytes.to_vec());
        rustls::RootCertStore::empty()
            .add(der.clone())
            .map_err(|_| Error::InvalidCertificate)?;
        Ok(Self(der))
    }
    pub fn from_pem(bytes: &[u8]) -> Result<Self, Error> {
        use rustls::pki_types::pem::PemObject;
        let der = rustls::pki_types::CertificateDer::from_pem_slice(bytes)
            .map_err(|_| Error::InvalidCertificate)?;
        Self::from_der(der.as_ref())
    }
}

#[cfg(any(feature = "std", feature = "tokio"))]
pub(crate) fn reqwest_roots(roots: Vec<Certificate>) -> Result<Vec<reqwest::Certificate>, Error> {
    roots
        .into_iter()
        .map(|cert| reqwest::Certificate::from_der(cert.0.as_ref()).map_err(Error::Transport))
        .collect()
}

#[derive(Debug)]
pub enum Error {
    Core(crate::Error),
    #[cfg(any(feature = "std", feature = "tokio"))]
    Transport(reqwest::Error),
    Io(std::io::Error),
    InvalidOrigin,
    ResponseTooLarge,
    UnsupportedEncoding,
    InvalidCertificate,
    Tls(rustls::Error),
    Timeout,
}
impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Core(e) => write!(f, "invalid UniFi request or session: {e:?}"),
            #[cfg(any(feature = "std", feature = "tokio"))]
            Self::Transport(e) => write!(f, "HTTP transport: {e}"),
            Self::Io(e) => write!(f, "response I/O: {e}"),
            Self::InvalidOrigin => f.write_str(
                "expected an http(s) origin without credentials, path, query or fragment",
            ),
            Self::ResponseTooLarge => f.write_str("response exceeds configured body limit"),
            Self::UnsupportedEncoding => f.write_str("unsupported response encoding"),
            Self::InvalidCertificate => f.write_str("invalid trust anchor certificate"),
            Self::Tls(e) => write!(f, "TLS configuration: {e}"),
            Self::Timeout => f.write_str("request timed out"),
        }
    }
}
impl std::error::Error for Error {}
impl From<crate::Error> for Error {
    fn from(e: crate::Error) -> Self {
        Self::Core(e)
    }
}
impl From<BuildError> for Error {
    fn from(e: BuildError) -> Self {
        Self::Core(e.into())
    }
}
#[cfg(any(feature = "std", feature = "tokio"))]
impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Self::Transport(e)
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// An owned response; its JSON/model views borrow from this value.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub retry_after: Option<String>,
}
impl Response {
    pub fn json(&self) -> Result<Json<'_>, JsonError> {
        Json::parse(&self.body)
    }
    pub fn success(self) -> Result<Self, HttpError> {
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
}
/// Non-success statuses retain the complete body and Retry-After metadata.
#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub body: Vec<u8>,
    pub retry_after: Option<String>,
}
impl core::fmt::Display for HttpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "UniFi HTTP {}", self.status)
    }
}
impl std::error::Error for HttpError {}

// No Debug implementation: this structure owns credentials.
pub(crate) struct Endpoint {
    origin: Url,
    #[cfg(feature = "async_std")]
    authority: String,
    base: String,
    auth: HeaderMap,
}
impl Endpoint {
    pub fn new(origin: &str, base: &str, auth: Authentication<'_>) -> Result<Self, Error> {
        let url = Url::parse(origin).map_err(|_| Error::InvalidOrigin)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::InvalidOrigin);
        }
        // Use the embedded client's exact base-path and credential validation.
        crate::Client::with_authentication(url.host_str().unwrap(), base, auth)?;
        Ok(Self {
            #[cfg(feature = "async_std")]
            authority: match url.port() {
                Some(port) => format!("{}:{port}", url.host_str().unwrap()),
                None => url.host_str().unwrap().to_string(),
            },
            origin: url,
            base: base.trim_end_matches('/').to_string(),
            auth: authentication_headers(auth)?,
        })
    }
    #[cfg(feature = "async_std")]
    pub fn core<'a>(&'a self, headers: &'a HeaderMap) -> Result<crate::Client<'a>, Error> {
        let auth = if let Some(key) = headers.get("x-api-key") {
            Authentication::ApiKey(key.to_str().map_err(|_| BuildError::InvalidHeader)?)
        } else if let Some(cookie) = headers.get("cookie") {
            Authentication::Session {
                cookie: cookie.to_str().map_err(|_| BuildError::InvalidHeader)?,
                csrf: headers
                    .get("x-csrf-token")
                    .map(|v| v.to_str())
                    .transpose()
                    .map_err(|_| BuildError::InvalidHeader)?,
            }
        } else {
            Authentication::None
        };
        Ok(crate::Client::with_authentication(
            &self.authority,
            &self.base,
            auth,
        )?)
    }
    pub fn request(
        &self,
        call: &Call,
        body: Option<Body<'_>>,
        auth: Option<Authentication<'_>>,
    ) -> Result<(Url, HeaderMap), Error> {
        crate::client::validate_request(call, body)?;
        let path = format!("{}{}", self.base, call.path());
        let url = Url::parse(&format!(
            "{}{}",
            self.origin.as_str().trim_end_matches('/'),
            path
        ))
        .map_err(|_| BuildError::InvalidParameter)?;
        // URL parsers normalize even percent-encoded dot segments. Never silently
        // send a valid bound call to a different resource or authority.
        let actual = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_string(),
        };
        if actual != path || url.origin() != self.origin.origin() {
            return Err(BuildError::InvalidParameter.into());
        }
        let mut headers = match auth {
            Some(auth) => authentication_headers(auth)?,
            None => self.auth.clone(),
        };
        headers.insert("accept", HeaderValue::from_static("*/*"));
        headers.insert("accept-encoding", HeaderValue::from_static("identity"));
        if let Some(body) = body {
            headers.insert(
                "content-type",
                HeaderValue::from_str(body.content_type())
                    .map_err(|_| BuildError::InvalidHeader)?,
            );
        }
        Ok((url, headers))
    }
}
fn authentication_headers(auth: Authentication<'_>) -> Result<HeaderMap, BuildError> {
    // Also checks caller-provided session snapshots before host I/O.
    crate::Client::with_authentication("localhost", "", auth)?;
    let mut headers = HeaderMap::new();
    let mut insert = |key: &'static str, value: &str| {
        let mut value = HeaderValue::from_str(value).map_err(|_| BuildError::InvalidHeader)?;
        value.set_sensitive(true);
        headers.insert(key, value);
        Ok(())
    };
    match auth {
        Authentication::ApiKey(key) => insert("x-api-key", key)?,
        Authentication::Session { cookie, csrf } => {
            insert("cookie", cookie)?;
            if let Some(csrf) = csrf {
                insert("x-csrf-token", csrf)?;
            }
        }
        Authentication::None => {}
    }
    Ok(headers)
}
#[cfg(any(feature = "std", feature = "tokio"))]
pub(crate) fn check_encoding(headers: &HeaderMap) -> Result<(), Error> {
    if headers
        .get_all("content-encoding")
        .iter()
        .any(|v| !v.as_bytes().eq_ignore_ascii_case(b"identity"))
    {
        return Err(Error::UnsupportedEncoding);
    }
    Ok(())
}
#[cfg(any(feature = "std", feature = "tokio"))]
pub(crate) fn metadata(status: u16, headers: &HeaderMap) -> Response {
    Response {
        status,
        body: Vec::new(),
        retry_after: headers
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string),
    }
}
