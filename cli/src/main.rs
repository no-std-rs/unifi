#![no_std]
#![no_main]

mod buffer;
mod builders;
mod cloud;
mod color;
mod compat;
mod config;
mod duration;
mod filter;
mod flows;
mod image;
mod jsonc;
mod keyring;
mod legacy;
mod lookup;
mod openssl;
mod output;
mod pages;
mod payload;
mod policies;
mod ports;
mod present;
mod process;
mod routes;
mod session_cmd;
mod settings;
mod setup;
mod stats;
mod storage;
mod terminal;
mod topology;
mod tui;
mod ui;
mod vpn;
mod watch;

use argtokens::{ArgTokens, Token};
use core::{
    ffi::{CStr, c_char, c_int},
    fmt::{self, Write as _},
    future::Future,
    net::{IpAddr, SocketAddr},
    task::{Context, Poll, Waker},
};
use routes::Surface;
use rustix::{
    fd::OwnedFd,
    fs::{self, Mode, OFlags},
    io,
    net::{self, AddressFamily, SendFlags, SocketFlags, SocketType},
    stdio,
};
use unifi::{Body, Client, Method, Operation};

// The platform C runtime supplies startup and memory intrinsics. Rust's std and alloc are absent.
#[link(name = "c")]
unsafe extern "C" {
    fn _exit(status: c_int) -> !;
    fn signal(number: c_int, handler: usize) -> usize;
    fn getenv(name: *const c_char) -> *const c_char;
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    unsafe { _exit(101) }
}

struct Output(bool);
impl fmt::Write for Output {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        // SAFETY: this program never closes or takes ownership of stdout/stderr.
        let fd = if self.0 {
            unsafe { stdio::stderr() }
        } else {
            unsafe { stdio::stdout() }
        };
        let mut bytes = text.as_bytes();
        while !bytes.is_empty() {
            match io::write(fd, bytes) {
                Ok(0) => return Err(fmt::Error),
                Ok(n) => bytes = &bytes[n..],
                Err(io::Errno::INTR) => {}
                Err(_) => return Err(fmt::Error),
            }
        }
        Ok(())
    }
}
macro_rules! output { ($($arg:tt)*) => { let _=write!(Output(false),$($arg)*); }; }

struct Socket(OwnedFd);
impl Socket {
    fn connect(
        url: &Url<'_>,
        options: &Options<'_>,
        timeout: core::time::Duration,
    ) -> Result<Self, &'static str> {
        let ip = options
            .connect_ip
            .unwrap_or(url.host)
            .parse::<IpAddr>()
            .map_err(|_| "TCP needs an IP URL or --connect IP")?;
        if !url.secure && !ip.is_loopback() {
            return Err("plain HTTP is limited to loopback; use HTTPS for a controller");
        }
        let socket = net::socket_with(
            if ip.is_ipv4() {
                AddressFamily::INET
            } else {
                AddressFamily::INET6
            },
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )
        .map_err(|_| "socket failed")?;
        for kind in [net::sockopt::Timeout::Recv, net::sockopt::Timeout::Send] {
            net::sockopt::set_socket_timeout(&socket, kind, Some(timeout))
                .map_err(|_| "cannot set timeout")?;
        }
        net::connect(&socket, &SocketAddr::new(ip, url.port)).map_err(|_| "connection failed")?;
        Ok(Self(socket))
    }
}
impl embedded_io::ErrorType for Socket {
    type Error = embedded_io::ErrorKind;
}
impl embedded_io_async::Read for Socket {
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        loop {
            if terminal::cancelled() {
                return Err(embedded_io::ErrorKind::Interrupted);
            }
            match io::read(&self.0, &mut *bytes) {
                Err(io::Errno::INTR) => {}
                Err(io::Errno::AGAIN) => return Err(embedded_io::ErrorKind::TimedOut),
                Ok(n) => return Ok(n),
                Err(_) => return Err(embedded_io::ErrorKind::Other),
            }
        }
    }
}
impl embedded_io_async::Write for Socket {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        loop {
            if terminal::cancelled() {
                return Err(embedded_io::ErrorKind::Interrupted);
            }
            match net::send(&self.0, bytes, SendFlags::NOSIGNAL) {
                Err(io::Errno::INTR) => {}
                Err(io::Errno::AGAIN) => return Err(embedded_io::ErrorKind::TimedOut),
                Ok(n) => return Ok(n),
                Err(_) => return Err(embedded_io::ErrorKind::Other),
            }
        }
    }
    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
fn run<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("blocking rustix stream unexpectedly yielded"),
    }
}
fn read_file<'a>(name: &str, bytes: &'a mut [u8]) -> Result<&'a [u8], &'static str> {
    let file = fs::open(name, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())
        .map_err(|_| "cannot open file")?;
    let mut n = 0;
    loop {
        if n == bytes.len() {
            let mut probe = [0];
            if io::read(&file, &mut probe).map_err(|_| "cannot read file")? != 0 {
                return Err("file exceeds buffer capacity");
            }
            break;
        }
        match io::read(&file, &mut bytes[n..]) {
            Ok(0) => break,
            Ok(count) => n += count,
            Err(io::Errno::INTR) => {}
            Err(_) => return Err("cannot read file"),
        }
    }
    Ok(&bytes[..n])
}
fn optional_file<'a>(name: &str, bytes: &'a mut [u8]) -> Result<Option<&'a [u8]>, &'static str> {
    match fs::stat(name) {
        Err(io::Errno::NOENT) => Ok(None),
        Err(_) => Err("cannot inspect optional file"),
        Ok(_) => read_file(name, bytes).map(Some),
    }
}
fn fill_random(bytes: &mut [u8]) -> Result<(), &'static str> {
    let mut offset = 0;
    while offset < bytes.len() {
        match rustix::rand::getrandom(&mut bytes[offset..], rustix::rand::GetRandomFlags::empty()) {
            Ok(0) => return Err("OS randomness unavailable"),
            Ok(n) => offset += n,
            Err(io::Errno::INTR) => {}
            Err(_) => return Err("OS randomness unavailable"),
        }
    }
    Ok(())
}
fn save_file(path: &str, bytes: &[u8]) -> Result<(), &'static str> {
    let file = fs::open(
        path,
        OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|_| "cannot open output file")?;
    write_fd(&file, bytes)
}
fn write_bytes(bytes: &[u8]) -> Result<(), &'static str> {
    // SAFETY: stdout remains open and borrowed for the duration of this call.
    write_fd(unsafe { stdio::stdout() }, bytes)
}
fn write_fd(fd: impl rustix::fd::AsFd, mut bytes: &[u8]) -> Result<(), &'static str> {
    while !bytes.is_empty() {
        match io::write(&fd, bytes) {
            Ok(0) => return Err("output failed"),
            Ok(n) => bytes = &bytes[n..],
            Err(io::Errno::INTR) => {}
            Err(_) => return Err("output failed"),
        }
    }
    Ok(())
}
fn setting<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    if text.contains('\0') {
        return text
            .split('\0')
            .filter_map(|entry| entry.split_once('='))
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value);
    }
    text.split(if text.contains('\0') { '\0' } else { '\n' })
        .filter_map(|line| {
            line.trim()
                .strip_prefix("export ")
                .unwrap_or(line.trim())
                .split_once('=')
        })
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| {
            let v = v.trim();
            if v.len() >= 2
                && ((v.starts_with('"') && v.ends_with('"'))
                    || (v.starts_with('\'') && v.ends_with('\'')))
            {
                &v[1..v.len() - 1]
            } else {
                v
            }
        })
}
fn environment_value(name: &str) -> Option<&'static str> {
    let mut key = heapless::Vec::<u8, 129>::new();
    if name.is_empty() || name.as_bytes().contains(&0) || name.contains('=') {
        return None;
    }
    key.extend_from_slice(name.as_bytes()).ok()?;
    key.push(0).ok()?;
    // SAFETY: the key is NUL-terminated. This single-threaded executable never
    // mutates the environment; returned values remain valid until process exit.
    let value = unsafe { getenv(key.as_ptr().cast()) };
    if value.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(value) }.to_str().ok()
}

struct Clock;
impl embedded_tls::TlsClock for Clock {
    fn now() -> Option<u64> {
        rustix::time::clock_gettime(rustix::time::ClockId::Realtime)
            .tv_sec
            .try_into()
            .ok()
    }
}
struct Provider<'a> {
    rng: rand_chacha::ChaCha20Rng,
    verifier: embedded_tls::pki::CertVerifier<'a, embedded_tls::Aes128GcmSha256, Clock, 16384>,
}
impl embedded_tls::CryptoProvider for Provider<'_> {
    type CipherSuite = embedded_tls::Aes128GcmSha256;
    type Signature = [u8; 64];
    fn rng(&mut self) -> impl rand_core::CryptoRngCore {
        &mut self.rng
    }
    fn verifier(
        &mut self,
    ) -> Result<&mut impl embedded_tls::TlsVerifier<Self::CipherSuite>, embedded_tls::TlsError>
    {
        Ok(&mut self.verifier)
    }
}

fn operations(surface: &str) -> Result<&'static [&'static Operation], &'static str> {
    match surface {
        "network" => Ok(unifi::network::OPERATIONS),
        "site-manager" => Ok(unifi::site_manager::OPERATIONS),
        "protect" => Ok(unifi::protect::OPERATIONS),
        "session-api" => Ok(unifi::session_api::OPERATIONS),
        _ => Err("unknown API surface"),
    }
}

