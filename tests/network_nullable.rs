use unifi::{Json, network::models};

#[test]
fn acl_update_accepts_documented_null_filters() {
    let value = Json::parse(
        br#"{"type":"IPV4","action":"ALLOW","enabled":true,"name":"all switches","enforcingDeviceFilter":null,"protocolFilter":null}"#,
    )
    .unwrap();
    models::AclRuleUpdate::validate(value).unwrap();
    let view = models::AclRuleUpdate::from_json(value).unwrap();
    assert!(view.enforcing_device_filter().unwrap().is_none());

    let invalid = Json::parse(
        br#"{"type":"IPV4","action":"ALLOW","enabled":true,"name":"all switches","enforcingDeviceFilter":false}"#,
    )
    .unwrap();
    assert!(models::AclRuleUpdate::validate(invalid).is_err());
}

#[test]
fn disabled_handoff_thresholds_allow_null_but_keep_numeric_bounds() {
    let value =
        Json::parse(br#"{"band5GHzRssiThreshold":null,"band6GHzRssiThreshold":-80}"#).unwrap();
    models::IntegrationWifiHandoffSuggestionsConfigurationDto::validate(value).unwrap();
    for bytes in [
        &br#"{"band5GHzRssiThreshold":-59}"#[..],
        &br#"{"band6GHzRssiThreshold":false}"#[..],
    ] {
        assert!(
            models::IntegrationWifiHandoffSuggestionsConfigurationDto::validate(
                Json::parse(bytes).unwrap()
            )
            .is_err()
        );
    }
}

#[test]
fn firewall_null_filters_preserve_non_null_enum_and_array_constraints() {
    for filters in [
        r#""ipsecFilter":null,"connectionStateFilter":null,"schedule":null"#,
        r#""ipsecFilter":"MATCH_ENCRYPTED","connectionStateFilter":["NEW"]"#,
        r#""ipsecFilter":"unknown""#,
        r#""connectionStateFilter":[]"#,
    ] {
        let bytes = format!(
            r#"{{"action":{{"type":"BLOCK"}},"destination":{{"zoneId":"dst"}},"enabled":true,"ipProtocolScope":{{"ipVersion":"IPV4_AND_IPV6","protocolFilter":null}},"loggingEnabled":false,"name":"policy","source":{{"zoneId":"src"}},{filters}}}"#
        );
        let result =
            models::CreateOrUpdateFirewallPolicy::validate(Json::parse(bytes.as_bytes()).unwrap());
        assert_eq!(
            result.is_ok(),
            !filters.contains("unknown") && !filters.contains("[]")
        );
    }
}

#[test]
fn nullable_schedule_view_retains_null() {
    let value = Json::parse(b"null").unwrap();
    models::FirewallSchedule::validate(value).unwrap();
    assert!(
        models::FirewallSchedule::from_json(value)
            .unwrap()
            .json()
            .is_null()
    );
    assert!(models::FirewallSchedule::validate(Json::parse(b"false").unwrap()).is_err());
}
