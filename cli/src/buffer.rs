use core::fmt;
pub struct Buffer<'a> {
    bytes: &'a mut [u8],
    used: usize,
}
impl<'a> Buffer<'a> {
    pub fn new(bytes: &'a mut [u8]) -> Self {
        Self { bytes, used: 0 }
    }
    pub fn finish(self) -> &'a [u8] {
        &self.bytes[..self.used]
    }
}
impl fmt::Write for Buffer<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self.used.checked_add(text.len()).ok_or(fmt::Error)?;
        self.bytes
            .get_mut(self.used..end)
            .ok_or(fmt::Error)?
            .copy_from_slice(text.as_bytes());
        self.used = end;
        Ok(())
    }
}
