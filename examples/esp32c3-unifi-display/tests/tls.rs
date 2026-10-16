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

async fn exercise(rsa: bool, wrong_pin: bool) {
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
        let client = unifi::Client::new(
            "controller.test",
            "/proxy/network/integration",
            "fixture-only-api-key",
        )
        .unwrap();
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
    }
}
#[tokio::test]
async fn ecdsa_peer_and_generated_sdk_request() {
    exercise(false, false).await;
}
#[tokio::test]
async fn rsa_peer_and_generated_sdk_request() {
    exercise(true, false).await;
}
#[tokio::test]
async fn wrong_pin_never_receives_the_api_key() {
    exercise(false, true).await;
}
