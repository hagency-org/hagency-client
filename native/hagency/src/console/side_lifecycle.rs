//! Project-side credential install/verify (staged) and side lifecycle
//! (board #14), TS parity with `backend-v2.js` (GET side :9849, PUT credential
//! :9881, POST verify :9918-9947, add project :10359, archive :10375,
//! deactivate/reactivate :10464/:10474, DELETE :10584) and
//! `lib/project-side-store.js:509-560` (staged `setCredential`).
//!
//! The routes carry TS's JSON shape verbatim — `{ok:true, side}` on success
//! (the projection, never the credential value), `{error, code}` on refusal
//! with the TS status words — and add no guard the TS route did not have: the
//! store owns every guarantee. `verify` is the one route that talks to the
//! homeserver: it calls `/_matrix/client/v3/account/whoami` with the stored
//! `as_token` (masquerading `?user_id=` for an appservice side), classifies
//! 401/403 → `rejected`, `M_USER_IN_USE` → `blocked`, everything else →
//! `unreachable` (`lib/matrix-representative.js:58`), promotes a staged
//! credential ONLY after the homeserver proves it accepts it, and records the
//! verdict + representative before answering.
use super::engagements::check_lifecycle;
use super::{Error, body, failed};
use crate::resources::domain;
use hagency_store::DomainStore;
use salvo::prelude::*;
use serde_json::{Value, json};

pub(super) fn router() -> Router {
    Router::with_path("project-sides/{id}")
        .get(get_side)
        .push(Router::with_path("credential").put(put_credential))
        .push(Router::with_path("verify").post(verify))
        .push(Router::with_path("projects").post(add_project))
        .push(Router::with_path("projects/{projectId}/archive").post(archive_project))
        .push(Router::with_path("deactivate").post(deactivate))
        .push(Router::with_path("reactivate").post(reactivate))
        .delete(remove)
}

/// The `{id}` path parameter: a Matrix server name (lowercased by the store).
fn side_id(req: &Request) -> Option<String> {
    req.param::<String>("id").map(|id| id.to_ascii_lowercase())
}

/// A store-side failure mapped to the TS `respondProjectSideError` words:
/// bad_request→400, conflict/side_active→409, not-found→404, else 503.
fn side_error(res: &mut Response, error: hagency_store::Error) {
    let (status, code) = match error {
        hagency_store::Error::Invalid(_) => (StatusCode::BAD_REQUEST, "bad_request"),
        hagency_store::Error::Conflict => (StatusCode::CONFLICT, "conflict"),
        hagency_store::Error::State => (StatusCode::CONFLICT, "side_active"),
        hagency_store::Error::NotFound => (StatusCode::NOT_FOUND, "not_found"),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "side_unavailable"),
    };
    res.status_code(status);
    res.render(Json(json!({"error": code, "code": code})));
}

fn not_found(res: &mut Response) {
    res.status_code(StatusCode::NOT_FOUND);
    res.render(Json(json!({"error": "project side not found"})));
}

#[handler]
async fn get_side(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let Some(id) = side_id(req) else {
        failed(res, Error::Invalid);
        return;
    };
    let Some(store) = domain(depot, res) else {
        return;
    };
    match store.side(id).await {
        Ok(Some(side)) => res.render(Json(json!({"ok": true, "side": side}))),
        Ok(None) => not_found(res),
        Err(error) => side_error(res, error),
    }
}

