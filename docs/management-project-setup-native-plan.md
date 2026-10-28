# Native managed Matrix setup adapter

Status: native-only implementation with isolated HTTP/private-journal regressions; no mini-app feature activation or live room operations. The existing native creation service, legacy `matrix-creations.json`, and `im.hagency.client.creation` marker remain unchanged. Server schema 7 is the authority for project approval, allocation readiness, assigned administrator, cancellation, and rebase.

## Entry and authority

The separate `ProjectSetupTransport` over `Arc<NativeManagement>`, following the allocation transport's bounded, device-authenticated pattern. It has no arbitrary URL, bearer, Matrix state type, JSON command body, or caller-supplied identity. Public inputs contain only project/setup identifiers, observed revision preconditions, and explicit actions. The initial creation driver supports server mode `create`; existing legacy adoption remains available through its existing API. Managed adoption can be a later explicit candidate-room workflow, not an implicit fallback.

Native permissions are distinct from read access: `ReadProjectSetup`, `ManageProjectSetup` (begin/confirm/cancel/rebase), `CreateProjectRooms` (Matrix creation/link/invitation effects), and `AssignProjectAdministrators` (server-admin assignment route). None is declared in the shipped mini-app manifest or exposed through its current service list. Every effect checks the current package digest/instance, Matrix account epoch, exact profile, stable installation/device identity, generation, local session, SDK authorization, and permission before and after awaits. Server role checks remain decisive: manager provisions; the designated server administrator assigns administrators; coordinator/resource owner do not acquire project-manager authority.

Use only these closed central operations under `/hagency/api/v1/management`: setup/read by project, administrator assignment, setup begin, setup confirm/cancel/rebase, and setup-command recovery. Validate each typed response and recompute the immutable original definition digest using the existing canonical JSON implementation and schema-7 fixture. Positive numeric revisions are JSON-safe; identifiers, names, response bytes, and collection sizes are bounded. `canConfirm`/`capacityReady` are current observations, not transferable creation capabilities.

## Closed native calls and routes

All central routes below use the existing device bearer and the `/hagency/api/v1/management/` prefix. The transport generates and privately persists mutation command IDs; IDs in server receipts are observations, not caller-controlled creation inputs.

| Native action | Exact central route | Caller data / boundary |
|---|---|---|
| `read(projectId)` | `GET projects/{projectId}/setup` | Read only; cannot initialize creation history |
| `assignAdministrator(input)` | `PUT projects/{projectId}/administrator` | Requested admin MXID plus expected authority revision; only server administrator may select this target |
| `beginCreate(input)` | `POST projects/{projectId}/setup` | Project ID and expected project/allocation/authority revisions; native fixes `mode=create` and generates command ID |
| `continueSetup(setupId, candidates)` | no single central route | Requires original local record; optional candidate space/room IDs are proof candidates only, never replacements for known IDs |
| `cancel(setupId, expectedRevision)` | `POST project-setups/{setupId}/cancel` | Clears readiness, never erases journal/rooms or releases tokens |
| `rebase(input)` | `POST project-setups/{setupId}/rebase` | Setup/allocation IDs and exact expected setup/project/allocation/authority revisions; never accepts a new creation definition |
| internal `confirm` | `POST project-setups/{setupId}/confirm` | Original proven room IDs, latest expected setup revision and private command ID |
| `resumeBegin(projectId)` / `retryCommand(commandId)` | no caller-selected fresh mutation | Requires the same profile's saved exact intent; recover first, then repeat only the same idempotent central command when unrecorded |
| internal `recover` | `POST project-setup-commands/{commandId}/recover` | Exact closed operation from the stored private intent; cannot create missing history |

A bounded read-only `progress(projectId)` also lists this installation's pending central command IDs. It never creates journal records. This makes a lost begin reply recoverable when the caller has not yet learned a setup ID. `beginCreate` refuses a second different begin while an earlier outcome is unknown; terminal rejected attempts may be acknowledged without deleting accepted begin custody or any Matrix stage history.

Responses are closed typed `ProjectReadiness`, `ProjectSetup`, `SetupBinding`, and native `SetupProgress` values. Progress separates original room state, central readiness, current administrator membership, and a bounded blocker code. It must not label an approved plan as allocated or an invited administrator as joined. Local bounds: 64 project records per profile, 2 MiB aggregate journal, 32 central command records and 32 invitation records per project, central responses at the existing 128 KiB limit, Matrix state at the existing 2 MiB limit, and at most 64 recovery room candidates before an explicit candidate ID is required. Capacity exhaustion refuses new effects before sending and never drops unknown history.

