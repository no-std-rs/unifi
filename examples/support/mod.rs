//! Host-only verified TLS adapter; all HTTP requests use the no_std unifi client.
use std::{
    future::Future,
    io::{Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    task::{Context, Poll, Waker},
};
use unifi::{Client, Method};

struct Tls {
    child: Child,
    tx: ChildStdin,
    rx: ChildStdout,
}
impl Tls {
    fn connect() -> Self {
        let mut child = Command::new("openssl")
            .args([
                "s_client",
                "-quiet",
                "-verify_return_error",
                "-verify_hostname",
                "api.ui.com",
                "-servername",
                "api.ui.com",
                "-connect",
                "api.ui.com:443",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("openssl is required");
        Self {
            tx: child.stdin.take().unwrap(),
            rx: child.stdout.take().unwrap(),
            child,
        }
    }
}
impl Drop for Tls {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl embedded_io::ErrorType for Tls {
    type Error = embedded_io::ErrorKind;
}
impl embedded_io_async::Read for Tls {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.rx.read(buf).map_err(|_| embedded_io::ErrorKind::Other)
    }
}
impl embedded_io_async::Write for Tls {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.tx
            .write(buf)
            .map_err(|_| embedded_io::ErrorKind::Other)
    }
    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.tx.flush().map_err(|_| embedded_io::ErrorKind::Other)
    }
}
fn run<F: Future>(f: F) -> F::Output {
    let mut f = core::pin::pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    match f.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("blocking I/O yielded"),
    }
}
pub fn get(client: &Client<'_>, call: unifi::Call) -> Option<Vec<u8>> {
    assert_eq!(
        call.operation.method,
        Method::GET,
        "live verification allows GET only"
    );
    let mut io = Tls::connect();
    let mut headers = vec![0; 16384];
    let mut body = vec![0; 2 * 1024 * 1024];
    let response =
        run(client.execute(&mut io, &call, None, &mut headers, &mut body)).expect("HTTP transport");
    println!(
        "{} HTTP {} ({} bytes)",
        call.operation.id,
        response.status,
        response.body.len()
    );
    if !(200..300).contains(&response.status) {
        if let Ok(json) = response.json()
            && let Ok(Some(code)) = json.get("code")
            && let Ok(code) = code.string().and_then(|s| s.decode::<256>())
        {
            println!("API code: {code}");
        }
        return None;
    }
    Some(response.body.to_vec())
}

pub fn api_key() -> String {
    let dotenv = std::fs::read_to_string(".env").unwrap_or_default();
    std::env::var("UNIFI_API_KEY")
        .ok()
        .or_else(|| {
            dotenv
                .lines()
                .find_map(|l| l.strip_prefix("UNIFI_API_KEY="))
                .map(|k| k.trim().trim_matches(['\'', '"']).to_owned())
        })
        .expect("UNIFI_API_KEY")
}
