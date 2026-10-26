//! Closed owner Project/Room management routes. Matrix and Hagency credentials
//! remain in the trusted Rust host; this is not an arbitrary URL proxy.
use super::{
    body, console, cookie, current,
    server_login::{OwnerError, OwnerOperation},
};
use salvo::{http::Method, prelude::*};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn router() -> Router {
    Router::with_path("owner-projects")
        .goal(dispatch)
        .push(Router::with_path("{**path}").goal(dispatch))
}
fn error(status: u16, code: &str) -> OwnerError {
    OwnerError {
        status,
        code: code.into(),
    }
}
fn key(value: &str) -> Result<(), OwnerError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        Err(error(400, "invalid_arguments"))
    } else {
        Ok(())
    }
}
fn room(value: &str) -> Result<String, OwnerError> {
    let decoded = percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .map_err(|_| error(400, "invalid_arguments"))?
        .into_owned();
    if !super::matrix_identifiers::room_id(&decoded)
        || decoded.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\' | '?' | '#' | '%')
        })
    {
        return Err(error(400, "invalid_arguments"));
    }
    Ok(decoded)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdoptProject {
    space_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdoptRoom {
    room_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PolicyUpdate<T> {
    expected_revision: i64,
    policy: T,
}
async fn empty(req: &mut Request) -> Result<(), OwnerError> {
    if body(req, 1)
        .await
        .map_err(|_| error(400, "invalid_arguments"))?
        .is_empty()
    {
        Ok(())
    } else {
        Err(error(400, "invalid_arguments"))
    }
}
async fn parse<T: serde::de::DeserializeOwned>(req: &mut Request) -> Result<T, OwnerError> {
    serde_json::from_slice(
        &body(req, 16384)
            .await
            .map_err(|_| error(400, "invalid_arguments"))?,
    )
    .map_err(|_| error(400, "invalid_arguments"))
}
// A 2 MiB attachment expands to ~2.8 MiB in the closed JSON command.
// Keep the original budget for all non-upload operations.
fn board_command(bytes: &[u8]) -> Result<hagency_core::project_board::BoardAction, OwnerError> {
    use hagency_core::project_board::BoardAction;
    if bytes.len() > 3 * 1024 * 1024 {
        return Err(error(400, "invalid_board_attachment"));
    }
    let action: BoardAction =
        serde_json::from_slice(bytes).map_err(|_| error(400, "invalid_board_payload"))?;
    if bytes.len() > 131072 && !matches!(action, BoardAction::UploadAttachment { .. }) {
        return Err(error(400, "invalid_board_payload"));
    }
    Ok(action)
}
async fn call(req: &mut Request, depot: &Depot) -> Result<Value, OwnerError> {
    current(req, depot).map_err(|_| error(401, "sign_in_required"))?;
    let raw = req
        .uri()
        .path()
        .strip_prefix("/console/api/owner-projects")
        .ok_or_else(|| error(404, "unknown_owner_operation"))?
        .to_owned();
    let parts = raw
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if raw.contains("//") || (raw.ends_with('/') && !raw.is_empty()) {
        return Err(error(400, "invalid_arguments"));
    }
    if req.method() == Method::GET && parts.as_slice() == ["space-candidates"] {
        let cursor = candidate_cursor(req.uri().query())?;
        let login = &console(depot)
            .map_err(|_| error(503, "local_state_unavailable"))?
            .0
            .server_login;
        let value = login
            .space_candidates(
                cookie(req).map_err(|_| error(401, "sign_in_required"))?,
                cursor,
            )
            .await?;
        current(req, depot).map_err(|_| error(401, "sign_in_required"))?;
        return Ok(value);
    }
    if req.uri().query().is_some() {
        return Err(error(400, "invalid_arguments"));
    }
    let operation = match (req.method().clone(), parts.as_slice()) {
        (Method::POST, [project, "board-command"]) => {
            key(project)?;
            let bytes = body(req, 3 * 1024 * 1024)
                .await
                .map_err(|_| error(400, "invalid_board_payload"))?;
            let action = board_command(&bytes)?;
            OwnerOperation::ProjectBoard {
                project: (*project).into(),
                action,
            }
        }
        (Method::GET, []) => OwnerOperation::Projects,
        (Method::GET, [project, "service-state"]) => {
            key(project)?;
            OwnerOperation::ScopeState {
                project: project.to_string(),
                room: None,
            }
        }
        (Method::GET, [project, "rooms", encoded, "service-state"]) => {
            key(project)?;
            OwnerOperation::ScopeState {
                project: project.to_string(),
                room: Some(room(encoded)?),
            }
        }
        (Method::POST, [project, action @ ("pause-service" | "clear-service-pause")]) => {
            key(project)?;
            empty(req).await?;
            OwnerOperation::ScopePause {
                project: project.to_string(),
                room: None,
                paused: *action == "pause-service",
            }
        }
        (
            Method::POST,
            [
                project,
                "rooms",
                encoded,
                action @ ("pause-service" | "clear-service-pause"),
            ],
        ) => {
            key(project)?;
            let room = room(encoded)?;
            empty(req).await?;
            OwnerOperation::ScopePause {
                project: project.to_string(),
                room: Some(room),
                paused: *action == "pause-service",
            }
        }
        (Method::PUT, [project, "creation-policy"]) => {
            key(project)?;
            let input: PolicyUpdate<super::native::ProjectCreationPolicy> = parse(req).await?;
            OwnerOperation::ProjectPolicy {
                project: project.to_string(),
                expected: input.expected_revision,
                policy: input.policy,
            }
        }
        (Method::PUT, [project, "rooms", encoded, "creation-policy"]) => {
            key(project)?;
            let room = room(encoded)?;
            let input: PolicyUpdate<super::native::RoomCreationPolicy> = parse(req).await?;
            OwnerOperation::RoomPolicy {
                project: project.to_string(),
                room,
                expected: input.expected_revision,
                policy: input.policy,
            }
        }

        (Method::POST, []) => {
            let input: AdoptProject = parse(req).await?;
            let space = room(&input.space_id)?;
            OwnerOperation::AdoptProject { space }
        }
        (Method::GET, [project, "rooms"]) => {
            key(project)?;
            OwnerOperation::ProjectRooms {
                project: project.to_string(),
            }
        }
        (Method::POST, [project, "rooms"]) => {
            key(project)?;
            let input: AdoptRoom = parse(req).await?;
            let room = room(&input.room_id)?;
            OwnerOperation::AdoptRoom {
                project: project.to_string(),
                room,
            }
        }
        (Method::GET, [project, "rooms", encoded, "agents"]) => {
            key(project)?;
            OwnerOperation::RoomRoster {
                project: project.to_string(),
                room: room(encoded)?,
            }
        }
        _ => return Err(error(404, "unknown_owner_operation")),
    };
    let reply = console(depot)
        .map_err(|_| error(503, "local_state_unavailable"))?
        .owner_api(
            cookie(req).map_err(|_| error(401, "sign_in_required"))?,
            operation,
        )
        .await?;
    current(req, depot).map_err(|_| error(401, "sign_in_required"))?;
    Ok(reply.value)
}
#[handler]
async fn dispatch(req: &mut Request, depot: &Depot, res: &mut Response) {
    res.add_header("cache-control", "no-store", true).unwrap();
    match call(req, depot).await {
        Ok(value) => res.render(Json(value)),
        Err(e) => {
            res.status_code(
                StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(json!({"code":e.code})));
        }
    }
}

fn candidate_cursor(query: Option<&str>) -> Result<Option<String>, OwnerError> {
    let Some(query) = query else {
        return Ok(None);
    };
    if query.len() > 1024 {
        return Err(error(400, "invalid_arguments"));
    }
    let parsed = reqwest::Url::parse(&format!("http://local.invalid/?{query}"))
        .map_err(|_| error(400, "invalid_arguments"))?;
    if parsed.fragment().is_some() {
        return Err(error(400, "invalid_arguments"));
    }
    let items = parsed.query_pairs().collect::<Vec<_>>();
    if items.len() != 1 || items[0].0 != "cursor" {
        return Err(error(400, "invalid_arguments"));
    }
    if items[0].1.contains('%') {
        return Err(error(400, "invalid_arguments"));
    }
    Ok(Some(room(&items[0].1)?))
}
#[cfg(test)]
mod candidate_query_tests {
    use super::*;
    #[test]
    fn screenshot_upload_can_cross_native_owner_without_expanding_other_commands() {
        use hagency_core::project_board::{AttachmentUpload, BoardAction};
        let upload = BoardAction::UploadAttachment {
            task: "tsk_test".into(),
            input: AttachmentUpload {
                idempotency_key: "screenshot_test".into(),
                filename: "screenshot.png".into(),
                content_type: "image/png".into(),
                data: "A".repeat(2_796_204),
            },
        };
        let bytes = serde_json::to_vec(&upload).unwrap();
        let parsed = board_command(&bytes).unwrap();
        assert!(super::super::project_board::request("prj_test", parsed).is_ok());
        let mut other = serde_json::to_vec(&BoardAction::Detail {
            task: "tsk_test".into(),
        })
        .unwrap();
        other.resize(131073, b' ');
        assert_eq!(
            board_command(&other).unwrap_err().code,
            "invalid_board_payload"
        );
        assert!(board_command(&vec![b' '; 3 * 1024 * 1024 + 1]).is_err());
    }
    #[test]
    fn scope_policy_payload_cannot_claim_owner_or_administration() {
        let valid = serde_json::json!({"expectedRevision":3,"policy":{"mode":"inherit_project","deny":["@blocked:test"]}});
        let parsed: PolicyUpdate<super::super::native::RoomCreationPolicy> =
            serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(parsed.expected_revision, 3);
        let mut claimed = valid;
        claimed["adminMxid"] = serde_json::json!("@admin:test");
        assert!(
            serde_json::from_value::<PolicyUpdate<super::super::native::RoomCreationPolicy>>(
                claimed
            )
            .is_err()
        );
        assert!(serde_json::from_value::<PolicyUpdate<super::super::native::ProjectCreationPolicy>>(serde_json::json!({"expectedRevision":1,"policy":{"defaultAllow":true,"allow":[],"deny":[],"resourcesApproved":true}})).is_err());
    }
    #[test]
    fn candidate_cursor_is_one_room_id_not_an_arbitrary_proxy_query() {
        assert!(candidate_cursor(None).unwrap().is_none());
        assert_eq!(
            candidate_cursor(Some("cursor=%21space%3Aexample.test"))
                .unwrap()
                .as_deref(),
            Some("!space:example.test")
        );
        for query in [
            "",
            "cursor=",
            "cursor=%2521space%253Atest",
            "cursor=%21s%3At&cursor=%21b%3At",
            "url=https://evil.test",
            "cursor=%21s%2Funsafe%3At",
        ] {
            assert!(candidate_cursor(Some(query)).is_err(), "{query}");
        }
    }
}
