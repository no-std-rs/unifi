use unifi::{Json, network::models};

#[test]
fn repeated_acl_properties_keep_endpoint_constraints() {
    for (kind, endpoint, extra) in [
        ("IPV4", r#"{"type":"PORTS","portFilter":[443]}"#, ""),
        (
            "MAC",
            r#"{"type":"MAC_ADDRESSES","macAddresses":["aa:bb:cc:dd:ee:ff"]}"#,
            r#","networkIdFilter":"network""#,
        ),
    ] {
        for (filter, valid) in [(endpoint, true), ("false", false), ("{}", false)] {
            let bytes = format!(
                r#"{{"type":"{kind}","action":"ALLOW","enabled":true,"id":"rule","index":0,"name":"rule","metadata":{{"origin":"USER_DEFINED"}},"destinationFilter":{filter},"sourceFilter":{filter}{extra}}}"#
            );
            let value = Json::parse(bytes.as_bytes()).unwrap();
            assert_eq!(models::AclRule::validate(value).is_ok(), valid);
            if valid && kind == "IPV4" {
                let view = models::IntegrationIpAclRuleDto::from_json(value).unwrap();
                let endpoint: models::IpAclRuleEndpoint<'_> =
                    view.destination_filter().unwrap().unwrap();
                assert!(endpoint.r#type().unwrap().equals("PORTS"));
            } else if valid {
                let view = models::IntegrationMacAclRuleDto::from_json(value).unwrap();
                let endpoint: models::MacAclRuleEndpoint<'_> =
                    view.source_filter().unwrap().unwrap();
                assert!(endpoint.r#type().unwrap().equals("MAC_ADDRESSES"));
            }
        }
    }
}

#[test]
fn required_nullable_pmf_distinguishes_null_from_missing() {
    for pmf in ["null", r#""OPTIONAL""#] {
        let bytes = format!(
            r#"{{"type":"WPA2_WPA3_PERSONAL","passphrase":"password","pmfMode":{pmf},"saeConfiguration":{{"anticloggingThresholdSeconds":5,"syncTimeSeconds":5}},"wpa3FastRoamingEnabled":false}}"#
        );
        let value = Json::parse(bytes.as_bytes()).unwrap();
        models::IntegrationWifiWpa2Wpa3PersonalSecurityConfigurationDetailDto::validate(value)
            .unwrap();
        let view =
            models::IntegrationWifiWpa2Wpa3PersonalSecurityConfigurationDetailDto::from_json(value)
                .unwrap();
        let mode = view.pmf_mode().unwrap();
        assert_eq!(mode.is_none(), pmf == "null");
        if let Some(mode) = mode {
            assert!(mode.equals("OPTIONAL"));
        }
        let enterprise = format!(
            r#"{{"type":"WPA2_WPA3_ENTERPRISE","coaEnabled":false,"pmfMode":{pmf},"radiusConfiguration":{{"nasId":{{"type":"DERIVED","source":"DEVICE_MAC_ADDRESS"}},"profileId":"radius"}},"wpa3FastRoamingEnabled":false}}"#
        );
        let value = Json::parse(enterprise.as_bytes()).unwrap();
        models::IntegrationWifiWpa2Wpa3EnterpriseSecurityConfigurationDetailDto::validate(value)
            .unwrap();
        let view =
            models::IntegrationWifiWpa2Wpa3EnterpriseSecurityConfigurationDetailDto::from_json(
                value,
            )
            .unwrap();
        assert_eq!(view.pmf_mode().unwrap().is_none(), pmf == "null");
    }
    let missing = Json::parse(br#"{"type":"WPA2_WPA3_PERSONAL"}"#).unwrap();
    let view =
        models::IntegrationWifiWpa2Wpa3PersonalSecurityConfigurationDetailDto::from_json(missing)
            .unwrap();
    assert!(view.pmf_mode().is_err());
}
