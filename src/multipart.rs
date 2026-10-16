//! Bounded multipart encoding for Protect device asset uploads.
use crate::{Body, BuildError};
use core::fmt::{self, Write};
use heapless::String;

pub struct Multipart<'a> {
    bytes: &'a [u8],
    content_type: String<128>,
}
impl<'a> Multipart<'a> {
    /// Encode one file into caller-owned memory. Supply a fresh boundary, normally random.
    /// A boundary that occurs in the file is rejected rather than producing an ambiguous body.
    pub fn file(
        output: &'a mut [u8],
        boundary: &str,
        field: &str,
        filename: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> Result<Self, BuildError> {
        if boundary.is_empty()
            || boundary.len() > 70
            || !boundary
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            || bytes
                .windows(boundary.len())
                .any(|w| w == boundary.as_bytes())
            || [field, filename, media_type]
                .iter()
                .any(|v| v.is_empty() || v.chars().any(char::is_control))
            || !media_type.bytes().all(|b| (33..127).contains(&b))
        {
            return Err(BuildError::InvalidParameter);
        }
        let mut writer = Buffer {
            bytes: output,
            used: 0,
        };
        write!(
            writer,
            "--{boundary}\r\nContent-Disposition: form-data; name=\""
        )
        .map_err(|_| BuildError::Capacity)?;
        writer.quoted(field)?;
        writer
            .write_str("\"; filename=\"")
            .map_err(|_| BuildError::Capacity)?;
        writer.quoted(filename)?;
        write!(writer, "\"\r\nContent-Type: {media_type}\r\n\r\n")
            .map_err(|_| BuildError::Capacity)?;
        writer.extend(bytes)?;
        write!(writer, "\r\n--{boundary}--\r\n").map_err(|_| BuildError::Capacity)?;
        let mut content_type = String::new();
        write!(content_type, "multipart/form-data; boundary={boundary}")
            .map_err(|_| BuildError::Capacity)?;
        Ok(Self {
            bytes: &writer.bytes[..writer.used],
            content_type,
        })
    }
    pub fn body(&self) -> Body<'_> {
        Body::Bytes {
            bytes: self.bytes,
            content_type: &self.content_type,
        }
    }
}
struct Buffer<'a> {
    bytes: &'a mut [u8],
    used: usize,
}
impl Buffer<'_> {
    fn extend(&mut self, bytes: &[u8]) -> Result<(), BuildError> {
        let end = self
            .used
            .checked_add(bytes.len())
            .ok_or(BuildError::Capacity)?;
        let dest = self
            .bytes
            .get_mut(self.used..end)
            .ok_or(BuildError::Capacity)?;
        dest.copy_from_slice(bytes);
        self.used = end;
        Ok(())
    }
    fn quoted(&mut self, text: &str) -> Result<(), BuildError> {
        for c in text.chars() {
            if c == '"' || c == '\\' {
                self.extend(b"\\")?;
            }
            self.write_char(c).map_err(|_| BuildError::Capacity)?;
        }
        Ok(())
    }
}
impl Write for Buffer<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.extend(text.as_bytes()).map_err(|_| fmt::Error)
    }
}
