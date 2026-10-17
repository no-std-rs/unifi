import { test } from "node:test";
import assert from "node:assert/strict";
import { localSettings } from "../setup-wizard.js";

const config = {
  controller_url: "https://console.id.ui.direct/proxy/network/integration",
  api_key: "private-local-key", certificate_sha256: "ab".repeat(32),
  site_id: "11111111-1111-4111-8111-111111111111", device_id: "22222222-2222-4222-8222-222222222222",
};
test("wizard imports only a validated local connection, excluding cloud and Wi-Fi fields", () => {
  assert.deepEqual(localSettings(config), config);
  for (const value of [null, [], {}, { ...config, cloud_api_key: "secret" }, { ...config, wifi_password: "secret" },
    { ...config, controller_url: "https://api.ui.com/v1/connector/example" },
    { ...config, controller_url: "https://203.0.113.5/integration" },
    { ...config, certificate_sha256: "invalid" }]) {
    assert.throws(() => localSettings(value));
  }
});
