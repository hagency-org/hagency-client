//! Existing mobile v1 storage selected only at compile time. This does not
//! qualify mobile native credentials or provide a desktop runtime fallback.
use super::*;
use hagency_store::credential_vault::{Error, document::Document};

pub(super) fn load(
    root: PathBuf,
    device: AuthorizedDevice,
    agent: String,
    generation: i64,
    authority: AgentCryptoAuthority,
) -> Result<AgentCryptoContext, Error> {
    if authority.is_retired() || device.bearer().is_err() {
        return Err(Error::Retired);
    }
    let ledger = owned_agents::ledger_path(
        &root,
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .map_err(|_| Error::Invalid)?;
    let profile = owned_agents::profile_identity(
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .map_err(|_| Error::Invalid)?;
    let parent = ledger.parent().ok_or(Error::Invalid)?.join("agent-crypto");
    hagency_store::private::directory(&parent).map_err(|_| Error::Storage)?;
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(
        serde_json::to_vec(&(&profile, &agent, device.device_id())).map_err(|_| Error::Invalid)?,
    );
    let directory = parent.join(format!("agent_{digest:x}"));
    hagency_store::private::directory(&directory).map_err(|_| Error::Storage)?;
    match std::fs::symlink_metadata(directory.join(".credential.json.vault-journal")) {
        Ok(_) => return Err(Error::Invalid),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err(Error::Storage),
    }
    let path = directory.join("credential.json");
    let doc = Document::open(&path, 16384)?;
    let source = doc.read()?;
    let mut credential = if let Some(source) = &source {
        let credential: Credential =
            serde_json::from_slice(source.bytes()).map_err(|_| Error::Invalid)?;
        if credential.version != 1
            || credential.profile != profile
            || credential.agent != agent
            || credential.device != device.device_id()
            || credential.passphrase.len() != 64
        {
            return Err(Error::Invalid);
        }
        credential
    } else {
        for entry in std::fs::read_dir(&directory).map_err(|_| Error::Storage)? {
            if entry.map_err(|_| Error::Storage)?.file_name() != ".credential.json.vault-lock" {
                return Err(Error::Missing);
            }
        }
        let mut random = Zeroizing::new([0u8; 48]);
        getrandom::fill(random.as_mut()).map_err(|_| Error::Randomness)?;
        use base64::Engine;
        Credential {
            version: 1,
            profile,
            agent: agent.clone(),
            device: device.device_id().into(),
            passphrase: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random.as_slice()),
        }
    };
    let _gate = authority
        .publication
        .lock()
        .map_err(|_| Error::Unavailable)?;
    if authority.is_retired() || device.bearer().is_err() {
        return Err(Error::Retired);
    }
    doc.check_original(source.as_ref())?;
    if source.is_none() {
        let bytes = Zeroizing::new(serde_json::to_vec(&credential).map_err(|_| Error::Invalid)?);
        hagency_store::private::write_new(&path, &bytes).map_err(|_| Error::DurabilityUnknown)?;
    }
    let sdk = directory.join("sdk");
    hagency_store::private::directory(&sdk).map_err(|_| Error::Storage)?;
    drop(_gate);
    Ok(AgentCryptoContext {
        authority,
        agent,
        generation,
        root: sdk,
        passphrase: Zeroizing::new(std::mem::take(&mut credential.passphrase)),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn credential_migration_mobile_legacy_agent_keeps_original_v1_and_refuses_v2_or_unknown()
    {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("state");
        let owner = NativeOwner::open_inner_with_credentials(
            &root,
            "http://127.0.0.1:9",
            "@owner:example.test",
            false,
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .unwrap();
        let device = crate::console::server_login::fixture_device(
            "http://127.0.0.1:9/",
            "owner",
            "@owner:example.test",
        )
        .0;
        let auth = || AgentCryptoAuthority {
            owner: Arc::downgrade(&owner.0),
            pin: DeviceIdentity::from(&device),
            retired: Arc::new(AtomicBool::new(false)),
            publication: owner
                .0
                .host
                .console
                .0
                .server_login
                .credential_publication
                .clone(),
        };
        let original = load(root.clone(), device.clone(), "agent".into(), 1, auth()).unwrap();
        let path = original.root().parent().unwrap().join("credential.json");
        let bytes = std::fs::read(&path).unwrap();
        let again = load(root.clone(), device.clone(), "agent".into(), 8, auth()).unwrap();
        assert_eq!(original.passphrase(), again.passphrase());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        for bytes in [
            b"{\"version\":2,\"reference\":{}}".as_slice(),
            b"{\"version\":99}".as_slice(),
        ] {
            hagency_store::private::replace(&path, bytes).unwrap();
            assert!(load(root.clone(), device.clone(), "agent".into(), 9, auth()).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        std::fs::remove_file(&path).unwrap();
        assert!(load(root, device.clone(), "agent".into(), 9, auth()).is_err());
        assert!(!path.exists());
        owner.shutdown_checked().await.unwrap();
    }
}
