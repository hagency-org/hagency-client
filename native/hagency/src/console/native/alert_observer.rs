//! Resource-scoped diagnostic observation owned by Desktop, not by a mini app.
//! No HTTP management listener, provider call or automatic admission decision.
use super::{NativeError, NativeOwner, management::ManagementPackage};
use hagency_store::management::{Permission, ResourceQuery};
use std::{
    sync::{Arc, Weak},
    time::Duration,
};

const IDLE: Duration = Duration::from_secs(30);
const PERIOD: Duration = Duration::from_secs(60 * 60);

/// Reuse the owner's retained cancellable task custody. Account retirement
/// cancels this producer before the management writer is drained.
pub(super) fn start(owner: &NativeOwner) {
    start_with_timing(owner, IDLE, PERIOD);
}

pub(super) fn start_with_timing(owner: &NativeOwner, idle: Duration, period: Duration) {
    let weak = Arc::downgrade(&owner.0);
    let task = tokio::spawn(run(weak, idle, period));
    owner
        .0
        .configuration_tasks
        .lock()
        .unwrap()
        .push(owner.0.configuration_custody.track_cancellable(task));
}

async fn run(weak: Weak<super::Inner>, idle: Duration, period: Duration) {
    let mut cursor = String::new();
    let mut delay = idle;
    loop {
        tokio::time::sleep(delay).await;
        let Some(inner) = weak.upgrade() else { break };
        if inner
            .configuration_stopped
            .load(std::sync::atomic::Ordering::Acquire)
        {
            break;
        }
        let owner = NativeOwner(inner);
        match observe_page(&owner, &mut cursor).await {
            Ok(Some(true)) => {
                cursor.clear();
                delay = period;
            }
            Ok(Some(false)) => delay = Duration::from_millis(100),
            Ok(None) => delay = idle,
            Err(error) => {
                // A failed or cancelled observation is diagnostic uncertainty;
                // retry only the unfinished cursor, never an execution command.
                tracing::warn!(code = %error.code, "Resource alert observation deferred");
                delay = idle;
            }
        }
    }
}

/// No database is created merely because someone signed in. Existing managed
/// history is reopened on startup, including when the mini app remains closed.
async fn has_backing(owner: &NativeOwner) -> Result<bool, NativeError> {
    if owner.0.management.has_store().await {
        return Ok(true);
    }
    owner.require_regular_mode()?;
    owner.sync_management_matrix().await?;
    let device = owner
        .0
        .host
        .console
        .authorized_device()
        .await
        .map_err(|_| super::failure(401, "owner_authorization_required"))?;
    let profile = crate::console::owned_agents::profile_identity(
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .map_err(|_| super::failure(401, "management_profile_mismatch"))?;
    let root = owner
        .0
        .host
        .console
        .0
        .server_login
        .state_directory()
        .ok_or_else(|| super::failure(503, "management_state_unavailable"))?;
    let path = root
        .join("owned-agent-owners")
        .join(format!("management_{profile}"))
        .join("domain.sqlite3");
    tokio::task::spawn_blocking(move || path.is_file())
        .await
        .map_err(|_| super::failure(503, "management_state_unavailable"))
}

async fn observe_page(
    owner: &NativeOwner,
    cursor: &mut String,
) -> Result<Option<bool>, NativeError> {
    if !has_backing(owner).await? {
        return Ok(None);
    }
    // This native-only package is never exposed through the script host or
    // consent catalog. Current SDK/device authority is checked on each boundary.
    let manager = owner
        .management(ManagementPackage {
            digest: "41".repeat(32),
            instance: 41,
            account_epoch: 1,
            permissions: vec![Permission::ReadResources, Permission::ObserveResourceAlerts],
        })
        .await?;
    let page = manager
        .resources(ResourceQuery {
            after: cursor.clone(),
            limit: 16,
            framework: None,
            capability: None,
            published: None,
        })
        .await?;
    for resource in page.resources {
        let (context, session) = manager.authorized().await?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| u64::try_from(d.as_millis()).ok())
            .ok_or_else(|| super::failure(503, "management_clock_unavailable"))?;
        let result = session
            .observe_resource_alert(&context, resource.id.clone(), now)
            .await;
        manager.finish(&context, result, true).await?;
        *cursor = resource.id;
    }
    match page.next_after {
        Some(next) => {
            *cursor = next;
            Ok(Some(false))
        }
        None => Ok(Some(true)),
    }
}
