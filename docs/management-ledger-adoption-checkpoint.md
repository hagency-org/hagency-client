# First identity-preserving resource adoption checkpoint

Date: 2026-10-16. Status: adoption implementation proposal, following the completed read-only source census. The finite authority and journal-only archive prerequisite below is implemented and unconnected to host flows. No real source has been fenced, imported or activated. This narrows `management-ledger-reconciliation-next.md`; it does not authorize live migration.

## Result to implement

An existing owner can explicitly transfer the current profile's resource definitions into the canonical OS-owner registry while retaining their resource IDs, configuration revisions, central contribution IDs and original private approval receipts. The old resource/personal allocation paths become permanently unavailable for those sources. The owner can still read the resources, recover completed commands and use the existing central project-request flow after restart.

The first transfer accepts one reviewed provider account, one exact original Seat, and the entire resource inventory of one current authenticated source profile. Every resource must use that Seat. A multi-Seat profile is rejected by this first transfer; fencing the whole source database must never strand an unselected resource. It requires no resource, project, agent, execution, usage, capacity or uncertain history in either selected ledger. Completed configuration, publication and admission/project command custody is allowed; an otherwise populated source is not rejected merely because the onboarding flow succeeded. A second distinct Seat for the same provider, unknown personal evidence, an unresolved intent or a conflicting destination account mapping blocks this transfer. Existing allocation/managed-agent history is not imported by this checkpoint.

Native grants, background allocation polling and agent execution remain unavailable until their own production integration passes. Transfer success means an adopted definition and preserved custody, not allocated capacity or a running agent.

## Findings from the current code

- `journal_profile_directory` now validates the exact private profile namespace and current or supported legacy marker without opening the personal SQLite database or rewriting the legacy issuer. Configuration and Matrix-creation journals use it, preserving their existing filenames. A personal refusal sentinel therefore need not block these journals.
- `NativeManagement` currently stores a profile-bound `DomainStore`; its `Session` combines package authority, resource methods and private intents. Replacing that store with the installation registry would expose unrelated resources and move command custody across profiles. This is the adapter that must change.
- `management::intents::execute_intent` already operates on a connection through a closed command. Its custody SQL can be shared with a journal-only worker after extracting its authority check from the resource store. The worker must not expose `DomainRepository`, arbitrary SQL or resource methods.
- `OwnerResourceRegistry::prepare_enrollment` currently derives `seat_owner_<provider digest>` even when a mapping might exist. Adoption must make enrollment resolve `owner_resource_accounts` first; otherwise keeping the original Seat during import would be defeated by the next model enrollment.
- `NativeOwner::shutdown` returns no drain result, and `management::Registry::shutdown` logs a failed `DomainStore::shutdown`. Neither result is sufficient evidence to write a migration fence. The adoption coordinator needs an acknowledged writer drain, retained handles and original ownership-lock reacquisition.
- `Ledger::open` creates a database when no file exists. For an existing file it checks its application ID before schema initialization. `DomainRepository` likewise rejects a foreign application ID before domain migration/recovery. A distinct archive identity can fence these openers, but a sidecar manifest alone cannot fence the absent-file case.
- The real installed Codex identity probe returned supported, stable observation and a trusted effective origin. No account identity was logged. A fresh proof is still required for each preparation, and execution-time identity binding remains separate.

## Typed boundary

The proposed types below are native Rust types, not deserializable mini-app arguments. The UI receives only a safe summary and an opaque review handle.

| API | Responsibility |
| --- | --- |
| `NativeOwner::prepare_resource_adoption(package, selection, host_gate)` | Read-only inventory and current SDK/provider checks; returns `PreparedResourceAdoption` or explicit blockers. Does not open or recover a source through a normal repository. |
| `PreparedResourceAdoption::summary()` | Resource IDs/revisions, one declared account ceiling, source counts, completed-custody counts and personal-runtime consequence. No provider subjects, paths or private receipts. |
| `PreparedResourceAdoption::approve()` | Consumes one native-owner confirmation, persists the original migration ID/plan and advances through source fencing and destination import. |
| `NativeOwner::recover_resource_adoption(package, migration_id, host_gate)` | Reopens only the exact saved plan. Returns/resumes its durable phase; cannot change source definitions, account identity or destination under an old command. |
| `PrivateManagementJournal` | Original profile/device-scoped prepare/read/reply/acknowledge operations and exact historical native receipt lookup. No resource or execution entry point. |

`selection` is constructed by trusted host code from retained census roots and exact resource selections. A mini-app cannot submit a source path, schema version, account digest or “complete” census assertion. The plan is bound to exact source/root physical identities, authenticated profile, original installation, source schema, canonical Seat/Resource definitions, custody inventory digest, provider digest, destination registry identity and explicit resource ACL.

