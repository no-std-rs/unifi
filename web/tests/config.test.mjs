import { test } from "node:test";
import assert from "node:assert/strict";
import { crc32, encodeConfig, manifestWithSettings, validateConfig, CONFIG_OFFSET } from "../config.js";

export const settings = () => ({ wifi_ssid: "test network", wifi_password: "test-password", controller_url: "https://unifi.local/proxy/network/integration", api_key: "test-api-key", certificate_sha256: "ab".repeat(32), site_id: "", device_id: "" });

test("settings sector has the firmware magic, size, checksum, and UTF-8 JSON", () => {
  const config = settings(); config.wifi_ssid = "Zażółć";
  const sector = encodeConfig(config);
  const view = new DataView(sector.buffer);
  const size = view.getUint32(8, true);
  assert.equal(sector.length, 4096);
  assert.equal(new TextDecoder().decode(sector.subarray(0, 8)), "UNIFICF1");
  assert.equal(view.getUint32(12, true), crc32(sector.subarray(16, 16 + size)));
  assert.deepEqual(JSON.parse(new TextDecoder().decode(sector.subarray(16, 16 + size))), config);
  assert.ok(sector.subarray(16 + size).every(b => b === 255));
  assert.equal(crc32(new TextEncoder().encode("123456789")), 0xcbf43926);
});

test("rejects invalid credentials, routes, Unicode byte limits, and unknown fields", () => {
  for (const patch of [{ wifi_ssid: "界".repeat(11) }, { wifi_password: "short" }, { controller_url: "http://unifi/integration" },
    { controller_url: "https://user:secret@unifi/integration" }, { controller_url: "https://unifi/integration\r\n" }, { controller_url: "https://unifi:0/" },
    { controller_url: "https://unifi/path/../integration" }, { api_key: "" }, { api_key: "key\r\nHeader: injected" }, { certificate_sha256: "0".repeat(63) },
    { site_id: "site/../../etc" }, { device_id: "not-a-uuid" }, { other: "unknown" }]) {
    assert.throws(() => validateConfig({ ...settings(), ...patch }));
  }
  assert.doesNotThrow(() => validateConfig({ ...settings(), wifi_password: "" }));
});

test("install manifest writes settings only to the dedicated partition", () => {
  const release = { application: "unifi-display", settings_schema: "UNIFICF1", settings_capabilities:["tls_server_name"], dirty: false, chip: "ESP32-C3", flash_size: 4194304, config_offset: CONFIG_OFFSET, revision: "a".repeat(40), sha256: "b".repeat(64), size: 1000000 };
  const manifest = manifestWithSettings(release, "blob:firmware", "blob:settings");
  assert.deepEqual(manifest.builds[0].parts, [{ path: "blob:firmware", offset: 0 }, { path: "blob:settings", offset: 0x3f0000 }]);
  assert.equal(manifest.new_install_improv_wait_time, 0);
  assert.throws(() => manifestWithSettings({ ...release, config_offset: 0x10000 }, "f", "s"));
  assert.throws(() => manifestWithSettings({ ...release, chip: "ESP32" }, "f", "s"));
  assert.throws(() => manifestWithSettings({ ...release, dirty: true }, "f", "s"));
  assert.throws(() => manifestWithSettings({ ...release, application: "ubus-display" }, "f", "s"));
  assert.throws(() => manifestWithSettings({ ...release, settings_schema: "UBUSCFG1" }, "f", "s"));
  assert.throws(() => manifestWithSettings({ ...release, settings_capabilities:[] }, "f", "s"), /needs an update/);
  assert.throws(() => manifestWithSettings({ ...release, settings_capabilities:undefined }, "f", "s"), /needs an update/);
});
test("a numeric connection destination keeps its optional TLS identity without changing legacy sectors",()=>{
 const config={...settings(),controller_url:"https://192.168.6.1/proxy/network/integration",tls_server_name:"console.id.ui.direct"};
 const sector=encodeConfig(config),size=new DataView(sector.buffer).getUint32(8,true);
 assert.deepEqual(JSON.parse(new TextDecoder().decode(sector.subarray(16,16+size))),config);
 for(const tls_server_name of ["https://console.id.ui.direct","console:443","*.ui.direct","bad\r\nHeader",null,12])assert.throws(()=>validateConfig({...config,tls_server_name}));
 const legacy=encodeConfig({...settings(),tls_server_name:""}),length=new DataView(legacy.buffer).getUint32(8,true);
 assert.deepEqual(JSON.parse(new TextDecoder().decode(legacy.subarray(16,16+length))),settings());
});
