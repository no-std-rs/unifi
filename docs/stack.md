# Review order

Each PR targets the previous branch and can be checked without files from later PRs. CI runs every applicable check in the current checkout; schema imports receive JSON structure checks before their bindings are added. Cross-validation tools, the schema request corpus, and live GET-only examples are introduced last.

1. CI and Git fetch maintenance.
2. Runnerless review configuration and enrollment instructions.
3. Allocation-free HTTP/JSON/session/WebSocket core and focused unit tests.
4. Site Manager schema import.
5. Site Manager generated SDK and offline generator.
6. Network schema import.
7. Network generated SDK.
8. Protect schema import.
9. Protect generated SDK.
10. Experimental community session inventory and bindings.
11. Linux CLI and offline smoke tests.
12. Older Protect schema used only as a compatibility reference.
13. Cross-validation: contract mock, independent request corpus, CLI parity/integration tests, and GET-only live examples.

CI and the Runnerless configuration already existed in the bootstrap on main; the first two PRs refine and document that setup without rewriting main. The original implementation PR is retained as the final cross-validation PR so its discussion is preserved. Merge from bottom to top; no merges are performed as part of the split.
