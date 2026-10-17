import { test } from "node:test";
import assert from "node:assert/strict";
import { CLOUD_ORIGIN, cloudBase, cloudClient, cloudTrust } from "../cloud.js";

const site = "11111111-1111-4111-8111-111111111111";
const device = "22222222-2222-4222-8222-222222222222";
const signal = () => new AbortController().signal;
test("discovery paginates and every dashboard read uses the official origin and header", async () => {
  const calls = [];
  const api = cloudClient("fixture-key", signal(), async (url, options) => {
    calls.push(url);
    assert.equal(new URL(url).origin, CLOUD_ORIGIN);
    assert.equal(options.headers["X-API-Key"], "fixture-key");
    assert.equal(options.credentials, "omit");
    assert.equal(options.redirect, "error");
    assert.equal(options.method, "GET");
    assert.ok(!url.includes("fixture-key"));
    let value = {};
    if (url.endsWith("/hosts")) value = { data: [], nextToken: "next/token" };
    else if (url.includes("/hosts?")) value = { data: [{ id: "console:1", type: "console", reportedState: { state: "connected" } }] };
    else if (url.endsWith("/sites?offset=0&limit=100")) value = { data: [{ id: site }], totalCount: 2 };
    else if (url.endsWith("/sites?offset=1&limit=100")) value = { data: [{ id: "other" }], totalCount: 2 };
    return Response.json(value);
  });
  assert.equal((await api.consoles()).length, 1);
  const base = cloudBase("console:1");
  assert.equal(base, "/v1/connector/consoles/console%3A1/proxy/network/integration");
  assert.equal((await api.sites(base)).length, 2);
  await api.verify(base, site, device);
  assert.equal(calls.length, 8);
  assert.ok(calls.some(url => url.endsWith(`/devices/${device}/statistics/latest`)));
  assert.ok(calls.at(-1).endsWith("/clients?limit=1"));
});
test("cloud errors identify authentication, permissions and rate limits without echoing API bodies", async () => {
  for (const status of [401, 403, 429, 502]) {
    const api = cloudClient("fixture-key", signal(), async () => new Response("secret upstream body", { status }));
    await assert.rejects(api.consoles(), error => error.message.includes(String(status)) && !error.message.includes("secret"));
  }
});
test("stalled pages and invalid identifiers fail before making unintended requests", async () => {
  const api = cloudClient("fixture-key", signal(), async () => Response.json({ data: [], totalCount: 1 }));
  await assert.rejects(api.sites(cloudBase("console")), /incomplete/);
  const repeated = cloudClient("fixture-key", signal(), async () => Response.json({ data: [], nextToken: "same" }));
  await assert.rejects(repeated.consoles(), /stalled/);
  for (const id of ["https://attacker.example", "../console", "console?key=x"]) assert.throws(() => cloudBase(id));
  await assert.rejects(api.verify(cloudBase("console"), "../site", device), /UUID/);
  assert.throws(() => cloudClient("key with spaces", signal()));
});
test("certificate metadata must identify the cloud and be verified and unexpired", () => {
  const now = Date.now();
  const metadata = { origin: CLOUD_ORIGIN, certificate_sha256: "ab".repeat(32), verified_at: new Date(now).toISOString(), not_after: new Date(now + 86400000).toISOString() };
  assert.equal(cloudTrust(metadata, now), metadata.certificate_sha256);
  for (const patch of [{ origin: "https://other.example" }, { certificate_sha256: "bad" }, { not_after: new Date(now - 1).toISOString() }, { verified_at: "invalid" }, { verified_at: new Date(now + 600000).toISOString() }]) {
    assert.throws(() => cloudTrust({ ...metadata, ...patch }, now));
  }
});
