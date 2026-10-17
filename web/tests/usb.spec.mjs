import { test, expect } from "@playwright/test";
import { config, device } from "./serial-fixture.mjs";
async function connect(page,options={}) { await device(page,options);await page.goto("/");await page.getByRole("button",{name:"Connect display",exact:true}).click(); }
async function verified(page) {await expect(page.locator("#wifi_status")).toContainText("Wi-Fi verified");}
async function password(page,value) {await page.getByLabel("Wi-Fi password",{exact:true}).fill(value);}
async function local(page) {
 await page.getByLabel("Setup method",{exact:true}).selectOption("local");
 await page.getByLabel("Local controller integration URL").fill(config.controller_url);
 await page.getByLabel("Local Network API key").fill(config.api_key);
 await page.getByLabel("Certificate SHA-256 fingerprint").fill(config.certificate_sha256);
}
test("connect automatically reads, scans, tests saved Wi-Fi and reuses local settings",async({page})=>{
 const requests=[],errors=[];page.on("request",r=>requests.push({url:r.url(),body:r.postData()}));page.on("pageerror",e=>errors.push(e.message));
 await connect(page);await verified(page);
 await expect(page.locator("#step_unifi")).toHaveAttribute("aria-current","step");
 await expect(page.getByLabel("Wi-Fi password",{exact:true})).toBeHidden();
 expect(await page.evaluate(()=>window.usbRequests.filter(r=>r.command.name!=="ping").map(r=>r.command.name))).toEqual(["hello","get_config","scan_wifi","test_wifi"]);
 await page.getByRole("button",{name:"Save and restart display"}).click();await expect(page.locator("#usb_status")).toContainText("Settings saved");
 expect(await page.evaluate(()=>window.usbRequests.find(r=>r.command.name==="save_config").command.config)).toEqual(config);
 expect(JSON.stringify(requests)).not.toContain(config.wifi_password);expect(JSON.stringify(requests)).not.toContain(config.api_key);
 expect(requests.every(r=>new URL(r.url).origin==="http://127.0.0.1:4173"||r.url.startsWith("blob:"))).toBe(true);expect(errors).toEqual([]);
});
test("new Wi-Fi asks inline, failed password stays editable and successful retry advances to UniFi",async({page})=>{
 await connect(page);await verified(page);
 await page.getByLabel("Wi-Fi network",{exact:true}).selectOption("scanned-network");
 await expect(page.getByLabel("Wi-Fi password",{exact:true})).toHaveValue("");await expect(page.locator("#unifi_step")).toBeHidden();
 await password(page,"wrong-password");await page.getByRole("button",{name:"Connect to Wi-Fi",exact:true}).click();
 await expect(page.locator("#wifi_status")).toContainText("association failed");
 await password(page,"verified-password");await page.getByRole("button",{name:"Try again",exact:true}).click();await verified(page);
 await expect(page.getByLabel("Local Network API key")).toHaveValue("");await local(page);
 await page.getByRole("button",{name:"Save and restart display"}).click();await expect(page.locator("#usb_status")).toContainText("Settings saved");
 const profiles=await page.evaluate(()=>JSON.parse(localStorage.getItem("unifi-display.networks.v1")).networks);
 expect(profiles.find(p=>p.ssid==="scanned-network")).toMatchObject({password:"verified-password",config:{api_key:config.api_key}});
 expect(profiles.find(p=>p.ssid==="saved-network").password).toBe(config.wifi_password);
});
test("a saved password can be changed and is replaced only after a passing test",async({page})=>{
 await connect(page);await verified(page);await page.getByRole("button",{name:"Change password",exact:true}).click();
 await password(page,"wrong-password");await page.getByRole("button",{name:"Connect to Wi-Fi",exact:true}).click();
 await expect(page.locator("#wifi_status")).toContainText("association failed");
 expect(await page.evaluate(()=>JSON.parse(localStorage.getItem("unifi-display.networks.v1")).networks[0].password)).toBe(config.wifi_password);
 await password(page,"replacement-password");await page.getByRole("button",{name:"Try again",exact:true}).click();await verified(page);
 expect(await page.evaluate(()=>JSON.parse(localStorage.getItem("unifi-display.networks.v1")).networks[0].password)).toBe("replacement-password");
});
test("blank firmware scans immediately and open networks need no password or helper",async({page})=>{
 const requests=[];page.on("request",r=>requests.push(r.url()));await connect(page,{blank:true});
 await expect(page.getByLabel("Wi-Fi network",{exact:true})).toBeEnabled();
 await page.getByLabel("Wi-Fi network",{exact:true}).selectOption("<script>noise</script>");await verified(page);
 expect(await page.evaluate(()=>window.usbRequests.find(r=>r.command.name==="test_wifi").command.password)).toBe("");
 expect(requests.some(url=>url.includes(":4177")||url.includes("setup-display.py"))).toBe(false);
 const manifest=await page.evaluate(async()=>(await fetch(document.getElementById("blank_installer").manifest)).json());expect(manifest.builds[0].parts).toHaveLength(1);
});
test("known networks restore their own password and local API settings after reopening",async({page})=>{
 await page.addInitScript(({config})=>{
  const local=Object.fromEntries(Object.entries(config).filter(([k])=>!k.startsWith("wifi_")));
  localStorage.setItem("unifi-display.networks.v1",JSON.stringify({version:1,networks:[{ssid:"scanned-network",password:"remembered-password",config:local}]}));
  const names=["wifi_ssid","wifi_password","api_key","cloud_api_key","controller_url","certificate_sha256","site_id","device_id","connection_mode","cloud_console","cloud_site","cloud_device"];
  const values=Object.fromEntries(names.map(k=>[k,""]));Object.assign(values,config,{wifi_ssid:"scanned-network",wifi_password:"old-draft-password",connection_mode:"local"});
  localStorage.setItem("unifi-display.settings.v1",JSON.stringify({version:1,remember:true,values}));
 },{config});
 await connect(page,{blank:true});await verified(page);
 expect(await page.evaluate(()=>window.usbRequests.find(r=>r.command.name==="test_wifi").command.password)).toBe("remembered-password");
 await expect(page.getByLabel("Local Network API key")).toHaveValue(config.api_key);
});
test("opt-out and forget remove per-network credentials",async({page})=>{
 await connect(page);await verified(page);await page.locator(".saved-settings").evaluate(el=>el.open=true);
 await page.getByLabel("Remember settings in this browser").uncheck();expect(await page.evaluate(()=>localStorage.getItem("unifi-display.networks.v1"))).toBeNull();
 await page.getByRole("button",{name:"Forget saved settings"}).click();await expect(page.getByRole("button",{name:"Connect display",exact:true})).toBeEnabled();
 expect(await page.evaluate(()=>localStorage.getItem("unifi-display.networks.v1"))).toBeNull();expect(await page.evaluate(()=>document.getElementById("api_key").value)).toBe("");
});
test("disconnect cancels an automatic read without refilling the form",async({page})=>{
 await connect(page,{delay:"get_config"});await expect(page.locator("#settings")).toHaveAttribute("aria-busy","true");
 await page.getByRole("button",{name:"Disconnect display",exact:true}).click();await expect(page.getByRole("button",{name:"Connect display",exact:true})).toBeEnabled();
 expect(await page.evaluate(()=>document.getElementById("api_key").value)).toBe("");
});
test("unsupported firmware reports how to recover",async({page})=>{await connect(page,{oldFirmware:true});await expect(page.locator("#usb_status")).toContainText("Install the latest firmware first");});
test("slow tests retain inline progress beyond 65 seconds",async({page})=>{
 await page.clock.install();await connect(page,{delay:"test_wifi"});await expect.poll(()=>page.evaluate(()=>typeof window.completeUsbOperation)).toBe("function");
 await page.clock.runFor(68000);await expect(page.locator("#wifi_status")).toContainText("Checking saved-network");await page.evaluate(()=>window.completeUsbOperation());await verified(page);
});

