# unifi

Allocation-free `#![no_std]` Rust bindings for UniFi **Network 10.6.106**, **Site Manager 1.0.0**, and **Protect 7.3.70**. The pinned official schemas expose 161 operations, including writes. Protect coverage includes every endpoint in the older 5.3.48 schema used by `unified`. Another 65 community-derived Network session endpoints are available in `session_api`; those endpoints are undocumented and firmware dependent.

CLI parity with both `unifly` and `unified` is in progress. See the [captured command inventory and remaining work](docs/parity/README.md). The current implementation does **not** claim complete parity.

The default library uses `reqwless`, `heapless`, and the embedded I/O traits. It has no allocator, runtime, DNS, TLS, or OS requirement. Optional serde integration also works without `std` or `alloc`. Development tools and the host verification example use the host standard library.

## Rust API

Operation constructors use the official `operationId` converted to snake case. Construction never sends a request. A `Client` authenticates and routes operations over a caller-provided connected stream implementing `embedded_io_async::{Read, Write}`. Provide a verified TLS stream for a real controller; reconnect for every request. The client sends `Connection: close`, does not follow redirects, and does not retry requests.

```rust,no_run
use unifi::{Client, network};

async fn list_sites<IO: embedded_io_async::Read + embedded_io_async::Write>(
    io: &mut IO,
    key: &str,
) {
    let client = Client::new("192.0.2.1", "/proxy/network/integration", key).unwrap();
    let call = network::get_site_overview_page().unwrap().page(0, 10).unwrap();
    let mut headers = [0; 4096];
    let mut body = [0; 16384];
    let response = client.execute(io, &call, None, &mut headers, &mut body)
        .await.unwrap().success().unwrap();
    let page = network::models::SiteOverviewPage::from_json(response.json().unwrap()).unwrap();
    for site in page.data().unwrap().iter() {
        let name = site.unwrap().name().unwrap().decode::<128>().unwrap();
        // Use name here. It lives in a fixed-capacity heapless::String.
        let _ = name;
    }
}
```

Model views borrow the response buffer, independent of how many objects it contains. Getters check field presence and types when accessed. `JsonString::as_str()` borrows an unescaped string; `decode::<N>()` handles escapes and Unicode in a bounded string. Arrays iterate without allocating. Unknown fields and polymorphic variant fields remain accessible through `.json()`, and concrete variant views are generated too. Parsing validates the entire JSON document with a nesting limit of 64. Views do not eagerly validate every semantic schema constraint.

Each object schema also has a body builder. Builders validate required fields, nested models, array elements, discriminator variants, enums, constants and declared range/size constraints. `extra_field` supports variant fields and explicit `null`; `finish()` validates the complete body. `Model::validate(Json)` offers explicit validation for response views. Formats such as UUID/date-time remain schema annotations; views stay lazy and raw request bodies remain caller-controlled. Build the entire body before sending it:

```rust
use unifi::{Json, network::models::DeviceRestartRequest};
let mut bytes = [0; 128];
let mut request = DeviceRestartRequest::builder(&mut bytes).unwrap();
request.action("RESTART").unwrap();
let body = request.finish().unwrap();
assert_eq!(body, br#"{"action":"RESTART"}"#);
// Pass Some(body) to Client::execute only when you intend to perform this action.
```

For custom bodies, use `json::ObjectWriter`, implement `json::Encode`, or enable the `serde` feature and use `json::encode_serde`. All use caller-owned output buffers. Raw, validated `Json` values support arbitrary nested fields without allocation.

Use `.query("name", "value")` for other documented query parameters, including repeated `hostIds[]`. Query names are checked against the operation; values are percent-encoded. Use `.filter("name.eq('example')")` for Network filters. Offset/limit pagination is explicit: continue until `offset + count >= totalCount`; cloud pagination uses `nextToken`. Capacities and semantic server limits remain the caller's responsibility. No endpoint silently fetches extra pages.

`execute` returns HTTP status, raw response bytes and `Retry-After` for successful and error HTTP responses. Call `.success()` to require 2xx. Error bodies remain available even when they are HTML or plain text. Buffer exhaustion, malformed framing and compressed responses return errors; responses are never silently truncated.

For cloud access, use host `api.ui.com`, base `""` for Site Manager, and base `/v1/connector/consoles/{consoleId}/proxy/network/integration` for Network. Discover the console ID using `site_manager::list_hosts()`. The cloud connector's wildcard operations are also bound, with each path segment encoded separately.

## Linux CLI

The CLI is a separate `no_std`, allocation-free Linux binary using `rustix` and `argtokens`. It links the platform C startup/runtime for startup and memory intrinsics. It has no Rust standard library or allocator.

