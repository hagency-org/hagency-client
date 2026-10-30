[English](hagency-quickstart.md) | [简体中文](hagency-quickstart.zh-CN.md)

# Native Hagency quick start

This guide describes the current Projects/Agents pages. The Palpo mini-app,
Fleet/engagement/resource requests and coordinator approvals are retired.

## Sign in once

Enter a Hagency server address on the login page, then finish Pasion sign-in and
consent in your browser. The server must identify itself as a supported Hagency
service; a normal Matrix server is rejected. The Matrix SDK owns this login's
refresh and persistence. Native management uses the same current account proof,
without asking for a copied token or a second Matrix login.

When Pasion advertises account creation, the selected server also shows **Create
an account**. This opens Pasion registration in your browser with OAuth
`prompt=create`; after registration and consent, the same SDK callback signs you
in to Desktop. A server that disables registration hides this option.

Saved accounts are available on the login page and under **Settings → Account**.
Switching accounts stops the current owner's local Agent runtime and clears its
active permissions. The previous account's separate data remains; Agent ownership
never transfers with a switch.

Desktop browser sign-in first checks native credential write/readback. Keep
Keychain (macOS), Credential Manager (Windows), or Secret Service (Linux)
available and unlocked. A locked, denied or missing store stops sign-in or
restore; retain the existing profile and original keys when resolving it.
Logout may report local completion with remote revocation still pending; the
original refresh result remains assigned to that old revoke-only scope.
iOS/Android keep their previous v1 private-file secret format and are outside
this desktop credential remediation. See [storage and recovery](privacy.md).

## Create a Project and discussion Rooms

Open **Projects**, then **New Project** (or Projects **⋯ → New project**).
Choose **Create a new Space** or **Connect an existing Space**. Existing candidates
are joined Spaces for this account. Partial lookup failures and pagination must
be resolved before choosing an unavailable candidate.

A Project is the Hagency record bound to a Space, not every Matrix Space. Its
name/topic come from the Space. Select the Project's details:

| Section | Purpose |
| --- | --- |
| Overview | Space identity, description and Open Space. |
| Rooms | Create a Room or connect an already Space-linked Room; open its chat and members. |
| Members | Read actual Space members; invite when Matrix grants permission. |
| Access | Read access/service state; permitted administrators can edit it. |

Space membership is not copied automatically into every Room. Each discussion
Room has its own membership and invitation rules. A Project tree entry's **⋯**
also opens Overview, Discussion Rooms, Project members, Invite members and Open
Space. If a creation result is unconfirmed, use **Resume original request**;
do not create a second Space/Room to guess what happened.

Project/Room access policy controls who may connect an Agent to that scope. Deny
entries take priority. It does not approve token allocations. **Service pause**
is separate and can stop current service; clearing it does not automatically
resume the owner's paused bindings. Ordinary members cannot write these settings.

## Create a global Agent

Open **Agents → New Agent**, enter its display name and create it. This allocates
a permanent owner-bound puppet identity and assigns the current authorized device.
It does not allocate a Project or discussion Room. Wait for provisioning to finish.
The owner-private-chat setup preserves its original creation request on retry.
Use **Set up / retry private chat** if needed, then **Open private chat** when ready.

Only the assigned device may execute this Agent. **Handle on this device** changes
that assignment to the current owner's device with an explicit confirmation and
revision check. It does not transfer ownership, copy budgets, reset spent tokens
or authorize repeating unknown work. Historical ledger recovery may still be
required before this device can run a new model request.

Add discussion bindings with **Add Room binding**, or open a discussion Room's
chat information, **Members → Invite my Agent**, and select an existing Agent.
The backend still checks actual Project/Room admission, membership and puppet
invitation. This picker does not create a second Agent identity.

## Configure the four Agent detail tabs

| Tab | Configure or inspect |
| --- | --- |
| Agent | Identity/device, owner private chat, Room bindings, pause/resume, leave and permanent retirement. |
| Model & resources | Provider account, model, reasoning options and saved Agent workspace on this device. |
| Limits | Agent total, selected Room total, and selected requester in that Room. |
| Service | Selected Room runtime, per-request token reservation, scoped files and pending approvals. |

