# Hagency

[Website](https://hagency.org/) · [Support](docs/support.md) · [Data and privacy](docs/privacy.md)

Client 0.6.0 is available from [Hagency Client Releases](https://github.com/hagency-org/hagency-client/releases/latest). The [release assessment](../../dev-vibe/desktop/release-0.5.0/assessment.zh.md) records outstanding upstream dependency security work; see the [0.6.0 release notes](https://github.com/hagency-org/hagency-client/releases/tag/v0.6.0) for current limitations.

English | [简体中文](README.zh-CN.md)

Hagency is a native Rust/Makepad client for Hagency servers. It combines
Matrix chat, Project workspaces and locally executed AI Agents in one application.
Sign in to a **Hagency server** through its Pasion browser authorization. Ordinary
Matrix servers are not supported by the current login flow.

## What is managed where

- A Project binds one Matrix Space. Its discussion Rooms keep their own membership.
- An Agent is a permanent, server-wide identity owned by its creator. Creation
  assigns the current authorized device; Room bindings are separate.
- The server checks identity, Room admission and the assigned execution device.
  Project/Room administrators manage Agent access and service pauses.
- The owner selects Codex, model, workspace, budgets and tool policy on their own
  device. These resource choices require no server administrator approval.
- One device can run multiple Agents. Switching accounts stops the current owner
  runtime; it does not transfer Agents or merge account data.

Desktop sign-in uses the operating system credential store for small original
DB/Agent secrets and wrapping keys. Session and revocation documents remain in
private local files as authenticated encrypted envelopes. Unlock the OS store
before signing in; unavailable credentials stop restore or migration. Logout
can finish locally with durable remote revocation still pending. iOS/Android
retain their previous private-file v1 format; this desktop change does not
remediate mobile credential storage. See [storage and recovery](docs/privacy.md).

## Quick start

1. Enter your Hagency server address and complete Pasion sign-in in the browser.
   The same authorization establishes your Matrix chat and owner-management session.
2. Open **Projects → New Project** (also available from the Projects **⋯** menu).
   Create a Space or select a joined Space. Add discussion Rooms from its details.
3. Open **Agents → New Agent**, enter a name, and wait for the identity and its
   owner private chat to become ready. No Project or Room is required to create it.
4. In **Codex**, choose **Use my signed-in local Codex account**. An
   already signed-in local account does not require another login. Alternatively,
   use the separate Codex sign-in controls. Choose and save a model and workspace.
5. Set the Agent, Room and requester limits in **Budgets**. Then select
   the private chat or a bound Room in **Run**, explicitly choose
   estimated accounting and a reservation, and start that Room.

See the [full native quick-start guide](docs/hagency-quickstart.md) for Room
bindings, member invitations, policy boundaries and recovery states.

Project tabs are **Overview / Rooms / Members / Access**. Agent tabs are
**Agent / Codex / Budgets / Run**, for identity/device, local resources, request
policy and quota, and scoped execution/approvals respectively.

## Navigation and retained features

The desktop sidebar has **Projects, Agents, Chats, Contacts and Mini apps**.
Projects, Agents and Chats each have a **⋯** creation menu. **More** opens Discover,
Explore rooms, Moments, File Transfer, Article editor, Browser and Settings.
Project entries have their own **⋯** menu for details, Rooms, members, invitations
and opening the Space. Invitation controls follow actual Matrix permissions.

Matrix chat, contacts, room search and attachments, encrypted human conversations,
Moments, the native article editor and scoped mini-app import/consent/run remain.
The default mini-app catalog has no Palpo app. Hagency management uses the native
Projects and Agents pages; the former Palpo resource/coordinator workflow is retired.
The current runtime supports encrypted tasks, Codex programming, encrypted replies
and file transfers in the owner's private chat after verifying the exact Agent
identity from a verified owner device. Newly created owner chats use encryption
by default; existing plaintext chats require an explicit upgrade. Budgets, tool
policies and explicit service start still apply.

Encrypted execution for other requesters in shared Projects and cross-host key
recovery are not delivered. The recorded end-to-end acceptance covers macOS with
an isolated OAuth test bridge; manual desktop operation, production OAuth and
other-platform qualification remain open. This does not mean that Pasion browser
sign-in is unavailable. See the [encrypted owner-chain acceptance record](../../dev-vibe/agent-encrypted-codex-validation-2026-10-27.zh.md)
for the implemented path and validation boundaries.

The renderer remains experimental; arbitrary WeChat HTML import and complete
WeChat compatibility are not claimed. See the [article integration evidence](lab/article-html-integration/README.md).

## Accounts, settings and local data

Use **Settings → Account** and its saved-account picker to add or switch identities.
The bottom-left account menu opens account settings and logout. Each account keeps
its own Matrix device/crypto data and owner profile. Only the selected account syncs.
Switching stops local Agent processing for the previous account; logout stops its
runtime and revokes its current authorizations.

**Settings** contains **Account, Preferences, Privacy and About**. Appearance and
language are in Preferences. Generic external **Agent Access** controls are separate
from Hagency's native Agents; a BotFather or loopback endpoint is not required for
Hagency setup.

`HAGENCY_DESKTOP_DATA_DIR` selects an absolute, isolated data directory. Existing
`RINX_DATA_DIR` and `ROBRIX_DATA_DIR` environment aliases remain in the desktop
launcher. They do not enable compatibility with retired Fleet data. Owner ledgers,
workspaces and provider associations stay scoped to the full authenticated account.
Moving execution to another device does not move ownership or reset historical
costs; restore the same owner's complete ledger when the recovery gate requires it.

## Build and check

Desktop is part of the [`hagency-client`](https://github.com/hagency-org/hagency-client)
workspace. Its owner engine is the local `../hagency` crate, and the repository
root Cargo.lock records all dependency pins. The workspace requires Rust 1.98 or newer;
engine CI selects Rust 1.98.0 explicitly.
Install CMake and the platform's native build dependencies, then run:

```sh
cargo run --locked --bin hagency
cargo test --locked --lib
cargo test --locked --manifest-path ../article-core/Cargo.toml
python3 tools/wechat-ux/check_i18n.py
```

The Cargo package is `hagency-app`, the executable is `hagency`, and the application
presents itself as Hagency on every platform. The engine is a shared library;
the former CLI and Web console have been removed. Default features use the standalone native application. The retired
`agent_chat` instrumentation is not needed to enable current Project/Agent management.
Mobile packaging requires platform signing and device validation.

Built-in native and OctoScript apps are declared in `system-apps.json`; the article
editor is under `apps/article-editor/`. See the [app development guide](apps/README.md)
and [catalog architecture](docs/adr/0008-rinx-system-app-catalog.md).

## Acknowledgements and license

This repository preserves its Robrix/Robrix2 and Rinx history. Thanks to Kevin Boos,
Project Robius and the contributors to Makepad, Robius, Matrix Rust SDK, Blitz and
Octoscript for the native client foundations.

Distributed under [Apache-2.0](LICENSE). Inherited Robrix code retains its
[MIT copyright and permission notice](LICENSE-MIT). See [NOTICE](NOTICE) and
[third-party notices](licenses/THIRD-PARTY-NOTICES.html).

See [development, isolated profiles and native acceptance](docs/local-development.md).

[GitHub CI and releases](docs/github-ci.md)