## Separate original definition from current binding

The server-derived original definition contains `setupId`, `projectId`, original project/allocation/authority revisions and allocation receipt digest, manager, original administrator, name, mode, definition digest, and the exact space/room creation command IDs. It is immutable. The server's `currentBinding` has a separate monotonic revision and may replace allocation/administrator after rebase. It never rewrites the original definition, command IDs, marker, room IDs, or an already-prepared Matrix request.

The marker is exactly the server contract:

```json
{"version":1,"setupId":"…","projectId":"…","definitionDigest":"…","commandId":"…","owner":"@manager:server","kind":"space|room","spaceId":null}
```

For the room, `spaceId` is the verified original space ID. The event type is `im.hagency.management.project_setup`; the state key is empty. The creation event sender and marker sender must both be the original manager. Recovery rejects duplicated/conflicting markers, wrong room type, public rooms, and a changed parent space.

## Journal and begin receipt custody

Use a new private `managed-project-setups-v1.json` in the existing exact owner-profile directory. Include explicit schema/version and original profile/client/installation/stable-device binding; never store OAuth/device credentials. Keep a bounded record count and serialized size, using the existing private atomic replacement rules and an exclusive workflow lock. Do not migrate or reinterpret the legacy creation journal.

Native code generates the begin command ID and saves the exact begin intent **before** sending it. Callers cannot select a historic begin ID to rebuild missing history. A server result is accepted only for that local intent and the current stable device's exact begin command. A lost begin response may be recovered through the device-scoped receipt, but only while the local pre-begin intent remains. Because no Matrix stage is allowed before the original descriptor and all initial stage records are durably saved, this recovery cannot conceal an earlier Matrix create.

A missing/corrupt journal or missing stage record is `local_setup_history_required`, never `not_issued`. Neither GET, latest readiness, a rebase receipt, nor a successful begin replay can reconstruct absent stage history. A new installation may observe central readiness. This initial native driver does not expose cross-installation confirmation or managed adoption; either needs a separate explicit observational workflow and cannot synthesize creation history. Generation renewal on the same stable installation permits recovery after fresh authorization, without changing historical command identity.

## Bounded state machine

1. **Begin pending:** save generated command ID, exact revision checks and `mode=create`; submit/recover the central command. A conflicting/pre-existing setup without this local intent does not initialize a new creation driver.
2. **Definition saved:** save the verified original definition and both explicit space/room stage records. Each exact typed create payload is durably saved before its sole POST; the room payload fixes the original space after that space is proven. Each stage begins `not_issued`; no later code path can reset a submitted stage to that value.
3. **Space:** persist `submitted` before the sole `POST createRoom`. Save a valid returned ID. Lost response, cancellation, task abort, or account change leaves `submitted/unknown`. Recovery only observes joined-room candidates or an explicit candidate ID and requires one exact original marker; zero/multiple matches stay unresolved.
4. **Room:** only after the original space is proven, prepare/save the original room payload with that space ID. Follow the same one-POST rule. Changing an app version or current binding never regenerates this payload or stage command ID.
5. **Links:** verify manager control and exact original room markers; set the reciprocal `m.space.child`/`m.space.parent` states only when absent. If present, require the exact expected values. Conflicts are surfaced; no blind overwrite. An uncertain PUT is reconciled by reading actual state.
6. **Administrator membership:** keep administrator invitations separate from immutable create payloads. Invite/check the **current binding's** administrator in both original rooms. A joined/invited member needs no new invitation; an unjoined administrator keeps setup pending. Record each invitation attempt by setup/binding/admin/room. An uncertain invitation is observed, not blindly resubmitted; a decline/unknown result requires explicit follow-up rather than an automatic invitation loop. Do not remove the previous administrator or fabricate joins.
7. **Confirm:** journal a native-generated central confirmation command with exact setup revision and original room IDs, then submit/recover it. Central confirmation alone registers the managed project/rooms after real Gateway observations. Never call legacy personal `AdoptProject`/`AdoptRoom`, create an agent, or infer resource execution from successful Matrix creation.

