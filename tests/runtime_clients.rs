//! Real loopback HTTP/TLS contracts shared by both optional host interfaces.
#[cfg(any(feature = "std", feature = "tokio", feature = "async_std"))]
mod host {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, mpsc},
        thread,
        time::Duration,
    };
    use unifi::{
        Authentication, Body, Call, Method, Operation,
        host::{Error, Options, Response},
        session::Session,
    };

    static GET: Operation = Operation {
        id: "hostFixture",
        method: Method::GET,
        path: "/v1/items/{id}",
        summary: "fixture",
        body_required: false,
        query: &[],
        request_schema: None,
        response_schema: None,
    };
    static POST: Operation = Operation {
        method: Method::POST,
        body_required: true,
        ..GET
    };
    fn call() -> Call {
        GET.bind(&[("id", "a/b ?#")]).unwrap()
    }
    fn read_request(io: &mut (impl Read + Write)) -> String {
        let mut bytes = Vec::new();
        let mut byte = [0];
        while !bytes.ends_with(b"\r\n\r\n") {
            io.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
        }
        let headers = String::from_utf8(bytes).unwrap();
        let length = headers
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .map(|(_, v)| v.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0; length];
        io.read_exact(&mut body).unwrap();
        headers + &String::from_utf8(body).unwrap()
    }
    fn serve(
        replies: Vec<&'static str>,
        tls: Option<Arc<rustls::ServerConfig>>,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let scheme = if tls.is_some() { "https" } else { "http" };
        let (tx, rx) = mpsc::channel();
        let task = thread::spawn(move || {
            for reply in replies {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                if let Some(config) = tls.as_ref() {
                    let mut io = rustls::StreamOwned::new(
                        rustls::ServerConnection::new(config.clone()).unwrap(),
                        stream,
                    );
                    // An untrusted client can terminate its handshake without sending HTTP.
                    if io.conn.complete_io(&mut io.sock).is_err() {
                        continue;
                    }
                    tx.send(read_request(&mut io)).unwrap();
                    let _ = io.write_all(reply.as_bytes());
                    let _ = io.flush();
                } else {
                    let mut io = stream;
                    tx.send(read_request(&mut io)).unwrap();
                    io.write_all(reply.as_bytes()).unwrap();
                }
            }
        });
        (format!("{scheme}://localhost:{port}"), rx, task)
    }
    trait Driver: Sized {
        fn new(url: &str, auth: Authentication<'_>, options: Options) -> Result<Self, Error>;
        fn execute(&self, call: &Call, body: Option<Body<'_>>) -> Result<Response, Error>;
        fn session(
            &self,
            call: &Call,
            body: Option<Body<'_>>,
            session: &mut Session,
        ) -> Result<Response, Error>;
    }
    #[cfg(feature = "std")]
    struct Blocking(unifi::blocking::Client);
    #[cfg(feature = "std")]
    impl Driver for Blocking {
        fn new(url: &str, auth: Authentication<'_>, options: Options) -> Result<Self, Error> {
            unifi::blocking::Client::with_options(url, "/proxy/network/integration", auth, options)
                .map(Self)
        }
        fn execute(&self, call: &Call, body: Option<Body<'_>>) -> Result<Response, Error> {
            self.0.execute_body(call, body)
        }
        fn session(
            &self,
            call: &Call,
            body: Option<Body<'_>>,
            session: &mut Session,
        ) -> Result<Response, Error> {
            self.0.execute_with_session(call, body, session)
        }
    }
    #[cfg(feature = "tokio")]
    struct Async {
        client: unifi::tokio::Client,
        runtime: tokio::runtime::Runtime,
    }
    #[cfg(feature = "tokio")]
    impl Driver for Async {
        fn new(url: &str, auth: Authentication<'_>, options: Options) -> Result<Self, Error> {
            Ok(Self {
                client: unifi::tokio::Client::with_options(
                    url,
                    "/proxy/network/integration",
                    auth,
                    options,
                )?,
                runtime: tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap(),
            })
        }
        fn execute(&self, call: &Call, body: Option<Body<'_>>) -> Result<Response, Error> {
            self.runtime.block_on(self.client.execute_body(call, body))
        }
        fn session(
            &self,
            call: &Call,
            body: Option<Body<'_>>,
            session: &mut Session,
        ) -> Result<Response, Error> {
            self.runtime
                .block_on(self.client.execute_with_session(call, body, session))
        }
    }
    #[cfg(feature = "async_std")]
    struct AsyncStd(unifi::async_std::Client);
    #[cfg(feature = "async_std")]
    impl Driver for AsyncStd {
        fn new(url: &str, auth: Authentication<'_>, options: Options) -> Result<Self, Error> {
            unifi::async_std::Client::with_options(url, "/proxy/network/integration", auth, options)
                .map(Self)
        }
        fn execute(&self, call: &Call, body: Option<Body<'_>>) -> Result<Response, Error> {
            ::async_std::task::block_on(self.0.execute_body(call, body))
        }
        fn session(
            &self,
            call: &Call,
            body: Option<Body<'_>>,
            session: &mut Session,
        ) -> Result<Response, Error> {
            ::async_std::task::block_on(self.0.execute_with_session(call, body, session))
        }
    }
    fn wire_and_errors<D: Driver>() {
        let (url, requests, task) = serve(
            vec![
                "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 4\r\nRetry-After: 19\r\nConnection: close\r\n\r\nslow",
            ],
            None,
        );
        let client = D::new(
            &url,
            Authentication::ApiKey("fixture-only-key"),
            Options::default(),
        )
        .unwrap();
        let body = br#"{"enabled":true}"#;
        let response = client
            .execute(
                &POST.bind(&[("id", "a/b ?#")]).unwrap(),
                Some(Body::Json(body)),
            )
            .unwrap();
        let error = response.success().unwrap_err();
        assert_eq!(error.status, 429);
        assert_eq!(error.body, b"slow");
        assert_eq!(error.retry_after.as_deref(), Some("19"));
        let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            request.starts_with(
                "POST /proxy/network/integration/v1/items/a%2Fb%20%3F%23 HTTP/1.1\r\n"
            ),
            "{request}"
        );
        let request = request.to_lowercase();
        assert!(request.contains("x-api-key: fixture-only-key\r\n"));
        assert!(request.contains("accept-encoding: identity\r\n"));
        assert!(request.ends_with(core::str::from_utf8(body).unwrap()));
        task.join().unwrap();
    }
    fn preflight<D: Driver>() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let client = D::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            Authentication::None,
            Options::default(),
        )
        .unwrap();
        assert!(
            client
                .execute(&POST.bind(&[("id", "id")]).unwrap(), None)
                .is_err()
        );
        assert!(
            client
                .execute(&call(), Some(Body::Json(b"{broken")))
                .is_err()
        );
        assert!(
            client
                .execute(&GET.bind(&[("id", "..")]).unwrap(), None)
                .is_err()
        );
        assert!(
            client
                .execute(
                    &call(),
                    Some(Body::Bytes {
                        bytes: b"data",
                        content_type: "text/plain\r\nx-injected: yes"
                    })
                )
                .is_err()
        );
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        for url in [
            "https://user:pass@example.com",
            "https://example.com/base",
            "https://example.com?x=y",
            "ftp://example.com",
            "https://example.com/#fragment",
        ] {
            assert!(
                D::new(url, Authentication::None, Options::default()).is_err(),
                "{url}"
            );
        }
        assert!(
            D::new(
                "http://localhost",
                Authentication::ApiKey("bad\r\nkey"),
                Options::default()
            )
            .is_err()
        );
    }
    fn limits_redirects_and_encoding<D: Driver>() {
        for (reply, kind) in [
            (
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:1/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                0,
            ),
            (
                "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n12345",
                1,
            ),
            (
                "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 3\r\nConnection: close\r\n\r\nzip",
                2,
            ),
            (
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n",
                1,
            ),
            (
                "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nshort",
                3,
            ),
        ] {
            let (url, requests, task) = serve(vec![reply], None);
            let client = D::new(
                &url,
                Authentication::ApiKey("fixture-only"),
                Options {
                    max_response_bytes: 4,
                    ..Options::default()
                },
            )
            .unwrap();
            match (client.execute(&call(), None), kind) {
                (Ok(response), 0) => assert_eq!(response.status, 307),
                (Err(Error::ResponseTooLarge), 1)
                | (Err(Error::UnsupportedEncoding), 2)
                | (Err(_), 3) => {}
                (result, _) => panic!("unexpected result {result:?} for case {kind}"),
            }
            requests.recv_timeout(Duration::from_secs(5)).unwrap();
            task.join().unwrap();
        }
    }
    fn session_rotation<D: Driver>() {
        let (url, requests, task) = serve(
            vec![
                "HTTP/1.1 200 OK\r\nSet-Cookie: TOKEN=first; Path=/\r\nSet-Cookie: AUX=kept; Path=/\r\nX-CSRF-Token: csrf-first\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                "HTTP/1.1 200 OK\r\nSet-Cookie: TOKEN=second; Path=/\r\nX-Updated-CSRF-Token: csrf-second\r\nContent-Length: 7\r\nConnection: close\r\n\r\nshort",
            ],
            None,
        );
        let client = D::new(
            &url,
            Authentication::ApiKey("must-not-be-sent"),
            Options::default(),
        )
        .unwrap();
        let mut session = Session::default();
        client.session(&call(), None, &mut session).unwrap();
        let first = requests
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .to_lowercase();
        assert!(!first.contains("x-api-key:"));
        assert!(!first.contains("cookie:"));
        assert!(client.session(&call(), None, &mut session).is_err());
        let second = requests
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .to_lowercase();
        assert!(second.contains("cookie: token=first; aux=kept\r\n"));
        assert!(second.contains("x-csrf-token: csrf-first\r\n"));
        match session.authentication().unwrap() {
            Authentication::Session { cookie, csrf } => {
                assert_eq!(cookie, "AUX=kept; TOKEN=second");
                assert_eq!(csrf, Some("csrf-second"));
            }
            _ => panic!("expected session"),
        };
        task.join().unwrap();
    }
    fn verified_tls<D: Driver>() {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(signing_key.serialize_der());
        let config = Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert.der().clone()], key.into())
                .unwrap(),
        );
        let (url, requests, task) = serve(
            vec!["HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"; 2],
            Some(config),
        );
        let untrusted = D::new(
            &url,
            Authentication::ApiKey("fixture-only"),
            Options::default(),
        )
        .unwrap();
        assert!(untrusted.execute(&call(), None).is_err());
        let roots = vec![unifi::host::Certificate::from_der(cert.der()).unwrap()];
        let trusted = D::new(
            &url,
            Authentication::ApiKey("fixture-only"),
            Options {
                root_certificates: roots,
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(trusted.execute(&call(), None).unwrap().status, 200);
        assert!(
            requests
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .contains("fixture-only")
        );
        assert!(requests.try_recv().is_err());
        task.join().unwrap();
    }
    macro_rules! suite {
        ($name:ident,$driver:ty) => {
            mod $name {
                use super::*;
                #[test]
                fn wire_errors() {
                    wire_and_errors::<$driver>()
                }
                #[test]
                fn validation_before_io() {
                    preflight::<$driver>()
                }
                #[test]
                fn bounded_no_redirect_no_decompression() {
                    limits_redirects_and_encoding::<$driver>()
                }
                #[test]
                fn rotates_session_even_on_body_failure() {
                    session_rotation::<$driver>()
                }
                #[test]
                fn requires_trusted_tls() {
                    verified_tls::<$driver>()
                }
            }
        };
    }
    #[cfg(feature = "std")]
    suite!(blocking, Blocking);
    #[cfg(feature = "tokio")]
    suite!(tokio_async, Async);
    #[cfg(feature = "async_std")]
    suite!(async_std_runtime, AsyncStd);
}

