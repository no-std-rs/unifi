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
          let result = null, error = null;
          if (command.name === "hello") { mode = true; window.appMode = mode; result = { nonce: command.nonce, protocol: 1, board: "ESP32-2424S012", capabilities: ["scan_wifi", "test_wifi", "save_config"] }; }
          if (command.name === "hello" && options.oldFirmware) result.capabilities = [];
          if (command.name === "status") result = { configured: !options.blank, ip: "192.168.6.20", app_mode: mode };
          if (command.name === "get_config") result = { config: options.blank ? null : { ...config, wifi_password: options.devicePassword || config.wifi_password } };
          if (command.name === "scan_wifi") result = { networks: options.networks || [{ ssid: "saved-network", rssi: -30, channel: 1, secure: true },{ ssid: "scanned-network", rssi: -40, channel: 6, secure: true }, { ssid: "scanned-network", rssi: -80, channel: 6, secure: true }, { ssid: "<script>noise</script>", rssi: -60, channel: 1, secure: false }] };
          if (command.name === "test_wifi") {
            if (command.password === "wrong-password") error = "Wi-Fi association failed; check network and password";
            else result = { ssid: command.ssid, ip: "192.168.6.42" };
          }
          if (command.name === "save_config") result = { restart: true };
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
