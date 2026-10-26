//! Local-only whole-owner ledger operations. No public route accepts paths.
use super::{Identity, NativeError, NativeOwner, failure};
use crate::console::{device_execution::DeviceIdentity, owned_agents};
use hagency_agent_local::{
    Ledger,
    backup::{MAX_BYTES, Snapshot},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// Paths come from an explicit native local-file selection, never a server
/// configuration request. Restore binds the digest of the inspected file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LedgerAction {
    Export { path: String },
    Inspect { path: String },
    Restore { path: String, digest: String },
}
pub(super) async fn execute(
    native: &NativeOwner,
    action: LedgerAction,
) -> Result<Value, NativeError> {
    let device = native
        .0
        .host
        .console
        .authorized_device()
        .await
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    let pin = DeviceIdentity::from(&device);
    let identity = native.identity().await?;
    if identity
        != (Identity {
            origin: pin.origin.clone(),
            issuer: pin.issuer.clone(),
            subject: pin.subject.clone(),
            owner: pin.owner.clone(),
        })
    {
        return Err(failure(401, "owner_authorization_required"));
    }
    let (path, restore) = match &action {
        LedgerAction::Export { path } | LedgerAction::Inspect { path } => (path, false),
        LedgerAction::Restore { path, .. } => (path, true),
    };
    let path = Path::new(path).to_path_buf();
    if !path.is_absolute() || path.as_os_str().len() > 16384 {
        return Err(failure(400, "invalid_backup_path"));
    }
    if restore
        && native
            .0
            .host
            .console
            .0
            .owned_runtime
            .profile_busy(&device)
            .await
    {
        return Err(failure(409, "ledger_runtime_busy"));
    }
    let profile = owned_agents::profile_identity(
        &identity.origin,
        &identity.issuer,
        &identity.subject,
        &identity.owner,
    )
    .map_err(|_| failure(403, "owner_scope_required"))?;
    let snapshot = match &action {
        LedgerAction::Export { .. } => {
            let ledger = local_ledger(native, &identity, &profile)?;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| failure(503, "local_state_unavailable"))?
                .as_secs();
            ledger
                .snapshot(
                    &identity.owner,
                    &profile,
                    now.try_into()
                        .map_err(|_| failure(503, "local_state_unavailable"))?,
                )
                .map_err(local_error)?
        }
        _ => Snapshot::inspect(read(&path)?, &identity.owner, &profile).map_err(local_error)?,
    };
    // An awaited source refresh can retire this exact account/device. Check all
    // fields again immediately before any local publication or restoration.
    let fresh = native
        .0
        .host
        .console
        .authorized_device()
        .await
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    if DeviceIdentity::from(&fresh) != pin || native.identity().await? != identity {
        return Err(failure(401, "owner_authorization_required"));
    }
    fresh
        .bearer()
        .map_err(|_| failure(401, "owner_authorization_required"))?;
    native.check_matrix_source().await?;
    let operation = match action {
        LedgerAction::Export { .. } => {
            write_new(&path, snapshot.bytes())?;
            "exported"
        }
        LedgerAction::Inspect { .. } => "inspected",
        LedgerAction::Restore { digest, .. } => {
            if digest != snapshot.digest() {
                return Err(failure(409, "ledger_backup_changed"));
            }
            if native
                .0
                .host
                .console
                .0
                .owned_runtime
                .profile_busy(&fresh)
                .await
            {
                return Err(failure(409, "ledger_runtime_busy"));
            }
            let mut ledger = local_ledger(native, &identity, &profile)?;
            ledger
                .restore_snapshot_checked(&identity.owner, &profile, &snapshot, || {
                    fresh
                        .bearer()
                        .map(|_| ())
                        .map_err(|_| hagency_agent_local::Error::Unauthorized)
                })
                .map_err(|error| match error {
                    hagency_agent_local::Error::Unauthorized => {
                        failure(401, "owner_authorization_required")
                    }
                    error => local_error(error),
                })?;
            "restored"
        }
    };
    Ok(
        json!({"operation":operation,"path":path.to_str(),"digest":snapshot.digest(),"snapshot":snapshot.summary()}),
    )
}
pub(super) fn local_ledger(
    native: &NativeOwner,
    identity: &Identity,
    profile: &str,
) -> Result<Ledger, NativeError> {
    let root = native.0.host.state_directory();
    let path = owned_agents::ledger_path(
        root,
        &identity.origin,
        &identity.issuer,
        &identity.subject,
        &identity.owner,
    )
    .map_err(|_| failure(503, "local_state_unavailable"))?;
    Ledger::open_scoped(path, &identity.owner, profile).map_err(local_error)
}
fn local_error(error: hagency_agent_local::Error) -> NativeError {
    use hagency_agent_local::Error::*;
    match error {
        Unauthorized => failure(403, "ledger_backup_owner_mismatch"),
        ForeignDatabase => failure(409, "ledger_backup_schema_unsupported"),
        Conflict => failure(409, "ledger_restore_requires_empty_ledger"),
        Invalid(_) | Json(_) => failure(400, "invalid_ledger_backup"),
        _ => failure(503, "ledger_backup_unavailable"),
    }
}
fn read(path: &Path) -> Result<Vec<u8>, NativeError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| failure(400, "invalid_backup_path"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_BYTES as u64
    {
        return Err(failure(400, "invalid_ledger_backup"));
    }
    let file = std::fs::File::open(path).map_err(|_| failure(400, "invalid_backup_path"))?;
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| failure(503, "ledger_backup_unavailable"))?;
    if bytes.len() > MAX_BYTES {
        return Err(failure(400, "invalid_ledger_backup"));
    }
    Ok(bytes)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), NativeError> {
    // create_new rejects existing files, directories and links; the file is
    // private from creation on both Windows and Unix. Never truncate a backup.
    let mut file = hagency_store::private::open(path, true)
        .map_err(|_| failure(409, "backup_destination_unavailable"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| failure(503, "ledger_backup_unavailable"))?;
    #[cfg(unix)]
    std::fs::File::open(
        path.parent()
            .ok_or_else(|| failure(400, "invalid_backup_path"))?,
    )
    .and_then(|directory| directory.sync_all())
    .map_err(|_| failure(503, "ledger_backup_unavailable"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_export_is_private_create_only_and_read_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner.hagency-ledger");
        write_new(&path, b"snapshot").unwrap();
        assert_eq!(read(&path).unwrap(), b"snapshot");
        assert_eq!(
            write_new(&path, b"replacement").unwrap_err().code,
            "backup_destination_unavailable"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"snapshot");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
                0
            );
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(read(&link).is_err());
            assert!(write_new(&link, b"bad").is_err());
        }
        let large = dir.path().join("large");
        std::fs::File::create(&large)
            .unwrap()
            .set_len((MAX_BYTES + 1) as u64)
            .unwrap();
        assert!(read(&large).is_err());
    }
}
