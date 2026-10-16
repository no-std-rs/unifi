use unifi::{Json, site_manager::models};

#[test]
fn nullable_device_entries_remain_visible_in_typed_arrays() {
    let value =
        Json::parse(br#"{"data":[null,{"hostId":"host","devices":[null,{"id":"device"}]}]}"#)
            .unwrap();
    models::ListDevicesResponse200::validate(value).unwrap();
    let view = models::ListDevicesResponse200::from_json(value).unwrap();
    let mut hosts = view.data().unwrap().unwrap().iter();
    assert!(hosts.next().unwrap().unwrap().json().is_null());
    let host = hosts.next().unwrap().unwrap();
    assert!(host.host_id().unwrap().unwrap().equals("host"));
    let mut devices = host.devices().unwrap().unwrap().iter();
    assert!(devices.next().unwrap().unwrap().json().is_null());
    assert!(
        devices
            .next()
            .unwrap()
            .unwrap()
            .id()
            .unwrap()
            .unwrap()
            .equals("device")
    );
    assert!(hosts.next().is_none());
    assert!(devices.next().is_none());
}

#[test]
fn nullable_query_sites_accept_null_and_reject_other_scalar_values() {
    let value = Json::parse(br#"{"sites":[null,{"hostId":"host","siteId":"site"}]}"#).unwrap();
    models::QueryIspMetricsRequest::validate(value).unwrap();
    let view = models::QueryIspMetricsRequest::from_json(value).unwrap();
    let mut sites = view.sites().unwrap().unwrap().iter();
    assert!(sites.next().unwrap().unwrap().json().is_null());
    let site = sites.next().unwrap().unwrap();
    assert!(site.host_id().unwrap().equals("host"));
    assert!(site.site_id().unwrap().equals("site"));
    let invalid = Json::parse(br#"{"sites":[false]}"#).unwrap();
    assert!(models::QueryIspMetricsRequest::validate(invalid).is_err());
    let view = models::QueryIspMetricsRequest::from_json(invalid).unwrap();
    assert!(
        view.sites()
            .unwrap()
            .unwrap()
            .iter()
            .next()
            .unwrap()
            .is_err()
    );
}