The migration's finite host gate must be independent of the ordinary source manager's retirement. Deliberately retiring the old resource manager is part of migration; changing SDK account, package grant, installation or host epoch still revokes migration authority immediately. Capturing a normal management Session and then calling `retire()` cannot serve both purposes. The host owns this distinct reviewed migration handle; it is not a new bearer or network credential. No network effect is needed between local source fencing and import. Recovery after an expired gate requires fresh authorization for the same saved plan.

## Resource authority and private custody

Extract the existing finite context/permission/retirement scope into a storage-independent authority object. Both the legacy resource Session and the journal-only Session use it; existing exact-operation checks, actor scope, command digest, reply immutability and post-queue checks remain unchanged.

`NativeManagement` selects a fixed storage mode from a validated local adoption record:

| Mode | Resources | Private command custody |
| --- | --- | --- |
| Unadopted | Existing profile DomainStore, subject to the installation authority gate | Existing profile journal |
| Fenced / transfer incomplete | Explicit unavailable result; no normal domain opener | Validated archive journal if available; otherwise recovery-required |
| Adopted | Canonical registry methods with exact profile ACL and resource-ID checks | Original profile archive journal |

The archive journal opens only a source with the dedicated archive application ID, exact original profile, matching migration manifest and retained original `domain.lock`. Its bounded worker calls only shared custody functions. Native receipts in `management_commands` remain immutable. A completed source result is not replayed as a destination effect. Original central commands still resolve current server visibility/result using their original command ID. Pending commands are a preparation blocker, so migration does not have to guess whether an unrecorded effect applied.

First-checkpoint resource operations are list/get and verified connection checks. Unsupported post-adoption configuration/enrollment changes return an explicit migration-required or unsupported result until they have canonical-registry implementations. They must never fall back to the archived legacy writer. Central catalog, project requests, approvals and their journals continue to use their existing authorization rules.

**Installation-wide prerequisite:** switching one profile is insufficient. Current native resource writer entry points must acquire an installation authority lease before issuing legacy resource mutations. Once adoption is prepared, all relevant leases are retired; canonical mode prevents another current-code profile/process from creating an independent legacy Seat for the same local account. A second process that cannot own the registry may keep central/private-journal operations available but cannot manufacture another resource ledger. Existing unidentified profiles/roots remain blockers rather than receiving an inferred account mapping. Already-running downlevel writers must be drained; a new flag cannot revoke an arbitrary old binary or copied database. Owner-reviewed root coverage is recorded as a scope limitation, not upgraded from the census's `external_roots_complete: false` into a factual discovery guarantee.

## Durable phases and exact effects

1. **Prepared.** In one canonical registry transaction, save the closed plan and original migration ID with destination access inactive. Do not insert a new Seat, resource ACL or capacity hold. Reject provider mapping/Seat/resource conflicts here. Re-read provider/SDK scope and expiry before committing preparation. A repeated command recovers the exact plan rather than creating a replacement.
2. **Quiesced.** Stop new source operations, retire their captured native resource permissions and drain all personal/runtime/provider writers for the selected roots. Obtain an explicit domain-worker acknowledgement and reacquire each original domain lock. Keep root locks and live source handles. Re-inventory the committed WAL under these locks and require exact plan hashes; shutdown timeout, missing evidence or file replacement stops here. This phase is transient evidence, never a durable assertion that future writers cannot exist.
3. **Sources fenced.** In each existing source, commit a versioned immutable adoption manifest and a dedicated archive application ID with full SQLite durability. The source's original tables, identities and custody remain intact. For an absent personal database, create a private archive-only SQLite refusal sentinel with the same plan/profile binding; it has no personal schema, budget, fake identity or fabricated call. Use a no-replace publication mechanism and directory synchronization so an unexpected new file cannot be overwritten. New normal openers check the manifest before create/recovery as well. A crash partway through source fencing leaves destination access inactive and resumes remaining fences; it never restores a fenced source to normal authority.
4. **Imported.** Under the canonical writer, require all exact fence receipts and recheck conflicts. Insert the unchanged original Seat and exact Resource definitions, the verified-provider-to-original-Seat mapping, inactive destination grants and the immutable import receipt in one transaction. No ceiling is summed; a preexisting different mapping is a conflict. Save original source/profile provenance without copying private command payloads into shared tables.
5. **Custody ready.** Open the new journal-only service against the fenced source and verify access to the saved completed admission receipt under the original actor/device. Verify the existing Matrix/configuration journal paths remain usable without opening the personal ledger. This produces no central mutation and no capacity effect.
6. **Active.** In one canonical transaction verify matching plan/fence/import receipts and enable only the explicit destination profile/resource ACL. A lost reply recovers that receipt. The new manager selects canonical resources plus original private custody. The previous NativeOwner/Session can never become live again; the host uses a new guarded instance if ordinary authentication was retired during drain.

