# Successor process attempts after a normal close

This is an additive ledger checkpoint, not an enabled runtime protocol. The paired session-resume
work preserves the original native device across normal close, saved-account
switch and resource adoption. It does not recreate the process right issued to
an earlier shared execution command.

## Existing boundary

The version 1 central `management_shared_runtimes` table has one row per
assignment. Native schema 80 likewise has one `owner_shared_runtime_lifetimes`
row per child and a unique assignment. Both preserve the original command and
one-shot claim. Native `shared_execution/work.rs::live` refuses a child after a
stopped observation; the original `owner_managed_effects` provision evidence is
also a single effect. A different command ID alone must not bypass these facts.

A central stopped process receipt fences the runtime row and ends its activity;
it does not by itself fence the assignment or original agent reservation. This
allows a separate future attempt while the approved agent remains valid. Agent,
assignment, project, parent-grant and device fences are still absolute. An
explicit logout or earlier generation rotation cannot be repaired by this path.

## Smallest usable protocol

Add a separately versioned process-attempt protocol for an otherwise unchanged
approved agent. Keep assignment generation, native device generation, reservation
ID, original reserve command/receipt, provider account, resource revision, policy
and budget unchanged. A monotonic `attemptNumber` is a new process identity, not
a way to rewrite any of those authorities.

A proposed closed command binds:

- New attempt/command ID, assignment ID and original agent reservation ID.
- Original reserve command and receipt digests, original grant/enrollment scope,
  current exact same-device authorization and the unchanged text/no-tools policy.
- Previous attempt ID and command digest, its exact stopped-observation digest,
  and an exact retained-liability manifest digest.
- Attempt number, immutable issuance time and a fresh spawn window of at most
  30 seconds, bounded by the original reservation/grant/delegation period.

Version 1 becomes the immutable predecessor of the first successor. It is never
updated to a new command or made runnable again. The new protocol needs paired
closed DTOs and canonical fixtures; do not add fields to old hashed commands.

The initial implementation should issue a successor only when all of these
conditions hold on both central and native sides:

1. Fresh SDK/account/device proof and current resource/provider identity match
   the original hold. The original owner grant and assigned-project-admin agent
   approval still cover the unchanged agent. No second coordinator or owner
   approval is introduced.
2. The exact predecessor has a durable observation of actual process exit/reap
   from its retained supervisor. Its activity is permanently ended. A timeout,
   missing PID, empty manager map, restored session or absent journal is not
   equivalent evidence.
3. All previous work, uncertain start responses, call usage and replies remain
   pinned to their original attempt. An observed stopped process cannot execute
   them again. An absent row or missing usage is not pre-issue cancellation
   evidence. A pending or unknown reply is still reported as such; its work is
   never moved to the successor.
4. Before a new model call, a typed aggregate of the same canonical child budget
   proves positive remaining headroom after every prior call's consumed amount
   and conservative hold. Incomplete but representable usage remains held and
   does not by itself prohibit a new process or independent new work. Overflow,
   unsupported or unrepresentable liability blocks fresh model work. Starting
   another process does not allocate another child hold, refund the old one or
   reset consumption. A reservation is accounting protection, not a provider
   hard limit; genuine overruns must still be recorded.
5. Current central Matrix membership, setup/admin binding and execution policy
   are valid. The check happens before issuance, at process qualification and
   before every model start/renewal, as in the first-turn protocol.

Condition 2 keeps crash-unknown process custody blocked. A claim committed just
before a crash cannot be interpreted as a proved unstarted process. Later
reconciliation must use independently qualified evidence; restart cannot
manufacture a stopped receipt. Old usage and reply uncertainty are distinct:
with exact stopped-process proof they remain conservatively held and
non-replayable, while genuinely new work can use independently proved remaining
capacity. The current first-turn policy reserves all available child capacity,
so an interrupted turn commonly leaves no headroom. That is a budget constraint,
not a reason to resurrect or permanently redefine the old process authority.

## Durable authority and accounting

Use additive native attempt, observation and call-to-attempt pin tables in the
same canonical ledger. Keep schema 80 rows and receipts byte-identical. Give each
new provision attempt its own immutable effect identity; do not reset the
original provision effect from qualified/stopped to pending. A new typed current
attempt proof must be checked inside call admission and issuance, alongside the
existing original-agent authority and budget checks. Deserialized attempt
history grants no execution right.

