use unifi::{Json, session_api};

#[test]
fn session_constructors_include_source_required_queries() {
    let roam = session_api::get_client_roams("default", "aa:bb:cc:dd:ee:ff").unwrap();
    assert_eq!(
        roam.path(),
        "/v2/api/site/default/system-log/client-connection/aa%3Abb%3Acc%3Add%3Aee%3Aff?mac=aa%3Abb%3Acc%3Add%3Aee%3Aff"
    );
    roam.validate(None).unwrap();
    let peers = session_api::list_wireguard_peers("default", "server id").unwrap();
    assert_eq!(
        peers.path(),
        "/v2/api/site/default/wireguard/server%20id/users?networkId=server%20id"
    );
    peers.validate(None).unwrap();
    let ports = session_api::suggest_openvpn_ports("default").unwrap();
    assert_eq!(
        ports.path(),
        "/v2/api/site/default/network/port-suggest?service=openvpn"
    );
    ports.validate(None).unwrap();
    assert!(
        session_api::GET_CLIENT_ROAMS
            .bind(&[("site", "default"), ("mac", "client")])
            .unwrap()
            .validate(None)
            .is_err()
    );
    assert!(
        session_api::LIST_WIREGUARD_PEERS
            .bind(&[("site", "default"), ("serverId", "server")])
            .unwrap()
            .validate(None)
            .is_err()
    );
    assert!(
        session_api::SUGGEST_OPENVPN_PORTS
            .bind(&[("site", "default")])
            .unwrap()
            .validate(None)
            .is_err()
    );
}

#[test]
fn v2_responses_do_not_claim_the_legacy_envelope() {
    for operation in session_api::OPERATIONS {
        if operation.path.starts_with("/v2/api/") {
            assert_ne!(operation.response_schema, Some("Envelope"));
        }
    }
    assert_eq!(session_api::GET_HEALTH.response_schema, Some("Envelope"));
    for bytes in [&b"[]"[..], &br#"{"connections":[]}"#[..]] {
        let value = Json::parse(bytes).unwrap();
        session_api::models::BareResponse::validate(value).unwrap();
        session_api::models::BareResponse::from_json(value).unwrap();
        assert!(session_api::models::Envelope::validate(value).is_err());
    }
}
