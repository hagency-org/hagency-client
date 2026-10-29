Projects, Matrix chat and locally executed Agents are available in this release.

- Create Projects and discussion Rooms, create owner-bound Agents and bind them to Rooms.
- Configure Codex, Claude Code or Octos, model/reasoning options, workspaces, budgets and exact-action approvals.
- Configure supported Agent resources on another assigned execution device; offline settings remain read-only.
- Control Codex live web search per Agent, enabled by default.
- Use the Hagency desktop branding and icons.
- Maintain the cross-platform Hagency app and its local owner engine together in [hagency-org/hagency-client](https://github.com/hagency-org/hagency-client), with a single root Cargo.lock and no sibling source checkout.

The client version is 0.6.0, following consolidation of the native app and the local engine.
Existing installations may need to install this version manually.

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

[Agent setup](https://github.com/hagency-org/hagency-client/blob/main/crates/hagency-app/docs/hagency-quickstart.md)

Dependency audit follow-up remains open for the embedded and bundled Octos runtime,
including PDF/XML parsing. See the [recorded assessment](https://github.com/hagency-org/hagency-client/blob/main/dev-vibe/desktop/release-0.5.0/assessment.zh.md)
for scope and evidence; this release does not claim a clean dependency audit.
