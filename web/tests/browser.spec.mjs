import { test, expect } from "@playwright/test";
import { readFile } from "node:fs/promises";

const site = "11111111-1111-4111-8111-111111111111";
const device = "22222222-2222-4222-8222-222222222222";
const otherDevice = "33333333-3333-4333-8333-333333333333";
async function offline(page) {
  if (!await page.getByRole("button", { name: "Prepare settings without a device" }).isVisible()) await page.locator("#device_tools > summary").click();
  await page.getByRole("button", { name: "Prepare settings without a device" }).click();
  if (!await page.getByLabel("Remember settings in this browser").isVisible()) await page.locator(".saved-settings > summary").click();
}
async function serial(page) {
  await page.addInitScript(() => { Object.defineProperty(navigator, "serial", { value: { getPorts: async () => [] } }); });
}
async function wifi(page) {
  await page.getByLabel("Wi-Fi name").fill("browser-test-network");
  await page.getByLabel("Wi-Fi password", { exact: true }).fill("private-wifi-password");
}
async function localCredentials(page) {
  await page.getByLabel("Local controller integration URL").fill("https://192.168.6.1/proxy/network/integration");
  await page.getByLabel("Local Network API key").fill("private-local-key");
  await page.getByLabel("Certificate SHA-256 fingerprint").fill("ab".repeat(32));
}
async function usbSettings(page) {
  return page.evaluate(async () => {
    const manifest = await (await fetch(document.getElementById("installer").manifest)).json();
    const sector = new Uint8Array(await (await fetch(manifest.builds[0].parts[1].path)).arrayBuffer());
    const length = new DataView(sector.buffer).getUint32(8, true);
    return JSON.parse(new TextDecoder().decode(sector.subarray(16, 16 + length)));
  });
}
async function mockCloud(page, requests, consoleId = "console:123") {
  await page.route("https://api.ui.com/**", async route => {
    const request = route.request();
    requests.push({ url: request.url(), headers: request.headers() });
    const path = new URL(request.url()).pathname;
    let body = {};
    if (path === "/v1/hosts") body = { data: [{ id: consoleId, type: "console", reportedState: { state: "connected", name: "Home console", mac: "aa:bb:cc:dd:ee:ff", ipAddrs: ["203.0.113.9", "192.168.6.1"] } }] };
    else if (path.endsWith("/sites")) body = { data: [{ id: site, name: "Home" }], totalCount: 1 };
    else if (path.endsWith("/devices")) body = { data: [{ id: device, name: "Gateway", macAddress: "aa:bb:cc:dd:ee:ff", ipAddress: "203.0.113.9", state: "ONLINE" }, { id: otherDevice, name: "Access point", state: "ONLINE" }], totalCount: 2 };
    else if (path.endsWith("/statistics/latest")) body = { interfaces: {} };
    else if (/\/devices\/[0-9a-f-]+$/i.test(path)) body = { id: path.split("/").at(-1) };
    else if (path.endsWith("/clients")) body = { data: [], totalCount: 50 };
    await route.fulfill({ json: body, headers: { "Access-Control-Allow-Origin": "*" } });
  });
}

test("public firmware verifies and settings stay in browser-created USB blobs", async ({ page }) => {
  const requests = [];
  page.on("request", request => requests.push({ url: request.url(), body: request.postData() }));
  await page.addInitScript(() => { Object.defineProperty(navigator, "serial", { value: { getPorts: async () => [] } }); });
  await page.goto("/"); await offline(page);
  await expect(page.getByRole("button", { name: "Prepare installation" })).toBeEnabled();
  await page.getByLabel("Setup method", { exact: true }).selectOption("local");
  await page.getByLabel("Wi-Fi name").fill("browser-test-network");
  await page.getByLabel("Wi-Fi password", { exact: true }).fill("private-wifi-password");
  await page.getByLabel("Local Network API key").fill("private-api-key");
  await page.getByLabel("Certificate SHA-256 fingerprint").fill("ab".repeat(32));
  await page.getByRole("button", { name: "Prepare installation" }).click();
  await expect(page.getByRole("button", { name: "Install on display" })).toBeVisible();
  const manifest = await page.evaluate(async () => {
    const manifest = await (await fetch(document.getElementById("installer").manifest)).json();
    const sector = new Uint8Array(await (await fetch(manifest.builds[0].parts[1].path)).arrayBuffer());
    const length = new DataView(sector.buffer).getUint32(8, true);
    return { manifest, settings: JSON.parse(new TextDecoder().decode(sector.subarray(16, 16 + length))) };
  });
  expect(manifest.manifest.builds[0].chipFamily).toBe("ESP32-C3");
  expect(manifest.manifest.builds[0].parts[1].offset).toBe(0x3f0000);
  expect(manifest.settings.wifi_password).toBe("private-wifi-password");
  expect(manifest.settings.api_key).toBe("private-api-key");
  for (const request of requests.filter(r => !r.url.startsWith("blob:"))) {
    expect(JSON.stringify(request)).not.toContain("private-wifi-password");
    expect(JSON.stringify(request)).not.toContain("private-api-key");
    expect(new URL(request.url).origin).toBe("http://127.0.0.1:4173");
  }
  await page.getByLabel("Wi-Fi name").fill("changed network");
  await expect(page.getByRole("button", { name: "Install on display" })).toBeHidden();
});