Select Codex, Claude Code or Octos in **Local agent runtime**.
Choose **Connect my local account** to reuse the current machine's
Codex login. No second login is needed when that account is already valid.
Disconnecting its association keeps your everyday Codex signed in. Separate
Codex sign-in is also available; cancel/open-URL/sign-out controls apply to that
separate account. Save a model and reasoning effort from the real catalog. A private workspace is
created automatically; a custom workspace is optional under Advanced settings.

Codex's **Live web search** is enabled by default and can be disabled in Model & resources.
The setting belongs to this Agent and applies across its Rooms. Save, then stop and restart
the service to apply changes. Live search retrieves current sources; local command networking
remains separately controlled.

The model/workspace belong to this Agent across its Rooms. Conversations, budgets
and approvals stay Room-scoped. The server administrator does not approve these
local resource choices.

Agent details select the owner’s private chat by default. Service shows the current chat directly; switching to another Room and managing its binding are under **Advanced Room settings**. Opening Service from a Project Room retains that exact Room.

In **Limits**, choose the entire Agent, one Room, or one member in a Room.
Missing limits allow requests by default; configured token limits apply together.
Each limit supports cumulative, UTC day, or UTC month periods. Zero tokens blocks
usage in that scope. Existing Unset values also mean no token limit.
Request handling can Allow, Deny or Ask owner. Tool policy can deny high risk, ask
per exact tool, or restrict exact tools/directories. Saving preserves spent usage
and unresolved holds. A budget hold is not measured usage of zero.

Claude Code uses its existing local sign-in and the Sonnet, Opus or Haiku model
alias with supported effort choices. Octos uses an existing local profile; profiles
with the same model remain distinct and keep their own reasoning settings.
Credentials remain in the provider app. Disconnecting removes this owner's
association without signing out that app. Stop the service before changing
runtimes, save the profile, then restart.

Claude/Octos native tools use the existing Agent, Room and requester permission
layers and owner approval list. A denial at any layer blocks the operation. The
Room-files switch is the Codex Hagency file bridge; this bridge is not registered
with Claude or Octos.

## Start and chat

In **Service**, select an active private chat or Room binding. Wait for
the saved local runtime profile and account check to finish. Estimated token accounting is used by default; set a positive per-request
reservation and save a supported reasoning effort in Model & resources. Actual usage may exceed
the estimate. Missing usage is recorded as zero and releases the reservation automatically; an uncertain execution is never replayed.
Scoped host files are opt-in and limited to list/read/create. Codex programming is a separate explicit mode described below.

**Start responding** starts only that selected scope. **Stop responding** stops it; another
running Room can stay active. Your owner private chat does not need mentions;
ordinary top-level DM turns share their Room conversation. Explicit threads remain
separate. In a collaboration Room, every message requires a real Matrix Agent @mention by default, including thread follow-ups.
Under Agent details → Advanced Room settings, select a Room and enable automatic thread follow-ups in **Room reply behavior**.
After an @mention activates a thread, any member can follow up there without another mention. New threads still require @.
The setting applies only to that Agent in that Room. A started model turn can receive the puppet's 👀 processing reaction.

For native commands, existing-file edits and MCP, enable **Codex programming** in **Model & resources** on the execution device. Save and restart the Agent service. This uses the Agent workspace shared by its Rooms; it does not provide operating-system filesystem confidentiality between Rooms. Set all applicable Agent, Room and requester high-risk policies to ask the owner or allow exact tools/directories. Any deny still blocks the operation.

The MCP JSON field accepts explicit servers, such as `{"docs":{"transport":"http","url":"https://example.test/mcp"}}` or `{"local":{"transport":"stdio","command":"/absolute/canonical/server","args":[]}}`. Personal Codex servers and hooks are not imported. These settings remain on the execution device and are excluded from the v1 remote model configuration transport.

Pending request/tool approvals appear in the Service tab for the selected Room.
While Service is open, approvals refresh automatically with an in-app notice for
new requests, exact action details and an expiry countdown. Failed refreshes disable
decisions. A submitted decision is not proof of successful execution; check the
Room reply and runtime status. An uncertain submission must not be resent.
Approval is for the exact
request or tool shown, not an unrestricted server resource grant.

### Encrypted owner private chat

The current runtime implements encrypted tasks, Codex programming, encrypted
replies and file transfers in the owner's private chat. To use this path:

1. Configure local Codex and the Agent workspace on the assigned execution device.
   Enable Codex programming if the task needs commands or file changes, and save
   the applicable budgets and tool policies.