The first claim transaction verifies the predecessor, current head and exact
liability manifest, then inserts a unique `(child, attemptNumber)` and unique
predecessor link. It returns non-cloneable custody once. Exact replay returns
history; a replacement command ID for the same predecessor cannot create a
parallel attempt. A lost claim response leaves conservative uncertainty.

The central side needs equivalent append-only attempt history, one active head
per assignment and compare-and-set predecessor closure. Its delivery lease is
execution authority only for the new attempt; history/recovery remains upload
only. An explicit cancellation or business fence must stop all attempts and
prevent delayed delivery from creating another one.

Existing `owner_managed_calls` remains the budget authority. Add an immutable
attempt pin to each fresh execution ID and retain all earlier calls under the
same child. Call headroom is original allocation minus the existing collector's
consumption/holds, never the latest attempt's usage alone. Extend process
liability collection to every attempt: any missing stopped proof or unknown
process retains uncertainty. Parent/project release must observe that aggregate.
Late original usage still increases known consumption; it cannot be discarded
because a successor exists. Recheck capacity at each effect boundary and retain
any genuine overrun.

Work IDs remain unique for the assignment/event across attempts. A previous
`start_requested`, uncertain start response, issued model call or completed work
cannot be rebound to a new runtime digest. Only new Matrix events can become new
work for the successor. Repeated delivery or reply recovery never reruns the
model. Preserve the original threaded Matrix transaction IDs for old replies.

## Native runtime and Desktop integration

Keep the existing retained task manager and close/drain semantics. Key entries by
new attempt command ID, with exact digest. Build a fresh supervised process from
fresh current provider metadata, then bind its actual process source to the new
one-shot custody. Never reconstruct a live source from the old process journal.
A normal close suspends credentials but stops and observes every current process.
The next Desktop instance may discover an eligible successor using the same
native-only executor; mini-apps receive only status, not an execution token or
consent boolean.

To avoid idle restart loops in the first slice, require an actual current start
intent or new authorized Matrix work to request a successor. Recover and report
predecessor stop/usage independently of new-work discovery. Retain fair cursor
scheduling across assignments so one blocked predecessor cannot starve others.

The current central version 1 work item is inserted only for a qualified runtime
and already hashes its runtime command. A message-driven successor therefore
needs a separate bounded pre-runtime event inbox: pin the exact assignment,
manager, room/thread, event ID and body first, then atomically create the final
work item once the new attempt qualifies. Deduplicate that event across waiting,
issued and completed history. Do not rebind an existing version 1 work item or
regenerate an event after a lost start/reply. An explicit native start-intent
alternative must likewise be durable and consumed once, not inferred anew by
every poll or restored session.

Expose distinct read-only reasons: stopped/eligible for a new attempt,
process-stop unknown, usage reconciliation required, reply outcome unknown,
capacity exhausted, original authorization changed, and expired original grant.
Do not label those states as a generic reconnect failure or imply refunded
capacity.

## Focused acceptance

- Normal native close and a new process retain the original device generation,
  resource/grant/child IDs and original receipt bytes. After complete usage and
  actual stop, exactly one successor can claim new custody.
- Concurrent issuers in two native processes and central transactions race the
  same predecessor; only one new effect wins. Exact or replacement-ID replay
  never returns a second process right.
- Cancellation before claim, after commit/before reply, during spawn, and after
  qualification preserves uncertainty. Recovery alone never launches a process.
- Lost stopped-observation upload is recovered and acknowledged before a fresh
  attempt; an old qualified central projection cannot hide a newer local stop.
- Missing exact stopped-process proof blocks a successor. Unknown call usage,
  central start reply loss, incomplete output and pending reply retain their
  original records and conservative charges. They cannot authorize old work in
  the successor, but do not forbid an independent call when positive aggregate
  headroom remains. Unrepresentable/overflow liability blocks fresh model work.
- A 100-token child with an incomplete 40-token call keeps the full 40-token
  liability and can admit no more than the remaining 60 tokens after actual
  stop. An incomplete all-headroom call leaves zero. Known overrun reduces
  remaining capacity to zero without clamping the recorded consumption.
- Completed first-call usage reduces the second call's available budget. A late
  positive observation and a true overrun are conserved across all attempts and
  through project release/parent settlement.
- Old event/work IDs, old epoch tokens, old device generation and old process
  sources cannot authorize a new model turn. Changed project admin, policy,
  provider, resource or revoked grant fails closed.
- Original metadata-only runtime and version 1 text rows stay immutable and
  mutually exclusive. Schema upgrade rollback retains every old hold, command
  and receipt; unknown new histories remain adoption blockers.