test("cloud discovery selects the root gateway, while the USB settings contain only local credentials", async ({ page }) => {
  const requests = [];
  await page.setViewportSize({ width: 390, height: 844 });
  await serial(page); await mockCloud(page, requests);
  await page.goto("/"); await offline(page); await wifi(page);
  await page.getByLabel("Cloud discovery API key").fill("private-cloud-key");
  await page.getByRole("button", { name: "Discover consoles" }).click();
  await expect(page.getByLabel("Device to display")).toBeEnabled();
  await expect(page.getByLabel("Device to display")).toHaveValue(device);
  await localCredentials(page);
  await page.getByLabel("Device to display").selectOption(device);
  await page.getByRole("button", { name: "Verify cloud access" }).click();
  await expect(page.locator("#cloud_status")).toContainText("Cloud reads verified");
  await page.getByRole("button", { name: "Prepare installation" }).click();
  await expect(page.getByRole("button", { name: "Install on display" })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  const settings = await usbSettings(page);
  expect(settings).toMatchObject({ controller_url: "https://192.168.6.1/proxy/network/integration", api_key: "private-local-key", certificate_sha256: "ab".repeat(32), site_id: site, device_id: device });
  expect(JSON.stringify(settings)).not.toContain("private-cloud-key");
  expect(requests).toHaveLength(7);
  for (const request of requests) {
    expect(new URL(request.url).origin).toBe("https://api.ui.com");
    expect(request.headers["x-api-key"]).toBe("private-cloud-key");
    expect(JSON.stringify(request)).not.toContain("private-local-key");
    expect(request.url).not.toContain("private-cloud-key");
    expect(JSON.stringify(request)).not.toContain("private-wifi-password");
  }
  await page.getByLabel("Device to display").selectOption(otherDevice);
  await expect(page.getByRole("button", { name: "Install on display" })).toBeHidden();
  await page.getByRole("button", { name: "Prepare installation" }).click();
  await expect(page.locator("#status")).toContainText("Verify cloud access");
});

test("a rejected cloud key is explained and cannot prepare installation", async ({ page }) => {
  await serial(page);
  await page.route("https://api.ui.com/**", route => route.fulfill({ status: 401, json: { message: "unauthorized" } }));
  await page.goto("/"); await offline(page); await wifi(page);
  await page.getByLabel("Cloud discovery API key").fill("invalid-test-key");
  await localCredentials(page);
  await page.getByRole("button", { name: "Discover consoles" }).click();
  await expect(page.locator("#cloud_status")).toContainText("HTTP 401");
  await page.getByRole("button", { name: "Prepare installation" }).click();
  await expect(page.locator("#status")).toContainText("Verify cloud access");
  await expect(page.getByRole("button", { name: "Install on display" })).toBeHidden();
});

test("changing the key cancels old discovery and requires new verification", async ({ page }) => {
  await serial(page);
  let release, started;
  const pending = new Promise(resolve => { release = resolve; });
  const ready = new Promise(resolve => { started = resolve; });
  await page.route("https://api.ui.com/**", async route => {
    started(); await pending;
    await route.fulfill({ json: { data: [{ id: "old", type: "console", reportedState: { state: "connected", name: "Old console" } }] } });
  });
  await page.goto("/"); await offline(page);
  await page.getByLabel("Cloud discovery API key").fill("old-test-key");
  await page.getByRole("button", { name: "Discover consoles" }).click();
  await ready;
  await page.getByLabel("Cloud discovery API key").fill("new-test-key");
  release();
  await expect(page.locator("#cloud_status")).toContainText("Key changed");
  await expect(page.locator("#cloud_choices")).toBeHidden();
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("unifi-display.settings.v1")).values.cloud_api_key)).toBe("new-test-key");
});

