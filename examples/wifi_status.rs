//! Read a live Wi-Fi snapshot through the no_std Rust client. GET requests only.
//! std and OpenSSL are used only for this host example's transport and output.
mod support;

use serde_json::{Value, json};
use std::collections::BTreeMap;
use unifi::{Call, Client, Json, JsonString, network, network::models, site_manager};

fn text(value: JsonString<'_>) -> String {
    value.chars().collect()
}

/// Read every Network page without silently imposing a device/client limit.
fn pages(client: &Client<'_>, call: Call) -> Vec<Vec<u8>> {
    let mut offset = 0;
    let mut output = Vec::new();
    loop {
        let bytes = support::get(client, call.clone().page(offset, 200).unwrap())
            .expect("Network page request failed");
        let page = Json::parse(&bytes).expect("valid Network JSON");
        let count = page.required("data").unwrap().items().unwrap().count() as u64;
        let total = page.required("totalCount").unwrap().unsigned().unwrap();
        offset = offset.checked_add(count).expect("pagination overflow");
        output.push(bytes);
        if count == 0 || offset >= total {
            break;
        }
    }
    output
}

fn site_snapshot(client: &Client<'_>, site: models::SiteOverview<'_>) -> Value {
    let site_id = text(site.id().unwrap());
    let mut client_counts = BTreeMap::<String, usize>::new();
    let mut clients_by_ap = BTreeMap::<String, usize>::new();
    for bytes in pages(
        client,
        network::get_connected_client_overview_page(&site_id).unwrap(),
    ) {
        let page = models::ClientOverviewPage::from_json(Json::parse(&bytes).unwrap()).unwrap();
        for entry in page.data().unwrap().iter() {
            let entry = entry.unwrap();
            let kind = text(entry.r#type().unwrap());
            if kind == "WIRELESS" {
                let wireless = models::WirelessClientOverview::from_json(entry.json()).unwrap();
                *clients_by_ap
                    .entry(text(wireless.uplink_device_id().unwrap()))
                    .or_default() += 1;
            }
            *client_counts.entry(kind).or_default() += 1;
        }
    }
    let mut access_points = Vec::new();
    for bytes in pages(
        client,
        network::get_adopted_device_overview_page(&site_id).unwrap(),
    ) {
        let page =
            models::AdoptedDeviceOverviewPage::from_json(Json::parse(&bytes).unwrap()).unwrap();
        for entry in page.data().unwrap().iter() {
            let entry = entry.unwrap();
            if !entry
                .features()
                .unwrap()
                .any(|v| v.string().unwrap().equals("accessPoint"))
            {
                continue;
            }
            let device_id = text(entry.id().unwrap());
            let details = support::get(
                client,
                network::get_adopted_device_details(&site_id, &device_id).unwrap(),
            )
            .expect("AP detail request failed");
            let details =
                models::AdoptedDeviceDetails::from_json(Json::parse(&details).unwrap()).unwrap();
            let statistics = support::get(
                client,
                network::get_adopted_device_latest_statistics(&site_id, &device_id).unwrap(),
            );
            let statistics = statistics.as_ref().map(|bytes| {
                models::LatestStatisticsForADevice::from_json(Json::parse(bytes).unwrap()).unwrap()
            });
            let mut radios = Vec::new();
            if let Some(items) = details.interfaces().unwrap().radios().unwrap() {
                for radio in items.iter() {
                    let radio = radio.unwrap();
                    let frequency = radio.frequency_g_hz().unwrap();
                    let mut retries = None;
                    if let Some(stats) = statistics
                        && let Some(items) = stats.interfaces().unwrap().radios().unwrap()
                    {
                        for item in items.iter() {
                            let item = item.unwrap();
                            if item.frequency_g_hz().unwrap() == frequency {
                                retries = item.tx_retries_pct().unwrap();
                            }
                        }
                    }
                    radios.push(json!({
                        "frequencyGHz": frequency,
                        "channel": radio.channel().unwrap(),
                        "widthMHz": radio.channel_width_m_hz().unwrap(),
                        "txRetriesPct": retries,
                    }));
                }
            }
            access_points.push(json!({
                "name": text(details.name().unwrap()),
                "model": text(details.model().unwrap()),
                "state": text(details.state().unwrap()),
                "wirelessClients": clients_by_ap.get(&device_id).copied().unwrap_or(0),
                "lastHeartbeatAt": statistics.and_then(|s| s.last_heartbeat_at().unwrap()).map(text),
                "radios": radios,
            }));
        }
    }
    let mut broadcasts = Vec::new();
    for bytes in pages(client, network::get_wifi_broadcast_page(&site_id).unwrap()) {
        let page = models::IntegrationWifiBroadcastPageDto::from_json(Json::parse(&bytes).unwrap())
            .unwrap();
        for wifi in page.data().unwrap().iter() {
            let wifi = wifi.unwrap();
            broadcasts.push(json!({
                "name": text(wifi.name().unwrap()),
                "enabled": wifi.enabled().unwrap(),
                "security": text(wifi.security_configuration().unwrap().r#type().unwrap()),
            }));
        }
    }
    json!({"site":text(site.name().unwrap()),"clients":client_counts,"accessPoints":access_points,"wifi":broadcasts})
}

fn main() {
    let key = support::api_key();
    let cloud = Client::new("api.ui.com", "", &key).unwrap();
    let mut token = None;
    let mut consoles = Vec::new();
    loop {
        let mut call = site_manager::list_hosts().unwrap();
        if let Some(token) = token.as_deref() {
            call = call.query("nextToken", token).unwrap();
        }
        let bytes = support::get(&cloud, call).expect("cloud host discovery failed");
        let hosts = Json::parse(&bytes).unwrap();
        for host in hosts.required("data").unwrap().items().unwrap() {
            if host
                .required("type")
                .unwrap()
                .string()
                .unwrap()
                .equals("console")
                && host
                    .required("reportedState")
                    .unwrap()
                    .required("state")
                    .unwrap()
                    .string()
                    .unwrap()
                    .equals("connected")
            {
                consoles.push(text(host.required("id").unwrap().string().unwrap()));
            }
        }
        token = hosts
            .optional("nextToken")
            .unwrap()
            .map(|t| text(t.string().unwrap()))
            .filter(|t| !t.is_empty());
        if token.is_none() {
            break;
        }
    }
    assert!(!consoles.is_empty(), "no connected cloud consoles");
    let mut sites = Vec::new();
    for console in consoles {
        let base = format!("/v1/connector/consoles/{console}/proxy/network/integration");
        let client = Client::new("api.ui.com", &base, &key).unwrap();
        for bytes in pages(&client, network::get_site_overview_page().unwrap()) {
            let page = models::SiteOverviewPage::from_json(Json::parse(&bytes).unwrap()).unwrap();
            for site in page.data().unwrap().iter() {
                sites.push(site_snapshot(&client, site.unwrap()));
            }
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"source":"unifi Rust API / reqwless","sites":sites}))
            .unwrap()
    );
}
