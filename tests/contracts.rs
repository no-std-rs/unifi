use core::{
    future::Future,
    task::{Context, Poll, Waker},
};
use unifi::{Client, Json, JsonError, network, site_manager};

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

fn upgrade(frames: &[u8]) -> Mock {
    let mut bytes = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n".to_vec();
    bytes.extend_from_slice(frames);
    Mock::new(&bytes)
}
#[test]
fn websocket_upgrade_fragmentation_ping_and_masking() {
    use unifi::websocket::{self, Message};
    let client = Client::new("test.invalid", "/proxy/protect/integration", "test").unwrap();
    let call = unifi::protect::get_subscribe_events().unwrap();
    // A multibyte UTF-8 scalar straddles data frames, with a ping in the middle.
    let mut mock = upgrade(&[
        0x01, 2, b'a', 0xc3, 0x89, 1, b'?', 0x80, 2, 0xa9, b'b', 0x88, 2, 3, 0xe8,
    ]);
    let mut headers = [0; 512];
    let mut ws = run(websocket::connect(
        &client,
        &mut mock,
        &call,
        *b"the sample nonce",
        &mut headers,
    ))
    .unwrap();
    let mut output = [0; 128];
    match run(ws.receive(&mut output, &mut || Ok([1, 2, 3, 4]))).unwrap() {
        Message::Text(text) => assert_eq!(text, "aéb"),
        other => panic!("{other:?}"),
    }
    run(ws.text("hello", [5, 6, 7, 8])).unwrap();
    assert!(matches!(
        run(ws.receive(&mut output, &mut || Ok([0; 4]))).unwrap(),
        Message::Close {
            code: Some(1000),
            ..
        }
    ));
    assert!(run(ws.text("closed", [0; 4])).is_err());
    let end = mock
        .output
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap()
        + 4;
    let request = core::str::from_utf8(&mock.output[..end]).unwrap();
    assert!(request.starts_with("GET /proxy/protect/integration/v1/subscribe/events HTTP/1.1"));
    assert!(request.contains("Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ=="));
    assert_eq!(
        &mock.output[end..end + 7],
        &[0x8a, 0x81, 1, 2, 3, 4, b'?' ^ 1]
    );
    assert_eq!(
        &mock.output[end + 7..end + 18],
        &[
            0x81,
            0x85,
            5,
            6,
            7,
            8,
            b'h' ^ 5,
            b'e' ^ 6,
            b'l' ^ 7,
            b'l' ^ 8,
            b'o' ^ 5
        ]
    );
}
#[test]
fn websocket_rejects_bad_upgrades_frames_and_overflow() {
    use unifi::websocket::{self, Error};
    let client = Client::new("test.invalid", "", "test").unwrap();
    let call = unifi::protect::get_subscribe_devices().unwrap();
    let mut bad = Mock::new(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: wrong\r\n\r\n");
    assert!(
        run(websocket::connect(
            &client,
            &mut bad,
            &call,
            *b"the sample nonce",
            &mut [0; 512]
        ))
        .is_err()
    );
    for frames in [
        &[0x81, 0x80][..],                      // masked server frame
        &[0xc1, 0][..],                         // unnegotiated compression
        &[0x80, 0][..],                         // continuation without a start
        &[0x09, 0][..],                         // fragmented control frame
        &[0x81, 126, 0, 1, b'a'][..],           // non-minimal length
        &[0x81, 1, 0xff][..],                   // invalid UTF-8
        &[0x88, 1, 0][..],                      // incomplete close code
        &[0x88, 2, 3, 0xed][..],                // forbidden close code 1005
        &[0x81, 4, b'a', b'b', b'c', b'd'][..], // caller capacity
        &[0x01, 0, 0x81, 0][..],                // new message while fragmented
    ] {
        let mut mock = upgrade(frames);
        let mut headers = [0; 512];
        let mut ws = run(websocket::connect(
            &client,
            &mut mock,
            &call,
            *b"the sample nonce",
            &mut headers,
        ))
        .unwrap();
        assert!(
            run(ws.receive(&mut [0; 3], &mut || Ok([0; 4]))).is_err(),
            "{frames:?}"
        );
        assert!(matches!(
            run(ws.receive(&mut [0; 3], &mut || Ok([0; 4]))),
            Err(Error::Closed)
        ));
    }
}
#[test]
fn multipart_and_session_headers_use_bounded_buffers() {
    use unifi::{Authentication, Body, Call, Method, multipart::Multipart, session::Session};
    let mut session = Session::<128, 32>::default();
    assert!(session.authentication().is_none());
    session
        .observe("Set-Cookie", b"TOKEN=first; Path=/; Secure; HttpOnly")
        .unwrap();
    session
        .observe("Set-Cookie", b"other=keep; Path=/")
        .unwrap();
    session
        .observe("Set-Cookie", b"TOKEN=replaced; Path=/")
        .unwrap();
    session.observe("X-CSRF-Token", b"csrf-value").unwrap();
    assert!(!format!("{session:?}").contains("replaced"));
    let client =
        Client::with_authentication("test.invalid", "", session.authentication().unwrap()).unwrap();
    let call = Call::raw(Method::POST, "/v1/files/animations")
        .unwrap()
        .query("custom", "a&b")
        .unwrap();
    let mut bytes = [0; 512];
    let multipart = Multipart::file(
        &mut bytes,
        "random-boundary",
        "file",
        "a\"b.png",
        "image/png",
        b"\x89PNG\0\xff",
    )
    .unwrap();
    let mut mock = Mock::new(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nSet-Cookie: TOKEN=renewed; Secure\r\n\r\n{}",
    );
    let mut response_body = [0; 32];
    let response = run(client.execute_with(
        &mut mock,
        &call,
        Some(multipart.body()),
        &mut [0; 512],
        &mut response_body,
        |name, value| {
            // A separate jar is used here because the client still borrows the outgoing credentials.
            assert!(!name.eq_ignore_ascii_case("set-cookie") || value.starts_with(b"TOKEN="));
            Ok(())
        },
    ))
    .unwrap();
    assert_eq!(response.status, 200);
    let wire = String::from_utf8_lossy(&mock.output);
    assert!(wire.contains("Content-Type: multipart/form-data; boundary=random-boundary\r\n"));
    assert!(wire.contains("Cookie: other=keep; TOKEN=replaced\r\n"));
    assert!(wire.contains("X-CSRF-Token: csrf-value\r\n"));
    assert!(!wire.contains("X-API-Key"));
    assert!(wire.contains("filename=\"a\\\"b.png\""));
    assert!(
        mock.output
            .ends_with(b"\x89PNG\0\xff\r\n--random-boundary--\r\n")
    );
    assert!(Multipart::file(&mut [0; 8], "x", "file", "a", "image/png", b"body").is_err());
    assert!(
        Multipart::file(
            &mut [0; 512],
            "collision",
            "file",
            "a",
            "image/png",
            b"collision"
        )
        .is_err()
    );
    assert!(Multipart::file(&mut [0; 512], "x", "file", "a\r\nB", "image/png", b"body").is_err());
    session.observe("Set-Cookie", b"TOKEN=; Max-Age=0").unwrap();
    let Some(Authentication::Session { cookie, .. }) = session.authentication() else {
        panic!()
    };
    assert_eq!(cookie, "other=keep");
    let client = Client::with_authentication("test.invalid", "", Authentication::None).unwrap();
    let mut mock = Mock::new(b"HTTP/1.1 204 No Content\r\n\r\n");
    run(client.execute_with(
        &mut mock,
        &call,
        Some(Body::Bytes {
            bytes: b"not JSON",
            content_type: "image/png",
        }),
        &mut [0; 256],
        &mut [0; 32],
        |_, _| Ok(()),
    ))
    .unwrap();
    assert!(
        core::str::from_utf8(&mock.output)
            .unwrap()
            .contains("Content-Type: image/png\r\n")
    );
    assert!(
        Client::with_authentication(
            "test.invalid",
            "",
            Authentication::Session {
                cookie: "bad\r\nheader",
                csrf: None
            }
        )
        .is_err()
    );
}

#[test]
fn every_pinned_operation_has_a_working_wire_binding() {
    for (spec, operations) in [
        (include_str!("../schemas/network.json"), network::OPERATIONS),
        (
            include_str!("../schemas/site-manager.json"),
            site_manager::OPERATIONS,
        ),
        (
            include_str!("../schemas/protect.json"),
            unifi::protect::OPERATIONS,
        ),
        (
            include_str!("../schemas/session-api.json"),
            unifi::session_api::OPERATIONS,
        ),
    ] {
        let spec: serde_json::Value = serde_json::from_str(spec).unwrap();
        let count = spec["paths"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| {
                v.as_object()
                    .unwrap()
                    .keys()
                    .filter(|m| ["get", "put", "post", "delete", "patch"].contains(&m.as_str()))
                    .count()
            })
            .sum::<usize>();
        assert_eq!(operations.len(), count);
        for op in operations {
            let contract = &spec["paths"][op.path][op.method.as_str().to_lowercase()];
            if let Some(id) = contract["operationId"].as_str() {
                assert_eq!(id, op.id);
            }
            let params: Vec<_> = contract["parameters"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p["in"] == "path")
                .map(|p| {
                    (
                        p["name"].as_str().unwrap(),
                        if p["name"] == "path" {
                            "network/integration/v1/sites"
                        } else {
                            "example"
                        },
                    )
                })
                .collect();
            let mut call = op.bind(&params).unwrap();
            for q in op.query.iter().filter(|q| q.required) {
                call = call.query(q.name, "example").unwrap();
            }
            let body = op.body_required.then_some(b"{}".as_slice());
            let mut mock = Mock::new(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}");
            let mut headers = [0; 512];
            let mut buffer = [0; 128];
            let client = Client::new("mock.invalid", "/integration", "test-key").unwrap();
            let response = run(client.execute(&mut mock, &call, body, &mut headers, &mut buffer))
                .unwrap()
                .success()
                .unwrap();
            assert!(
                response
                    .json()
                    .unwrap()
                    .required("ok")
                    .unwrap()
                    .boolean()
                    .unwrap()
            );
            let request = core::str::from_utf8(&mock.output).unwrap();
            assert!(
                request.starts_with(&format!(
                    "{} /integration{} HTTP/1.1\r\n",
                    op.method.as_str(),
                    call.path()
                )),
                "{}",
                op.id
            );
            assert!(request.contains("X-API-Key: test-key\r\n"));
            if body.is_some() {
                assert!(request.ends_with("\r\n\r\n{}"));
                assert!(request.contains("Content-Length: 2\r\n"));
            }
        }
    }
}

#[test]
fn encodes_untrusted_paths_queries_and_connector_segments() {
    let repaged = network::get_site_overview_page()
        .unwrap()
        .page(0, 25)
        .unwrap()
        .page(25, 25)
        .unwrap();
    assert_eq!(repaged.path(), "/v1/sites?offset=25&limit=25");
    let retokened = site_manager::list_devices()
        .unwrap()
        .query("hostIds[]", "one")
        .unwrap()
        .query("hostIds[]", "two")
        .unwrap()
        .query("nextToken", "old")
        .unwrap()
        .set_query("nextToken", "new&token")
        .unwrap();
    assert_eq!(
        retokened.path(),
        "/v1/devices?hostIds%5B%5D=one&hostIds%5B%5D=two&nextToken=new%26token"
    );
    let call = network::get_adopted_device_details("../bad/?#\r\n", "🦀").unwrap();
    assert_eq!(
        call.path(),
        "/v1/sites/%2E%2E%2Fbad%2F%3F%23%0D%0A/devices/%F0%9F%A6%80"
    );
    let call = network::get_site_overview_page()
        .unwrap()
        .page(10, 25)
        .unwrap()
        .filter("name.eq('a&b')")
        .unwrap();
    assert_eq!(
        call.path(),
        "/v1/sites?offset=10&limit=25&filter=name.eq%28%27a%26b%27%29"
    );
    let call = site_manager::connector_get("id:1", "/network/integration/v1/sites").unwrap();
    assert_eq!(
        call.path(),
        "/v1/connector/consoles/id%3A1/network/integration/v1/sites"
    );
    assert!(network::GET_INFO.bind(&[("wrong", "x")]).is_err());
    assert!(
        network::GET_ADOPTED_DEVICE_DETAILS
            .bind(&[("siteId", "s"), ("siteId", "s"), ("deviceId", "d")])
            .is_err()
    );
    assert!(
        network::get_site_overview_page()
            .unwrap()
            .query("typo", "x")
            .is_err()
    );
    assert!(
        network::get_site_overview_page()
            .unwrap()
            .filter(&"x".repeat(5000))
            .is_err()
    );
}

#[test]
fn validates_before_any_io_and_redacts_credentials() {
    assert!(Client::new("host\r\nx:1", "", "key").is_err());
    assert!(Client::new("host", "", "key\n").is_err());
    let client = Client::new("host", "", "secret").unwrap();
    assert!(!format!("{client:?}").contains("secret"));
    for call in [
        network::create_network("s").unwrap(),
        network::delete_vouchers("s").unwrap(),
    ] {
        let mut mock = Mock::new(b"");
        let mut h = [0; 128];
        let mut b = [0; 128];
        assert!(run(client.execute(&mut mock, &call, None, &mut h, &mut b)).is_err());
        assert!(mock.output.is_empty());
    }
}

#[test]
fn strict_json_unicode_and_lazy_model_views() {
    let json=Json::parse(br#"{"applicationVersion":"10.6.106","unknown":{"a":[null,true,1.2e-2]},"na\u006de":"caf\u00e9 \ud83e\udd80"}"#).unwrap();
    let info = network::models::ApplicationInfo::from_json(json).unwrap();
    assert!(info.application_version().unwrap().equals("10.6.106"));
    assert_eq!(
        json.required("name")
            .unwrap()
            .string()
            .unwrap()
            .decode::<32>()
            .unwrap(),
        "café 🦀"
    );
    assert!(
        json.required("unknown")
            .unwrap()
            .required("a")
            .unwrap()
            .items()
            .unwrap()
            .next()
            .unwrap()
            .is_null()
    );
    let page=Json::parse(br#"{"count":1,"offset":0,"limit":25,"totalCount":1,"data":[{"id":"site-1","name":"Test","internalReference":"default"}]}"#).unwrap();
    let page = network::models::SiteOverviewPage::from_json(page).unwrap();
    assert_eq!(page.total_count().unwrap(), 1);
    assert!(
        page.data()
            .unwrap()
            .iter()
            .next()
            .unwrap()
            .unwrap()
            .name()
            .unwrap()
            .equals("Test")
    );
    for bad in [
        "",
        "[1,]",
        "{\"a\":1,}",
        "01",
        "-.1",
        "1.",
        "1e",
        "true false",
        "{\"a\":invalid}",
        "\"\\ud800\"",
        "\"\\udc00\"",
        "\"\\x\"",
        "\"\n\"",
    ] {
        assert!(Json::parse(bad.as_bytes()).is_err(), "{bad}");
    }
    let deep = format!("{}0{}", "[".repeat(66), "]".repeat(66));
    assert_eq!(Json::parse(deep.as_bytes()), Err(JsonError::Depth));
    assert_eq!(Json::parse(b"\xff"), Err(JsonError::Utf8));
}

#[test]
fn nested_request_builders_validate_shape_elements_and_required_fields() {
    use unifi::network::models::{AclRuleOrdering, CreateOrUpdateNetwork};
    let mut bytes = [0; 4096];
    let mut ordering = AclRuleOrdering::builder(&mut bytes).unwrap();
    assert!(
        ordering
            .ordered_acl_rule_ids(Json::parse(b"true").unwrap())
            .is_err()
    );
    assert!(
        ordering
            .ordered_acl_rule_ids(Json::parse(b"[true]").unwrap())
            .is_err()
    );
    assert!(ordering.finish().is_err());
    let mut network = CreateOrUpdateNetwork::builder(&mut bytes).unwrap();
    assert!(
        network
            .dhcp_guarding(Json::parse(b"false").unwrap())
            .is_err()
    );
    assert!(network.dhcp_guarding(Json::parse(b"{}").unwrap()).is_err());
    assert!(
        network
            .dhcp_guarding(Json::parse(br#"{"trustedDhcpServerIpAddresses":[]}"#).unwrap())
            .is_err()
    );
    assert!(
        network
            .dhcp_guarding(
                Json::parse(br#"{"trustedDhcpServerIpAddresses":["192.0.2.1"]}"#).unwrap()
            )
            .is_ok()
    );
    // Extension fields also pass the complete model validator at finish.
    let mut ordering = AclRuleOrdering::builder(&mut bytes).unwrap();
    ordering
        .extra_field("orderedAclRuleIds", Json::parse(b"false").unwrap())
        .unwrap();
    assert!(ordering.finish().is_err());
}
#[test]
fn protect_validators_enforce_variants_constants_ranges_and_nested_objects() {
    use unifi::protect::models::{PosTransactionRequest, TalkbackStreamSamplingRate};
    let parse = |v: &'static [u8]| Json::parse(v).unwrap();
    let valid=br#"{"type":"sale","externalId":"mock-id","amount":1.25,"currency":"USD","lineItems":[{"title":"item","quantity":1}]}"#;
    PosTransactionRequest::validate(parse(valid)).unwrap();
    for bytes in [
        br#"{"type":"unknown","externalId":"mock-id","amount":1}"#.as_slice(),
        br#"{"type":"sale","externalId":"mock-id","amount":-1}"#,
        br#"{"type":"sale","externalId":"mock-id","amount":1,"currency":"usd"}"#,
        br#"{"type":"sale","externalId":"mock-id","amount":1,"lineItems":[{"title":"item","quantity":0}]}"#,
        br#"{"type":"sale","externalId":"mock-id","amount":1,"lineItems":[{"title":"item"}]}"#,
        br#"{"type":"sale","externalId":"mock-id","amount":1,"lineItems":[{"title":"item","quantity":1,"unexpected":true}]}"#,
    ] {assert!(PosTransactionRequest::validate(parse(bytes)).is_err(),"{}",core::str::from_utf8(bytes).unwrap());}
    assert!(TalkbackStreamSamplingRate::validate(parse(b"0")).is_err());
    TalkbackStreamSamplingRate::validate(parse(b"24000")).unwrap();
    assert!(unifi::validation::unique(parse(r#"["\ud83e\udd80","🦀"]"#.as_bytes())).is_err());
    assert!(unifi::validation::equivalent(
        parse(br#"{"a":1,"b":2}"#),
        parse(br#"{"b":2,"\u0061":1}"#)
    ));
    assert!(!unifi::validation::equivalent(
        parse(b"9223372036854775807"),
        parse(b"9223372036854775806")
    ));
}

#[test]
fn typed_request_builders_enforce_required_fields_and_scalar_constraints() {
    let mut buffer = [0; 128];
    assert_eq!(
        network::models::DeviceRestartRequest::builder(&mut buffer)
            .unwrap()
            .finish(),
        Err(JsonError::Missing)
    );
    let mut request = network::models::DeviceRestartRequest::builder(&mut buffer).unwrap();
    request.action("INVALID").unwrap();
    assert_eq!(request.finish(), Err(JsonError::Type));
    let mut request = network::models::DeviceRestartRequest::builder(&mut buffer).unwrap();
    request.action("RESTART").unwrap();
    assert!(
        Json::parse(request.finish().unwrap())
            .unwrap()
            .required("action")
            .unwrap()
            .string()
            .unwrap()
            .equals("RESTART")
    );
    let mut writer = unifi::json::ObjectWriter::new(&mut buffer).unwrap();
    writer.field("controls", "\0\u{8}\u{1f}\"\\🦀").unwrap();
    writer.field("array", &[1u32, 2, 3]).unwrap();
    assert!(writer.field("invalid", &f64::INFINITY).is_err());
    let json = Json::parse(writer.finish().unwrap()).unwrap();
    assert!(
        json.required("controls")
            .unwrap()
            .string()
            .unwrap()
            .equals("\0\u{8}\u{1f}\"\\🦀")
    );
    assert_eq!(json.required("array").unwrap().items().unwrap().count(), 3);
    assert_eq!(
        core::mem::size_of::<network::models::NetworkDetails<'_>>(),
        core::mem::size_of::<&str>()
    );
}

#[test]
fn cloud_metric_builder_validates_array_items_despite_spurious_array_required_annotation() {
    use unifi::{Json, site_manager::models::QueryIspMetricsRequest};
    let mut bytes = [0; 1024];
    let mut request = QueryIspMetricsRequest::builder(&mut bytes).unwrap();
    request
        .sites(Json::parse(br#"[{"hostId":"fixture-host","siteId":"fixture-site"}]"#).unwrap())
        .unwrap();
    let body = request.finish().unwrap();
    QueryIspMetricsRequest::validate(Json::parse(body).unwrap()).unwrap();
    assert!(
        QueryIspMetricsRequest::validate(
            Json::parse(br#"{"sites":[{"siteId":"missing-host"}]}"#).unwrap()
        )
        .is_err()
    );
}

#[test]
fn firewall_named_protocols_follow_wire_enums_and_validate_specialized_fields() {
    use unifi::network::models::{
        FirewallPolicyIPv4NamedProtocol, FirewallPolicyIPv6NamedProtocol,
    };
    for body in [
        br#"{"name":"tcp"}"#.as_slice(),
        br#"{"name":"ax.25"}"#,
        br#"{"name":"tcp_udp"}"#,
    ] {
        FirewallPolicyIPv4NamedProtocol::validate(Json::parse(body).unwrap()).unwrap();
    }
    FirewallPolicyIPv4NamedProtocol::validate(
        Json::parse(br#"{"name":"icmp","typenameFilter":"ECHO_REQUEST"}"#).unwrap(),
    )
    .unwrap();
    FirewallPolicyIPv6NamedProtocol::validate(Json::parse(br#"{"name":"icmpv6"}"#).unwrap())
        .unwrap();
    for body in [
        br#"{"name":"TCP"}"#.as_slice(),
        br#"{"name":"icmp","typenameFilter":"bogus"}"#,
        br#"{"name":"bogus"}"#,
    ] {
        assert!(FirewallPolicyIPv4NamedProtocol::validate(Json::parse(body).unwrap()).is_err());
    }
}
