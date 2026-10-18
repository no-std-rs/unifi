import { encodeConfig, manifestWithSettings } from "./config.js";
import { setupCloud } from "./cloud-setup.js";
import { SAVED_SETTINGS_KEY, savedFields, readSavedSettings, writeSavedSettings } from "./saved-settings.js";
import { setupDevice } from "./device-setup.js";
import { NETWORKS_KEY, localFields, localConfig, profile, readNetworks, writeNetworks } from "./network-profiles.js";
import { setupProvision } from "./local-provision.js";

const $ = id => document.getElementById(id);
const form = $("settings");
const status = $("status");
const install = $("installer");
const objectUrls = [];
let release;
let firmwareUrl;
let blankManifestUrl;
let networks = new Map();
let importGeneration = 0;

function message(text, error = false) {
  status.textContent = text;
  status.classList.toggle("error", error);
  status.setAttribute("role", error ? "alert" : "status");
}

function forgetSettings() {
  install.manifest = "";
  install.removeAttribute("manifest");
  $("install-area").hidden = true;
  for (const url of objectUrls.splice(0)) URL.revokeObjectURL(url);
}

function storageAction(action) {
  try { action(window.localStorage); return true; }
  catch {
    $("saved_status").textContent = "This browser cannot save settings. You can still install using the settings for this visit.";
    return false;
  }
}

function restoreSettings() {
  let saved;
  storageAction(storage => { saved = readSavedSettings(storage); });
  if (saved) {
    $("remember_settings").checked = saved.remember;
    if (saved.remember) {
      for (const [name, value] of Object.entries(saved.values)) {
        if (!["cloud_console", "cloud_site", "cloud_device"].includes(name)) $(name).value = value;
      }
      storageAction(storage => { networks = readNetworks(storage); });
      const values = saved.values;
      const known = networks.get(values.wifi_ssid);
      if (known) $("wifi_password").value = known.password;
      if (values.wifi_ssid && (!known || !known.config && values.connection_mode === "local")) {
        let config = null;
        if (values.connection_mode === "local") {
          try { config = localConfig(Object.fromEntries(localFields.map(name => [name, values[name]]))); } catch { /* Incomplete draft. */ }
        }
        try { networks.set(values.wifi_ssid, profile(values.wifi_ssid, known?.password ?? values.wifi_password, config)); } catch { /* Invalid old Wi-Fi draft. */ }
      }
      $("saved_status").textContent = "Saved settings restored from this browser." + (saved.values.connection_mode === "cloud" ? " Discover consoles and verify cloud access again before installing." : "");
    }
  }
  return saved?.values || {};
}

function saveSettings() {
  const remember = $("remember_settings").checked;
  const values = { ...Object.fromEntries(Object.keys(savedFields).filter(name => !["cloud_console", "cloud_site", "cloud_device"].includes(name)).map(name => [name, $(name).value])), ...cloud.selection() };
  if (storageAction(storage => { writeSavedSettings(storage, remember, values); writeNetworks(storage, remember, networks); })) {
    $("saved_status").textContent = remember ? "Settings saved in this browser, including your API key and Wi-Fi password." : "Remembering is off. Your credentials have been removed from browser storage.";
  }
}

