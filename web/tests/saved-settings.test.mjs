import { test } from "node:test";
import assert from "node:assert/strict";
import { SAVED_SETTINGS_KEY, savedFields, readSavedSettings, writeSavedSettings } from "../saved-settings.js";

function storage() {
  const values = new Map();
  return { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) };
}
test("settings persist only explicit fields and opt-out deletes credential values", () => {
  const s = storage();
  const values = Object.fromEntries(Object.keys(savedFields).map(key => [key, ""]));
  Object.assign(values, { connection_mode: "cloud", cloud_api_key: "cloud-secret", api_key: "local-secret", wifi_password: "wifi-secret" });
  writeSavedSettings(s, true, { ...values, verified: true, manifest: "blob:old" });
  assert.deepEqual(readSavedSettings(s), { remember: true, values });
  assert.ok(!s.getItem(SAVED_SETTINGS_KEY).includes("verified"));
  writeSavedSettings(s, false);
  assert.deepEqual(readSavedSettings(s), { remember: false });
  for (const secret of ["cloud-secret", "local-secret", "wifi-secret"]) assert.ok(!s.getItem(SAVED_SETTINGS_KEY).includes(secret));
});
test("corrupt, incompatible and oversized stored settings do not restore", () => {
  const s = storage();
  for (const input of ["not JSON", "null", '{"version":2,"remember":true}', '{"version":1,"remember":true,"values":[]}']) {
    s.setItem(SAVED_SETTINGS_KEY, input);
    assert.equal(readSavedSettings(s), null);
  }
  const values = Object.fromEntries(Object.keys(savedFields).map(key => [key, ""]));
  Object.assign(values, { connection_mode: "cloud", api_key: "x".repeat(513) });
  writeSavedSettings(s, true, values);
  assert.equal(readSavedSettings(s), null);
});
test("older browser settings restore with an empty optional TLS identity",()=>{
 const s=storage(),values=Object.fromEntries(Object.keys(savedFields).map(key=>[key,""]));
 values.connection_mode="local";values.api_key="local-key";delete values.tls_server_name;
 s.setItem(SAVED_SETTINGS_KEY,JSON.stringify({version:1,remember:true,values}));
 assert.deepEqual(readSavedSettings(s),{remember:true,values:{...values,tls_server_name:""}});
});
