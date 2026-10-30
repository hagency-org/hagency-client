spec: task
name: "Migrate native client secrets to scoped operating system credentials"
satisfies: [REQ-CLIENT-QUALITY-REMEDIATION]
tags: [active, rust, desktop, security, persistence]
---

## Intent

Repair R05 without changing original encrypted database keys, losing fresh refresh
credentials, or treating unavailable native credentials as permission to fall back
to plaintext. Keep large authentication and revocation documents in authenticated
private envelopes and only small scoped keys in the native credential store.

## Constraints

### Must
- On desktop select explicit macOS Keychain, Windows Credential Manager and Linux Secret Service implementations; absent or inaccessible storage fails closed.
- Migrate legacy secrets only after native write and readback verification, encrypted atomic replacement and parent directory sync; preserve unknown durability outcomes for recovery.
- Bind keys and authenticated payloads to installation, purpose and complete account or Agent identity; preserve the original database passphrase and retained database references.
- Recover interrupted migrations using private nonsecret journals, original source identity and digest, and account epoch and SDK identity fences before publication.
- Keep synchronous SDK save callbacks synchronous and free of native credential calls; record a scoped failure and the latest actual token snapshot if local persistence fails.
- Preserve revoke-only queue progress and Agent key continuity across authentication generations, and refuse orphan database cleanup for missing keys or unknown schemas.
- Defer production startup database deletion until database creators and cleanup share a cross-process protocol; a complete diagnostic snapshot alone cannot authorize deletion of newly created directories.
- Exercise explicit fake backend and real isolated filesystem faults without reading existing native credential namespaces or displaying native prompts.

### Must Not
- Do not use the keyring default backend or silently switch to mock or plaintext storage.
- Do not generate a replacement key for an existing encrypted database when its original native credential is missing.
- Do not delete arbitrary native entries, rollback a published encrypted session to plaintext, or claim that a synchronous callback is awaited asynchronous durability.
- Do not change primary worktrees, real user accounts or existing native credentials, push, or create a pull request before parent review.

## Boundaries

### Allowed Changes
- .github/workflows/app-builds.yml
- .github/workflows/app-main.yml
- .github/workflows/app-release.yml
- .github/workflows/engine-checks.yml
- .github/workflows/ci.yml
- .github/workflows/windows-probe.yml
- ./Cargo.toml
- ./Cargo.lock
- crates/hagency-platform/src/file_identity.rs
- crates/hagency-platform/src/lib.rs
- crates/hagency-store/Cargo.toml
- crates/hagency-store/src/lib.rs
- crates/hagency-store/src/credential_vault.rs
- crates/hagency-store/src/credential_vault/**
- crates/hagency-app/Cargo.toml
- crates/hagency-app/src/persistence/**
- crates/hagency-app/src/accounts.rs
- crates/hagency-app/src/history_store.rs
- crates/hagency-app/src/history_store/**
- crates/hagency-app/src/sliding_sync.rs
- crates/hagency-app/src/account_session.rs
- crates/hagency-app/src/session_auth.rs
- crates/hagency-app/src/app.rs
- crates/hagency-app/src/logout/logout_state_machine.rs
- crates/hagency-app/src/logout/logout_state_machine/tests.rs
- crates/hagency-app/src/logout/logout_confirm_modal.rs
- crates/hagency-app/src/login/login_screen.rs
- crates/hagency-app/src/login/homeserver.rs
- crates/hagency-app/src/login/oauth_tests.rs
- crates/hagency-app/src/lib.rs
- crates/hagency-app/resources/i18n/en.json
- crates/hagency-app/resources/i18n/zh-CN.json
- crates/hagency/README.md
- crates/hagency/README.zh-CN.md
- crates/hagency/Cargo.toml
- crates/hagency/src/console.rs
- crates/hagency/src/console/native.rs
- crates/hagency/src/owner_host.rs
- crates/hagency/src/console/device_execution/encrypted.rs
- crates/hagency/src/console/owned_runtime/http_tests.rs
- crates/hagency/src/console/owned_runtime/http_tests/file_history.rs
- crates/hagency/src/console/owned_runtime/http_tests/joint_files.rs
- crates/hagency/src/console/owned_runtime/http_tests/recovery.rs
- crates/hagency/src/console/owned_runtime/http_tests/remote_approvals.rs
- crates/hagency/src/console/native/configuration.rs
- crates/hagency/src/console/native/configuration/http_tests.rs
- crates/hagency/src/server_admission.rs
- crates/hagency/src/console/native/agent_crypto.rs
- crates/hagency/src/console/native/agent_crypto/**
- crates/hagency/src/console/server_login.rs
- crates/hagency/src/console/server_login/**
- crates/hagency-app/specs/task-client-quality-credentials.spec.md
- crates/hagency-app/README.md
- crates/hagency-app/README.zh-CN.md
- crates/hagency-app/docs/privacy.md
- crates/hagency-app/docs/privacy.zh-CN.md
- crates/hagency-app/docs/hagency-quickstart.md
- crates/hagency-app/docs/hagency-quickstart.zh-CN.md
- crates/hagency-app/packaging/release-notes.md
- dev-vibe/client-credential-storage-remediation-2026-10-30.zh.md
- dev-vibe/client-quality-remediation-2026-10-29.zh.md
- docs/progress.md
- docs/agent-knowledge.md

## Acceptance Criteria

Scenario: Scoped keys and large authenticated documents preserve original secrets
  Test:
    Package: hagency-store
    Filter: credential_vault::tests
    Level: integration
    Test Double: explicit isolated fake credential backend and actual private files
  Given large OAuth documents native storage faults context swaps and interrupted migration phases
  When the production vault prepares verifies and commits a secret migration
  Then only scoped small keys reach the native backend plaintext is not emitted and unknown outcomes preserve recoverable original ownership

Scenario: Account restore and refresh retain database identity and latest credentials
  Test:
    Package: hagency-app
    Filter: credential_migration
    Level: integration
    Test Double: actual SDK session fixtures isolated files and explicit fake vault
  Given retained databases refresh callback failures stale epochs and mixed legacy or unknown session schemas
  When session storage restores migrates refreshes logs out and inventories databases
  Then original database keys remain usable stale callbacks cannot publish and incomplete inventory refuses orphan collection

Scenario: Agent and revoke-only persistence preserve their distinct authority
  Test:
    Package: hagency
    Filter: credential_migration
    Level: integration
    Test Double: isolated Agent directories explicit fake vault and controlled revocation fixtures
  Given original Agent keys changed authentication generations pending revocations and inaccessible native credentials
  When the native owner migrates or reopens its persisted credentials
  Then original keys and revoke progress survive without restoring revoked authority or accepting a replacement key

## Out of Scope

iOS and Android retain an explicit compile-time v1 compatibility adapter; R05
remains unremediated on mobile and no native keystore qualification is claimed.
Host legacy helper tests prove v1 preservation and native or unknown schema
read/write rejection, not mobile device or cross-compilation qualification.

Native prompt behavior and system credential availability require separately
recorded disposable namespace platform qualification. Existing Codex provider
credentials remain owned by their original provider and are never imported.
