---
kind: decision
id: ADR-193
status: Accepted
---

## Context

A shared Agent runs one process attempt at a time. Each attempt names its stopped predecessor, and each queued message waits for the one before it. When the executor could not prove what happened to an attempt, the chain waited for explicit evidence forever. This happens when:
- an attempt was issued but never claimed, because the executor stalled past its 30 s window;
- an attempt was claimed but its process state is unknown;
- a start or a stop was never confirmed.

On 2026-11-06 two pool Agents stopped answering this way. The daemon missed the claim window of two successor attempts. Its cleanup path then fenced them, found no process evidence and recorded nothing, so every later message waited. The v3 budget lane already recorded `not_started` for an unclaimed attempt; the v2 lane did not.

The owner decided that uncertainty must be time-bounded ("不卡死优先").

## Decision

1. **Never claimed: `not_started` at once.** An attempt that was issued but never claimed is fenced and recorded `not_started` as soon as the server asks for cleanup after its window, in v2 as in v3.
   - In v2 the receipt is derived from the immutable unclaimed fence and carries the fence's ledger revision, so every replay uploads identical bytes. No table changes.
   - The server accepts it only on a cleanup ticket after `expires_at_ms`.
2. **Claimed but uncertain: a grace period.** A claimed attempt without stop evidence, an uncertain start and an uncertain stop are bounded by 3 minutes (`UNCERTAIN_GRACE_MS`) after the later of:
   - the attempt's `expires_at_ms`, which bounds every execute lease;
   - its first activity lease, which ends at most 30 s after the window;
   - later process evidence the server holds: activity renewals and work transitions.

   Observe and cleanup leases never extend it.
3. **Within the grace period nothing changes.** The attempt stays fenced. The server refuses a time-bound stop with `shared_attempt_timeout_pending`, and no second process can start for the same work.
4. **After the grace period:**
   - The server stops the attempt by itself, on every executor poll and in its background cleanup worker. It fences the attempt and, if the attempt reported a process, ends the demand that started it (`head_changed`; see the amendment for an attempt that never did). It writes no receipt and no usage, so no executor is needed for this step.
   - The executor records explicit evidence: a `stopped` receipt with `reason: "uncertain_timeout"` that names only the claim's own process digest. Calls close at their known usage; no usage is invented.
   - The next attempt names that receipt as its predecessor stop. After `not_started` its process digest is empty.
5. **No replay.** The interrupted message's demand is terminal on both sides (amended: a message that provably never ran is retried, see below). The executor keeps an authenticated terminal record (`head_changed`, `expired` or `cancelled`) that only releases the message's place in the queue, so later messages run.

## Consequences

Good, because:
- no stalled or lost run can hold an Agent's queue indefinitely;
- the server recovers its own state without a healthy executor;
- every stop still has an explicit receipt, and known usage is unchanged.

Bad, because:
- a process that kept running past the bound may produce a duplicate reply, and the interrupted message gets no reply (the room shows "interrupted, please resend");
- the server bounds uncertainty with its own, coarser view of process evidence;
- executors from before this decision cannot continue a chain past a `not_started` or time-bound v2 attempt, so both sides must be upgraded;
- process receipts gain an optional `reason` field, which older executors cannot parse.

## Alternatives Considered

- **Keep waiting for explicit evidence.** Rejected: one lost run blocks an Agent forever.
- **Let the server write the stop receipt.** Rejected: a receipt carries the executor's ledger revision and process identity, which the server cannot know. The server stops the attempt; the executor supplies the evidence.
- **Store the v2 `not_started` receipt in its own row, as v3 does.** Rejected for now: the v3 table accepts only v3 fences, so it needs a schema migration that an older build cannot open. The unclaimed fence already proves non-issuance.

## Amendment (2026-11-06): retry a message that provably never ran

### Context

Two live incidents followed the rollout:
- Under load, a demand start was accepted but its reply was lost (`management_outcome_unknown`). The attempt was never claimed, ended `not_started`, then `head_changed`, so the message was dropped.
- A run failed mid-flight after it had read the room history.

The owner approved retrying a message that provably never executed, and only that.

### Decision

Three states, each with its own policy:

1. **Provably not executed: retry the same message.** Evidence is an immutable unclaimed fence on the executor (`not_started`), or a start the server never accepted.
   - A start the server never accepted is already replayed with its original start intent; nothing changes.
   - For `not_started`, the executor sends its evidence with an explicit retry request (`POST demands/{id}/retry`). In one transaction the server records the receipt and re-offers the same event: same demand, offer, event and FIFO place, with `state: "retry_offered"` and `retry: n`.
   - The executor then starts retry `n`: a new attempt that names the never-started attempt (empty process digest). Its start intent id is derived, `sdr<n>_<demand id>`.
   - At most 3 retries per message (`MAX_DEMAND_RETRIES`). A fourth `not_started` ends the message as before: `head_changed`, block payload `"retry_exhausted"`.
