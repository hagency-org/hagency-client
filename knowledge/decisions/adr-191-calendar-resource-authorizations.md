# ADR-191: Calendar resource authorizations and persistent Agent identity

Status: implemented, validated and deployed to River and the owner's MacBook on 2026-11-02.

## Decision

The resource owner authorizes a token allowance per natural day, week or month
in an explicitly selected IANA time zone. Weeks begin on Monday. Windows are
half-open: start inclusive, end exclusive. Daylight saving transitions are
calendar transitions, not fixed durations. A nonexistent midnight resolves to
the first valid instant after it; an ambiguous midnight uses its first occurrence.

Allocation selects automatic renewal or owner renewal. These terms require
resource-owner approval. An applicant cannot authorize recurring spending.
Automatic renewal is standing owner consent, not a grant of authority to the
applicant or server. Manual renewal authorizes the current new period once; it
cannot replenish an exhausted current period. Neither mode carries unused
tokens or credits missed periods when the executor was offline.

Agent identity, Matrix membership, room bindings, encryption keys, working
directories and history outlive individual resource authorizations. Exhaustion,
expiry and waiting for renewal pause admission of new work. They do not fence
the identity or leave its rooms. Explicit revocation/deletion continues to fence.

## Accounting and transport

Each owner-approved policy has an immutable revision and exact canonical digest.
Each period authorization records its policy revision, local calendar key,
UTC start/end, amount, predecessor, command ID and native owner receipt.
A unique policy/period key prevents duplicate issuance across retries, concurrent
daemon instances and owner actions. A retry replays the same command and receipt.
Changing cadence, zone or allowance cannot create another authorization for an
already funded period. A published resource retains its calendar; a different
calendar uses a separate resource. Agent period amounts are fixed by their
owner-approved allocation; additive top-ups are unavailable for recurring Agents.

Calls pin their authorization when admitted. A call crossing the boundary stays
charged to its original period, including late usage. Settled consumption from
old periods is historical. A period change never rewrites a previous receipt,
clears an irreversible fence or manufactures settlement.

Recurring Agents hold only known usage. A call's reservation is not charged
against the allowance, and new work is refused only when known consumption
reaches the period's amount. Once a call's process is confirmed stopped, usage
that never arrived counts as zero: the call closes at its known figure, with that
evidence retained, and a later report still adds to it. A process that cannot be
confirmed stopped stays unresolved. The owner chose this on 2026-11-05 so that an
interrupted run cannot stop an Agent. The ledger can therefore understate real
provider usage, and one run can pass the allowance by its own usage. Cumulative
Agents follow the same rule: no Agent holds a call's reservation.

The executor checks current owner approval, effective parent capacity, current
period, revocation and token availability transactionally before admitting work.
The server and UI display these facts but cannot mint native credit. Renewal
failure leaves the Agent paused with a specific reason and a retryable command.
Authority changes and explicit revocation invalidate outstanding work admission.

## Compatibility and recovery

Existing signed v1–v3 receipts retain their exact canonical representation and
validation. Existing one-off allocations are not silently converted to recurring
authority. Recovery of an already fenced Agent requires a new owner-authorized
allocation generation for its existing identity, not removal of the old fence.
Provider/account hardcoded budgets are not introduced.

## UX and acceptance

Allocation shows tokens per period, cadence, owner time zone, automatic/owner
renewal and the first reset time together. Details distinguish available,
current-period exhausted, awaiting owner renewal, awaiting executor/capacity and
revoked. Exhausted views show the next reset in the owner's zone. The manual
renew action appears only when a new period can be authorized.

Before enabling: verify native/server contract parity; DST and year boundaries;
offline skips; concurrent/replayed renewal; late and unknown usage; revocation;
unchanged Agent/room identity; migration from legacy grants; and headless native
allocation/detail screens in English and Chinese. A calendar helper alone is
not completion of this ADR.

## Implementation

Integration, compatibility, release-build and deployment validation are complete.
Existing one-off authorizations remain unchanged; recurring terms still require
the resource owner's explicit confirmation.

