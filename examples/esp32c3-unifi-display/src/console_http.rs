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
        }
    }
    // Login success/session/verification responses can be large; only their status
    // and headers are needed. The connection is closed rather than buffering them.
    let parse = request.json && !(request.path == "/api/auth/login" && status != 499);
    let data = if parse {
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
