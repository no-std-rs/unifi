import { CLOUD_ORIGIN, cloudBase, cloudClient, cloudTrust } from "./cloud.js";

export function setupCloud(invalidate) {
  const $ = id => document.getElementById(id);
  const mode = $("connection_mode"), key = $("api_key");
  const consoleSelect = $("cloud_console"), siteSelect = $("cloud_site"), deviceSelect = $("cloud_device");
  let trust, controller = new AbortController(), verified = false;
  const isCloud = () => mode.value === "cloud";
  function note(message, error = false) {
    $("cloud_status").textContent = message;
    $("cloud_status").classList.toggle("error", error);
    $("cloud_status").setAttribute("role", error ? "alert" : "status");
  }
  function cancel() {
    controller.abort(); controller = new AbortController(); verified = false;
    $("verify_cloud").disabled = !deviceSelect.value;
    invalidate();
    return controller.signal;
  }
  function clear(select) { select.replaceChildren(); select.disabled = true; }
  function choices(select, rows, placeholder, label) {
    select.replaceChildren(new Option(placeholder, ""), ...rows.map(row => new Option(label(row), row.id)));
    select.disabled = false;
    if (rows.length === 1) select.value = rows[0].id;
  }
  function client(signal) { return cloudClient(key.value, signal); }
  function run(signal, action) {
    action().catch(error => {
      if (!signal.aborted) note(error instanceof TypeError ? "Could not reach the UniFi cloud API. Check your internet connection and retry." : error.message, true);
    });
  }
  async function loadDevices(signal) {
    if (!siteSelect.value) return;
    const rows = await client(signal).devices(cloudBase(consoleSelect.value), siteSelect.value);
    if (signal.aborted) return;
    choices(deviceSelect, rows, "Choose a device…", row => `${row.name || row.model || row.id}${row.state ? ` · ${row.state}` : ""}`);
    $("verify_cloud").disabled = !deviceSelect.value;
    note(rows.length ? "Choose the device to display, then verify access." : "No adopted devices found in this site.");
  }
  async function loadSites(signal) {
    if (!consoleSelect.value) return;
    const rows = await client(signal).sites(cloudBase(consoleSelect.value));
    if (signal.aborted) return;
    choices(siteSelect, rows, "Choose a site…", row => row.name || row.id);
    note(rows.length ? "Choose a site." : "No Network sites found on this console.");
    await loadDevices(signal);
  }
  function resetSelections() {
    clear(consoleSelect); clear(siteSelect); clear(deviceSelect); cancel();
    $("cloud_choices").hidden = true;
  }
  function updateMode() {
    resetSelections();
    $("cloud_setup").hidden = !isCloud();
    $("local_settings").hidden = isCloud();
    for (const input of $("local_settings").querySelectorAll("input")) input.disabled = isCloud();
    $("key_help").textContent = isCloud()
      ? "Create this key in unifi.ui.com → Settings → API Keys. Discovery sends it directly to api.ui.com; our site has no backend."
      : "Create a local key in your Network application's Integrations settings. A Site Manager cloud key uses the cloud connection option.";
    note(trust ? "Enter your cloud key, then discover consoles." : "Loading verified cloud certificate information…");
  }
  mode.addEventListener("change", updateMode);
  key.addEventListener("input", () => { resetSelections(); note("Key changed. Discover consoles again."); });
  $("discover_cloud").addEventListener("click", () => {
    resetSelections();
    const signal = controller.signal;
    run(signal, async () => {
      cloudTrust(trust);
      note("Discovering online consoles…");
      const rows = await client(signal).consoles();
      if (signal.aborted) return;
      choices(consoleSelect, rows, "Choose a console…", row => row.reportedState?.name || row.reportedState?.hostname || row.id);
      $("cloud_choices").hidden = false;
      note(rows.length ? "Choose a console." : "No online consoles found. Check remote management and your account's access.");
      await loadSites(signal);
    });
  });
  consoleSelect.addEventListener("change", () => {
    clear(siteSelect); clear(deviceSelect);
    const signal = cancel(); run(signal, () => loadSites(signal));
  });
  siteSelect.addEventListener("change", () => {
    clear(deviceSelect);
    const signal = cancel(); run(signal, () => loadDevices(signal));
  });
  deviceSelect.addEventListener("change", () => { cancel(); note("Verify access for this device before preparing installation."); });
  $("verify_cloud").addEventListener("click", () => {
    const signal = cancel();
    $("verify_cloud").disabled = true;
    run(signal, async () => {
      try {
        cloudTrust(trust);
        note("Checking device details, statistics, and site counts…");
        await client(signal).verify(cloudBase(consoleSelect.value), siteSelect.value, deviceSelect.value);
        if (signal.aborted) return;
        verified = true;
        note("Cloud access verified for all dashboard reads. Ready to prepare installation.");
      } finally { if (!signal.aborted) $("verify_cloud").disabled = !deviceSelect.value; }
    });
  });
  updateMode();
  fetch("./cloud.json", { cache: "no-store", credentials: "omit" }).then(async response => {
    if (!response.ok) throw new Error("Cloud certificate information is unavailable. Use local setup or retry after the next site build.");
    const metadata = await response.json();
    cloudTrust(metadata); trust = metadata;
    $("discover_cloud").disabled = false;
    $("cloud_certificate").textContent = `Certificate verified by the site build; valid until ${new Date(trust.not_after).toLocaleDateString()}. If UniFi rotates it, update the display's settings using a refreshed site build.`;
    note("Enter your cloud key, then discover consoles.");
  }).catch(error => note(error.message, true));
  return {
    config() {
      if (!isCloud()) return {};
      if (!verified) throw new Error("Verify cloud access before preparing installation.");
      return { controller_url: CLOUD_ORIGIN + cloudBase(consoleSelect.value), certificate_sha256: cloudTrust(trust), site_id: siteSelect.value, device_id: deviceSelect.value };
    },
    reset() { resetSelections(); updateMode(); },
  };
}
