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

#[test]
fn object_equality_uses_the_last_value_for_each_decoded_key() {
    use unifi::validation::{equivalent, unique};
    for (left, right, expected) in [
        (r#"{"x":0,"x":0}"#, r#"{"x":0,"x":1}"#, false),
        (r#"{"x":0,"x":1}"#, r#"{"x":1}"#, true),
        (r#"{"x":0,"\u0078":1}"#, r#"{"x":1}"#, true),
        (r#"{"x":1,"y":2}"#, r#"{"y":2,"x":1}"#, true),
        (r#"{"x":1,"y":2}"#, r#"{"x":1}"#, false),
    ] {
        let a = Json::parse(left.as_bytes()).unwrap();
        let b = Json::parse(right.as_bytes()).unwrap();
        assert_eq!(equivalent(a, b), expected, "{left} versus {right}");
        assert_eq!(equivalent(b, a), expected, "{right} versus {left}");
        for array in [format!("[{left},{right}]"), format!("[{right},{left}]")] {
            assert_eq!(
                unique(Json::parse(array.as_bytes()).unwrap()).is_err(),
                expected
            );
        }
    }
}

#[test]
fn identical_numbers_outside_f64_range_are_duplicates() {
    use unifi::validation::{equivalent, unique};
    let value = Json::parse(b"1e400").unwrap();
    assert!(equivalent(value, value));
    assert!(unique(Json::parse(b"[1e400,1e400]").unwrap()).is_err());
    assert!(!equivalent(value, Json::parse(b"1e401").unwrap()));
    assert!(unique(Json::parse(b"[1e400,1e401]").unwrap()).is_ok());
}

#[test]
fn numeric_equality_preserves_decimal_precision_and_exponents() {
    use unifi::validation::{equivalent, unique};
    for (left, right, expected) in [
        ("9223372036854775808", "9223372036854775809", false),
        ("-9223372036854775809", "-9223372036854775810", false),
        ("18446744073709551616", "18446744073709551617", false),
        ("1.0000000000000000001", "1.0000000000000000002", false),
        ("1", "1.0", true),
        ("1e400", "10e399", true),
        ("1e-400", "0", false),
        ("1e1", "100e-1", true),
        ("1.20e+0002", "120", true),
        ("-0e99999999999999999999999999999999999999999", "0", true),
        (
            "1e1000000000000000000000000000000000000000000",
            "10e999999999999999999999999999999999999999999",
            true,
        ),
        (
            "1e-1000000000000000000000000000000000000000000",
            "0.1e-999999999999999999999999999999999999999999",
            true,
        ),
        (
            "1e1000000000000000000000000000000000000000000",
            "1e999999999999999999999999999999999999999999",
            false,
        ),
    ] {
        let a = Json::parse(left.as_bytes()).unwrap();
        let b = Json::parse(right.as_bytes()).unwrap();
        assert_eq!(equivalent(a, b), expected, "{left} versus {right}");
        assert_eq!(equivalent(b, a), expected, "{right} versus {left}");
        let array = format!("[{left},{right}]");
        assert_eq!(
            unique(Json::parse(array.as_bytes()).unwrap()).is_err(),
            expected
        );
    }
}

#[test]
fn authenticated_response_rotates_the_same_session_using_a_snapshot() {
    use unifi::{Authentication, session::Session};
    let mut session = Session::<64, 32>::from_cookie("TOKEN=old", Some("csrf-old")).unwrap();
    let credentials = session.clone();
    let client =
        Client::with_authentication("h", "", credentials.authentication().unwrap()).unwrap();
    let mut mock = Mock::new(b"HTTP/1.1 200 OK\r\nSet-Cookie: TOKEN=new; Path=/\r\nX-CSRF-Token: csrf-new\r\nContent-Length: 0\r\n\r\n");
    run(client.execute_with_session(
        &mut mock,
        &unifi::Call::raw(unifi::Method::GET, "/session").unwrap(),
        None,
        &mut [0; 512],
        &mut [0; 128],
        &mut session,
    ))
    .unwrap();
    let request = core::str::from_utf8(&mock.output).unwrap();
    assert!(request.contains("Cookie: TOKEN=old\r\n"));
    assert!(request.contains("X-CSRF-Token: csrf-old\r\n"));
    assert!(matches!(
        session.authentication(),
        Some(Authentication::Session {
            cookie: "TOKEN=new",
            csrf: Some("csrf-new")
        })
    ));
}

#[test]
fn removing_the_last_cookie_clears_csrf_before_a_new_session() {
    use unifi::{Authentication, session::Session};
    for deletion in [
        "TOKEN=; Max-Age=0",
        "TOKEN=deleted; Max-Age=0",
        "TOKEN=deleted; Max-Age=-1",
        "TOKEN=",
    ] {
        let mut session = Session::<64, 32>::from_cookie("TOKEN=old", Some("csrf-old")).unwrap();
        session.observe("Set-Cookie", deletion.as_bytes()).unwrap();
        assert!(session.authentication().is_none());
        session.observe("Set-Cookie", b"TOKEN=new").unwrap();
        assert!(matches!(
            session.authentication(),
            Some(Authentication::Session {
                cookie: "TOKEN=new",
                csrf: None
            })
        ));
    }
}

#[test]
fn updated_csrf_headers_rotate_tokens_and_validate_values() {
    use unifi::{Authentication, session::Session};
    let mut session = Session::<64, 16>::from_cookie("TOKEN=one", Some("csrf-old")).unwrap();
    session
        .observe("X-UpDaTeD-CsRf-ToKeN", b"csrf-new")
        .unwrap();
    assert!(matches!(
        session.authentication(),
        Some(Authentication::Session {
            csrf: Some("csrf-new"),
            ..
        })
    ));
    for invalid in [b"".as_slice(), b"bad\r\nheader", b"too-long-for-the-buffer"] {
        assert!(session.observe("X-Updated-CSRF-Token", invalid).is_err());
        assert!(matches!(
            session.authentication(),
            Some(Authentication::Session {
                csrf: Some("csrf-new"),
                ..
            })
        ));
    }
}

fn websocket_mock(frames: &[u8]) -> Mock {
    let mut wire = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n".to_vec();
    wire.extend_from_slice(frames);
    Mock::new(&wire)
}

#[test]
fn websocket_close_waits_for_the_peer_and_answers_pings_once() {
    use unifi::websocket::{self, Error, Message};
    let mut mock = websocket_mock(&[0x89, 1, b'x', 0x88, 2, 3, 232]);
    let client = Client::new("h", "", "k").unwrap();
    let call = unifi::Call::raw(unifi::Method::GET, "/events").unwrap();
    let mut headers = [0; 512];
    {
        let mut socket = run(websocket::connect(
            &client,
            &mut mock,
            &call,
            *b"the sample nonce",
            &mut headers,
        ))
        .unwrap();
        run(socket.close(1000, "", [1, 2, 3, 4])).unwrap();
        assert!(matches!(
            run(socket.text("late", [0; 4])),
            Err(Error::Closed)
        ));
        let mut masks_used = 0;
        let mut masks = || {
            masks_used += 1;
            Ok([5, 6, 7, 8])
        };
        let mut output = [0; 16];
        assert!(matches!(
            run(socket.receive(&mut output, &mut masks)).unwrap(),
            Message::Close {
                code: Some(1000),
                ..
            }
        ));
        assert!(matches!(
            run(socket.receive(&mut output, &mut masks)),
            Err(Error::Closed)
        ));
        assert_eq!(masks_used, 1, "only the pong needs a new masking key");
    }
    let start = mock
        .output
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap()
        + 4;
    assert_eq!(
        &mock.output[start..],
        &[
            0x88,
            0x82,
            1,
            2,
            3,
            4,
            2,
            234,
            0x8a,
            0x81,
            5,
            6,
            7,
            8,
            b'x' ^ 5
        ]
    );
}

#[test]
fn websocket_peer_close_receives_a_single_reply() {
    use unifi::websocket::{self, Error, Message};
    let mut mock = websocket_mock(&[0x88, 2, 3, 232]);
    let client = Client::new("h", "", "k").unwrap();
    let call = unifi::Call::raw(unifi::Method::GET, "/events").unwrap();
    let mut headers = [0; 512];
    {
        let mut socket = run(websocket::connect(
            &client,
            &mut mock,
            &call,
            *b"the sample nonce",
            &mut headers,
        ))
        .unwrap();
        let mut masks_used = 0;
        let mut masks = || {
            masks_used += 1;
            Ok([1, 2, 3, 4])
        };
        let mut output = [0; 16];
        assert!(matches!(
            run(socket.receive(&mut output, &mut masks)).unwrap(),
            Message::Close {
                code: Some(1000),
                ..
            }
        ));
        assert!(matches!(
            run(socket.text("late", [0; 4])),
            Err(Error::Closed)
        ));
        assert_eq!(masks_used, 1);
    }
    let start = mock
        .output
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap()
        + 4;
    assert_eq!(&mock.output[start..], &[0x88, 0x82, 1, 2, 3, 4, 2, 234]);
}