Private creation uses the verified local service MXID from trusted server discovery, the manager's current SDK authorization, private invite-only unencrypted rooms, and the managed marker. The creator remains the manager. The assigned administrator is invited in step 6, so an administrator replacement does not require recreating a room or changing its original request. Server confirmation still requires actual service/admin membership and privileges.

Cancellation/rebase/admin replacement is read from the server before each new effect and checked afterward. Cancellation stops progression and preserves all IDs, unknown outcomes, and capacity holds. A fresh rebase can continue only original **locally proven never-issued** stages; submitted/unknown stages remain recovery-only. Replacing the administrator only reconciles invitations/membership in the existing original rooms. A new binding never means a new project setup or a new room command.

There is no atomic transaction spanning server cancellation and Matrix `createRoom`. An already-in-flight create can complete after cancellation; record that outcome in its original journal, stop further effects, and let central confirmation reject the stale setup revision. Do not claim cancellation can erase or prevent every in-flight Matrix side effect. A stronger instantaneous fence would require an additional server lease/execution protocol and is outside this bounded adapter.

## Implementation and verification gates

Implementation files: a separate `console/native/project_setup` transport/driver and closed routes/wire types; a private managed journal module; private managed Matrix operations. The existing private Matrix authorization scope has a separate managed constructor and synchronous native-retirement gate, leaving the legacy journal/marker and public behavior unchanged. The bounded private-file reader is shared. No raw generic transport becomes public. Keep allocation transport and native ledger ownership separate.

The isolated loopback HTTP/private-journal suite exercises: exact fixture/digest alignment; sole space/room POST; lost begin/create responses across restart; cancellation/account switch after accepted create; same/different installation recovery; journal absence/corruption refusal; simultaneous drivers; no reset of submitted stages; rebase with unchanged original payload; current-admin replacement and real join waiting; reciprocal-link conflict refusal; stale confirm rejection; forbidden personal adoption; no hidden mini-app capability. Retain the existing 13 creation and 7 non-live login/route regressions. No live rooms or production accounts are used for this implementation checkpoint.

Source evidence: client `console/native/matrix_creations.rs`, `console/server_login/matrix_creations/service.rs`, `console/native/allocation_transport.rs`; server `management_setup.rs`, `management_setup_recovery.rs`, `management_setup_recovery_schema.sql`, and `tests/fixtures/management-setup-recovery-v1.json`.

## Implemented checkpoint and remaining activation work

Production entrypoints are in `native/hagency/src/console/native/project_setup.rs`; closed projections, commands, journal and driver are in its `project_setup/` files. `acknowledge_failure(commandId)` removes only a terminal rejected command; accepted begin custody and all Matrix stage history remain. The shared `OwnerHost` retains the OS-level process lock, and a process-wide setup mutex serializes drivers before reading the current journal.

The final journal audit rejects a successful begin whose descriptor or either stage was removed, refuses to derive fresh stages from a recovered setup with already-confirmed room IDs, and pins confirmed server IDs against local room IDs. Private journal rollback to an older unconfirmed snapshot is outside this trust boundary; retain the current private owner state rather than restoring old snapshots as active custody.

This checkpoint is not a real Palpo/Matrix end-to-end activation: the workflow tests use an isolated loopback HTTP server and private temporary journals; separate existing authenticated Matrix transport tests exercise SDK pinning, retirement and legacy compatibility. A joint central allocation/setup plus real Matrix provisioning fixture, reviewed mini-app grants and UI, and deployment feature activation remain separate gates. Cancellation cannot undo an already-in-flight Matrix create. No live rooms, production ledgers or credentials were changed.

Validation at this source checkpoint:

- Setup HTTP/private-journal regressions: 18 passed, `.run/native-managed-setup-tests-final-2.log`.
- Legacy creation plus managed SDK/retirement scope: 15 passed, `.run/native-managed-setup-legacy-regressions-2.log`.
- Existing owner-login routes: 7 passed; 2 pre-existing live-fixture opt-in cases ignored, `.run/native-managed-setup-route-regressions-1.log`.
- Existing retained OS-owner-lock regression: 1 passed, `.run/native-managed-setup-owner-lock-2.log`.
- Combined `cargo clippy -p hagency --lib --tests -- -D warnings`, scoped rustfmt and diff checks passed; `.run/native-managed-setup-clippy-final.log`.
