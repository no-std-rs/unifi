//! Explicit, host-scoped session state for the undocumented Network API.
//! Keep one instance per origin; do not share its credentials between controllers.
use crate::{Authentication, BuildError, Json, JsonError};
use heapless::String;

/// Captures cookies and CSRF tokens without storing passwords or replaying requests.
/// This clock-free jar handles explicit deletion; wall-clock expiry is caller-managed.
#[derive(Clone)]
pub struct Session<const COOKIE: usize = 4096, const CSRF: usize = 512> {
    cookie: String<COOKIE>,
    csrf: String<CSRF>,
}
impl<const C: usize, const T: usize> Default for Session<C, T> {
    fn default() -> Self {
        Self {
            cookie: String::new(),
            csrf: String::new(),
        }
    }
}
impl<const C: usize, const T: usize> core::fmt::Debug for Session<C, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Session([REDACTED])")
    }
}
impl<const C: usize, const T: usize> Session<C, T> {
    /// Restore one origin's caller-owned cookie/CSRF pair before observing a
    /// response. Never combine tokens from different sessions or controllers.
    pub fn from_cookie(cookie: &str, csrf: Option<&str>) -> Result<Self, BuildError> {
        if !cookie.bytes().all(|b| (32..127).contains(&b)) {
            return Err(BuildError::InvalidHeader);
        }
        let mut session = Self::default();
        for pair in cookie.split(';') {
            if pair.trim().is_empty() {
                return Err(BuildError::InvalidHeader);
            }
            session.observe("Set-Cookie", pair.as_bytes())?;
        }
        if let Some(csrf) = csrf {
            session.observe("X-CSRF-Token", csrf.as_bytes())?;
        }
        if session.authentication().is_none() {
            return Err(BuildError::InvalidHeader);
        }
        Ok(session)
    }
    pub fn clear(&mut self) {
        self.cookie.clear();
        self.csrf.clear();
    }
    pub fn authentication(&self) -> Option<Authentication<'_>> {
        if self.cookie.is_empty() {
            return None;
        }
        Some(Authentication::Session {
            cookie: &self.cookie,
            csrf: (!self.csrf.is_empty()).then_some(self.csrf.as_str()),
        })
    }
    /// Apply an isolated header. For a complete response use `observe_response` or
    /// `Client::execute_with_session`, which apply related headers atomically.
    /// Capacity errors are explicit; cookie replacement is transactional.
    /// If the client authenticates with this jar, first clone it into a credential snapshot
    /// and borrow the client's authentication from that snapshot. The response observer
    /// can then mutate the original jar while the outgoing credentials remain stable.
    /// This copies bounded, inline buffers and does not allocate.
    pub fn observe(&mut self, name: &str, bytes: &[u8]) -> Result<(), BuildError> {
        self.apply_header(name, bytes, true).map(|_| ())
    }

    /// Apply one response's headers together, independently of cookie/CSRF header order.
    /// Invalid or oversized values leave the original jar unchanged.
    /// Removing a stored cookie invalidates CSRF unless the response supplies a fresh token.
    pub fn observe_response<'h>(
        &mut self,
        headers: impl IntoIterator<Item = (&'h str, &'h [u8])>,
    ) -> Result<(), BuildError> {
        let mut pending = self.response_update();
        for (name, value) in headers {
            pending.observe(name, value)?;
        }
        *self = pending.finish();
        Ok(())
    }

    pub(crate) fn response_update(&self) -> ResponseUpdate<C, T> {
        ResponseUpdate {
            pending: self.clone(),
            csrf_invalidated: false,
            csrf_updated: false,
        }
    }

    fn apply_header(
        &mut self,
        name: &str,
        bytes: &[u8],
        clear_empty: bool,
    ) -> Result<bool, BuildError> {
        let mut removed = false;
        if name.eq_ignore_ascii_case("x-csrf-token")
            || name.eq_ignore_ascii_case("x-updated-csrf-token")
        {
            let value = core::str::from_utf8(bytes).map_err(|_| BuildError::InvalidHeader)?;
            if value.is_empty() || !value.bytes().all(|b| (33..127).contains(&b)) {
                return Err(BuildError::InvalidHeader);
            }
            self.csrf = String::try_from(value).map_err(|_| BuildError::Capacity)?;
        } else if name.eq_ignore_ascii_case("set-cookie") {
            let value = core::str::from_utf8(bytes).map_err(|_| BuildError::InvalidHeader)?;
            let mut attributes = value.split(';');
            let pair = attributes.next().ok_or(BuildError::InvalidHeader)?.trim();
            let (key, val) = pair.split_once('=').ok_or(BuildError::InvalidHeader)?;
            let unquoted = val
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(val);
            if key.is_empty()
                || !key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                || !unquoted
                    .bytes()
                    .all(|b| (33..127).contains(&b) && !b";\"\\,".contains(&b))
            {
                return Err(BuildError::InvalidHeader);
            }
            let remove = unquoted.is_empty()
                || attributes.any(|v| {
                    v.trim().split_once('=').is_some_and(|(k, v)| {
                        k.eq_ignore_ascii_case("max-age")
                            && v.trim().parse::<i64>().is_ok_and(|n| n <= 0)
                    })
                });
            let mut cookie = String::<C>::new();
            for old in self.cookie.split("; ").filter(|s| !s.is_empty()) {
                if old.split_once('=').is_some_and(|(k, _)| k == key) {
                    removed |= remove;
                    continue;
                }
                if !cookie.is_empty() {
                    cookie.push_str("; ").map_err(|_| BuildError::Capacity)?;
                }
                cookie.push_str(old).map_err(|_| BuildError::Capacity)?;
            }
            if !remove {
                if !cookie.is_empty() {
                    cookie.push_str("; ").map_err(|_| BuildError::Capacity)?;
                }
                cookie.push_str(pair).map_err(|_| BuildError::Capacity)?;
            }
            self.cookie = cookie;
            if clear_empty && (removed || self.cookie.is_empty()) {
                self.csrf.clear();
            }
        }
        Ok(removed)
    }
}

