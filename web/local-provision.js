import { localConfig } from "./network-profiles.js";

export function setupProvision({ device, cloud, wifi, apply }) {
  const $ = id => document.getElementById(id);
  let generation = 0, busy = false, target, login, phase = "inspect", timer, requested = false, layout;
  const fields = ["console_username", "console_password", "console_token"];
  const note = (text, error = false) => { $("provision_status").textContent = text; $("provision_status").classList.toggle("error", error); };
  function clear() {
    generation++; clearTimeout(timer); target = undefined; login = undefined; phase = "inspect"; requested = false;
    for (const id of fields) $(id).value = "";
    $("console_auth").hidden = true; $("console_mfa").hidden = true;
    $("console_credentials").hidden = false; $("provision_retry").hidden = true;
    refresh();
  }
  function refresh() {
    $("automatic_local").hidden = $("connection_mode").value !== "cloud";
    $("console_signin").disabled = busy || !device.consoleReady();
    $("console_mfa_send").disabled = busy;
    $("provision_retry").disabled = busy || !device.consoleReady();
    $("provision_update").hidden = !device.connected() || device.supportsConsole();
    $("provision_update").disabled = busy;
    const nextLayout = $("connection_mode").value + device.connected();
    if (nextLayout !== layout) { layout = nextLayout; $("local_settings").open = $("connection_mode").value === "local" || !device.connected(); }
  }
  async function run(action) {
    if (busy) return;
    const started = generation; busy = true; refresh();
    const current = () => started === generation && device.consoleReady();
    try { await action(current); }
    catch (error) { if (started === generation) { if (phase === "login") login = undefined; note(error.message, true); $("provision_retry").hidden = false; } }
    finally { busy = false; refresh(); if (requested && device.consoleReady()) { requested = false; void run(inspect); } }
  }
  async function command(name, values = {}, timeout) { return device.consoleCommand({ name, ...values }, timeout); }
  async function finish(current, localKey = "") {
    phase = "verify"; note(localKey ? "Checking your existing local key on the display…" : "Creating and checking your local display key…");
    const result = await command("console_provision", { site_id: target.site_id, device_id: target.device_id, local_key: localKey }, 150000);
    if (!current()) return;
    const config = localConfig(result?.config);
    if (result.state !== "verified" || config.site_id !== target.site_id || config.device_id !== target.device_id
        || config.controller_url !== `https://${target.hostname}/proxy/network/integration`) throw new Error("The display returned settings for a different console. Discover again.");
    login = undefined; for (const id of fields) $(id).value = "";
    $("console_auth").hidden = true; $("provision_retry").hidden = true;
    apply(config); phase = "done";
    note("Local access ready. Key and certificate verified by your display. Save below to finish.");
  }
  async function signedIn(result, current) {
    if (!current()) return;
    if (result?.state === "authenticated") { login = undefined; await finish(current); return; }
    if (result?.state !== "mfa" || !Array.isArray(result.methods) || !result.methods.length) throw new Error("The console did not return a supported sign-in result.");
    phase = "mfa"; $("console_credentials").hidden = true; $("console_mfa").hidden = false;
    $("console_method").replaceChildren(...result.methods.map(m => new Option(({push:"Approve in UniFi Verify",totp:"Authenticator code",email:"Email code",sms:"SMS code",backup:"Backup code"})[m.type] || m.type, m.id)));
    for (const [index, method] of result.methods.entries()) $("console_method").options[index].dataset.type = method.type;
    $("console_signin").textContent = "Verify code & create local access";
    note("Complete the console's two-factor sign-in.");
  }
  async function inspect(current) {
    target = cloud.target();
    if (!target) throw new Error("Discover your console first.");
    phase = "inspect"; note("Your display is verifying the gateway certificate over Wi-Fi…");
    let key = "";
    try {
      const configured = new URL($("controller_url").value);
      if (configured.protocol === "https:" && [target.address, target.hostname].includes(configured.hostname)) key = $("api_key").value;
    } catch { /* An unrelated manual draft is not a key for this console. */ }
    const values = wifi();
    const result = await command("console_begin", { ssid: values.wifi_ssid, password: values.wifi_password,
      address: target.address, hostname: target.hostname, now: Math.floor(Date.now() / 1000) }, 120000);
    if (!current()) return;
    if (result?.hostname !== target.hostname || !/^[a-f0-9]{64}$/i.test(result.certificate_sha256 || "")) throw new Error("The display could not verify this console's certificate.");
    $("certificate_sha256").value = result.certificate_sha256;
    $("controller_url").value = `https://${target.hostname}/proxy/network/integration`;
    $("api_key").value = key;
    $("console_auth").hidden = false; $("console_credentials").hidden = false;
    $("console_signin").textContent = "Sign in & create local access";
    phase = "login";
    if (key) {
      try { await finish(current, key); return; }
      catch (error) { if (!current()) return; phase = "login"; note(`${error.message}. Sign in below to create a replacement local key.`, true); }
    } else note("Certificate verified automatically. Sign in once to create the display's local key.");
    $("console_username").focus();
  }
  $("console_signin").addEventListener("click", () => run(async current => {
    if (!target) { await inspect(current); return; }
    if (phase === "verify") { await finish(current); return; }
    if (!login) {
      login = { username: $("console_username").value, password: $("console_password").value };
      if (!login.username || !login.password) { login = undefined; throw new Error("Enter your console username and password."); }
      $("console_password").value = "";
    }
    note("Signing in to your console through the display…");
    const result = await command("console_login", { ...login, token: $("console_token").value });
    if (!current()) return;
    await signedIn(result, current);
  }));
  $("console_auth").addEventListener("keydown", event => {
    if (event.key === "Enter" && !event.isComposing && event.target instanceof HTMLInputElement) { event.preventDefault(); $("console_signin").click(); }
  });
  function poll(started) {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      if (started !== generation || !device.consoleReady() || phase !== "mfa") return;
      if (busy) { poll(started); return; }
      await run(async current => {
        const result = await command("console_poll");
        if (!current()) return;
        if (result?.state === "waiting") { poll(started); return; }
        await signedIn(result, current);
      });
    }, 2000);
  }
  $("console_mfa_send").addEventListener("click", () => run(async current => {
    const option = $("console_method").selectedOptions[0];
    if (!option || !["push", "email", "sms"].includes(option.dataset.type)) { note("Enter the code from your authenticator."); return; }
    await command("console_mfa", { id: option.value }); if (!current()) return;
    note(option.dataset.type === "push" ? "Approve sign-in in UniFi Verify. This page will continue automatically." : "Enter the code sent by your console.");
    if (option.dataset.type === "push") poll(generation);
  }));
  $("provision_retry").addEventListener("click", () => run(async current => { if (phase === "verify" && target) await finish(current); else await inspect(current); }));
  $("provision_update").addEventListener("click", () => { clear(); void device.updateFirmware(); });
  window.addEventListener("cloud-selection", () => { clear(); note("Discover your gateway to set up its local key and certificate automatically."); });
  window.addEventListener("cloud-verified", () => {
    clear();
    if (device.consoleReady()) { if (busy) requested = true; else void run(inspect); }
    else { note(device.connected() ? "Install the latest firmware to create local access over USB." : "Connect your display to set up its local key and certificate automatically."); $("provision_retry").hidden = false; }
  });
  window.addEventListener("display-state", () => { if (!device.connected()) clear(); else refresh(); });
  window.addEventListener("pagehide", clear);
  refresh();
  return { reset: clear };
}
