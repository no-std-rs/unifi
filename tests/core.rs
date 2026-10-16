use core::{
    future::Future,
    task::{Context, Poll, Waker},
};
use unifi::{Client, Json};

struct Mock {
    input: Vec<u8>,
    cursor: usize,
    output: Vec<u8>,
    fragment: usize,
}
impl Mock {
    fn new(response: &[u8]) -> Self {
        Self {
            input: response.to_vec(),
            cursor: 0,
            output: Vec::new(),
            fragment: 13,
        }
    }
}
impl embedded_io::ErrorType for Mock {
    type Error = embedded_io::ErrorKind;
}
impl embedded_io_async::Read for Mock {
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        let n = bytes
            .len()
            .min(self.input.len() - self.cursor)
            .min(self.fragment);
        bytes[..n].copy_from_slice(&self.input[self.cursor..self.cursor + n]);
        self.cursor += n;
        Ok(n)
    }
}
impl embedded_io_async::Write for Mock {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        let n = bytes.len().min(self.fragment);
        self.output.extend_from_slice(&bytes[..n]);
        Ok(n)
    }
    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
fn run<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("in-memory I/O must complete immediately"),
    }
}

#[test]
fn http_framing_and_errors() {
    for (wire, status, body) in [
        (
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\ntrue\r\n0\r\n\r\n"
                .as_slice(),
            200,
            b"true".as_slice(),
        ),
        (b"HTTP/1.1 204 No Content\r\n\r\n", 204, b""),
        (
            b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 5\r\nRetry-After: 30\r\n\r\nlimit",
            429,
            b"limit",
        ),
        (
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 6\r\n\r\ndenied",
            401,
            b"denied",
        ),
        (b"HTTP/1.1 200 OK\r\n\r\ntrue", 200, b"true"),
    ] {
        let mut mock = Mock::new(wire);
        let mut h = [0; 512];
        let mut b = [0; 128];
        let response = run(Client::new("h", "", "k").unwrap().execute(
            &mut mock,
            &unifi::Call::raw(unifi::Method::GET, "/v1/info").unwrap(),
            None,
            &mut h,
            &mut b,
        ))
        .unwrap();
        assert_eq!(response.status, status);
        assert_eq!(response.body, body);
        if status == 429 {
            assert_eq!(response.retry_after.as_deref(), Some("30"));
        }
        if status >= 400 {
            let error = response.success().unwrap_err();
            assert_eq!(error.body, body);
            if status == 429 {
                assert_eq!(error.retry_after.as_deref(), Some("30"));
            }
        }
    }
    for wire in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n1234".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n12345",
    ] {
        let mut mock = Mock::new(wire);
        let mut h = [0; 512];
        let mut b = [0; 4];
        assert!(
            run(Client::new("h", "", "k").unwrap().execute(
                &mut mock,
                &unifi::Call::raw(unifi::Method::GET, "/v1/info").unwrap(),
                None,
                &mut h,
                &mut b
            ))
            .is_err()
        );
    }
}

#[test]
fn object_writer_handles_escaping_and_capacity_atomically() {
    let mut bytes = [0; 128];
    let mut writer = unifi::json::ObjectWriter::new(&mut bytes).unwrap();
    writer
        .field("name", "café\n\"")
        .unwrap()
        .field("enabled", &true)
        .unwrap();
    writer
        .raw_field("nested", Json::parse(br#"{"x":1}"#).unwrap())
        .unwrap();
    let value = Json::parse(writer.finish().unwrap()).unwrap();
    assert!(
        value
            .required("name")
            .unwrap()
            .string()
            .unwrap()
            .equals("café\n\"")
    );
    let mut bytes = [0; 16];
    let mut writer = unifi::json::ObjectWriter::new(&mut bytes).unwrap();
    assert!(writer.field("long", "this value is too long").is_err());
    writer.field("x", &1).unwrap();
    assert_eq!(writer.finish().unwrap(), b"{\"x\":1}");
}

#[cfg(feature = "serde")]
#[test]
fn optional_serde_remains_allocation_free_for_owned_strings() {
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Payload {
        name: heapless::String<64>,
        enabled: bool,
    }
    let value = Json::parse(br#"{"name":"caf\u00e9","enabled":true}"#).unwrap();
    let payload: Payload = value.decode(&mut [0; 64]).unwrap();
    assert_eq!(payload.name, "café");
    let mut bytes = [0; 128];
    let encoded = unifi::json::encode_serde(&payload, &mut bytes).unwrap();
    assert!(
        Json::parse(encoded)
            .unwrap()
            .required("name")
            .unwrap()
            .string()
            .unwrap()
            .equals("café")
    );
}

#[test]
fn rejects_ambiguous_framing_and_unsupported_compression() {
    for wire in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\nx".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Length: nonsense\r\n\r\nx",
        b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nx",
        b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 1\r\n\r\nx",
    ] {
        let mut mock = Mock::new(wire);
        let mut headers = [0; 512];
        let mut body = [0; 128];
        assert!(
            run(Client::new("h", "", "k").unwrap().execute(
                &mut mock,
                &unifi::Call::raw(unifi::Method::GET, "/v1/info").unwrap(),
                None,
                &mut headers,
                &mut body
            ))
            .is_err()
        );
    }
}

#[test]
fn restored_session_pairs_rotate_transactionally_and_reject_header_injection() {
    use unifi::{Authentication, session::Session};
    let mut jar = Session::<32, 16>::from_cookie("TOKEN=one; other=two", Some("csrf-one")).unwrap();
    jar.observe("Set-Cookie", b"TOKEN=three; Path=/").unwrap();
    jar.observe("X-CSRF-Token", b"csrf-two").unwrap();
    assert!(matches!(
        jar.authentication(),
        Some(Authentication::Session {
            cookie: "other=two; TOKEN=three",
            csrf: Some("csrf-two")
        })
    ));
    assert!(
        jar.observe("Set-Cookie", b"too-long=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .is_err()
    );
    assert!(matches!(
        jar.authentication(),
        Some(Authentication::Session {
            cookie: "other=two; TOKEN=three",
            ..
        })
    ));
    for cookie in [
        "",
        "TOKEN=x; Secure",
        "TOKEN=x\n",
        "TOKEN=x\r\nInjected: header",
    ] {
        assert!(Session::<32, 16>::from_cookie(cookie, None).is_err());
    }
    assert!(Session::<32, 16>::from_cookie("TOKEN=x", Some("csrf\n")).is_err());
}
