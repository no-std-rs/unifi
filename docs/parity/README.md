# CLI parity tracking

The target is all functionality from both CLIs installed on the reference machine. `reference-cli.json` records help-only command/option discovery on 2026-10-16: **184 unifly actions**, **54 unified actions**, and **138 distinct option names**, including default group actions. No reference command contacted a controller during capture. A prose-only `brew --prefix` mention was removed from the completion option inventory after checking the installed help.

References: [unifly 0.10.0](https://github.com/hyperb1iss/unifly/tree/v0.10.0) (`b565654`) and [unified](https://github.com/ClifHouck/unified/tree/848139ccc34080e8df00c2eac471f324664d8861).

Implemented foundations:

- All 73 official Network, 14 Site Manager, and 74 Protect endpoint bindings; Protect includes all 35 operations in unified's 5.3.48 baseline.
- Resource commands for those APIs, JSON body files, multipart assets, binary snapshots, direct Protect subscriptions, and explicit write opt-in.
- Readable tables, JSON/compact JSON/YAML/plain output, context-aware bash/zsh/fish/PowerShell/Elvish completions, JSON profiles, cloud console/site discovery, and verified TLS.
- Core session credentials, cookie/CSRF capture, login body encoding, 65 legacy endpoint bindings, and raw requests.
- Schema-checked Wi-Fi, network, DNS, traffic-list, firewall policy and ACL helpers; preserving updates, policy ordering, legacy NAT/firewall groups, structured cloud ISP queries, and local legacy filters.
- Port exports can include read-only client/device observations; reimport ignores those annotations and preserves unrelated port overrides.
- VPN family create/update defaults, preserving updates, WireGuard batch bodies and peer selectors, VPN setting aliases/exported-field patches, and firmware-aware station/site DPI queries.
- Guided local/cloud setup with GET-only discovery, credential storage choices, atomic active-profile creation, profile TLS defaults and custom credential environment references; JSONC body files and cloud API-key access to legacy Network paths.
- Session rotation between workflow requests, command-specific help/completions, Unified page flags/metadata and viewer liveview selection, and fractional/compound subscription intervals.

Remaining parity work (not complete):

- All captured action paths have routes or local workflows, and every captured option name passes an offline parser check. Recognition alone does not establish complete behavioral equivalence; convenience workflows are checked against local contracts, with further cross-version comparison remaining.
- Keyring references, explicit login/logout, MFA, origin-bound caches and rotation are implemented. Automatic credential refresh/reauthentication and session behavior across controller versions remain under comparison.
- The interactive dashboard, search, scrolling, command palette and themed output are implemented. Local/cloud setup and terminal cancellation are tested; exact reference dashboard widgets remain under comparison; profiles currently use this project's JSON format.
- Cross-version and real-hardware validation. No live mutations are authorized or tested.

An endpoint binding or raw API escape hatch does not establish CLI workflow parity. Keep this list explicit while the implementation PR is a draft.
