import { test } from "node:test";
import assert from "node:assert/strict";
import { CLOUD_ORIGIN, cloudBase, cloudClient, consoleLanUrl, rootGateway, consoleTarget } from "../cloud.js";

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
    else if (url.endsWith("/statistics/latest")) value = { interfaces: {} };
    else if (url.endsWith(`/devices/${device}`)) value = { id: device };
    else if (url.endsWith("?limit=1")) value = { data: [], totalCount: 0 };
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
test("LAN discovery ignores the WAN address and identifies the root gateway by console MAC", () => {
  const host = { reportedState: { mac: "AA:BB:CC:DD:EE:FF", ipAddrs: ["203.0.113.9", "fe80::1", "192.168.6.1"] } };
  assert.equal(consoleLanUrl(host), "https://192.168.6.1/proxy/network/integration");
  assert.equal(rootGateway(host, [{ id: "ap", macAddress: "11:22:33:44:55:66" }, { id: "root", macAddress: "aa-bb-cc-dd-ee-ff" }]), "root");
  assert.equal(rootGateway({}, [{ id: "first" }, { id: "second" }]), "");
  assert.equal(consoleLanUrl({ reportedState: { ipAddrs: ["10.0.0.1", "192.168.6.1"] } }), "");
});
test("automatic enrollment binds a private address to the cloud-reported direct-connect name", () => {
  const host = {reportedState:{directConnectDomain:"console.id.ui.direct",ipAddrs:["203.0.113.9","192.168.6.1"]}};
  assert.deepEqual(consoleTarget(host),{address:"192.168.6.1",hostname:"console.id.ui.direct"});
  for (const name of ["console.ui.direct.evil.test","-bad.ui.direct","console.ui.direct\r\nX: value", "ui.direct"]) {
    assert.throws(()=>consoleTarget({reportedState:{...host.reportedState,directConnectDomain:name}}));
  }
  assert.throws(()=>consoleTarget({reportedState:{...host.reportedState,ipAddrs:["203.0.113.9"]}}));
});
test("HTTP 200 does not verify another device, incompatible statistics or invalid page data",async()=>{
 const base=cloudBase("console");
 const paths=[`/devices/${device}`,`/devices/${device}/statistics/latest`,"/devices?limit=1","/clients?limit=1"];
 const valid=[{id:device},{interfaces:{}},{data:[],totalCount:0},{data:[],totalCount:0}];
 for(const [index,bad] of [[0,{id:site}],[1,{}],[1,{interfaces:{radios:{}}}],[2,{data:{},totalCount:1}],[3,{data:[],totalCount:-1}]]){
  let calls=0;const api=cloudClient("fixture-key",signal(),async url=>{const i=paths.findIndex(path=>url.endsWith(path));calls++;return Response.json(i===index?bad:valid[i]);});
  await assert.rejects(api.verify(base,site,device), /device|statistics|count/i);assert.equal(calls,index+1);
 }
 const html=cloudClient("fixture-key",signal(),async()=>new Response("private-test-material: <html>login</html>"));
 await assert.rejects(html.verify(base,site,device),error=>/invalid JSON/i.test(error.message) && !error.message.includes("private"));
});