pub(crate) struct ResponseUpdate<const C: usize, const T: usize> {
    pending: Session<C, T>,
    csrf_invalidated: bool,
    csrf_updated: bool,
}

impl<const C: usize, const T: usize> ResponseUpdate<C, T> {
    pub(crate) fn observe(&mut self, name: &str, value: &[u8]) -> Result<(), BuildError> {
        self.csrf_invalidated |= self.pending.apply_header(name, value, false)?;
        self.csrf_updated |= name.eq_ignore_ascii_case("x-csrf-token")
            || name.eq_ignore_ascii_case("x-updated-csrf-token");
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Session<C, T> {
        if self.pending.cookie.is_empty() || (self.csrf_invalidated && !self.csrf_updated) {
            self.pending.csrf.clear();
        }
        self.pending
    }
}

/// Encode a login payload. TOTP is supplied fresh by the caller, never generated or retained.
pub fn login_body<'a>(
    output: &'a mut [u8],
    username: &str,
    password: &str,
    totp: Option<&str>,
) -> Result<&'a [u8], JsonError> {
    let mut writer = crate::json::ObjectWriter::new(output)?;
    writer.field("username", &username)?;
    writer.field("password", &password)?;
    if let Some(code) = totp {
        if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(JsonError::Type);
        }
        writer.field("ubic_2fa_token", &code)?;
    }
    writer.finish()
}

/// A legacy API can return HTTP 200 with `meta.rc = error`; validate the envelope separately.
pub fn data(response: Json<'_>) -> Result<Json<'_>, JsonError> {
    if !response
        .required("meta")?
        .required("rc")?
        .string()?
        .equals("ok")
    {
        return Err(JsonError::Type);
    }
    response.required("data")
}