const restored = restoreSettings();
let connectionDraft;
const cloud = setupCloud(forgetSettings, restored, saveSettings, () => device.connected());
const readForm = () => Object.fromEntries(["wifi_ssid", "wifi_password", "controller_url", "api_key", "certificate_sha256", "site_id", "device_id"].map(name => [name, $(name).value]));
const readSaveConfig = () => ({ ...readForm(), ...cloud.config() });
const device = setupDevice({
  readForm,
  readSaveConfig,
  applyConfig: config => {
    connectionDraft = undefined;
    importGeneration++; forgetSettings(); $("connection_mode").value = "local"; cloud.reset();
    for (const [name, value] of Object.entries(config)) $(name).value = value;
    saveSettings();
  },
  lookupNetwork: ssid => {
    if (connectionDraft?.wifi_ssid === ssid) return { password: connectionDraft.wifi_password, config: null };
    return networks.get(ssid);
  },
  rememberNetwork: (ssid, password, config) => {
    const previous = networks.get(ssid);
    if (connectionDraft?.wifi_ssid === ssid) connectionDraft.wifi_password = password;
    networks.delete(ssid); networks.set(ssid, profile(ssid, password, config || previous?.config));
    if (networks.size > 16) networks.delete(networks.keys().next().value);
    saveSettings();
  },
  applyLocal: (config, { offline = false } = {}) => {
    importGeneration++; forgetSettings();
    const draft = connectionDraft?.wifi_ssid === $("wifi_ssid").value ? connectionDraft : null;
    const sameCloud = draft?.connection_mode === "cloud" && $("connection_mode").value === "cloud"
      && ["cloud_console", "cloud_site", "cloud_device"].every(name => cloud.selection()[name] === draft[name]);
    $("connection_mode").value = draft?.connection_mode || (config ? "local" : offline ? $("connection_mode").value : "cloud");
    if (!sameCloud) cloud.reset(draft || {});
    for (const name of localFields) $(name).value = (draft ? draft[name] : config?.[name]) ?? (name === "controller_url" ? "https://unifi.local/proxy/network/integration" : "");
    saveSettings();
  },
  beforeManual: () => {
    importGeneration++; connectionDraft = undefined; forgetSettings();
    const known = networks.get($("wifi_ssid").value);
    if (known) $("wifi_password").value = known.password;
  },
  beforeConnect: () => {
    importGeneration++; forgetSettings();
    connectionDraft = { ...readForm(), connection_mode: $("connection_mode").value, ...cloud.selection() };
  },
});

setupProvision({ device, cloud, wifi: readForm, invalidateKey: key => {
  for (const [ssid, known] of networks) {
    if (known.config?.api_key === key) networks.set(ssid, profile(ssid, known.password, null));
  }
  if (connectionDraft?.api_key === key) connectionDraft.api_key = "";
}, apply: config => {
  connectionDraft = undefined; importGeneration++; forgetSettings();
  for (const name of localFields) $(name).value = config[name];
  const values = readForm();
  networks.delete(values.wifi_ssid);
  networks.set(values.wifi_ssid, profile(values.wifi_ssid, values.wifi_password, config));
  if (networks.size > 16) networks.delete(networks.keys().next().value);
  saveSettings(); device.localChanged();
} });

$("forget_saved").addEventListener("click", () => {
  importGeneration++;
  networks.clear();
  connectionDraft = undefined;
  device.reset();
  form.reset();
  $("remember_settings").checked = false;
  cloud.reset();
  // Keep only the preference to avoid saving credentials again on reopening.
  if (storageAction(storage => {
    storage.removeItem(SAVED_SETTINGS_KEY);
    storage.removeItem(NETWORKS_KEY);
    writeSavedSettings(storage, false);
  })) $("saved_status").textContent = "Saved settings forgotten. Remembering is off.";
  if (release) message("Settings cleared. Enter your Wi-Fi and UniFi settings to prepare another installation.");
});

$("import_setup").addEventListener("change", async event => {
  const file = event.target.files?.[0]; if (!file) return;
  const generation = ++importGeneration;
  try {
    if (file.size > 16384) throw new Error("The settings file is too large.");
    const config = localConfig(JSON.parse(await file.text()));
    if (generation !== importGeneration) return;
    connectionDraft = undefined;
    forgetSettings(); $("connection_mode").value = "local"; cloud.reset();
    for (const [name, value] of Object.entries(config)) $(name).value = value;
    saveSettings(); device.localChanged();
    $("import_status").textContent = "Local connection imported.";
  } catch (error) { if (generation === importGeneration) $("import_status").textContent = error.message; }
  event.target.value = "";
});

