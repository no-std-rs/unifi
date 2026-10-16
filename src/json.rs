//! Borrowed JSON views with bounded nesting, complete validation, and no allocation.
use heapless::String;

#[cfg(feature = "serde")]
mod serde_adapter;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JsonError {
    Syntax,
    Utf8,
    Depth,
    Type,
    Missing,
    Capacity,
    Decode,
}

/// A validated JSON value borrowing the original bytes. Unknown fields remain available.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Json<'a>(&'a str);

impl<'a> Json<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, JsonError> {
        let text = core::str::from_utf8(bytes).map_err(|_| JsonError::Utf8)?;
        let text = text.trim_matches([' ', '\r', '\n', '\t']);
        if scan(text.as_bytes(), 0, 0)? != text.len() {
            return Err(JsonError::Syntax);
        }
        Ok(Self(text))
    }
    pub fn raw(self) -> &'a str {
        self.0
    }
    pub fn is_null(self) -> bool {
        self.0 == "null"
    }
    pub fn object(self) -> Result<Self, JsonError> {
        if self.0.starts_with('{') {
            Ok(self)
        } else {
            Err(JsonError::Type)
        }
    }
    pub fn string(self) -> Result<JsonString<'a>, JsonError> {
        if self.0.starts_with('"') {
            Ok(JsonString(&self.0[1..self.0.len() - 1]))
        } else {
            Err(JsonError::Type)
        }
    }
    pub fn boolean(self) -> Result<bool, JsonError> {
        match self.0 {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(JsonError::Type),
        }
    }
    pub fn integer(self) -> Result<i64, JsonError> {
        self.0.parse().map_err(|_| JsonError::Type)
    }
    pub fn unsigned(self) -> Result<u64, JsonError> {
        self.0.parse().map_err(|_| JsonError::Type)
    }
    pub fn number(self) -> Result<f64, JsonError> {
        self.0.parse().map_err(|_| JsonError::Type)
    }
    /// Lookup an object field. For duplicate keys the last value wins.
    pub fn get(self, key: &str) -> Result<Option<Self>, JsonError> {
        let mut found = None;
        for (name, value) in self.fields()? {
            if name.equals(key) {
                found = Some(value);
            }
        }
        Ok(found)
    }
    pub fn required(self, key: &str) -> Result<Self, JsonError> {
        self.get(key)?.ok_or(JsonError::Missing)
    }
    /// Optional schema fields treat both a missing value and JSON null as absent.
    pub fn optional(self, key: &str) -> Result<Option<Self>, JsonError> {
        Ok(self.get(key)?.filter(|v| !v.is_null()))
    }
    pub fn items(self) -> Result<Items<'a>, JsonError> {
        if !self.0.starts_with('[') {
            return Err(JsonError::Type);
        }
        Ok(Items {
            rest: &self.0[1..self.0.len() - 1],
        })
    }
    pub fn fields(self) -> Result<Fields<'a>, JsonError> {
        self.object()?;
        Ok(Fields {
            rest: &self.0[1..self.0.len() - 1],
        })
    }
    /// Deserialize into caller-defined serde types, with caller-owned string escape scratch.
    #[cfg(feature = "serde")]
    pub fn decode<T: serde::de::Deserialize<'a>>(self, scratch: &mut [u8]) -> Result<T, JsonError> {
        serde_adapter::decode(self.0.as_bytes(), scratch)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JsonString<'a>(&'a str);
impl<'a> JsonString<'a> {
    /// Borrow strings without escape sequences directly from the input.
    pub fn as_str(self) -> Option<&'a str> {
        (!self.0.contains('\\')).then_some(self.0)
    }
    pub fn chars(self) -> impl Iterator<Item = char> + Clone + 'a {
        let mut chars = self.0.chars();
        core::iter::from_fn(move || {
            let c = chars.next()?;
            Some(if c != '\\' {
                c
            } else {
                match chars.next()? {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'u' => {
                        let mut n = 0;
                        for _ in 0..4 {
                            n = n * 16 + chars.next()?.to_digit(16)?;
                        }
                        if (0xd800..=0xdbff).contains(&n) {
                            chars.next()?;
                            chars.next()?;
                            let mut low = 0;
                            for _ in 0..4 {
                                low = low * 16 + chars.next()?.to_digit(16)?;
                            }
                            n = 0x10000 + ((n - 0xd800) << 10) + low - 0xdc00;
                        }
                        char::from_u32(n)?
                    }
                    _ => return None,
                }
            })
        })
    }
    pub fn equals(self, text: &str) -> bool {
        self.chars().eq(text.chars())
    }
    pub fn decode<const N: usize>(self) -> Result<String<N>, JsonError> {
        let mut out = String::new();
        for c in self.chars() {
            out.push(c).map_err(|_| JsonError::Capacity)?;
        }
        Ok(out)
    }
}

