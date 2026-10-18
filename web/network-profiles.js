import { validateConfig, validateWifi } from "./config.js";

export const NETWORKS_KEY = "unifi-display.networks.v1";
export const localFields = ["controller_url", "api_key", "certificate_sha256", "site_id", "device_id", "tls_server_name"];
export function localConfig(value) {
  if (!value || typeof value !== "object" || Array.isArray(value) || Object.keys(value).some(name => !localFields.includes(name))) throw new Error("Choose a local UniFi settings JSON file.");
  const config = Object.fromEntries(localFields.filter(name => name !== "tls_server_name" || value[name] !== undefined && value[name] !== "").map(name => [name, value[name]]));
  validateConfig({ wifi_ssid: "validation", wifi_password: "", ...config });
  return config;
}
export function profile(ssid, password, config = null) {
  validateWifi(ssid, password);
  return { ssid, password, config: config ? localConfig(config) : null };
}
export function readNetworks(storage) {
  const raw = storage.getItem(NETWORKS_KEY);
  if (!raw || raw.length > 65536) return new Map();
  try {
    const saved = JSON.parse(raw);
    if (saved.version !== 1 || !Array.isArray(saved.networks) || saved.networks.length > 16) return new Map();
    return new Map(saved.networks.map(p => [p.ssid, profile(p.ssid, p.password, p.config)]));
  } catch { return new Map(); }
}
export function writeNetworks(storage, remember, networks) {
  if (!remember) { storage.removeItem(NETWORKS_KEY); return; }
  storage.setItem(NETWORKS_KEY, JSON.stringify({ version: 1, networks: [...networks.values()].slice(-16) }));
}
