// This key belongs to this installer; never clear other apps' origin storage.
export const SAVED_SETTINGS_KEY = "unifi-display.settings.v1";
export const savedFields = {
  wifi_ssid: 32, wifi_password: 64, api_key: 512, cloud_api_key: 512, controller_url: 256,
  certificate_sha256: 64, site_id: 36, device_id: 36, connection_mode: 5,
  tls_server_name: 253,
  cloud_console: 128, cloud_site: 36, cloud_device: 36,
};

export function readSavedSettings(storage) {
  let saved;
  try { saved = JSON.parse(storage.getItem(SAVED_SETTINGS_KEY)); }
  catch (error) { if (error instanceof SyntaxError) return null; throw error; }
  if (!saved || saved.version !== 1 || typeof saved.remember !== "boolean") return null;
  if (!saved.remember) return { remember: false };
  if (!saved.values || typeof saved.values !== "object" || Array.isArray(saved.values)) return null;
  const values = {};
  for (const [name, max] of Object.entries(savedFields)) {
    const value = name === "tls_server_name" && saved.values[name] === undefined ? "" : saved.values[name];
    if (typeof value !== "string" || value.length > max || value.includes("\0")) return null;
    values[name] = value;
  }
  if (!["cloud", "local"].includes(values.connection_mode)) return null;
  return { remember: true, values };
}

export function writeSavedSettings(storage, remember, values) {
  const saved = { version: 1, remember };
  if (remember) saved.values = Object.fromEntries(Object.keys(savedFields).map(name => [name, values[name]]));
  storage.setItem(SAVED_SETTINGS_KEY, JSON.stringify(saved));
}
