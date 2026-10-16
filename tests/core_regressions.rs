#[cfg(feature = "serde")]
use unifi::Json;
use unifi::{Authentication, Call, Method, session::Session};

#[test]
fn percent_encoded_query_names_are_replaced() {
    let call = Call::raw(Method::GET, "/x?%6Cimit=10&limit=11&keep=one&keep=two")
        .unwrap()
        .set_query("limit", "20")
        .unwrap();
    assert_eq!(call.path(), "/x?keep=one&keep=two&limit=20");
    let call = Call::raw(Method::GET, "/x?%6Fffset=3&%6cimit=10")
        .unwrap()
        .page(40, 20)
        .unwrap();
    assert_eq!(call.path(), "/x?offset=40&limit=20");
}

#[cfg(feature = "serde")]
#[test]
fn serde_decodes_surrogate_pairs_in_nested_values_and_keys() {
    #[derive(serde::Deserialize)]
    struct Payload {
        #[serde(rename = "😀")]
        names: heapless::Vec<heapless::String<32>, 4>,
    }
    let json =
        Json::parse(br#"{"\uD83D\uDE00":["\uD83D\uDE00","literal \\uD83D","caf\u00e9"]}"#).unwrap();
    let payload: Payload = json.decode(&mut [0; 32]).unwrap();
    assert_eq!(payload.names[0], "😀");
    assert_eq!(payload.names[1], "literal \\uD83D");
    assert_eq!(payload.names[2], "café");
}

#[cfg(feature = "serde")]
#[test]
fn serde_adapter_preserves_borrowing_options_and_enum_payloads() {
    #[derive(serde::Deserialize)]
    struct Borrowed<'a> {
        name: &'a str,
        enabled: bool,
        count: u32,
    }
    let value: Borrowed<'_> = Json::parse(br#"{"name":"borrowed","enabled":true,"count":7}"#)
        .unwrap()
        .decode(&mut [])
        .unwrap();
    assert_eq!(
        (value.name, value.enabled, value.count),
        ("borrowed", true, 7)
    );
    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Name(heapless::String<16>);
    #[derive(serde::Deserialize, Debug, PartialEq)]
    enum Event {
        #[serde(rename = "😀")]
        Ready,
        Name(Name),
        Pair(Option<heapless::String<16>>, u8),
        Record {
            label: heapless::String<16>,
        },
    }
    let name = heapless::String::try_from("😀").unwrap();
    for (wire, expected) in [
        (br#""\uD83D\uDE00""#.as_slice(), Event::Ready),
        (
            br#"{"Name":"\uD83D\uDE00"}"#,
            Event::Name(Name(name.clone())),
        ),
        (
            br#"{"Pair":["\uD83D\uDE00",7]}"#,
            Event::Pair(Some(name.clone()), 7),
        ),
        (
            br#"{"Record":{"label":"\uD83D\uDE00"}}"#,
            Event::Record { label: name },
        ),
    ] {
        let event: Event = Json::parse(wire).unwrap().decode(&mut [0; 16]).unwrap();
        assert_eq!(event, expected);
    }
}

#[test]
fn logout_header_order_does_not_leak_csrf_to_a_later_session() {
    for reversed in [false, true] {
        let mut session = Session::<64, 32>::from_cookie("TOKEN=old", Some("csrf-old")).unwrap();
        let headers = if reversed {
            [
                ("X-Updated-CSRF-Token", "csrf-last"),
                ("Set-Cookie", "TOKEN=; Max-Age=0"),
            ]
        } else {
            [
                ("Set-Cookie", "TOKEN=; Max-Age=0"),
                ("X-Updated-CSRF-Token", "csrf-last"),
            ]
        };
        session
            .observe_response(
                headers
                    .iter()
                    .map(|(name, value)| (*name, value.as_bytes())),
            )
            .unwrap();
        session.observe("Set-Cookie", b"TOKEN=new").unwrap();
        assert!(matches!(
            session.authentication(),
            Some(Authentication::Session { csrf: None, .. })
        ));
    }
}

#[test]
fn response_cookie_replacement_retains_only_a_fresh_csrf_token() {
    for csrf_position in 0..=3 {
        let mut session = Session::<64, 32>::from_cookie("TOKEN=old", Some("csrf-old")).unwrap();
        let mut headers = vec![
            ("Set-Cookie", "TOKEN=; Max-Age=0"),
            ("Set-Cookie", "TOKEN=new"),
        ];
        if csrf_position < 3 {
            headers.insert(csrf_position, ("X-Updated-CSRF-Token", "csrf-new"));
        }
        session
            .observe_response(
                headers
                    .iter()
                    .map(|(name, value)| (*name, value.as_bytes())),
            )
            .unwrap();
        assert!(matches!(
            session.authentication(),
            Some(Authentication::Session {
                cookie: "TOKEN=new",
                ..
            })
        ));
        match session.authentication().unwrap() {
            Authentication::Session { csrf, .. } => {
                assert_eq!(csrf, (csrf_position < 3).then_some("csrf-new"))
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn invalid_response_headers_leave_the_original_session_intact() {
    let mut session = Session::<64, 32>::from_cookie("TOKEN=old", Some("csrf-old")).unwrap();
    assert!(
        session
            .observe_response([
                ("Set-Cookie", b"TOKEN=new".as_slice()),
                ("X-Updated-CSRF-Token", b"invalid\n".as_slice()),
            ])
            .is_err()
    );
    assert!(matches!(
        session.authentication(),
        Some(Authentication::Session {
            cookie: "TOKEN=old",
            csrf: Some("csrf-old")
        })
    ));
}
