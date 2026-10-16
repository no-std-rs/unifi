//! RFC 6455 client framing for Protect subscriptions and Network event streams.
//! Messages use caller-owned buffers. Compression and extensions are never negotiated.
use crate::{Authentication, BuildError, Call, Client, Method};
use embedded_io_async::{Read, Write};
use heapless::String;
use reqwless::request::{Request, RequestBuilder};

#[derive(Debug)]
pub enum Error {
    Http(crate::Error),
    Transport(embedded_io::ErrorKind),
    Handshake,
    Protocol,
    Capacity,
    Closed,
    UnexpectedEnd,
    Random,
}
impl From<reqwless::Error> for Error {
    fn from(value: reqwless::Error) -> Self {
        Self::Http(crate::Error::Transport(value))
    }
}
impl From<BuildError> for Error {
    fn from(value: BuildError) -> Self {
        Self::Http(crate::Error::Build(value))
    }
}
fn transport(error: impl embedded_io::Error) -> Error {
    Error::Transport(error.kind())
}
async fn exact(io: &mut impl Read, mut output: &mut [u8]) -> Result<(), Error> {
    while !output.is_empty() {
        let n = io.read(output).await.map_err(transport)?;
        if n == 0 {
            return Err(Error::UnexpectedEnd);
        }
        output = &mut output[n..];
    }
    Ok(())
}

/// A borrowed message remains valid until the caller reuses its buffer.
#[derive(Debug)]
pub enum Message<'a> {
    Text(&'a str),
    Binary(&'a [u8]),
    Close {
        code: Option<u16>,
        reason: String<123>,
    },
}
pub struct WebSocket<IO> {
    io: IO,
    close_sent: bool,
    closed: bool,
    failed: bool,
}

/// Perform an authenticated HTTP upgrade. `nonce` must contain 16 fresh random bytes.
/// The endpoint must be local: UniFi's cloud connector does not support WebSocket upgrades.
pub async fn connect<IO: Read + Write>(
    client: &Client<'_>,
    mut io: IO,
    call: &Call,
    nonce: [u8; 16],
    headers: &mut [u8],
) -> Result<WebSocket<IO>, Error> {
    if call.operation.method != Method::GET {
        return Err(Error::Handshake);
    }
    call.validate(None)?;
    let mut path = String::<8192>::try_from(client.base).map_err(|_| Error::Capacity)?;
    path.push_str(call.path()).map_err(|_| Error::Capacity)?;
    let key = base64::<24>(&nonce);
    let expected = accept(&key);
    let mut extra = heapless::Vec::<(&str, &str), 7>::new();
    extra.push(("Connection", "Upgrade")).unwrap();
    extra.push(("Upgrade", "websocket")).unwrap();
    extra.push(("Sec-WebSocket-Version", "13")).unwrap();
    extra.push(("Sec-WebSocket-Key", &key)).unwrap();
    match client.authentication {
        Authentication::ApiKey(key) => {
            extra.push(("X-API-Key", key)).unwrap();
        }
        Authentication::Session { cookie, csrf } => {
            extra.push(("Cookie", cookie)).unwrap();
            if let Some(csrf) = csrf {
                extra.push(("X-CSRF-Token", csrf)).unwrap();
            }
        }
        Authentication::None => {}
    }
    Request::new(Method::GET, &path)
        .host(client.host)
        .headers(&extra)
        .build()
        .write_header(&mut io)
        .await?;
    io.flush().await.map_err(transport)?;
    {
        // reqwless rejects data following an informational response. Limit the header
        // reader to one byte so the first WebSocket frame is left untouched in the stream.
        let mut reader = HeaderRead(&mut io);
        let response =
            reqwless::response::Response::read(&mut reader, Method::GET, headers).await?;
        if response.status.0 != 101 {
            return Err(Error::Handshake);
        }
        let mut upgrade = false;
        let mut connection = false;
        let mut accepted = false;
        for (name, value) in response.headers() {
            if name.eq_ignore_ascii_case("upgrade") {
                if upgrade || !value.eq_ignore_ascii_case(b"websocket") {
                    return Err(Error::Handshake);
                }
                upgrade = true;
            } else if name.eq_ignore_ascii_case("connection") {
                connection |= core::str::from_utf8(value).is_ok_and(|v| {
                    v.split(',')
                        .any(|v| v.trim().eq_ignore_ascii_case("upgrade"))
                });
            } else if name.eq_ignore_ascii_case("sec-websocket-accept") {
                if accepted || value != expected.as_bytes() {
                    return Err(Error::Handshake);
                }
                accepted = true;
            } else if name.eq_ignore_ascii_case("sec-websocket-extensions")
                || name.eq_ignore_ascii_case("sec-websocket-protocol")
            {
                return Err(Error::Handshake);
            }
        }
        if !upgrade || !connection || !accepted {
            return Err(Error::Handshake);
        }
    }
    Ok(WebSocket {
        io,
        close_sent: false,
        closed: false,
        failed: false,
    })
}
struct HeaderRead<'a, IO>(&'a mut IO);
impl<IO: Read> embedded_io::ErrorType for HeaderRead<'_, IO> {
    type Error = IO::Error;
}
impl<IO: Read> Read for HeaderRead<'_, IO> {
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        let n = bytes.len().min(1);
        self.0.read(&mut bytes[..n]).await
    }
}