2. In **Agent encryption identity**, initialize the identity if needed. From a
   verified owner device, check the full Agent master key and device fingerprints,
   then use **Verify exact Agent identity**. If private signing keys are missing,
   follow the recovery guidance; a cached verified label is not sufficient.
3. Use the owner's private chat. Newly created owner chats enable encryption by
   default; an existing plaintext chat needs an explicit upgrade. Check its actual
   encryption state, then explicitly start the service for that scope.
4. Send the task or attachment in that encrypted chat. Existing approvals still
   apply to commands and files; check actual reply and file-delivery results.

Shared Project requester trust and cross-host key recovery are not delivered.
The [existing end-to-end acceptance](../../../dev-vibe/agent-encrypted-codex-validation-2026-10-27.zh.md)
proves the owner path on macOS with an isolated OAuth test bridge and same-host
service reopens. Manual desktop operation, production OAuth and other-platform
qualification remain open; Pasion browser sign-in is still the supported login
path. Ordinary encrypted Matrix chat remains available independently.

### Restart recovery

Restart recovery restores saved runtime intentions. Its rule is to restore only Room scopes you explicitly started and have not explicitly stopped, using the saved reservation, effort and host-files choice after fresh identity, device, lease, resource and ledger checks. **Stop responding excludes that scope from restart recovery**; a normal app shutdown is not the same instruction. An active Agent or saved model alone is not an instruction to run every Room. Missing resources, unknown costs, revoked membership or another assigned device must block recovery; no model replay or automatic takeover.

Agent Unlimited does not bypass finite Room/member limits or explicit request denial. A sent paused-policy reply is not successful model inference. See the [runtime recovery record](../../hagency-client/docs/design/2026-10-08-agent-runtime-recovery.zh-CN.md) for ordinary/quick restart, explicit Stop exclusion, and remaining untested branches.

## Failure and recovery states

| State | Action |
| --- | --- |
| Loading/unavailable | Refresh or retry after checking the connection; late results from a previous account/view cannot replace the current one. |
| Agent creating or private chat pending | Wait; retry the same saved request instead of creating another identity/Room. |
| Other execution device | Use that device, or explicitly assign this device and satisfy ledger recovery checks. |
| Missing model/workspace/account | Connect the account and save a model in Model & resources; the private workspace is automatic. |
| Budget exhausted/denied/approval pending | Configure the matching layer or review the exact pending request. |
| Unknown cost/side effects or ledger recovery required | Preserve the hold and restore the same owner's complete state. Do not reset balances or replay the model. |
| Puppet/owner membership lost | Restore real permissions, wait for confirmed departure, then explicitly rebind/re-adopt. Old dispatches stay fenced. |
| Ordinary service pause | Clear the applicable pause and explicitly resume when permitted. |
| Encrypted owner private chat unavailable | Check the exact Agent identity, verified owner device, encryption state and current execution-device authorization; start only after those checks succeed. |
| Encrypted shared Project request | Shared requester trust is not delivered. Keep the refusal; use the verified owner private path for the owner's tasks. |
| Cross-host encrypted key recovery | Not delivered. Same-host service restart evidence does not authorize a fresh host or reconstruct its keys. |

## Other retained pages

### Windows startup and system tray

**Settings → General → Startup and system tray** controls sign-in startup,
starting hidden, minimizing to the tray, and keeping the app running after closing
its main window. Sign-in startup and starting hidden are off by default; the two
tray behaviors are on. Changes are saved for this device across accounts.
Click the Hagency tray icon to restore the window, or right-click for **Settings**
and **Quit Hagency**. Launching the same profile again restores its existing window.
If the icon is in Windows' hidden-icons area, expand that area or pin the icon.
The settings page also links to Windows Startup apps, where Windows may separately
disable startup. Tray support currently applies to the standalone Windows app.

**Chats** includes search, attachments, Room information, members and invitations.
**Contacts** opens ordinary conversations. **Mini apps** keeps non-Palpo built-ins,
App Hub, local import, permission review and scoped execution. **More** opens
Discover, Explore rooms, Moments, File Transfer, Article editor, Browser and Settings.

Settings has Account, Preferences, Privacy and About. Generic external Agent
Access/BotFather controls are separate tools, not part of native Hagency Agent
setup. Some management text is still English when the interface language is Chinese.

See [development and local diagnostics](local-development.md) for building and connection checks.
