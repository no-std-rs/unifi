# UniFi display USB installer

The static site at <https://no-std-rs.github.io/unifi/> installs the
[UniFi ESP32-C3 dashboard](../examples/esp32c3-unifi-display/README.md) on the
4 MB GUITION ESP32-2424S012-V1.0. It bundles pinned ESP Web Tools 10.4.0 and
uses Web Serial; no third-party runtime scripts, analytics, or application server
are involved. Other browsers can download the verified factory binary.

The public image contains no credentials. Wi-Fi settings, controller integration
URL, API key, trusted leaf-certificate fingerprint, and site/device UUIDs
stay in browser memory and local Blob URLs until written directly over USB.
Prepared USB blobs are discarded on form changes and form fields reset on
navigation. Values are never sent to a site backend or saved in browser storage.
In cloud mode, the API key is sent directly to `https://api.ui.com` in the
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

The default **UniFi cloud Connector** flow accepts a key created in
[Site Manager → Settings → API Keys](https://unifi.ui.com/), discovers online
consoles, paginates sites/devices, and requires an explicit device choice when
there are several. **Verify cloud access** checks the same four GET routes used
by the display's initial sample. Changing the key, console, site, device, or
connection mode invalidates verification and any prepared installation.
The resulting URL is
`https://api.ui.com/v1/connector/consoles/CONSOLE_ID/proxy/network/integration`;
the selected UUIDs are written to the existing settings format. No firmware
credential exchange is involved and no local key is needed. The display needs
internet access. Personal cloud keys can access only consoles their owner owns;
organization keys need access to the console. UniFi OS 5.0.3 or newer is required.
See the [official Connector documentation](https://developer.ui.com/network/v10.6.106/connectorget).

`cloud.json` contains a public SHA-256 leaf-certificate pin obtained by the site
build with OpenSSL CA and hostname verification, using the firmware-compatible
TLS 1.3 cipher/signatures. The browser supplies this pin automatically and
rejects expired metadata. Browsers cannot extract server certificates themselves.
Pinning still verifies both the exact certificate and its handshake signature
on the display. After Ubiquiti rotates the certificate, rerun the main firmware
workflow to refresh the site, then reinstall settings over USB. The display
does not silently replace its trusted pin or bypass TLS checks.

**Local Network controller** preserves direct LAN access with a locally created
Network integration key, integration URL and trusted controller certificate pin.
Cloud discovery cannot retrieve a local key. There is no documented public
Site Manager OAuth authorization flow; this installer uses the supported API-key flow.

From a **clean committed checkout**, build and preview:

```sh
sh tools/build-dashboard-artifact.sh
npm --prefix web ci
npm --prefix web run build
python3 tools/assemble-flasher.py dist/firmware dist/flasher
python3 tools/cloud-certificate.py dist/flasher/cloud.json
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
