# UniFi display USB installer

The static site at <https://no-std-rs.github.io/unifi/> installs the
[UniFi ESP32-C3 dashboard](../examples/esp32c3-unifi-display/README.md) on the
4 MB GUITION ESP32-2424S012-V1.0. It bundles pinned ESP Web Tools 10.4.0 and
uses Web Serial. There are no third-party runtime scripts, analytics, or hosted
application servers. An optional local Python helper provides guided provisioning. Other browsers can download the verified factory binary.

The public image contains no credentials. Wi-Fi settings, controller integration
URL, separate cloud/local API keys, trusted local leaf-certificate fingerprint, and site/device UUIDs
are automatically saved in this browser's `localStorage` by default, so they
autofill on reopening the installer. **Remember settings in this browser** can
be turned off to remove saved credentials and use settings only for that visit.
**Forget saved settings** clears the form, credentials, cloud selections and
prepared installation, and leaves remembering off. Only that preference remains
stored. These controls never clear storage belonging to other applications.
Credentials are stored without encryption in the browser profile; use remembering
only on a trusted profile. They persist until forgotten or browser data is cleared,
and do not sync to other browsers or devices through this application. If browser
storage is unavailable, installation still works for the current visit.
Prepared USB blobs are discarded on form changes and navigation. Saved cloud
IDs are only selection hints: discovery reloads available choices and requires
fresh verification. The local certificate fingerprint is restored as the saved trust setting, whether
entered manually or obtained from guided setup.
No access-verification state, firmware bytes, or USB Blob URLs are persisted.
Values are never sent to a site backend.
In cloud discovery mode, the cloud API key is sent directly to `https://api.ui.com` in the
`X-API-Key` header for discovery and verification; Wi-Fi credentials never leave
the browser except over USB. Requests omit cookies and reject redirects.
The 4 KiB `UNIFICF1` settings sector goes to `0x3f0000`; settings
are unencrypted in the device's flash. Firmware validates its header, length,
CRC-32 and fields before starting Wi-Fi. The image is specific to the named board;
the USB dialog can check the chip, but cannot establish the physical wiring.

`latest.json` identifies the application, settings schema, source revision,
size and SHA-256 of the factory binary. The page verifies both metadata and bytes
before enabling preparation. It flashes those exact verified Blob bytes even if
the published main image changes while the page remains open. Dirty builds,
wrong-board metadata, altered bytes, or a mismatched schema are rejected.

`.github/workflows/firmware.yml` runs host/TLS tests, a real C3 cross-build,
configuration tests and Chromium tests on PRs. Only pushes to `main` publish
GitHub Pages. No crates.io publishing credential is required. The deployment job
performs anonymous GETs with an unrelated Origin header and requires wildcard
CORS, the expected main revision, byte length and SHA-256 on the public download.
The current public pointer is <https://no-std-rs.github.io/unifi/latest.json>.

