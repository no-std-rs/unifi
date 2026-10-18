export const config = { wifi_ssid: "saved-network", wifi_password: "saved-password", controller_url: "https://192.168.6.1/proxy/network/integration", api_key: "device-local-key", certificate_sha256: "ab".repeat(32), site_id: "", device_id: "" };
export async function device(page, options = {}) {
  await page.addInitScript(({ config, options }) => {
    window.usbRequests = [];
    let controller, pending = "", mode = false;
    const encoder = new TextEncoder();
    const respond = data => {
      const bytes = encoder.encode(`bootloader\nUNIFI_APP ${JSON.stringify(data)}\n`);
      // Exercise the browser decoder across arbitrary USB chunk boundaries.
      controller.enqueue(bytes.slice(0, 17)); controller.enqueue(bytes.slice(17));
    };
    const port = {
      open: async () => {}, close: async () => { mode = false; window.appMode = mode; },
      readable: new ReadableStream({ start(c) { controller = c; } }),
      writable: new WritableStream({ write(bytes) {
        pending += new TextDecoder().decode(bytes);
        const lines = pending.split("\n"); pending = lines.pop();
        for (const line of lines) {
          if (!line.startsWith("UNIFI_APP ")) continue;
          const request = JSON.parse(line.slice(10)); window.usbRequests.push(request);
          const command = request.command;
          if (["scan_wifi", "test_wifi"].includes(command.name)) { window.consoleTarget = undefined; window.consoleKey = undefined; }
          let result = null, error = null;
          if (command.name === "hello") { mode = true; window.appMode = mode; result = { nonce: command.nonce, protocol: 1, board: "ESP32-2424S012", capabilities: ["scan_wifi", "test_wifi", "save_config", ...(options.noProvision ? [] : ["console_provision"])] }; }
          if (command.name === "hello" && options.oldFirmware) result.capabilities = [];
          if (command.name === "status") result = { configured: !options.blank, ip: "192.168.6.20", app_mode: mode };
          if (command.name === "get_config") result = { config: options.blank ? null : { ...config, wifi_password: options.devicePassword || config.wifi_password } };
          if (command.name === "scan_wifi") result = { networks: window.scanNetworks || options.networks || [{ ssid: "saved-network", rssi: -30, channel: 1, secure: true },{ ssid: "scanned-network", rssi: -40, channel: 6, secure: true }, { ssid: "scanned-network", rssi: -80, channel: 6, secure: true }, { ssid: "<script>noise</script>", rssi: -60, channel: 1, secure: false }] };
          if (command.name === "test_wifi") {
            if (command.password === "wrong-password") error = "Wi-Fi association failed; check network and password";
            else result = { ssid: command.ssid, ip: "192.168.6.42" };
          }
          if (command.name === "save_config") result = { restart: true };
          if (command.name === "console_begin") {
            window.consoleTarget = command;
            window.consoleKey = undefined;
            if (options.badCertificate) error = "Console certificate could not be verified";
            else result = { hostname: command.hostname, certificate_sha256: "cd".repeat(32) };
          }
          if (command.name === "console_login") {
            window.consoleKey = undefined;
            if (command.password === "wrong-login") error = "Console access denied";
            else result = options.mfa && !command.token ? { state: "mfa", methods: [{ id: "method-1", type: options.mfa }] } : { state: "authenticated" };
          }
          if (command.name === "console_mfa") result = { state: "sent" };
          if (command.name === "console_poll") result = { state: "authenticated" };
          if (command.name === "console_provision" && window.consoleTarget) {
            window.consoleKey = command.local_key || window.consoleKey || "created-local-key";
            if (options.provisionFailure && !window.provisionRetried) { window.provisionRetried = true; error = options.transientExisting ? "Console rate limit reached; wait before trying again" : command.local_key ? "Console access denied; check the account or local API key" : "Local verification failed; retry"; }
            else result = { state: "verified", config: { controller_url: `https://${window.consoleTarget.hostname}/proxy/network/integration`, api_key: window.consoleKey, certificate_sha256: "cd".repeat(32), site_id: command.site_id, device_id: command.device_id } };
          }
          if (["console_login", "console_mfa", "console_poll", "console_provision"].includes(command.name) && !window.consoleTarget) error = "Verify the console certificate first";
          if (command.name === "bye") { mode = false; window.appMode = mode; }
          const data = { v: 1, id: request.id, session: 321, ok: !error, result, error };
          if (command.name === options.delay) window.completeUsbOperation = () => respond(data);
          else respond(data);
        }
      } }),
    };
    Object.defineProperty(navigator, "serial", { value: { getPorts: async () => [], requestPort: async () => port } });
    window.unplugUsb = () => controller.close();
  }, { config, options });
}