mod embedded_async {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    use unifi::embedded_async::{Client as EmbeddedClient, Connector};
    use unifi::{Authentication, Body, Client, session::Session, site_manager};
    #[derive(Default)]
    struct State {
        connects: Cell<usize>,
        drops: Cell<usize>,
        hang: Cell<bool>,
        requests: RefCell<Vec<Vec<u8>>>,
    }
    struct Factory(Rc<State>);
    struct Stream {
        state: Rc<State>,
        offset: usize,
        request: Vec<u8>,
    }
    impl Drop for Stream {
        fn drop(&mut self) {
            self.state.drops.set(self.state.drops.get() + 1);
            self.state.requests.borrow_mut().push(self.request.clone());
        }
    }
    impl embedded_io::ErrorType for Stream {
        type Error = embedded_io::ErrorKind;
    }
    impl embedded_io_async::Read for Stream {
        async fn read(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
            tokio::task::yield_now().await;
            if self.state.hang.get() {
                core::future::pending::<()>().await;
            }
            let bytes=b"HTTP/1.1 200 OK\r\nSet-Cookie: TOKEN=rotated; Path=/\r\nX-CSRF-Token: csrf-new\r\nContent-Length: 2\r\n\r\n{}";
            let n = out.len().min(3).min(bytes.len() - self.offset);
            out[..n].copy_from_slice(&bytes[self.offset..self.offset + n]);
            self.offset += n;
            Ok(n)
        }
    }
    impl embedded_io_async::Write for Stream {
        async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
            tokio::task::yield_now().await;
            let n = bytes.len().min(3);
            self.request.extend_from_slice(&bytes[..n]);
            Ok(n)
        }
        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }
    impl Connector for Factory {
        type Error = core::convert::Infallible;
        type Connection<'a> = Stream;
        async fn connect(&mut self) -> Result<Stream, Self::Error> {
            self.0.connects.set(self.0.connects.get() + 1);
            tokio::task::yield_now().await;
            Ok(Stream {
                state: self.0.clone(),
                offset: 0,
                request: Vec::new(),
            })
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn yields_reconnects_and_rotates_sessions() {
        let state = Rc::new(State::default());
        let mut headers = [0; 1024];
        let mut output = [0; 1024];
        let core = Client::with_authentication("fixture.local", "", Authentication::None).unwrap();
        let mut client = EmbeddedClient::new(core, Factory(state.clone()), &mut headers);
        let call = site_manager::list_hosts().unwrap();
        assert!(
            client
                .execute_body(&call, Some(Body::Json(b"broken")), &mut output)
                .await
                .is_err()
        );
        assert_eq!(state.connects.get(), 0);
        let mut session: Session = Session::default();
        assert_eq!(
            client
                .execute_with_session(&call, None, &mut output, &mut session)
                .await
                .unwrap()
                .body,
            b"{}"
        );
        client
            .execute_with_session(&call, None, &mut output, &mut session)
            .await
            .unwrap();
        assert_eq!(state.connects.get(), 2);
        assert_eq!(state.drops.get(), 2);
        let requests = state.requests.borrow();
        let first = String::from_utf8_lossy(&requests[0]);
        let second = String::from_utf8_lossy(&requests[1]);
        assert!(!first.contains("Cookie:"));
        assert!(second.contains("Cookie: TOKEN=rotated\r\n"));
        assert!(second.contains("X-CSRF-Token: csrf-new\r\n"));
    }
    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_drops_connection_without_replaying() {
        let state = Rc::new(State::default());
        state.hang.set(true);
        let mut headers = [0; 1024];
        let mut output = [0; 1024];
        let mut client = EmbeddedClient::new(
            Client::new("fixture.local", "", "fixture-key").unwrap(),
            Factory(state.clone()),
            &mut headers,
        );
        let call = site_manager::list_hosts().unwrap();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                client.execute(&call, None, &mut output)
            )
            .await
            .is_err()
        );
        assert_eq!(state.connects.get(), 1);
        assert_eq!(state.drops.get(), 1);
        state.hang.set(false);
        client.execute(&call, None, &mut output).await.unwrap();
        assert_eq!(state.connects.get(), 2);
        assert_eq!(state.drops.get(), 2);
    }
}
