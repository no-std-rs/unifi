import { cloudBase, cloudClient, consoleLanUrl, rootGateway, consoleTarget } from "./cloud.js";

export function setupCloud(invalidate, initialSelection = {}, selectionChanged = () => {}, autoVerify = () => false) {
  const $ = id => document.getElementById(id);
  const mode = $("connection_mode"), key = $("cloud_api_key");
  const consoleSelect = $("cloud_console"), siteSelect = $("cloud_site"), deviceSelect = $("cloud_device");
  let controller = new AbortController(), verified = false, consoles = [], discovering = false, attemptedKey = "";
  let remembered = { ...initialSelection };
  let previousConsole = initialSelection.cloud_console || "";
  const isCloud = () => mode.value === "cloud";
  function note(message, error = false) {
    $("cloud_status").textContent = message;
    $("cloud_status").classList.toggle("error", error);
    $("cloud_status").setAttribute("role", error ? "alert" : "status");
  }
  function cancel() {
    controller.abort(); controller = new AbortController(); verified = false; discovering = false;
    $("verify_cloud").disabled = !deviceSelect.value;
    invalidate();
    window.dispatchEvent(new Event("cloud-selection"));
    return controller.signal;
  }
  function clear(select) { select.replaceChildren(); select.disabled = true; }
  function choices(select, rows, placeholder, label, preferred = "") {
    select.replaceChildren(new Option(placeholder, ""), ...rows.map(row => new Option(label(row), row.id)));
    select.disabled = false;
    if (rows.some(row => row.id === remembered[select.id])) select.value = remembered[select.id];
    else if (preferred && rows.some(row => row.id === preferred)) select.value = preferred;
    else if (rows.length === 1) select.value = rows[0].id;
    remembered[select.id] = select.value;
    selectionChanged();
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
    const gateway = rootGateway(consoles.find(row => row.id === consoleSelect.value), rows);
    choices(deviceSelect, rows, "Choose a device…", row => `${row.name || row.model || row.id}${row.id === gateway ? " · root gateway" : ""}${row.state ? ` · ${row.state}` : ""}`, gateway);
    $("verify_cloud").disabled = !deviceSelect.value;
    note(rows.length ? "Choose the device to display, then verify access." : "No adopted devices found in this site.");
    if (deviceSelect.value && autoVerify()) $("verify_cloud").click();
  }
  async function loadSites(signal) {
    if (!consoleSelect.value) return;
    const url = consoleLanUrl(consoles.find(row => row.id === consoleSelect.value));
    if (previousConsole !== consoleSelect.value) {
      if (previousConsole || !$("api_key").value) {
        if (previousConsole || url !== $("controller_url").value) {
          $("api_key").value = "";
          $("certificate_sha256").value = "";
          $("tls_server_name").value = "";
        }
        $("controller_url").value = url;
      }
      previousConsole = consoleSelect.value;
      selectionChanged();
    }
    $("lan_address_note").textContent = url ? "LAN address discovered from the console. The display connects here directly." : "The console did not report a unique LAN IPv4 address. Enter its reachable local address below.";
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
    attemptedKey = "";
    resetSelections();
    $("cloud_discovery").open = true;
    $("cloud_discovery_summary").textContent = "Discover your gateway";
    $("cloud_setup").hidden = !isCloud();
    key.disabled = !isCloud();
    note("Discover consoles with your cloud key. The device will use a separate local Network key.");
  }
  mode.addEventListener("change", () => { remembered = {}; updateMode(); });
  key.addEventListener("input", () => { remembered = {}; resetSelections(); note("Key changed. Discover consoles again."); });
  $("discover_cloud").addEventListener("click", () => {
    resetSelections();
    const signal = controller.signal;
    discovering = true; attemptedKey = key.value;
    run(signal, async () => {
      try {
        note("Discovering online consoles…");
        const rows = await client(signal).consoles();
        if (signal.aborted) return;
        consoles = rows;
        choices(consoleSelect, rows, "Choose a console…", row => row.reportedState?.name || row.reportedState?.hostname || row.id);
        $("cloud_choices").hidden = false;
        note(rows.length ? "Choose a console." : "No online consoles found. Check remote management and your account's access.");
        await loadSites(signal);
      }
      finally { if (!signal.aborted) discovering = false; }
    });
  });
  key.addEventListener("keydown", event => {
    if (event.key === "Enter" && !event.isComposing) { event.preventDefault(); $("discover_cloud").click(); }
  });
  consoleSelect.addEventListener("change", () => {
    remembered = { cloud_console: consoleSelect.value };
    clear(siteSelect); clear(deviceSelect);
    const signal = cancel(); run(signal, () => loadSites(signal));
  });
  siteSelect.addEventListener("change", () => {
    remembered = { cloud_console: consoleSelect.value, cloud_site: siteSelect.value };
    clear(deviceSelect);
    const signal = cancel(); run(signal, () => loadDevices(signal));
  });
  deviceSelect.addEventListener("change", () => { remembered.cloud_device = deviceSelect.value; cancel(); note("Verify access for this device before preparing installation."); if (deviceSelect.value && autoVerify()) $("verify_cloud").click(); });
  $("verify_cloud").addEventListener("click", () => {
    const signal = cancel();
    $("verify_cloud").disabled = true;
    run(signal, async () => {
      try {
        note("Checking device details, statistics, and site counts…");
        await client(signal).verify(cloudBase(consoleSelect.value), siteSelect.value, deviceSelect.value);
        if (signal.aborted) return;
        verified = true;
        $("site_id").value = siteSelect.value;
        $("device_id").value = deviceSelect.value;
        selectionChanged();
        note("Cloud reads verified. Local setup uses your connected display.");
        $("cloud_discovery_summary").textContent = `${consoleSelect.selectedOptions[0]?.textContent} · ${deviceSelect.selectedOptions[0]?.textContent}`;
        if (autoVerify()) $("cloud_discovery").open = false;
        window.dispatchEvent(new Event("cloud-verified"));
      } finally { if (!signal.aborted) $("verify_cloud").disabled = !deviceSelect.value; }
    });
  });
  updateMode();
  $("discover_cloud").disabled = false;
  return {
    verified: () => isCloud() && verified,
    resume() {
      if (isCloud() && !verified && !discovering && key.value && key.value !== attemptedKey) $("discover_cloud").click();
    },
    target() {
      if (!isCloud() || !verified) return null;
      return { ...consoleTarget(consoles.find(row => row.id === consoleSelect.value)), site_id: siteSelect.value, device_id: deviceSelect.value };
    },
    config() {
      if (!isCloud()) return {};
      if (!verified) throw new Error("Verify cloud access before preparing installation.");
      return { site_id: siteSelect.value, device_id: deviceSelect.value };
    },
    selection() {
      return Object.fromEntries([consoleSelect, siteSelect, deviceSelect].map(select => [select.id, select.value || remembered[select.id] || ""]));
    },
    reset(selection = {}) {
      remembered = { ...selection };
      previousConsole = selection.cloud_console || "";
      updateMode();
    },
  };
}
