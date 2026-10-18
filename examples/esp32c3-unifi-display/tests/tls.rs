//! Real TLS 1.3 handshakes carrying the SDK HTTP request, independent of hardware.
use embedded_io_adapters::tokio_1::FromTokio;
use embedded_tls::{Aes128GcmSha256, TlsConfig, TlsConnection, TlsContext};
use rand_chacha::{rand_core::SeedableRng, ChaCha20Rng};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    thread,
    time::Duration,
};

async fn exercise(rsa: bool, wrong_pin: bool, ip_endpoint: bool) {
    let host = if ip_endpoint {
        "192.168.6.1"
    } else {
        "controller.test"
    };
    if ip_endpoint {
        // A broken or unavailable resolver cannot affect this connection.
        assert!(unifi_display::local_net::resolve(host, || async {
            Err(unifi_display::local_net::ResolveError::Dns)
        })
        .await
        .is_ok());
    }
    let key = if rsa {
        use rsa::pkcs8::EncodePrivateKey;
        let key = rsa::RsaPrivateKey::new(&mut ChaCha20Rng::from_seed([7; 32]), 2048).unwrap();
        let der = key.to_pkcs8_der().unwrap();
        rcgen::KeyPair::from_pkcs8_der_and_sign_algo(
            &rustls::pki_types::PrivatePkcs8KeyDer::from(der.as_bytes()),
            &rcgen::PKCS_RSA_SHA256,
        )
        .unwrap()
    } else {
        rcgen::KeyPair::generate().unwrap()
    };
    let certificate = rcgen::CertificateParams::new(vec!["controller.test".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let mut pin: [u8; 32] = Sha256::digest(certificate.der()).into();
    if wrong_pin {
        pin[0] ^= 1;
    }
    let server = Arc::new(
        rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_no_client_auth()
            .with_single_cert(
                vec![certificate.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
            )
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut io =
            rustls::StreamOwned::new(rustls::ServerConnection::new(server).unwrap(), stream);
        if io.conn.complete_io(&mut io.sock).is_err() {
            return String::new();
        }
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            if io.read_exact(&mut byte).is_err() {
                return String::new();
            }
            request.push(byte[0]);
        }
        io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"interfaces\":{}}").unwrap();
        io.flush().unwrap();
        String::from_utf8(request).unwrap()
    });
    let stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let mut read = vec![0; 16640];
    let mut write = vec![0; 4096];
    let mut tls =
        TlsConnection::<_, Aes128GcmSha256>::new(FromTokio::new(stream), &mut read, &mut write);
    // The socket and HTTP authority use the IP. SNI selects the enrolled
    // certificate locally and never causes a lookup of this name.
    let config = TlsConfig::new().with_server_name("controller.test");
    let result = tls
        .open(TlsContext::new(
            &config,
            unifi_display::tls::Provider::new(ChaCha20Rng::from_seed([9; 32]), pin),
        ))
        .await;
    if wrong_pin {
        assert!(result.is_err());
    } else {
        result.unwrap();
        let client =
            unifi::Client::new(host, "/proxy/network/integration", "fixture-only-api-key").unwrap();
        let mut headers = [0; 4096];
        let mut output = [0; 4096];
        let call = unifi::network::get_adopted_device_latest_statistics("site", "device").unwrap();
        let reply = client
            .execute(&mut tls, &call, None, &mut headers, &mut output)
            .await
            .unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, br#"{"interfaces":{}}"#);
    }
    drop(tls);
    let request = server.join().unwrap();
    if wrong_pin {
        assert!(
            request.is_empty(),
            "API credentials reached an untrusted TLS peer"
        );
    } else {
        assert!(request.starts_with("GET /proxy/network/integration/v1/sites/site/devices/device/statistics/latest HTTP/1.1"));
        assert!(request.contains("X-API-Key: fixture-only-api-key"));
        assert!(request
            .to_ascii_lowercase()
            .contains(&format!("host: {host}")));
    }
}
#[tokio::test]
async fn ecdsa_peer_and_generated_sdk_request() {
    exercise(false, false, false).await;
}
#[tokio::test]
async fn rsa_peer_and_generated_sdk_request() {
    exercise(true, false, false).await;
}
#[tokio::test]
async fn wrong_pin_never_receives_the_api_key() {
    exercise(false, true, false).await;
    exercise(false, true, true).await;
}

