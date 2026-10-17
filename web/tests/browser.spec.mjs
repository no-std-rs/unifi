import { test, expect } from "@playwright/test";
import { readFile } from "node:fs/promises";

const site = "11111111-1111-4111-8111-111111111111";
const device = "22222222-2222-4222-8222-222222222222";
const otherDevice = "33333333-3333-4333-8333-333333333333";
test.beforeEach(async ({ page }) => {
  await page.route("**/cloud.json", route => route.fulfill({ json: { origin: "https://api.ui.com", certificate_sha256: "ab".repeat(32), verified_at: new Date().toISOString(), not_after: new Date(Date.now() + 86400000).toISOString() } }));
});
async function serial(page) {
  await page.addInitScript(() => { Object.defineProperty(navigator, "serial", { value: { getPorts: async () => [] } }); });
}
async function wifi(page) {
  await page.getByLabel("Wi-Fi name").fill("browser-test-network");
  await page.getByLabel("Wi-Fi password", { exact: true }).fill("private-wifi-password");
}
async function usbSettings(page) {
  return page.evaluate(async () => {
    const manifest = await (await fetch(document.getElementById("installer").manifest)).json();
    const sector = new Uint8Array(await (await fetch(manifest.builds[0].parts[1].path)).arrayBuffer());
    const length = new DataView(sector.buffer).getUint32(8, true);
    return JSON.parse(new TextDecoder().decode(sector.subarray(16, 16 + length)));
  });
}
async function mockCloud(page, requests) {
  await page.route("https://api.ui.com/**", async route => {
    const request = route.request();
    requests.push({ url: request.url(), headers: request.headers() });
    const path = new URL(request.url()).pathname;
    let body = {};
    if (path === "/v1/hosts") body = { data: [{ id: "console:123", type: "console", reportedState: { state: "connected", name: "Home console" } }] };
    else if (path.endsWith("/sites")) body = { data: [{ id: site, name: "Home" }], totalCount: 1 };
    else if (path.endsWith("/devices")) body = { data: [{ id: device, name: "Gateway", state: "ONLINE" }, { id: otherDevice, name: "Access point", state: "ONLINE" }], totalCount: 2 };
    else if (path.endsWith("/clients")) body = { data: [], totalCount: 50 };
    await route.fulfill({ json: body, headers: { "Access-Control-Allow-Origin": "*" } });
  });
}

test("public firmware verifies and settings stay in browser-created USB blobs", async ({ page }) => {
  const requests = [];
  page.on("request", request => requests.push({ url: request.url(), body: request.postData() }));
  await page.addInitScript(() => { Object.defineProperty(navigator, "serial", { value: { getPorts: async () => [] } }); });
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Prepare installation" })).toBeEnabled();
  await page.getByLabel("Connection", { exact: true }).selectOption("local");
  await page.getByLabel("Wi-Fi name").fill("browser-test-network");
  await page.getByLabel("Wi-Fi password", { exact: true }).fill("private-wifi-password");
  await page.getByLabel("UniFi API key").fill("private-api-key");
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

test("cloud discovery selects explicit IDs, verifies every dashboard read, and flashes the cloud configuration", async ({ page }) => {
  const requests = [];
  await page.setViewportSize({ width: 390, height: 844 });
  await serial(page); await mockCloud(page, requests);
  await page.goto("/"); await wifi(page);
  await page.getByLabel("UniFi API key").fill("private-cloud-key");
  await page.getByRole("button", { name: "Discover consoles" }).click();
  await expect(page.getByLabel("Device to display")).toBeEnabled();
  await expect(page.getByLabel("Device to display")).toHaveValue("");
  await page.getByLabel("Device to display").selectOption(device);
  await page.getByRole("button", { name: "Verify cloud access" }).click();
  await expect(page.locator("#cloud_status")).toContainText("Cloud access verified");
  await page.getByRole("button", { name: "Prepare installation" }).click();
  await expect(page.getByRole("button", { name: "Install on display" })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  const settings = await usbSettings(page);
  expect(settings).toMatchObject({ controller_url: "https://api.ui.com/v1/connector/consoles/console%3A123/proxy/network/integration", api_key: "private-cloud-key", certificate_sha256: "ab".repeat(32), site_id: site, device_id: device });
  expect(requests).toHaveLength(7);
  for (const request of requests) {
    expect(new URL(request.url).origin).toBe("https://api.ui.com");
    expect(request.headers["x-api-key"]).toBe("private-cloud-key");
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
  await page.goto("/"); await wifi(page);
  await page.getByLabel("UniFi API key").fill("invalid-test-key");
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
  await page.goto("/");
  await page.getByLabel("UniFi API key").fill("old-test-key");
  await page.getByRole("button", { name: "Discover consoles" }).click();
  await ready;
  await page.getByLabel("UniFi API key").fill("new-test-key");
  release();
  await expect(page.locator("#cloud_status")).toContainText("Key changed");
  await expect(page.locator("#cloud_choices")).toBeHidden();
  expect(await page.evaluate(() => localStorage.length + sessionStorage.length)).toBe(0);
});

test("corrupt firmware cannot be prepared or flashed", async ({ page }) => {
  await page.route("**/unifi-display.bin", route => route.fulfill({ body: Buffer.alloc(32), contentType: "application/octet-stream" }));
  await page.goto("/");
  await expect(page.getByRole("alert")).toContainText("download size changed");
  await expect(page.getByRole("button", { name: "Prepare installation" })).toBeDisabled();
});

test("a same-size firmware change fails SHA-256 verification", async ({ page }) => {
  const binary = await readFile(new URL("../../dist/flasher/unifi-display.bin", import.meta.url));
  binary[binary.length - 1] ^= 1;
  await page.route("**/unifi-display.bin", route => route.fulfill({ body: binary, contentType: "application/octet-stream" }));
  await page.goto("/");
  await expect(page.getByRole("alert")).toContainText("checksum did not match");
  await expect(page.getByRole("button", { name: "Prepare installation" })).toBeDisabled();
});

test("downloads the verified bytes even after the published binary changes", async ({ page }) => {
  const binary = await readFile(new URL("../../dist/flasher/unifi-display.bin", import.meta.url));
  await page.goto("/");
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
  await page.goto("/");
  await expect(page.getByRole("link", { name: "Download binary" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Prepare installation" })).toBeDisabled();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});