#[derive(Clone, Debug)]
pub struct Items<'a> {
    rest: &'a str,
}
impl<'a> Iterator for Items<'a> {
    type Item = Json<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        self.rest = self.rest.trim_start_matches([' ', '\r', '\n', '\t', ',']);
        if self.rest.is_empty() {
            return None;
        }
        // The parent value has already been fully validated.
        let n = scan(self.rest.as_bytes(), 0, 0).ok()?;
        let (value, rest) = self.rest.split_at(n);
        self.rest = rest;
        Some(Json(value))
    }
}
#[derive(Clone, Debug)]
pub struct Fields<'a> {
    rest: &'a str,
}
impl<'a> Iterator for Fields<'a> {
    type Item = (JsonString<'a>, Json<'a>);
    fn next(&mut self) -> Option<Self::Item> {
        self.rest = self.rest.trim_start_matches([' ', '\r', '\n', '\t', ',']);
        if self.rest.is_empty() {
            return None;
        }
        let n = string_end(self.rest.as_bytes(), 0).ok()?;
        let key = JsonString(&self.rest[1..n - 1]);
        self.rest = self.rest[n..].trim_start_matches([' ', '\r', '\n', '\t']);
        self.rest = self
            .rest
            .strip_prefix(':')?
            .trim_start_matches([' ', '\r', '\n', '\t']);
        let n = scan(self.rest.as_bytes(), 0, 0).ok()?;
        let (value, rest) = self.rest.split_at(n);
        self.rest = rest;
        Some((key, Json(value)))
    }
}

fn whitespace(bytes: &[u8], mut i: usize) -> usize {
    while bytes
        .get(i)
        .is_some_and(|b| matches!(b, b' ' | b'\r' | b'\n' | b'\t'))
    {
        i += 1;
    }
    i
}
fn hex(bytes: &[u8], i: usize) -> Result<u32, JsonError> {
    let mut n = 0;
    for b in bytes.get(i..i + 4).ok_or(JsonError::Syntax)? {
        n = n * 16 + (*b as char).to_digit(16).ok_or(JsonError::Syntax)?;
    }
    Ok(n)
}
fn string_end(bytes: &[u8], start: usize) -> Result<usize, JsonError> {
    if bytes.get(start) != Some(&b'"') {
        return Err(JsonError::Syntax);
    }
    let mut i = start + 1;
    loop {
        match *bytes.get(i).ok_or(JsonError::Syntax)? {
            b'"' => return Ok(i + 1),
            0..=31 => return Err(JsonError::Syntax),
            b'\\' => {
                i += 1;
                match *bytes.get(i).ok_or(JsonError::Syntax)? {
                    b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => i += 1,
                    b'u' => {
                        let n = hex(bytes, i + 1)?;
                        i += 5;
                        if (0xd800..=0xdbff).contains(&n) {
                            if bytes.get(i..i + 2) != Some(b"\\u")
                                || !(0xdc00..=0xdfff).contains(&hex(bytes, i + 2)?)
                            {
                                return Err(JsonError::Syntax);
                            }
                            i += 6;
                        } else if (0xdc00..=0xdfff).contains(&n) {
                            return Err(JsonError::Syntax);
                        }
                    }
                    _ => return Err(JsonError::Syntax),
                }
            }
            _ => i += 1,
        }
    }
}
fn scan(bytes: &[u8], start: usize, depth: usize) -> Result<usize, JsonError> {
    if depth > 64 {
        return Err(JsonError::Depth);
    }
    let first = *bytes.get(start).ok_or(JsonError::Syntax)?;
    match first {
        b'"' => string_end(bytes, start),
        b'{' | b'[' => {
            let object = first == b'{';
            let end = if object { b'}' } else { b']' };
            let mut i = whitespace(bytes, start + 1);
            if bytes.get(i) == Some(&end) {
                return Ok(i + 1);
            }
            loop {
                if object {
                    i = whitespace(bytes, string_end(bytes, i)?);
                    if bytes.get(i) != Some(&b':') {
                        return Err(JsonError::Syntax);
                    }
                    i = whitespace(bytes, i + 1);
                }
                i = whitespace(bytes, scan(bytes, i, depth + 1)?);
                if bytes.get(i) == Some(&end) {
                    return Ok(i + 1);
                }
                if bytes.get(i) != Some(&b',') {
                    return Err(JsonError::Syntax);
                }
                i = whitespace(bytes, i + 1);
            }
        }
        b't' | b'f' | b'n' => {
            let literal: &[u8] = match first {
                b't' => b"true",
                b'f' => b"false",
                _ => b"null",
            };
            if bytes.get(start..start + literal.len()) == Some(literal) {
                Ok(start + literal.len())
            } else {
                Err(JsonError::Syntax)
            }
        }
        b'-' | b'0'..=b'9' => {
            let mut i = start;
            if first == b'-' {
                i += 1;
            }
            match bytes.get(i) {
                Some(b'0') => i += 1,
                Some(b'1'..=b'9') => {
                    i += 1;
                    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                        i += 1;
                    }
                }
                _ => return Err(JsonError::Syntax),
            }
            if bytes.get(i) == Some(&b'.') {
                i += 1;
                let begin = i;
                while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                if i == begin {
                    return Err(JsonError::Syntax);
                }
            }
            if bytes.get(i).is_some_and(|b| matches!(b, b'e' | b'E')) {
                i += 1;
                if bytes.get(i).is_some_and(|b| matches!(b, b'+' | b'-')) {
                    i += 1;
                }
                let begin = i;
                while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                if i == begin {
                    return Err(JsonError::Syntax);
                }
            }
            Ok(i)
        }
        _ => Err(JsonError::Syntax),
    }
}

