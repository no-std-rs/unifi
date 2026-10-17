import { test, expect } from "@playwright/test";
const config = { wifi_ssid: "saved-network", wifi_password: "saved-password", controller_url: "https://192.168.6.1/proxy/network/integration", api_key: "device-local-key", certificate_sha256: "ab".repeat(32), site_id: "", device_id: "" };
async function device(page, options = {}) {
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
          if (command.name === "status") result = { configured: !options.blank, ip: "192.168.6.20", app_mode: mode };
          if (command.name === "get_config") result = { config: options.blank ? null : config };
          if (command.name === "scan_wifi") result = { networks: [{ ssid: "scanned-network", rssi: -40, channel: 6, secure: true }, { ssid: "scanned-network", rssi: -80, channel: 6, secure: true }, { ssid: "<script>noise</script>", rssi: -60, channel: 1, secure: false }] };
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

test("live USB reads settings, scans on device, rejects wrong passwords and saves only local settings", async ({ page }) => {
  const requests = [], errors = [];
  page.on("request", request => requests.push({ url: request.url(), body: request.postData() }));
  page.on("pageerror", e => errors.push(e.message));
  await device(page); await page.goto("/");
  await page.getByRole("button", { name: "Connect display", exact: true }).click();
  await expect(page.locator("#usb_status")).toContainText("APP MODE connected");
  expect(await page.evaluate(() => window.appMode)).toBe(true);
  await page.getByRole("button", { name: "Read saved device settings" }).click();
  await expect(page.getByLabel("Local Network API key")).toHaveValue(config.api_key);
  await expect(page.getByLabel("Setup method", { exact: true })).toHaveValue("local");
  await page.getByRole("button", { name: "Scan Wi-Fi", exact: true }).click();
  await expect(page.locator("#wifi_networks option")).toHaveCount(3);
  await page.getByLabel("Nearby Wi-Fi networks").selectOption("scanned-network");
  await expect(page.getByLabel("Wi-Fi name")).toHaveValue("scanned-network");
  await page.getByLabel("Wi-Fi password", { exact: true }).fill("wrong-password");
  await page.getByRole("button", { name: "Test Wi-Fi on display" }).click();
  await expect(page.locator("#usb_status")).toContainText("association failed");
  await page.getByLabel("Wi-Fi password", { exact: true }).fill("verified-password");
  await page.getByRole("button", { name: "Test Wi-Fi on display" }).click();
  await expect(page.locator("#usb_status")).toContainText("IP 192.168.6.42");
  await page.getByRole("button", { name: "Save settings and restart" }).click();
  await expect(page.locator("#usb_status")).toContainText("restarting into its local dashboard");
  const commands = await page.evaluate(() => window.usbRequests);
  expect(commands.find(r => r.command.name === "save_config").command.config).toEqual({ ...config, wifi_ssid: "scanned-network", wifi_password: "verified-password" });
  expect(commands.filter(r => r.command.name !== "hello").every(r => r.session === 321)).toBe(true);
  for (const request of requests) {
    expect(JSON.stringify(request)).not.toContain("verified-password");
    expect(JSON.stringify(request)).not.toContain("device-local-key");
  }
  expect(await page.evaluate(() => window.appMode)).toBe(false);
  expect(errors).toEqual([]);
});

test("blank firmware supports setup before Wi-Fi and firmware-only installation requires no credentials", async ({ page }) => {
  await device(page, { blank: true }); await page.goto("/");
  await expect.poll(() => page.locator("#blank_installer").evaluate(el => el.manifest || "")).toMatch(/^blob:/);
  const manifest = await page.evaluate(async () => (await fetch(document.getElementById("blank_installer").manifest)).json());
  expect(manifest.builds[0].parts).toHaveLength(1);
  expect(manifest.builds[0].parts[0].offset).toBe(0);
  await page.getByRole("button", { name: "Connect display", exact: true }).click();
  await expect(page.locator("#usb_status")).toContainText("Scan Wi-Fi");
  await page.getByRole("button", { name: "Read saved device settings" }).click();
  await expect(page.locator("#usb_status")).toContainText("no saved settings");
  await page.getByRole("button", { name: "Disconnect display", exact: true }).click();
  await expect(page.getByRole("button", { name: "Connect display", exact: true })).toBeEnabled();
});

test("disconnect during a pending read cannot refill the form and leaves controls reusable", async ({ page }) => {
  await device(page, { delay: "get_config" }); await page.goto("/");
  await page.getByRole("button", { name: "Connect display", exact: true }).click();
  await expect(page.locator("#usb_status")).toContainText("APP MODE connected");
  await page.getByRole("button", { name: "Read saved device settings" }).click();
  await expect(page.getByLabel("Local Network API key")).toBeDisabled();
  await page.getByRole("button", { name: "Disconnect display", exact: true }).click();
  await expect(page.getByLabel("Local Network API key")).toBeEnabled();
  await expect(page.getByLabel("Local Network API key")).toHaveValue("");
  await expect(page.getByRole("button", { name: "Connect display", exact: true })).toBeEnabled();
});
