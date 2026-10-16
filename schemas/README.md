# Pinned official schemas
Retrieved 2026-10-16. These upstream API documents are retained verbatim; their metadata and any upstream inconsistencies are preserved.
- [network.json](https://developer.ui.com/network/v10.6.106/openapi.json): SHA-256 `c91054d31bcefa38d7408cb12a975dd660a7304017d781ffdbc884caaf84c75d`.
- [site-manager.json](https://developer.ui.com/site-manager/v1.0.0/openapi.json): SHA-256 `16daa37f17c9489a6f579d88611bd5827232f65be603125e3c707bb4ef30ad9f`.

The Site Manager document contains nested arrays inside some string `enum` declarations. Those cannot describe a valid string enum; the generator preserves the source and leaves those malformed scalar enum constraints unenforced. Model views always expose the actual string value.

Network publishes some integer enums as quoted decimal strings (Wi-Fi basic data rates and PoE types); bindings validate their numeric wire values, consistent with the declared types and examples. The three named firewall protocol models publish uppercase discriminator constants alongside lowercase wire-name enums. Bindings dispatch those variants using the declared lowercase names, including punctuation; enum names without a mapping receive base-model validation. Protect liveview request wrappers combine inherited fields with `additionalProperties: false` but omit the inherited names from the wrapper. Bindings allow the flattened inherited fields. These interpretations are also explicit in the independent Python contract mock; pinned source documents remain unchanged.

Network also states some null allowances only in descriptions (for example, “When null” or “If this field is omitted or null”). Generation restores those explicit allowances in memory, including inherited properties, references, and enums. Other values retain their type, range, and enum constraints; the pinned document remains unchanged.

`tools/generate-request-fixtures.py` checks synthetic valid and invalid bodies with Python's independent JSON Schema validator and emits a Rust corpus for every official JSON-body operation. CI checks both reproducibility and SDK agreement with that corpus. It exercises required-only and optional-field payloads and each top-level discriminator variant; it is not exhaustive proof of every nested combination or controller behavior.

There is no generator-time or build-time access to a controller or API key. Nested inline Site Manager objects are promoted to named views. Network `allOf` inheritance is flattened into the model getters and builders. Discriminator variants retain their own concrete model views; unknown properties remain accessible as borrowed JSON.

Protect schemas were retrieved from official documentation on the same date.
- [protect.json](https://developer.ui.com/protect/v7.3.70/openapi.json): SHA-256 `5724feee6f48b88f8b28f692e85215f26035b4227c21d68387d024ed05be10c8`. JSON formatting is normalized; schema content is preserved.
- [protect-5.3.48.json](https://developer.ui.com/protect/v5.3.48/openapi.json): SHA-256 `820c4558bdeaba3bb7d3e699b1a2efe5100e709435e99052ac06d0daaa10e229`. JSON formatting is normalized; schema content is preserved.

Protect omits `operationId`; names are derived from HTTP method and path, such as `get_cameras_by_id`. Nullable scalar unions remain borrowed `Json` values. Enum and const constraints apply independently of nullable type declarations: `null` must also be an allowed enum/const value to pass explicit validation. The generated models do not enforce cross-field schema rules.

`session-api.json` is a community endpoint inventory, **not an official schema**. It records routes used by [unifly v0.10.0](https://github.com/hyperb1iss/unifly/tree/v0.10.0/crates/unifly-api/src/session). Its response shapes are intentionally loose because firmware versions differ. Legacy health, WLAN configuration and NAT listing paths have returned HTTP 200 through the cloud API-key connector; direct cookie authentication and the other session endpoints remain unverified live. See [verification](../docs/verification.md).

Session v2 responses are bare JSON rather than the legacy `meta`/`data` envelope. The `x-value-from-path` parameter extension repeats path identifiers in required queries; fixed string `const` query values are also bound by constructors. Network event WebSockets require `Authentication::Session` with a controller cookie; API keys only cover the HTTP surface.

The `x-constructor-default` extension supplies the pinned client's required traffic-flow queries (`period=DAY`, `top=30`); callers can replace them with `set_query`. These are client choices, not claims about controller defaults. UniFi OS login returns a bare administrator object. Both logout requests are bodyless, and their response bodies are left unspecified.