- V4 native receipts and central publications bind `calendar:{cadence,timeZone}`.
  Receipt and child-command canonical vectors are shared between native/server;
  omitted optional fields retain legacy bytes. The native resource calendar is
  separate from the immutable offered resource definition (schema108).
- The native owner publication confirmation explicitly grants standing capacity
  on the selected calendar until withdrawal. The daemon and in-process Desktop
  host call `OwnerGrantAccess::renew_recurring`; only a published V4 predecessor
  can authorize an unchanged renewal. Each successor uses the existing native
  consent, capacity transaction, durable command and publication recovery flow.
  No missed period is backfilled and no legacy receipt silently opts in.
- An expired publication with an unknown result first recovers the original
  command. If absent, a server transaction serializes an immutable
  `expired_unpublished` tombstone against publication, providing definitive
  proof before the native pending successor is rejected. Time alone does not
  prove absence or authorize release of unknown liability.
- Original Agent allocation records `renewalMode:automatic|owner`. Central
  automatic scheduling and the owner-only manual endpoint enqueue the same
  `agent.period.renew` command. Its native receipt must precede new execution.
  Unique disjoint Agent/period rows prevent repeat credit across command retries,
  concurrent requests and replacement allocation generations (schema107).
- Each call pins its immutable period. Settled old consumption remains history;
  old unresolved calls remain charged to their original funding grant. Late
  settlement cannot replenish the new period. A current period's allowance is
  checked on preparation, issue and successor execution. Unknown consumption
  remains unknown in projections rather than being displayed as zero.
- Expiry or exhaustion pauses new work while preserving Agent/Matrix identity,
  room bindings, keys and work directories. Reauthorization of an old fenced
  Agent creates a new allocation generation; old fences, receipts and liabilities
  remain immutable. Additional room bindings are copied for fresh membership
  validation before rejoining, including rooms left by the previous expiry path.
  Additional rooms are restored only with proven expiry cleanup or fresh
  membership evidence. Deliberate removals and legacy departures with no known
  cause require a fresh invitation; the UI reports how many rooms need it.
- Period-bound start intents (schema109) and demand admission prevent stale
  exhausted messages or an unsent previous-period startup from blocking fresh
  work. Unknown already-issued processes still require explicit stopped/no-start
  evidence before replacement.
- An expired initial runtime command that was never issued can be replaced
  only after the native ledger records an immutable `not_started` tombstone
  (schema110). The transaction rejects any prior process/issuance/call custody.
  The server retains the old command and proof, then permits a fresh initial
  command under current authority. Delayed originals and clock rollback cannot
  bypass the tombstone; historical proof recovery survives the replacement.
- Publication exposes day/week/month and IANA zone selection, with inline invalid
  zone feedback and fixed calendar readback for an existing resource. Allocation
  requires an explicit renewal mode. Agent detail shows per-period allowance,
  used/remaining or unsettled usage, owner-zone reset, manual renew and legacy
  reauthorization. Reversible top-up is replaced by a fixed-period explanation.

Explicit shared account ceilings remain binding. That existing ledger uses UTC:
when an owner sets an account-wide cap, all resources sharing it must use UTC
and the same cadence. Native reads and grant writes reject incompatible sibling
calendars, including calendars published before the account cap was added. This
prevents a local reset from freeing spend inside the account's still-open UTC
period. The UI explains the blocker; no owner cap is silently removed. Without
an explicit account-wide cap, arbitrary supported IANA resource zones remain
available. Independent account/resource calendar accounting is outside this ADR.

## Verification record

The calendar contract covers 24 cases per implementation, including DST,
nonexistent and repeated midnight, skipped dates, Monday week/year boundaries,
leap months, offline skips, duplicate refill and late/unknown usage. Native and
PostgreSQL suites additionally exercise actual ledger writes, authenticated
renewal, replay, revocation and same-identity generation recovery. Native UI
journeys exercise the actual shipped Splash/widgets in English and Chinese,
including narrow and wide windows. Final build/regression evidence is maintained
under `hagency-desktop/.run/recurring-periods`; it must identify remaining failures
and production deployment separately from implementation status.

Local acceptance evidence (2026-11-02):

