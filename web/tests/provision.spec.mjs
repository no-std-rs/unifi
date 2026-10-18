import { test, expect } from "@playwright/test";
import { device, config } from "./serial-fixture.mjs";
const site="11111111-1111-4111-8111-111111111111",gateway="22222222-2222-4222-8222-222222222222";
async function setup(page, options={}) {
 if(options.rememberedCloud)await page.addInitScript(config=>localStorage.setItem("unifi-display.settings.v1",JSON.stringify({version:1,remember:true,values:{...config,api_key:"",certificate_sha256:"",connection_mode:"cloud",cloud_api_key:"fixture-cloud-key",cloud_console:"",cloud_site:"",cloud_device:""}})),config);
 if(options.existing)await page.addInitScript(config=>localStorage.setItem("unifi-display.settings.v1",JSON.stringify({version:1,remember:true,values:{...config,connection_mode:"local",cloud_api_key:"fixture-cloud-key",cloud_console:"",cloud_site:"",cloud_device:""}})),config);
 if(options.sharedKey)await page.addInitScript(config=>{
  const local=Object.fromEntries(Object.entries(config).filter(([key])=>!key.startsWith("wifi_")));
  localStorage.setItem("unifi-display.networks.v1",JSON.stringify({version:1,networks:[{ssid:"scanned-network",password:"other-password",config:local},{ssid:"different-key-network",password:"different-password",config:{...local,api_key:"unrelated-local-key"}}]}));
 },config);
 await device(page,{blank:!options.existing,...options});
 let failCloud=options.cloudFailure, badStatistics=options.badCloudStatistics;
 await page.route("https://api.ui.com/**",route=>{
  const path=new URL(route.request().url()).pathname;
  if(path==="/v1/hosts" && failCloud){failCloud=false;return route.fulfill({status:403,json:{}});}
  if(path.endsWith("/statistics/latest") && badStatistics){badStatistics=false;return route.fulfill({json:{interfaces:null}});}
  const data=path==="/v1/hosts"?{data:[{id:"console:1",type:"console",reportedState:{state:"connected",name:"Gateway",mac:"aa:bb:cc:dd:ee:ff",ipAddrs:["192.168.6.1"],directConnectDomain:"console.id.ui.direct"}}]}:
   path.endsWith("/sites")?{data:[{id:site,name:"Default"}],totalCount:1}:
   path.endsWith("/devices")?{data:[{id:gateway,name:"Root gateway",macAddress:"aa:bb:cc:dd:ee:ff"}],totalCount:1}:
   path.endsWith("/statistics/latest")?{interfaces:{}}:
   path.endsWith(`/devices/${gateway}`)?{id:gateway}:
   path.endsWith("/clients")?{data:[],totalCount:0}:{};
  return route.fulfill({json:data});
 });
 await page.goto("/");await page.getByRole("button",{name:"Connect display",exact:true}).click();
 if(!options.existing && !options.rememberedCloud){await page.getByLabel("Wi-Fi network",{exact:true}).selectOption("saved-network");await page.getByLabel("Wi-Fi password",{exact:true}).fill("fixture-password");await page.getByRole("button",{name:"Connect to Wi-Fi",exact:true}).click();}
 await expect(page.locator("#wifi_status")).toContainText("Wi-Fi verified");
 if(options.existing){await expect(page.locator("#unifi_editor")).not.toHaveAttribute("open");await page.getByRole("button",{name:"Set up key & certificate automatically"}).click();return;}
 if(options.rememberedCloud)return;
 await page.getByLabel("Cloud discovery API key").fill("fixture-cloud-key");await page.getByRole("button",{name:"Discover consoles",exact:true}).click();
}
async function signIn(page,password="fixture-console-password") {
 await page.getByLabel("Console username or email").fill("fixture-user");await page.getByLabel("Console password",{exact:true}).fill(password);
 await page.getByRole("button",{name:"Sign in & create local access",exact:true}).click();
}
test("cloud discovery automatically enrolls the certificate and creates local access through USB",async({page})=>{
 const requests=[];page.on("request",r=>requests.push({url:r.url(),body:r.postData()}));await setup(page);
 await expect(page.locator("#provision_status")).toContainText("Certificate verified automatically");
 await expect(page.getByLabel("Certificate SHA-256 fingerprint")).toHaveValue("cd".repeat(32));
 await signIn(page);await expect(page.locator("#provision_status")).toContainText("Local access ready");
 await page.getByRole("button",{name:"Save and restart display"}).click();await expect(page.locator("#usb_status")).toContainText("Settings saved");
 const commands=await page.evaluate(()=>window.usbRequests.map(r=>r.command));
 expect(commands.find(c=>c.name==="save_config").config).toMatchObject({api_key:"created-local-key",certificate_sha256:"cd".repeat(32),site_id:site,device_id:gateway,controller_url:"https://console.id.ui.direct/proxy/network/integration"});
 expect(JSON.stringify(commands)).not.toContain("fixture-cloud-key");expect(JSON.stringify(requests)).not.toContain("fixture-console-password");
 expect(await page.evaluate(()=>JSON.stringify(localStorage))).not.toContain("fixture-console-password");
 expect(requests.every(r=>r.url.startsWith("blob:") || [new URL(page.url()).origin,"https://api.ui.com"].includes(new URL(r.url).origin))).toBe(true);
});
test("an untrusted console certificate never exposes the sign-in form or sends credentials",async({page})=>{
 await setup(page,{badCertificate:true});await expect(page.locator("#provision_status")).toContainText("certificate could not be verified");
 await expect(page.locator("#console_auth")).toBeHidden();
 expect(await page.evaluate(()=>window.usbRequests.some(r=>r.command.name==="console_login"))).toBe(false);
});
test("authenticator approval continues enrollment without manual key or SHA fields",async({page})=>{
 await setup(page,{mfa:"totp"});await expect(page.locator("#console_auth")).toBeVisible();await signIn(page);
 await expect(page.locator("#console_mfa_send")).toBeHidden();await expect(page.locator("#console_token")).toBeFocused();
 await page.getByLabel("Two-factor or backup code").fill("123456");await page.getByRole("button",{name:"Verify code & create local access"}).click();
 await expect(page.locator("#provision_status")).toContainText("Local access ready");
 const log=await page.evaluate(()=>window.usbRequests.filter(r=>r.command.name==="console_login").map(r=>r.command));expect(log[1].token).toBe("123456");expect(log[1].password).toBe("fixture-console-password");
});
test("push approval automatically finishes local enrollment",async({page})=>{
 await setup(page,{mfa:"push"});await expect(page.locator("#console_auth")).toBeVisible();await signIn(page);
 await expect(page.locator("#console_code")).toBeHidden();await expect(page.locator("#console_signin")).toBeHidden();
 await expect(page.locator("#provision_status")).toContainText("Local access ready",{timeout:15000});
 expect(await page.evaluate(()=>window.usbRequests.filter(r=>r.command.name==="console_mfa").length)).toBe(1);
});
test("verification retry reuses enrollment without another login",async({page})=>{
 await setup(page,{provisionFailure:true});await expect(page.locator("#console_auth")).toBeVisible();await signIn(page);
 await expect(page.locator("#provision_status")).toContainText("Local verification failed");await page.getByRole("button",{name:"Retry local setup"}).click();
 await expect(page.locator("#provision_status")).toContainText("Local access ready");expect(await page.evaluate(()=>window.usbRequests.filter(r=>r.command.name==="console_login").length)).toBe(1);
});
test("a rejected console password can be replaced inline",async({page})=>{
 await setup(page);await expect(page.locator("#console_auth")).toBeVisible();await signIn(page,"wrong-login");await expect(page.locator("#provision_status")).toContainText("access denied");
 await signIn(page);await expect(page.locator("#provision_status")).toContainText("Local access ready");
});
test("older APP firmware offers an update before attempting enrollment",async({page})=>{
 await setup(page,{noProvision:true});await expect(page.locator("#provision_status")).toContainText("Install the latest firmware");
 expect(await page.evaluate(()=>window.usbRequests.some(r=>r.command.name==="console_begin"))).toBe(false);
 await page.getByRole("button",{name:"Update display firmware"}).click();
 await expect(page.getByRole("button",{name:"Install firmware first"})).toBeEnabled();await expect(page.getByRole("button",{name:"Connect display",exact:true})).toBeVisible();
});
test("forget cancels pending push approval and clears transient console credentials",async({page})=>{
 await page.clock.install();await setup(page,{mfa:"push"});await expect(page.locator("#console_auth")).toBeVisible();await signIn(page);
 await expect(page.locator("#provision_status")).toContainText("Approve sign-in");
 await page.locator(".saved-settings > summary").click();await page.getByRole("button",{name:"Forget saved settings"}).click();await page.clock.fastForward(5000);
 expect(await page.evaluate(()=>window.usbRequests.some(r=>["console_poll","console_provision"].includes(r.command.name)))).toBe(false);
 await expect(page.locator("#console_password")).toHaveValue("");await expect(page.locator("#console_username")).toHaveValue("");
 expect(await page.evaluate(()=>JSON.stringify(localStorage))).not.toContain("fixture-console-password");
});
test("an existing display exposes automatic repair and reuses its saved cloud key",async({page})=>{
 await setup(page,{existing:true,provisionFailure:true});
 await expect(page.locator("#console_auth")).toBeVisible();await expect(page.locator("#provision_status")).toContainText("create a replacement local key");
 await expect(page.getByRole("button",{name:"Save and restart display"})).toBeHidden({timeout:1000});
 await expect(page.locator("#certificate_sha256")).toHaveValue("cd".repeat(32));
 await signIn(page);await expect(page.locator("#provision_status")).toContainText("Local access ready");
 await page.getByRole("button",{name:"Save and restart display"}).click();await expect(page.locator("#usb_status")).toContainText("Settings saved");
 expect(await page.evaluate(()=>window.usbRequests.find(r=>r.command.name==="save_config").command.config.api_key)).toBe("created-local-key");
});
test("a rejected cached key stays invalidated after rescanning the saved network",async({page})=>{
 await setup(page,{existing:true,provisionFailure:true});
 await expect(page.locator("#provision_status")).toContainText("create a replacement local key");
 await page.getByRole("button",{name:"Scan again",exact:true}).click();await expect(page.locator("#settings")).toHaveAttribute("aria-busy","false");
 await expect(page.locator("#wifi_status")).toContainText("Wi-Fi verified");
 await expect(page.locator("#api_key")).toHaveValue("",{timeout:1000});
 await expect(page.getByRole("button",{name:"Save and restart display"})).toBeHidden();
});
test("every cached copy of the denied key is invalidated while other credentials remain",async({page})=>{
 await setup(page,{existing:true,provisionFailure:true,sharedKey:true});await expect(page.locator("#provision_status")).toContainText("create a replacement local key");
 await page.getByLabel("Wi-Fi network",{exact:true}).selectOption("scanned-network");await expect(page.locator("#settings")).toHaveAttribute("aria-busy","false");await expect(page.locator("#wifi_status")).toContainText("Wi-Fi verified");
 await expect(page.locator("#api_key")).toHaveValue("",{timeout:1000});await expect(page.getByRole("button",{name:"Save and restart display"})).toBeHidden();
 const profiles=await page.evaluate(()=>JSON.parse(localStorage.getItem("unifi-display.networks.v1")).networks);
 expect(profiles.find(p=>p.ssid==="scanned-network")).toMatchObject({password:"other-password",config:null});
 expect(profiles.find(p=>p.ssid==="different-key-network").config.api_key).toBe("unrelated-local-key");
});
test("a transient existing-key check retries without discarding the key or signing in",async({page})=>{
 await setup(page,{existing:true,provisionFailure:true,transientExisting:true});await expect(page.locator("#provision_status")).toContainText("rate limit reached");
 await expect(page.locator("#api_key")).toHaveValue(config.api_key,{timeout:1000});await expect(page.locator("#console_auth")).toBeHidden();
 await page.getByRole("button",{name:"Retry local setup"}).click();await expect(page.locator("#provision_status")).toContainText("Local access ready");
 await expect(page.locator("#api_key")).toHaveValue(config.api_key);
 expect(await page.evaluate(()=>window.usbRequests.some(r=>r.command.name==="console_login"))).toBe(false);
});
test("changing the Wi-Fi password cancels approval and automatically verifies the console again",async({page})=>{
 await page.clock.install();await setup(page,{mfa:"push"});await expect(page.locator("#console_auth")).toBeVisible();await signIn(page);
 await expect(page.locator("#provision_status")).toContainText("Approve sign-in");
 await page.getByRole("button",{name:"Change password",exact:true}).click();
 await page.getByLabel("Wi-Fi password",{exact:true}).fill("replacement-password");
 await page.getByRole("button",{name:"Connect to Wi-Fi",exact:true}).click();
 await expect.poll(()=>page.evaluate(()=>window.usbRequests.filter(r=>r.command.name==="console_begin").length)).toBe(2);
 await expect(page.locator("#provision_status")).toContainText("Certificate verified automatically");
 await expect(page.locator("#console_username")).toHaveValue("");await expect(page.locator("#console_mfa")).toBeHidden();
 await page.clock.fastForward(5000);
 expect(await page.evaluate(()=>window.usbRequests.some(r=>r.command.name==="console_poll"))).toBe(false);
 await signIn(page);await expect(page.locator("#provision_status")).toContainText("Approve sign-in");await page.clock.fastForward(2500);
 await expect(page.locator("#provision_status")).toContainText("Local access ready");
});
test("rescanning the enrolled network rechecks local access without cloud rediscovery or another login",async({page})=>{
 let discoveries=0;page.on("request",request=>{if(request.url()==="https://api.ui.com/v1/hosts")discoveries++;});
 await setup(page);await expect(page.locator("#console_auth")).toBeVisible();await signIn(page);
 await expect(page.locator("#provision_status")).toContainText("Local access ready");
 await page.getByRole("button",{name:"Scan again",exact:true}).click();
 await expect.poll(()=>page.evaluate(()=>window.usbRequests.filter(r=>r.command.name==="console_begin").length)).toBe(2);
 await expect(page.locator("#provision_status")).toContainText("Local access ready");
 await expect(page.locator("#connection_mode")).toHaveValue("cloud");expect(discoveries).toBe(1);
 expect(await page.evaluate(()=>window.usbRequests.filter(r=>r.command.name==="console_login").length)).toBe(1);
});
test("a saved cloud key resumes discovery as soon as remembered Wi-Fi connects",async({page})=>{
 let discoveries=0;page.on("request",r=>{if(r.url()==="https://api.ui.com/v1/hosts")discoveries++;});
 await setup(page,{rememberedCloud:true});await expect(page.locator("#console_auth")).toBeVisible();
 await expect(page.locator("#provision_status")).toContainText("Certificate verified automatically");expect(discoveries).toBe(1);
 expect(await page.evaluate(()=>JSON.stringify(window.usbRequests))).not.toContain("fixture-cloud-key");
 await signIn(page);await expect(page.locator("#provision_status")).toContainText("Local access ready");
});
test("failed saved-key discovery stays inline and does not loop on a Wi-Fi retry",async({page})=>{
 let discoveries=0;page.on("request",r=>{if(r.url()==="https://api.ui.com/v1/hosts")discoveries++;});
 await setup(page,{rememberedCloud:true,cloudFailure:true});await expect(page.locator("#cloud_status")).toContainText("HTTP 403");
 await page.getByRole("button",{name:"Change password",exact:true}).click();await page.getByLabel("Wi-Fi password",{exact:true}).fill("replacement-password");
 await page.getByRole("button",{name:"Connect to Wi-Fi",exact:true}).click();await expect(page.locator("#settings")).toHaveAttribute("aria-busy","false");
 expect(discoveries).toBe(1);await expect(page.locator("#cloud_status")).toContainText("HTTP 403");
 await page.getByRole("button",{name:"Discover consoles",exact:true}).click();await expect(page.locator("#console_auth")).toBeVisible();expect(discoveries).toBe(2);
});
test("push polling retries the same approval without another login or certificate check",async({page})=>{
 await page.clock.install();await setup(page,{mfa:"push",pollFailure:true});await expect(page.locator("#console_auth")).toBeVisible();await signIn(page);
 await expect(page.locator("#provision_status")).toContainText("Approve sign-in");await page.clock.fastForward(2500);
 await expect(page.locator("#provision_status")).toContainText("rate limit reached");await page.getByRole("button",{name:"Retry local setup"}).click();
 await expect(page.locator("#provision_status")).toContainText("Local access ready");
 const commands=await page.evaluate(()=>window.usbRequests.map(r=>r.command.name));
 for(const name of ["console_begin","console_login","console_mfa"])expect(commands.filter(c=>c===name)).toHaveLength(1);
 expect(commands.filter(c=>c==="console_poll")).toHaveLength(2);
});
test("switching from push to an authenticator cancels polling and focuses the code",async({page})=>{
 await page.clock.install();await setup(page,{mfa:"push",methods:["push","totp"]});await expect(page.locator("#console_auth")).toBeVisible();await signIn(page);
 await expect(page.locator("#provision_status")).toContainText("Approve sign-in");await page.getByLabel("Two-factor method").selectOption("method-2");
 await expect(page.locator("#console_mfa_send")).toBeHidden();await expect(page.locator("#console_token")).toBeFocused();await page.clock.fastForward(5000);
 expect(await page.evaluate(()=>window.usbRequests.some(r=>r.command.name==="console_poll"))).toBe(false);
 await page.getByLabel("Two-factor or backup code").fill("123456");await page.getByRole("button",{name:"Verify code & create local access"}).click();
 await expect(page.locator("#provision_status")).toContainText("Local access ready");
});
test("email approval sends the code automatically and keeps a resend action",async({page})=>{
 await setup(page,{mfa:"email"});await expect(page.locator("#console_auth")).toBeVisible();await signIn(page);
 await expect(page.locator("#provision_status")).toContainText("Enter the code sent");await expect(page.locator("#console_token")).toBeFocused();
 await page.getByRole("button",{name:"Resend code",exact:true}).click();
 expect(await page.evaluate(()=>window.usbRequests.filter(r=>r.command.name==="console_mfa").length)).toBe(2);
 await page.getByLabel("Two-factor or backup code").fill("123456");await page.getByRole("button",{name:"Verify code & create local access"}).click();
 await expect(page.locator("#provision_status")).toContainText("Local access ready");
});
test("restarting two-factor sign-in clears credentials and cancels the old approval",async({page})=>{
 await page.clock.install();await setup(page,{mfa:"push"});await expect(page.locator("#console_auth")).toBeVisible();await signIn(page);
 await expect(page.locator("#provision_status")).toContainText("Approve sign-in");await page.getByRole("button",{name:"Restart sign-in",exact:true}).click();
 await expect(page.locator("#provision_status")).toContainText("Certificate verified automatically");await expect(page.locator("#console_username")).toHaveValue("");
 await expect(page.locator("#console_password")).toHaveValue("");await expect(page.locator("#console_mfa")).toBeHidden();await page.clock.fastForward(5000);
 expect(await page.evaluate(()=>window.usbRequests.some(r=>r.command.name==="console_poll"))).toBe(false);
 await signIn(page);await expect(page.locator("#provision_status")).toContainText("Approve sign-in");await page.clock.fastForward(2500);
 await expect(page.locator("#provision_status")).toContainText("Local access ready");
});

test("incompatible cloud statistics stop enrollment and retry keeps the selection",async({page})=>{
 let discoveries=0;page.on("request",r=>{if(r.url()==="https://api.ui.com/v1/hosts")discoveries++;});
 await setup(page,{badCloudStatistics:true});
 await expect(page.locator("#cloud_status")).toContainText("Device statistics: UniFi returned incompatible data");
 await expect(page.locator("#console_auth")).toBeHidden();
 expect(await page.evaluate(()=>window.usbRequests.some(r=>r.command.name==="console_begin"))).toBe(false);
 await page.getByRole("button",{name:"Verify cloud access",exact:true}).click();
 await expect(page.locator("#provision_status")).toContainText("Certificate verified automatically");
 await signIn(page);await expect(page.locator("#provision_status")).toContainText("Local access ready");
 expect(discoveries).toBe(1);
});