```sh
cargo build -p unifi-cli
./target/debug/unifi wifi list
./target/debug/unifi devices list --output json
./target/debug/unifi cloud hosts
./target/debug/unifi protect cameras list --controller https://controller.example
./target/debug/unifi protect subscribe protect-events --controller https://controller.example
./target/debug/unifi protect cameras snapshot CAMERA --save snapshot.jpg
./target/debug/unifi config cloud-setup --profile cloud
./target/debug/unifi tui
./target/debug/unifi operations protect
```

Credentials and URL settings come from command options, `UNIFI_*` environment variables, `.env`, and JSON profiles. Environment values override `.env`; profiles provide fallback controller/site/console/API key values. Without a URL, the CLI uses `https://api.ui.com`, discovers a unique connected console, then a unique site. Use `--console` and `--site` to disambiguate. Profiles default to `$XDG_CONFIG_HOME/unifi/config.json` or `$HOME/.config/unifi/config.json`; `--config` overrides that path. `config set profiles.home.controller URL` updates a field; `config use home` selects a profile. Configuration output redacts credential fields. `config set-password [PROFILE] --secret-kind api-key|password|both` stores exported credentials or prompts with terminal echo disabled. Profiles retain `keyring:PROFILE/api-key` or `keyring:PROFILE/password` references; reads resolve these through [`/usr/bin/secret-tool`](https://github.com/GNOME/libsecret/blob/main/tool/secret-tool.c). The attributes match [unifly’s Secret Service convention](https://github.com/hyperb1iss/unifly/blob/v0.10.0/crates/unifly/src/cli/commands/config_cmd/interactive.rs) (`service=unifly`, `username=PROFILE/KIND`). `UNIFI_SECRET_TOOL` selects a trusted absolute helper path. A desktop Secret Service is required only when keyring references are used. Helpers return exact secret bytes, including stored trailing newlines, matching [libsecret 0.21.4+ pipe output](https://github.com/GNOME/libsecret/blob/0.21.4/tool/secret-tool.c#L178). Settings updates read and merge the current section, preserving existing credentials; clearing a credential requires `--allow-secret-removal`. Settings and VPN JSON output, including saved responses, masks secrets unless `--with-secrets` is requested. `--demo` masks identifying strings even with that flag.

`config init` without connection arguments opens a terminal wizard for API-key, session or hybrid profiles. `config cloud-setup` validates the cloud key with GET-only console/site discovery before saving. Both select the new profile atomically, preserve other profiles, and offer environment references, Secret Service or explicit file storage. Headless cloud setup uses exported credentials and accepts `--console` and `--site`; `--credential-storage env|keyring|file` makes storage explicit. Profiles support `apiKeyEnv`, `passwordEnv` and `totpEnv` variable names, plus saved TLS/CA/timeout settings. Explicit connection flags override those defaults, including `--insecure false`.

Resource commands default to tables and normalize paginated resource responses to arrays. The `network ... list` compatibility namespace retains page metadata in JSON/YAML output unless `--hide-page` is supplied; it accepts Unified’s `--page-limit` and `--page-offset` aliases. Command help and completions show resource-specific options; `help connection` lists the advanced connection options. `--all` aggregates every page with a bounded 1 MiB result and rejects stalled pagination. `--output` also accepts `json`, `json-compact`, `yaml`, and `plain`. `--color auto|always|never` controls table headers, with plain/nord/dracula/silkcircuit-neon accents selected through `--theme` or saved configuration. `tui` offers ten refreshing resource views, scroll/search, and a quoted command palette. It invokes this executable directly, inherits controller/profile context and the explicit write guard, and restores terminal state on exit or handled signals. `--log-file` records timestamps and exit status, without command arguments or response contents. `--no-cache` is accepted; resource reads are always fresh. Streams emit compact JSON messages by default; `--count N` bounds a subscription. `--keep-alive-interval` accepts durations such as `250ms` or `1m30s`, independently of the connection/handshake timeout. Shell completions are available for bash, zsh, fish, PowerShell and Elvish. `call SURFACE OPERATION` remains available for exact schema operation names; `--param NAME=VALUE` binds path parameters and `--query NAME=VALUE` appends documented queries.

All non-GET operations require `--allow-write`, including read-like POSTs and session login/logout. `--body FILE` and `--from-file FILE` accept JSONC comments and trailing commas; inline `--data` remains strict JSON; Protect `files upload FILE` builds a multipart asset upload. Live verification only uses GET requests. Uploads and mutations are tested against local mocks. Protect stream commands accept `--low`, `--medium`, `--high`, and `--package`. Snapshot `--low-quality` controls the API request; explicit `--jpeg-quality 1..100` uses the optional system ImageMagick `/usr/bin/convert` encoder. `--id-only` projects listing results to IDs.

HTTPS uses `/usr/bin/openssl` with the system trust store by default, providing DNS and RSA certificate support. `--ca-file PEM` adds an explicit trust file and `--connect IP` overrides the connection address while retaining hostname verification. Rust code and its buffers remain allocation-free; the external TLS process uses its own memory. The optional `--tls embedded --ca-cert DER` backend performs TLS 1.3 verification in-process for ECDSA/Ed25519 certificate chains; it needs an IP URL or `--connect IP`. Subscriptions currently use the system TLS backend. Plain HTTP is limited to loopback mocks.

CLI limits include 128 arguments, 16 path parameters, 32 queries, 8 KiB environment/dotenv data, 128 KiB JSON bodies, 256 KiB upload files, 8 KiB headers, 64 KiB profile files and 1 MiB responses/messages and combined pagination results. Port workflows also bound individual device/network records and JSONC files to 128 KiB, with at most 1,024 ports and 64 tagged networks per patch. Four fixed 1 MiB working buffers live in process storage rather than on the stack. Capacity failures are explicit. Use the library with caller-selected buffers for other requirements. Protect WebSockets require direct controller access: UniFi's cloud connector does not support upgrades.

Cloud API keys also reach legacy Network commands through the console connector, resolving Integration site IDs/names to internal site references. Cookies, login/logout and WebSocket upgrades require a direct controller. For direct legacy commands, `session login --controller URL --allow-write` reads `UNIFI_USERNAME`, `UNIFI_PASSWORD`, and an optional fresh `UNIFI_TOTP`. It stores cookies and CSRF state in a mode-0600, origin-bound profile cache; `--cookie-file` overrides that location. Subsequent session reads do not log in automatically. Cached sessions capture rotated cookies and CSRF tokens between requests, including within update workflows; exported cookie/token pairs are never persisted. An explicitly selected missing cache is an error. Use `--standalone` for controllers without UniFi OS and `session logout --allow-write` to invalidate and remove the cache. Never commit credential files. Session commands include neighboring APs, channels, events, alarms, backups, statistics, legacy NAT/firewall/VPN operations, and explicit raw `api PATH --method METHOD --data JSON` requests.

Device and client selectors accept IDs, names, IPs or MACs; lookups follow all pages and reject ambiguous matches. Adopted-device MAC inputs are normalized. `--field KEY=JSON` adds a raw JSON body field, and common action/voucher/admin flags provide bounded payload helpers. `clients set-ip MAC --ip IPV4 [--network NAME]` resolves a reservation network, and `clients remove-ip MAC [--network NAME]` clears it. `topology` follows all resource pages and adopted-device uplinks. `devices ports DEVICE --with-clients` reads port state; `ports-export DEVICE [--all] [--with-clients]` produces an editable JSON file; observations under `last_seen` are ignored during reimport. `devices port-set DEVICE PORT` accepts name/mode/VLAN/PoE/speed flags or `--reset`; `--from-file FILE` applies a JSONC batch. Updates preserve untouched overrides and validate the complete batch before writing. Wi-Fi, networks, DNS, traffic lists, policies, ACLs, NAT and firewall groups have typed convenience options, with schema validation for official payloads. VPN family commands accept JSON body files, apply purpose/enabled defaults and preserve existing fields on update and check the VPN family before updates/deletion; peer commands encode the WireGuard batch protocol. DPI queries select station statistics for MAC filters and try modern traffic-flow statistics before legacy site/DPI fallbacks. Full-body files remain necessary for some complex create/update workflows.

`Client::execute_with` accepts JSON or arbitrary binary bodies with explicit content types and a response-header observer. `multipart::Multipart` encodes a Protect file upload into a caller-owned buffer. `Authentication::Session` sends cookies and CSRF tokens; `session::Session::from_cookie` restores a caller-owned pair, and the session jar can capture header updates and `session::login_body` builds login/MFA payloads. Clone outgoing session state when the response observer updates the original jar. State is host-scoped and has no automatic login, expiration timer, persistence or retries. Validate legacy HTTP-200 error envelopes using `session::data`. `Call::raw` supports explicit firmware-specific paths and queries.

`websocket::connect` authenticates an HTTP upgrade over a supplied transport. The resulting stream uses caller-owned message buffers, masks outgoing frames, assembles fragments, answers pings, and validates UTF-8 and close frames. Supply fresh random nonce/mask bytes. Protocol, I/O and capacity errors poison the stream so callers can reconnect explicitly. Compression and subprotocols are not negotiated.

The host-only example uses the library's actual HTTP implementation over a verified OpenSSL subprocess, so it can validate cloud endpoints with the platform trust store and RSA support. It discovers the connected console and prints statuses/counts, without printing keys or identifiers:

```sh
cargo run --example cloud_read
# Read AP states, radio retries, client counts and SSID settings through the Rust API:
cargo run --example wifi_status
```

`wifi_status` follows pagination for cloud hosts and Network resources, uses the generated model views, and prints a filtered Wi-Fi report. Its shared host TLS adapter permits GET requests only. Passwords, API keys and device identifiers are omitted from the report.

## Local tests and mock

No official hosted sandbox was identified in the reviewed Ubiquiti docs or the linked GitHub schema projects. The checked-in schema mock serves synthetic responses, validates request bodies (including the top-level discriminator), and never connects to a controller. It is a contract mock, not a stateful UniFi emulator. Writes can be exercised here without touching a live system.

```sh
python3 -m venv /tmp/unifi-test-env
/tmp/unifi-test-env/bin/pip install -r tools/requirements-test.txt
/tmp/unifi-test-env/bin/python tools/mock_server.py --port 4010
# In another terminal:
UNIFI_API_KEY=mock ./target/debug/unifi call network getInfo --url http://127.0.0.1:4010
```

An alternative is [Prism](https://github.com/stoplightio/prism) against a pinned OpenAPI file. A disposable real [UniFi OS Server installation](https://help.ui.com/hc/en-us/articles/34210126298775-Self-Hosting-UniFi) is needed to validate device/controller behavior beyond schemas; a schema mock cannot establish that.

```sh
cargo test
cargo test --all-features
cargo check --lib --target riscv32imac-unknown-none-elf
cargo check --lib --all-features --target riscv32imac-unknown-none-elf
cargo build -p unifi-cli
cargo clippy -p unifi --all-targets --all-features -- -D warnings
cargo clippy -p unifi-cli --bin unifi -- -D warnings
python3 tools/generate.py --check
python3 tools/generate-cli.py --check
python3 tools/generate-request-fixtures.py --check
python3 -m unittest discover -s tests -p 'test_*.py' -v
```

Lint the library and CLI separately: a workspace-wide all-target/all-feature lint would unify the library’s host-only `std` development dependencies into the `no_std` executable.

Rust contract tests check every pinned operation against its source schema and exercise real `reqwless` encoding/decoding over fragmented in-memory I/O, including writes. An independently checked corpus covers all 50 official JSON-body operations with 174 valid and 4,428 invalid fixtures; documented upstream schema inconsistencies are interpreted explicitly. Python tests cover the CLI over loopback HTTP/TLS, request validation, captured command/option recognition and Git worktree safety. No tests automatically contact UniFi. The live example is an explicit GET-only command.

## Git fetch maintenance

`python3 tools/install-git-hooks.py` enables `fetch.prune` and the repository's `reference-transaction` hook. An ordinary `git fetch` then prunes stale remote refs and fast-forwards local `main` to its configured upstream (default `origin/main`) when `main` is not checked out in any worktree. Local commits/divergence are preserved; a compare-and-swap ref update protects concurrent ref changes. Existing hook configurations are not overwritten. The helper `python3 tools/git-sync.py [remote]` also works explicitly. The hook requires this source directory to remain present.

Fetch maintenance is installed in the development worktree. Public clones can enable it explicitly using the installer above.

## Schema provenance

The authoritative inputs are [Ubiquiti's Network OpenAPI document](https://developer.ui.com/network/v10.6.106/openapi.json) , [Site Manager OpenAPI document](https://developer.ui.com/site-manager/v1.0.0/openapi.json), and [Protect OpenAPI document](https://developer.ui.com/protect/v7.3.70/openapi.json), downloaded on 2026-10-16. See [schema provenance](schemas/README.md) for hashes and [live verification](docs/verification.md) for observed controller behavior.

GitHub research included [beezly/unifi-apis](https://github.com/beezly/unifi-apis) and [ubiquiti-community/unifi-api](https://github.com/ubiquiti-community/unifi-api); bindings are generated from Ubiquiti's own documents rather than those mirrors. `python3 tools/generate.py` regenerates everything offline. Updating a schema requires regeneration and the contract checks above.

The implementation is reviewed as a [self-contained PR stack](docs/stack.md), with [Runnerless enrollment](docs/runnerless.md) separate from build CI. `python3 tools/ci-native.py` runs all applicable native checks.
