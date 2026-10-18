import { test } from "node:test";
import assert from "node:assert/strict";
import { localConfig, profile, readNetworks, writeNetworks, NETWORKS_KEY } from "../network-profiles.js";
function storage() { const m=new Map(); return {getItem:k=>m.get(k)||null,setItem:(k,v)=>m.set(k,v),removeItem:k=>m.delete(k)}; }
const config={controller_url:"https://192.168.1.1/integration",api_key:"local-key",certificate_sha256:"ab".repeat(32),site_id:"",device_id:""};
test("passwords and local credentials belong to the exact SSID, including prototype-like names",()=>{
 const s=storage(),networks=new Map([profile("First","password",config),profile("__proto__","other-password")].map(p=>[p.ssid,p]));
 writeNetworks(s,true,networks);const restored=readNetworks(s);
 assert.deepEqual(restored.get("First").config,config);assert.equal(restored.get("__proto__").password,"other-password");assert.equal(restored.has("first"),false);
 writeNetworks(s,false,networks);assert.equal(s.getItem(NETWORKS_KEY),null);
});
test("rejects corrupt/oversized profiles and excludes cloud keys or Wi-Fi from connection imports",()=>{
 const s=storage();for(const value of ["broken",JSON.stringify({version:1,networks:[null]}),"x".repeat(65537)]) {s.setItem(NETWORKS_KEY,value);assert.equal(readNetworks(s).size,0);}
 assert.throws(()=>localConfig({...config,cloud_api_key:"cloud-key"}));assert.throws(()=>localConfig({...config,wifi_password:"password"}));
 assert.throws(()=>profile("name","short"));assert.throws(()=>localConfig({...config,controller_url:"https://api.ui.com"}));
 const networks=new Map(Array.from({length:20},(_,i)=>[String(i),profile(String(i),"password")]));writeNetworks(s,true,networks);assert.equal(readNetworks(s).size,16);
});
test("TLS identity follows its LAN connection through import and per-network storage",()=>{
 const local={...config,tls_server_name:"console.id.ui.direct"},s=storage();
 assert.deepEqual(localConfig(local),local);writeNetworks(s,true,new Map([["First",profile("First","password",local)]]));
 assert.deepEqual(readNetworks(s).get("First").config,local);
 assert.throws(()=>localConfig({...local,tls_server_name:"https://bad/"}));
});