impl<IO: Read + Write> WebSocket<IO> {
    /// Send a text message using a fresh, unpredictable client masking key.
    pub async fn text(&mut self, text: &str, mask: [u8; 4]) -> Result<(), Error> {
        self.send(1, text.as_bytes(), mask).await
    }
    pub async fn binary(&mut self, bytes: &[u8], mask: [u8; 4]) -> Result<(), Error> {
        self.send(2, bytes, mask).await
    }
    pub async fn ping(&mut self, bytes: &[u8], mask: [u8; 4]) -> Result<(), Error> {
        self.send(9, bytes, mask).await
    }
    /// Start the closing handshake. Keep calling `receive` until the peer's Close arrives.
    /// Further data sends are rejected while control frames can still be received.
    pub async fn close(&mut self, code: u16, reason: &str, mask: [u8; 4]) -> Result<(), Error> {
        if !close_code(code) || reason.len() > 123 {
            return Err(Error::Protocol);
        }
        let mut payload = heapless::Vec::<u8, 125>::new();
        payload.extend_from_slice(&code.to_be_bytes()).unwrap();
        payload.extend_from_slice(reason.as_bytes()).unwrap();
        self.send(8, &payload, mask).await?;
        self.close_sent = true;
        Ok(())
    }
    async fn send(&mut self, opcode: u8, bytes: &[u8], mask: [u8; 4]) -> Result<(), Error> {
        if self.closed || self.failed || (self.close_sent && opcode != 10) {
            return Err(Error::Closed);
        }
        if opcode >= 8 && bytes.len() > 125 {
            return Err(Error::Protocol);
        }
        self.failed = true;
        self.write_frame(opcode, bytes, mask).await?;
        self.failed = false;
        Ok(())
    }
    async fn write_frame(&mut self, opcode: u8, bytes: &[u8], mask: [u8; 4]) -> Result<(), Error> {
        let mut header = heapless::Vec::<u8, 14>::new();
        header.push(0x80 | opcode).unwrap();
        if bytes.len() < 126 {
            header.push(0x80 | bytes.len() as u8).unwrap();
        } else if bytes.len() <= u16::MAX as usize {
            header.extend_from_slice(&[0xfe]).unwrap();
            header
                .extend_from_slice(&(bytes.len() as u16).to_be_bytes())
                .unwrap();
        } else {
            header.extend_from_slice(&[0xff]).unwrap();
            header
                .extend_from_slice(&(bytes.len() as u64).to_be_bytes())
                .unwrap();
        }
        header.extend_from_slice(&mask).unwrap();
        self.io.write_all(&header).await.map_err(transport)?;
        let mut offset = 0;
        let mut chunk = [0; 256];
        for bytes in bytes.chunks(chunk.len()) {
            for (i, byte) in bytes.iter().enumerate() {
                chunk[i] = byte ^ mask[(offset + i) % 4];
            }
            self.io
                .write_all(&chunk[..bytes.len()])
                .await
                .map_err(transport)?;
            offset += bytes.len();
        }
        self.io.flush().await.map_err(transport)?;
        Ok(())
    }
    /// Reassemble fragmented messages, answer pings and validate UTF-8 and close frames.
    /// An error or cancellation poisons the stream; reconnect rather than attempting to resume.
    pub async fn receive<'a>(
        &mut self,
        output: &'a mut [u8],
        masks: &mut impl FnMut() -> Result<[u8; 4], Error>,
    ) -> Result<Message<'a>, Error> {
        if self.closed || self.failed {
            return Err(Error::Closed);
        }
        self.failed = true;
        let result = self.receive_inner(output, masks).await;
        self.failed = result.is_err();
        result
    }
    async fn receive_inner<'a>(
        &mut self,
        output: &'a mut [u8],
        masks: &mut impl FnMut() -> Result<[u8; 4], Error>,
    ) -> Result<Message<'a>, Error> {
        let mut used: usize = 0;
        let mut kind = None;
        loop {
            let mut header = [0; 2];
            exact(&mut self.io, &mut header).await?;
            let fin = header[0] & 0x80 != 0;
            let opcode = header[0] & 15;
            if header[0] & 0x70 != 0
                || header[1] & 0x80 != 0
                || ![0, 1, 2, 8, 9, 10].contains(&opcode)
            {
                return Err(Error::Protocol);
            }
            let short = header[1] & 127;
            let length = if short == 126 {
                let mut bytes = [0; 2];
                exact(&mut self.io, &mut bytes).await?;
                let n = u16::from_be_bytes(bytes) as u64;
                if n < 126 {
                    return Err(Error::Protocol);
                }
                n
            } else if short == 127 {
                let mut bytes = [0; 8];
                exact(&mut self.io, &mut bytes).await?;
                let n = u64::from_be_bytes(bytes);
                if n <= u16::MAX as u64 || n >> 63 != 0 {
                    return Err(Error::Protocol);
                }
                n
            } else {
                short as u64
            };
            if opcode >= 8 {
                if !fin || length > 125 {
                    return Err(Error::Protocol);
                }
                let mut payload = [0; 125];
                let length = length as usize;
                exact(&mut self.io, &mut payload[..length]).await?;
                if opcode == 9 {
                    self.write_frame(10, &payload[..length], masks()?).await?;
                }
                if opcode == 8 {
                    if length == 1 {
                        return Err(Error::Protocol);
                    }
                    let code = (length >= 2).then(|| u16::from_be_bytes([payload[0], payload[1]]));
                    if code.is_some_and(|c| !close_code(c)) {
                        return Err(Error::Protocol);
                    }
                    let text =
                        core::str::from_utf8(&payload[if length >= 2 { 2 } else { 0 }..length])
                            .map_err(|_| Error::Protocol)?;
                    let reason = String::try_from(text).map_err(|_| Error::Capacity)?;
                    if !self.close_sent {
                        self.write_frame(8, &payload[..length], masks()?).await?;
                        self.close_sent = true;
                    }
                    self.closed = true;
                    return Ok(Message::Close { code, reason });
                }
                continue;
            }
            if opcode == 0 {
                if kind.is_none() {
                    return Err(Error::Protocol);
                }
            } else if kind.replace(opcode).is_some() {
                return Err(Error::Protocol);
            }
            let length = usize::try_from(length).map_err(|_| Error::Capacity)?;
            let end = used.checked_add(length).ok_or(Error::Capacity)?;
            let dest = output.get_mut(used..end).ok_or(Error::Capacity)?;
            exact(&mut self.io, dest).await?;
            used = end;
            if fin {
                return if kind == Some(1) {
                    Ok(Message::Text(
                        core::str::from_utf8(&output[..used]).map_err(|_| Error::Protocol)?,
                    ))
                } else {
                    Ok(Message::Binary(&output[..used]))
                };
            }
        }
    }
}
fn close_code(code: u16) -> bool {
    (1000..=1014).contains(&code) && ![1004, 1005, 1006].contains(&code)
        || (3000..=4999).contains(&code)
}

