//! Human Run/Stop intent. Readiness is descriptive; every effect reauthorizes.
use super::*;
use reqwest::Method;
const FEATURE: &str = "shared-run-control-v1";
const MAX: i64 = 9_007_199_254_740_991;
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunMode {
    Running,
    Paused,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Control {
    assignment_id: String,
    revision: i64,
    mode: String,
    running_since_ms: Option<i64>,
    updated_at_ms: i64,
    origin: String,
}
impl Control {
    fn check(&self, id: &str) -> Result<(), NativeError> {
        if self.assignment_id != id
            || !(1..=MAX).contains(&self.revision)
            || !matches!(self.mode.as_str(), "manual" | "running" | "paused")
            || !(1..=MAX).contains(&self.updated_at_ms)
            || self
                .running_since_ms
                .is_some_and(|v| !(1..=MAX).contains(&v))
            || (self.mode == "running") != self.running_since_ms.is_some()
            || self.origin.is_empty()
            || self.origin.len() > 80
            || self.origin.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Readiness {
    control: Control,
    can_run: bool,
    can_stop: bool,
    requires_local_stop: bool,
    run_blocking_reason: Option<String>,
    intake_enabled: bool,
    automatic_delivery_enabled: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadinessReply {
    readiness: Readiness,
    observed_at_ms: i64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ControlReply {
    command_id: String,
    applied_revision: i64,
    control: Control,
    observed_at_ms: i64,
}
fn invalid() -> NativeError {
    failure(503, "management_run_control_invalid")
}
pub(in crate::console::native) fn known_failure(e: &NativeError) -> bool {
    // A local Stop can already be durable when a later remote check fails.
    e.status == 400
        && matches!(
            e.code.as_str(),
            "invalid_management_arguments" | "invalid_management_revision"
        )
}
async fn request(
    m: &NativeManagement,
    permission: Permission,
    operation: ServerOperation,
    mutation: bool,
) -> Result<Value, NativeError> {
    let (context, session) = m.authorized().await?;
    session
        .authorize(&context, permission)
        .map_err(|_| failure(403, "management_permission_required"))?;
    let console = &m.owner.0.host.console;
    let device = console
        .authorized_device()
        .await
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    let pin = DeviceIdentity::from(&device);
    let (before_send, fresh) = m.authorized().await?;
    if before_send != context {
        return Err(failure(409, "management_scope_changed"));
    }
    fresh
        .authorize(&context, permission)
        .map_err(|_| failure(403, "management_permission_required"))?;
    let result = console
        .owner_api_pinned(&pin, OwnerOperation::Management(operation))
        .await;
    let (current, live) = m.authorized().await.map_err(|e| {
        if mutation && result.is_ok() {
            failure(409, "management_outcome_unknown")
        } else {
            e
        }
    })?;
    if current != context {
        return Err(failure(
            409,
            if mutation {
                "management_outcome_unknown"
            } else {
                "management_scope_changed"
            },
        ));
    }
    live.authorize(&current, permission)
        .map_err(|_| failure(403, "management_permission_required"))?;
    let reply = result.map_err(|e| failure(e.status, &e.code))?;
    if reply.owner != device.owner_mxid()
        || reply.origin != device.origin()
        || reply.issuer != device.issuer()
        || reply.subject != device.subject()
    {
        return Err(failure(401, "management_scope_changed"));
    }
    Ok(reply.value)
}
async fn readiness(
    m: &NativeManagement,
    id: &str,
    permission: Permission,
) -> Result<ReadinessReply, NativeError> {
    let value = request(
        m,
        permission,
        ServerOperation {
            method: Method::GET,
            suffix: format!("shared-execution/assignments/{id}/control/readiness"),
            input: None,
        },
        false,
    )
    .await?;
    let r: ReadinessReply = serde_json::from_value(value).map_err(|_| invalid())?;
    r.readiness.control.check(id)?;
    if !(1..=MAX).contains(&r.observed_at_ms)
        || r.readiness
            .run_blocking_reason
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 80 || s.chars().any(char::is_control))
    {
        return Err(invalid());
    }
    Ok(r)
}
pub(super) async fn execute(
    m: &NativeManagement,
    c: ManagementCommand,
) -> Result<Value, NativeError> {
    if !cfg!(feature = "shared-intake") {
        return Err(failure(409, "shared_intake_disabled"));
    }
    let (id, input) = match &c {
        ManagementCommand::AgentRunReadiness { assignment_id } => (assignment_id, None),
        ManagementCommand::AgentRunControl {
            command_id,
            assignment_id,
            expected_revision,
            mode,
        } => {
            super::super::key(command_id)?;
            if !(1..MAX).contains(expected_revision) {
                return Err(failure(400, "invalid_management_revision"));
            }
            (assignment_id, Some((command_id, *expected_revision, *mode)))
        }
        _ => return Err(failure(400, "invalid_management_operation")),
    };
    super::super::key(id)?;
    let permission = c.permission();
    #[cfg(feature = "shared-intake")]
    let local = if let Some((command_id, revision, AgentRunMode::Paused)) = input {
        match m.owner_stop_transport().await {
            Ok((transport, registry)) => match transport
                .prepare_reviewed_owner_stop(
                    &registry,
                    id.clone(),
                    hagency_store::owner_resources::shared_execution::SharedDemandStopInput {
                        command_id: command_id.clone(),
                        expected_revision: revision as u64,
                        mode: "paused".into(),
                    },
                )
                .await
            {
                Ok(Some(intent)) => Ok(Some((transport, registry, intent))),
                Ok(None) => Ok(None),
                Err(e) => return Err(e),
            },
            Err(e) => Err(e),
        }
    } else {
        Ok(None)
    };
    #[cfg(feature = "shared-intake")]
    let local_saved = local.as_ref().is_ok_and(Option::is_some);
    #[cfg(not(feature = "shared-intake"))]
    let local_saved = false;
    let result = execute_remote(
        m,
        id,
        input,
        permission,
        #[cfg(feature = "shared-intake")]
        local,
        local_saved,
    )
    .await;
    local_stop_result(local_saved, result)
}
#[cfg(feature = "shared-intake")]
type LocalStop = (
    super::super::shared_execution::SharedExecutionTransport,
    hagency_store::owner_resources::OwnerResourceRegistry,
    hagency_store::owner_resources::shared_execution::SharedDemandStopIntent,
);
async fn execute_remote(
    m: &NativeManagement,
    id: &str,
    input: Option<(&String, i64, AgentRunMode)>,
    permission: Permission,
    #[cfg(feature = "shared-intake")] local: Result<Option<LocalStop>, NativeError>,
    local_saved: bool,
) -> Result<Value, NativeError> {
    let session = request(
        m,
        permission,
        ServerOperation {
            method: Method::GET,
            suffix: "session".into(),
            input: None,
        },
        false,
    )
    .await?;
    let features = session["features"]
        .as_array()
        .filter(|v| v.len() <= 64)
        .ok_or_else(invalid)?;
    if features
        .iter()
        .any(|v| v.as_str().is_none_or(|s| s.len() > 128))
    {
        return Err(invalid());
    }
    if !features.iter().any(|v| v.as_str() == Some(FEATURE)) {
        return Err(failure(409, "shared_run_control_unavailable"));
    }
    let review = readiness(m, id, permission).await?;
    let Some((command_id, expected_revision, mode)) = input else {
        return serde_json::to_value(review).map_err(|_| invalid());
    };
    if mode == AgentRunMode::Paused && review.readiness.requires_local_stop {
        #[cfg(feature = "shared-intake")]
        {
            let (transport, registry, intent) =
                local?.ok_or_else(|| failure(409, "owner_stop_local_custody_required"))?;
            let receipt = transport
                .finish_reviewed_owner_stop(&registry, intent)
                .await?;
            let latest = readiness(m, id, permission).await?;
            if latest.readiness.control.revision < receipt.applied_revision as i64 {
                return Err(invalid());
            }
            return Ok(
                json!({"commandId":receipt.command_id,"appliedRevision":receipt.applied_revision,"control":latest.readiness.control,"observedAtMs":latest.observed_at_ms}),
            );
        }
        #[cfg(not(feature = "shared-intake"))]
        return Err(failure(409, "shared_intake_disabled"));
    }
    if local_saved {
        // A current response that disagrees with proven original local owner
        // custody cannot redirect this Stop into the generic central path.
        return Err(failure(409, "management_scope_changed"));
    }
    let value = request(
        m,
        permission,
        ServerOperation {
            method: Method::POST,
            suffix: format!("shared-execution/assignments/{id}/control"),
            input: Some(
                json!({"commandId":command_id,"expectedRevision":expected_revision,"mode":mode}),
            ),
        },
        true,
    )
    .await?;
    let r: ControlReply = serde_json::from_value(value).map_err(|_| invalid())?;
    r.control.check(id)?;
    if r.command_id != *command_id
        || !(expected_revision..=expected_revision + 1).contains(&r.applied_revision)
        || r.control.revision < r.applied_revision
        || !(1..=MAX).contains(&r.observed_at_ms)
    {
        return Err(invalid());
    }
    serde_json::to_value(r).map_err(|_| invalid())
}
#[cfg(all(test, feature = "shared-intake"))]
pub(in crate::console::native) async fn test_saved_local_stop(
    m: &NativeManagement,
    local: LocalStop,
) -> Result<Value, NativeError> {
    let id = local.2.original().assignment_id.clone();
    let command = local.2.input().command_id.clone();
    let revision = local.2.input().expected_revision as i64;
    let result = execute_remote(
        m,
        &id,
        Some((&command, revision, AgentRunMode::Paused)),
        Permission::ChangeAgentRunControl,
        Ok(Some(local)),
        true,
    )
    .await;
    local_stop_result(true, result)
}
fn local_stop_result(
    local_saved: bool,
    result: Result<Value, NativeError>,
) -> Result<Value, NativeError> {
    if local_saved && result.is_err() {
        // Only a successfully committed exact Stop86 fence earns this code.
        // It remains pending: neither remote pause nor acknowledgement is claimed.
        Err(failure(409, "management_agent_control_local_stop_pending"))
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_run_control_surface_and_unknown_custody_are_closed() {
        assert_eq!(
            serde_json::to_value(AgentRunMode::Paused).unwrap(),
            "paused"
        );
        assert!(serde_json::from_value::<AgentRunMode>(json!("automatic")).is_err());
        for (status, code) in [
            (409, "shared_control_revision_changed"),
            (403, "shared_run_role_required"),
            (503, "readiness_unavailable"),
            (409, "owner_stop_local_custody_required"),
            (409, "management_agent_control_local_stop_pending"),
        ] {
            assert!(!known_failure(&failure(status, code)));
        }
        assert_eq!(
            local_stop_result(true, Err(failure(503, "readiness_unavailable")))
                .unwrap_err()
                .code,
            "management_agent_control_local_stop_pending"
        );
        assert_eq!(
            local_stop_result(false, Err(failure(503, "readiness_unavailable")))
                .unwrap_err()
                .code,
            "readiness_unavailable"
        );
        let c = ManagementCommand::AgentRunControl {
            command_id: "same_command".into(),
            assignment_id: "same_assignment".into(),
            expected_revision: 1,
            mode: AgentRunMode::Paused,
        };
        assert_eq!(
            c.intent().unwrap().unwrap().operation(),
            hagency_store::management::IntentOperation::AgentRunControl
        );
        assert!(
            ServerOperation::prepare(&c, None).is_err(),
            "generic POST cannot bypass native owner routing"
        );
    }
}
