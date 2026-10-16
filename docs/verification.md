# Verification

Live checks were performed on 2026-10-16 using only GET requests and the API key from the ignored `.env`. The tested Network application reported **10.6.106**, matching the pinned official schema. No live POST, PUT, PATCH or DELETE requests were sent.

The Rust `cloud_read` example exercised the actual `reqwless` client through a verified host OpenSSL transport. Its completed run made 24 GET requests: 22 returned HTTP 200; the two firewall requests returned the documented feature-state error below. Device details and latest statistics were sampled.

| Surface | Operations | Result |
| --- | --- | --- |
| Site Manager | `listHosts`, `listSites`, `listDevices` | HTTP 200 |
| Network | `getInfo`, `getSiteOverviewPage` | HTTP 200 |
| Network | `getAdoptedDeviceOverviewPage`, `getAdoptedDeviceDetails`, `getAdoptedDeviceLatestStatistics` | HTTP 200 |
| Network | `getConnectedClientOverviewPage`, `getNetworksOverviewPage`, `getWifiBroadcastPage` | HTTP 200 |
| Network | `getDnsPolicyPage`, `getAclRulePage`, `getWansOverviewPage` | HTTP 200 |
| Network | `getFirewallZones`, `getFirewallPolicies` | HTTP 400, `api.firewall.zone-based-firewall-not-configured` |

The `wifi_status` example subsequently read the complete Wi-Fi snapshot through the same Rust client: all 15 GET requests returned HTTP 200. It follows host/resource pagination and uses generated model views for AP details, radio statistics, wireless client counts and Wi-Fi broadcasts. Its output omits credentials and device identifiers.

The firewall result reflects the console's current configuration. No configuration was changed to enable that feature. Other endpoints have schema and local transport coverage; they have not all been validated against live hardware.

Local Rust tests compare all 161 official operations and 65 community session routes to the pinned schema and pass fragmented request/response I/O through `reqwless`. They cover method/path/query encoding, required parameters, request bodies, JSON syntax/Unicode, typed views/builders, status/error preservation, rate-limit headers, truncation, conflicting framing, compression rejection and capacity rollback. Optional serde is tested separately.

The library is checked for `riscv32imac-unknown-none-elf` with and without serde. The Linux CLI is built without Rust `std` or `alloc`; its undefined symbols are checked for allocator references. CLI tests use loopback TCP and TLS servers, including certificate/hostname rejection, case-insensitive cloud routing, YAML Unicode, paginated selector ambiguity, session MFA/origin-bound caching and write-body validation. Git tests use disposable repositories to cover pruning, unchecked-out main fast-forwarding, other worktrees and diverged local commits.

The mock produces schema-based synthetic data and does not persist changes or emulate hardware. Passing it establishes contract behavior, not production write semantics. Live writes remain untested pending explicit authorization.

The no_std CLI also successfully read Wi-Fi broadcasts through the cloud connector using its default OpenSSL transport and automatic console/site discovery. No private account inventory is stored in this repository.

The CLI also issued successful GET requests through the cloud API-key connector for legacy Network health, WLAN configuration and NAT policy listing (`api/s/{site}/stat/health`, `api/s/{site}/rest/wlanconf`, `v2/api/site/{site}/nat`). Natural `system health` and `nat policies list` commands were verified too. This validates those paths with cloud API-key authentication; direct cookie login and the other legacy endpoints remain covered by local contracts only. Protect has not been verified against live hardware.

Setup tests drive a real pseudo-terminal through API-key/session profile creation, hidden password entry, cancellation and Ctrl-C, and check terminal restoration and atomic mode-0600 profile writes. Cloud setup discovery, custom credential environment references, JSONC bodies, profile TLS precedence, family-safe VPN deletion, saved secret redaction, traffic-list address families and port mode transitions use loopback mocks.