| Area | Result | Evidence under `hagency-desktop/.run/recurring-periods` |
| --- | --- | --- |
| Desktop integration | 67 groups passed, including 372 Rust tests | `final-regressions/report.json` |
| Native schema/runtime | 115 latest-schema tests passed, including actual process rollover | `evidence/hagency-native-final-schema110-fixed.log` |
| Owner publication | 91 tests passed | `evidence/hagency-recurring-owner-final2.log` |
| Native combined features | Initial run: 665 passed, 14 outdated schema-fixture failures; corrected affected group: 39 passed, including all 14 | `evidence/hagency-store-combined-full.log`, `evidence/hagency-store-combined-fixture-rerun.log` |
| Server | Full run: 446 passed, 2 encrypted-ingress clock failures; corrected room suite: 5 passed on each of 3 successive runs; encrypted/E2EE filter: 7 passed | `evidence/period-server-full-tests-final.log`, `evidence/period-server-room-clock-tests.log`, `evidence/period-server-room-clock-repeat2.log`, `evidence/period-server-room-clock-repeat3.log`, `evidence/period-server-encrypted-final.log` |
| Native UI | 20 allocation/detail, 12 publication, 8 reinvitation and 4 account-calendar blocker cases passed | `native-recurring-reviewed/report.json`, `native-publication-final/report.json`, `native-reinvite-final/report.json`, `native-account-period-mismatch/report.json` |
| Script VM | 38 allocation checks and both full English/Chinese bundles passed | `allocation-script-reviewed/report.json`, `full-bundle-shipping/report.json` |

The native UI checks use actual headless widgets at narrow and wide sizes, with
retained screenshots and interaction assertions. They are not an external UX
score or a claim that live Matrix/provider accounts have been exercised.

The server failures exposed an existing cross-clock comparison: ingress stamped
by the server process could be rejected when PostgreSQL lagged by 1 ms. The
correction validates future ingress against its source clock without changing
the retained timestamp or database authorization/period checks. A deterministic
regression covers both that skew and genuinely future ingress. The full-suite
failure record is retained; it is not represented as a single clean full run.

The local Desktop and Hosting release bundles are in `shipping/`, with hashes
and signing/build reports. The corrected server release binary also passed its
version smoke check; its hash is in `shipping/server-build-report.json`.
Those original local candidates were superseded by the deployed builds described below.
The machine-readable `implementation-status.json` distinguishes remaining work,
known limitations and deployment state. Existing server strict-clippy findings
and OpenAPI source-hash drift are recorded separately from these feature checks.

## Deployment record

River release `20261102T195109Z` is active with management schema 30. Its restored
database rehearsal passed 29 → 30 → 29 → 30 and reopening; the production
migration preserved the original records across 98 tables. The six immutable
native command/grant/receipt tables also remained unchanged after startup.
Public and private health, readiness, Matrix and discovery checks passed. The
previous release, database backups and existing service identities were retained.
Evidence is in `hagency-server/.run/river-recurring-20261102T195109Z`.

The owner's MacBook now runs `~/Applications/Hagency Desktop.app` with the
existing Edward and dspfac profiles, plus `~/Applications/Hagency Hosting.app`
and its independent LaunchAgent. These profiles require the existing isolated
installation support; the installed local bundles preserve those exact roots.
Both active native ledgers upgraded 106 → 110, preserving 1,340 original rows in
the offline rehearsal. The protected schema-99 archive remains unchanged.

Live integration exposed a preexisting remote-management handle leak: each
chat-permission poll created a manager that was not retired, eventually filling
the daemon's 64-manager bound. Temporary poll handles now retire explicitly, and
final-reference cleanup releases the original remote lease even after a cancelled
or failed request. All 18 bridge tests passed, including 96 consecutive polls,
retained clones and cancellation. After replacement builds, both accounts and
Hosting ran together for over 150 seconds; owner management and all three pools
remained readable and refreshable with no `daemon_bridge_busy` errors.

Final installed hashes, running processes, backups, captures and exact results
are in `hagency-desktop/.run/recurring-periods/deployment`. Deployment did not
create new resource authorizations, convert legacy grants or send test messages.
