use super::*;
const MARKER: &str = "im.hagency.management.project_setup";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console) struct Create {
    pub setup: ProjectSetup,
    pub space: bool,
    pub parent: Option<String>,
    pub service: String,
}
impl Create {
    pub(super) fn validate(&self) -> Result<(), NativeError> {
        self.setup.validate()?;
        mxid(&self.service)?;
        if self.setup.mode != "create" || self.space == self.parent.is_some() {
            return Err(invalid());
        }
        if let Some(p) = &self.parent {
            room_id(p)?;
        }
        Ok(())
    }
    pub(super) fn marker(&self) -> Value {
        json!({"version":1,"setupId":self.setup.id,"projectId":self.setup.project_id,"definitionDigest":self.setup.definition_digest,"commandId":if self.space{&self.setup.space_creation_command_id}else{&self.setup.room_creation_command_id},"owner":self.setup.manager_mxid,"kind":if self.space{"space"}else{"room"},"spaceId":self.parent})
    }
    pub(super) fn body(&self) -> Value {
        let mut v = json!({"name":self.setup.name,"visibility":"private","preset":"private_chat","invite":[self.service],"initial_state":[{"type":"m.room.join_rules","state_key":"","content":{"join_rule":"invite"}},{"type":MARKER,"state_key":"","content":self.marker()}]});
        if self.space {
            v["creation_content"] = json!({"type":"m.space"});
        }
        v
    }
}
pub(in crate::console) enum Operation {
    Discovery,
    Joined,
    State(String),
    Create(Box<Create>),
    Link {
        space: String,
        room: String,
        parent: bool,
        server: String,
    },
    Invite {
        room: String,
        user: String,
    },
}
fn segment(s: &str) -> String {
    let mut u = reqwest::Url::parse("https://matrix.invalid/").unwrap();
    u.path_segments_mut().unwrap().push(s);
    u.path()[1..].into()
}
impl Operation {
    pub(in crate::console) fn request(
        self,
    ) -> Result<(reqwest::Method, String, Option<Value>), crate::console::server_login::OwnerError>
    {
        self.parts()
            .map_err(|e| crate::console::server_login::OwnerError {
                status: e.status,
                code: e.code,
            })
    }
    fn parts(self) -> Result<(reqwest::Method, String, Option<Value>), NativeError> {
        use reqwest::Method;
        Ok(match self {
            Self::Discovery => (Method::GET, "/hagency/api/v1/discovery".into(), None),
            Self::Joined => (Method::GET, "/_matrix/client/v3/joined_rooms".into(), None),
            Self::State(id) => {
                room_id(&id)?;
                (
                    Method::GET,
                    format!("/_matrix/client/v3/rooms/{}/state", segment(&id)),
                    None,
                )
            }
            Self::Create(c) => {
                c.validate()?;
                (
                    Method::POST,
                    "/_matrix/client/v3/createRoom".into(),
                    Some(c.body()),
                )
            }
            Self::Invite { room, user } => {
                room_id(&room)?;
                mxid(&user)?;
                (
                    Method::POST,
                    format!("/_matrix/client/v3/rooms/{}/invite", segment(&room)),
                    Some(json!({"user_id":user})),
                )
            }
            Self::Link {
                space,
                room,
                parent,
                server,
            } => {
                room_id(&room)?;
                room_id(&space)?;
                if server.is_empty()
                    || server.len() > 255
                    || server.chars().any(char::is_whitespace)
                {
                    return Err(invalid());
                }
                let (room, kind, key) = if parent {
                    (room, "m.space.parent", space)
                } else {
                    (space, "m.space.child", room)
                };
                (
                    Method::PUT,
                    format!(
                        "/_matrix/client/v3/rooms/{}/state/{kind}/{}",
                        segment(&room),
                        segment(&key)
                    ),
                    Some(link_content(parent, &server)),
                )
            }
        })
    }
}
pub(super) fn link_content(parent: bool, server: &str) -> Value {
    if parent {
        json!({"via":[server],"canonical":true})
    } else {
        json!({"via":[server]})
    }
}
pub(super) fn event<'a>(
    state: &'a Value,
    kind: &str,
    key: &str,
) -> Result<Option<&'a Value>, NativeError> {
    let mut all = state
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .filter(|e| e["type"] == kind && e["state_key"] == key);
    let found = all.next();
    if all.next().is_some() {
        return Err(failure(409, "project_matrix_proof_conflict"));
    }
    Ok(found)
}
pub(super) fn member(state: &Value, user: &str) -> Result<Option<String>, NativeError> {
    Ok(event(state, "m.room.member", user)?
        .and_then(|e| e["content"]["membership"].as_str())
        .map(str::to_owned))
}
pub(super) fn prove(state: &Value, payload: &Create) -> Result<(), NativeError> {
    let create = event(state, "m.room.create", "")?.ok_or_else(invalid)?;
    let mark = event(state, MARKER, "")?.ok_or_else(invalid)?;
    let owner = &payload.setup.manager_mxid;
    if create["sender"] != *owner
        || create["content"].get("creator").is_some_and(|v| v != owner)
        || mark["sender"] != *owner
        || mark["content"] != payload.marker()
        || (create["content"]["type"] == "m.space") != payload.space
        || event(state, "m.room.encryption", "")?.is_some()
        || event(state, "m.room.join_rules", "")?
            .is_none_or(|e| e["content"]["join_rule"] != "invite")
        || member(state, owner)?.as_deref() != Some("join")
    {
        return Err(failure(409, "project_matrix_marker_mismatch"));
    }
    let power = event(state, "m.room.power_levels", "")?.ok_or_else(invalid)?;
    let c = &power["content"];
    let level = c["users"][owner]
        .as_i64()
        .or_else(|| c["users_default"].as_i64())
        .unwrap_or(0);
    let needed = c["state_default"]
        .as_i64()
        .unwrap_or(50)
        .max(c["invite"].as_i64().unwrap_or(0));
    if level < 100 || level < needed {
        return Err(failure(403, "project_matrix_control_required"));
    }
    Ok(())
}