/// Build an object in caller-owned memory. Nested JSON values can be inserted without copies to a heap.
pub struct ObjectWriter<'a> {
    buffer: &'a mut [u8],
    len: usize,
    fields: usize,
}
impl<'a> ObjectWriter<'a> {
    pub fn new(buffer: &'a mut [u8]) -> Result<Self, JsonError> {
        if buffer.is_empty() {
            return Err(JsonError::Capacity);
        }
        buffer[0] = b'{';
        Ok(Self {
            buffer,
            len: 1,
            fields: 0,
        })
    }
    fn prefix(&mut self, key: &str) -> Result<(), JsonError> {
        if self.fields != 0 {
            self.append(b",")?;
        }
        let n = encode_to_slice(key, &mut self.buffer[self.len..])?;
        self.len += n;
        self.append(b":")
    }
    fn append(&mut self, bytes: &[u8]) -> Result<(), JsonError> {
        self.buffer
            .get_mut(self.len..self.len + bytes.len())
            .ok_or(JsonError::Capacity)?
            .copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
    pub fn field<T: Encode + ?Sized>(
        &mut self,
        key: &str,
        value: &T,
    ) -> Result<&mut Self, JsonError> {
        let previous = self.len;
        let result = (|| {
            self.prefix(key)?;
            let n = encode_to_slice(value, &mut self.buffer[self.len..])?;
            self.len += n;
            Ok(())
        })();
        if let Err(e) = result {
            self.len = previous;
            return Err(e);
        }
        self.fields += 1;
        Ok(self)
    }
    pub fn raw_field(&mut self, key: &str, value: Json<'_>) -> Result<&mut Self, JsonError> {
        let previous = self.len;
        if let Err(e) = self
            .prefix(key)
            .and_then(|_| self.append(value.raw().as_bytes()))
        {
            self.len = previous;
            return Err(e);
        }
        self.fields += 1;
        Ok(self)
    }
    pub fn finish(mut self) -> Result<&'a [u8], JsonError> {
        self.append(b"}")?;
        Ok(&self.buffer[..self.len])
    }
}

pub trait View<'a>: Sized {
    fn from_json(value: Json<'a>) -> Result<Self, JsonError>;
}

/// Iterate model views over an array without storing its elements.
#[derive(Clone, Copy, Debug)]
pub struct Array<'a, T> {
    json: Json<'a>,
    marker: core::marker::PhantomData<T>,
}
impl<'a, T: View<'a>> Array<'a, T> {
    pub fn new(json: Json<'a>) -> Result<Self, JsonError> {
        json.items()?;
        Ok(Self {
            json,
            marker: core::marker::PhantomData,
        })
    }
    pub fn iter(self) -> impl Iterator<Item = Result<T, JsonError>> + 'a
    where
        T: 'a,
    {
        // Construction verifies the array's type.
        Items {
            rest: &self.json.0[1..self.json.0.len() - 1],
        }
        .map(T::from_json)
    }
    pub fn json(self) -> Json<'a> {
        self.json
    }
}

