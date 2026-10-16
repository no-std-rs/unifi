# Pinned API documents

Official documents are retained without controller access or credentials. Raw imports and generated SDKs are reviewed separately.

- `site-manager.json`: official Site Manager 1.0.0 document from https://developer.ui.com/site-manager/v1.0.0/openapi.json. SHA-256 `16daa37f17c9489a6f579d88611bd5827232f65be603125e3c707bb4ef30ad9f`.
- `network.json`: official Network 10.6.106 document from https://developer.ui.com/network/v10.6.106/openapi.json. SHA-256 `c91054d31bcefa38d7408cb12a975dd660a7304017d781ffdbc884caaf84c75d`.

Network integer enums documented as quoted decimals use numeric wire values. Named firewall protocol discriminators follow the declared lowercase wire-name enums. Inherited allOf fields are flattened into borrowed views and builders; pinned inputs are unchanged.

Network also states some null allowances only in descriptions (for example, “When null” or “If this field is omitted or null”). Generation restores those explicit allowances in memory, including inherited properties, references, and enums. Other values retain their type, range, and enum constraints; the pinned document remains unchanged.
