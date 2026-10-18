# UniFi ESP32-C3 display

A `no_std` + `alloc` UniFi Network dashboard for the **GUITION
ESP32-2424S012-V1.0**: ESP32-C3, 4 MB flash, round 240 × 240 GC9A01 LCD,
and CST816 capacitive touch. This is the same board and tiled renderer as the
[ubus-zero gadget](https://github.com/no-std-rs/ubus-zero/tree/main/examples/esp32c3-router-display),
using this repository's generated Network API and generic embedded async client.
The SDK itself remains allocator-free; the standalone board example has a 160 KiB
heap for Wi-Fi, TLS, bounded JSON responses, and dashboard state.

## Install

Open the [USB installer](https://no-std-rs.github.io/unifi/) in a desktop
Chromium browser with Web Serial, connect the board using a data-capable USB
cable. Connect, choose Wi-Fi, then discover with your Site Manager cloud key.
The root gateway is selected when its MAC matches the console. The display verifies
the console's public CA chain, cloud-reported `ui.direct` identity and certificate
validity (using the browser's current time), and fills the SHA-256 automatically.
Sign in once in the installer to create a named **UniFi ESP32 Display** local key;
authenticator/backup codes and UniFi Verify push approval are supported. The display
checks all four dashboard reads locally before returning the key and certificate
to the installer. Save and restart to finish. Console passwords/cookies stay in
temporary memory and are never saved in browser storage or flash.

For manual setup or a self-hosted controller, use **Advanced local settings**:

- Wi-Fi name and password (2.4 GHz; an empty password selects an open network).
- The controller's **HTTPS integration base**, usually
  `https://CONTROLLER/proxy/network/integration` on UniFi OS, or
  `https://CONTROLLER:8443/integration` on a self-hosted Network server.
  Automatic setup saves the gateway's LAN IPv4 address and connects without DNS.
  Manually configured DNS names remain supported; IPv6 is not enabled on this board.
- A Network integration API key with access to the intended site and device.
  Dashboard polling uses GET requests. Automatic setup creates the local key
  explicitly through your console session; it does not change network configuration.
- The SHA-256 fingerprint of the controller's **leaf TLS certificate**, as 64
  hexadecimal characters without colons. Obtain the certificate through a
  trusted administrative connection or exported certificate; do not blindly
  trust a certificate fetched over an unverified connection. For an already
  trusted exported certificate: `openssl x509 -in controller.pem -noout -fingerprint -sha256`.
  Remove the colons from that command's fingerprint.
- Optionally, the site and adopted-device UUIDs returned by the Network API.
  Automatic selection works only when exactly one site and one device are
  visible. With multiple choices the display asks for the corresponding UUID;
  it never chooses the first device arbitrarily. IDs are UUIDs, not MAC addresses
  or names.

Alternatively, choose **Discover with cloud · device stays local** in the installer.
A Site Manager API key discovers the console's LAN address and site/device UUIDs,
and prefers the root gateway when its MAC matches the console. Cloud verification
checks the data paths using that cloud key, which stays in the browser.
**The display always uses LAN access**. A cloud key cannot authorize local key
creation by itself: console sign-in supplies that authorization through USB, while
the cloud key stays in the browser. Existing local keys are verified and reused;
missing/rejected keys offer console sign-in. Certificate enrollment and console
requests run on the display over the chosen Wi-Fi. No local helper, installed
application or hosted backend is involved. See [installer setup](../../web/README.md).
Cloud discovery requires UniFi OS 5.0.3 or newer and appropriate console access.
The display itself does not need internet access and has no cloud fallback.

The installer also supports **live USB setup**. For a fresh board, choose
**Install firmware first**, close the flasher, then **Connect display**. Firmware
with no settings keeps USB setup available without a Wi-Fi connection. Existing
boards can connect directly after updating to firmware that supports APP MODE.

While connected, the screen shows **APP MODE**. Setup reads device settings and
scans the C3's 2.4 GHz radio automatically. Choose a discovered SSID or **Other
network…**. A saved password is tested automatically; new networks ask for one
inline. **Change password** and **Try again** stay beside that network. The test
actually associates using the draft password and waits
for DHCP. It then restores the saved radio configuration without writing flash;
disconnect APP MODE to resume its saved connection. A successful test verifies
Wi-Fi and DHCP, not the controller's API key or TLS certificate.

The form reads saved settings from USB automatically (including credentials).
**Save and restart display** validates and verifies a write to the settings
partition, then reboots into the local dashboard without reflashing. The previous
valid sector is retained as a recovery copy if a write is interrupted. Both copies
contain unencrypted credentials. New USB sessions require a fresh nonce handshake;
heartbeat loss leaves APP MODE within 10 seconds. Radio operations have bounded
timeouts and finish restoring the saved configuration before normal reconnects.
If radio cleanup fails, the display restarts using its saved settings.
Disconnect returns to the previous page and lock; reboot resets the page lock.
No cloud key is part of the device protocol or saved configuration. Automatic
local setup temporarily keeps the tested Wi-Fi connected; sign-in state is bound
to the current USB session and discarded when it ends. The saved radio settings
are restored on disconnect or heartbeat expiry. Older APP firmware must be updated
to advertise both `console_provision` and `tls_server_name` capabilities.

Automatic enrollment saves the gateway's numeric LAN URL plus an optional
`tls_server_name` that selects its enrolled certificate on that same LAN socket.
The TLS name is never resolved. Polling needs no public DNS, cloud access or
clock synchronization once the local key and certificate pin are saved.

Prepare the installation, then use the USB dialog. The public factory image has
no credentials. The page verifies its advertised size and SHA-256, then creates
local Blob images for the firmware and settings. Cloud discovery sends only the cloud key to `api.ui.com`; the local Network key
and Wi-Fi credentials only go to the device over USB.
Settings are remembered in this browser by default for your next visit. Turn off
**Remember settings in this browser** or choose **Forget saved settings** to
remove stored credentials. Browser storage is unencrypted; use a trusted browser
profile. Settings are never sent to a site backend, logged, or embedded in the
published binary. Settings
remain **unencrypted in device flash**. Reinstalling replaces firmware/settings;
back up anything else on the board first. Hardware flashing is not part of CI.

TLS uses `embedded-tls` TLS 1.3 with an exact SHA-256 leaf-certificate pin and
CertificateVerify signature verification (P-256/P-384 ECDSA or RSA-PSS
SHA-256/384/512). A matching certificate alone cannot bypass proof of its private
key. Automatic enrollment validates CA chain, hostname and expiration first using
Web PKI and RustCrypto, with no C crypto dependency. Subsequent pinned dashboard
connections need no wall clock. Re-run local setup through USB when the trusted
controller certificate rotates. Servers must offer a compatible TLS 1.3 cipher
suite and signature algorithm. There is no plaintext or skip-verification mode. Cloud Connector URLs and public
IPv4 destinations are rejected, including after DNS resolution.

## Dashboard and polling

Tap to advance among five pages; automatic advance is every eight seconds.
**Triple-tap within 650 ms to lock/unlock the current page.** A small closed gold
padlock means both tap navigation and automatic rotation are stopped; data keeps
updating. Triple-tap again to unlock. One/two taps wait for that gesture window
before advancing. The lock starts unlocked after reboot and does not write flash:


1. Selected-device uplink RX/TX in Mb/s and a 60-sample history.
2. Device CPU, memory, load averages, and uptime.
3. Radio bands and transmit retry percentages.
4. Site connected-client and adopted-device totals.
5. Selected device name, model, firmware, IP address, and state.

These are **device uplink rates**, not WAN speed or a sum of client traffic.
Missing measurements display `--`; no unavailable value is fabricated. Unicode
labels are bounded and clipped for the small built-in font. Failed requests and
reboots leave graph gaps. Errors, samples older than 15 seconds, or an unchanged
reported `lastHeartbeatAt` for more than 45 seconds mark the display stale.

Statistics normally poll every five seconds. Device details and site totals
refresh every 12 successful polls. Optional inaccessible site counts remain
unknown. HTTP 429 stops the sample and backs off using numeric Retry-After
(default 60 seconds, bounded to 5–3600 seconds). DNS, TCP, TLS, headers, and body
share an eight-second deadline per request. Responses are bounded to 16 KiB;
headers to 4 KiB. Each request opens a new verified TLS connection. Wi-Fi
reconnects independently and drawing continues while network requests wait.
The API routes come from the pinned official `schemas/network.json`.

## Board wiring

| Signal | GPIO |
|---|---|
| LCD SCK / MOSI | 6 / 7 |
| LCD DC / CS | 2 / 10 |
| LCD backlight | 3 |
| Touch SDA / SCL | 4 / 5 |
| Touch reset | 1 |

LCD CS stays low. Touch interrupt shares GPIO10, so touch is polled instead.
The LCD uses 40 MHz SPI, software reset, BGR/inversion, and a 16-row RGB565
buffer (7.5 KiB), rather than a full framebuffer. Other ESP32 displays need
board-specific wiring and panel changes; this binary targets only the board above.

The factory image uses DIO / 80 MHz / 4 MB, a factory app at `0x10000` of length
`0x3e0000`, and a settings partition at `0x3f0000` of length `0x10000`.
Only its first 4 KiB sector is written by the installer. Settings contain the
`UNIFICF1` magic, little-endian JSON length and CRC-32, then at most 2048 bytes of
JSON. Firmware validates the partition, sector, and fields before starting Wi-Fi.
Missing or corrupt settings display a USB setup prompt while the USB service remains running. There is no compile-time
credential configuration.

## Build and verify

From the repository root (Rust 1.95.0 and the C3 target are pinned inside the example):

```sh
cargo +1.95.0 test --locked --manifest-path examples/esp32c3-unifi-display/Cargo.toml --lib --test tls
cargo +1.95.0 clippy --locked --manifest-path examples/esp32c3-unifi-display/Cargo.toml --lib --examples --tests -- -D warnings
cargo +1.95.0 fmt --manifest-path examples/esp32c3-unifi-display/Cargo.toml -- --check
cargo install espflash --version 4.4.0 --locked
sh tools/build-dashboard-artifact.sh
```

The build helper enters the board directory, cross-builds the firmware, merges
the bootloader and partition table, and records commit/size/SHA-256 metadata in
`dist/firmware`. It honors `CARGO_TARGET_DIR`. A dirty source tree is marked in
metadata and is rejected by the public installer and publishing workflow.

Render the actual five tiled pages on a host with illustrative data:

```sh
cargo +1.95.0 run --locked --manifest-path examples/esp32c3-unifi-display/Cargo.toml --example preview -- /tmp/unifi-display.ppm
```

Host tests cover configuration, generated request selection, ambiguity, rate
limits, missing statistics, history, rendering, and real loopback TLS using both
RSA and ECDSA certificates. A wrong-pin test verifies no HTTP/API key reaches the
peer. Enrollment tests reject unknown CAs, wrong names and expired certificates;
console tests cover CSRF rotation, legacy MFA, key creation and verification retries
without duplicate keys. The firmware cross-build checks the actual Embassy/ESP connector. Physical
LCD, touch, Wi-Fi and live-controller operation still require the specified board;
host/CI results do not establish hardware operation.

The browser build, tests, and publishing workflow are described in
[web/README.md](../../web/README.md).

## Attribution

Board initialization, panel/tile support, installer, and artifact tooling are
adapted from `no-std-rs/ubus-zero` commit
`051b507b5a41a7207385381e498202516bb8094d`. The adapted example and web installer
retain that project's Apache-2.0 license; see [LICENSE](LICENSE). The UniFi SDK
retains its repository license.
