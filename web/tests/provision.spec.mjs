import { test, expect } from "@playwright/test";
import { device, config } from "./serial-fixture.mjs";
const site="11111111-1111-4111-8111-111111111111",gateway="22222222-2222-4222-8222-222222222222";
async function setup(page, options={}) {
 if(options.existing)await page.addInitScript(config=>localStorage.setItem("unifi-display.settings.v1",JSON.stringify({version:1,remember:true,values:{...config,connection_mode:"local",cloud_api_key:"fixture-cloud-key",cloud_console:"",cloud_site:"",cloud_device:""}})),config);
 await device(page,{blank:!options.existing,...options});
 await page.route("https://api.ui.com/**",route=>{
  const path=new URL(route.request().url()).pathname;
  const data=path==="/v1/hosts"?{data:[{id:"console:1",type:"console",reportedState:{state:"connected",name:"Gateway",mac:"aa:bb:cc:dd:ee:ff",ipAddrs:["192.168.6.1"],directConnectDomain:"console.id.ui.direct"}}]}:
   path.endsWith("/sites")?{data:[{id:site,name:"Default"}],totalCount:1}:
   path.endsWith("/devices")?{data:[{id:gateway,name:"Root gateway",macAddress:"aa:bb:cc:dd:ee:ff"}],totalCount:1}:{};
  return route.fulfill({json:data});
 });
 await page.goto("/");await page.getByRole("button",{name:"Connect display",exact:true}).click();
 if(!options.existing){await page.getByLabel("Wi-Fi network",{exact:true}).selectOption("saved-network");await page.getByLabel("Wi-Fi password",{exact:true}).fill("fixture-password");await page.getByRole("button",{name:"Connect to Wi-Fi",exact:true}).click();}
 await expect(page.locator("#wifi_status")).toContainText("Wi-Fi verified");
 if(options.existing){await expect(page.locator("#unifi_editor")).not.toHaveAttribute("open");await page.getByRole("button",{name:"Set up key & certificate automatically"}).click();return;}
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
 await page.getByLabel("Two-factor or backup code").fill("123456");await page.getByRole("button",{name:"Verify code & create local access"}).click();
 await expect(page.locator("#provision_status")).toContainText("Local access ready");
 const log=await page.evaluate(()=>window.usbRequests.filter(r=>r.command.name==="console_login").map(r=>r.command));expect(log[1].token).toBe("123456");expect(log[1].password).toBe("fixture-console-password");
});
test("push approval automatically finishes local enrollment",async({page})=>{
 await setup(page,{mfa:"push"});await expect(page.locator("#console_auth")).toBeVisible();await signIn(page);
 await page.getByRole("button",{name:"Send code / approval"}).click();await expect(page.locator("#provision_status")).toContainText("Local access ready",{timeout:15000});
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
 await page.getByRole("button",{name:"Send code / approval"}).click();await expect(page.locator("#provision_status")).toContainText("Approve sign-in");
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
