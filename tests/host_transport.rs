#![cfg(feature = "std")]

use std::{
    collections::VecDeque,
    io::{self, Cursor, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use unifi::{
    Authentication, Body,
    host::{
        self, AsyncBody, AsyncClient, AsyncTransport, BlockingClient, BlockingTransport, Error,
        Limits, Request,
    },
    session::Session,
    site_manager,
};

struct Backend {
    requests: Mutex<Vec<(Request, Duration)>>,
    payload: Vec<u8>,
    fail: bool,
    hang: bool,
    dropped: Arc<AtomicUsize>,
}
impl Backend {
    fn new(payload: &[u8], fail: bool, hang: bool) -> Self {
        Self {
            requests: Mutex::default(),
            payload: payload.to_vec(),
            fail,
            hang,
            dropped: Arc::default(),
        }
    }
    fn response(&self, request: Request, timeout: Duration) -> host::http::Response<Payload> {
        self.requests.lock().unwrap().push((request, timeout));
        let mut chunks = VecDeque::new();
        chunks.push_back(self.payload.clone());
        host::http::Response::builder()
            .status(429)
            .header("set-cookie", "TOKEN=rotated; Path=/")
            .header("set-cookie", "AUX=kept; Path=/")
            .header("x-csrf-token", "rotated-csrf")
            .header("retry-after", "7")
            .body(Payload {
                bytes: Cursor::new(self.payload.clone()),
                chunks,
                fail: self.fail,
                hang: self.hang,
                dropped: self.dropped.clone(),
            })
            .unwrap()
    }
}
struct Payload {
    bytes: Cursor<Vec<u8>>,
    chunks: VecDeque<Vec<u8>>,
    fail: bool,
    hang: bool,
    dropped: Arc<AtomicUsize>,
}
impl Drop for Payload {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}
impl Read for Payload {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.fail && self.bytes.position() == self.bytes.get_ref().len() as u64 {
            return Err(io::Error::other("fixture body failure"));
        }
        self.bytes.read(output)
    }
}
impl BlockingTransport for Backend {
    type Body = Payload;
    fn send(
        &self,
        request: Request,
        timeout: Duration,
    ) -> Result<host::http::Response<Payload>, Error> {
        Ok(self.response(request, timeout))
    }
}
impl AsyncBody for Payload {
    type Chunk = Vec<u8>;
    async fn next_chunk(&mut self) -> Result<Option<Self::Chunk>, Error> {
        if self.hang {
            core::future::pending::<()>().await;
        }
        tokio::task::yield_now().await;
        match self.chunks.pop_front() {
            Some(chunk) => Ok(Some(chunk)),
            None if self.fail => Err(Error::transport(io::Error::other("fixture body failure"))),
            None => Ok(None),
        }
    }
}
impl AsyncTransport for Backend {
    type Body = Payload;
    async fn send(
        &self,
        request: Request,
        timeout: Duration,
    ) -> Result<host::http::Response<Payload>, Error> {
        tokio::task::yield_now().await;
        Ok(self.response(request, timeout))
    }
}

#[test]
fn custom_blocking_backend_receives_validated_request_and_deadline() {
    let limits = Limits {
        timeout: Duration::from_secs(9),
        max_response_bytes: 4,
    };
    let client = BlockingClient::with_transport(
        "https://controller.test",
        "/proxy/network",
        Authentication::ApiKey("fixture-key"),
        Backend::new(b"slow", false, false),
        limits,
    )
    .unwrap();
    let response = client
        .execute(&site_manager::list_hosts().unwrap(), None)
        .unwrap();
    let error = response.success().unwrap_err();
    assert_eq!(
        (
            error.status,
            error.body.as_slice(),
            error.retry_after.as_deref()
        ),
        (429, b"slow".as_slice(), Some("7"))
    );
    let requests = client.transport().requests.lock().unwrap();
    let (request, timeout) = &requests[0];
    assert_eq!(
        request.uri(),
        "https://controller.test/proxy/network/v1/hosts"
    );
    assert_eq!(request.headers()["x-api-key"], "fixture-key");
    assert!(request.headers()["x-api-key"].is_sensitive());
    assert_eq!(timeout, &limits.timeout);
    drop(requests);
    let invalid = unifi::Call::raw(unifi::Method::POST, "/fixture").unwrap();
    assert!(
        client
            .execute_body(&invalid, Some(Body::Json(b"{bad")))
            .is_err()
    );
    assert_eq!(client.transport().requests.lock().unwrap().len(), 1);
}

fn assert_rotation(session: &Session) {
    match session.authentication().unwrap() {
        Authentication::Session { cookie, csrf } => {
            assert_eq!(cookie, "TOKEN=rotated; AUX=kept");
            assert_eq!(csrf, Some("rotated-csrf"));
        }
        _ => panic!("expected rotated session"),
    }
}
#[test]
fn custom_blocking_body_failure_and_limit_preserve_session() {
    for (payload, fail, max) in [
        (b"abcd".as_slice(), true, 10),
        (b"abcde".as_slice(), false, 4),
    ] {
        let client = BlockingClient::with_transport(
            "https://controller.test",
            "",
            Authentication::ApiKey("must-not-be-sent"),
            Backend::new(payload, fail, false),
            Limits {
                max_response_bytes: max,
                ..Limits::default()
            },
        )
        .unwrap();
        let mut session = Session::default();
        assert!(
            client
                .execute_with_session(&site_manager::list_hosts().unwrap(), None, &mut session)
                .is_err()
        );
        assert_rotation(&session);
        assert_eq!(client.transport().dropped.load(Ordering::SeqCst), 1);
        let requests = client.transport().requests.lock().unwrap();
        assert!(!requests[0].0.headers().contains_key("x-api-key"));
        assert!(!requests[0].0.headers().contains_key("cookie"));
    }
}
#[tokio::test(flavor = "current_thread")]
async fn custom_async_backend_is_send_and_preserves_session_on_body_failure() {
    let client = AsyncClient::with_transport(
        "https://controller.test",
        "",
        Authentication::None,
        Backend::new(b"abcd", true, false),
        Limits::default(),
    )
    .unwrap();
    let task = tokio::spawn(async move {
        let mut session = Session::default();
        let error = client
            .execute_with_session(&site_manager::list_hosts().unwrap(), None, &mut session)
            .await
            .unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
        assert_rotation(&session);
        assert_eq!(client.transport().dropped.load(Ordering::SeqCst), 1);
    });
    task.await.unwrap();
}
#[tokio::test(flavor = "current_thread")]
async fn custom_async_limit_and_cancellation_drop_body_without_replay() {
    for hang in [false, true] {
        let client = AsyncClient::with_transport(
            "https://controller.test",
            "",
            Authentication::None,
            Backend::new(b"abcde", false, hang),
            Limits {
                max_response_bytes: 4,
                ..Limits::default()
            },
        )
        .unwrap();
        let mut session = Session::default();
        let call = site_manager::list_hosts().unwrap();
        let result = tokio::time::timeout(
            Duration::from_millis(20),
            client.execute_with_session(&call, None, &mut session),
        )
        .await;
        if hang {
            assert!(result.is_err());
        } else {
            assert!(matches!(result, Ok(Err(Error::ResponseTooLarge))));
        }
        assert_rotation(&session);
        assert_eq!(client.transport().dropped.load(Ordering::SeqCst), 1);
        assert_eq!(client.transport().requests.lock().unwrap().len(), 1);
    }
}