The default **Discover with cloud · device stays local** flow accepts a key
created in [Site Manager → Settings → API Keys](https://unifi.ui.com/), discovers
online consoles, and paginates sites/devices. The root gateway is preferred only
when its MAC matches the console. A remembered explicit device selection takes
precedence; otherwise multiple unmatched devices require a choice.
The installer uses private IPv4 entries from `reportedState.ipAddrs` to suggest
the console's LAN address; it never substitutes the reported WAN address. With
multiple LAN addresses, enter the one reachable from the display's Wi-Fi network.
**Verify cloud access** checks the dashboard's four GET routes through the cloud
Connector. This verifies cloud access, not the separate local credentials.
Personal cloud keys require console ownership; organization keys need access to
the console, and the Connector requires UniFi OS 5.0.3 or newer.

The display always uses a **direct local connection**, even with cloud discovery.
Enter a separate **Local Network API key** created in **Network → Integrations**,
a local integration URL (usually `https://GATEWAY/proxy/network/integration`), and
the trusted local controller leaf-certificate SHA-256 fingerprint. The cloud key
is excluded from the USB settings sector. The optional guided setup passes it to
the helper on your computer for the same cloud discovery requests. Cloud discovery
cannot retrieve or mint a local key or establish trust in a self-signed local
certificate. Obtain the fingerprint through a trusted administrative connection.
Guided setup can instead verify the console’s CA-signed direct-connect certificate
and create a local key after one console sign-in, as described below.
The browser's normal TLS validation handles the cloud API; no cloud certificate
pin is shipped to the display and no periodic cloud-pin publication is needed.

Firmware rejects cloud Connector URLs and public controller IPv4 addresses. DNS
names remain supported, but their resolved address must be private (RFC1918),
link-local, or shared-address space before opening TCP/TLS. There is no cloud
fallback. The firmware still verifies the exact local certificate and its TLS
handshake signature. Reinstall settings if that local certificate rotates.
**Enter local settings manually** bypasses cloud discovery altogether.

**Open guided setup** removes manual certificate copying and key creation:

1. Download **setup-display.py** from the installer and run `python3 setup-display.py`
   (Python 3.10+, standard library only). Keep the terminal open, on the gateway LAN.
2. In the installer, enter your cloud key and choose **Open guided setup**. The
   local window discovers online consoles; choose one and verify its certificate.
3. Sign in to the verified console once. Authenticator/backup codes, email/SMS
   codes and push approval are supported through the console’s own login API.
   Alternatively, paste an existing local key to verify and reuse it.
4. The helper creates one **UniFi ESP32 Display** key and verifies the same four
   LAN API reads used by the firmware. Choose **Use these settings in the installer**;
   the URL, key, fingerprint and site/device IDs fill automatically. Add Wi-Fi and flash.

The console password and two-factor codes are neither saved nor passed to the
installer. The helper keeps temporary login cookies in memory and logs out after
successful verification. A created key inherits the signing-in account’s existing
permissions, has no requested expiry, and can be revoked in **Console → Integrations**.
A failed verification retains the new key in memory so **Retry local verification**
does not create another. A lost key-creation response is not automatically retried;
check Integrations before restarting. Existing keys are never revoked by setup.

The helper binds only `127.0.0.1:4177`. Its APIs require the exact loopback Host,
Origin and a fresh session token; they expose no CORS access. The installer’s popup
handoff checks the helper origin, exact window and one-time channel. It transfers
only local device settings; Wi-Fi never enters the helper. **Forget saved settings**
also cancels an outstanding handoff. If popups are unavailable, download local
settings in the helper and use **Import settings from the helper** in the installer;
that JSON contains a secret API key. Stop the helper with Ctrl-C after use.

Automatic trust requires the console’s cloud-reported `directConnectDomain` under
`ui.direct`, a reported LAN IPv4 address, and a valid system-trusted certificate
for that exact hostname. HTTPS connects directly to the LAN IP with that TLS name,
and pins the verified leaf for the rest of setup. It never accepts an unverified
self-signed certificate. The firmware uses the same hostname and pin; configure
its Wi-Fi to use the gateway DNS, which resolves that hostname locally. A different
DNS server may not resolve it. Consoles without this identity use manual setup.
Certificate rotation requires updating the display settings; rerun setup with the
existing local key to obtain and verify the new certificate.

The key-creation route is the console UI’s private API,
`POST /proxy/users/api/v2/user/self/keys`. The cloud Connector rejects that service
with HTTP 403, even for a key with Network owner access; console authentication is
required. This is a console login, not an OAuth grant to this website. The flow was
traced on UniFi OS 5.1.132 / Network 10.6.106; future UI API changes may require updates.

For a read-only certificate/discovery check using an existing environment key:

```sh
python3 tools/setup-display.py --probe
# Or explicitly read the saved key from a local dotenv file:
python3 tools/setup-display.py --probe --env-file .env
```

The probe never logs in or creates keys. It prints the discovered LAN identity,
certificate fingerprint and site choices, without printing the cloud key.

From a **clean committed checkout**, build and preview:

```sh
sh tools/build-dashboard-artifact.sh
npm --prefix web ci
npm --prefix web run build
python3 tools/assemble-flasher.py dist/firmware dist/flasher
node web/tests/server.mjs
```

Visit <http://127.0.0.1:4173/>. Localhost is a secure context for Web Serial.
There is no compile-time credential input to the firmware build.

```sh
npm --prefix web test
cd web
npx playwright install chromium
npm run test:browser
```

Browser tests inspect the actual USB manifest/settings Blob, verify cloud keys
are sent only to the official API origin and Wi-Fi passwords never enter network
requests, reject corrupt downloads, preserve the
verified bytes across publication changes, and exercise mobile/unsupported
browser behavior. They do not flash physical hardware.

Adapted from `no-std-rs/ubus-zero` commit
`051b507b5a41a7207385381e498202516bb8094d`, retaining Apache-2.0; see
[LICENSE](LICENSE). References: [ESP Web Tools](https://esphome.github.io/esp-web-tools/),
[Web Serial](https://developer.chrome.com/docs/capabilities/serial).
