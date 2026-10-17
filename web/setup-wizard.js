import { validateConfig } from "./config.js";

const localFields = ["controller_url", "api_key", "certificate_sha256", "site_id", "device_id"];
export const HELPER_ORIGIN = "http://127.0.0.1:4177";

export function localSettings(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)
      || Object.keys(value).length !== localFields.length || localFields.some(name => typeof value[name] !== "string")) {
    throw new Error("Choose a local settings file downloaded by the setup helper.");
  }
  validateConfig({ wifi_ssid: "setup-validation", wifi_password: "", ...value });
  return Object.fromEntries(localFields.map(name => [name, value[name]]));
}

export function setupWizard(apply) {
  const $ = id => document.getElementById(id);
  let pending, generation = 0;
  function note(text, error = false) {
    $("wizard_status").textContent = text;
    $("wizard_status").classList.toggle("error", error);
    $("wizard_status").setAttribute("role", error ? "alert" : "status");
  }
  function cancel() { pending = undefined; generation++; }
  $("open_wizard").addEventListener("click", () => {
    cancel();
    const channel = [...crypto.getRandomValues(new Uint8Array(16))].map(byte => byte.toString(16).padStart(2, "0")).join("");
    const params = new URLSearchParams({ origin: location.origin, channel });
    const popup = window.open(`${HELPER_ORIGIN}/#${params}`, "_blank", "popup,width=660,height=840");
    if (!popup) { note("Allow the setup window to open, then try again.", true); return; }
    pending = { popup, channel, cloudKey: $("cloud_api_key").value };
    note("Continue in the local setup window. If it cannot connect, start the downloaded helper and open setup again.");
  });
  window.addEventListener("message", event => {
    if (!pending || event.origin !== HELPER_ORIGIN || event.source !== pending.popup || event.data?.channel !== pending.channel) return;
    if ($("cloud_api_key").value !== pending.cloudKey) {
      cancel(); note("Your cloud key changed. Open guided setup again.", true); return;
    }
    if (event.data.type === "unifi-setup-ready") {
      pending.popup.postMessage({ type: "unifi-setup-start", channel: pending.channel, cloud_key: pending.cloudKey }, HELPER_ORIGIN);
    } else if (event.data.type === "unifi-setup-result") {
      try {
        const config = localSettings(event.data.config);
        cancel(); apply(config);
        note("Local settings filled in. Add your Wi-Fi details, then prepare installation.");
      } catch (error) { note(error.message, true); }
    }
  });
  $("import_setup").addEventListener("change", async event => {
    const file = event.target.files?.[0];
    if (!file) return;
    cancel();
    const started = generation;
    try {
      if (file.size > 16384) throw new Error("This settings file is too large.");
      const config = localSettings(JSON.parse(await file.text()));
      if (generation !== started) return;
      apply(config);
      note("Local settings imported. Add your Wi-Fi details, then prepare installation.");
    } catch (error) { note(error instanceof SyntaxError ? "This is not a valid settings JSON file." : error.message, true); }
    event.target.value = "";
  });
  window.addEventListener("pagehide", cancel);
  return { cancel };
}
