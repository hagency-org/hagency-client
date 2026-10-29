//! Discovery cursors are private to this live owner and its exact device pin.
use super::{NativeError, NativeOwner, failure};
use crate::console::device_execution::DeviceIdentity;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant},
};
const PAGE_SIZE: usize = 32;
const MAX_CURSORS: usize = 128;
const CURSOR_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::console) struct Summary {
    pub agent_id: String,
    pub binding_id: String,
    pub room_id: String,
    pub requester: String,
    pub proposal_id: String,
    pub args_digest: String,
    pub tool: String,
    pub expires: i64,
}
struct Cursor {
    pin: DeviceIdentity,
    after: (String, String),
    until: Instant,
}
#[derive(Default)]
pub(super) struct Pager(Mutex<BTreeMap<String, Cursor>>);
fn invalid() -> NativeError {
    failure(409, "approval_cursor_expired")
}
impl Pager {
    fn after(&self, token: &str, pin: &DeviceIdentity) -> Result<(String, String), NativeError> {
        if token.len() != 64 {
            return Err(invalid());
        }
        let mut cursors = self.0.lock().map_err(|_| invalid())?;
        cursors.retain(|_, c| c.until > Instant::now());
        cursors
            .get(token)
            .filter(|c| &c.pin == pin)
            .map(|c| c.after.clone())
            .ok_or_else(invalid)
    }
    fn issue(&self, pin: DeviceIdentity, after: (String, String)) -> Result<String, NativeError> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| failure(503, "approvals_unavailable"))?;
        let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let mut cursors = self.0.lock().map_err(|_| invalid())?;
        cursors.retain(|_, c| c.until > Instant::now());
        if cursors.len() >= MAX_CURSORS {
            let oldest = cursors
                .iter()
                .min_by_key(|(_, c)| c.until)
                .map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                cursors.remove(&oldest);
            }
        }
        cursors.insert(
            token.clone(),
            Cursor {
                pin,
                after,
                until: Instant::now() + CURSOR_TTL,
            },
        );
        Ok(token)
    }
}
pub(super) async fn execute(
    native: &NativeOwner,
    cursor: Option<String>,
) -> Result<Value, NativeError> {
    native.check_matrix_source().await?;
    let device = native.authorized_execution_device().await?;
    let pin = DeviceIdentity::from(&device);
    let after = cursor
        .as_deref()
        .map(|c| native.0.pending_approvals.after(c, &pin))
        .transpose()?;
    let (rows, total, more) = native
        .0
        .host
        .console
        .0
        .owned_runtime
        .pending_page(&device, after.as_ref(), PAGE_SIZE)
        .await
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    native.check_matrix_source().await?;
    let current = native.authorized_execution_device().await?;
    if DeviceIdentity::from(&current) != pin {
        return Err(failure(401, "matrix_account_changed"));
    }
    let next = if more {
        rows.last()
            .map(|r| {
                native
                    .0
                    .pending_approvals
                    .issue(pin, (r.agent_id.clone(), r.proposal_id.clone()))
            })
            .transpose()?
    } else {
        None
    };
    current
        .bearer()
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    Ok(json!({"approvals":rows,"total":total,"nextCursor":next}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pin() -> DeviceIdentity {
        DeviceIdentity {
            origin: "https://server.test/".into(),
            issuer: "https://server.test/auth/".into(),
            subject: "subject".into(),
            owner: "@owner:test".into(),
            user: "subject".into(),
            id: "device".into(),
            generation: 1,
        }
    }
    #[test]
    fn discovery_cursors_expire_and_cannot_cross_device_owner_or_instance() {
        let pager = Pager::default();
        let token = pager
            .issue(pin(), ("agent".into(), "proposal".into()))
            .unwrap();
        assert_eq!(
            pager.after(&token, &pin()).unwrap(),
            ("agent".into(), "proposal".into())
        );
        let mut other = pin();
        other.generation += 1;
        assert!(pager.after(&token, &other).is_err());
        other = pin();
        other.id = "another-device".into();
        assert!(pager.after(&token, &other).is_err());
        other = pin();
        other.origin = "https://other.test/".into();
        assert!(pager.after(&token, &other).is_err());
        other = pin();
        other.owner = "@other:test".into();
        assert!(pager.after(&token, &other).is_err());
        assert!(Pager::default().after(&token, &pin()).is_err());
        pager.0.lock().unwrap().get_mut(&token).unwrap().until =
            Instant::now() - Duration::from_secs(1);
        assert!(pager.after(&token, &pin()).is_err());
        for index in 0..MAX_CURSORS + 2 {
            pager
                .issue(pin(), ("agent".into(), index.to_string()))
                .unwrap();
        }
        assert_eq!(pager.0.lock().unwrap().len(), MAX_CURSORS);
    }
}