Source fencing precedes destination usability. Cancellation before fencing may abandon an inactive plan. After any source fence, cancellation cannot restore its old allocation authority; recovery continues or explicitly leaves it unavailable. Missing source files, failed WAL recovery or a changed account mapping after a crash require reconciliation, never fresh empty initialization.

## Admission receipt eligibility

Validate every row's size, closed operation, actor scope, canonical input digest and terminal state. A replied success/failure may be retained, including an unacknowledged completed admission; acknowledgment is not proof of nonexecution. An acknowledged row with intentionally erased input/reply remains an immutable historical command tombstone, not a source of new execution authority. An unknown operation or pending outcome blocks transfer. Supported native definition-effect receipts must match the exact retained definitions. Supported central receipts retain their original bytes and are later checked against the server before presentation.

The first implementation enumerates the allowlist from the current typed operations: resource enroll/configure/publish and contribution/project request/decision/revoke/verify/resume. Grant publication or any allocation, parent settlement, legacy engagement, personal call, runtime intent or managed-agent history is excluded. It must not accept an operation by treating a string prefix as a category.

## File ownership and implementation order

1. **Native projects agent:** new `hagency-store/src/management/journal_store.rs` and scoped authority extraction; archive identity/manifest reader and isolated old-opener refusal tests. Coordinate changes in `management.rs`/`intents.rs`, `database.rs` and `NativeManagement` before editing. This prerequisite can land without any production migration entry point or source mutation.
2. **Native projects agent after review:** `console/native/resource_adoption{.rs,/...}` plus `domain/owner_resources/adoption.rs`, typed plan/journal and source fence helpers. Reserve the next schema migration only after the managed-agent schema75 freeze; do not edit its migration or capacity/parent modules.
3. **Root:** trusted native confirmation, SDK/host migration gate, service quiescence/new guarded manager and installation resource-authority lease. No generic mini-app file operation. Existing package/permission displays remain exact.
4. **UI agent coordination only:** retain the extracted profile-directory helper and its no-personal-DB tests. Changes to the configuration or Matrix journal path are unnecessary. Mini-app wording/activation waits for the migration fixture flow.

## Acceptance tests before activation

- Populate a synthetic current source with the real onboarding-shaped Seat/resources, immutable native effect receipts and completed central admission/project intents. Transfer and restart; assert exact public IDs/revisions and original receipt bytes. No replacement contribution or new central approval is created.
- Run the actual normal `DomainRepository` and `Ledger::open_scoped` openers, including the absent-personal sentinel case, against fenced fixtures in a separate process. They must reject without schema recovery, fake rows or changed original evidence. Journal-only operations must remain usable and must reject resource SQL/API entry points.
- Kill the transfer between every durable phase, including each source fence, import commit and ACL activation. Recover the same migration/command and assert at most one usable resource authority, one provider mapping, one declaration and no duplicate hold.
- Block a real domain writer, retire it while queued, and verify migration cannot fence/activate until acknowledged shutdown and lock reacquisition. Repeat with a second process holding the original root/domain lock.
- Race new profile enrollment against preparation/activation. It must not create a new independent Seat. Same verified account through two runtimes/models resolves the adopted original Seat; a denied profile cannot read or use it.
- Reject personal unknown/unreported-zero history, runtime intents, pending central outcomes, managed-agent rows, duplicate conflicting Seats, account switches, expired proofs, malformed markers, replaced files and incomplete source coverage. No rejected preparation changes source evidence.
- Recover and acknowledge an old admission card after migration; open the original Matrix creation/configuration journal paths; verify the personal sentinel stays a sentinel. Exercise source/archive journal account and package isolation.

These tests qualify only the explicitly selected and fenced source set. They do not turn an owner assertion into proof that unmanaged copies or external provider consumers do not exist.

## Implemented prerequisite: authority and private archive custody

`management::ManagementAuthority` now carries the existing finite context, exact permissions and shared retirement fence independently of any database. `Session::authority()` preserves shared retirement with the legacy Session; its public API and resource checks remain compatible.

`PrivateManagementJournal::open_existing` opens only an already fenced, exact profile/migration/registry-bound source. It has no fence creator or adoption API. Its bounded worker retains the original domain lock and exposes only original prepare/reply/list/acknowledge custody. An SQLite authorizer denies resource reads/writes, schema changes, attached databases and trigger/view side effects. It reuses the existing closed intent executor and original actor/command/digest rules. Initialization installs its SQL progress deadline before manifest/shape/count/size validation; queued commands also recheck finite authority inside the writer. Missing source files or WAL companions are not created by this prerequisite.

