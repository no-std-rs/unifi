import { localConfig } from "./network-profiles.js";

export function setupProvision({ device, cloud, wifi, apply, invalidateKey }) {
  const $ = id => document.getElementById(id);
  let generation = 0, busy = false, target, login, phase = "inspect", timer, requested = false, layout;
  let approvalSent = false;
  const fields = ["console_username", "console_password", "console_token"];
  const note = (text, error = false) => { $("provision_status").textContent = text; $("provision_status").classList.toggle("error", error); };
  function clear() {
    generation++; clearTimeout(timer); target = undefined; login = undefined; phase = "inspect"; requested = false; approvalSent = false;
    for (const id of fields) $(id).value = "";
    $("console_auth").hidden = true; $("console_mfa").hidden = true;
    $("console_credentials").hidden = false; $("provision_retry").hidden = true;
    $("console_signin").hidden = false;
    $("console_restart").hidden = true;
    refresh();
  }
  function refresh() {
    $("automatic_setup").hidden = !device.connected() || $("connection_mode").value !== "local";
    $("automatic_setup").disabled = busy;
    $("automatic_local").hidden = $("connection_mode").value !== "cloud";
    $("console_signin").disabled = busy || !device.consoleReady();
    $("console_mfa_send").disabled = busy;
    $("console_restart").disabled = busy || !device.consoleReady();
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
    catch (error) { if (started === generation) { if (phase === "login") login = undefined; note(error.message, true); $("provision_retry").hidden = phase === "login" || phase === "mfa" && selectedMethod()?.dataset.type !== "push"; } }
    finally { busy = false; refresh(); if (requested && device.consoleReady()) { requested = false; void run(inspect); } }
  }
  async function command(name, values = {}, timeout) { return device.consoleCommand({ name, ...values }, timeout); }
  async function finish(current, localKey = "") {
    $("console_auth").hidden = true;
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
    $("console_restart").hidden = false;
    $("console_method").replaceChildren(...result.methods.map(m => new Option(({push:"Approve in UniFi Verify",totp:"Authenticator code",email:"Email code",sms:"SMS code",backup:"Backup code"})[m.type] || m.type, m.id)));
    for (const [index, method] of result.methods.entries()) $("console_method").options[index].dataset.type = method.type;
    await chooseMethod(current);
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
    $("console_auth").hidden = !!key; $("console_credentials").hidden = false;
    $("console_signin").hidden = false;
    $("console_signin").textContent = "Sign in & create local access";
    phase = "login";
    if (key) {
      try { await finish(current, key); return; }
      catch (error) {
        if (!current()) return;
        if (!/access denied/i.test(error.message)) throw error;
        $("api_key").value = "";
        invalidateKey(key);
        $("api_key").dispatchEvent(new Event("input", { bubbles: true }));
        $("console_auth").hidden = false;
        phase = "login"; note(`${error.message}. Sign in below to create a replacement local key.`, true);
      }
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
      if (started !== generation || !device.consoleReady() || phase !== "mfa" || selectedMethod()?.dataset.type !== "push") return;
      if (busy) { poll(started); return; }
      await run(async current => {
        await checkApproval(current);
      });
    }, 2000);
  }
  const selectedMethod = () => $("console_method").selectedOptions[0];
  async function checkApproval(current) {
    const result = await command("console_poll");
    if (!current()) return;
    if (result?.state === "waiting") { note("Approve sign-in in UniFi Verify. This page will continue automatically."); poll(generation); return; }
    await signedIn(result, current);
  }
  async function sendApproval(current) {
    const option = selectedMethod();
    if (!option || !["push", "email", "sms"].includes(option.dataset.type)) return;
    clearTimeout(timer); approvalSent = false;
    await command("console_mfa", { id: option.value }); if (!current()) return;
    approvalSent = true; $("provision_retry").hidden = true;
    note(option.dataset.type === "push" ? "Approve sign-in in UniFi Verify. This page will continue automatically." : "Enter the code sent by your console.");
    if (option.dataset.type === "push") poll(generation);
    else $("console_token").focus();
  }
  async function chooseMethod(current) {
    clearTimeout(timer); approvalSent = false; $("console_token").value = "";
    const kind = selectedMethod()?.dataset.type;
    $("console_code").hidden = kind === "push";
    $("console_signin").hidden = kind === "push";
    $("console_signin").textContent = "Verify code & create local access";
    $("console_mfa_send").hidden = !["push", "email", "sms"].includes(kind);
    $("console_mfa_send").textContent = kind === "push" ? "Resend approval" : "Resend code";
    $("provision_retry").hidden = true;
    if (["push", "email", "sms"].includes(kind)) await sendApproval(current);
    else { note(kind === "backup" ? "Enter one of your backup codes." : "Enter the code from your authenticator."); $("console_token").focus(); }
  }
  $("console_method").addEventListener("change", () => run(chooseMethod));
  $("console_restart").addEventListener("click", () => { if (!busy) { clear(); void run(inspect); } });
  $("console_mfa_send").addEventListener("click", () => run(sendApproval));
  $("provision_retry").addEventListener("click", () => run(async current => {
    $("provision_retry").hidden = true;
    if (phase === "verify" && target) await finish(current);
    else if (phase === "mfa") { if (approvalSent) await checkApproval(current); else await sendApproval(current); }
    else await inspect(current);
  }));
  $("provision_update").addEventListener("click", () => { clear(); void device.updateFirmware(); });
  $("automatic_setup").addEventListener("click", () => {
    $("unifi_editor").open = true;
    $("connection_mode").value = "cloud";
    $("connection_mode").dispatchEvent(new Event("change", { bubbles: true }));
    if ($("cloud_api_key").value) $("discover_cloud").click();
    else $("cloud_api_key").focus();
  });
  window.addEventListener("cloud-selection", () => { clear(); note("Discover your gateway to set up its local key and certificate automatically."); });
  function enroll() {
    clear();
    $("unifi_editor").open = true;
    if (device.consoleReady()) { if (busy) requested = true; else void run(inspect); }
    else { note(device.connected() ? "Install the latest firmware to create local access over USB." : "Connect your display to set up its local key and certificate automatically."); $("provision_retry").hidden = false; }
  }
  window.addEventListener("cloud-verified", enroll);
  window.addEventListener("wifi-changing", () => { clear(); note("Check Wi-Fi to continue local console setup."); });
  window.addEventListener("wifi-verified", () => { if (cloud.verified()) enroll(); else cloud.resume(); });
  window.addEventListener("display-state", () => { if (!device.connected()) clear(); else refresh(); });
  window.addEventListener("pagehide", clear);
  refresh();
  return { reset: clear };
}
