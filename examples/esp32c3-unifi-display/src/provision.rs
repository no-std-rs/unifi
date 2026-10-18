//! Ephemeral console sign-in and local integration-key enrollment over pinned LAN TLS.
//! No console password/session cookie is written to flash or returned to the website.
use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec::Vec,
};
use serde_json::{json, Value};

pub type Result<T> = core::result::Result<T, &'static str>;
#[derive(Clone)]
pub struct Target {
    pub address: core::net::Ipv4Addr,
    pub hostname: String,
}
impl Target {
    pub fn new(address: &str, hostname: &str) -> Result<Self> {
        let address = address.parse().map_err(|_| "Invalid console LAN address")?;
        let labels = hostname.split('.');
        if !crate::config::local_ipv4(address)
            || hostname.len() > 200
            || !hostname.ends_with(".ui.direct")
            || !labels.clone().all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err("Choose a console with a trusted direct-connect name and LAN address");
        }
        Ok(Self {
            address,
            hostname: hostname.to_string(),
        })
    }
}
pub struct Response {
    pub status: u16,
    pub cookies: Vec<String>,
    pub csrf: Option<String>,
    pub data: Value,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}
pub struct HttpRequest<'a> {
    pub target: &'a Target,
    pub pin: [u8; 32],
    pub method: Method,
    pub path: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<&'a [u8]>,
    pub json: bool,
}
#[allow(async_fn_in_trait)]
pub trait Http {
    async fn exchange(&mut self, request: HttpRequest<'_>) -> Result<Response>;
}

pub struct Session {
    pub owner: u32,
    pub target: Target,
    pub pin: [u8; 32],
    cookies: BTreeMap<String, String>,
    csrf: String,
    challenge: Option<Value>,
    legacy_mfa: bool,
    authenticated: bool,
    creation_attempted: bool,
    key: Option<String>,
    created: bool,
}
fn api_key(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 512 || !value.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        Err("Invalid local API key")
    } else {
        Ok(())
    }
}
fn successful(status: u16) -> Result<()> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err("Console access denied; check the account or local API key"),
        429 => Err("Console rate limit reached; wait before trying again"),
        _ => Err("Console request failed; check its Network application and retry"),
    }
}
impl Session {
    pub fn new(owner: u32, target: Target, pin: [u8; 32]) -> Self {
        Self {
            owner,
            target,
            pin,
            cookies: BTreeMap::new(),
            csrf: String::new(),
            challenge: None,
            legacy_mfa: false,
            authenticated: false,
            creation_attempted: false,
            key: None,
            created: false,
        }
    }
    pub fn fingerprint(&self) -> String {
        self.pin.iter().map(|b| format!("{b:02x}")).collect()
    }
    fn cookie(&mut self, header: &str) -> Result<()> {
        let pair = header
            .split(';')
            .next()
            .ok_or("Invalid console session cookie")?;
        let (name, value) = pair
            .split_once('=')
            .ok_or("Invalid console session cookie")?;
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || value.len() > 4096
            || !value.bytes().all(|b| (0x21..=0x7e).contains(&b))
        {
            return Err("Invalid console session cookie");
        }
        if value.is_empty() {
            self.cookies.remove(name);
        } else {
            self.cookies.insert(name.to_string(), value.to_string());
        }
        if self.cookies.len() > 8
            || self
                .cookies
                .iter()
                .map(|(k, v)| k.len() + v.len() + 2)
                .sum::<usize>()
                > 6144
        {
            self.cookies.clear();
            return Err("Console session cookies exceed the display limit");
        }
        Ok(())
    }
    async fn request(
        &mut self,
        http: &mut impl Http,
        method: Method,
        path: &str,
        body: Option<Value>,
        use_key: bool,
        parse_json: bool,
    ) -> Result<Response> {
        let cookie = self
            .cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        let origin = format!("https://{}", self.target.hostname);
        let body = body
            .map(|value| serde_json::to_vec(&value))
            .transpose()
            .map_err(|_| "Cannot encode console request")?;
        let mut headers = alloc::vec![
            ("Accept", "application/json"),
            ("Accept-Encoding", "identity"),
            ("Content-Type", "application/json"),
            ("Connection", "close")
        ];
        if use_key {
            headers.push((
                "X-API-Key",
                self.key.as_deref().ok_or("No local key available")?,
            ));
        } else {
            headers.push(("Origin", &origin));
            if !cookie.is_empty() {
                headers.push(("Cookie", &cookie));
            }
            if !self.csrf.is_empty() {
                headers.push(("X-CSRF-Token", &self.csrf));
            }
        }
        let response = http
            .exchange(HttpRequest {
                target: &self.target,
                pin: self.pin,
                method,
                path,
                headers: &headers,
                body: body.as_deref(),
                json: parse_json,
            })
            .await?;
        for cookie in &response.cookies {
            self.cookie(cookie)?;
        }
        if let Some(csrf) = &response.csrf {
            if csrf.len() > 2048 || !csrf.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
                return Err("Invalid console CSRF token");
            }
            self.csrf = csrf.clone();
        }
        Ok(response)
    }
    pub async fn login(
        &mut self,
        http: &mut impl Http,
        username: &str,
        password: &str,
        token: &str,
    ) -> Result<Value> {
        if username.is_empty()
            || username.len() > 256
            || password.is_empty()
            || password.len() > 512
            || token.len() > 256
        {
            return Err("Enter your console username and password");
        }
        self.authenticated = false;
        if !self.created {
            self.key = None;
        }
        let mut body = json!({"username":username,"password":password,"rememberMe":false});
        body[if self.legacy_mfa {
            "ubic_2fa_token"
        } else {
            "token"
        }] = token.into();
        let response = self
            .request(
                http,
                Method::Post,
                "/api/auth/login",
                Some(body),
                false,
                true,
            )
            .await?;
        if response.status == 499 {
            if let Some(cookie) = response.data.get("token").and_then(Value::as_str) {
                self.cookie(cookie)?;
                self.legacy_mfa = true;
                self.challenge = Some(json!({"authenticators":[{"id":"totp","type":"totp"}]}));
            } else {
                self.challenge = response.data.get("data").cloned();
                self.legacy_mfa = false;
            }
            let methods = self
                .challenge
                .as_ref()
                .and_then(|c| c.get("authenticators"))
                .and_then(Value::as_array)
                .ok_or("Console did not provide two-factor methods")?;
            let methods = methods
                .iter()
                .filter_map(|m| {
                    Some(json!({"id":m.get("id")?.as_str()?,"type":m.get("type")?.as_str()?}))
                })
                .collect::<Vec<_>>();
            return Ok(json!({"state":"mfa","methods":methods}));
        }
        successful(response.status)?;
        self.authenticated = true;
        self.challenge = None;
        Ok(json!({"state":"authenticated"}))
    }
    pub async fn mfa(&mut self, http: &mut impl Http, id: &str) -> Result<Value> {
        let challenge = self
            .challenge
            .as_ref()
            .ok_or("Sign in before requesting approval")?;
        let method = challenge
            .get("authenticators")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .find(|m| m.get("id").and_then(Value::as_str) == Some(id))
            })
            .ok_or("Choose an available two-factor method")?;
        let kind = method
            .get("type")
            .and_then(Value::as_str)
            .ok_or("Invalid two-factor method")?;
        if !["push", "email", "sms"].contains(&kind) {
            return Err("Enter the code from your authenticator");
        }
        let body = json!({"id":id,"method":kind,"mfaCookie":challenge.get("mfaCookie").ok_or("Missing two-factor session")?});
        let response = self
            .request(
                http,
                Method::Post,
                "/api/sso/login/mfa/send",
                Some(body),
                false,
                true,
            )
            .await?;
        successful(response.status)?;
        if let Some(cookie) = response.data.get("mfaCookie") {
            self.challenge.as_mut().unwrap()["mfaCookie"] = cookie.clone();
        }
        Ok(json!({"state":"sent"}))
    }
    pub async fn poll(&mut self, http: &mut impl Http) -> Result<Value> {
        let cookie = self
            .challenge
            .as_ref()
            .and_then(|v| v.get("mfaCookie"))
            .ok_or("Sign in before checking approval")?;
        let response = self
            .request(
                http,
                Method::Post,
                "/api/auth/login/poll",
                Some(json!({"mfaCookie":cookie,"rememberMe":false})),
                false,
                false,
            )
            .await?;
        if response.status == 202 {
            return Ok(json!({"state":"waiting"}));
        }
        successful(response.status)?;
        self.authenticated = true;
        self.challenge = None;
        Ok(json!({"state":"authenticated"}))
    }
    pub async fn provision(
        &mut self,
        http: &mut impl Http,
        site: &str,
        device: &str,
        existing_key: &str,
    ) -> Result<Value> {
        let uuid = |s: &str| {
            s.len() == 36
                && s.bytes().enumerate().all(|(i, b)| {
                    if [8, 13, 18, 23].contains(&i) {
                        b == b'-'
                    } else {
                        b.is_ascii_hexdigit()
                    }
                })
        };
        if !uuid(site) || !uuid(device) {
            return Err("Choose a site and device from cloud discovery");
        }
        if !existing_key.is_empty() {
            api_key(existing_key)?;
            self.key = Some(existing_key.to_string());
            self.created = false;
        }
        if self.key.is_none() {
            if !self.authenticated {
                return Err("Sign in to create the local display key");
            }
            if self.creation_attempted {
                return Err("Key creation was already attempted; check Console Integrations before starting again");
            }
            let response = self
                .request(http, Method::Get, "/api/users/self", None, false, false)
                .await?;
            successful(response.status)?;
            self.creation_attempted = true;
            let response = self
                .request(
                    http,
                    Method::Post,
                    "/proxy/users/api/v2/user/self/keys",
                    Some(json!({"name":"UniFi ESP32 Display"})),
                    false,
                    true,
                )
                .await?;
            successful(response.status)?;
            if response
                .data
                .get("code")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                != 0
            {
                return Err("Console rejected local key creation");
            }
            let key = response
                .data
                .pointer("/data/full_api_key")
                .and_then(Value::as_str)
                .ok_or("Console did not return the new key; check Integrations before retrying")?;
            api_key(key)?;
            self.key = Some(key.to_string());
            self.created = true;
        }
        for (index, suffix) in [
            format!("/devices/{device}"),
            format!("/devices/{device}/statistics/latest"),
            "/devices?limit=1".into(),
            "/clients?limit=1".into(),
        ]
        .into_iter()
        .enumerate()
        {
            let path = format!("/proxy/network/integration/v1/sites/{site}{suffix}");
            let response = self
                .request(http, Method::Get, &path, None, true, true)
                .await?;
            successful(response.status)?;
            let data = &response.data;
            let usable = match index {
                0 => data
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id.eq_ignore_ascii_case(device)),
                1 => data
                    .get("interfaces")
                    .and_then(Value::as_object)
                    .is_some_and(|interfaces| {
                        interfaces
                            .get("radios")
                            .is_none_or(|radios| radios.is_null() || radios.is_array())
                    }),
                _ => {
                    data.get("data").is_some_and(Value::is_array)
                        && data.get("totalCount").and_then(Value::as_u64).is_some()
                }
            };
            if !usable {
                return Err(match index {
                    0 => "Local device details do not match the selected device; check Network and retry",
                    1 => "Local device statistics are incompatible; check Network and retry",
                    2 => "Local device count is incompatible; check Network and retry",
                    _ => "Local client count is incompatible; check Network and retry",
                });
            }
        }
        let result = json!({"state":"verified","config":{"controller_url":format!("https://{}/proxy/network/integration",self.target.hostname),
            "api_key":self.key,"certificate_sha256":self.fingerprint(),"site_id":site,"device_id":device}});
        if self.authenticated {
            let _ = self
                .request(
                    http,
                    Method::Post,
                    "/api/auth/logout",
                    Some(json!({})),
                    false,
                    false,
                )
                .await;
        }
        self.authenticated = false;
        self.cookies.clear();
        self.csrf.clear();
        self.challenge = None;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Console {
        created: usize,
        fail_verify: bool,
        invalid_payload: Option<(&'static str, Value)>,
        legacy: bool,
        log: Vec<(String, Value)>,
    }
    impl Http for Console {
        async fn exchange(&mut self, request: HttpRequest<'_>) -> Result<Response> {
            assert_eq!(
                request.target.address,
                core::net::Ipv4Addr::new(192, 168, 6, 1)
            );
            assert_eq!(request.pin, [0xab; 32]);
            let body = request
                .body
                .map(|b| serde_json::from_slice::<Value>(b).unwrap())
                .unwrap_or(Value::Null);
            self.log.push((request.path.into(), body.clone()));
            let mut response = Response {
                status: 200,
                cookies: Vec::new(),
                csrf: None,
                data: Value::Null,
            };
            match request.path {
                "/api/auth/login" => {
                    if self.legacy
                        && body.get("ubic_2fa_token").and_then(Value::as_str) != Some("123456")
                    {
                        response.status = 499;
                        response.data = json!({"token":"UBIC_2FA=challenge; Path=/; Secure"});
                    } else {
                        if self.legacy {
                            assert!(request
                                .headers
                                .iter()
                                .any(|(k, v)| *k == "Cookie" && v.contains("UBIC_2FA=challenge")));
                        }
                        response
                            .cookies
                            .push("TOKEN=session-only; Path=/; Secure; HttpOnly".into());
                        response.csrf = Some("first-csrf".into());
                    }
                }
                "/api/users/self" => {
                    assert!(
                        request.headers.contains(&("Cookie", "TOKEN=session-only")) || self.legacy
                    );
                    response.csrf = Some("updated-csrf".into());
                }
                "/proxy/users/api/v2/user/self/keys" => {
                    assert!(request.headers.contains(&("X-CSRF-Token", "updated-csrf")));
                    assert_eq!(body, json!({"name":"UniFi ESP32 Display"}));
                    self.created += 1;
                    response.data = json!({"code":0,"data":{"full_api_key":"fixture-local-key"}});
                }
                path if path.starts_with("/proxy/network/integration/") => {
                    assert!(request
                        .headers
                        .contains(&("X-API-Key", "fixture-local-key")));
                    assert!(!request.headers.iter().any(|(name, _)| *name == "Cookie"));
                    if self.fail_verify {
                        response.status = 403;
                        self.fail_verify = false;
                    }
                    response.data = if path.ends_with("statistics/latest") {
                        json!({"interfaces":{}})
                    } else if path.contains('?') {
                        json!({"data":[],"totalCount":0})
                    } else {
                        json!({"id":DEVICE})
                    };
                    if self
                        .invalid_payload
                        .as_ref()
                        .is_some_and(|(suffix, _)| path.ends_with(suffix))
                    {
                        response.data = self.invalid_payload.take().unwrap().1;
                    }
                }
                "/api/auth/logout" => {}
                _ => panic!("Unexpected endpoint"),
            }
            Ok(response)
        }
    }
    const SITE: &str = "11111111-1111-4111-8111-111111111111";
    const DEVICE: &str = "22222222-2222-4222-8222-222222222222";
    fn session() -> Session {
        Session::new(
            9,
            Target::new("192.168.6.1", "console.id.ui.direct").unwrap(),
            [0xab; 32],
        )
    }
    fn console() -> Console {
        Console {
            created: 0,
            fail_verify: false,
            invalid_payload: None,
            legacy: false,
            log: Vec::new(),
        }
    }
    #[tokio::test]
    async fn creates_and_verifies_local_access_then_discards_console_login() {
        let mut session = session();
        let mut console = console();
        assert_eq!(
            session
                .login(&mut console, "user", "password", "")
                .await
                .unwrap()["state"],
            "authenticated"
        );
        let result = session
            .provision(&mut console, SITE, DEVICE, "")
            .await
            .unwrap();
        assert_eq!(result["config"]["api_key"], "fixture-local-key");
        assert_eq!(result["config"]["certificate_sha256"], "ab".repeat(32));
        assert_eq!(console.created, 1);
        assert!(session.cookies.is_empty());
        assert!(session.csrf.is_empty());
        assert!(!session.authenticated);
        assert_eq!(
            console
                .log
                .iter()
                .filter(|(p, _)| p.starts_with("/proxy/network/"))
                .count(),
            4
        );
    }
    #[tokio::test]
    async fn verification_retry_reuses_the_created_key_instead_of_minting_another() {
        let mut session = session();
        let mut console = console();
        console.fail_verify = true;
        session
            .login(&mut console, "user", "password", "")
            .await
            .unwrap();
        assert!(session
            .provision(&mut console, SITE, DEVICE, "")
            .await
            .is_err());
        session
            .provision(&mut console, SITE, DEVICE, "")
            .await
            .unwrap();
        assert_eq!(console.created, 1);
    }
    #[tokio::test]
    async fn unusable_dashboard_data_is_not_verified_and_retry_keeps_the_new_key() {
        for (suffix, payload) in [
            (DEVICE, json!({"id":SITE})),
            ("statistics/latest", json!({"interfaces":null})),
            ("statistics/latest", json!({"interfaces":{"radios":{}}})),
            ("/devices?limit=1", json!({"data":{},"totalCount":1})),
            ("/clients?limit=1", json!({"data":[],"totalCount":-1})),
        ] {
            let mut session = session();
            let mut console = console();
            console.invalid_payload = Some((suffix, payload));
            session
                .login(&mut console, "user", "password", "")
                .await
                .unwrap();
            assert!(
                session
                    .provision(&mut console, SITE, DEVICE, "")
                    .await
                    .is_err(),
                "accepted an unusable response for {suffix}"
            );
            session
                .provision(&mut console, SITE, DEVICE, "")
                .await
                .unwrap();
            assert_eq!(console.created, 1);
        }
    }
    #[tokio::test]
    async fn legacy_two_factor_preserves_the_challenge_cookie() {
        let mut session = session();
        let mut console = console();
        console.legacy = true;
        assert_eq!(
            session
                .login(&mut console, "user", "password", "")
                .await
                .unwrap()["state"],
            "mfa"
        );
        assert_eq!(
            session
                .login(&mut console, "user", "password", "123456")
                .await
                .unwrap()["state"],
            "authenticated"
        );
        session
            .provision(&mut console, SITE, DEVICE, "")
            .await
            .unwrap();
        assert_eq!(console.created, 1);
    }
    #[tokio::test]
    async fn push_approval_retains_the_challenge_across_poll_failures() {
        struct PushConsole {
            console: Console,
            polls: usize,
        }
        impl Http for PushConsole {
            async fn exchange(&mut self, request: HttpRequest<'_>) -> Result<Response> {
                let body = request
                    .body
                    .map(|bytes| serde_json::from_slice::<Value>(bytes).unwrap())
                    .unwrap_or(Value::Null);
                let mut response = Response {
                    status: 200,
                    cookies: Vec::new(),
                    csrf: None,
                    data: Value::Null,
                };
                match request.path {
                    "/api/auth/login" => {
                        response.status = 499;
                        response.data = json!({"data":{"mfaCookie":"initial-challenge","authenticators":[{"id":"verify","type":"push"}]}});
                    }
                    "/api/sso/login/mfa/send" => {
                        assert_eq!(
                            body,
                            json!({"id":"verify","method":"push","mfaCookie":"initial-challenge"})
                        );
                        response.data = json!({"mfaCookie":"rotated-challenge"});
                    }
                    "/api/auth/login/poll" => {
                        assert_eq!(
                            body,
                            json!({"mfaCookie":"rotated-challenge","rememberMe":false})
                        );
                        self.polls += 1;
                        response.status = match self.polls {
                            1 => 429,
                            2 => 202,
                            _ => 200,
                        };
                        if response.status == 200 {
                            response.cookies.push("TOKEN=session-only; Secure".into());
                            response.csrf = Some("first-csrf".into());
                        }
                    }
                    _ => return self.console.exchange(request).await,
                }
                Ok(response)
            }
        }
        let mut session = session();
        let mut console = PushConsole {
            console: console(),
            polls: 0,
        };
        assert_eq!(
            session
                .login(&mut console, "user", "password", "")
                .await
                .unwrap()["state"],
            "mfa"
        );
        assert_eq!(
            session.mfa(&mut console, "verify").await.unwrap()["state"],
            "sent"
        );
        assert_eq!(
            session.poll(&mut console).await.unwrap_err(),
            "Console rate limit reached; wait before trying again"
        );
        assert_eq!(
            session.poll(&mut console).await.unwrap()["state"],
            "waiting"
        );
        assert_eq!(
            session.poll(&mut console).await.unwrap()["state"],
            "authenticated"
        );
        session
            .provision(&mut console, SITE, DEVICE, "")
            .await
            .unwrap();
        assert_eq!(console.console.created, 1);
        assert!(session.challenge.is_none());
        assert!(session.cookies.is_empty());
    }
    #[test]
    fn only_cloud_direct_connect_names_and_lan_addresses_are_accepted() {
        for (ip, host) in [
            ("8.8.8.8", "console.id.ui.direct"),
            ("192.168.6.1", "ui.direct.evil.test"),
            ("192.168.6.1", "-bad.ui.direct"),
            ("192.168.6.1", "console.ui.direct\r\nInjected: yes"),
        ] {
            assert!(Target::new(ip, host).is_err());
        }
    }
}