Actual normal-opener tests exposed an error-path side effect: the domain opener formerly installed `NO_CKPT_ON_CLOSE` after schema validation, so a refused foreign/archive file could still be checkpointed on connection drop. It now installs that existing close policy immediately after opening the connection. The regression proves archive main/WAL bytes survive rejected current normal opens and the journal can reopen afterward. Synthetic personal refusal sentinels are rejected by the actual `Ledger::open`/`open_scoped`, including a separate process, without initializing a personal schema.

This does not prove downlevel binaries preserve WAL companions, does not fence already-open writers, and does not make a missing personal file evidence of no past use. Production source fencing still needs the migration coordinator, complete selected inventory, retained runtime/provider drain acknowledgements and physical lock reacquisition. Root's separate management-domain drain acknowledgement alone is not all-runtime quiescence. `NativeManagement` resource routing is not yet switched to the canonical registry.

## Next coordinator checkpoint: retained quiescence and installation authority

The next bounded implementation should produce `QuiescedSourceSet`, a native-only retained proof for the exact selected source set. It must not yet write archive identities, create personal sentinels or activate destination ACLs. This tests the last lifecycle boundary before irreversible transfer with the real current managers instead of assuming `NativeOwner::shutdown()` proves completion.

Proposed files and interfaces:

- `console/native/source_quiescence.rs`: `prepare_source_quiescence(reviewed_selection, migration_gate)` fixes the selected root/profile/physical identities and entire resource inventory. `close_and_drain()` retains one task/outcome across canceled and concurrent callers. Its result owns the original root/domain lock handles and exact component receipts; its fields have no public constructor. The independent migration gate still checks current SDK/package/host epoch even after ordinary source managers are retired.
- `console/owned_runtime/drain.rs`, with narrow wiring in `owned_runtime.rs` and `owned_runtime/service.rs`: `drain_for_migration()` synchronously closes new starts/entry publication, retains entry/service/binding task handles through completion and returns a receipt only after every selected task has joined. A dropped status sender or empty registry is not success. Panicked/aborted tasks, uncertain provider shutdown and unresolved ledger calls remain explicit failure evidence. Existing ordinary `stop_all` may delegate for cancellation safety, but must not convert a failed migration drain into success.
- `console/owner_provider/drain.rs`, with narrow wiring in `owner_provider.rs` and its external-provider process adapter: `drain_for_migration()` closes new launches, retains each process while cancel/kill/wait completes and returns only actual exit acknowledgements. A `start_kill` request or `kill_on_drop` is not an exit receipt. In-flight identity/status children must either be registered in this same lifecycle or keep an activity guard until a retained cleanup task reaps them. Constructor cancellation cannot make an unregistered child invisible.
- `hagency-platform` private retained file-lock helper plus `owner_resources/authority_lease.rs`: one OS-owner `resource-authority.lock`, derived through the same trusted owner-home rule as the canonical registry. `LegacyResourceLease` is a shared lease retained across each actual resource/personal allocation effect, including its queued commit. `AdoptionExclusiveLease` requires all such current-code writers to leave; acquisition is nonblocking and reports Busy if another process still owns one. New resource effects check durable mode after obtaining their lease. Central requests and private-journal-only operations do not acquire a resource lease merely to read/write command custody.

The installation lock is writer coordination, not another capacity ledger. Durable prepared/canonical mode and migration receipts belong in the existing canonical owner database. A shared lease acquired before a mode transition cannot outlive exclusive acquisition, and a lease acquired afterward must observe canonical mode and refuse the old resource path. A canonical process still uses the existing owner-registry writer lock; another process cannot create a fallback ledger when it cannot obtain that writer. Downlevel code that never acquired this lease is not covered and remains subject to explicit selected-root draining/fencing and the stated discovery limitation.

Lock ordering is retained source-root ownership, then installation authority lease, then the relevant database writer/lock. Never wait indefinitely while upgrading a shared lock: retire and drain selected effects, release their shared leases, try exclusive acquisition and recheck every saved source identity/inventory. A concurrent unselected process makes this checkpoint unavailable rather than silently broadening the selected source set. A canceled caller retains the cleanup task and every unacknowledged handle; another caller waits for the same result. Process restart discards in-memory quiescence proof and repeats actual lock/drain checks from the saved plan.

Focused acceptance: pause a real provider child at exit, pause a real runtime task with an open personal ledger and pause the actual domain writer. Cancel the first coordinator caller, start a second and verify it cannot return success before all original acknowledgements and locks are obtained. A constructor racing the close gate must either publish its retained handle before drain enumeration or retire/exit before dropping its activity guard. A second process holding a legacy shared lease must prevent exclusive adoption; a fresh lease after canonical-mode publication must refuse legacy allocation. Failure or lost acknowledgement stays failed, even if registries later appear empty. These proofs qualify only the selected/drained roots and must continue to report `external_roots_complete: false`.

After this checkpoint, the already proposed coordinator can consume `QuiescedSourceSet` to perform the durable fence/import/activate phases. It must never accept a boolean supplied by a mini-app as a replacement for the retained proof.


