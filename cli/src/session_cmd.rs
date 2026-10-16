use crate::{Options, Output, Url, config, read_file, request_observed};
use core::fmt::Write;
use heapless::String;
use unifi::{Body, Json, session::Session};

pub struct Cached {
    pub cookie: String<4096>,
    pub csrf: Option<String<512>>,
}
fn origin(url: &Url<'_>) -> Result<String<512>, &'static str> {
    let mut origin = String::new();
    write!(origin, "{}://", if url.secure { "https" } else { "http" })
        .map_err(|_| "origin too long")?;
    match url.host.parse::<core::net::IpAddr>() {
        Ok(core::net::IpAddr::V6(ip)) => write!(origin, "[{ip}]").map_err(|_| "origin too long")?,
        Ok(core::net::IpAddr::V4(ip)) => write!(origin, "{ip}").map_err(|_| "origin too long")?,
        Err(_) => {
            for c in url.host.chars() {
                origin
                    .push(c.to_ascii_lowercase())
                    .map_err(|_| "origin too long")?;
            }
        }
    }
    if url.port != if url.secure { 443 } else { 80 } {
        write!(origin, ":{}", url.port).map_err(|_| "origin too long")?;
    }
    Ok(origin)
}
pub fn load(path: &str, url: &Url<'_>) -> Result<Option<Cached>, &'static str> {
    match rustix::fs::stat(path) {
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(_) => return Err("cannot inspect session cache"),
        Ok(_) => {}
    }
    let mut buffer = [0; 8192];
    let bytes = read_file(path, &mut buffer)?;
    let json = Json::parse(bytes).map_err(|_| "invalid session cache")?;
    let expected = origin(url)?;
    let stored: String<512> = json
        .required("origin")
        .and_then(Json::string)
        .and_then(|s| s.decode())
        .map_err(|_| "invalid cached origin")?;
    if origin(&crate::parse_url(&stored)?)? != expected {
        return Err("cached session belongs to a different controller; log in with this URL first");
    }
    let cookie = json
        .required("cookie")
        .and_then(Json::string)
        .and_then(|s| s.decode())
        .map_err(|_| "invalid cached cookie")?;
    let csrf = json
        .optional("csrf")
        .map_err(|_| "invalid cached CSRF token")?
        .map(|v| v.string().and_then(|s| s.decode()))
        .transpose()
        .map_err(|_| "invalid cached CSRF token")?;
    Ok(Some(Cached { cookie, csrf }))
}
#[allow(clippy::too_many_arguments)]
pub fn login(
    url: &Url<'_>,
    call: &unifi::Call,
    options: &Options<'_>,
    path: &str,
    username: Option<&str>,
    password: Option<&str>,
    totp: Option<&str>,
    response: &mut [u8],
) -> Result<i32, &'static str> {
    if url.host.eq_ignore_ascii_case("api.ui.com") {
        return Err("session login requires a direct controller URL");
    }
    let username = username.ok_or("session login needs UNIFI_USERNAME")?;
    let password = password.ok_or("session login needs UNIFI_PASSWORD")?;
    let mut buffer = [0; 8192];
    let body = unifi::session::login_body(&mut buffer, username, password, None)
        .map_err(|_| "login body exceeds capacity")?;
    let mut jar = Session::<4096, 512>::default();
    let mut outgoing = Options {
        anonymous: true,
        ..*options
    };
    // UniFi OS login sits outside the application's proxy path.
    let base = if url.base == "/proxy/network" || url.base == "/proxy/network/integration" {
        ""
    } else {
        url.base
    };
    let first = request_observed(
        url,
        base,
        "",
        call,
        Some(Body::Json(body)),
        &outgoing,
        response,
        &mut |name, value| jar.observe(name, value),
    )?;
    validate_login(&first)?;
    let status = if first.status == 499 {
        let code =
            totp.ok_or("controller requires MFA; supply a fresh UNIFI_TOTP code and log in again")?;
        if let Some(token) = first
            .json()
            .ok()
            .and_then(|v| v.get("token").ok().flatten())
        {
            let token: String<4096> = token
                .string()
                .and_then(|s| s.decode())
                .map_err(|_| "invalid MFA challenge cookie")?;
            jar.observe("Set-Cookie", token.as_bytes())
                .map_err(|_| "invalid MFA challenge cookie")?;
        }
        let snapshot = jar.clone();
        outgoing.anonymous = false;
        if let Some(unifi::Authentication::Session { cookie, csrf }) = snapshot.authentication() {
            outgoing.cookie = Some(cookie);
            outgoing.csrf = csrf;
        } else {
            outgoing.anonymous = true;
        }
        let body = unifi::session::login_body(&mut buffer, username, password, Some(code))
            .map_err(|_| "MFA code must contain six digits")?;
        let second = request_observed(
            url,
            base,
            "",
            call,
            Some(Body::Json(body)),
            &outgoing,
            response,
            &mut |name, value| jar.observe(name, value),
        )?;
        validate_login(&second)?;
        second.status
    } else {
        first.status
    };
    if !(200..300).contains(&status) {
        let _ = writeln!(Output(true), "login failed (HTTP {status})");
        return Ok(1);
    }
    let Some(_) = jar.authentication() else {
        return Err("login response did not establish a session cookie");
    };
    store(path, url, &jar)?;
    if !options.quiet {
        writeln!(Output(false), "Authenticated; session saved.").map_err(|_| "output failed")?;
    }
    Ok(0)
}
pub fn store(path: &str, url: &Url<'_>, jar: &Session<4096, 512>) -> Result<(), &'static str> {
    let Some(unifi::Authentication::Session { cookie, csrf }) = jar.authentication() else {
        return match rustix::fs::unlink(path) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(_) => Err("cannot remove expired session cache"),
        };
    };
    let mut output = [0; 8192];
    let mut writer = unifi::json::ObjectWriter::new(&mut output).unwrap();
    writer
        .field("origin", &origin(url)?.as_str())
        .map_err(|_| "session cache too large")?;
    writer
        .field("cookie", &cookie)
        .map_err(|_| "session cache too large")?;
    if let Some(csrf) = csrf {
        writer
            .field("csrf", &csrf)
            .map_err(|_| "session cache too large")?;
    }
    config::store(
        path,
        writer.finish().map_err(|_| "session cache too large")?,
    )?;
    Ok(())
}

fn validate_login(reply: &unifi::Response<'_>) -> Result<(), &'static str> {
    if (200..300).contains(&reply.status) && !reply.body.is_empty() {
        let json = reply.json().map_err(|_| "invalid login response")?;
        if json.get("meta").ok().flatten().is_some() {
            unifi::session::data(json).map_err(|_| "login API returned an error envelope")?;
        }
    }
    Ok(())
}
