[English](privacy.md) | [简体中文](privacy.zh-CN.md)

# Hagency data and privacy

This describes the desktop application's data flows. Your Hagency server,
Pasion identity provider and configured model services have their own policies.

## Accounts and local storage

Hagency signs in to your selected Hagency server through Pasion in your browser.
The Matrix SDK maintains the chat session. Account and message data sent to the
server is governed by that server's policies and Room membership.

The app stores session information, encryption state, Room caches, settings,
article drafts and selected article assets in its local profile. Agent profiles,
workspaces, usage ledgers and recovery state are also stored locally and scoped
to the authenticated account. Switching accounts stops the previous account's
local Agent runtime; it does not erase that account's stored data.

On macOS, Windows and Linux, small original database/Agent secrets and wrapping
keys use Keychain, Credential Manager and Secret Service respectively. Large
session, OAuth and revoke-only documents use authenticated encrypted envelopes
in private files. Native availability and write/readback verification precede
new desktop browser authorization. Missing or inaccessible native keys stop
restore; they do not authorize a new passphrase for an existing encrypted DB.
Legacy migration preserves its original key and replaces plaintext only after
native verification and encrypted publication. Unknown file or commit outcomes
remain blocked for recovery. Keep the application profile and its native
credentials together when planning recovery; copying the profile alone does not
establish cross-host key recovery.

Account switching retains a previous grant's late refresh outcome in its own
scope. Explicit logout first commits a revoke-only intent; it can complete
locally while remote access/refresh revocation is pending. A late original
refresh is added to that old grant's revocation record. A remote timeout remains
unknown. Synchronous SDK save callbacks still perform synchronous local file IO;
remote rotation and local durability cannot be one atomic transaction. Old grant
keys may remain in the native store until a complete reference inventory proves
them unreferenced; no native-store enumeration or blanket deletion is performed.
Startup deletion of old database directories is deferred until database writers
and cleanup share a cross-process protocol; existing stores can continue to use
disk space. Matrix logout with an expired access token and an unconfirmed refresh
token may stay pending indefinitely: retry attempts do not prove that the remote
session has ended.

These credential changes target desktop. iOS/Android retain v1 private-file
secret storage without this native wrapping and reject versioned desktop files
rather than downgrade them. The automated migration tests use explicit isolated
fake stores and real private files; they do not qualify real OS credential
prompts, unlock/deny behavior or mobile devices. Drafts, caches and Agent
workspaces continue to follow their existing storage rules.

`HAGENCY_APP_DATA_DIR` selects an absolute application data directory.
`HAGENCY_DESKTOP_DATA_DIR`, `RINX_DATA_DIR` and `ROBRIX_DATA_DIR` remain accepted
aliases. Clearing the event cache reloads Room events from the server; it does
not delete server messages or erase the entire application profile.

## Agents and configured services

Running an Agent can send its Room conversation, instructions and permitted file
content to the model/runtime service selected for that Agent. Enabled web search,
tools and speech services may contact their configured services. Review those
providers' policies and your Agent's resource and permission settings.

Human Matrix chat supports encryption. The current Agent runtime also supports
encrypted tasks, Codex programming, replies and file transfers in the owner's
private chat, with the exact Agent identity verified from a verified owner device.
New owner chats enable encryption by default; existing plaintext chats require
an explicit upgrade. Encryption of the Matrix conversation does not hide task
content from the execution device or the selected model service: the device
decrypts inputs and permitted files for processing, and retains local task,
workspace and recovery data.

Encrypted execution for other requesters in shared Projects and cross-host key
recovery are not delivered. The [recorded end-to-end acceptance](../../../dev-vibe/agent-encrypted-codex-validation-2026-10-27.zh.md)
covers macOS with an isolated OAuth test bridge, not completed manual desktop,
production OAuth or other-platform qualification. Pasion browser sign-in remains
the supported login path. Use the conversation's actual encryption state and
verified identity; do not infer encryption for every Agent Room from this private
owner path.

## Mini apps, articles and external links

Mini-app authorization is tied to the signed-in Matrix user and the requested
capabilities. The article mini app receives scoped host operations, not the
account password or a raw Matrix access token. Publishing or sharing an article
sends its content to the selected Matrix conversation.

Native HTML/CSS article preview uses the app's authorized article assets. Other
web mini apps and links you choose to open may contact their own sites. Optional
Hagency integrations communicate with the services configured for that workflow.

## Diagnostics and support

Logs and exported diagnostics can contain account or room identifiers. Review
them before sharing them in a public issue, and remove credentials, private
conversation content and personal file paths. See [support](support.md) for the
current feedback channel. Technical details of mini-app grants
are documented in [ADR 0002](adr/0002-octoscript-mini-app-authority.md).
