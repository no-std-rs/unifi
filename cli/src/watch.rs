use crate::{Options, Output, Socket, Url, fill_random, run, write_bytes};
use core::{cell::Cell, fmt::Write};
use embedded_io_async::{Read, Write as IoWrite};
use unifi::{
    Authentication, Client,
    websocket::{self, Message},
};
trait ReadTimeout {
    fn set_read_timeout(
        &mut self,
        timeout: core::time::Duration,
    ) -> Result<(), embedded_io::ErrorKind>;
}
impl ReadTimeout for Socket {
    fn set_read_timeout(
        &mut self,
        timeout: core::time::Duration,
    ) -> Result<(), embedded_io::ErrorKind> {
        rustix::net::sockopt::set_socket_timeout(
            &self.0,
            rustix::net::sockopt::Timeout::Recv,
            Some(timeout),
        )
        .map_err(|_| embedded_io::ErrorKind::Other)
    }
}
impl ReadTimeout for crate::openssl::Tls {
    fn set_read_timeout(
        &mut self,
        timeout: core::time::Duration,
    ) -> Result<(), embedded_io::ErrorKind> {
        self.set_read_timeout(timeout);
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub fn watch(
    url: &Url<'_>,
    base: &str,
    authentication: Authentication<'_>,
    call: &unifi::Call,
    options: &Options<'_>,
    buffer: &mut [u8],
    filtered: &mut [u8],
    sanitized: &mut [u8],
) -> Result<i32, &'static str> {
    if options.cloud || url.host.eq_ignore_ascii_case("api.ui.com") {
        return Err(
            "WebSockets require a direct controller URL; the cloud connector does not support upgrades",
        );
    }
    if options.ca_cert.is_some() || options.tls == Some("embedded") {
        return Err(
            "subscriptions use system TLS; select --tls openssl and optionally --ca-file PEM",
        );
    }
    let client = Client::with_authentication(url.authority, base, authentication)
        .map_err(|_| "invalid URL or credentials")?;
    let timeout = core::time::Duration::from_secs(options.timeout.unwrap_or(30));
    if url.secure {
        let stream = crate::openssl::Tls::connect(
            url.host,
            options.connect_ip.unwrap_or(url.host),
            url.port,
            options.ca_file,
            timeout,
            options.insecure,
        )?;
        consume(stream, &client, call, options, buffer, filtered, sanitized)
    } else {
        consume(
            Socket::connect(url, options, timeout)?,
            &client,
            call,
            options,
            buffer,
            filtered,
            sanitized,
        )
    }
}
fn consume<IO: Read + IoWrite + ReadTimeout>(
    stream: IO,
    client: &Client<'_>,
    call: &unifi::Call,
    options: &Options<'_>,
    buffer: &mut [u8],
    filtered: &mut [u8],
    sanitized: &mut [u8],
) -> Result<i32, &'static str> {
    let armed = Cell::new(false);
    let stream = Heartbeat {
        stream,
        armed: &armed,
        configured: false,
        interval: options
            .keep_alive
            .unwrap_or(core::time::Duration::from_secs(30)),
    };
    let mut nonce = [0; 16];
    fill_random(&mut nonce)?;
    let mut headers = [0; 8192];
    let mut ws = run(websocket::connect(
        client,
        stream,
        call,
        nonce,
        &mut headers,
    ))
    .map_err(|_| "WebSocket upgrade failed; check direct controller access and credentials")?;
    armed.set(true);
    let types = options.event_types.map(crate::compat::csv).transpose()?;
    if types.as_ref().is_some_and(|v| v.is_empty()) {
        return Err("--types needs at least one event type");
    }
    let mut count = 0;
    let mut masks = || {
        let mut bytes = [0; 4];
        fill_random(&mut bytes).map_err(|_| websocket::Error::Random)?;
        Ok(bytes)
    };
    loop {
        match run(ws.receive(buffer, &mut masks))
            .map_err(|_| "event stream failed or message exceeded capacity")?
        {
            Message::Text(text) => {
                let json =
                    unifi::Json::parse(text.as_bytes()).map_err(|_| "event was not valid JSON")?;
                let json = if let Some(types) = &types {
                    let records = json
                        .get("data")
                        .ok()
                        .flatten()
                        .filter(|v| v.items().is_ok());
                    let matches = |v: unifi::Json<'_>| {
                        ["key", "type", "eventType"].iter().any(|field| {
                            v.get(field)
                                .ok()
                                .flatten()
                                .and_then(|v| v.string().ok())
                                .is_some_and(|value| {
                                    types.iter().any(|wanted| {
                                        value
                                            .chars()
                                            .map(|c| c.to_ascii_lowercase())
                                            .eq(wanted.chars().map(|c| c.to_ascii_lowercase()))
                                    })
                                })
                        })
                    };
                    if let Some(records) = records {
                        let kept = crate::payload::filter(records, matches, filtered)?;
                        if kept.items().unwrap().next().is_none() {
                            continue;
                        }
                        kept
                    } else if matches(json) {
                        json
                    } else {
                        continue;
                    }
                } else {
                    json
                };
                let json = if options.demo {
                    crate::payload::sanitize(json, true, false, sanitized)?
                } else {
                    json
                };
                if !options.quiet {
                    crate::output::render(
                        &mut Output(false),
                        json,
                        options.format.unwrap_or(crate::output::Format::Compact),
                    )
                    .map_err(|_| "output failed")?;
                }
            }
            Message::Binary(bytes) => {
                if types.is_some() {
                    return Err("--types requires JSON events");
                }
                if options.demo {
                    return Err("demo mode cannot sanitize binary events");
                }
                if !options.quiet {
                    write_bytes(bytes)?;
                }
            }
            Message::Close { code, reason } => {
                if code.is_some_and(|c| c != 1000 && c != 1001) {
                    // Escape control characters from a server-supplied reason.
                    let _ = write!(Output(true), "stream closed ({}) ", code.unwrap());
                    for c in reason.chars() {
                        let _ = Output(true).write_char(if c.is_control() { '?' } else { c });
                    }
                    let _ = writeln!(Output(true));
                    return Ok(1);
                }
                return Ok(0);
            }
        }
        count += 1;
        if options.count.is_some_and(|n| count >= n) {
            run(ws.close(1000, "", masks().map_err(|_| "OS randomness unavailable")?))
                .map_err(|_| "closing stream failed")?;
            return Ok(0);
        }
    }
}
struct Heartbeat<'a, IO> {
    stream: IO,
    armed: &'a Cell<bool>,
    configured: bool,
    interval: core::time::Duration,
}
impl<IO: Read + IoWrite> embedded_io::ErrorType for Heartbeat<'_, IO> {
    type Error = embedded_io::ErrorKind;
}
impl<IO: Read + IoWrite + ReadTimeout> Read for Heartbeat<'_, IO> {
    async fn read(&mut self, output: &mut [u8]) -> Result<usize, Self::Error> {
        if self.armed.get() && !self.configured {
            self.stream.set_read_timeout(self.interval)?;
            self.configured = true;
        }
        use embedded_io::Error as _;
        let mut misses = 0;
        loop {
            match self.stream.read(output).await {
                Ok(n) => return Ok(n),
                Err(error)
                    if error.kind() == embedded_io::ErrorKind::TimedOut
                        && self.armed.get()
                        && misses < 2 =>
                {
                    let mut mask = [0; 4];
                    fill_random(&mut mask).map_err(|_| embedded_io::ErrorKind::Other)?;
                    let ping = [0x89, 0x80, mask[0], mask[1], mask[2], mask[3]];
                    self.stream.write_all(&ping).await.map_err(|e| e.kind())?;
                    self.stream.flush().await.map_err(|e| e.kind())?;
                    misses += 1;
                }
                Err(error) => return Err(error.kind()),
            }
        }
    }
}
impl<IO: Read + IoWrite> IoWrite for Heartbeat<'_, IO> {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        use embedded_io::Error as _;
        self.stream.write(bytes).await.map_err(|e| e.kind())
    }
    async fn flush(&mut self) -> Result<(), Self::Error> {
        use embedded_io::Error as _;
        self.stream.flush().await.map_err(|e| e.kind())
    }
}
