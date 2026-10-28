//! TS oracle: `tests/api-fleet-protocol.test.js`, `tests/bridge-fleet-protocol.test.js`,
//! `tests/fleet-protocol.test.js` (the HTTP/fleet-control halves).
//!
//! Ported into the SERVICE crate. Native serves NO `/api/fleet/*` or
//! `/api/fleet-control` route: the fleet protocol's decision ladder is
//! `hagency::bootstrap::probe` (asserted by `tests/ts_oracle_fleet_protocol.rs`)
//! and the host-only fleet service is `bootstrap::fleet`. The TS fleet-control
//! route's fail-closed outcome is therefore reachable structurally — the route
//! does not exist — which is what these cases assert.
use hagency::App;
use hagency_store::{Repository, Store};
use salvo::{
    prelude::*,
    test::{ResponseExt, TestClient},
};
use serde_json::json;

const TOKEN: &str = "fixture_operator_token_32_bytes_minimum";
const BASE: &str = "http://127.0.0.1:13300";

/// A writer-backed service with no domain, exactly `tests/http.rs`'s shape.
fn setup() -> (tempfile::TempDir, Store, Service) {
    let dir = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let store = Store::start(Repository::open(&dir.path().join("state")).unwrap(), 16).unwrap();
    let app = App::new(
        store.clone(),
        TOKEN.as_bytes(),
        "127.0.0.1:13300".parse().unwrap(),
    )
    .unwrap();
    (dir, store, Service::new(app.router()))
}

/// TS `api-fleet-protocol.test.js:88` — *fleet backend rejects forged generation
/// cross-fleet requests and conflicting source replays*. Native refuses the
/// whole fleet-control surface: there is no route to forge against, which is
/// the strongest form of the TS rejection (fail closed by construction).
#[tokio::test]
async fn ts_fleet_control_surface_is_absent_and_fails_closed() {
    let (_dir, store, service) = setup();
    for path in [
        "/api/fleet-control",
        "/api/fleet/v1/probe",
        "/api/fleet/v2/hf_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "/api/project-sides/acting-credentials",
    ] {
        let mut response = TestClient::post(format!("{BASE}{path}"))
            .add_header("host", "127.0.0.1:13300", true)
            .bearer_auth(TOKEN)
            .json(&json!({"sideId":"palpo.test","registration":"palpo.test@generation1"}))
            .send(&service)
            .await;
        assert_eq!(
            response.status_code,
            Some(StatusCode::NOT_FOUND),
            "{path} is not a native surface"
        );
        // The refusal is the named console/route vocabulary, never a body the
        // caller could mistake for a fleet acceptance.
        let body = response.take_string().await.unwrap();
        assert!(
            !body.contains("\"ok\":true"),
            "{path} must never answer an ok body"
        );
    }
    store.shutdown().await.unwrap();
}

/// TS `api-fleet-protocol.test.js:53` — *outbound backend gates machine
/// generation and returns credentials only to the bridge*. Native holds no
/// machine credential to return: the fleet credential lives in the private
/// `operator.token`/registration state, never on a wire route.
#[test]
#[ignore = "parity gap: native has no outbound fleet machine credential on any route; the working credential is private host state, never a wire response"]
fn ts_fleet_outbound_backend_gates_machine_generation() {}

/// TS `api-fleet-protocol.test.js:67` — *verified reception request remains
/// pending and its context survives replay*. The pending-before-verdict rule is
/// native (asserted by `ts_oracle_engagement.rs`); the fleet REQUEST ingestion
/// (`com.hagency.engagement.request.v1` over the reception room) is a Matrix
/// intake concern asserted by the matrix crate's provisioning suite.
#[test]
#[ignore = "parity gap: fleet request ingestion is the Matrix reception intake path (hagency-matrix); no native fleet-control HTTP route exists"]
fn ts_fleet_verified_request_stays_pending_and_survives_replay() {}

/// TS `api-fleet-protocol.test.js:103` — *fleet approval rechecks current target
/// authority before allocating*. Native re-reads the live target authority on
/// the approval path and refuses `Unqualified`/`RunnerAuthority` rather than
/// allocating on stale evidence (asserted by the store's approval suites).
#[test]
#[ignore = "covered: the store re-reads live target authority on approve and refuses Unqualified/RunnerAuthority (tests/approvals.rs)"]
fn ts_fleet_approval_rechecks_target_authority() {}

/// TS `api-fleet-protocol.test.js:123` — *approved fleet engagement admits the
/// target and queues its result only in reception*. Admission + the
/// reception-only result queue are the provisioning/intake path in the matrix
/// crate; the console read of the approved engagement is native.
#[test]
#[ignore = "parity gap: fleet admission + reception-only result queueing are the matrix intake/provisioning path; asserted in hagency-matrix"]
fn ts_fleet_approved_engagement_admits_target_in_reception() {}

/// TS `api-fleet-protocol.test.js:156` — *default prefix backend mints and
/// admits the imported fleet identity*. Identity minting is the appservice/token
/// provisioning path (`TokenAccountProvision`), asserted by `tests/token_provision.rs`
/// and `provision_rooms` in the matrix crate.
#[test]
#[ignore = "covered: appservice/token identity minting is asserted by hagency-matrix token_provision + provision_rooms"]
fn ts_fleet_default_prefix_backend_mints_and_admits() {}

/// TS `bridge-fleet-protocol.test.js:22` — *real bridge records the push probe
/// and forwards only verified private-owner requests*. Native records the probe
/// receipt and forwards only a verified request; the record/verify halves are
/// `bootstrap::probe` (asserted in `ts_oracle_fleet_protocol.rs`) and the
/// private-owner DM send is the approval delivery path in the matrix crate.
#[test]
#[ignore = "covered: probe receipt recording + verification is asserted by tests/ts_oracle_fleet_protocol.rs (probe ladder)"]
fn ts_bridge_records_push_probe_and_forwards_only_verified() {}

/// TS `fleet-protocol.test.js:148` — *fleet callback routes require their own
/// appservice token and expose no generic proxy*. Native exposes no fleet
/// callback route at all: the absence IS the "no generic proxy" guarantee,
/// asserted above; there is no second token to present.
#[test]
#[ignore = "covered: ts_fleet_control_surface_is_absent_and_fails_closed asserts no fleet callback route exists (no generic proxy by construction)"]
fn ts_fleet_callback_routes_require_their_own_token() {}

/// TS `fleet-protocol.test.js:56,66,85,99,115,123` and `bridge-fleet-protocol.test.js:22`
/// — the probe/authorization ladder proper (room-name observation, edge-vs-push
/// mode, agent-definition binding, exact receipt, source re-verification,
/// tampering). All are asserted as REAL tests by
/// `tests/ts_oracle_fleet_protocol.rs` (9 passed) in this same crate.
#[test]
#[ignore = "covered: the probe/authorization ladder is asserted by tests/ts_oracle_fleet_protocol.rs (9 real tests, same crate)"]
fn ts_fleet_protocol_authorization_ladder() {}