## Installation lease and native definition-writer checkpoint

`hagency-store::installation_authority` now implements an OS-owner-root shared
`LegacyResourceLease` and finite exclusive `AdoptionExclusiveLease`. Real file
locks exclude cooperating processes; clones retain exclusion through queued
work. Retirement rejects effects immediately but does not release another
queued command's lock. Bounded, private, exact refusal-marker validation denies
legacy access after a recorded transfer; this checkpoint has no marker writer.
Test-only roots keep synthetic tests out of the actual owner registry.

NativeManagement enrollment, configuration and publication now acquire the
shared lease on a blocking worker and retain it inside the original queued
command. Each existing authority check inside the SQLite transaction also checks
the lease; post-commit failure remains outcome-unknown. The lease augments the
original permission, expiry, retirement, expected revision and command receipt.
Resource reads and central/private journal operations do not acquire this lease.
Leases are per operation, so retaining a retired management handle cannot itself
keep an otherwise drained installation locked indefinitely.

Seven file-lock tests pass, including a real second process. Four real SQLite
queue tests exercise all three mutation paths: live effects and receipts, lease
retirement, original-session retirement, and caller cancellation. The existing
worker intentionally skips abandoned work still in the queue; the test proves
its closure retains exclusion until dequeue rather than assuming cancellation
is proof of completed cleanup. All twelve existing management regressions pass,
and strict store library/test Clippy passes. Logs are retained under
`.run/management-resource-lease-*` and `.run/installation-resource-authority-*`.

This is partial writer coverage. Personal runtime, standalone/CLI configuration,
legacy coordinator effects, and initial legacy ledger creation do not yet use
the installation lease. Their coverage and downlevel-writer draining are still
required before the exclusive lease can qualify an adoption. The separate
quiescence proof must continue reporting this blocker. No source manifest,
archive marker, canonical import or destination ACL has been written by these
changes.

## Implemented proof-only quiescence checkpoint

`NativeOwner::prepare_source_quiescence` accepts a native-only request with the distinct `PrepareSourceQuiescence` permission, an existing canonical registry handle and a finite host migration gate. It fixes the current authenticated profile and selected original root; it does not accept source paths or asserted completion from a mini-app. It verifies the SDK source and exact device identity before and after capturing existing source handles, and repeats that verification immediately before intentionally retiring the source. The post-retirement migration gate remains independent of the retired source Session.

`SourceQuiescence::close_and_drain` retains its original task and result across cancelled and concurrent callers. It closes runtime/provider constructor gates, retires source authority, waits admitted native calls and the management mutation lane, then waits configuration task termination, runtime/service joins, provider exit/reap observations and the original domain-worker acknowledgement. Ordinary runtime/service stops now retain entries through their join; provider stops retain entries through kill/wait. The shared task custodian retains original outcomes even when an earlier caller removes or stops observing its handle. A panic, failed reap or cancelled/failed low-level provider constructor remains an unknown result. A background poller deliberately aborted on retirement is acknowledged only as terminated; its provider cleanup is tracked separately.

After these acknowledgements, the checkpoint acquires the real finite `AdoptionExclusiveLease`, validates the original `OwnerHost` root/directory/lock identities and reacquires the existing domain lock. Where a personal database exists, it retains an existing-file SQLite `BEGIN IMMEDIATE` writer lock with no create/recovery/DDL and `NO_CKPT_ON_CLOSE`. Tests preserve its main/WAL bytes. An absent personal database is not created and remains a history blocker. The opaque `QuiescedSources` retains those handles and checks the live migration gate on use; it has no deserializer, fence writer, import method or capacity API.

This is **not adoption activation**. Its status always reports `adoption: not_enabled`, `external_roots_complete: false`, incomplete installation writer coverage and adoption inventory not yet assessed. Current runtime/CLI/personal paths are not all covered by the installation shared lease. It does not certify orphaned processes from a previous host, downlevel writers, unselected roots, no historical personal use, or a destination resource ACL. Any future durable coordinator must re-inventory the locked sources and preserve all original and unknown liabilities; it cannot reinterpret these remaining blockers as success.


The quiescence acceptance run passed 5 source-coordinator tests, 3 task-custody tests and 4 runtime/provider drain tests. These include cancellation while original work remains blocked, a real child exit/reap, actual domain/personal writer exclusion, unchanged personal main/WAL bytes, rejected SDK authority before retirement, root-lock replacement, and an unchanged cached busy failure after the competing writer exits. Existing runtime (61), provider (15) and configuration (9) regressions also passed; previously ignored live-fixture tests were not enabled. Schema 77 audit (12), archive (12) and native census (15) tests passed after the additive managed-fence review. These tests establish local synthetic-fixture behavior, not real-profile adoption or provider identity availability.

## Lifetime legacy-writer custody checkpoint

