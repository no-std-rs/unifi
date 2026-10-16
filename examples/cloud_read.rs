//! Host-only GET verification. API keys and identifiers are never printed.
mod support;
use support::get;
use unifi::{Client, Json, network, site_manager};

fn main() {
    let key = support::api_key();
    let cloud = Client::new("api.ui.com", "", &key).unwrap();
    let id = discover_console(|call| get(&cloud, call).ok_or("host discovery failed"))
        .expect("console discovery failed");
    let base = format!("/v1/connector/consoles/{id}/proxy/network/integration");
    let network = Client::new("api.ui.com", &base, &key).unwrap();
    let info = get(&network, network::get_info().unwrap()).expect("application info failed");
    let info = network::models::ApplicationInfo::from_json(Json::parse(&info).unwrap()).unwrap();
    println!(
        "Network {}",
        info.application_version().unwrap().decode::<64>().unwrap()
    );
    let sites = get(&network, network::get_site_overview_page().unwrap()).expect("sites failed");
    let sites = network::models::SiteOverviewPage::from_json(Json::parse(&sites).unwrap()).unwrap();
    println!("{} sites", sites.total_count().unwrap());
    for site in sites.data().unwrap().iter() {
        let id = site.unwrap().id().unwrap().decode::<256>().unwrap();
        let devices = get(
            &network,
            network::get_adopted_device_overview_page(&id)
                .unwrap()
                .page(0, 5)
                .unwrap(),
        )
        .expect("devices failed");
        let devices =
            network::models::AdoptedDeviceOverviewPage::from_json(Json::parse(&devices).unwrap())
                .unwrap();
        for device in devices.data().unwrap().iter() {
            let device = device.unwrap();
            let device_id = device.id().unwrap().decode::<256>().unwrap();
            let details = get(
                &network,
                network::get_adopted_device_details(&id, &device_id).unwrap(),
            )
            .expect("device detail failed");
            network::models::AdoptedDeviceDetails::from_json(Json::parse(&details).unwrap())
                .unwrap()
                .state()
                .unwrap();
            get(
                &network,
                network::get_adopted_device_latest_statistics(&id, &device_id).unwrap(),
            );
        }
        for call in [
            network::get_connected_client_overview_page(&id),
            network::get_networks_overview_page(&id),
            network::get_wifi_broadcast_page(&id),
            network::get_firewall_zones(&id),
            network::get_firewall_policies(&id),
            network::get_dns_policy_page(&id),
            network::get_acl_rule_page(&id),
            network::get_wans_overview_page(&id),
        ] {
            get(&network, call.unwrap().page(0, 5).unwrap());
        }
    }
    get(&cloud, site_manager::list_sites().unwrap());
    get(&cloud, site_manager::list_devices().unwrap());
}

fn discover_console(
    mut fetch: impl FnMut(unifi::Call) -> Result<Vec<u8>, &'static str>,
) -> Result<heapless::String<256>, &'static str> {
    let mut next = None::<heapless::String<512>>;
    let mut selected = None;
    loop {
        let mut call = site_manager::list_hosts().unwrap();
        if let Some(token) = &next {
            call = call
                .query("nextToken", token)
                .map_err(|_| "host token too long")?;
        }
        let bytes = fetch(call)?;
        let json = Json::parse(&bytes).map_err(|_| "invalid host response")?;
        for host in json
            .required("data")
            .and_then(Json::items)
            .map_err(|_| "invalid hosts")?
        {
            let console = host
                .get("type")
                .ok()
                .flatten()
                .and_then(|v| v.string().ok())
                .is_some_and(|v| v.equals("console"));
            let connected = host
                .get("reportedState")
                .ok()
                .flatten()
                .and_then(|v| v.get("state").ok().flatten())
                .and_then(|v| v.string().ok())
                .is_some_and(|v| v.equals("connected"));
            if console && connected {
                if selected.is_some() {
                    return Err("multiple connected consoles; select explicitly in the CLI");
                }
                selected = Some(
                    host.required("id")
                        .and_then(Json::string)
                        .and_then(|v| v.decode())
                        .map_err(|_| "invalid host ID")?,
                );
            }
        }
        let token = json
            .optional("nextToken")
            .map_err(|_| "invalid host token")?
            .map(|v| v.string().and_then(|s| s.decode::<512>()))
            .transpose()
            .map_err(|_| "invalid host token")?
            .filter(|s| !s.is_empty());
        if token.is_none() {
            break;
        }
        if token == next {
            return Err("host pagination did not advance");
        }
        next = token;
    }
    selected.ok_or("no connected console")
}
#[test]
fn discovers_console_on_later_host_page() {
    let mut calls = 0;
    let id=discover_console(|call| {
        calls+=1;
        if calls==1 {assert_eq!(call.path(),"/v1/hosts");Ok(br#"{"data":[],"nextToken":"later"}"#.to_vec())}
        else {assert_eq!(call.path(),"/v1/hosts?nextToken=later");Ok(br#"{"data":[{"id":"mock-console","type":"console","reportedState":{"state":"connected"}}]}"#.to_vec())}
    }).unwrap();
    assert_eq!(id, "mock-console");
    assert_eq!(calls, 2);
    assert!(discover_console(|_| Ok(br#"{"data":[],"nextToken":"same"}"#.to_vec())).is_err());
}