/// PUT `credential`: the field MUST be present (`{credential:{...}}` to set,
/// `{credential:null}` to withdraw). TS `backend-v2.js:9881-9897`.
#[handler]
async fn put_credential(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !check_lifecycle(depot, res) {
        return;
    }
    let Some(id) = side_id(req) else {
        failed(res, Error::Invalid);
        return;
    };
    let raw = match body(req, 16 * 1024).await {
        Ok(raw) => raw,
        Err(_) => {
            failed(res, Error::Invalid);
            return;
        }
    };
    let value: Value = match serde_json::from_slice(&raw) {
        Ok(value) => value,
        Err(_) => {
            failed(res, Error::Invalid);
            return;
        }
    };
    let Some(object) = value.as_object() else {
        failed(res, Error::Invalid);
        return;
    };
    if !object.contains_key("credential") {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(json!({
            "error": "credential is required: send { credential: {...} } to set one, or { credential: null } to withdraw it",
            "code": "bad_request",
        })));
        return;
    }
    let credential = match object.get("credential") {
        Some(Value::Null) | None => None,
        Some(value) => Some(value.clone()),
    };
    let Some(store) = domain(depot, res) else {
        return;
    };
    // The homeserver's API base URL rides beside the credential on install: it
    // is the address `verify` must reach to call whoami, and the native create
    // route (a fleet Registration) carries no such field. Optional.
    if let Some(url) = object
        .get("apiBaseUrl")
        .or_else(|| object.get("api_base_url"))
        .and_then(Value::as_str)
        && let Err(error) = store
            .set_api_base_url(id.clone(), Some(url.to_string()))
            .await
    {
        side_error(res, error);
        return;
    }
    match store.set_credential(id, credential, false).await {
        Ok(Some(side)) => res.render(Json(json!({"ok": true, "side": side}))),
        Ok(None) => not_found(res),
        Err(error) => side_error(res, error),
    }
}

/// POST `verify`: ask the homeserver whether the credential works, promote a
/// staged credential that proves itself, and record the verdict. TS
/// `backend-v2.js:9918-9947` + `lib/matrix-representative.js`.
#[handler]
async fn verify(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !check_lifecycle(depot, res) {
        return;
    }
    if req.uri().query().is_some() {
        failed(res, Error::Invalid);
        return;
    }
    let Some(id) = side_id(req) else {
        failed(res, Error::Invalid);
        return;
    };
    let Some(store) = domain(depot, res) else {
        return;
    };
    let side = match store.side(id.clone()).await {
        Ok(Some(side)) => side,
        Ok(None) => {
            not_found(res);
            return;
        }
        Err(error) => {
            side_error(res, error);
            return;
        }
    };
    let base_url = match side.api_base_url.as_deref() {
        Some(url) => url.to_string(),
        None => {
            // No API base URL recorded: we do not know where the homeserver
            // answers, which is exactly the verdict TS's `ensureRepresentative`
            // reports as `unverified` with a missing-url detail.
            verdict_and_answer(
                &store,
                &id,
                "unverified",
                Some("no api base url configured"),
                false,
                false,
                res,
            )
            .await;
            return;
        }
    };
    let staged = match store.pending_credential_for(id.clone()).await {
        Ok(staged) => staged,
        Err(error) => {
            side_error(res, error);
            return;
        }
    };
    let live = match store.credential_for(id.clone()).await {
        Ok(live) => live,
        Err(error) => {
            side_error(res, error);
            return;
        }
    };

    let mut promoted = false;
    // A staged credential is tried FIRST; a live one is the fallback so a
    // verify during the issue→install window reports the truth about what is
    // currently working (TS :9928-9953).
    let mut result = None;
    if let Some(staged_cred) = &staged {
        let attempt = whoami(&base_url, &side, staged_cred).await;
        if attempt.state == "accepted" {
            if let Err(error) = store.promote_pending_credential(id.clone()).await {
                side_error(res, error);
                return;
            }
            promoted = true;
            result = Some(attempt);
        }
    }
    if result.is_none() {
        match &live {
            Some(live_cred) => result = Some(whoami(&base_url, &side, live_cred).await),
            None => {
                result = Some(WhoamiOutcome {
                    state: "unverified".into(),
                    detail: Some("no credential configured for this project side".into()),
                    mxid: None,
                });
            }
        }
    }
    let outcome = result.expect("a result is always set above");
    if let Some(mxid) = outcome.mxid.clone() {
        match store.set_representative(id.clone(), mxid).await {
            Ok(_) => {}
            Err(_error) => {
                // A refusal here must not turn a successful verification into a
                // 500: recorded as bad_request against the side (TS :9965-9976).
                res.status_code(StatusCode::BAD_REQUEST);
                let side = store.side(id).await.ok().flatten();
                res.render(Json(json!({
                    "error": "representative mxid refused",
                    "code": "representative_rejected",
                    "side": side,
                })));
                return;
            }
        }
    }
    verdict_and_answer(
        &store,
        &id,
        &outcome.state,
        outcome.detail.as_deref(),
        outcome.mxid.is_some(),
        promoted,
        res,
    )
    .await;
}

