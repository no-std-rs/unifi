import { DisplaySerial } from "./serial-client.js";
import { validateConfig, validateWifi } from "./config.js";
import { localConfig, localFields } from "./network-profiles.js";

export function setupDevice({ readForm, readSaveConfig, applyConfig, applyLocal, lookupNetwork, rememberNetwork, beforeConnect }) {
  const $ = id => document.getElementById(id);
  let busy = false, manual = false, ready = false, generation = 0, verified = null, editing = false, saved = false;
  let accessPoints = new Map();
  function note(id, text, error = false) {
    $(id).textContent = text; $(id).classList.toggle("error", error);
    $(id).setAttribute("role", error ? "alert" : "status");
  }
  const matches = () => verified && verified.ssid === $("wifi_ssid").value && verified.password === $("wifi_password").value;
  function refresh() {
    const connected = client.connected;
    $("display_title").textContent = connected ? "Display connected" : "Connect your display";
    $("display_title").closest("fieldset").classList.toggle("connected", connected);
    $("settings").setAttribute("aria-busy", String(busy));
    $("usb_connect").hidden = connected; $("usb_connect").disabled = busy || !navigator.serial || !window.isSecureContext;
    $("usb_disconnect").hidden = !connected;
    $("manual_setup").disabled = busy || connected;
    $("connected_tools").hidden = !connected;
    for (const id of ["usb_read", "usb_status_read"]) $(id).disabled = busy || !connected;
    $("blank_installer").querySelector("button").disabled = busy || connected;
    $("wifi_step").hidden = !connected && !manual;
    $("wifi_step").disabled = busy;
    $("unifi_step").hidden = !manual && !(connected && matches());
    $("unifi_step").disabled = busy;
    $("usb_test").hidden = !connected || (!!matches() && !editing);
    $("usb_test").disabled = busy || !$("wifi_ssid").value;
    $("wifi_change").hidden = !connected || !$("wifi_ssid").value || editing;
    $("usb_save").hidden = !connected || !matches();
    $("usb_save").disabled = busy;
    $("prepare").hidden = !manual;
    $("prepare").disabled = busy || !ready;
    $("install-area").hidden = connected || !$("installer").manifest;
    $("network_picker").hidden = !connected || !accessPoints.size;
    $("password_editor").hidden = connected && !editing;
    const step = !connected && !manual ? "display" : connected && matches() ? "unifi" : "wifi";
    for (const name of ["display", "wifi", "unifi"]) {
      $("step_" + name).classList.toggle("complete", name === "display" && connected || name === "wifi" && !!matches());
      if (step === name) $("step_" + name).setAttribute("aria-current", "step");
      else $("step_" + name).removeAttribute("aria-current");
    }
  }
  const client = new DisplaySerial(() => {
    generation++; verified = null; saved ? note("usb_status", "Settings saved. Your display is restarting.") : note("usb_status", "Display disconnected. Connect again to continue.");
    refresh();
  });
  async function run(action) {
    if (busy) return;
    busy = true; refresh();
    const started = generation;
    try { await action(() => started === generation); }
    catch (error) { if (started === generation) note("usb_status", error.message, true); }
    finally { busy = false; refresh(); if (started === generation && client.connected && editing) $("wifi_password").focus(); }
  }
  function localChanged() {
    note("unifi_status", "");
    try {
      const config = localConfig(Object.fromEntries(localFields.map(name => [name, readForm()[name]])));
      $("unifi_summary").textContent = `Local controller: ${new URL(config.controller_url).host}. You can change its connection settings below.`;
    } catch { $("unifi_summary").textContent = "Add your local Network API key and trusted certificate, or discover your gateway with a cloud key."; }
  }
  async function testWifi(current) {
    const { wifi_ssid: ssid, wifi_password: password } = readForm();
    verified = null; refresh();
    try {
      validateWifi(ssid, password);
      note("wifi_status", `Checking ${ssid} on your display…`);
      // Includes a possible 20s reconnect lock, association, DHCP, cleanup and reply deadlines.
      const result = await client.command({ name: "test_wifi", ssid, password }, 80000);
      if (!current()) return;
      verified = { ssid, password }; editing = false;
      rememberNetwork(ssid, password);
      note("wifi_status", `Wi-Fi verified · ${result.ip}`);
      $("usb_test").textContent = "Connect to Wi-Fi";
      localChanged();
    } catch (error) {
      if (!current()) return;
      editing = true;
      note("wifi_status", `${error.message.replace(/\.+$/, "")}. Change the password or try again.`, true);
      $("usb_test").textContent = "Try again";
    }
  }
  async function choose(ssid, current, secure = true) {
    verified = null; editing = false;
    $("wifi_ssid").value = ssid;
    const known = lookupNetwork(ssid);
    $("wifi_password").value = secure ? known?.password || "" : "";
    applyLocal(known?.config || null);
    $("unifi_editor").open = !known?.config;
    $("manual_wifi").hidden = !!accessPoints.get(ssid);
    refresh();
    if (!secure || known && known.password !== "") await testWifi(current);
    else { editing = true; note("wifi_status", `Enter the password for ${ssid}, then connect.`); }
  }
  async function scan(current) {
    note("wifi_status", "Finding nearby 2.4 GHz networks…");
    const { networks } = await client.command({ name: "scan_wifi" }, 40000);
    if (!current()) return;
    accessPoints = new Map();
    const select = $("wifi_networks");
    const placeholder = new Option("Choose a network…", ""); placeholder.disabled = true;
    select.replaceChildren(placeholder);
    for (const ap of [...networks].sort((a, b) => b.rssi - a.rssi)) {
      if (typeof ap.ssid !== "string" || !ap.ssid || accessPoints.has(ap.ssid)) continue;
      accessPoints.set(ap.ssid, ap);
      select.add(new Option(`${ap.ssid}${lookupNetwork(ap.ssid) ? " · saved" : ""}${ap.secure ? "" : " · open"}`, ap.ssid));
    }
    select.add(new Option("Other network…", ""));
    const previous = $("wifi_ssid").value;
    if (accessPoints.has(previous)) {
      select.value = previous;
      await choose(previous, current, accessPoints.get(previous).secure);
    } else {
      select.selectedIndex = 0;
      editing = false; $("manual_wifi").hidden = !!accessPoints.size;
      note("wifi_status", accessPoints.size ? "Choose a network. Its saved password will be checked automatically." : "No networks found. Enter a 2.4 GHz network name and password.");
      if (!accessPoints.size) editing = true;
    }
  }
  async function read(current) {
    const { config } = await client.command({ name: "get_config" });
    if (!current() || !config) return;
    validateConfig(config); applyConfig(config);
    rememberNetwork(config.wifi_ssid, lookupNetwork(config.wifi_ssid)?.password ?? config.wifi_password, Object.fromEntries(localFields.map(name => [name, config[name]])));
  }
  $("usb_connect").addEventListener("click", () => run(async current => {
    saved = false; beforeConnect(); note("usb_status", "Choose your display’s USB port.");
    await client.connect(); if (!current()) return;
    manual = false; verified = null; editing = false;
    note("usb_status", "Display connected · APP MODE");
    await read(current); if (!current()) return;
    refresh();
    await scan(current);
  }));
  $("usb_disconnect").addEventListener("click", () => { generation++; void client.disconnect(); });
  $("usb_read").addEventListener("click", () => run(async current => { beforeConnect(); await read(current); if (current()) await scan(current); }));
  $("usb_scan").addEventListener("click", () => run(scan));
  $("wifi_networks").addEventListener("change", () => run(async current => {
    const ssid = $("wifi_networks").value;
    if (!ssid) {
      verified = null; editing = true; $("manual_wifi").hidden = false;
      $("wifi_ssid").value = ""; $("wifi_password").value = ""; applyLocal(null);
      note("wifi_status", "Enter your 2.4 GHz network name and password.");
      return;
    }
    await choose(ssid, current, accessPoints.get(ssid).secure);
  }));
  $("wifi_change").addEventListener("click", () => { verified = null; editing = true; note("wifi_status", "Enter a new password, then connect to check it."); refresh(); $("wifi_password").focus(); });
  for (const id of ["wifi_ssid", "wifi_password"]) $(id).addEventListener("input", () => { verified = null; if (client.connected) editing = true; refresh(); });
  $("wifi_ssid").addEventListener("change", () => {
    if (client.connected && $("wifi_ssid").value) void run(current => choose($("wifi_ssid").value, current));
  });
  $("wifi_step").addEventListener("keydown", event => {
    if (event.key === "Enter" && !event.isComposing && client.connected && event.target instanceof HTMLInputElement) {
      event.preventDefault();
      if (event.target.id === "wifi_ssid") void run(current => choose($("wifi_ssid").value, current));
      else void run(testWifi);
    }
  });
  $("usb_test").addEventListener("click", () => run(testWifi));
  $("usb_status_read").addEventListener("click", () => run(async current => {
    const result = await client.command({ name: "status" });
    if (current()) note("usb_status", `APP MODE · ${result.configured ? "settings saved" : "setup required"}${result.ip ? ` · IP ${result.ip}` : ""}`);
  }));
  $("usb_save").addEventListener("click", () => run(async current => {
    if (!matches()) throw new Error("Check the Wi-Fi connection before saving.");
    let config;
    try { config = validateConfig(readSaveConfig()); }
    catch (error) { $("unifi_editor").open = true; note("unifi_status", error.message, true); return; }
    note("usb_status", "Saving settings on your display…");
    await client.command({ name: "save_config", config }, 10000);
    if (!current()) return;
    rememberNetwork(config.wifi_ssid, lookupNetwork(config.wifi_ssid)?.password ?? config.wifi_password, Object.fromEntries(localFields.map(name => [name, config[name]])));
    saved = true; await client.disconnect(false);
    note("usb_status", "Settings saved. Your display is restarting.");
  }));
  $("manual_setup").addEventListener("click", () => { manual = true; editing = true; $("manual_wifi").hidden = false; refresh(); note("usb_status", "Prepare settings here, then install them with the firmware. A Wi-Fi test needs a connected display."); });
  window.addEventListener("pagehide", () => { generation++; verified = null; void client.disconnect(false); });
  refresh();
  return {
    connected: () => client.connected,
    firmwareReady: value => { ready = value; refresh(); },
    localChanged,
    reset: () => { generation++; verified = null; accessPoints.clear(); manual = false; saved = false; void client.disconnect(false); refresh(); },
  };
}