#[tokio::test]
async fn pinned_ip_endpoint_uses_a_local_tls_name_without_dns() {
    exercise(false, false, true).await;
}

async fn enroll(wrong_name: bool, expired: bool, untrusted: bool) {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign];
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["console.id.ui.direct".into()]).unwrap();
    params.not_before = rcgen::date_time_ymd(2025, 1, 1);
    params.not_after = rcgen::date_time_ymd(if expired { 2025 } else { 2030 }, 12, 31);
    let leaf = params.signed_by(&key, &issuer).unwrap();
    let roots = if untrusted {
        Vec::new()
    } else {
        vec![webpki::anchor_from_trusted_cert(ca.der()).unwrap()]
    };
    let server = Arc::new(
        rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_no_client_auth()
            .with_single_cert(
                vec![leaf.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
            )
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut io =
            rustls::StreamOwned::new(rustls::ServerConnection::new(server).unwrap(), stream);
        let _ = io.conn.complete_io(&mut io.sock);
    });
    let stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let mut read = vec![0; 16640];
    let mut write = vec![0; 4096];
    let mut tls =
        TlsConnection::<_, Aes128GcmSha256>::new(FromTokio::new(stream), &mut read, &mut write);
    let hostname = if wrong_name {
        "other.id.ui.direct"
    } else {
        "console.id.ui.direct"
    };
    let config = TlsConfig::new().with_server_name(hostname);
    let observed = std::cell::Cell::new(None);
    let result = tls
        .open(TlsContext::new(
            &config,
            unifi_display::tls::BootstrapProvider::new(
                ChaCha20Rng::from_seed([9; 32]),
                hostname,
                1_790_000_000,
                &roots,
                &observed,
            ),
        ))
        .await;
    if wrong_name || expired || untrusted {
        assert!(result.is_err());
        assert!(observed.get().is_none());
    } else {
        result.unwrap();
        assert_eq!(observed.get(), Some(Sha256::digest(leaf.der()).into()));
    }
    drop(tls);
    worker.join().unwrap();
}
#[tokio::test]
async fn enrolls_the_authenticated_certificate_without_a_manual_pin() {
    enroll(false, false, false).await;
}
#[tokio::test]
async fn enrollment_rejects_a_different_cloud_identity() {
    enroll(true, false, false).await;
}
#[tokio::test]
async fn enrollment_rejects_an_expired_certificate() {
    enroll(false, true, false).await;
}
#[tokio::test]
async fn enrollment_rejects_an_unknown_ca() {
    enroll(false, false, true).await;
}

