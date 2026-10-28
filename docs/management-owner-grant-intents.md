# Native owner grant consent custody

This is an additive prerequisite. It does not activate a mini-app capability,
trusted confirmation window, public enrollment route, owner-ledger
migration, grant publication or agent execution.

`OwnerGrantIntents` accepts an already verified, non-deserializable
`VerifiedManagementEnrollment` from trusted native code. It creates private
intent, grant and command IDs and persists the exact profile, installation,
execution device, enrollment, resource revision, coordinator, token amount,
manager allowlist, delegation expiry and native UTC accounting period **before**
returning an opaque `OwnerGrantReview`. The review cannot be cloned or
constructed from JSON. Its safe summary contains the human review fields and
explicit period; provider subjects, private profile identity and receipts remain
native/private.

`prepare`, `review_existing`, `confirm` and `decline` require the native-only
`ConsentGrants` permission plus `ReadResources`. No Desktop service or package
capability maps to `ConsentGrants`. `PublishGrants` retains its narrower existing
meaning: publish an already recorded immutable grant receipt. SDK account,
package session, execution device and full local identity are checked after
asynchronous work. Confirmation must use the exact manager instance that minted
the review. A restriction-only confirmation gate is AND-composed with the
original enrollment/profile gate, so retirement while a SQLite write is queued
cannot be bypassed. Existing period, delegation and monotonic expiry checks are
unchanged.

The version-1 private JSON journal is stored as `owner-grant-intents-v1.json`
under `journal_profile_directory`. It uses the original NativeOwner installation
lock and serialized management mutation lane, private atomic replacement, a
2 MiB limit and at most 128 retained intents. Opening this journal does not open,
initialize or migrate personal-agent SQLite. A second process cannot acquire
custody of the same native installation. A private identity marker is written
before first journal persistence; losing either file then fails closed. The
marker is not grant authority: even if both files are lost, fresh preparation
checks canonical contribution history in the registry. Confirmation repeats that
restriction inside the actual grant transaction, after exact historical receipt
replay. It cannot silently issue a second grant or move it to a new period.
The bounded history read examines no more than the existing 2,048-grant table
limit and returns only an existence result; closed or expired original grants
also require an explicit future renewal protocol.

| Operation | Result and restriction |
| --- | --- |
| `prepare` | Saves an unattempted original intent before showing a review. Another unresolved intent for that contribution must be recovered instead. |
| `review_existing` | Rechecks the canonical receipt first. Only an unattempted review can acquire a fresh review handle, with identical IDs, selection, enrollment binding and frozen UTC period. |
| `confirm` | Durably marks the original intent `applying` before consuming registry consent. It never creates replacement IDs. |
| `list_existing` | Requires `ReadResources` and the exact original local profile/installation, then returns at most 128 descriptive summaries and original intent IDs. It exposes no cached success state; each result still requires canonical recovery. |
| `recover` | Requires fresh same-profile/installation authorization and `ReadResources`, then reads `recover_capacity_grant` from the actual owner registry. Cached JSON cannot establish success. |
| `decline` | Closes only an unattempted review. It cannot remove, refund or cancel an applying/recorded hold. |

If a result is lost, a matching immutable receipt is recovered and saved without
rewriting its original fields. If the canonical receipt is absent after an
attempt, the state remains `outcome_unknown`. It cannot be converted back into a
fresh approval by this slice. Likewise, a cached success whose canonical row is
missing does not authorize a replacement hold. A separate future protocol would
need explicit evidence of no prior effect to support such a retry. A recorded
receipt remains recoverable after the original review period or authorization
has expired; this is not renewal authority.

An unattempted review whose period or enrollment generation has changed cannot
be silently rebuilt. If its original opaque handle is no longer available to
decline it, a future explicit stale-review closure protocol is needed. This
conservative limitation does not erase the original journal or permit a new
grant in its place.

The [native enrollment factory](management-native-enrollment.md) privately creates
an enrollment ID once after verifying current admitted contribution/resource,
coordinator, exact execution generation, provider account and SDK-owned identity
through closed native transport. A contribution ID or an approved mini-app flag
cannot stand in for that proof. Owner-ledger adoption and trusted native UI remain
separate gates; there is no user-facing working grant flow yet.

Focused tests use synthetic HTTP/SDK authorization and an actual isolated owner
SQLite database. They cover persistence before confirmation, actual NativeOwner
restart, immutable original IDs/receipt, commit-reply loss, canonical-row absence,
cached receipt tamper, journal loss and identity mismatch, expired historical
period and changed proof, duplicate confirmation, original proof composition,
loss of both private files after commit, a competing canonical grant committed
between review and confirmation,
retirement while the real SQLite writer is blocked, retirement immediately after
commit, SDK invalidation after the receipt journal is saved, bounded read-only
restart discovery, and a separate-process installation-lock refusal. No live provider,
Matrix room, deployed server or production ledger is used. Logs are retained in
`.run/owner-grant-intent-tests-checkpoint.log`; native UI/E2E activation remains a
separate gate.
