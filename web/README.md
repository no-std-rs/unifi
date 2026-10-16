# UniFi display USB installer

The static site at <https://no-std-rs.github.io/unifi/> installs the
[UniFi ESP32-C3 dashboard](../examples/esp32c3-unifi-display/README.md) on the
4 MB GUITION ESP32-2424S012-V1.0. It bundles pinned ESP Web Tools 10.4.0 and
uses Web Serial; no third-party runtime scripts, analytics, or application server
are involved. Other browsers can download the verified factory binary.

The public image contains no credentials. Wi-Fi settings, controller integration
URL, API key, trusted leaf-certificate fingerprint, and optional site/device UUIDs
stay in browser memory and local Blob URLs until written directly over USB.
Prepared USB blobs are discarded on form changes and form fields reset on
navigation. Values are never uploaded or saved in browser storage.
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

Browser tests inspect the actual USB manifest/settings Blob, verify keys and
passwords never enter network requests, reject corrupt downloads, preserve the
verified bytes across publication changes, and exercise mobile/unsupported
browser behavior. They do not flash physical hardware.

Adapted from `no-std-rs/ubus-zero` commit
`051b507b5a41a7207385381e498202516bb8094d`, retaining Apache-2.0; see
[LICENSE](LICENSE). References: [ESP Web Tools](https://esphome.github.io/esp-web-tools/),
[Web Serial](https://developer.chrome.com/docs/capabilities/serial).
