# Native parent grant cleanup

The additive owner-ledger implementation is in
`native/hagency-store/src/domain/owner_resources/capacity/parent.rs`. It is not
connected to a transport, advertised by a session capability, or exposed as a
mini-app service. It does not allocate or run agents.

`VerifiedParentDelivery` requires the same native authenticated enrollment and
short execution lease as the child reservation path. An ordinary parsed command
or a recovery response with `executionAllowed:false` cannot authorize a write.
The writer rechecks the proof after waiting in its queue, before commit and after
commit. Lost authority after commit reports an unknown outcome rather than
pretending no effect occurred.

`grant.close` establishes a permanent fence in the same SQLite transaction as
the exact receipt and capacity projection. Every distinct parent or child
command gets a unique, monotonically increasing ledger revision. Exact replay
returns its original revision and receipt; it never overwrites the latest
settlement. The current parent result and exact historical receipt have separate
read-only APIs, both scoped to the original Matrix profile and installation.
Cleanup can use a fresh execution generation and changed resource configuration
after access or coordinator changes; it cannot reopen a reservation grant.

The original grant receipt and allocated amount remain immutable. The first
close retains all live child reservations. Child cleanup has its own command
and revision; the parent keeps its previous hold until an explicit
`grant.settle` commits a complete new snapshot. That command must bind the exact
latest settlement digest. Delayed or unseen reserves are rejected after the
permanent fence, independently of the grant's active flag. Parent cleanup never
fabricates child receipts.

Schema 74 is file `076-owner-parent-settlements.sql`: filenames 074 and 075 were
already used by older board-numbered migrations. Existing grants and children
receive accounting mode `unknown`. Migration does not reinterpret an old hold as
unused. New grants and children explicitly use `reservation_only_v1`, whose
complete current API has no execution or agent attachment operation. Both modes
and the expected reserved/released balance shape must match before a child can
be released with zero consumption. A future executor must change this durable
mode atomically before work and implement actual consumed/uncertain settlement.

Any unknown or inconsistent child accounting keeps the entire original parent
allocation as unresolved, alongside any previously proven consumption and any
greater unresolved liability. This deliberately over-retains until that
accounting mode has a supported settlement path. Grant expiry, UTC rollover,
resource-access revocation, and missing replies never refund it. Independent
legacy engagements on the same provider seat continue to be counted by the
existing allocation engine. The current parent liability replaces only that
parent's original hold in the same resource-preset and provider-seat accounting;
child reservations are not counted twice.

The liability digest commits a versioned canonical manifest. Its
`childrenDigest` is SHA-256 of the canonical JSON array of all child rows in
ascending allocation-ID order, including released tombstones. Each row contains
`id`, `state`, `requestedTokens`, `retainedTokens`, `bindingDigest` and
`accountingMode`. The manifest also binds grant ID, parent accounting mode,
child count, unknown-accounting flag, known consumed tokens, unresolved tokens
and liability count. At most 100,000 children are streamed; all token arithmetic
is checked against the JSON-safe integer bound. The central server validates
the digest shape and receipt conservation; it does not interpret this private
native manifest.

The copied server wire fixture tests exact command and receipt digests. Isolated
SQLite tests cover concurrent close/reserve orderings, delayed unseen reserve,
explicit child release then settlement, historical replay after newer results,
restart and wrong-profile/device recovery, lease/profile loss while queued,
unknown and inconsistent accounting, schema-73 migration and rollback, and
preservation of unrelated legacy holds. Server tests separately cover both
parent/child receipt-arrival orders. These are not an end-to-end transport pass:
authenticated delivery/reporting, owner consent, legacy-account reconciliation,
live shared-account conservation and executable-agent settlement remain gates
before activation.