Normal `DomainRepository` openers and personal `Ledger::open` now acquire a shared installation guard before opening, initializing, migrating or recovering SQLite. The connection retains that guard until after the connection and its original physical ownership lock are dropped. The three finite NativeManagement mutation leases remain additional permission/expiry checks. Snapshot and restore also take the shared guard before filesystem effects; read-only snapshot verification does not. This covers the current normal CLI, coordinator and personal-ledger constructor paths rather than relying on every caller to remember an optional lease.

The canonical registry uses a private, sealed opener constructed only after validation of its fixed OS-owner root (or an explicit isolated test root). No public path or boolean enables the bypass. The shared private-file implementation, including Windows SID/ACL handling, now lives in `hagency-platform`; the store retains error-compatible wrappers. The lock and refusal-marker parser are shared by lifetime and finite guards. There is still no refusal-marker writer in this checkpoint.

Desktop Unix has an implemented legacy guard. Actual adoption runtime qualification is separately limited to macOS. Windows, Android and OpenHarmony retain explicit migration-unavailable startup admission rather than receiving an untested coverage claim. Windows has an OS-token-derived profile directory implementation and passed cross-target platform compilation/Clippy, but no Windows runtime acceptance was available. Linux likewise has no runtime acceptance for adoption in this checkpoint.

The test-support feature never falls back to the real OS-owner root: a fixture explicitly selects one private synthetic installation root before its first opener, and it cannot switch that root afterward. The native host exposes only the test-feature helper `hagency::test_support::initialize_resource_authority()` for Desktop fixtures. Existing fixture directory creation was updated to make this selection explicit, including shared Matrix fixtures.

Acceptance evidence: 6 actual normal-opener/subprocess tests cover domain plus personal exclusion, fresh-source and old-schema refusal, immutable test-root selection, snapshot/restore refusal and the private canonical opener. A separate phase-paused test proves the guard remains held through SQLite close and physical ownership-lock release. Platform lifetime tests (5), finite lease tests (7), personal-ledger tests (86 passed, 3 existing ignored), and a bounded-concurrency complete store run (204 passed) also passed. The actual personal archive-refusal integration (3), resource management integration (12) and private command-custody integration (4) also passed. Strict platform/store/personal all-target Clippy and native library/test Clippy with the explicit test-support feature passed. Logs are `.run/legacy-writer-*` and `.run/managed-matrix-native-store-all.log`.

The broad native library run was **356 passed, 4 failed, 5 ignored**. All four failing cases passed when rerun individually; the original concurrent-run failures remain recorded and are not claimed as pre-existing or resolved by isolation. This checkpoint establishes the targeted lifetime-custody behavior, not a clean unrestricted-concurrency native suite.

No source was imported, no real profile was changed, and no adoption permission or mini-app route was exposed. Downlevel binaries, unknown external roots, missing personal evidence and unknown liabilities still require the durable coordinator's explicit handling; the proof-only quiescence status does not turn their absence into complete inventory.

### Disabled physical adoption checkpoint

`resource-adoption` is an opt-in native/store/platform feature. No mini-app
capability, production registry initializer or existing-profile mutation is enabled.
The selected-source coordinator now retains the actual owner, installation,
domain and personal writer custody. It records the original canonical plan,
verifies the entire one-Seat inventory, saves bounded create-once main/WAL backups,
publishes an irreversible legacy-writer refusal, and writes exact archive manifests.
The personal fence commits under its original `BEGIN IMMEDIATE` lock; a partial
personal/domain fence is revalidated against the original inventory on restart.
Unknown history and unsupported original resource enforcement fields still reject
adoption rather than being silently discarded.

The canonical phases are `prepared`, `sources_fenced`, `imported_inactive`,
`custody_ready`, and `active`. Import retains original resource/Seat IDs and
configuration revisions. ACLs remain inactive until the original private journal
worker validates its command custody and the provider identity is rechecked.
The native handoff requires a live archive worker, excludes simultaneous adoption
handles and retains one drain acknowledgement across cancelled callers. The
original source lock is reacquired after acknowledged journal shutdown.

`NativeOwner::open_for_resource_adoption_with_matrix_guarded` authenticates with
the existing SDK and validates the exact original canonical migration plus physical
source binding. It starts no legacy configuration worker. Missing/wrong migration
IDs fail, and generic owner commands and normal legacy management remain denied.
This recovery opener is **not** the post-adoption Resources integration: an explicit
canonical ACL-backed management session plus original archive journal is the next
required hookup before grant confirmation can be exposed.

