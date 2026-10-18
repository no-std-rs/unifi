//! Bounded HTTP exchange shared by the board and host integration tests.
use crate::provision::{HttpRequest, Method as ConsoleMethod, Response, Result};
use alloc::{string::ToString, vec, vec::Vec};
use embedded_io_async::{Read, Write};
use reqwless::{
    client::HttpConnection,
    request::{Method, Request, RequestBuilder},
};

pub async fn exchange<S: Read + Write>(
    stream: &mut S,
    request: HttpRequest<'_>,
) -> Result<Response> {
    let method = if request.method == ConsoleMethod::Get {
        Method::GET
    } else {
        Method::POST
    };
    let mut connection = HttpConnection::Plain(stream);
    let request_body = request.body.unwrap_or(&[]);
    let outgoing = Request::new(method, request.path)
        .host(&request.target.hostname)
        .headers(request.headers)
        .body(request_body)
        .build();
    let mut buffer = vec![0; 8192];
    let response = connection
        .send(outgoing, &mut buffer)
        .await
        .map_err(|_| "Console HTTP exchange failed")?;
    let status: u16 = response.status.0;
    let mut cookies = Vec::new();
    let mut csrf = None;
    let mut encoded = false;
    for (name, value) in response.headers() {
        if name.eq_ignore_ascii_case("set-cookie") {
            cookies.push(
                core::str::from_utf8(value)
                    .map_err(|_| "Invalid console cookie")?
                    .to_string(),
            );
        } else if name.eq_ignore_ascii_case("x-csrf-token")
            || name.eq_ignore_ascii_case("x-updated-csrf-token")
        {
            csrf = Some(
                core::str::from_utf8(value)
                    .map_err(|_| "Invalid console CSRF token")?
                    .to_string(),
            );
        } else if name.eq_ignore_ascii_case("content-encoding")
            && !value.eq_ignore_ascii_case(b"identity")
        {
            encoded = true;
        }
    }
    // Login success/session responses can be large; only their status
    // and headers are needed. The connection is closed rather than buffering them.
    // Error bodies may be HTML/plain text from the console's reverse proxy.
    // Preserve their status for authentication/rate-limit handling. MFA's 499
    // challenge is the one error response whose JSON is part of the protocol.
    let login = request.path == "/api/auth/login";
    let parse =
        request.json && ((!login && (200..300).contains(&status)) || (login && status == 499));
    let data = if parse {
        if encoded {
            return Err("Console returned unsupported content encoding");
        }
        let mut body = vec![0; 16385];
        let mut reader = response.body().reader();
        let mut used = 0;
        loop {
            if used == body.len() {
                return Err("Console response exceeds the display limit");
            }
            let count = reader
                .read(&mut body[used..])
                .await
                .map_err(|_| "Cannot read console response")?;
            if count == 0 {
                break;
            }
            used += count;
        }
        if used == 0 {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&body[..used]).map_err(|_| "Console returned invalid JSON")?
        }
    } else {
        serde_json::Value::Null
    };
    Ok(Response {
        status,
        cookies,
        csrf,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_io_adapters::tokio_1::FromTokio;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn reply(status: u16, path: &str, body: &str) -> Result<Response> {
        encoded_reply(status, path, body, "").await
    }

    async fn encoded_reply(
        status: u16,
        path: &str,
        body: &str,
        encoding: &str,
    ) -> Result<Response> {
        let (client, mut server) = tokio::io::duplex(32768);
        let wire = alloc::format!(
            "HTTP/1.1 {status} Response\r\n{encoding}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let serving = tokio::spawn(async move {
            let mut byte = [0];
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                server.read_exact(&mut byte).await.unwrap();
                headers.push(byte[0]);
            }
            server.write_all(wire.as_bytes()).await.unwrap();
        });
        let target = crate::provision::Target::new("192.168.6.1", "console.id.ui.direct").unwrap();
        let result = exchange(
            &mut FromTokio::new(client),
            HttpRequest {
                target: &target,
                pin: [0xab; 32],
                method: ConsoleMethod::Post,
                path,
                headers: &[],
                body: None,
                json: true,
            },
        )
        .await;
        serving.await.unwrap();
        result
    }

    #[tokio::test]
    async fn parsed_responses_require_identity_encoding_and_bounded_json() {
        let path = "/proxy/network/integration/v1/sites/site/devices/device";
        assert_eq!(
            encoded_reply(200, path, "{}", "Content-Encoding: gzip\r\n")
                .await
                .err(),
            Some("Console returned unsupported content encoding")
        );
        assert!(
            encoded_reply(200, path, "{}", "Content-Encoding: IDENTITY\r\n")
                .await
                .is_ok()
        );
        assert_eq!(
            encoded_reply(403, path, "error", "Content-Encoding: gzip\r\n")
                .await
                .unwrap()
                .status,
            403
        );
        let oversized = " ".repeat(16385);
        assert_eq!(
            reply(200, path, &oversized).await.err(),
            Some("Console response exceeds the display limit")
        );
    }

    #[tokio::test]
    async fn http_errors_preserve_status_even_with_non_json_bodies() {
        for status in [401, 403, 429, 502] {
            let response = reply(
                status,
                "/proxy/users/api/v2/user/self/keys",
                "<html>upstream error</html>",
            )
            .await
            .unwrap();
            assert_eq!(response.status, status);
            assert!(response.data.is_null());
        }
        for status in [200, 499] {
            let path = if status == 499 {
                "/api/auth/login"
            } else {
                "/proxy/users/api/v2/user/self/keys"
            };
            assert!(reply(status, path, "invalid JSON").await.is_err());
        }
        assert_eq!(
            reply(499, "/api/auth/login", r#"{"token":"challenge"}"#)
                .await
                .unwrap()
                .data["token"],
            "challenge"
        );
    }
}
