# Managed-agent reserve and cleanup transport checkpoint

This checkpoint connects the frozen central schema9 executor protocol to the
canonical native owner ledger. It remains native-only and unadvertised. It does
not start an agent, create a Matrix identity or room, obtain a provider process,
issue a prompt, or grant runtime custody.

## Authority and wire contract

`console/native/managed_agent_transport.rs` uses the existing authenticated
`AllocationTransport` boundary: SDK-owned Matrix account checks, the complete
origin/issuer/subject/owner profile, pinned device authorization, package/session
permission, and monotonic deadlines are checked before and after asynchronous
work. The new `ExecuteManagedAgents` permission has no Desktop mini-app mapping.

The only central requests are the closed schema9 POST routes:

- `management/agent-executor/poll`, with an optional validated continuation ID;
- `management/agent-executor/{commandId}/recover`, with the exact command digest;
- `management/agent-executor/{commandId}/result`, with the exact command digest,
  finite upload token, and original native receipt.

Fresh delivery may create `VerifiedManagedAgentDelivery` or
`VerifiedManagedAgentFence` only after reading the exact original successful
project reserve command and receipt from the same canonical registry and
checking the authenticated current contribution. Missing native project history
is an error. The adapter does not reconstruct native authority from central JSON.
The original reserve generation must equal the current installation generation
for a new hold. A newer generation on the same original installation may recover
receipts and perform cleanup; another device/profile cannot.

Historical recovery is observation-only. It never constructs either executable
proof, even if no receipt exists locally. An improperly executable historical
delivery is rejected. A recorded central result without its original local
receipt is reported as missing native history. If both exist, canonical digests
must match. Lost result responses retain the original native receipt and can be
resolved by recovery without creating another hold or issuing another effect.

The paired closed fixtures are in
`native/hagency-store/src/domain/owner_resources/capacity/managed_agents/`:
`management-agent-admission-v1.json` and `management-agent-fence-v1.json`. They
are copied from the frozen central schema9 fixture contract; reserve-v1 bytes
are unchanged. Result acknowledgement checks include the exact command and
receipt digests, manager/device/generation, and native child identity. A fence
cannot acknowledge a different assignment or claim execution is available.

## Permanent cleanup and capacity accounting

Schema77 (`079-owner-managed-fences.sql`) adds an immutable, undeletable semantic
tombstone tied to profile, agent/request, original reserve bytes and digest,
project reservation, device, and first fence revision. An immediate SQLite
transaction writes the tombstone, fences any existing child, cancels unissued
prepared calls, and stores the exact receipt together. Receipt-write failure
rolls the entire transaction back. Every new result advances the existing global
owner ledger revision; exact replay returns the original result/revision.

If cleanup arrives before reservation, `absent_fenced` means the semantic
tombstone was committed. A later reserve, including replacement command IDs for
the same agent/request, cannot create a hold. A missing-row lookup alone never
produces an absence receipt.

If a child exists, zero retained liability is reported only when its provision
effect was never issued and every call is cancelled before issue with no usage
or unrepresentable evidence. An issued/qualified/failed/unknown provision remains
`processState: unknown`, `retainedTokens: null`, `usageComplete: false`; a fence
does not manufacture proof that a process stopped. Known consumption is retained
conservatively. This protocol does not refund the parent or settle a project
directly. Existing derived liability checks continue to control project release
and parent settlement.

Original positive reserve receipts remain immutable after fencing. They are
historical evidence, not permission to restart. Schema77 has been reviewed by
the read-only source audit/archive layer; populated tombstones block automatic
definition-only adoption and do not erase original usage liability.

## Validation

Real SQLite tests cover canonical fixture parity, cleanup before reserve,
semantic replacement IDs, issued-process unknown liability, original receipt
replay, generation rotation/resource retirement, wrong profile/project binding,
transaction rollback, and atomic schema76-to77 upgrade/reopen. Loopback HTTP
tests use the actual owner authentication and native transport with fixture-only
provider identity constructors. They cover lost replies/restart, historical
no-effect recovery, late upload acknowledgement, account invalidation before and
after effects, wrong digest/device/permission, generation2 cleanup, and assignment
substitution. See the `.run/managed-agent-*` logs for the specific completed runs.

## Activation remains blocked

The existing trusted enrollment, owner-consent, canonical resource reconciliation
and source-writer fencing prerequisites remain mandatory. This adapter does not
replace them or manufacture legacy registrations. No running backend, Desktop
manifest, production resource, or server flag was changed by this checkpoint.

The next distinct stage must bind an assigned-project-admin-approved pending
assignment to native one-shot provision/call custody, actual provider execution,
durable complete-or-unknown usage, Matrix delivery authorization, and real
stop/revoke evidence. No coordinator or resource-owner second human agent
approval is introduced. Resume/reassignment after stop remains unsupported until
the successor-generation protocol is defined and validated.