Evidence: `.run/resource-adoption-store-phases-final.log` passed 14 store tests;
`.run/resource-adoption-refusal-tests.log` passed 6 lifetime/refusal tests;
`.run/resource-adoption-native-physical-tests-final.log` passed the isolated process
restart scenario. That scenario fences real original sources, restarts before
canonical acknowledgement, rejects a missing migration ID, preserves original
backup bytes, denies old normal domain/personal openers, and activates two original
resources on one original Seat. Both canonical reads observe the same 700-token
account headroom and zero new commitments; another profile is denied. Strict
platform/store all-target feature Clippy passed in
`.run/resource-adoption-store-platform-clippy.log`.

Limits remain explicit: this checkpoint requires a present, proven empty personal
ledger; an absent personal file is not treated as empty history. It does not import
legacy executions or unknown liabilities, discover all external roots, qualify
downlevel non-cooperating writers, or provide Windows/mobile runtime qualification.
The installation refusal blocks other legacy roots too; activation requires an
explicitly reviewed installation transition, not an inference that unselected
profiles are absent. Real user profiles and the user's canonical registry have not
been modified by these tests.

### Adopted resource reads and private command custody

The default-disabled `resource-adoption` host entrypoint
`NativeOwner::open_adopted_management_with_matrix_guarded` now reopens an **active**
original migration using the SDK and original owner-root lock. It requires the
saved canonical plan, exact retained source-file binding, and the permanent
archive schema/manifests. A prepared or merely fenced plan cannot use this entry.
No legacy configuration worker starts.

`NativeManagement` retains an explicit backing: the original legacy worker, or
canonical resource access plus the original bounded private archive worker.
Canonical resource list/detail queries check the authenticated profile ACL before
returning definitions or headroom; private command prepare/reply/recovery/ack uses
the original account/device scope in the archive. Acknowledging an old admission
or saving a new pending command does not invalidate reopening: the immutable
archive identity remains fixed, while its command history legitimately evolves.
Session renewal also rechecks the current SDK source and exact profile, including
before returning a cached command outcome.

This adapter exposes resource observations and existing typed central/journal
operations. Generic legacy owner execution and legacy resource enrollment,
configuration, and publication remain denied before a new mutation intent is
created. Canonical resource editing still reports contribution history unproven.
Profile shutdown drains its original archive acknowledgement and does not shut
other profiles' shared canonical registry.

The isolated native subprocess regression extends the original two-resource,
one-Seat adoption through Resources/detail/pagination; foreign-resource denial;
original admission acknowledgement; a new pending command; another full owner
restart; ACL withdrawal; and cancelled/concurrent archive drain. The shared Seat
retains the reviewed 700-token ceiling and remaining headroom. This is native
fixture evidence, not a deployed desktop flow or consent to mutate the actual
user's registry. Default features, external-root completeness, missing-personal
history restrictions, and platform qualification limits are unchanged.

Checkpoint evidence is saved in `.run/adopted-management-checkpoint.json` with
source hashes. Final focused results: native physical/reopen 2/2; legacy retained
drain 3/3; archive/lease 17/17; management 12/12; private intents 4/4; resource
detail 5/5; SDK invalidation/admission 4/4. Strict all-target store Clippy with
`resource-adoption` and native lib/tests Clippy with
`resource-adoption,managed-runtime` passed. The debug macOS test link reports a
large unwind-table warning; no test failed, and this is not a release performance
measurement.

### Native initialization and grant handoff

`AdoptedManagementSelection` is a native, non-deserializable initialization choice.
Preparation saves an immutable, bounded `adopted-management-initializer-v1.json`
inside the original private owner root before any source fence. It records the
original migration, registry identity, profile and device, with no registry path,
provider credential or delegated authority. Resume and finish require that exact
record; missing or malformed identity cannot be reconstructed from the currently
signed-in device. The active migration's archive worker must drain, and the old
owner must be shut down and dropped, before opening the replacement owner.

Desktop's optional `resource-adoption` feature loads this native descriptor before
selecting a normal initializer. Production reload opens only the existing OS-owner
canonical registry, then reauthenticates through the SDK and verifies the active
plan and original source archive. It does not initialize an empty replacement
registry. Missing/invalid original records never authorize import or a new grant.
The explicit same-process selection survives cancellation while the old owner is
drained; its cleanup handles remain retained until that drain finishes. Legacy
runtime restoration is skipped for the adopted owner.

A fresh device under the same authenticated profile may read canonical Resources.
Original private command history retains its original device scope, and the grant
handoff rejects that replacement device until its authority is re-established.
The saved original-device descriptor is never rewritten to bypass this boundary.
Only after a canonical Resources read succeeds does the host attach a separate
native `OwnerGrantAccess` manager to the existing grant UI. The script's ordinary
manager never receives `ConsentGrants`. Host cache installation rechecks retirement
and the current package lease under the cache locks.