2. **Outcome uncertain: never call the model again; reconcile.** This covers a start accepted with its reply lost, and a claim with an unknown process.
   - A replayed start returns the same attempt; the poll lane may still claim it in its window, and then it runs once, normally.
   - After the window, cleanup fences the attempt unclaimed: `not_started`, case 1.
   - A claimed attempt may have run: it is never retried. After the time bound it ends `uncertain_timeout`, as before.
   - A server-side start only proves the start was accepted, never that the model produced output.
3. **Model started: no retry, as before.** A claim, a qualified process or any work evidence makes the message permanently ineligible.

The time bound never replays on its own:
- The sweep still stops an expired attempt, but ends its demand only if the attempt reported a process.
- An attempt that never did keeps its demand open (`start_committed`) for the executor's evidence: `not_started` re-opens the message, `uncertain_timeout` ends it. Without the executor nothing could start anyway, because the next start needs the predecessor's stop receipt.
- Only `not_started` evidence sent with the retry request re-opens a message. The same evidence on the plain result route, as older executors send it, ends the message as before.

Revocation still stops at once:
- A stopped Run, a new control revision, a changed device generation or a logout means the server opens no retry, and a running retry loses its authority immediately.
- The executor's retry start needs the same fresh authority as any start.

Local proof on the executor:
- A retry is prepared only when every earlier start of the message has an unclaimed fence and no claim.
- A replay returns the saved retry start, so a lost reply never creates a second start.
- Retry starts are claimed as the demand's own (Run revision pinned, work binding accepted) and are never picked up by manual-start recovery.

### Compatibility

- The server advertises `shared-demand-retry-v1`, and an executor uses the retry route only then.
- New server with an older executor: no retry requests, so the previous terminal behavior. The only change is that a never-qualified attempt stopped by the time bound waits for the executor's evidence; executors with this ADR's first part supply it.
- New executor with an older server: no capability, so the plain evidence upload and the previous behavior.
- The capability is advertised only while every gate of the retry route is on (the route and discovery share one predicate). If the route refuses anyway, the executor uploads the same evidence on the plain result route, so `not_started` evidence is always recorded.
- Wire: `SharedDemandStartInput` and `SharedDemandStatus` gain an optional `retry`, and there is a new state, `retry_offered`, and a new route. Existing bytes and digests are unchanged.
- Retry fields are sent only to an executor that declares retry support in that request (`retries: true` on demand poll, recover and block; declared only to a server that advertises the capability). Any other executor never sees `retry` or `retry_offered`. An unbound retry it cannot run ends the message as before (`head_changed`, block payload `"retry_unsupported"`), and a bound one reads as `bound`.
- Server schema: two additive tables and a replaced demand validator, applied idempotently at every start, with no version change. The previous binary ignores them; a retry in flight during a rollback ends `head_changed`.
- Executor: no SQLite migration. Retry starts live in the existing start-intent table.
- A retry stopped by a budget block ends the message locally once the server confirms the block; the original start's decision cannot change, so a terminal record releases the queue place.
- v3 budget demands keep the previous behavior.

### Rollback rules

- **Executor:** no downgrade below this build once retries have occurred. An older executor reads no retry fields and ends an unbound retry as before, but it cannot recover a binding to a retry attempt: that Agent's later messages wait until the event expires or the owner stops and re-runs the Run. If a downgrade is unavoidable, first let retries finish (bound and recorded locally).
- **Server:** before rolling back, let retries finish: no demand `retry_offered` and no unbound retry start.
  - The previous binary cannot run a retry. It ends an unbound one as `head_changed`, because its refresh sees a newer head than the original start.
  - It reports a demand bound to a retry attempt with the original start's runtime, which this executor rejects. If such a demand is still unbound locally, stop and re-run the Agent's Run to release it.
- **Never drop the retry tables.** Their rows are the only record of which attempt a demand started and which runtime its binding names. The validator, `current()`, `allowed()` and the sweep read them.
- `validate_shared_demand()` is owned by the retry schema file and re-created at every start. Startup refuses to replace a body that a later versioned migration changed, so a later change is never silently undone. A future change to that function belongs in the retry schema file.

### Known limit: start slots of ended messages

A start intent saved locally but never committed by the server (the Run stopped or the event expired before the server accepted it) holds its head's start slot (`owner_shared_start_intent_period`) for the rest of the period. Later messages naming that head wait.
- Original starts already have this limit.
- A retry adds the same exposure once per retry, only in the short window between saving the retry start and the server committing it.
- Releasing the slot of an ended message needs a client migration (the slot is a unique index), so it is left as a follow-up.

### Consequences

Good, because:
- a lost start or a missed claim window no longer loses the message, which keeps its place in the queue;
- the model runs at most once per message.

Bad, because:
- a never-started attempt now holds its Agent's queue until the executor reports, not for the 3-minute bound alone;
- a message can be retried up to three times before it ends;
- the protocol has one more route and state.

