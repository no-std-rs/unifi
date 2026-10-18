# UniFi display USB installer

The static site at <https://no-std-rs.github.io/unifi/> installs the
[UniFi ESP32-C3 dashboard](../examples/esp32c3-unifi-display/README.md) on the
4 MB GUITION ESP32-2424S012-V1.0. It bundles pinned ESP Web Tools 10.4.0 and
uses Web Serial. There are no third-party runtime scripts, analytics, or hosted
application servers. The public installer needs only GitHub Pages and a USB connection; it never connects to a localhost service. Other browsers can download the verified factory binary.

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
entered manually or read from saved device settings.
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
With a connected display running current firmware, discovery verifies the chosen
gateway automatically. The display joins the selected Wi-Fi, connects to its
cloud-reported LAN address with the console-specific `ui.direct` TLS name, verifies
the public CA chain/name/validity, and returns the leaf-certificate SHA-256. No
fingerprint copying is needed. The browser supplies its current time for this
initial verification; dashboard polling subsequently uses the enrolled pin.

Saved local keys are checked through the display. Otherwise **Sign in & create
local access** signs in to the console over pinned LAN TLS, handles authenticator
codes or push/email/SMS approval, creates one **UniFi ESP32 Display** key, and
verifies all four dashboard reads. Retry verification retains a newly created key;
an uncertain creation attempt never silently creates another. Passwords, MFA codes
and console session cookies are temporary and excluded from browser storage and
device flash. The local integration key and trusted pin are remembered normally.
Saved cloud keys resume discovery automatically once Wi-Fi is verified. A failed
discovery stays available to retry instead of repeating on every Wi-Fi check.
Push/email/SMS challenges send automatically; authenticator and backup codes show
only code entry. Switching methods cancels pending polling, and a failed push
check retries the same approval. **Restart sign-in** clears the pending login and
starts a fresh certificate check when an account or approval needs replacing.
The console login is separate authorization: the cloud key alone cannot create a
local key. It stays in the browser and is excluded from USB commands/settings.
The device remains connected to the tested Wi-Fi during provisioning and restores
its saved radio settings when the USB session ends. Firmware without the
`console_provision` capability offers an update before sending login commands.
Changing or rescanning Wi-Fi clears pending console sign-in and approval. After a
successful Wi-Fi recheck, an already verified cloud selection automatically starts
a fresh certificate check before sign-in can continue.
Already configured displays also show **Set up key & certificate automatically**
outside the collapsed manual settings. It opens enrollment and starts discovery
with the saved cloud key, or focuses the key input when none was saved.

**Advanced local settings** remains available for self-hosted controllers,
self-signed certificates, consoles without a direct-connect identity or a unique
LAN address, and manual keys. Enter/import a trusted local connection in that
mode. All setup runs on GitHub Pages plus USB; no local provisioning service runs.
The browser's normal TLS validation handles the cloud API; no cloud certificate
pin is shipped to the display and no periodic cloud-pin publication is needed.

Firmware rejects cloud Connector URLs and public controller IPv4 addresses. DNS
names remain supported, but their resolved address must be private (RFC1918),
link-local, or shared-address space before opening TCP/TLS. There is no cloud
fallback. The firmware still verifies the exact local certificate and its TLS
handshake signature. Reinstall settings if that local certificate rotates.
**Enter local settings manually** bypasses cloud discovery altogether.

The main setup is one panel: **Display → Wi-Fi → UniFi**. Connecting reads the
device settings and scans nearby 2.4 GHz networks automatically. A selected SSID
uses its remembered password and starts an association/DHCP test without another
button. New secured networks reveal the password input. Failed tests keep the
input and **Try again** beside the selected network; **Change password** is also
available after a successful test. Open networks test with an empty password.

Up to 16 exact-SSID profiles retain passwords and local UniFi settings. A failed
test never replaces the last working profile password. Switching to a network
without saved local settings clears the previous network's API key, pin and IDs.
A successful **Save and restart display** also remembers its local connection.
Opt-out and forget remove all saved network credentials, including the separate
profile store. USB save requires a successful test of the current Wi-Fi draft.
The test verifies Wi-Fi and DHCP, not the local API key or certificate.

Firmware installation, offline preparation and device diagnostics are in
**Firmware & other options**. Offline preparation preserves the original
configuration-plus-firmware flash path and cannot test a physical connection.
Local connection JSON can be imported inline; only the five local fields are
accepted, excluding cloud and Wi-Fi credentials. No password or API key is logged.

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
