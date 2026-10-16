# Runnerless review check

The root `.runnerless-ci.ts` is the trusted-main program for the `codex-review` workflow. PR, review, comment, thread and check-run events refresh the `Codex review` check for open PRs. Non-PR and terminal-PR events are ignored. This reports review policy; GitHub Actions verifies Rust builds and tests separately.

Repository registration requires a signed-in Runnerless organization administrator. The GitHub App must cover `no-std-rs/unifi`. Configuration in Git does not enroll the repository by itself.

Use the organization's ID with the CLI's browser device login, requesting only `repositories:read repositories:write`, then register the repository:

```sh
runl auth login --organization ORG_ID --scope 'repositories:read repositories:write' --expires-in 3600
runl repositories add no-std-rs/unifi --github-id 1421479429
```

Complete the browser confirmation and any hidden credential prompt. Never place tokens in arguments, source files or PR descriptions. Device codes expire; start a fresh login when an administrator is available. Enrollment and the resulting check must be verified before treating Runnerless as active or requiring its check in branch protection.
