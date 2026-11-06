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
   - The server stops the attempt by itself, on every executor poll and in its background cleanup worker. It fences the attempt and ends the demand that started it (`head_changed`). It writes no receipt and no usage, so no executor is needed for this step.
   - The executor records explicit evidence: a `stopped` receipt with `reason: "uncertain_timeout"` that names only the claim's own process digest. Calls close at their known usage; no usage is invented.
   - The next attempt names that receipt as its predecessor stop. After `not_started` its process digest is empty.
5. **No replay.** The interrupted message's demand is terminal on both sides. The executor keeps an authenticated terminal record (`head_changed`, `expired` or `cancelled`) that only releases the message's place in the queue, so later messages run.

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
