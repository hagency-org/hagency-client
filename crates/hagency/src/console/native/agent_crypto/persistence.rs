//! Native small-secret storage; the SDK directory always reuses its original key.
use super::*;
use hagency_store::credential_vault::document::{self, Document, Published};
use hagency_store::credential_vault::{Error, Purpose, Scope, SecretRef, Vault};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    version: u32,
    profile: String,
    agent: String,
    device: String,
    reference: SecretRef,
}
impl Descriptor {
    fn check(&self, profile: &str, agent: &str, device: &str) -> Result<(), Error> {
        if self.version != 2
            || self.profile != profile
            || self.agent != agent
            || self.device != device
        {
            return Err(Error::Integrity);
        }
        Ok(())
    }
}
fn original(
    vault: &Vault,
    reference: &SecretRef,
    scope: &Scope,
) -> Result<Zeroizing<String>, Error> {
    let bytes = vault.load(reference, scope)?;
    let key = std::str::from_utf8(&bytes).map_err(|_| Error::Invalid)?;
    if key.len() != 64 {
        return Err(Error::Invalid);
    }
    Ok(Zeroizing::new(key.to_owned()))
}
fn publish(
    doc: &Document,
    prepared: document::Prepared,
    snapshot: Option<&document::Snapshot>,
    authority: &AgentCryptoAuthority,
    device: &AuthorizedDevice,
) -> Result<(), Error> {
    // No OS credential calls or await under this gate. Retirement acknowledges
    // only after crossing this same gate and waiting for the original init task.
    let _gate = authority
        .publication
        .lock()
        .map_err(|_| Error::Unavailable)?;
    let current = || !authority.is_retired() && device.bearer().is_ok();
    match doc.publish(prepared, snapshot, current)? {
        Published::Durable => Ok(()),
        Published::DurabilityUnknown => Err(Error::DurabilityUnknown),
    }
}
pub(super) fn load(
    vault: Vault,
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
    let path = directory.join("credential.json");
    let doc = Document::open(&path, 16384)?;
    let source = doc.read()?;
    #[derive(Deserialize)]
    struct Version {
        version: u32,
    }
    let version = source
        .as_ref()
        .map(|s| {
            serde_json::from_slice::<Version>(s.bytes())
                .map(|v| v.version)
                .map_err(|_| Error::Invalid)
        })
        .transpose()?;
    if version.is_some_and(|v| v != 1 && v != 2) {
        return Err(Error::Invalid);
    }
    if source.is_none() && directory.join("sdk").exists() {
        return Err(Error::Missing);
    }
    if std::fs::symlink_metadata(directory.join(".credential.json.vault-journal")).is_ok()
        && !root.join("credentials-v1/installation").exists()
    {
        return Err(Error::Missing);
    }
    let installation = document::installation(&root, version != Some(2))?;
    let identity = serde_json::to_vec(&("agent-database-v2", &profile, &agent, device.device_id()))
        .map_err(|_| Error::Invalid)?;
    let scope = Scope::new(&installation, Purpose::AgentDatabase, &identity)?;
    let pending = doc.pending(&scope)?;
    let passphrase = if version == Some(2) {
        let descriptor: Descriptor =
            serde_json::from_slice(source.as_ref().ok_or(Error::Missing)?.bytes())
                .map_err(|_| Error::Invalid)?;
        descriptor.check(&profile, &agent, device.device_id())?;
        let key = original(&vault, &descriptor.reference, &scope)?;
        if let Some(journal) = pending {
            doc.finish_authenticated(&journal, std::slice::from_ref(&descriptor.reference))?;
        }
        key
    } else {
        let legacy = if let Some(source) = &source {
            let value: Credential =
                serde_json::from_slice(source.bytes()).map_err(|_| Error::Invalid)?;
            if value.version != 1
                || value.profile != profile
                || value.agent != agent
                || value.device != device.device_id()
                || value.passphrase.len() != 64
            {
                return Err(Error::Invalid);
            }
            Some(value)
        } else {
            None
        };
        // An SDK directory without its descriptor is evidence of original keys,
        // even when empty. Never manufacture a replacement for that directory.
        if source.is_none() && directory.join("sdk").exists() {
            return Err(Error::Missing);
        }
        if source.is_none() && pending.is_none() {
            for item in std::fs::read_dir(&directory).map_err(|_| Error::Storage)? {
                let name = item.map_err(|_| Error::Storage)?.file_name();
                if name != ".credential.json.vault-lock" {
                    return Err(Error::Missing);
                }
            }
        }
        let reference = match &pending {
            Some(j) if j.references.len() == 1 => j.references[0].clone(),
            Some(_) => return Err(Error::Invalid),
            None => Vault::fresh_reference(&scope)?,
        };
        let journal = doc.begin(&scope, source.as_ref(), vec![reference.clone()])?;
        let key = match original(&vault, &reference, &scope) {
            Ok(key) => {
                if let Some(legacy) = &legacy
                    && key.as_str() != legacy.passphrase
                {
                    return Err(Error::Integrity);
                }
                key
            }
            Err(Error::Missing) if journal.candidate_digest().is_none() => {
                let key = match &legacy {
                    Some(legacy) => Zeroizing::new(legacy.passphrase.clone()),
                    None => {
                        let mut random = Zeroizing::new([0u8; 48]);
                        getrandom::fill(random.as_mut()).map_err(|_| Error::Randomness)?;
                        use base64::Engine;
                        Zeroizing::new(
                            base64::engine::general_purpose::URL_SAFE_NO_PAD
                                .encode(random.as_slice()),
                        )
                    }
                };
                match vault.create_at(&reference, key.as_bytes()) {
                    Ok(()) => (),
                    Err(Error::Conflict) => {
                        if original(&vault, &reference, &scope)?.as_str() != key.as_str() {
                            return Err(Error::Conflict);
                        }
                    }
                    Err(error) => return Err(error),
                }
                if original(&vault, &reference, &scope)?.as_str() != key.as_str() {
                    return Err(Error::Integrity);
                }
                key
            }
            Err(error) => return Err(error),
        };
        let descriptor = Descriptor {
            version: 2,
            profile: profile.clone(),
            agent: agent.clone(),
            device: device.device_id().into(),
            reference: reference.clone(),
        };
        let bytes = serde_json::to_vec(&descriptor).map_err(|_| Error::Invalid)?;
        let prepared = if journal.candidate_digest().is_some() {
            match doc.resume_candidate(&journal) {
                Ok((prepared, candidate)) => {
                    let decoded: Descriptor =
                        serde_json::from_slice(&candidate).map_err(|_| Error::Invalid)?;
                    decoded.check(&profile, &agent, device.device_id())?;
                    if decoded.reference != reference || candidate.as_slice() != bytes {
                        return Err(Error::Integrity);
                    }
                    prepared
                }
                Err(Error::Missing) => {
                    // This descriptor is deterministic and contains no token.
                    // Recover the journal-before-candidate crash only when its
                    // exact recorded digest matches this authenticated native ref.
                    let digest = format!("{:x}", Sha256::digest(&bytes));
                    if journal.candidate_digest() != Some(digest.as_str()) {
                        return Err(Error::Integrity);
                    }
                    doc.prepare(journal, &bytes)?
                }
                Err(error) => return Err(error),
            }
        } else {
            doc.prepare(journal, &bytes)?
        };
        publish(&doc, prepared, source.as_ref(), &authority, &device)?;
        key
    };
    if authority.is_retired() || device.bearer().is_err() {
        return Err(Error::Retired);
    }
    let sdk = directory.join("sdk");
    hagency_store::private::directory(&sdk).map_err(|_| Error::Storage)?;
    if authority.is_retired() || device.bearer().is_err() {
        return Err(Error::Retired);
    }
    Ok(AgentCryptoContext {
        authority,
        agent,
        generation,
        root: sdk,
        passphrase,
    })
}