async fn verdict_and_answer(
    store: &DomainStore,
    id: &str,
    state: &str,
    detail: Option<&str>,
    _has_mxid: bool,
    promoted: bool,
    res: &mut Response,
) {
    let observed = store
        .observe_access(id.to_string(), state.to_string(), detail.map(String::from))
        .await;
    match observed {
        Ok(_) => match store.side(id.to_string()).await {
            Ok(Some(side)) => res.render(Json(
                json!({"ok": true, "promoted": promoted, "side": side}),
            )),
            Ok(None) => not_found(res),
            Err(error) => side_error(res, error),
        },
        Err(error) => side_error(res, error),
    }
}

/// One whoami outcome, classified (TS `ensureRepresentative` + `classifyMatrixFailure`).
pub(in crate::console) struct WhoamiOutcome {
    pub(in crate::console) state: String,
    detail: Option<String>,
    pub(in crate::console) mxid: Option<String>,
}

/// Ask the homeserver who the credential belongs to. Appservice sides
/// masquerade with `?user_id=` and the `as_token`; registration-token sides
/// use the representative's own token. `base_url` is the side's API base URL.
pub(in crate::console) async fn whoami(
    base_url: &str,
    side: &hagency_store::SideRecord,
    credential: &hagency_store::Credential,
) -> WhoamiOutcome {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(20))
        .build();
    let Ok(client) = client else {
        return WhoamiOutcome {
            state: "unreachable".into(),
            detail: Some("could not build an http client".into()),
            mxid: None,
        };
    };
    let mut url = format!("{base_url}/_matrix/client/v3/account/whoami");
    let token;
    if credential.kind == "appservice" {
        let Some(as_token) = credential.as_token.as_deref() else {
            return WhoamiOutcome {
                state: "unverified".into(),
                detail: Some("appservice credential has no asToken".into()),
                mxid: None,
            };
        };
        let as_user = side
            .representative
            .as_ref()
            .map(|r| r.mxid.clone())
            .unwrap_or_else(|| {
                format!(
                    "@{}:{}",
                    credential.sender_localpart.as_deref().unwrap_or("hagency"),
                    side.server_name
                )
            });
        url = format!("{url}?user_id={}", urlencode(&as_user));
        token = as_token;
    } else {
        // registrationToken: no masquerade; the representative's own token.
        match credential.representative_token.as_deref() {
            Some(tok) => token = tok,
            None => {
                return WhoamiOutcome {
                    state: "unverified".into(),
                    detail: Some(
                        "no representative token and no registration token to obtain one with"
                            .into(),
                    ),
                    mxid: None,
                };
            }
        }
    }
    let response = client.get(&url).bearer_auth(token).send().await;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            return WhoamiOutcome {
                state: "unreachable".into(),
                detail: Some(error.to_string()),
                mxid: None,
            };
        }
    };
    let status = response.status();
    let body: Value = match response.bytes().await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    if !status.is_success() {
        return WhoamiOutcome {
            state: classify(status.as_u16(), body.get("errcode").and_then(Value::as_str)),
            detail: Some(
                body.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("whoami failed")
                    .to_string(),
            ),
            mxid: None,
        };
    }
    match body.get("user_id").and_then(Value::as_str) {
        Some(user_id) if !user_id.trim().is_empty() => WhoamiOutcome {
            state: "accepted".into(),
            detail: None,
            mxid: Some(user_id.trim().to_string()),
        },
        _ => WhoamiOutcome {
            state: "unreachable".into(),
            detail: Some("whoami did not return a user_id".into()),
            mxid: None,
        },
    }
}

/// TS `classifyMatrixFailure`: 401/403 → rejected, M_USER_IN_USE → blocked,
/// anything else → unreachable.
fn classify(status: u16, errcode: Option<&str>) -> String {
    if status == 401 || status == 403 {
        "rejected".into()
    } else if errcode == Some("M_USER_IN_USE") {
        "blocked".into()
    } else {
        "unreachable".into()
    }
}

fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' | b'@' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// POST `projects`: add or update a project under a side (TS :10359).
#[handler]
async fn add_project(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !check_lifecycle(depot, res) {
        return;
    }
    let Some(id) = side_id(req) else {
        failed(res, Error::Invalid);
        return;
    };
    let raw = match body(req, 4 * 1024).await {
        Ok(raw) => raw,
        Err(_) => {
            failed(res, Error::Invalid);
            return;
        }
    };
    let input: Value = match serde_json::from_slice(&raw) {
        Ok(input) => input,
        Err(_) => {
            failed(res, Error::Invalid);
            return;
        }
    };
    let Some(store) = domain(depot, res) else {
        return;
    };
    match store.upsert_project(id.clone(), input).await {
        Ok(Some(project)) => match store.side(id).await {
            Ok(Some(side)) => {
                res.render(Json(json!({"ok": true, "project": project, "side": side})))
            }
            Ok(None) => not_found(res),
            Err(error) => side_error(res, error),
        },
        Ok(None) => not_found(res),
        Err(error) => side_error(res, error),
    }
}

/// POST `projects/{projectId}/archive`: archive or restore a project. No
/// delete (TS :10375).
#[handler]
async fn archive_project(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !check_lifecycle(depot, res) {
        return;
    }
    let Some(id) = side_id(req) else {
        failed(res, Error::Invalid);
        return;
    };
    let Some(project_id) = req.param::<String>("projectId") else {
        failed(res, Error::Invalid);
        return;
    };
    // Body is optional; `archived` defaults true (TS :10378).
    let archived = match body(req, 256).await {
        Ok(raw) if raw.is_empty() => true,
        Ok(raw) => match serde_json::from_slice::<Value>(&raw) {
            Ok(value) => value
                .get("archived")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            Err(_) => {
                failed(res, Error::Invalid);
                return;
            }
        },
        Err(_) => {
            failed(res, Error::Invalid);
            return;
        }
    };
    let Some(store) = domain(depot, res) else {
        return;
    };
    match store.set_project_archived(id, project_id, archived).await {
        Ok(Some(project)) => res.render(Json(json!({"ok": true, "project": project}))),
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(
                json!({"error": "project not found on this project side"}),
            ));
        }
        Err(error) => side_error(res, error),
    }
}

#[handler]
async fn deactivate(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !check_lifecycle(depot, res) {
        return;
    }
    let Some(id) = side_id(req) else {
        failed(res, Error::Invalid);
        return;
    };
    let Some(store) = domain(depot, res) else {
        return;
    };
    match store.deactivate_side(id).await {
        Ok(Some(side)) => res.render(Json(json!({"ok": true, "side": side}))),
        Ok(None) => not_found(res),
        Err(error) => side_error(res, error),
    }
}

#[handler]
async fn reactivate(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !check_lifecycle(depot, res) {
        return;
    }
    let Some(id) = side_id(req) else {
        failed(res, Error::Invalid);
        return;
    };
    let Some(store) = domain(depot, res) else {
        return;
    };
    match store.reactivate_side(id).await {
        Ok(Some(side)) => res.render(Json(json!({"ok": true, "side": side}))),
        Ok(None) => not_found(res),
        Err(error) => side_error(res, error),
    }
}

/// DELETE: an active side is refused (409 `side_active`) unless deactivated or
/// `?force=true` (TS :10584).
#[handler]
async fn remove(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !check_lifecycle(depot, res) {
        return;
    }
    let Some(id) = side_id(req) else {
        failed(res, Error::Invalid);
        return;
    };
    let force = req.query::<String>("force").as_deref() == Some("true");
    let Some(store) = domain(depot, res) else {
        return;
    };
    let side = store.side(id.clone()).await;
    match side {
        Ok(None) => {
            not_found(res);
            return;
        }
        Err(error) => {
            side_error(res, error);
            return;
        }
        _ => {}
    }
    match store.remove_side(id.clone(), force).await {
        Ok(()) => {
            let side = store.side(id).await.ok().flatten();
            res.render(Json(json!({
                "ok": true,
                "side": side,
                "cascade": "performed",
                "cascadeNote": "side deactivated and forgotten — all records kept, since ended and inactive are history",
            })));
        }
        Err(error) => side_error(res, error),
    }
}