test("corrupt firmware cannot be prepared or flashed", async ({ page }) => {
  await page.route("**/unifi-display.bin", route => route.fulfill({ body: Buffer.alloc(32), contentType: "application/octet-stream" }));
  await page.goto("/"); await offline(page);
  await expect(page.getByRole("alert")).toContainText("download size changed");
  await expect(page.getByRole("button", { name: "Prepare installation" })).toBeDisabled();
});

test("a same-size firmware change fails SHA-256 verification", async ({ page }) => {
  const binary = await readFile(new URL("../../dist/flasher/unifi-display.bin", import.meta.url));
  binary[binary.length - 1] ^= 1;
  await page.route("**/unifi-display.bin", route => route.fulfill({ body: binary, contentType: "application/octet-stream" }));
  await page.goto("/"); await offline(page);
  await expect(page.getByRole("alert")).toContainText("checksum did not match");
  await expect(page.getByRole("button", { name: "Prepare installation" })).toBeDisabled();
});

test("downloads the verified bytes even after the published binary changes", async ({ page }) => {
  const binary = await readFile(new URL("../../dist/flasher/unifi-display.bin", import.meta.url));
  await page.goto("/"); await offline(page);
  await page.locator(".build > summary").click();
  const link = page.getByRole("link", { name: "Download binary" });
  await expect(link).toBeVisible();
  await page.route("**/unifi-display.bin", route => route.fulfill({ body: Buffer.alloc(binary.length), contentType: "application/octet-stream" }));
  const [download] = await Promise.all([page.waitForEvent("download"), link.click()]);
  expect(download.suggestedFilename()).toBe("unifi-display.bin");
  expect(await readFile(await download.path())).toEqual(binary);
});