The isolated fixture uses the same explicit test-only installation root in three
separate processes: original definitions/admission → adoption → native grant review
and confirmation → a lost publication reply; restart and original-command recovery;
then replacement-device inventory access without private-command or grant access.
External provider identity and the HTTP server are synthetic fixtures; real native
SDK-source gates, SQLite writers, private journals, source fences, original IDs and
process restart are exercised. This is not evidence of a real provider turn or a
live PostgreSQL deployment. No actual user's registry/profile is changed, and the
feature remains disabled by default. The native coordinator entrypoint is prepared
for a reviewed onboarding UI; no migration button or automatic user migration is
enabled in this checkpoint.

Validation for this handoff: the three-process scenario passes in
`.run/adoption-grant-handoff-tests-final.log` (one parent test, three child phases).
It preserves the two original resource IDs and original admission receipt, creates
exactly one 300-token parent grant under the shared 700-token Seat, recovers the
lost reply with one total publication POST, and leaves 400 tokens of shared
headroom. Invalid, foreign and missing descriptor cases are rejected; a new device
reads inventory but sees none of the original device's private saved commands.
The existing original-source crash/fence/restart regression remains green at
`.run/adoption-handoff-physical-regression.log`. Combined strict native/store
lib/tests Clippy passes with `resource-adoption,managed-runtime` at
`.run/adoption-grant-handoff-clippy-final.log`.

Desktop's nine actual NativeOwner retirement tests pass, including cancellation
while the adopted transition waits for revocation, at
`../hagency-desktop/.run/adoption-grant-desktop-retirement-tests.log`. Feature checks
are recorded under `../hagency-desktop/.run/adoption-grant-desktop-check*.log`.
The optional coordinator entrypoint is not attached to a visible migration action
and is reported as unused by the Desktop compiler; this does not imply that an
on-screen migration flow has been exercised. Debug test linkage still reports the
large unwind-table warning. Neither warning is performance-validation evidence.

### Visible native onboarding checkpoint

The optional Desktop feature now connects Resources → Onboarding to a native
review and confirmation. The mini-app may request only an empty status/review
object. Source paths, provider identity, original migration/device IDs and explicit
ceiling input are never accepted from script. Native review binds the authenticated
owner, original source revisions, stable device and displayed ceiling for 120 seconds;
confirmation rechecks those bindings before retirement and again against the
physically locked source inventory before the plan is saved.

A confirmed operation is retained in Desktop independently of its UI waiter.
The original owner slots remain held until real native shutdown/drain completes;
logout cannot return early because another caller temporarily owns cleanup. An
incomplete descriptor opens a recovery-only owner with SDK-only renewal, preserving
the original plan and ceiling. Active canonical Resources must be read before
original-device grant access is enabled. Same-account replacement devices retain
inventory access but cannot reconstruct private grant custody.

For isolated UI acceptance, the nondefault `onboarding-test-support` feature adds
a startup-only absolute installation-root argument. Every fixture process uses
the same selected authority/canonical root, fixed before any opener. Its optional
empty-personal bootstrap requires an authenticated state physically beneath that
root and atomic create-new; it never opens/migrates existing personal evidence.
There is no production environment authority override. Detailed build/bootstrap
and instrumented-flow instructions are in Desktop's
`docs/hagency-onboarding-isolated-validation.md`.

Final native confirmation tests pass 3/3 in
`.run/resource-activation-native-fixture-final-3.log`: two original resources sharing
one Seat, original prepared-plan restart, and a real authority failure immediately
after durable descriptor creation followed by exact original-plan recovery. Each
scenario continues through native grant confirmation and lost-publication recovery
across separate processes. The original admission receipt, resource IDs and shared
700-token budget are preserved, with one 300-token grant and one publication POST.
Provider identity and central HTTP responses remain deterministic test fixtures;
these results do not claim actual provider or deployed PostgreSQL acceptance.
Expired/cancelled review and a changed actual source revision pass 2/2 separately in
`.run/resource-activation-authority-tests-final.log`.

Desktop's backend 14/14 regressions pass in
`../hagency-desktop/.run/onboarding-backend-tests.log`, including a cancelled UI
waiter, exact cached task outcome, and logout during a real native revoke barrier.
The same executable passes the three grant permission/handoff tests. The native
Makepad panel has separate 4/4 phone/desktop EN/CN geometry/input tests from the UI
lane. Actual instrumented onboarding acceptance remains the next independent gate;
none of the running role fixtures or real user's resources were migrated here.
The feature remains disabled by default, and selected-root evidence still does not
prove complete discovery of external legacy roots.

The final onboarding source/evidence manifest is
`.run/resource-activation-checkpoint.json`. Strict combined native/store lib/tests
Clippy with `resource-adoption,managed-runtime,test-support` passes in
`.run/resource-activation-combined-clippy-final.log`. Desktop feature-on library
and binary check and the normal default-feature library check both pass. Desktop
Clippy reports inherited warning-only diagnostics separately; the new native/store
checkpoint is warning-free under `-D warnings`.
