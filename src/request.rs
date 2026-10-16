use crate::Method;
use core::fmt::Write;
use heapless::String;

/// Upper bound for an encoded path and query. Bodies use caller-owned buffers.
pub const PATH_CAPACITY: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildError {
    Capacity,
    MissingParameter,
    InvalidParameter,
    InvalidHeader,
    InvalidBasePath,
    InvalidBody,
}

#[derive(Clone, Copy, Debug)]
pub struct QueryParameter {
    pub name: &'static str,
    pub required: bool,
}

/// An operation from a pinned official OpenAPI document.
#[derive(Clone, Copy, Debug)]
pub struct Operation {
    pub id: &'static str,
    pub method: Method,
    pub path: &'static str,
    pub summary: &'static str,
    pub body_required: bool,
    pub query: &'static [QueryParameter],
    pub request_schema: Option<&'static str>,
    pub response_schema: Option<&'static str>,
}

impl Operation {
    /// Bind path parameters by their exact OpenAPI names. Values are percent-encoded.
    pub fn bind(&'static self, params: &[(&str, &str)]) -> Result<Call, BuildError> {
        if !self.path.starts_with('/')
            || self
                .path
                .bytes()
                .any(|b| !(33..127).contains(&b) || b == b'?' || b == b'#')
        {
            return Err(BuildError::InvalidParameter);
        }
        let mut path = String::new();
        let mut rest = self.path;
        while !rest.is_empty() {
            if let Some(tail) = rest.strip_prefix('{') {
                let (key, tail) = tail.split_once('}').ok_or(BuildError::InvalidParameter)?;
                let value = find(params, key)?;
                if value.is_empty() {
                    return Err(BuildError::InvalidParameter);
                }
                encode(&mut path, value, false)?;
                rest = tail;
            } else if let Some(key) = rest.strip_prefix('*') {
                let value = find(params, key)?;
                // A connector path consists of individually encoded segments.
                for (i, segment) in value.trim_start_matches('/').split('/').enumerate() {
                    if segment.is_empty() {
                        return Err(BuildError::InvalidParameter);
                    }
                    if i > 0 {
                        path.push('/').map_err(|_| BuildError::Capacity)?;
                    }
                    encode(&mut path, segment, false)?;
                }
                rest = "";
            } else {
                let count = rest.find(['{', '*']).unwrap_or(rest.len());
                path.push_str(&rest[..count])
                    .map_err(|_| BuildError::Capacity)?;
                rest = &rest[count..];
            }
        }
        // Reject unused and duplicate parameters: a typo must never select a different resource.
        for (i, (key, _)) in params.iter().enumerate() {
            if params[..i].iter().any(|(k, _)| k == key)
                || !(self.path.contains(format_key(key)?.as_str())
                    || self
                        .path
                        .strip_suffix(key)
                        .is_some_and(|p| p.ends_with('*')))
            {
                return Err(BuildError::InvalidParameter);
            }
        }
        Ok(Call {
            operation: self,
            path,
            raw: false,
        })
    }
}

fn format_key(key: &str) -> Result<String<128>, BuildError> {
    let mut out = String::new();
    write!(out, "{{{key}}}").map_err(|_| BuildError::Capacity)?;
    Ok(out)
}

fn find<'a>(params: &[(&str, &'a str)], name: &str) -> Result<&'a str, BuildError> {
    params
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| *value)
        .ok_or(BuildError::MissingParameter)
}

/// A prepared request. Construction never performs network I/O.
#[derive(Clone, Debug)]
pub struct Call {
    pub operation: &'static Operation,
    path: String<PATH_CAPACITY>,
    raw: bool,
}

const fn raw_operation(method: Method) -> Operation {
    Operation {
        id: "$raw",
        method,
        path: "/",
        summary: "Explicit raw request",
        body_required: false,
        query: &[],
        request_schema: None,
        response_schema: None,
    }
}
static RAW_GET: Operation = raw_operation(Method::GET);
static RAW_POST: Operation = raw_operation(Method::POST);
static RAW_PUT: Operation = raw_operation(Method::PUT);
static RAW_PATCH: Operation = raw_operation(Method::PATCH);
static RAW_DELETE: Operation = raw_operation(Method::DELETE);
static RAW_HEAD: Operation = raw_operation(Method::HEAD);
static RAW_OPTIONS: Operation = raw_operation(Method::OPTIONS);