test("hidden network entry accepts Enter to test instead of submitting incomplete UniFi settings",async({page})=>{
 await connect(page,{blank:true});await expect(page.getByLabel("Wi-Fi network",{exact:true})).toBeEnabled();
 await page.getByLabel("Wi-Fi network",{exact:true}).selectOption({label:"Other network…"});
 await page.getByLabel("Wi-Fi name").fill("hidden-network");await password(page,"hidden-password");
 await page.getByLabel("Wi-Fi password",{exact:true}).press("Enter");await verified(page);
 expect(await page.evaluate(()=>window.usbRequests.find(r=>r.command.name==="test_wifi").command)).toMatchObject({ssid:"hidden-network",password:"hidden-password"});
});

test("USB save still requires fresh verification after changing a cloud device",async({page})=>{
 const site="11111111-1111-4111-8111-111111111111",first="22222222-2222-4222-8222-222222222222",second="33333333-3333-4333-8333-333333333333";
 await page.route("https://api.ui.com/**",async route=>{
  const path=new URL(route.request().url()).pathname;let body={};
  if(path==="/v1/hosts")body={data:[{id:"console:1",type:"console",reportedState:{state:"connected",name:"Gateway",mac:"aa:bb:cc:dd:ee:ff",ipAddrs:["192.168.6.1"]}}]};
  else if(path.endsWith("/sites"))body={data:[{id:site,name:"Home"}],totalCount:1};
  else if(path.endsWith("/devices"))body={data:[{id:first,name:"First",macAddress:"aa:bb:cc:dd:ee:ff"},{id:second,name:"Second"}],totalCount:2};
  else if(path.endsWith("/clients"))body={data:[],totalCount:0};
  await route.fulfill({json:body});
 });
 await connect(page);await verified(page);await page.locator("#unifi_editor > summary").click();
 await page.getByLabel("Setup method",{exact:true}).selectOption("cloud");await page.getByLabel("Cloud discovery API key").fill("fixture-cloud-key");
 await page.getByRole("button",{name:"Discover consoles"}).click();await expect(page.getByLabel("Device to display")).toHaveValue(first);
 await local(page);await page.getByLabel("Setup method",{exact:true}).selectOption("cloud");
 await page.getByRole("button",{name:"Discover consoles"}).click();await expect(page.getByLabel("Device to display")).toHaveValue(first);
 await page.getByRole("button",{name:"Verify cloud access"}).click();await expect(page.locator("#cloud_status")).toContainText("Cloud reads verified");
 await page.getByLabel("Device to display").selectOption(second);await page.getByRole("button",{name:"Save and restart display"}).click();
 await expect(page.locator("#unifi_status")).toContainText("Verify cloud access");expect(await page.evaluate(()=>window.usbRequests.some(r=>r.command.name==="save_config"))).toBe(false);
 await page.getByRole("button",{name:"Verify cloud access"}).click();await expect(page.locator("#cloud_status")).toContainText("Cloud reads verified");
 await page.getByRole("button",{name:"Save and restart display"}).click();await expect(page.locator("#usb_status")).toContainText("Settings saved");
 expect(await page.evaluate(()=>window.usbRequests.find(r=>r.command.name==="save_config").command.config.device_id)).toBe(second);
});

test("forget during a pending local file import keeps secrets cleared",async({page})=>{
 await page.addInitScript(()=>{const original=File.prototype.text;File.prototype.text=async function(){await new Promise(resolve=>window.releaseImport=resolve);return original.call(this);};});
 await connect(page);await verified(page);await page.getByText("Import a local connection",{exact:true}).click();
 const local=Object.fromEntries(Object.entries(config).filter(([k])=>!k.startsWith("wifi_")));
 await page.locator("#import_setup").setInputFiles({name:"local.json",mimeType:"application/json",buffer:Buffer.from(JSON.stringify(local))});
 await page.locator(".saved-settings > summary").click();await page.getByRole("button",{name:"Forget saved settings"}).click();await page.evaluate(()=>window.releaseImport());
 expect(await page.evaluate(()=>document.getElementById("api_key").value)).toBe("");expect(await page.evaluate(()=>localStorage.getItem("unifi-display.networks.v1"))).toBeNull();
});
