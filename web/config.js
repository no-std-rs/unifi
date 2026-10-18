// Matches examples/esp32c3-unifi-display/src/config.rs and partitions.csv.
export const CONFIG_OFFSET = 0x3f0000;
export const SECTOR_SIZE = 4096;
const encoder = new TextEncoder();
const fields = ["wifi_ssid", "wifi_password", "controller_url", "api_key", "certificate_sha256", "site_id", "device_id"];
const length = value => encoder.encode(value).length;
export function validateWifi(ssid, password) {
  if (typeof ssid !== "string" || typeof password !== "string" || ssid.includes("\0") || password.includes("\0")) throw new Error("Enter a valid Wi-Fi name and password.");
  if (length(ssid) < 1 || length(ssid) > 32) throw new Error("The Wi-Fi name must be 1–32 UTF-8 bytes.");
  if (password !== "" && !(length(password) >= 8 && length(password) <= 63) && !/^[\da-fA-F]{64}$/.test(password)) throw new Error("Use an 8–63 byte Wi-Fi password, or leave it empty for an open network.");
}

export function isLocalIPv4(value) {
  const parts = value.split(".");
  if (parts.length !== 4 || parts.some(p => !/^(0|[1-9]\d{0,2})$/.test(p) || Number(p) > 255)) return false;
  const [a, b] = parts.map(Number);
  return a === 10 || a === 172 && b >= 16 && b <= 31 || a === 192 && b === 168
    || a === 169 && b === 254 || a === 100 && b >= 64 && b <= 127;
}
export function validateConfig(config) {
  if (Object.keys(config).some(k => ![...fields, "tls_server_name"].includes(k)) || fields.some(k => typeof config[k] !== "string" || config[k].includes("\0"))) throw new Error("Complete the Wi-Fi and UniFi settings.");
  if (config.tls_server_name !== undefined && (typeof config.tls_server_name !== "string" || config.tls_server_name.length > 253
      || config.tls_server_name !== "" && config.tls_server_name.split(".").some(label => !label || label.length > 63 || !/^[a-zA-Z0-9](?:[a-zA-Z0-9-]*[a-zA-Z0-9])?$/.test(label)))) throw new Error("Enter a valid TLS certificate name, or leave it empty.");
  validateWifi(config.wifi_ssid, config.wifi_password);
  const match = /^https:\/\/([^/]+)(.*)$/.exec(config.controller_url);
  const authority = match?.[1] || "";
  const parts = authority.split(":");
  const host = parts[0];
  const port = parts[1];
  if (!match || /[^\x21-\x7e]/.test(config.controller_url) || length(config.controller_url)>256 || parts.length>2
      || !host || host.length>253 || host.split(".").some(label=>!label || label.length>63 || !/^[a-zA-Z0-9](?:[a-zA-Z0-9-]*[a-zA-Z0-9])?$/.test(label))
      || port !== undefined && (!/^\d+$/.test(port) || Number(port)<1 || Number(port)>65535)
      || /[?#]/.test(match?.[2] || "") || (match?.[2] || "").split("/").some(p=>p==="." || p==="..")) throw new Error("Use an https:// controller URL and integration base, with a DNS name or IPv4 address.");
  if (host.toLowerCase() === "api.ui.com" || (match?.[2] || "").includes("/v1/connector/")
      || /^\d+\.\d+\.\d+\.\d+$/.test(host) && !isLocalIPv4(host)) throw new Error("The display uses direct LAN access only. Enter your local controller's HTTPS URL, not a cloud Connector or public IP address.");
  if (!/^[\x21-\x7e]{1,512}$/.test(config.api_key)) throw new Error("Enter the UniFi API key (up to 512 ASCII bytes).");
  if (!/^[a-fA-F0-9]{64}$/.test(config.certificate_sha256)) throw new Error("Enter the trusted controller certificate's SHA-256 fingerprint as 64 hexadecimal characters.");
  for (const name of ["site_id","device_id"]) if (config[name] !== "" && !/^[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}$/.test(config[name])) throw new Error("Site and device IDs must be UUIDs, or empty for an unambiguous single site/device.");
  return config;
}

export function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return (crc ^ 0xffffffff) >>> 0;
}

export function encodeConfig(config) {
  validateConfig(config);
  const saved = { ...config };
  if (!saved.tls_server_name) delete saved.tls_server_name;
  const payload = encoder.encode(JSON.stringify(saved));
  if (payload.length > 2048) throw new Error("The settings exceed the device's configuration limit.");
  const sector = new Uint8Array(SECTOR_SIZE).fill(0xff);
  sector.set(encoder.encode("UNIFICF1"));
  const view = new DataView(sector.buffer);
  view.setUint32(8, payload.length, true);
  view.setUint32(12, crc32(payload), true);
  sector.set(payload, 16);
  return sector;
}

export function manifestWithSettings(release, firmwareUrl, settingsUrl) {
  if (release.application !== "unifi-display" || release.settings_schema !== "UNIFICF1" || release.dirty !== false || release.chip !== "ESP32-C3" || release.config_offset !== CONFIG_OFFSET || release.flash_size !== 4194304
      || !/^[0-9a-f]{40}$/.test(release.revision) || !/^[0-9a-f]{64}$/.test(release.sha256)
      || !Number.isSafeInteger(release.size) || release.size < 65536 || release.size >= CONFIG_OFFSET) throw new Error("The firmware metadata does not match this board.");
  return {
    name: "UniFi display", version: `main-${release.revision.slice(0, 12)}`,
    new_install_prompt_erase: true, new_install_improv_wait_time: 0,
    builds: [{ chipFamily: "ESP32-C3", parts: [{ path: firmwareUrl, offset: 0 }, { path: settingsUrl, offset: CONFIG_OFFSET }] }],
  };
}
