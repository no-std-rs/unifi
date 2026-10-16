# unifi

Allocation-free `#![no_std]` Rust access to UniFi over caller-provided embedded I/O streams. The library uses `reqwless` and `heapless`; optional serde also works without `std` or `alloc`.

The core supports bounded JSON, typed operation metadata, percent-encoded paths and queries, explicit pagination, status/error preservation, binary/multipart bodies, session cookie/CSRF state, and WebSockets. Requests use caller-owned buffers. Provide verified TLS and reconnect for each request; the core does not retry or log in automatically.

```rust
use unifi::{Call, Json, Method};
let call = Call::raw(Method::GET, "/v1/info").unwrap();
assert_eq!(call.path(), "/v1/info");
assert!(Json::parse(br#"{"ready":true}"#).unwrap().required("ready").unwrap().boolean().unwrap());
```

Only send a request when you intend to perform its operation. Model constructors and body builders do not send network traffic.

Available generated surfaces:

- `site_manager`: 14 operations; [pinned source and provenance](schemas/README.md).
- `network`: 73 operations; [pinned source and provenance](schemas/README.md).
- `protect`: 74 operations; [pinned source and provenance](schemas/README.md).
- `session_api`: 65 operations; [pinned source and provenance](schemas/README.md).

`python3 tools/generate.py --check` verifies the checked-in generated modules. Use `--surface NAME` to generate a new surface explicitly.

The Linux `unifi` CLI uses `rustix` and `argtokens` without Rust `std` or an allocator. Build with `cargo build -p unifi-cli`; `unifi --help` lists commands. HTTPS uses verified system OpenSSL by default, with an optional embedded TLS backend. Read connection settings from options, environment, `.env`, or profiles. See the [CLI inventory](docs/parity/README.md).

Every non-GET operation requires `--allow-write`, including session login and read-like POST queries. Never test live writes without explicit authorization. CLI limits fail explicitly instead of truncating requests or responses.

Run `python3 tools/ci-native.py` for the checks available in this checkout. CI also checks bare-metal default/serde builds on stable Rust 1.99.1 and MSRV 1.91.1. See [Runnerless setup](docs/runnerless.md) and [review stack](docs/stack.md).