test("mobile layout remains usable and unsupported browsers can download", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => { Object.defineProperty(navigator, "serial", { value: undefined }); });
  await page.goto("/"); await offline(page);
  await page.locator(".build > summary").click();
  await expect(page.getByRole("link", { name: "Download binary" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Prepare installation" })).toBeDisabled();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});

test("local credentials autofill in a new tab and can be forgotten without clearing other apps", async ({ page, context }) => {
  await serial(page); await page.goto("/"); await offline(page);
  await page.getByLabel("Setup method", { exact: true }).selectOption("local");
  await wifi(page); await localCredentials(page);
  await page.evaluate(() => localStorage.setItem("other-app", "keep"));
  await page.close();
  const reopened = await context.newPage();
  await serial(reopened); await reopened.goto("/"); await offline(reopened);
  await expect(reopened.getByLabel("Wi-Fi name")).toHaveValue("browser-test-network");
  await expect(reopened.getByLabel("Wi-Fi password", { exact: true })).toHaveValue("private-wifi-password");
  await expect(reopened.getByLabel("Local Network API key")).toHaveValue("private-local-key");
  await expect(reopened.getByLabel("Setup method", { exact: true })).toHaveValue("local");
  await expect(reopened.getByLabel("Certificate SHA-256 fingerprint")).toHaveValue("ab".repeat(32));
  await reopened.getByRole("button", { name: "Prepare installation" }).click();
  await expect(reopened.getByRole("button", { name: "Install on display" })).toBeVisible();
  await reopened.getByRole("button", { name: "Forget saved settings" }).click();
  await expect(reopened.getByRole("button", { name: "Install on display" })).toBeHidden();
  await reopened.reload();
  await expect(reopened.getByLabel("Wi-Fi name")).toHaveValue("");
  await expect(reopened.getByLabel("Local Network API key")).toHaveValue("");
  await expect(reopened.getByLabel("Remember settings in this browser")).not.toBeChecked();
  expect(await reopened.evaluate(() => localStorage.getItem("other-app"))).toBe("keep");
});

test("restored cloud choices require rediscovery and verification while both keys stay separate", async ({ page }) => {
  const requests = [];
  await serial(page); await mockCloud(page, requests);
  await page.goto("/"); await offline(page); await wifi(page);
  await page.getByLabel("Cloud discovery API key").fill("private-cloud-key");
  await page.getByRole("button", { name: "Discover consoles" }).click();
  await expect(page.getByLabel("Device to display")).toBeEnabled();
  await localCredentials(page);
  await page.getByLabel("Device to display").selectOption(otherDevice);
  await page.getByRole("button", { name: "Verify cloud access" }).click();
  await expect(page.locator("#cloud_status")).toContainText("Cloud reads verified");
  await page.reload(); await offline(page);
  await expect(page.getByLabel("Cloud discovery API key")).toHaveValue("private-cloud-key");
  await expect(page.getByLabel("Local Network API key")).toHaveValue("private-local-key");
  await page.getByRole("button", { name: "Prepare installation" }).click();
  await expect(page.locator("#status")).toContainText("Verify cloud access");
  await page.getByRole("button", { name: "Discover consoles" }).click();
  await expect(page.getByLabel("Device to display")).toHaveValue(otherDevice);
  await page.getByRole("button", { name: "Prepare installation" }).click();
  await expect(page.locator("#status")).toContainText("Verify cloud access");
  await page.getByRole("button", { name: "Verify cloud access" }).click();
  await expect(page.locator("#cloud_status")).toContainText("Cloud reads verified");
  await page.getByRole("button", { name: "Prepare installation" }).click();
  await expect(page.getByRole("button", { name: "Install on display" })).toBeVisible();
  expect(await usbSettings(page)).toMatchObject({ device_id: otherDevice, api_key: "private-local-key", controller_url: "https://192.168.6.1/proxy/network/integration" });
  await page.getByRole("button", { name: "Forget saved settings" }).click();
  await offline(page);
  await mockCloud(page, requests, "different-console:456");
  await page.getByLabel("Cloud discovery API key").fill("new-cloud-key");
  await localCredentials(page);
  await page.getByLabel("Local controller integration URL").fill("https://gateway.local/proxy/network/integration");
  await page.getByRole("button", { name: "Discover consoles" }).click();
  await expect(page.getByLabel("Device to display")).toHaveValue(device);
  await expect(page.getByLabel("Local controller integration URL")).toHaveValue("https://gateway.local/proxy/network/integration");
  await expect(page.getByLabel("Local Network API key")).toHaveValue("private-local-key");
});

test("opting out removes credentials and stays off on reopening", async ({ page }) => {
  await page.goto("/"); await offline(page); await wifi(page);
  await page.getByLabel("Remember settings in this browser").uncheck();
  await page.getByLabel("Local Network API key").fill("session-only-key");
  const stored = await page.evaluate(() => localStorage.getItem("unifi-display.settings.v1"));
  expect(stored).not.toContain("session-only-key");
  expect(stored).not.toContain("private-wifi-password");
  await page.reload();
  await expect(page.getByLabel("Remember settings in this browser")).not.toBeChecked();
  await expect(page.getByLabel("Local Network API key")).toHaveValue("");
});

test("unavailable browser storage still permits a local installation", async ({ page }) => {
  await serial(page);
  await page.addInitScript(() => Object.defineProperty(window, "localStorage", { get() { throw new DOMException("Disabled", "SecurityError"); } }));
  await page.goto("/"); await offline(page);
  await page.getByLabel("Setup method", { exact: true }).selectOption("local");
  await wifi(page); await localCredentials(page);
  await expect(page.locator("#saved_status")).toContainText("cannot save");
  await page.getByRole("button", { name: "Prepare installation" }).click();
  await expect(page.getByRole("button", { name: "Install on display" })).toBeVisible();
});

test("switching consoles clears local credentials even when both use the same LAN address", async ({ page }) => {
  await serial(page); await mockCloud(page, []);
  await page.route("https://api.ui.com/v1/hosts", route => route.fulfill({ json: { data: ["first:1", "second:2"].map(id => ({ id, type: "console", reportedState: { state: "connected", name: id, mac: "aa:bb:cc:dd:ee:ff", ipAddrs: ["192.168.6.1"] } })) } }));
  await page.goto("/"); await offline(page); await wifi(page);
  await page.getByLabel("Cloud discovery API key").fill("private-cloud-key");
  await page.getByRole("button", { name: "Discover consoles" }).click();
  await expect(page.getByLabel("Console", { exact: true })).toBeEnabled();
  await page.getByLabel("Console", { exact: true }).selectOption("first:1");
  await expect(page.getByLabel("Device to display")).toHaveValue(device);
  await localCredentials(page);
  await page.getByRole("button", { name: "Verify cloud access" }).click();
  await expect(page.locator("#cloud_status")).toContainText("Cloud reads verified");
  await page.getByRole("button", { name: "Prepare installation" }).click();
  await expect(page.getByRole("button", { name: "Install on display" })).toBeVisible();
  await page.getByLabel("Console", { exact: true }).selectOption("second:2");
  await expect(page.getByLabel("Local controller integration URL")).toHaveValue("https://192.168.6.1/proxy/network/integration");
  await expect(page.getByLabel("Local Network API key")).toHaveValue("");
  await expect(page.getByLabel("Certificate SHA-256 fingerprint")).toHaveValue("");
  await expect(page.getByRole("button", { name: "Install on display" })).toBeHidden();
});
