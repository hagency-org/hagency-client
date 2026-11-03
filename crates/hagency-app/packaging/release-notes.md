## Hagency Client 0.6.3

- Show safe diagnostics when the server refuses Agent encryption initialization, so the message distinguishes that failure from local Codex sign-in.
- Includes the desktop icon and account-check fixes previously shipped in 0.6.2.
- The Agent encryption fix also requires an updated Hagency Server containing [Server PR #32](https://github.com/hagency-org/hagency-server/pull/32). With that server, an isolated test verified Agent identity, one real Codex call, and an encrypted reply. Production rollout is verified separately.

This is an unsigned release for manual installation. macOS packages are for Apple Silicon. Linux packages are available for amd64 and arm64, and the Windows package is for amd64. Code signing, macOS notarization and automatic updates are unavailable for this release. The real Codex sandbox and two-Agent release qualification records remain incomplete. The isolated conversation test does not replace them.

[Agent setup](https://github.com/hagency-org/hagency-client/blob/main/crates/hagency-app/docs/hagency-quickstart.md)

Current source supports encrypted owner-private-chat tasks, Codex programming,
encrypted replies and file transfers after exact Agent identity verification from
a verified owner device. New owner chats are encrypted by default; existing
plaintext chats require an explicit upgrade. Existing budgets, tool policies and
explicit service start still apply. Ordinary encrypted Matrix chat remains available.

Encrypted execution for other requesters in shared Projects and cross-host key
recovery are not delivered. The [recorded macOS end-to-end acceptance](https://github.com/hagency-org/hagency-client/blob/main/dev-vibe/agent-encrypted-codex-validation-2026-10-27.zh.md)
uses an isolated OAuth test bridge; it does not complete manual desktop,
production OAuth or other-platform qualification, or establish that every
previously published package contains this source path. Pasion browser sign-in
remains the supported login path.
Windows ARM64, mobile store packages, automatic updates and install-and-launch verification on every OS are not included.
Provider runtimes and accounts must be installed/configured on the execution device.

Dependency audit follow-up remains open for the embedded and bundled Octos runtime,
including PDF/XML parsing. See the [recorded assessment](https://github.com/hagency-org/hagency-client/blob/main/dev-vibe/desktop/release-0.5.0/assessment.zh.md)
for scope and evidence; this release does not claim a clean dependency audit.

Desktop credential source changes use explicit OS storage for small original
DB/Agent secrets and wrapping keys, with authenticated private envelopes for
sessions and revoke-only records. Unavailable native credentials stop restore;
original database keys are retained. Local logout and remote revocation have
separate confirmed/pending outcomes. Old grant OS keys may remain until complete
reference proof permits removal. Synchronous SDK persistence and remote rotation
still have crash/unknown-outcome limits. iOS/Android retain their v1 private-file
secret format. Isolated fake-store migration tests do not establish real native
credential-store or mobile qualification; package installation/launch coverage
remains separate from these source changes.
Startup database-directory deletion remains deferred until all writers and
cleanup share a cross-process protocol; old databases can retain disk space.
A Matrix expired-access response with an unconfirmed refresh token can remain
pending; retries do not guarantee remote session termination.