impl Call {
    /// Explicit escape hatch for firmware-specific endpoints. `path` is an already encoded,
    /// origin-relative URI; credentials always remain on the client's configured host.
    pub fn raw(method: Method, path: &str) -> Result<Self, BuildError> {
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.contains('#')
            || path.bytes().any(|b| !(33..127).contains(&b))
        {
            return Err(BuildError::InvalidParameter);
        }
        let operation = match method {
            Method::GET => &RAW_GET,
            Method::POST => &RAW_POST,
            Method::PUT => &RAW_PUT,
            Method::PATCH => &RAW_PATCH,
            Method::DELETE => &RAW_DELETE,
            Method::HEAD => &RAW_HEAD,
            Method::OPTIONS => &RAW_OPTIONS,
            _ => return Err(BuildError::InvalidParameter),
        };
        Ok(Self {
            operation,
            path: String::try_from(path).map_err(|_| BuildError::Capacity)?,
            raw: true,
        })
    }
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Append a supported query parameter. Repeated parameters are preserved.
    pub fn query(mut self, name: &str, value: &str) -> Result<Self, BuildError> {
        if name.is_empty() || (!self.raw && !self.operation.query.iter().any(|p| p.name == name)) {
            return Err(BuildError::InvalidParameter);
        }
        let original = self.path.len();
        let result = (|| {
            self.path
                .push(if self.path.contains('?') { '&' } else { '?' })
                .map_err(|_| BuildError::Capacity)?;
            encode(&mut self.path, name, true)?;
            self.path.push('=').map_err(|_| BuildError::Capacity)?;
            encode(&mut self.path, value, true)
        })();
        if result.is_err() {
            self.path.truncate(original);
        }
        result?;
        Ok(self)
    }

    /// Replace every occurrence of one query parameter while retaining other repeated parameters.
    pub fn set_query(mut self, name: &str, value: &str) -> Result<Self, BuildError> {
        if name.is_empty() || (!self.raw && !self.operation.query.iter().any(|p| p.name == name)) {
            return Err(BuildError::InvalidParameter);
        }
        let mut encoded = String::<128>::new();
        encode(&mut encoded, name, true)?;
        let previous = core::mem::take(&mut self.path);
        let (path, query) = previous.split_once('?').unwrap_or((&previous, ""));
        self.path.push_str(path).map_err(|_| BuildError::Capacity)?;
        let mut first = true;
        for part in query.split('&').filter(|p| !p.is_empty()) {
            if part.split_once('=').map(|(key, _)| key).unwrap_or(part) == encoded {
                continue;
            }
            self.path
                .push(if first { '?' } else { '&' })
                .map_err(|_| BuildError::Capacity)?;
            first = false;
            self.path.push_str(part).map_err(|_| BuildError::Capacity)?;
        }
        self.query(name, value)
    }

    /// Network API offset/limit paging. Limit is validated by the server for each operation.
    pub fn page(self, offset: u64, limit: u32) -> Result<Self, BuildError> {
        let mut a = String::<20>::new();
        let mut b = String::<10>::new();
        write!(a, "{offset}").map_err(|_| BuildError::Capacity)?;
        write!(b, "{limit}").map_err(|_| BuildError::Capacity)?;
        self.set_query("offset", &a)?.set_query("limit", &b)
    }

    pub fn filter(self, expression: &str) -> Result<Self, BuildError> {
        self.query("filter", expression)
    }

    pub fn validate(&self, body: Option<&[u8]>) -> Result<(), BuildError> {
        if self.operation.body_required && body.is_none() {
            return Err(BuildError::InvalidBody);
        }
        for param in self.operation.query.iter().filter(|p| p.required) {
            let mut encoded = String::<128>::new();
            encode(&mut encoded, param.name, true)?;
            if !self.path.split_once('?').is_some_and(|(_, q)| {
                q.split('&').any(|p| {
                    p.split_once('=')
                        .is_some_and(|(k, v)| k == encoded && !v.is_empty())
                })
            }) {
                return Err(BuildError::MissingParameter);
            }
        }
        Ok(())
    }
}

pub(crate) fn encode<const N: usize>(
    out: &mut String<N>,
    value: &str,
    query: bool,
) -> Result<(), BuildError> {
    const HEX: &[u8] = b"0123456789ABCDEF";
    for b in value.bytes() {
        // Encode dots in path parameters as well, including dot-only traversal segments.
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'~' || (query && b == b'.')
        {
            out.push(b as char).map_err(|_| BuildError::Capacity)?;
        } else {
            for c in [b'%', HEX[(b >> 4) as usize], HEX[(b & 15) as usize]] {
                out.push(c as char).map_err(|_| BuildError::Capacity)?;
            }
        }
    }
    Ok(())
}