Native schema 81 / migration 083 is now registered with immutable successor
attempts, process observations, call pins and exact cancellation tombstones. The
first frozen ledger checkpoint accepted version 1 to attempt 2, reread the exact
stopped predecessor and aggregate call liability inside the writer, and returned
one-shot custody once. That checkpoint exposed no actual-process binding or
model-call API.

The paired canonical version 2 command/process fixture matches independently on
the native side. Full store validation passed 254 tests, with six existing
ignored tests, including migration rollback/reopen, immutable original holds,
exact historical recovery after a generation change, concurrent claims, stale
budget refusal and audit/archive blockers for all new histories. Logs are in
`.run/shared-successor-schema81-store-final.log`; this result is not provider or
Matrix execution evidence.

Exact process-only cancellation now commits a permanent semantic fence before a
delayed leased claim can issue. Both the native writer and a SQLite trigger deny
later claims for that attempt/predecessor/start intent. Cancellation after claim
records that original custody existed and retains its uncertainty. Exact replay
keeps the first fence revision, including after restart and generation change;
no cancellation creates a stopped-process receipt or refunds old calls.

The separate observer checkpoint now binds each claimed attempt to one actual
supervised process source. Clones cannot bind another child or attempt. Binding
after expiry retains evidence only: qualification still checks current native
authority in the writer, while actual stop can be recorded after retirement.
Version 2 qualification/stop records use distinct global ledger revisions and
leave the version 1 provision result unchanged. An exact latest stopped version
2 predecessor permits attempt 3; original work IDs, receipts and incomplete
charges remain bound to their original attempt. History never recreates custody.

The focused successor suite passed 17 tests, including four actual supervised
offline-process cases. They cover attempt 3 with a retained incomplete 40-token
call, original-source reuse refusal, foreign source rejection, queued retirement,
permanent fencing and late stopped-record recovery after restart. No model or
provider credentials are used by these fixtures.

The observer-only checkpoint stopped before full account/Seat conservation,
durable native start-intent custody and the version 2 transport/controller. Its
observation API cannot admit a model call. The following separate checkpoint
adds those native boundaries; actual provider/Matrix repeated-turn acceptance
still requires paired transport and isolated-fixture activation.


## Default-disabled native controller checkpoint

The next native slice registers schema 82 / migration 084. It saves one immutable
accepted-start request per original child and stopped predecessor before HTTP.
Restart recovers the exact original request and identifier; a poll never creates
a new intent. A lost start reply cannot cause a replacement request or recreate
process custody. Fresh authenticated delivery still needs the exact native
journal, current original enrollment, and the one-shot attempt claim.

The closed attempt routes reuse the existing SDK-owned device transport and
finite send-anchored upload deadlines. Cleanup deliveries persist the exact
permanent attempt fence before looking for a retained process, including when
no claim exists. Historical delivery cannot execute or create cleanup authority.
A fence without actual process evidence remains unknown; it is never translated
to a stopped receipt. Actual stop is uploaded before old work reconciliation, so
an unavailable work-result endpoint cannot hide known process termination.

New call admission uses the original child/call ledger and the actual successor
process source. It rereads the current grant, project, resource, provider, Seat,
original call pins, and aggregate liabilities in the writer. Supported bounded
sibling calls are conserved inside their original parent holds and do not cause
a blanket incompleteness refusal. Unsupported accounting, unrepresentable usage,
and pool/Seat overruns deny new issue. Settled but incomplete calls retain the
unreported part of their original holds in both child and parent projections.
Legacy engagement rows on the same resource or Seat remain unsupported in this
canonical call path rather than being assumed empty.

Only an exact `newlyStarted=true` response for the saved work can issue one model
call. Lost or replayed start replies retain the hold and never rerun the model.
The sent text, actual source, runtime digest, and per-call usage/output remain
bound to that work. Renewal retains the same pending event read; retiring the
owner stops/reaps the original child while preserving uncertain usage. Process
stop is independent of usage completeness and never refunds an unresolved call.

The `shared-successors` Cargo feature is separate and default-off. Neither
`shared-execution` nor onboarding enables it. This checkpoint exposes native
start/poll/recovery methods only: no Desktop scheduler, mini-app capability,
production profile migration, real provider prompt, or Matrix repeated-turn
acceptance is included. Offline children and a fixed local HTTP fixture qualify
the implementation; the paired central schema 14 fixture is unchanged.
