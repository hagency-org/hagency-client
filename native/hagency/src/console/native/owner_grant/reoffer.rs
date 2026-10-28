//! Reconcile original local grants before sending a new resource offer.
use super::*;

pub(in crate::console::native) async fn check(
    manager: &NativeManagement, contribution: &str,
) -> Result<(), NativeError> {
    #[cfg(not(feature = "resource-adoption"))]
    {
        let _ = (manager, contribution);
        Err(failure(409, "management_reoffer_local_custody_required"))
    }
    #[cfg(feature = "resource-adoption")]
    {
        super::super::key(contribution)?;
        manager.authorized().await?;
        let cookie = manager.owner.0.cookie.lock().await
            .strip_prefix("hagency_console=").map(str::to_owned)
            .ok_or_else(|| failure(401, "owner_authorization_required"))?;
        let io = NativeIo::open(manager, &cookie, Permission::ContributeResources, None).await?;
        let registry = io.session.contribution_registry(&io.context).map_err(|error| {
            if matches!(error, hagency_store::Error::LocalAuthority) {
                failure(409, "management_reoffer_local_custody_required")
            } else { storage(error) }
        })?;
        check_originals(&io, &registry, contribution).await
    }
}

#[cfg(any(feature = "resource-adoption", test))]
pub(super) async fn check_originals(
    io: &NativeIo<'_>, registry: &OwnerResourceRegistry, contribution: &str,
) -> Result<(), NativeError> {
    io.check().await?;
    let closed = registry.closed_contribution_grants(io.profile.clone(),
        io.identity.device.clone(), contribution.into()).await.map_err(|error| {
            if matches!(error, hagency_store::Error::Conflict) {
                failure(409, "owner_grant_prior_intent_required")
            } else { storage(error) }
        })?;
    io.check().await?;
    let mut history = History::load(&io.path, io.identity.clone())?;
    for index in 0..history.data.records.len() {
        if history.data.records[index].original.binding.contribution_id != contribution { continue }
        let progress = recover_record(io, registry, &mut history, index).await?;
        match progress.state {
            OwnerGrantState::Declined => {},
            OwnerGrantState::Recorded if closed.contains(&history.data.records[index].original.selection.grant_id) => {},
            _ => return Err(failure(409, "owner_grant_prior_intent_required")),
        }
    }
    io.check().await
}
