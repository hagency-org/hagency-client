# Managed-agent Matrix membership observation checkpoint

This is paired with Hagency Server management schema 10. Native owner-ledger
schema stays 77. It is not activated by Desktop or any mini-app manifest, and
server `enable_agent_matrix_provisioning` defaults to false. Schema 9's reserve
and fence command bytes and all unrelated personal agent flows remain unchanged.

## Native boundary

`ManagedAgentMatrixTransport` polls a closed `agent.matrix.check` challenge via
`POST /hagency/api/v1/management/agent-matrix/poll`. It uses existing SDK-backed
profile/device authority and native-only `ExecuteManagedAgents` permission.
It recovers the exact original local project and agent reservation, verifies
current admitted enrollment, and passes a non-deserializable
`VerifiedManagedMatrixCheck` to the already-owned canonical registry.

The registry performs a deferred SELECT snapshot. It verifies the original
successful reserve receipt and command, active canonical parent/project/child,
resource revision and capability, profile ACL, current period/delegation and
same executor generation. If runtime assignment custody already has a pin, the
assignment ID, generation, canonical binding and provider digest must match;
this check cannot create that pin. Main and WAL bytes remain unchanged.

The observation freezes the central challenge digest, original reserve command
and receipt digests, child and assignment IDs, eligibility and the challenge's
original expiry. `observedLedgerRevision` is a read snapshot, not a new mutation
receipt. It grants no provider, tool, prompt, call or usage authority and does
not invoke schema 76's runtime `claim_managed_provision`/qualification path.

Result upload and acknowledgement use the finite delivery ticket with live
SDK/profile/device checks before and after I/O. A lost acknowledgement recovers
the first central observation with `POST agent-matrix/{id}/recover`; historical
responses cannot rerun a native check or renew validity. Changed generation
cannot execute an old challenge. Closed DTOs reject unknown runtime fields.

## Central boundary

The server repeats current assigned-project-admin, manager, setup, reservation,
resource and enrolled-device checks before accepting the observation. Only then
may the existing appservice identity join the exact previously verified project
room. Every network call has authority checks on both sides. Revoke retains the
original puppet and room for repeated cleanup, including a late join arriving
after an earlier observed absence.

The shared identity remains suspended without generic personal bindings or work
assignment. `GET management/agent-requests/{requestId}/matrix` reports Matrix
membership separately, always `runtimeState: "unsupported"` and
`workAvailable: false`. A Matrix-ready observation never means runtime-ready.

## Canonical fixture

Native: `native/hagency-store/src/domain/owner_resources/capacity/managed_agents/management-agent-matrix-v1.json`.
Server: `crates/agent-service/tests/fixtures/management-agent-matrix-v1.json`.
The files are byte-identical, contain synthetic public identifiers only, and
both repositories validate closed deserialization and canonical SHA-256:

- Challenge: `8aea9c392062419a638494b16895f21e2a9192c52baa581859a2f44915237544`.
- Observation: `5d23c49b414cef60c991216fb705b3ec7fc4aa71adf0bfaa33b8732c914ca795`.

## Activation dependencies

Trusted owner enrollment, legacy resource reconciliation and grant publication
must already be complete for the real canonical resource. This checkpoint does
not invent enrollment/provider/adoption evidence or make old profile ledgers
safe to share. No Desktop dispatcher or capability activation is added.

Actual provider provisioning, fresh execution custody, durable authoritative
usage, work/reply delivery, stop/revoke integration and their complete end-to-end
acceptance are a later checkpoint. Agent creation business approval stays with
the administrator assigned to the project; resource coordinators get no second
human approval over agents.

## Validation checkpoint (2026-10-16)

- `cargo test -p hagency-store --lib -- --test-threads=2`: **204 passed, 0 failed**, 103.06 seconds; `.run/managed-matrix-native-store-all.log`. Includes five new Matrix tests and all original owner-ledger/custody/shutdown regressions.
- `cargo test -p hagency --lib native_managed_matrix`: **4 passed**, real loopback HTTP plus real canonical SQLite; `.run/managed-matrix-native-http-tests.log`.
- `cargo clippy -p hagency --lib --tests -- -D warnings`: passed; `.run/managed-matrix-native-clippy.log`.
- `cargo clippy -p hagency-store --all-targets -- -D warnings`: passed; `.run/managed-matrix-native-store-clippy.log`.
- Paired server full real PostgreSQL/HTTP suite: **176 passed, 0 failed**, 164.98 seconds; server `.run/managed-matrix-postgres-final.log`. Agent-service strict all-target Clippy and locked/offline backend build passed.

Native source hashes: `.run/managed-matrix-native-source-hashes.json`.
Frozen server artifact: server `.run/management-schema10-build/hagency-server`,
SHA-256 `17e3b3f81d67d331bddfd5eb264fac5e8cd7284f71d8413a4059059e1c42011f`.
No running backend, real Matrix homeserver membership or provider was changed
by these tests. Loopback Matrix HTTP is protocol/controller acceptance, not a
claim that a deployed real shared agent has joined or can execute.