/// Encode a JSON value into a fallible formatting sink, without allocation.
/// Custom request structs may implement this trait, or use the optional serde integration.
pub trait Encode {
    fn encode(&self, out: &mut dyn core::fmt::Write) -> Result<(), JsonError>;
}
impl<T: Encode + ?Sized> Encode for &T {
    fn encode(&self, out: &mut dyn core::fmt::Write) -> Result<(), JsonError> {
        (**self).encode(out)
    }
}
impl Encode for str {
    fn encode(&self, out: &mut dyn core::fmt::Write) -> Result<(), JsonError> {
        out.write_char('"').map_err(|_| JsonError::Capacity)?;
        for c in self.chars() {
            let result = match c {
                '"' => out.write_str("\\\""),
                '\\' => out.write_str("\\\\"),
                '\n' => out.write_str("\\n"),
                '\r' => out.write_str("\\r"),
                '\t' => out.write_str("\\t"),
                c if c < '\u{20}' => write!(out, "\\u{:04x}", c as u32),
                c => out.write_char(c),
            };
            result.map_err(|_| JsonError::Capacity)?;
        }
        out.write_char('"').map_err(|_| JsonError::Capacity)
    }
}
impl<const N: usize> Encode for heapless::String<N> {
    fn encode(&self, out: &mut dyn core::fmt::Write) -> Result<(), JsonError> {
        self.as_str().encode(out)
    }
}
macro_rules! encode_display {
    ($($ty:ty),*) => { $(impl Encode for $ty {
        fn encode(&self, out: &mut dyn core::fmt::Write) -> Result<(), JsonError> { write!(out,"{self}").map_err(|_| JsonError::Capacity) }
    })* };
}
encode_display!(
    bool, u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize
);
macro_rules! encode_float {
    ($($ty:ty),*) => { $(impl Encode for $ty {
        fn encode(&self, out: &mut dyn core::fmt::Write) -> Result<(), JsonError> {
            if !self.is_finite() { return Err(JsonError::Type); }
            write!(out,"{self}").map_err(|_| JsonError::Capacity)
        }
    })* };
}
encode_float!(f32, f64);
impl<T: Encode> Encode for Option<T> {
    fn encode(&self, out: &mut dyn core::fmt::Write) -> Result<(), JsonError> {
        match self {
            Some(value) => value.encode(out),
            None => out.write_str("null").map_err(|_| JsonError::Capacity),
        }
    }
}
impl<T: Encode> Encode for [T] {
    fn encode(&self, out: &mut dyn core::fmt::Write) -> Result<(), JsonError> {
        out.write_char('[').map_err(|_| JsonError::Capacity)?;
        for (i, value) in self.iter().enumerate() {
            if i != 0 {
                out.write_char(',').map_err(|_| JsonError::Capacity)?;
            }
            value.encode(out)?;
        }
        out.write_char(']').map_err(|_| JsonError::Capacity)
    }
}
impl<T: Encode, const N: usize> Encode for [T; N] {
    fn encode(&self, out: &mut dyn core::fmt::Write) -> Result<(), JsonError> {
        self.as_slice().encode(out)
    }
}
impl<T: Encode, const N: usize> Encode for heapless::Vec<T, N> {
    fn encode(&self, out: &mut dyn core::fmt::Write) -> Result<(), JsonError> {
        self.as_slice().encode(out)
    }
}
impl Encode for Json<'_> {
    fn encode(&self, out: &mut dyn core::fmt::Write) -> Result<(), JsonError> {
        out.write_str(self.raw()).map_err(|_| JsonError::Capacity)
    }
}
struct SliceWriter<'a> {
    buffer: &'a mut [u8],
    len: usize,
}
impl core::fmt::Write for SliceWriter<'_> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        self.buffer
            .get_mut(self.len..self.len + text.len())
            .ok_or(core::fmt::Error)?
            .copy_from_slice(text.as_bytes());
        self.len += text.len();
        Ok(())
    }
}
pub fn encode_to_slice<T: Encode + ?Sized>(
    value: &T,
    buffer: &mut [u8],
) -> Result<usize, JsonError> {
    let mut writer = SliceWriter { buffer, len: 0 };
    value.encode(&mut writer)?;
    Ok(writer.len)
}

/// Adapt a serde model without enabling std or alloc.
#[cfg(feature = "serde")]
pub fn encode_serde<'a, T: serde::Serialize + ?Sized>(
    value: &T,
    buffer: &'a mut [u8],
) -> Result<&'a [u8], JsonError> {
    let n = serde_json_core::to_slice(value, buffer).map_err(|_| JsonError::Capacity)?;
    Ok(&buffer[..n])
}