form.addEventListener("input", () => {
  forgetSettings();
  saveSettings();
  device.localChanged();
  if (release) message(device.connected() ? "" : "Your settings stay here. Prepare when you're ready.");
});
form.addEventListener("change", saveSettings);
form.addEventListener("submit", event => {
  event.preventDefault();
  if (device.connected()) { message("Use Save and restart display to apply these settings.", true); return; }
  forgetSettings();
  saveSettings();
  try {
    if (!release || !firmwareUrl) throw new Error("Wait for the latest firmware to finish loading.");
    const config = readSaveConfig();
    const sector = encodeConfig(config);
    const settingsUrl = URL.createObjectURL(new Blob([sector], { type: "application/octet-stream" }));
    objectUrls.push(settingsUrl);
    const manifest = manifestWithSettings(release, firmwareUrl, settingsUrl);
    const manifestUrl = URL.createObjectURL(new Blob([JSON.stringify(manifest)], { type: "application/json" }));
    objectUrls.push(manifestUrl);
    install.manifest = manifestUrl;
    $("install-area").hidden = false;
    message("Ready. Connect your display by USB, then choose Install on display.");
    $("install-area").scrollIntoView({ behavior: "smooth", block: "nearest" });
  } catch (error) { message(error.message, true); }
});

window.addEventListener("pagehide", () => {
  forgetSettings();
  if (blankManifestUrl) URL.revokeObjectURL(blankManifestUrl);
  blankManifestUrl = undefined;
  if (firmwareUrl) URL.revokeObjectURL(firmwareUrl);
  firmwareUrl = undefined;
  release = undefined;
  $("download").hidden = true;
  $("download").removeAttribute("href");
  device.firmwareReady(false);
  $("blank_installer").removeAttribute("manifest");
  $("blank_install_area").hidden = true;
  importGeneration++; networks.clear(); connectionDraft = undefined; form.reset();
  cloud.reset();
});
window.addEventListener("pageshow", event => {
  if (event.persisted) {
    device.reset(); const values = restoreSettings();
    connectionDraft = undefined; cloud.reset(values);
    loadRelease().catch(error => message(error.message, true));
  }
});

async function loadRelease() {
  const response = await fetch("./latest.json", { cache: "no-store", credentials: "omit" });
  if (!response.ok) throw new Error("The latest main build is not available yet. Try again shortly.");
  const candidate = await response.json();
  manifestWithSettings(candidate, "blob:firmware", "blob:settings");
  const binaryResponse = await fetch("./unifi-display.bin", { cache: "no-store", credentials: "omit" });
  if (!binaryResponse.ok) throw new Error("The firmware download failed. Please reload to retry.");
  const buffer = await binaryResponse.arrayBuffer();
  if (buffer.byteLength !== candidate.size) throw new Error("The download size changed. Reload for the newest build.");
  const digest = [...new Uint8Array(await crypto.subtle.digest("SHA-256", buffer))].map(x => x.toString(16).padStart(2, "0")).join("");
  if (digest !== candidate.sha256) throw new Error("The download checksum did not match. Reload to retry.");
  firmwareUrl = URL.createObjectURL(new Blob([buffer], { type: "application/octet-stream" }));
  release = candidate;
  $("revision").textContent = `main · ${release.revision.slice(0, 12)}`;
  $("revision").href = `https://github.com/no-std-rs/unifi/commit/${release.revision}`;
  $("digest").textContent = release.sha256;
  $("download").href = firmwareUrl;
  $("download").hidden = false;
  device.firmwareReady(!!navigator.serial && window.isSecureContext);
  const blank = manifestWithSettings(release, firmwareUrl, "unused");
  blank.builds[0].parts.pop();
  const blankUrl = URL.createObjectURL(new Blob([JSON.stringify(blank)], { type: "application/json" }));
  // Keep this firmware-only manifest separate from short-lived credential blobs.
  if (blankManifestUrl) URL.revokeObjectURL(blankManifestUrl);
  blankManifestUrl = blankUrl;
  $("blank_installer").manifest = blankUrl;
  $("blank_install_area").hidden = false;
  if (!navigator.serial || !window.isSecureContext) {
    $("prepare").disabled = true;
    message("USB flashing needs a desktop browser with Web Serial, such as Chrome or Edge. You can still download the binary.");
  } else { message(""); }
}

loadRelease().catch(error => message(error.message, true));