#[tokio::test]
#[ignore = "Explicit live LAN probe; no console login or key creation"]
async fn live_console_enrollment() {
    let hostname = std::env::var("UNIFI_PROBE_HOSTNAME").expect("Set the cloud-reported hostname");
    let address = std::env::var("UNIFI_PROBE_ADDRESS").expect("Set its reported LAN address");
    let target = unifi_display::provision::Target::new(&address, &hostname).unwrap();
    let stream = tokio::net::TcpStream::connect((target.address, 443))
        .await
        .unwrap();
    let mut read = vec![0; 16640];
    let mut write = vec![0; 4096];
    let mut tls =
        TlsConnection::<_, Aes128GcmSha256>::new(FromTokio::new(stream), &mut read, &mut write);
    let config = TlsConfig::new().with_server_name(&hostname);
    let observed = std::cell::Cell::new(None);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    tokio::time::timeout(
        Duration::from_secs(30),
        tls.open(TlsContext::new(
            &config,
            unifi_display::tls::BootstrapProvider::new(
                ChaCha20Rng::from_seed([8; 32]),
                &hostname,
                now,
                webpki_roots::TLS_SERVER_ROOTS,
                &observed,
            ),
        )),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(observed.get().is_some());
    println!(
        "Authenticated live LAN console certificate: {}",
        observed
            .get()
            .unwrap()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
}

#[tokio::test]
async fn console_key_creation_uses_real_tls_http_cookies_and_csrf() {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["console.id.ui.direct".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let pin: [u8; 32] = Sha256::digest(cert.der()).into();
    let server = Arc::new(
        rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
            )
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let mut reads = 0;
        for _ in 0..8 {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut io = rustls::StreamOwned::new(
                rustls::ServerConnection::new(server.clone()).unwrap(),
                stream,
            );
            let mut bytes = Vec::new();
            let mut byte = [0];
            while !bytes.ends_with(b"\r\n\r\n") {
                io.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
            }
            let header = String::from_utf8(bytes).unwrap();
            let length = header
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            io.read_exact(&mut body).unwrap();
            let (extra, data) = if header.starts_with("POST /api/auth/login ") {
                let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(body["password"], "fixture-password");
                ("Set-Cookie: TOKEN=fixture-session; Path=/; Secure; HttpOnly\r\nX-CSRF-Token: first\r\n",r#"{"id":"fixture-user"}"#)
            } else if header.starts_with("GET /api/users/self ") {
                assert!(header.contains("Cookie: TOKEN=fixture-session"));
                (
                    "X-Updated-CSRF-Token: rotated\r\n",
                    r#"{"id":"fixture-local-user"}"#,
                )
            } else if header.starts_with("POST /proxy/users/api/v2/user/fixture-local-user/keys ") {
                assert!(header.contains("Cookie: TOKEN=fixture-session"));
                assert!(header.contains("X-CSRF-Token: rotated"));
                assert!(header.contains("Origin: https://console.id.ui.direct"));
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&body).unwrap()["name"],
                    "UniFi ESP32 Display"
                );
                (
                    "",
                    r#"{"code":1,"codeS":"SUCCESS","msg":"success","data":{"full_api_key":"fixture-new-key"}}"#,
                )
            } else {
                if header.starts_with("GET /proxy/network/integration/") {
                    assert!(header.contains("X-API-Key: fixture-new-key"));
                    assert!(header.contains("Accept-Encoding: identity"));
                    reads += 1;
                    if header.contains("/statistics/latest ") {
                        ("", r#"{"interfaces":{}}"#)
                    } else if header.contains("?limit=1 ") {
                        ("", r#"{"data":[],"totalCount":0}"#)
                    } else {
                        ("", r#"{"id":"22222222-2222-4222-8222-222222222222"}"#)
                    }
                } else {
                    assert!(header.starts_with("POST /api/auth/logout "));
                    ("", "{}")
                }
            };
            let reply=format!("HTTP/1.1 200 OK\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",data.len());
            io.write_all(reply.as_bytes()).unwrap();
            io.flush().unwrap();
        }
        reads
    });
    struct Transport {
        address: std::net::SocketAddr,
    }
    impl unifi_display::provision::Http for Transport {
        async fn exchange(
            &mut self,
            request: unifi_display::provision::HttpRequest<'_>,
        ) -> unifi_display::provision::Result<unifi_display::provision::Response> {
            let stream = tokio::net::TcpStream::connect(self.address).await.unwrap();
            let mut read = vec![0; 16640];
            let mut write = vec![0; 4096];
            let mut tls = TlsConnection::<_, Aes128GcmSha256>::new(
                FromTokio::new(stream),
                &mut read,
                &mut write,
            );
            let config = TlsConfig::new().with_server_name(&request.target.hostname);
            tls.open(TlsContext::new(
                &config,
                unifi_display::tls::Provider::new(ChaCha20Rng::from_seed([3; 32]), request.pin),
            ))
            .await
            .unwrap();
            unifi_display::console_http::exchange(&mut tls, request).await
        }
    }
    let mut http = Transport { address };
    let mut session = unifi_display::provision::Session::new(
        1,
        unifi_display::provision::Target::new("192.168.6.1", "console.id.ui.direct").unwrap(),
        pin,
    );
    session
        .login(&mut http, "fixture-user", "fixture-password", "")
        .await
        .unwrap();
    let config = session
        .provision(
            &mut http,
            "11111111-1111-4111-8111-111111111111",
            "22222222-2222-4222-8222-222222222222",
            "",
        )
        .await
        .unwrap();
    assert_eq!(config["config"]["api_key"], "fixture-new-key");
    assert_eq!(
        config["config"]["controller_url"],
        "https://192.168.6.1/proxy/network/integration"
    );
    assert_eq!(config["config"]["tls_server_name"], "console.id.ui.direct");
    assert_eq!(worker.join().unwrap(), 4);
}