#[derive(Default, Clone, Copy)]
struct Options<'a> {
    url: Option<&'a str>,
    host: Option<&'a str>,
    command_url: Option<&'a str>,
    order: Option<&'a str>,
    order_after: bool,
    policy_after: bool,
    metric_sites: Option<&'a str>,
    unarchived: bool,
    event_types: Option<&'a str>,
    local_filter: Option<&'a str>,
    local_offset: usize,
    local_limit: Option<usize>,
    color: Option<&'a str>,
    theme: Option<&'a str>,
    log_file: Option<&'a str>,
    no_effects: bool,
    qualities: u8,
    jpeg_quality: Option<u8>,
    low_quality: bool,
    id_only: bool,
    hide_page: bool,
    config: Option<&'a str>,
    profile: Option<&'a str>,
    api_key: Option<&'a str>,
    cookie: Option<&'a str>,
    csrf: Option<&'a str>,
    cookie_file: Option<&'a str>,
    session_cache: Option<&'a str>,
    method: Option<&'a str>,
    interval: Option<&'a str>,
    kind: Option<&'a str>,
    data: Option<&'a str>,
    port_mode: Option<&'a str>,
    native_vlan: Option<&'a str>,
    tagged_vlans: Option<&'a str>,
    poe: Option<&'a str>,
    speed: Option<&'a str>,
    reset: bool,
    with_clients: bool,
    ip: Option<&'a str>,
    network: Option<&'a str>,
    secret_kind: Option<&'a str>,
    standalone: bool,
    anonymous: bool,
    cloud: bool,
    credential_storage: Option<&'a str>,
    insecure: bool,
    insecure_explicit: bool,
    all: bool,
    demo: bool,
    with_secrets: bool,
    allow_secret_removal: bool,
    site: Option<&'a str>,
    console: Option<&'a str>,
    body_file: Option<&'a str>,
    save: Option<&'a str>,
    upload: Option<&'a str>,
    connect_ip: Option<&'a str>,
    ca_cert: Option<&'a str>,
    ca_file: Option<&'a str>,
    tls: Option<&'a str>,
    format: Option<output::Format>,
    timeout: Option<u64>,
    keep_alive: Option<core::time::Duration>,
    count: Option<u64>,
    allow_write: bool,
    quiet: bool,
    verbose: bool,
}
impl Options<'_> {
    fn authentication<'a>(&'a self, key: &'a str) -> unifi::Authentication<'a> {
        if self.anonymous {
            unifi::Authentication::None
        } else if let Some(cookie) = self.cookie {
            unifi::Authentication::Session {
                cookie,
                csrf: self.csrf,
            }
        } else {
            unifi::Authentication::ApiKey(key)
        }
    }
}
struct Url<'a> {
    authority: &'a str,
    host: &'a str,
    base: &'a str,
    port: u16,
    secure: bool,
}
fn parse_url(url: &str) -> Result<Url<'_>, &'static str> {
    let (secure, rest) = if url
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
    {
        (true, &url[8..])
    } else if url
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
    {
        (false, &url[7..])
    } else {
        return Err("URL must start with https:// or http://");
    };
    let (authority, base) = rest
        .find('/')
        .map(|i| rest.split_at(i))
        .unwrap_or((rest, ""));
    let default = if secure { 443 } else { 80 };
    let (host, port) = if let Some(bracket) = authority.strip_prefix('[') {
        let (host, tail) = bracket.split_once(']').ok_or("invalid IPv6 URL")?;
        let port = if tail.is_empty() {
            default
        } else {
            tail.strip_prefix(':')
                .ok_or("invalid port")?
                .parse()
                .map_err(|_| "invalid port")?
        };
        (host, port)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        (host, port.parse().map_err(|_| "invalid port")?)
    } else {
        (authority, default)
    };
    Ok(Url {
        authority,
        host,
        base: base.trim_end_matches('/'),
        port,
        secure,
    })
}
fn request<'b>(
    url: &Url<'_>,
    base: &str,
    key: &str,
    call: &unifi::Call,
    body: Option<Body<'_>>,
    options: &Options<'_>,
    output: &'b mut [u8],
) -> Result<unifi::Response<'b>, &'static str> {
    let Some(path) = options.session_cache else {
        return request_observed(url, base, key, call, body, options, output, &mut |_, _| {
            Ok(())
        });
    };
    let cached = session_cmd::load(path, url)?.ok_or("session cache was removed; log in again")?;
    let mut jar =
        unifi::session::Session::<4096, 512>::from_cookie(&cached.cookie, cached.csrf.as_deref())
            .map_err(|_| "invalid cached session headers")?;
    let outgoing = Options {
        cookie: Some(cached.cookie.as_str()),
        csrf: cached.csrf.as_deref(),
        ..*options
    };
    let response = request_observed(
        url,
        base,
        key,
        call,
        body,
        &outgoing,
        output,
        &mut |name, value| jar.observe(name, value),
    )?;
    let changed = match jar.authentication() {
        Some(unifi::Authentication::Session { cookie, csrf }) => {
            cookie != cached.cookie.as_str() || csrf != cached.csrf.as_deref()
        }
        _ => true,
    };
    if changed {
        session_cmd::store(path, url, &jar)
            .map_err(|_| "request completed, but rotated session credentials could not be saved")?;
    }
    Ok(response)
}
#[allow(clippy::too_many_arguments)]
fn request_observed<'b>(
    url: &Url<'_>,
    base: &str,
    key: &str,
    call: &unifi::Call,
    body: Option<Body<'_>>,
    options: &Options<'_>,
    output: &'b mut [u8],
    observer: &mut HeaderObserver<'_>,
) -> Result<unifi::Response<'b>, &'static str> {
    call.validate(body.map(Body::bytes))
        .map_err(|_| "required query parameter or JSON request body missing")?;
    if let Some(Body::Json(body)) = body {
        unifi::Json::parse(body).map_err(|_| "invalid JSON request body")?;
    }
    let client = Client::with_authentication(url.authority, base, options.authentication(key))
        .map_err(|_| "invalid URL or credentials")?;
    let mut headers = [0; 8192];
    let timeout = options.timeout.unwrap_or(30);
    if options.verbose {
        let _ = writeln!(
            Output(true),
            "{} {}",
            call.operation.method.as_str(),
            call.operation.id
        );
    }
    let tls = options.tls.unwrap_or("auto");
    if !["auto", "embedded", "openssl"].contains(&tls) {
        return Err("TLS backend must be auto, embedded or openssl");
    }
    if url.secure && (tls == "openssl" || (tls == "auto" && options.ca_cert.is_none())) {
        if options.ca_cert.is_some() {
            return Err("OpenSSL uses --ca-file PEM; --ca-cert DER selects embedded TLS");
        }
        let mut stream = openssl::Tls::connect(
            url.host,
            options.connect_ip.unwrap_or(url.host),
            url.port,
            options.ca_file,
            core::time::Duration::from_secs(timeout),
            options.insecure,
        )?;
        return run(client.execute_with(
            &mut stream,
            call,
            body,
            &mut headers,
            output,
            &mut *observer,
        ))
        .map_err(|_| "HTTPS request failed; check the hostname, trust file and /usr/bin/openssl");
    }
    let ip = options
        .connect_ip
        .unwrap_or(url.host)
        .parse::<IpAddr>()
        .map_err(|_| "embedded TLS needs an IP URL or --connect IP")?;
    if url.secure && options.insecure {
        return Err("--insecure requires the system TLS backend");
    }
    if !url.secure && !ip.is_loopback() {
        return Err("plain HTTP is limited to loopback; use HTTPS for a controller");
    }
    let address = SocketAddr::new(ip, url.port);
    let socket = net::socket_with(
        if ip.is_ipv4() {
            AddressFamily::INET
        } else {
            AddressFamily::INET6
        },
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )
    .map_err(|_| "socket failed")?;
    for kind in [net::sockopt::Timeout::Recv, net::sockopt::Timeout::Send] {
        net::sockopt::set_socket_timeout(
            &socket,
            kind,
            Some(core::time::Duration::from_secs(timeout)),
        )
        .map_err(|_| "cannot set timeout")?;
    }
    // Validate the embedded backend trust file before opening a connection.
    let mut ca = [0; 16384];
    let ca = if url.secure {
        Some(read_file(
            options
                .ca_cert
                .ok_or("embedded TLS requires --ca-cert FILE (DER)")?,
            &mut ca,
        )?)
    } else {
        None
    };
    net::connect(&socket, &address).map_err(|_| "connection failed")?;
    let mut socket = Socket(socket);
    if let Some(ca) = ca {
        use rand_core::SeedableRng;
        let mut seed = [0; 32];
        let mut count = 0;
        while count < seed.len() {
            count +=
                rustix::rand::getrandom(&mut seed[count..], rustix::rand::GetRandomFlags::empty())
                    .map_err(|_| "OS randomness unavailable")?;
        }
        let provider = Provider {
            rng: rand_chacha::ChaCha20Rng::from_seed(seed),
            verifier: embedded_tls::pki::CertVerifier::new(embedded_tls::Certificate::X509(ca)),
        };
        let config = embedded_tls::TlsConfig::new().with_server_name(url.host);
        let mut rx = [0; 18432];
        let mut tx = [0; 18432];
        let mut stream = embedded_tls::TlsConnection::<_, embedded_tls::Aes128GcmSha256>::new(
            socket, &mut rx, &mut tx,
        );
        run(stream.open(embedded_tls::TlsContext::new(&config, provider))).map_err(
            |_| "TLS verification or handshake failed (TLS 1.3 and ECDSA/Ed25519 required)",
        )?;
        run(client.execute_with(
            &mut stream,
            call,
            body,
            &mut headers,
            output,
            &mut *observer,
        ))
        .map_err(|_| "HTTP request failed or response exceeded buffer")
    } else {
        run(client.execute_with(
            &mut socket,
            call,
            body,
            &mut headers,
            output,
            &mut *observer,
        ))
        .map_err(|_| "HTTP request failed or response exceeded buffer")
    }
}
type HeaderObserver<'a> = dyn FnMut(&str, &[u8]) -> Result<(), unifi::BuildError> + 'a;
fn string(value: unifi::Json<'_>) -> Result<heapless::String<512>, &'static str> {
    value
        .string()
        .and_then(|s| s.decode())
        .map_err(|_| "invalid string or value exceeds capacity")
}
fn discover_console(
    url: &Url<'_>,
    key: &str,
    options: &Options<'_>,
    buffer: &mut [u8],
) -> Result<heapless::String<512>, &'static str> {
    let mut selected = None;
    let mut token = None::<heapless::String<512>>;
    loop {
        let mut call = unifi::site_manager::list_hosts().unwrap();
        if let Some(token) = &token {
            call = call
                .query("nextToken", token)
                .map_err(|_| "cloud page token exceeds capacity")?;
        }
        let response = request(url, "", key, &call, None, options, buffer)?;
        if !(200..300).contains(&response.status) {
            return Err("cloud console discovery failed; use cloud hosts or --console ID");
        }
        let json = response.json().map_err(|_| "invalid cloud host response")?;
        for host in json
            .required("data")
            .and_then(unifi::Json::items)
            .map_err(|_| "invalid cloud host list")?
        {
            let is_console = host
                .get("type")
                .ok()
                .flatten()
                .and_then(|v| v.string().ok())
                .is_some_and(|s| s.equals("console"));
            let connected = host
                .required("reportedState")
                .and_then(|s| s.required("state"))
                .and_then(unifi::Json::string)
                .is_ok_and(|s| s.equals("connected"));
            if is_console && connected {
                if selected.is_some() {
                    return Err(
                        "multiple connected consoles; choose --console ID from cloud hosts",
                    );
                }
                selected = Some(string(
                    host.required("id").map_err(|_| "cloud host ID missing")?,
                )?);
            }
        }
        let next = json
            .optional("nextToken")
            .map_err(|_| "invalid cloud nextToken")?
            .map(string)
            .transpose()?
            .filter(|s| !s.is_empty());
        if next.is_none() {
            break;
        }
        if next == token {
            return Err("cloud pagination did not advance");
        }
        token = next;
    }
    selected.ok_or("no connected cloud console; inspect cloud hosts")
}
fn discover_site(
    url: &Url<'_>,
    base: &str,
    key: &str,
    selector: Option<&str>,
    options: &Options<'_>,
    buffer: &mut [u8],
) -> Result<heapless::String<512>, &'static str> {
    discover_site_field(url, base, key, selector, "id", options, buffer)
}
fn discover_site_field(
    url: &Url<'_>,
    base: &str,
    key: &str,
    selector: Option<&str>,
    field: &str,
    options: &Options<'_>,
    buffer: &mut [u8],
) -> Result<heapless::String<512>, &'static str> {
    if let Some(id) = selector.filter(|id| field == "id" && lookup::uuid(id)) {
        return heapless::String::try_from(id).map_err(|_| "site ID is too long");
    }
    let mut offset = 0;
    let mut selected = None;
    let mut selected_exact = false;
    let mut ambiguous = false;
    loop {
        let call = unifi::network::get_site_overview_page()
            .unwrap()
            .page(offset, 200)
            .unwrap();
        let response = request(url, base, key, &call, None, options, buffer)?;
        if !(200..300).contains(&response.status) {
            return Err("site discovery failed; use sites list or --param siteId=ID");
        }
        let json = response.json().map_err(|_| "invalid site list response")?;
        let mut count = 0;
        for site in json
            .required("data")
            .and_then(unifi::Json::items)
            .map_err(|_| "invalid site list")?
        {
            count += 1;
            let exact = selector.is_none_or(|wanted| {
                ["id", "name", "internalReference"].iter().any(|field| {
                    site.get(field)
                        .ok()
                        .flatten()
                        .and_then(|v| v.string().ok())
                        .is_some_and(|v| v.equals(wanted))
                })
            });
            let folded = selector.is_some_and(|wanted| {
                ["id", "name", "internalReference"].iter().any(|field| {
                    site.get(field)
                        .ok()
                        .flatten()
                        .and_then(|v| v.string().ok())
                        .is_some_and(|s| {
                            s.chars()
                                .map(|c| c.to_ascii_lowercase())
                                .eq(wanted.chars().map(|c| c.to_ascii_lowercase()))
                        })
                })
            });
            if exact || folded {
                if selected.is_some() && selected_exact && !exact {
                    continue;
                }
                if selected.is_some() && (selected_exact || !exact) {
                    ambiguous = true;
                    continue;
                }
                ambiguous = false;
                selected_exact = exact;
                selected = Some(string(
                    site.required(field).map_err(|_| "site reference missing")?,
                )?);
            }
        }
        offset += count;
        let total = json
            .required("totalCount")
            .and_then(unifi::Json::unsigned)
            .map_err(|_| "invalid site totalCount")?;
        if offset >= total {
            break;
        }
        if count == 0 {
            return Err("site pagination did not advance");
        }
    }
    if ambiguous {
        return Err("multiple sites match; choose --site ID from sites list");
    }
    selected.ok_or("no matching site; inspect sites list")
}
fn app(
    args: &[&str],
    environment: &str,
    storage: &mut storage::Storage,
) -> Result<i32, &'static str> {
    if args.first() == Some(&"__complete") {
        ui::complete(&mut Output(false), &args[1..]).map_err(|_| "output failed")?;
        return Ok(0);
    }
    let mut extra = compat::Flags::new();
    let mut tokens = ArgTokens::new(args.iter().copied());
    let mut positionals = unifi_vec::<8>();
    let site;
    let session_site;
    let resolved_device;
    let resolved_client;
    let resolved_mac;
    let mut count_field = heapless::String::<20>::new();
    let mut qualities_field = heapless::String::<32>::new();
    let expanded_host;
    let saved_theme;
    let profile_site;
    let profile_console;
    let profile_ca_file;
    let profile_ca_cert;
    let profile_tls;
    let cached;
    let cookie_path;
    let mut env_buffer = [0; 8192];
    let mut params = heapless::Vec::<(&str, &str), 16>::new();
    let mut body_fields = heapless::Vec::<(&str, &str), 64>::new();
    let mut typed_options = heapless::Vec::<&str, 64>::new();
    let mut body_json_fields = heapless::Vec::<(&str, &str), 64>::new();
    let mut selector = None;
    let mut queries = heapless::Vec::<(&str, &str), 32>::new();
    let mut options = Options::default();
    let mut help = false;
    while let Some(token) = tokens.next() {
        match token {
            Token::Freestanding("--") => {
                while let Some(value) = tokens.next_value() {
                    positionals.push(value).map_err(|_| "too many arguments")?;
                }
                break;
            }
            Token::Freestanding(s) => positionals.push(s).map_err(|_| "too many arguments")?,
            Token::Long("insecure") | Token::Short(b'k') => {
                options.insecure_explicit = true;
                options.insecure = match tokens.peek_value() {
                    Some("true") => {
                        tokens.next_value();
                        true
                    }
                    Some("false") => {
                        tokens.next_value();
                        false
                    }
                    _ => true,
                };
            }
            Token::Long("ignore-limit") => {
                typed_options
                    .push("ignore-limit")
                    .map_err(|_| "too many options")?;
                body_fields
                    .push(("=ignoreDeviceLimit", "true"))
                    .map_err(|_| "too many fields")?;
            }
            Token::Long("field") => {
                let value = tokens.next_value().ok_or("field needs KEY=JSON")?;
                let (key, value) = value.split_once('=').ok_or("field needs KEY=JSON")?;
                if key.is_empty() {
                    return Err("field name is empty");
                }
                body_json_fields
                    .push((key, value))
                    .map_err(|_| "too many fields")?;
            }
            Token::Long("on") => {
                typed_options.push("on").map_err(|_| "too many options")?;
                let value = tokens.next_value().ok_or("--on needs true or false")?;
                body_fields
                    .push((
                        "cmd",
                        match value {
                            "true" => "set-locate",
                            "false" => "unset-locate",
                            _ => return Err("--on needs true or false"),
                        },
                    ))
                    .map_err(|_| "too many fields")?;
            }
            Token::Long(
                name @ ("name" | "email" | "role" | "description" | "minutes" | "guest-limit"
                | "data-limit-mb" | "data-limit" | "rx-limit-kbps" | "tx-limit-kbps"
                | "rx-limit" | "tx-limit" | "time-limit" | "enabled" | "start" | "end"
                | "attrs" | "macs" | "liveview"),
            ) => {
                let value = tokens.next_value().ok_or("option needs a value")?;
                typed_options.push(name).map_err(|_| "too many options")?;
                let key = match name {
                    "name" | "email" | "role" | "liveview" => name,
                    "description" => "desc",
                    "minutes" | "time-limit" => "=timeLimitMinutes",
                    "guest-limit" => "=authorizedGuestLimit",
                    "data-limit-mb" | "data-limit" => "=dataUsageLimitMBytes",
                    "rx-limit-kbps" | "rx-limit" => "=rxRateLimitKbps",
                    "tx-limit-kbps" | "tx-limit" => "=txRateLimitKbps",
                    "enabled" => "=enabled",
                    "start" => "=start",
                    "end" => "=end",
                    "attrs" => "~attrs",
                    "macs" => "~macs",
                    _ => unreachable!(),
                };
                if name == "enabled" && !["true", "false"].contains(&value) {
                    return Err("enabled needs true or false");
                }
                body_fields
                    .push((key, value))
                    .map_err(|_| "too many body fields")?;
            }
            Token::Long("help") | Token::Short(b'h') => help = true,
            Token::Long("version") | Token::Short(b'V') => {
                output!("unifi {}\n", env!("CARGO_PKG_VERSION"));
                return Ok(0);
            }
            Token::Long("allow-write") | Token::Long("yes") | Token::Short(b'y') => {
                options.allow_write = true
            }
            Token::Long("quiet") | Token::Short(b'q') => options.quiet = true,
            Token::Long("standalone") => options.standalone = true,
            Token::Long("all") | Token::Short(b'a') => options.all = true,
            Token::Long("hide-page") => options.hide_page = true,
            Token::Long("low-quality") => options.low_quality = true,
            Token::Long("id-only") => options.id_only = true,
            Token::Long(name @ ("low" | "medium" | "high" | "package")) => {
                options.qualities |= match name {
                    "low" => 1,
                    "medium" => 2,
                    "high" => 4,
                    _ => 8,
                }
            }
            Token::Long("no-cache") => {}
            Token::Long("no-effects") => options.no_effects = true,
            Token::Long("debug" | "trace") => options.verbose = true,
            Token::Long("reset") => options.reset = true,
            Token::Long("with-clients") => options.with_clients = true,
            Token::Long("demo") => options.demo = true,
            Token::Long("with-secrets") => options.with_secrets = true,
            Token::Long("allow-secret-removal") => options.allow_secret_removal = true,
            Token::Long("verbose") | Token::Short(b'v') => options.verbose = true,
            Token::Short(short @ (b'c' | b's' | b'o' | b'p' | b'F')) => {
                let value = tokens.next_value().ok_or("option needs a value")?;
                match short {
                    b'c' => options.url = Some(value),
                    b'p' => options.profile = Some(value),
                    b'F' => options.body_file = Some(value),
                    b's' => options.site = Some(value),
                    b'o' => options.format = Some(output::Format::parse(value)?),
                    _ => unreachable!(),
                }
            }
            Token::Long(
                name @ ("jpeg-quality"
                | "color"
                | "theme"
                | "log-file"
                | "host"
                | "secret-kind"
                | "credential-storage"
                | "mode"
                | "native-vlan"
                | "tagged-vlans"
                | "poe"
                | "speed"
                | "ip"
                | "network"
                | "type"
                | "sites"
                | "interval"
                | "within"
                | "cookie-file"
                | "method"
                | "data"
                | "config"
                | "profile"
                | "api-key"
                | "from-file"
                | "url"
                | "controller"
                | "site"
                | "console"
                | "output"
                | "param"
                | "query"
                | "body"
                | "connect"
                | "ca-cert"
                | "ca-file"
                | "tls"
                | "timeout"
                | "limit"
                | "offset"
                | "page-limit"
                | "page-offset"
                | "filter"
                | "save"
                | "upload"
                | "count"
                | "keep-alive-interval"),
            ) => {
                let value = tokens.next_value().ok_or("option needs a value")?;
                match name {
                    "jpeg-quality" => {
                        options.jpeg_quality = Some(
                            value
                                .parse::<u8>()
                                .ok()
                                .filter(|n| (1..=100).contains(n))
                                .ok_or("JPEG quality must be 1–100")?,
                        )
                    }
                    "color" => {
                        if !["auto", "always", "never"].contains(&value) {
                            return Err("color must be auto, always or never");
                        }
                        options.color = Some(value);
                    }
                    "theme" => options.theme = Some(value),
                    "log-file" => options.log_file = Some(value),
                    "host" => options.host = Some(value),
                    "secret-kind" => options.secret_kind = Some(value),
                    "credential-storage" => options.credential_storage = Some(value),
                    "mode" => options.port_mode = Some(value),
                    "native-vlan" => options.native_vlan = Some(value),
                    "tagged-vlans" => options.tagged_vlans = Some(value),
                    "poe" => options.poe = Some(value),
                    "speed" => options.speed = Some(value),
                    "ip" => options.ip = Some(value),
                    "network" => options.network = Some(value),
                    "type" => options.kind = Some(value),
                    "sites" => options.metric_sites = Some(value),
                    "interval" => options.interval = Some(value),
                    "within" => queries
                        .push(("within", value))
                        .map_err(|_| "too many queries")?,
                    "cookie-file" => options.cookie_file = Some(value),
                    "method" => options.method = Some(value),
                    "data" => options.data = Some(value),
                    "config" => options.config = Some(value),
                    "profile" => options.profile = Some(value),
                    "api-key" => options.api_key = Some(value),
                    "from-file" => options.body_file = Some(value),
                    "url" => options.command_url = Some(value),
                    "controller" => options.url = Some(value),
                    "site" => options.site = Some(value),
                    "console" => options.console = Some(value),
                    "output" => options.format = Some(output::Format::parse(value)?),
                    "body" => options.body_file = Some(value),
                    "save" => options.save = Some(value),
                    "upload" => options.upload = Some(value),
                    "connect" => options.connect_ip = Some(value),
                    "ca-cert" => options.ca_cert = Some(value),
                    "ca-file" => options.ca_file = Some(value),
                    "tls" => options.tls = Some(value),
                    "count" => {
                        options.count = Some(
                            value
                                .parse::<u64>()
                                .ok()
                                .filter(|n| *n > 0)
                                .ok_or("count must be positive")?,
                        )
                    }
                    "keep-alive-interval" => options.keep_alive = Some(duration::parse(value)?),
                    "timeout" => {
                        let n = value
                            .parse::<u64>()
                            .map_err(|_| "timeout must be 1–3600 seconds")?;
                        if !(1..=3600).contains(&n) {
                            return Err("timeout must be 1–3600 seconds");
                        }
                        options.timeout = Some(n);
                    }
                    "param" => params
                        .push(value.split_once('=').ok_or("parameter needs NAME=VALUE")?)
                        .map_err(|_| "too many parameters")?,
                    "query" => queries
                        .push(value.split_once('=').ok_or("query needs NAME=VALUE")?)
                        .map_err(|_| "too many queries")?,
                    "limit" | "offset" | "filter" => queries
                        .push((name, value))
                        .map_err(|_| "too many queries")?,
                    "page-limit" | "page-offset" => queries
                        .push((
                            if name == "page-limit" {
                                "limit"
                            } else {
                                "offset"
                            },
                            value,
                        ))
                        .map_err(|_| "too many queries")?,
                    _ => unreachable!(),
                }
            }
            Token::Long(name) if compat::known(name) => {
                let value = if compat::boolean(name) {
                    match tokens.peek_value() {
                        Some("true" | "false") => tokens.next_value().unwrap(),
                        _ => "true",
                    }
                } else {
                    tokens.next_value().ok_or("option requires a value")?
                };
                extra.push(name, value)?;
            }
            _ => return Err("unknown option or unexpected attached value; use --help"),
        }
    }
    if positionals.first() == Some(&"help") {
        positionals.remove(0);
        help = true;
    }
    if help || positionals.is_empty() {
        ui::help(&mut Output(false), &positionals).map_err(|_| "output failed")?;
        return Ok(0);
    }
    ui::validate_typed_options(&positionals, &typed_options)?;
    if options.credential_storage.is_some()
        && !matches!(positionals.as_slice(), ["config", "init" | "cloud-setup"])
    {
        return Err("--credential-storage requires config init or cloud-setup");
    }
    if let Some(value) = options.command_url {
        if positionals.starts_with(&["devices", "upgrade"]) {
            body_fields
                .push(("url", value))
                .map_err(|_| "too many body fields")?;
        } else {
            options.url = Some(value);
        }
    }
    if positionals.starts_with(&["alarms", "list"]) {
        options.unarchived = extra.boolean("unarchived")?.unwrap_or(false);
    }
    if positionals.starts_with(&["events", "watch"]) {
        options.event_types = extra.take("types");
    }
    if positionals.starts_with(&["stats", "dpi"])
        && let Some(group) = extra.take("group-by")
    {
        body_fields
            .push((
                "type",
                match group {
                    "app" | "by_app" => "by_app",
                    "category" | "by_cat" => "by_cat",
                    _ => return Err("DPI group must be app or category"),
                },
            ))
            .map_err(|_| "too many body fields")?;
    }
    if op_binary_path(&positionals) {
        options.save = options.save.or_else(|| extra.take("path"));
    }
    validate_body_sources(&options)?;
    if positionals[0] == "completions" || positionals[0] == "completion" {
        if positionals.len() != 2 {
            return Err("usage: unifi completions bash|zsh|fish|powershell|elvish");
        }
        extra.boolean("no-descriptions")?;
        extra.finish()?;
        ui::completions(&mut Output(false), positionals[1])?;
        return Ok(0);
    }
    if positionals[0] == "operations" {
        if positionals.len() > 2 {
            return Err("operations accepts one surface");
        }
        for op in operations(positionals.get(1).copied().unwrap_or("network"))? {
            output!(
                "{}\t{}\t{}\t{}\n",
                op.id,
                op.method.as_str(),
                op.path,
                op.summary
            );
        }
        return Ok(0);
    }
    if let Some(host) = options.host {
        if positionals.starts_with(&["cloud", "devices"]) {
            queries
                .push(("hostIds[]", host))
                .map_err(|_| "too many queries")?;
        } else {
            let mut value = heapless::String::<4096>::new();
            if !host.contains("://") {
                value.push_str("https://").unwrap();
            }
            value.push_str(host).map_err(|_| "host too long")?;
            expanded_host = value;
            options.url = Some(&expanded_host);
        }
    }
    let dotenv = optional_file(".env", &mut env_buffer)?
        .map(core::str::from_utf8)
        .transpose()
        .map_err(|_| ".env must contain UTF-8 text")?
        .unwrap_or("");
    let config = |name| {
        setting(environment, name)
            .or_else(|| environment_value(name))
            .or_else(|| setting(dotenv, name))
    };
    options.config = options.config.or_else(|| config("UNIFI_CONFIG"));
    options.profile = options.profile.or_else(|| config("UNIFI_PROFILE"));
    options.url = options.url.or_else(|| config("UNIFI_URL"));
    options.theme = options.theme.or_else(|| config("UNIFI_THEME"));
    let profile_file = config::File::load(environment, options.config)?;
    saved_theme = profile_file.theme()?;
    options.theme = options.theme.or(saved_theme.as_deref());
    if positionals.first() == Some(&"tui") {
        if positionals.len() != 1 {
            return Err("tui takes no positional arguments");
        }
        let child_environment = profile_file.child_environment(environment)?;
        return tui::run(&options, &child_environment, &mut storage.combined);
    }

    if options.log_file.is_some() {
        return Err("--log-file requires tui");
    }
    if positionals.starts_with(&["config", "set-password"]) {
        return keyring::configure(&profile_file, &positionals, &options, environment);
    }
    if positionals[0] == "config" {
        if positionals.get(1) == Some(&"cloud-setup")
            || positionals.get(1) == Some(&"init")
                && (options.credential_storage.is_some()
                    || options.url.is_none() && options.profile.is_none() && terminal::available())
        {
            if positionals.len() != 2 {
                return Err("setup takes no positional arguments");
            }
            extra.finish()?;
            return setup::run(
                &profile_file,
                positionals.get(1) == Some(&"cloud-setup"),
                &options,
                environment,
                config("UNIFI_API_KEY"),
                config("UNIFI_USERNAME"),
                config("UNIFI_PASSWORD"),
                storage,
            );
        }
        return profile_file.command(&positionals, &options);
    }
    let profile_name = profile_file.active()?;
    let selected_profile = options
        .profile
        .or_else(|| setting(environment, "UNIFI_PROFILE"))
        .unwrap_or(&profile_name);
    if (options.profile.is_some()
        || setting(environment, "UNIFI_PROFILE").is_some()
        || profile_file.has_active_profile()?)
        && !profile_file.has_profile(selected_profile)?
    {
        return Err("selected profile does not exist; inspect config profiles");
    }
    let profile_url = profile_file.value::<4096>(selected_profile, "controller")?;
    profile_site = profile_file.value::<512>(selected_profile, "site")?;
    profile_console = profile_file.value::<512>(selected_profile, "console")?;
    let profile_key = profile_file.value::<4096>(selected_profile, "apiKey")?;
    let profile_key_env = profile_file.value::<128>(selected_profile, "apiKeyEnv")?;
    let profile_env_key = profile_key_env.as_deref().and_then(config);
    options.cloud = profile_file
        .value::<64>(selected_profile, "authMode")?
        .as_deref()
        == Some("cloud");
    if !options.insecure_explicit {
        options.insecure = profile_file
            .field(selected_profile, "insecure")?
            .map(|v| v.boolean().map_err(|_| "profile insecure must be boolean"))
            .transpose()?
            .unwrap_or(false);
    }
    if options.timeout.is_none() {
        options.timeout = profile_file
            .field(selected_profile, "timeout")?
            .map(|v| {
                v.unsigned()
                    .ok()
                    .filter(|n| (1..=3600).contains(n))
                    .ok_or("profile timeout must be 1–3600 seconds")
            })
            .transpose()?;
    }
    profile_ca_file = profile_file.value::<4096>(selected_profile, "caFile")?;
    profile_ca_cert = profile_file.value::<4096>(selected_profile, "caCert")?;
    profile_tls = profile_file.value::<32>(selected_profile, "tls")?;
    options.ca_file = options.ca_file.or(profile_ca_file.as_deref());
    options.ca_cert = options.ca_cert.or(profile_ca_cert.as_deref());
    options.tls = options.tls.or(profile_tls.as_deref());
    if positionals.starts_with(&["cloud", "switch"]) {
        if positionals.len() != 3 {
            return Err("usage: unifi cloud switch SITE");
        }
        if !profile_file.has_profile(selected_profile)? {
            return Err("cloud switch requires an existing profile");
        }
        let url = parse_url(
            options
                .url
                .or(profile_url.as_deref())
                .unwrap_or("https://api.ui.com"),
        )?;
        if !url.host.eq_ignore_ascii_case("api.ui.com") && !options.cloud {
            return Err("cloud switch requires a cloud controller profile");
        }
        let source = options
            .api_key
            .or_else(|| config("UNIFI_API_KEY"))
            .or(profile_env_key)
            .or(profile_key.as_deref())
            .ok_or("cloud switch needs an API key")?;
        let key = if source.starts_with("keyring:") {
            keyring::resolve(source, environment)?
        } else {
            heapless::String::<8192>::try_from(source).map_err(|_| "API key too long")?
        };
        let response = &mut storage.response[..];
        let console = options
            .console
            .or(profile_console.as_deref())
            .map(heapless::String::<512>::try_from)
            .transpose()
            .map_err(|_| "console ID too long")?
            .map_or_else(|| discover_console(&url, &key, &options, response), Ok)?;
        let mut base =
            heapless::String::<4096>::try_from(url.base).map_err(|_| "API base path too long")?;
        if base.is_empty() {
            base.push_str(
                unifi::site_manager::connector_get(&console, "proxy/network/integration")
                    .map_err(|_| "invalid console ID")?
                    .path(),
            )
            .map_err(|_| "API base path too long")?;
        }
        let selected = discover_site_field(
            &url,
            &base,
            &key,
            Some(positionals[2]),
            "internalReference",
            &options,
            response,
        )?;
        profile_file.select_site(selected_profile, &selected)?;
        if !options.quiet {
            output!("Site selector saved to profile {selected_profile}.\n");
        }
        return Ok(0);
    }
    let flow = flows::kind(&positionals);
    let raw = if positionals[0] == "api" {
        if positionals.len() != 2 {
            return Err("usage: unifi api PATH --method METHOD --data JSON");
        }
        let method = match options.method.unwrap_or("get") {
            "get" | "GET" => Method::GET,
            "post" | "POST" => Method::POST,
            "put" | "PUT" => Method::PUT,
            "patch" | "PATCH" => Method::PATCH,
            "delete" | "DELETE" => Method::DELETE,
            "head" | "HEAD" => Method::HEAD,
            "options" | "OPTIONS" => Method::OPTIONS,
            _ => return Err("unsupported HTTP method"),
        };
        let mut path = heapless::String::<4096>::new();
        if !positionals[1].starts_with('/') {
            path.push('/').unwrap();
        }
        path.push_str(positionals[1])
            .map_err(|_| "API path too long")?;
        Some(unifi::Call::raw(method, &path).map_err(|_| "invalid API path")?)
    } else {
        None
    };
    let (mut op, surface, default_body, remaining, route_params) = if let Some(raw) = &raw {
        (raw.operation, Surface::Session, None, &[][..], &[][..])
    } else if positionals[0] == "call" {
        if positionals.len() != 3 {
            return Err("usage: unifi call SURFACE OPERATION");
        }
        let op = *operations(positionals[1])?
            .iter()
            .find(|op| op.id == positionals[2])
            .ok_or("unknown operation; use unifi operations")?;
        (
            op,
            match positionals[1] {
                "site-manager" => Surface::Cloud,
                "protect" => Surface::Protect,
                "session-api" => Surface::Session,
                _ => Surface::Network,
            },
            None,
            &[][..],
            &[][..],
        )
    } else if let Some(flow) = flow {
        (
            flow.operation(),
            if flow == flows::Kind::Topology {
                Surface::Network
            } else {
                Surface::Session
            },
            None,
            &[][..],
            &[][..],
        )
    } else {
        let route = routes::ROUTES
            .iter()
            .filter(|route| positionals.starts_with(route.words))
            .max_by_key(|route| route.words.len());
        let Some(route) = route else {
            if routes::ROUTES
                .iter()
                .any(|route| route.words.starts_with(&positionals))
            {
                ui::help(&mut Output(false), &positionals).map_err(|_| "output failed")?;
                return Ok(0);
            }
            return Err("unknown command; use unifi --help or unifi RESOURCE --help");
        };
        let (route_operation, route_params) =
            if route.words == ["vpn", "peers", "list"] && positionals.len() == 4 {
                (&unifi::session_api::LIST_WIREGUARD_PEERS, &["serverId"][..])
            } else {
                (route.operation, route.params)
            };
        let mut remaining = &positionals[route.words.len()..];
        let supplied_field = |name: &&str| {
            name.strip_prefix('$').is_some_and(|field| {
                body_fields.iter().any(|(key, _)| {
                    key.trim_start_matches(['@', '~', '=']) == field.trim_start_matches('@')
                })
            })
        };
        let expected = route_params
            .iter()
            .filter(|name| !supplied_field(name))
            .count();
        if route_operation.id == "updateSetting"
            && (positionals.starts_with(&["settings", "set"])
                || positionals.starts_with(&["vpn", "settings", "set"]))
            && remaining.len() == expected + 2
        {
            let (key, value) = (remaining[expected], remaining[expected + 1]);
            if unifi::Json::parse(value.as_bytes()).is_ok() {
                body_json_fields
                    .push((key, value))
                    .map_err(|_| "too many fields")?;
            } else {
                body_fields
                    .push((key, value))
                    .map_err(|_| "too many fields")?;
            }
            remaining = &remaining[..expected];
        }
        if route.surface == Surface::Protect && remaining.len() == expected + 1 {
            if route_operation.path.ends_with("/snapshot") {
                if options.save.is_some() {
                    return Err("snapshot destination supplied twice");
                }
                options.save = Some(remaining[expected]);
                remaining = &remaining[..expected];
            } else if route_operation.request_schema.is_some() {
                if options.body_file.is_some() {
                    return Err("body file supplied twice");
                }
                options.body_file = Some(remaining[expected]);
                remaining = &remaining[..expected];
            }
        }
        if remaining.len() != expected
            && !(route_operation.path == "/v1/files/{fileType}"
                && remaining.is_empty()
                && (route_operation.method == Method::GET || options.upload.is_some()))
        {
            return Err("missing or extra resource IDs; use unifi RESOURCE --help");
        }
        (
            route_operation,
            route.surface,
            route.body,
            remaining,
            route_params,
        )
    };
    if options.metric_sites.is_some() && op.id != "queryISPMetrics" {
        return Err("--sites requires cloud isp query");
    }
    if options.hide_page
        && !(surface == Surface::Network
            && op.method == Method::GET
            && op.query.iter().any(|q| q.name == "offset"))
    {
        return Err("--hide-page requires a paged Network list command");
    }
    if options.network.is_some()
        && !matches!(op.id, "createWifiBroadcast" | "updateWifiBroadcast")
        && !matches!(flow, Some(flows::Kind::SetIp | flows::Kind::RemoveIp))
    {
        return Err("--network requires a Wi-Fi or reservation command");
    }
    if options.ip.is_some() && flow != Some(flows::Kind::SetIp) {
        return Err("--ip requires clients set-ip");
    }
    if options.reset && flow != Some(flows::Kind::PortSet) {
        return Err("--reset requires devices port-set");
    }
    if options.with_clients && !matches!(flow, Some(flows::Kind::Ports | flows::Kind::PortsExport))
    {
        return Err("--with-clients requires devices ports or ports-export");
    }
    if options.method.is_some() && positionals[0] != "api" {
        return Err("--method requires an explicit api request");
    }
    if options.count.is_some()
        && op.id != "createVouchers"
        && op.id != "subscribeEvents"
        && !(surface == Surface::Protect && op.path.starts_with("/v1/subscribe/"))
    {
        return Err("--count requires a subscription, TUI or voucher creation");
    }
    if options.qualities != 0 {
        if surface != Surface::Protect
            || !op.path.ends_with("/rtsps-stream")
            || !matches!(op.method, Method::POST | Method::DELETE)
        {
            return Err("quality selection requires cameras stream-create or stream-delete");
        }
        for (bit, name) in [(1, "low"), (2, "medium"), (4, "high"), (8, "package")] {
            if options.qualities & bit != 0 {
                if !qualities_field.is_empty() {
                    qualities_field.push(',').unwrap();
                }
                qualities_field.push_str(name).unwrap();
            }
        }
        if op.method == Method::POST {
            body_fields
                .push(("~qualities", &qualities_field))
                .map_err(|_| "too many fields")?;
        } else {
            queries
                .push(("qualities", &qualities_field))
                .map_err(|_| "too many queries")?;
        }
    }
    if options.low_quality || options.jpeg_quality.is_some() {
        if surface != Surface::Protect || !op.path.ends_with("/snapshot") {
            return Err("snapshot quality flags require cameras snapshot");
        }
        if options.low_quality {
            queries
                .push(("highQuality", "false"))
                .map_err(|_| "too many queries")?;
        }
    }
    if positionals.starts_with(&["acl", "reorder"])
        || positionals.starts_with(&["firewall", "policies", "reorder"])
    {
        let get = extra.boolean("get")?.unwrap_or(false);
        let set = extra.take("set");
        if get && set.is_some() {
            return Err("--get and --set are mutually exclusive");
        }
        let firewall = positionals[0] == "firewall";
        op = if firewall {
            if set.is_some() {
                &unifi::network::UPDATE_FIREWALL_POLICY_ORDERING
            } else {
                &unifi::network::GET_FIREWALL_POLICY_ORDERING
            }
        } else if set.is_some() {
            &unifi::network::UPDATE_ACL_RULE_ORDERING
        } else {
            &unifi::network::GET_ACL_RULE_ORDERING
        };
        if firewall {
            for (flag, query) in [
                ("source-zone", "sourceFirewallZoneId"),
                ("dest-zone", "destinationFirewallZoneId"),
            ] {
                queries
                    .push((
                        query,
                        extra
                            .take(flag)
                            .ok_or("policy reordering needs --source-zone and --dest-zone")?,
                    ))
                    .map_err(|_| "too many queries")?;
            }
        }
        if let Some(set) = set {
            if firewall {
                options.order = Some(set);
                options.order_after = extra.boolean("after-system")?.unwrap_or(false);
            } else {
                for id in compat::csv(set)? {
                    if !lookup::uuid(id) {
                        return Err("ordering requires UUIDs");
                    }
                }
                body_fields
                    .push(("~orderedAclRuleIds", set))
                    .map_err(|_| "too many body fields")?;
            }
        }
    }
    if op.id == "createFirewallPolicy" {
        options.policy_after = extra.boolean("after-system")?.unwrap_or(false);
    }
    if options.id_only && op.method != Method::GET {
        return Err("--id-only requires a GET list command");
    }
    if matches!(op.id, "deleteNetwork" | "deleteWifiBroadcast")
        && let Some(force) = extra.boolean("force")?
    {
        if queries.iter().any(|(key, _)| *key == "force") {
            return Err("force query supplied twice");
        }
        queries
            .push(("force", if force { "true" } else { "false" }))
            .map_err(|_| "too many queries")?;
    }
    validate_body_sources(&options)?;
    if options.standalone {
        op = match op.id {
            "loginUnifiOs" => &unifi::session_api::LOGIN_STANDALONE,
            "logoutUnifiOs" => &unifi::session_api::LOGOUT_STANDALONE,
            _ => op,
        };
    }
    if op.method != Method::GET && !options.allow_write {
        return Err("non-GET operations require --allow-write");
    }
    if op.id == "createVouchers"
        && let Some(count) = options.count
    {
        write!(count_field, "{count}").map_err(|_| "count is too large")?;
        body_json_fields
            .push(("count", &count_field))
            .map_err(|_| "too many fields")?;
    }
    if surface == Surface::Protect
        && op.path == "/v1/files/{fileType}"
        && !params.iter().any(|(name, _)| *name == "fileType")
    {
        let file_type = options
            .kind
            .or_else(|| {
                (op.method == Method::GET)
                    .then(|| remaining.first().copied())
                    .flatten()
            })
            .unwrap_or("animations");
        let mut bytes = [0; 1024];
        let mut writer = unifi::json::ObjectWriter::new(&mut bytes).unwrap();
        writer
            .field("type", &file_type)
            .map_err(|_| "asset type exceeds capacity")?;
        let json = unifi::Json::parse(writer.finish().map_err(|_| "asset type exceeds capacity")?)
            .unwrap();
        unifi::protect::models::AssetFileType::validate(json.required("type").unwrap())
            .map_err(|_| "unsupported Protect asset type")?;
        params
            .push(("fileType", file_type))
            .map_err(|_| "too many parameters")?;
        if op.method == Method::POST {
            if options.upload.is_some() && !remaining.is_empty() {
                return Err("upload file supplied twice");
            }
            options.upload = options.upload.or_else(|| remaining.first().copied());
            validate_body_sources(&options)?;
        }
    }
    let mut values = remaining.iter();
    for name in route_params {
        if name.strip_prefix('$').is_some_and(|field| {
            body_fields.iter().any(|(key, _)| {
                key.trim_start_matches(['@', '~', '=']) == field.trim_start_matches('@')
            })
        }) {
            continue;
        }
        let Some(value) = values.next() else {
            continue;
        };
        let value =
            if positionals.starts_with(&["vpn", "settings"]) && ["key", "?key"].contains(name) {
                present::vpn_key(value)?
            } else {
                *value
            };
        if let Some(field) = name.strip_prefix('!') {
            selector = Some((field, value));
            options.all = true;
            continue;
        }
        if let Some(field) = name.strip_prefix('$') {
            body_fields
                .push((field, value))
                .map_err(|_| "too many body fields")?;
            continue;
        }
        if let Some(field) = name.strip_prefix('?') {
            selector = Some((field, value));
            continue;
        }
        if surface == Surface::Protect && *name == "fileType" {
            continue;
        }
        params
            .push((name, value))
            .map_err(|_| "too many parameters")?;
    }
    options.site = options
        .site
        .or_else(|| config("UNIFI_SITE"))
        .or(profile_site.as_deref());
    options.console = options
        .console
        .or_else(|| config("UNIFI_CONSOLE"))
        .or(profile_console.as_deref());
    if options.format.is_none() {
        options.format = config("UNIFI_OUTPUT")
            .map(output::Format::parse)
            .transpose()?;
    }
    let url = parse_url(
        options
            .url
            .or_else(|| config("UNIFI_URL"))
            .or(profile_url.as_deref())
            .unwrap_or("https://api.ui.com"),
    )?;
    let response_buffer = &mut storage.response[..];
    options.cloud |= url.host.eq_ignore_ascii_case("api.ui.com");
    let cloud_session = surface == Surface::Session && options.cloud;
    let explicit_session_file = options.cookie_file.or_else(|| config("UNIFI_COOKIE_FILE"));
    if cloud_session && (explicit_session_file.is_some() || config("UNIFI_COOKIE").is_some()) {
        return Err(
            "cloud legacy endpoints use an API key; session cookies require a direct controller URL",
        );
    }
    cookie_path = if surface == Surface::Session
        && !cloud_session
        && explicit_session_file.is_none()
        && (profile_file.can_persist() || matches!(op.id, "loginUnifiOs" | "loginStandalone"))
        && (config("UNIFI_COOKIE").is_none() || matches!(op.id, "loginUnifiOs" | "loginStandalone"))
    {
        Some(profile_file.session_path(selected_profile)?)
    } else {
        None
    };
    let session_file = explicit_session_file
        .or(cookie_path.as_deref())
        .unwrap_or("");
    if op.id == "loginUnifiOs" || op.id == "loginStandalone" {
        if cloud_session {
            return Err("session login requires a direct controller URL");
        }
        let profile_user = profile_file.value::<512>(selected_profile, "username")?;
        let profile_password = profile_file.value::<8192>(selected_profile, "password")?;
        let password_env = profile_file.value::<128>(selected_profile, "passwordEnv")?;
        let totp_env = profile_file.value::<128>(selected_profile, "totpEnv")?;
        let password = config("UNIFI_PASSWORD")
            .or_else(|| password_env.as_deref().and_then(config))
            .or(profile_password.as_deref());
        let keyring_password = password
            .filter(|s| s.starts_with("keyring:"))
            .map(|s| keyring::resolve(s, environment))
            .transpose()?;
        return session_cmd::login(
            &url,
            &op.bind(&[]).unwrap(),
            &options,
            session_file,
            config("UNIFI_USERNAME").or(profile_user.as_deref()),
            keyring_password.as_deref().or(password),
            config("UNIFI_TOTP").or_else(|| totp_env.as_deref().and_then(config)),
            response_buffer,
        );
    }
    cached = if surface == Surface::Session
        && config("UNIFI_COOKIE").is_none()
        && !session_file.is_empty()
    {
        session_cmd::load(session_file, &url)?
    } else {
        None
    };
    if surface == Surface::Session
        && explicit_session_file.is_some()
        && config("UNIFI_COOKIE").is_none()
        && cached.is_none()
    {
        return Err("explicit session cache is missing; log in with this --cookie-file first");
    }
    if surface == Surface::Session {
        if let Some(cookie) = config("UNIFI_COOKIE") {
            options.cookie = Some(cookie);
            options.csrf = config("UNIFI_CSRF");
        } else {
            if config("UNIFI_CSRF").is_some() {
                return Err("UNIFI_CSRF requires UNIFI_COOKIE from the same session");
            }
            options.cookie = cached.as_ref().map(|s| s.cookie.as_str());
            options.csrf = cached.as_ref().and_then(|s| s.csrf.as_deref());
            options.session_cache = cached.as_ref().map(|_| session_file);
        }
        if cloud_session
            && matches!(
                op.id,
                "logoutUnifiOs" | "logoutStandalone" | "subscribeEvents"
            )
        {
            return Err("session login/logout and WebSockets require a direct controller URL");
        }
    }

    let keyring_key = if options.cookie.is_none()
        && options.api_key.is_none()
        && config("UNIFI_API_KEY").is_none()
        && profile_env_key.is_none()
    {
        profile_key
            .as_deref()
            .filter(|s| s.starts_with("keyring:"))
            .map(|s| keyring::resolve(s, environment))
            .transpose()?
    } else {
        None
    };
    let key = options
        .api_key
        .or_else(|| config("UNIFI_API_KEY"))
        .or(profile_env_key)
        .or(keyring_key.as_deref())
        .or(profile_key
            .as_deref()
            .filter(|s| !s.starts_with("keyring:")))
        .unwrap_or("");
    if key.is_empty() && surface != Surface::Session {
        return Err("UNIFI_API_KEY is missing");
    }
    if surface == Surface::Session && options.cookie.is_none() && key.is_empty() {
        return Err("session credentials are missing; use session login or UNIFI_COOKIE");
    }
    let mut base =
        heapless::String::<4096>::try_from(url.base).map_err(|_| "API base path is too long")?;
    if surface == Surface::Session {
        if base.ends_with("/proxy/network/integration") {
            base.truncate(base.len() - "/integration".len());
        }
        if cloud_session && base.is_empty() {
            let console = if let Some(id) = options.console {
                heapless::String::<512>::try_from(id).map_err(|_| "console ID too long")?
            } else {
                discover_console(&url, key, &options, response_buffer)?
            };
            base.push_str(
                unifi::site_manager::connector_get(&console, "proxy/network")
                    .map_err(|_| "invalid console ID")?
                    .path(),
            )
            .map_err(|_| "API base path too long")?;
        } else if base.is_empty() && !options.standalone {
            base.push_str("/proxy/network").unwrap();
        }
        if (op.id == "logoutUnifiOs" || op.id == "logoutStandalone") && base == "/proxy/network" {
            base.clear();
        }
    } else if surface != Surface::Cloud && url.base.is_empty() {
        let application = if surface == Surface::Protect {
            "protect"
        } else {
            "network"
        };
        if options.cloud {
            let console = if let Some(id) = options.console {
                heapless::String::try_from(id).map_err(|_| "console ID is too long")?
            } else {
                discover_console(&url, key, &options, response_buffer)?
            };
            base.push_str(
                unifi::site_manager::connector_get(
                    &console,
                    if application == "protect" {
                        "proxy/protect/integration"
                    } else {
                        "proxy/network/integration"
                    },
                )
                .map_err(|_| "invalid console ID")?
                .path(),
            )
            .map_err(|_| "API base path is too long")?;
        } else {
            write!(base, "/proxy/{application}/integration")
                .map_err(|_| "API base path is too long")?;
        }
    } else if surface != Surface::Cloud
        && (url.base.ends_with("/proxy/network") || url.base.ends_with("/proxy/protect"))
    {
        base.push_str("/integration")
            .map_err(|_| "API base path is too long")?;
    }
    if op.path.contains("{siteId}") && !params.iter().any(|(name, _)| *name == "siteId") {
        site = discover_site(&url, &base, key, options.site, &options, response_buffer)?;
        params
            .push(("siteId", &site))
            .map_err(|_| "too many parameters")?;
    }
    if op.path.contains("{site}") && !params.iter().any(|(key, _)| *key == "site") {
        session_site = if cloud_session {
            let mut integration = base.clone();
            integration
                .push_str("/integration")
                .map_err(|_| "API base path too long")?;
            discover_site_field(
                &url,
                &integration,
                key,
                options.site,
                "internalReference",
                &options,
                response_buffer,
            )?
        } else {
            heapless::String::<512>::try_from(options.site.unwrap_or("default"))
                .map_err(|_| "site selector too long")?
        };
        options.site = Some(&session_site);
        params
            .push(("site", &session_site))
            .map_err(|_| "too many parameters")?;
    }
    if flow.is_some() {
        extra.finish()?;
    }
    if let Some(flow @ (flows::Kind::SetIp | flows::Kind::RemoveIp)) = flow {
        let site = params.iter().find(|(name, _)| *name == "site").unwrap().1;
        flows::reservation(
            &mut flows::Remote {
                url: &url,
                base: &base,
                key,
                options: &options,
                response: response_buffer,
            },
            flow,
            &positionals,
            site,
        )?;
        if !options.quiet {
            output!("Reservation updated.\n");
        }
        return Ok(0);
    }
    if let Some(flow @ (flows::Kind::Ports | flows::Kind::PortsExport | flows::Kind::PortSet)) =
        flow
    {
        let site = params.iter().find(|(name, _)| *name == "site").unwrap().1;
        let json = ports::run(
            &mut flows::Remote {
                url: &url,
                base: &base,
                key,
                options: &options,
                response: response_buffer,
            },
            flow,
            &positionals,
            site,
            &body_fields,
            &mut storage.combined,
        )?;
        if flow == flows::Kind::PortsExport && options.format.is_none() {
            options.format = Some(output::Format::Json);
        }
        return present::show(
            json,
            None,
            &positionals,
            &options,
            &mut storage.filtered,
            &mut storage.sanitized,
        );
    }
    if flow == Some(flows::Kind::Topology) {
        if positionals.len() != 1 {
            return Err("topology takes no positional arguments");
        }
        let site = params.iter().find(|(name, _)| *name == "siteId").unwrap().1;
        let json = topology::build(
            &mut flows::Remote {
                url: &url,
                base: &base,
                key,
                options: &options,
                response: response_buffer,
            },
            site,
            &mut storage.combined,
        )?;
        if options.save.is_none() && matches!(options.format, None | Some(output::Format::Table)) {
            if !options.quiet {
                topology::render(&mut Output(false), json, options.demo)
                    .map_err(|_| "topology output exceeds capacity")?;
            }
            return Ok(0);
        }
        return present::show(
            json,
            None,
            &positionals,
            &options,
            &mut storage.filtered,
            &mut storage.sanitized,
        );
    }
    if positionals[0] != "call" && positionals[0] != "api" {
        if let Some(index) = params
            .iter()
            .position(|(name, value)| *name == "deviceId" && !lookup::uuid(value))
        {
            let selector = params[index].1;
            let site = params
                .iter()
                .find(|(name, _)| *name == "siteId")
                .ok_or("site ID missing")?
                .1;
            resolved_device = lookup::resolve(
                &url,
                &base,
                key,
                unifi::network::get_adopted_device_overview_page(site).unwrap(),
                selector,
                "id",
                &options,
                response_buffer,
            )?;
            params[index].1 = &resolved_device;
        }
        if let Some(index) = params
            .iter()
            .position(|(name, value)| *name == "clientId" && !lookup::uuid(value))
        {
            let selector = params[index].1;
            let site = params
                .iter()
                .find(|(name, _)| *name == "siteId")
                .ok_or("site ID missing")?
                .1;
            resolved_client = lookup::resolve(
                &url,
                &base,
                key,
                unifi::network::get_connected_client_overview_page(site).unwrap(),
                selector,
                "id",
                &options,
                response_buffer,
            )?;
            params[index].1 = &resolved_client;
        }
    }
    if let Some(index) = body_fields
        .iter()
        .position(|(name, _)| ["mac", "macAddress", "@macs"].contains(name))
    {
        let value = body_fields[index].1;
        resolved_mac = if let Ok(mac) = lookup::mac(value) {
            mac
        } else {
            if body_fields[index].0 == "macAddress" {
                return Err("adopt requires a MAC address");
            }
            let site = params
                .iter()
                .find(|(name, _)| *name == "site")
                .ok_or("session site missing")?
                .1;
            let list = if positionals[0] == "devices" {
                unifi::session_api::list_devices(site)
            } else {
                unifi::session_api::list_clients(site)
            }
            .unwrap();
            lookup::mac(&lookup::resolve(
                &url,
                &base,
                key,
                list,
                value,
                "mac",
                &options,
                response_buffer,
            )?)?
        };
        body_fields[index].1 = &resolved_mac;
    }
    if op.id == "updateSetting"
        && positionals[0] == "dpi"
        && !params.iter().any(|(key, _)| *key == "key")
    {
        params
            .push(("key", "dpi"))
            .map_err(|_| "too many parameters")?;
    }
    if selector.is_some() && op.method == Method::GET && op.query.iter().any(|p| p.name == "offset")
    {
        options.all = true;
    }
    if op.path.contains("{interval}") && !params.iter().any(|(name, _)| *name == "interval") {
        let interval = match options.interval.unwrap_or("hourly") {
            "5m" => "5minutes",
            s @ ("5minutes" | "hourly" | "daily" | "monthly") => s,
            _ => return Err("interval must be 5m, hourly, daily or monthly"),
        };
        params
            .push(("interval", interval))
            .map_err(|_| "too many parameters")?;
    }
    if (op.id == "getISPMetrics" || op.id == "queryISPMetrics")
        && !params.iter().any(|(name, _)| *name == "type")
    {
        let kind = options.kind.unwrap_or("5m");
        if !["5m", "1h"].contains(&kind) {
            return Err("ISP metric type must be 5m or 1h");
        }
        params
            .push(("type", kind))
            .map_err(|_| "too many parameters")?;
    }
    let mut call = if let Some(raw) = raw {
        raw
    } else {
        op.bind(&params)
            .map_err(|_| "invalid or missing path parameters")?
    };
    if op.id == "getClientRoams" {
        let mac = params
            .iter()
            .find(|(name, _)| *name == "mac")
            .ok_or("client MAC is missing")?
            .1;
        call = call
            .query("mac", mac)
            .and_then(|c| c.query("separateConnectionSignalParam", "false"))
            .map_err(|_| "invalid roam query")?;
    }
    if op.id == "suggestOpenvpnPorts" {
        call = call.query("service", "openvpn").unwrap();
    }
    if op.id == "listWireguardPeers" {
        let id = params
            .iter()
            .find(|(name, _)| *name == "serverId")
            .unwrap()
            .1;
        call = call
            .query("networkId", id)
            .map_err(|_| "invalid WireGuard query")?;
    }
    for (mut name, value) in queries {
        let zone = if matches!(
            op.id,
            "getFirewallPolicyOrdering" | "updateFirewallPolicyOrdering"
        ) && ["sourceFirewallZoneId", "destinationFirewallZoneId"].contains(&name)
            && !matches!(positionals.first(), Some(&"call" | &"api"))
        {
            let site = params
                .iter()
                .find(|(key, _)| *key == "siteId")
                .ok_or("policy ordering needs a site")?
                .1;
            Some(policies::resolve(
                &mut flows::Remote {
                    url: &url,
                    base: &base,
                    key,
                    options: &options,
                    response: response_buffer,
                },
                site,
                value,
                true,
            )?)
        } else {
            None
        };
        if surface == Surface::Cloud
            && name == "limit"
            && op.query.iter().any(|q| q.name == "pageSize")
        {
            name = "pageSize";
        }
        if op.id == "listEvents" && name == "limit" {
            name = "_limit";
        }
        if !op.query.iter().any(|q| q.name == name)
            && matches!(name, "limit" | "offset" | "filter")
            && op.method == Method::GET
            && positionals[0] != "call"
            && positionals[0] != "api"
        {
            match name {
                "limit" => {
                    options.local_limit = Some(value.parse().map_err(|_| "limit must be numeric")?)
                }
                "offset" => {
                    options.local_offset = value.parse().map_err(|_| "offset must be numeric")?
                }
                _ => options.local_filter = Some(value),
            };
            continue;
        }

        call = call
            .query(name, zone.as_deref().unwrap_or(value))
            .map_err(|_| "unsupported query parameter or URL too long")?;
    }
    if (surface == Surface::Protect && op.path.starts_with("/v1/subscribe/"))
        || op.id == "subscribeEvents"
    {
        extra.finish()?;
        return watch::watch(
            &url,
            &base,
            options.authentication(key),
            &call,
            &options,
            response_buffer,
            &mut storage.filtered,
            &mut storage.sanitized,
        );
    }
    let mut body_buffer = [0; 131072];
    let mut cleaned_body = [0; 131072];
    let body = options
        .body_file
        .map(|file| {
            read_file(file, &mut body_buffer).and_then(|bytes| {
                jsonc::parse(bytes, &mut cleaned_body).map(|v| v.raw().as_bytes())
            })
        })
        .transpose()?
        .or(options.data.map(str::as_bytes))
        .or(default_body)
        .or_else(|| {
            (op.id == "logoutUnifiOs" || op.id == "logoutStandalone").then_some(b"{}".as_slice())
        });
    let mut merged_body = [0; 131072];
    let body = if positionals.starts_with(&["vpn", "settings", "patch"]) {
        body.map(|bytes| {
            let json = unifi::Json::parse(bytes).map_err(|_| "invalid VPN settings JSON")?;
            Ok(json
                .get("fields")
                .map_err(|_| "invalid VPN settings JSON")?
                .filter(|v| v.object().is_ok())
                .unwrap_or(json)
                .raw()
                .as_bytes())
        })
        .transpose()?
    } else {
        body
    };
    let body = if op.id == "updateSetting" {
        let site = params
            .iter()
            .find(|(name, _)| *name == "site")
            .ok_or("session site missing")?
            .1;
        let section = params
            .iter()
            .find(|(name, _)| *name == "key")
            .ok_or("setting section missing")?
            .1;
        if positionals.starts_with(&["vpn", "settings"]) && !present::VPN_KEYS.contains(&section) {
            return Err("unknown VPN setting section");
        }
        Some(settings::prepare(
            &url,
            &base,
            key,
            site,
            section,
            &options,
            body,
            &body_fields,
            &body_json_fields,
            response_buffer,
            &mut merged_body,
        )?)
    } else if body_fields.is_empty() && body_json_fields.is_empty() {
        body
    } else {
        Some(payload::merge(
            body,
            &body_fields,
            &body_json_fields,
            &mut merged_body,
        )?)
    };
    if positionals.starts_with(&["stats", "dpi"]) {
        if options.all || options.upload.is_some() {
            return Err("DPI statistics do not accept --all or --upload");
        }
        extra.finish()?;
        let json = stats::dpi(
            &mut flows::Remote {
                url: &url,
                base: &base,
                key,
                options: &options,
                response: response_buffer,
            },
            options.site.unwrap_or("default"),
            body.ok_or("DPI request body missing")?,
            &mut storage.combined,
        )?;
        return present::show(
            json,
            None,
            &positionals,
            &options,
            &mut storage.filtered,
            &mut storage.sanitized,
        );
    }
    let mut advanced_body = [0; 131072];
    let body = builders::prepare(
        &mut flows::Remote {
            url: &url,
            base: &base,
            key,
            options: &options,
            response: response_buffer,
        },
        &call,
        &positionals,
        params
            .iter()
            .find(|(name, _)| *name == "siteId" || *name == "site")
            .map_or("default", |(_, site)| *site),
        body,
        &mut extra,
        &mut advanced_body,
    )?;
    extra.finish()?;
    let mut upload_file = [0; 262144];
    let mut multipart_buffer = [0; 263168];
    let multipart = if let Some(file) = options.upload {
        if options.body_file.is_some() {
            return Err("--body and --upload are mutually exclusive");
        }
        let bytes = read_file(file, &mut upload_file)?;
        let filename = file.rsplit('/').next().ok_or("invalid upload filename")?;
        let media = match filename.rsplit('.').next() {
            Some("gif") => "image/gif",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("png") => "image/png",
            Some("mp3") => "audio/mpeg",
            Some("mp4") => "audio/mp4",
            Some("wav") => "audio/wave",
            Some("caf") => "audio/x-caf",
            _ => return Err("unsupported asset file extension"),
        };
        let mut random = [0; 16];
        fill_random(&mut random)?;
        let mut boundary = heapless::String::<48>::new();
        boundary.push_str("unifi-").unwrap();
        for byte in random {
            write!(boundary, "{byte:02x}").unwrap();
        }
        Some(
            unifi::multipart::Multipart::file(
                &mut multipart_buffer,
                &boundary,
                "file",
                filename,
                media,
                bytes,
            )
            .map_err(|_| "multipart encoding failed")?,
        )
    } else {
        None
    };
    let body = multipart
        .as_ref()
        .map(|m| m.body())
        .or(body.map(Body::Json));
    let ordering_zones = if options.policy_after {
        let json = match body {
            Some(Body::Json(bytes)) => {
                unifi::Json::parse(bytes).map_err(|_| "invalid policy JSON")?
            }
            _ => return Err("--after-system requires a JSON policy body"),
        };
        Some((
            flows::owned::<512>(
                json.required("source")
                    .map_err(|_| "policy needs a source zone for ordering")?,
                "zoneId",
            )?,
            flows::owned::<512>(
                json.required("destination")
                    .map_err(|_| "policy needs a destination zone for ordering")?,
                "zoneId",
            )?,
        ))
    } else {
        None
    };
    if options.all {
        if body.is_some() || op.method != Method::GET {
            return Err("--all requires a GET list operation without a body");
        }
        let output_buffer = &mut storage.combined[..];
        let bytes = pages::collect(
            &url,
            &base,
            key,
            &call,
            &options,
            response_buffer,
            output_buffer,
        )?;
        let json = unifi::Json::parse(bytes).map_err(|_| "invalid combined response")?;
        return present::show(
            json,
            selector,
            &positionals,
            &options,
            &mut storage.filtered,
            &mut storage.sanitized,
        );
    }
    let response = request(&url, &base, key, &call, body, &options, response_buffer)?;
    if !(200..300).contains(&response.status) {
        let _ = writeln!(Output(true), "HTTP {}", response.status);
        if let Ok(mut json) = response.json() {
            if options.demo
                || positionals.first() == Some(&"settings")
                || positionals.first() == Some(&"vpn")
            {
                json = payload::sanitize(
                    json,
                    options.demo,
                    options.with_secrets,
                    &mut storage.sanitized,
                )?;
            }
            let _ = output::render(&mut Output(true), json, output::Format::Json);
        }
        return Ok(1);
    }
    if options.policy_after {
        let json = response
            .json()
            .map_err(|_| "policy created but response JSON was invalid")?;
        let id = flows::owned::<512>(json, "id")?;
        let created = flows::copy(json, &mut storage.combined)?;
        let (source, destination) = ordering_zones.as_ref().unwrap();
        let site = params.iter().find(|(key, _)| *key == "siteId").unwrap().1;
        policies::after_create(
            &mut flows::Remote {
                url: &url,
                base: &base,
                key,
                options: &options,
                response: response_buffer,
            },
            site,
            &id,
            source,
            destination,
        )
        .map_err(|_| "policy was created but reordering failed; inspect firewall policies list")?;
        return present::show(
            created,
            None,
            &positionals,
            &options,
            &mut storage.filtered,
            &mut storage.sanitized,
        );
    }
    let session_json = if surface == Surface::Session && !response.body.is_empty() {
        response.json().ok()
    } else {
        None
    };
    if let Some(json) = session_json
        && json.get("meta").ok().flatten().is_some()
    {
        let rc = json
            .required("meta")
            .and_then(|v| v.required("rc"))
            .and_then(unifi::Json::string)
            .map_err(|_| "invalid session response envelope")?;
        if !rc.equals("ok") {
            let _ = writeln!(Output(true), "session API returned an error envelope");
            return Ok(1);
        }
    }
    if op.id == "logoutUnifiOs" || op.id == "logoutStandalone" {
        match fs::unlink(session_file) {
            Ok(()) | Err(io::Errno::NOENT) => {}
            Err(_) => return Err("logout succeeded but session cache could not be removed"),
        }
        if !options.quiet {
            output!("Logged out.\n");
        }
        return Ok(0);
    }
    if let Some(quality) = options.jpeg_quality {
        if options.demo {
            return Err("demo mode cannot sanitize binary responses");
        }
        let bytes = image::jpeg(response.body, quality, &mut storage.filtered)?;
        if let Some(path) = options.save {
            save_file(path, bytes)?;
        } else if !options.quiet {
            write_bytes(bytes)?;
        }
        return Ok(0);
    }
    if let Some(file) = options.save {
        if options.demo
            || (positionals.first() == Some(&"settings") || positionals.first() == Some(&"vpn"))
                && op.id != "downloadOpenvpnConfiguration"
        {
            let json = response
                .json()
                .map_err(|_| "cannot sanitize non-JSON response")?;
            let json = payload::sanitize(
                json,
                options.demo,
                options.with_secrets,
                &mut storage.sanitized,
            )?;
            save_file(file, json.raw().as_bytes())?;
        } else {
            save_file(file, response.body)?;
        }
        return Ok(0);
    }
    if (surface == Surface::Protect && op.path.ends_with("/snapshot"))
        || op.id == "downloadBackup"
        || op.id == "downloadOpenvpnConfiguration"
    {
        if options.demo {
            return Err("demo mode cannot sanitize binary responses");
        }
        if !options.quiet {
            write_bytes(response.body)?;
        }
        return Ok(0);
    }
    if response.body.is_empty() {
        return Ok(0);
    }
    let mut json = response
        .json()
        .map_err(|_| "server returned invalid JSON")?;
    if surface == Surface::Session && json.get("meta").ok().flatten().is_some() {
        json = unifi::session::data(json).map_err(|_| "session API returned an error envelope")?;
    }
    present::show(
        json,
        selector,
        &positionals,
        &options,
        &mut storage.filtered,
        &mut storage.sanitized,
    )
}

