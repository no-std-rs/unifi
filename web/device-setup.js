import { DisplaySerial } from "./serial-client.js";
import { validateConfig, validateWifi } from "./config.js";

export function setupDevice({ readForm, applyConfig, beforeConnect }) {
  const $ = id => document.getElementById(id);
  const status = $("usb_status");
  const controls = ["usb_read", "usb_scan", "usb_test", "usb_save", "usb_status_read"];
  let busy = false, generation = 0;
  const message = (text, error = false) => { status.textContent = text; status.classList.toggle("error", error); status.setAttribute("role", error ? "alert" : "status"); };
  function refresh() {
    $("blank_installer").querySelector("button").disabled = busy || client.connected;
    $("usb_connect").disabled = busy || client.connected;
    $("usb_disconnect").disabled = !client.connected;
    for (const id of controls) $(id).disabled = busy || !client.connected;
    $("wifi_networks").disabled = busy || !client.connected;
  }
  const client = new DisplaySerial(() => {
    generation++;
    message("Display disconnected. Its dashboard returns automatically; an unplugged session expires within 10 seconds.");
    refresh();
  });
  async function run(action) {
    if (busy) return;
    busy = true; refresh();
    // Keep the draft stable while a device operation reads or writes it.
    const fields = [...document.querySelectorAll("#settings input, #settings select, #settings button")];
    const disabled = fields.map(field => field.disabled);
    fields.forEach(field => { field.disabled = true; });
    const current = generation;
    let after;
    try { after = await action(() => current === generation); }
    catch (error) { if (current === generation) message(error.message, true); }
    finally { fields.forEach((field, i) => { field.disabled = disabled[i]; }); busy = false; refresh(); if (current === generation) after?.(); }
  }
  $("usb_connect").addEventListener("click", () => run(async () => {
    beforeConnect(); message("Choose your display’s USB port. Close the firmware flasher first.");
    await client.connect();
    const result = await client.command({ name: "status" });
    message(result.configured ? "APP MODE connected. Read saved settings to edit them, or scan and test a new network." : "APP MODE connected. Scan Wi-Fi, test its password, then save your local UniFi settings below.");
  }));
  $("usb_disconnect").addEventListener("click", () => { generation++; void client.disconnect(); });
  $("usb_read").addEventListener("click", () => run(async current => {
    const result = await client.command({ name: "get_config" });
    if (!current()) return;
    if (!result.config) { message("This display has no saved settings. Fill in the form below."); return; }
    const config = validateConfig(result.config);
    return () => { applyConfig(config); message("Saved device settings loaded into the form. Edit and save to apply changes."); };
  }));
  $("usb_scan").addEventListener("click", () => run(async current => {
    message("Scanning nearby 2.4 GHz networks on the display…");
    const result = await client.command({ name: "scan_wifi" }, 40000);
    if (!current()) return;
    const select = $("wifi_networks");
    select.replaceChildren(new Option("Choose a network…", ""));
    const seen = new Set();
    for (const ap of [...result.networks].sort((a, b) => b.rssi - a.rssi)) {
      if (typeof ap.ssid !== "string" || !ap.ssid || seen.has(ap.ssid)) continue;
      seen.add(ap.ssid);
      select.add(new Option(`${ap.ssid} · ${ap.rssi} dBm · ${ap.secure ? "secured" : "open"}`, ap.ssid));
    }
    select.hidden = false;
    message(seen.size ? "Choose a network, or enter a hidden network’s name in the form below." : "No networks found. Retry or enter a hidden network’s name below.");
  }));
  $("wifi_networks").addEventListener("change", () => {
    if (!$("wifi_networks").value) return;
    $("wifi_ssid").value = $("wifi_networks").value;
    $("wifi_ssid").dispatchEvent(new Event("input", { bubbles: true }));
  });
  $("usb_test").addEventListener("click", () => run(async current => {
    const { wifi_ssid: ssid, wifi_password: password } = readForm();
    validateWifi(ssid, password);
    message("The display is testing this Wi-Fi password and requesting an IP address…");
    const result = await client.command({ name: "test_wifi", ssid, password }, 65000);
    if (current()) message(`Wi-Fi verified by the display: ${result.ssid}, IP ${result.ip}. The test did not save settings. Disconnect APP MODE to reconnect the saved network.`);
  }));
  $("usb_status_read").addEventListener("click", () => run(async current => {
    const result = await client.command({ name: "status" });
    if (current()) message(`APP MODE · ${result.configured ? "settings saved" : "setup required"} · ${result.ip ? `IP ${result.ip}` : "no active DHCP address"}`);
  }));
  $("usb_save").addEventListener("click", () => run(async current => {
    const config = validateConfig(readForm());
    message("Saving local settings on the display…");
    await client.command({ name: "save_config", config }, 10000);
    if (current()) message("Settings saved and verified. The display is restarting into its local dashboard.");
    await client.disconnect(false);
    message("Settings saved and verified. The display is restarting into its local dashboard.");
  }));
  window.addEventListener("pagehide", () => { generation++; void client.disconnect(false); });
  refresh();
  return { disconnect: () => client.disconnect(), connected: () => client.connected };
}