fn base64<const N: usize>(bytes: &[u8]) -> String<N> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as u32;
        let b = *chunk.get(1).unwrap_or(&0) as u32;
        let c = *chunk.get(2).unwrap_or(&0) as u32;
        for ch in [
            TABLE[(a >> 2) as usize],
            TABLE[(((a & 3) << 4) | (b >> 4)) as usize],
            if chunk.len() > 1 {
                TABLE[(((b & 15) << 2) | (c >> 6)) as usize]
            } else {
                b'='
            },
            if chunk.len() > 2 {
                TABLE[(c & 63) as usize]
            } else {
                b'='
            },
        ] {
            out.push(ch as char).unwrap();
        }
    }
    out
}
fn accept(key: &str) -> String<28> {
    // SHA-1 is required solely by RFC 6455's public handshake, not used as cryptographic authentication.
    // The input is always a 24-byte Base64 nonce plus the 36-byte protocol GUID (two padded blocks).
    let mut bytes = [0; 128];
    bytes[..24].copy_from_slice(key.as_bytes());
    bytes[24..60].copy_from_slice(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    bytes[60] = 0x80;
    bytes[120..].copy_from_slice(&480u64.to_be_bytes());
    let mut h = [
        0x67452301u32,
        0xefcdab89,
        0x98badcfe,
        0x10325476,
        0xc3d2e1f0,
    ];
    for chunk in bytes.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, bytes) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*bytes);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, word) in w.into_iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5a827999),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (h, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *h = h.wrapping_add(v);
        }
    }
    let mut digest = [0; 20];
    for (out, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        out.copy_from_slice(&word.to_be_bytes());
    }
    base64(&digest)
}