fn op_binary_path(args: &[&str]) -> bool {
    args.starts_with(&["system", "backup", "download"])
        || args.starts_with(&["vpn", "remote-access", "download-config"])
}

fn validate_body_sources(options: &Options<'_>) -> Result<(), &'static str> {
    if usize::from(options.data.is_some())
        + usize::from(options.body_file.is_some())
        + usize::from(options.upload.is_some())
        > 1
    {
        return Err("--body/--from-file, --data and --upload are mutually exclusive");
    }
    Ok(())
}

// A bounded collection for argument references; never allocates.
fn unifi_vec<'a, const N: usize>() -> heapless::Vec<&'a str, N> {
    heapless::Vec::new()
}

/// # Safety
/// Called only by the platform C startup with its valid argc/argv/envp arrays.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(
    argc: c_int,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    // SAFETY: Linux SIGPIPE=13 and SIG_IGN=1; broken TLS/output pipes become I/O errors.
    unsafe {
        signal(13, 1);
    }
    let mut args = heapless::Vec::<&str, 128>::new();
    for i in 1..argc {
        // SAFETY: C startup guarantees argc NUL-terminated arguments.
        let bytes = unsafe { CStr::from_ptr(*argv.add(i as usize)) };
        let Ok(text) = bytes.to_str() else {
            let _ = writeln!(Output(true), "arguments must be UTF-8");
            return 2;
        };
        if args.push(text).is_err() {
            return 2;
        }
    }
    let mut env = heapless::String::<8192>::new();
    let mut i = 0;
    loop {
        // SAFETY: C startup guarantees envp is a NUL-terminated array of strings.
        let ptr = unsafe { *envp.add(i) };
        if ptr.is_null() {
            break;
        }
        let text = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or("");
        if (text.starts_with("UNIFI_")
            || text.starts_with("HOME=")
            || text.starts_with("XDG_CONFIG_HOME=")
            || text.starts_with("XDG_RUNTIME_DIR=")
            || text.starts_with("DBUS_SESSION_BUS_ADDRESS=")
            || text.starts_with("DISPLAY=")
            || text.starts_with("WAYLAND_DISPLAY="))
            && env.push_str(text).and_then(|_| env.push('\0')).is_err()
        {
            return 2;
        }
        i += 1;
    }
    let Some(storage) = storage::take() else {
        return 2;
    };
    match app(&args, &env, storage) {
        Ok(code) => code,
        Err(message) => {
            let _ = writeln!(Output(true), "unifi: {message}");
            if terminal::cancelled() {
                terminal::interrupted_code()
            } else {
                2
            }
        }
    }
}

// Precompiled core can retain a reference to this symbol even with panic=abort.
// Unwinding across this binary is unsupported; terminate if a foreign unwinder invokes it.
#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality() -> ! {
    unsafe { _exit(101) }
}
