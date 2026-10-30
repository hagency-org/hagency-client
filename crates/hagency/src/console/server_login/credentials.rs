//! Shared stateless native IO worker; every operation carries its explicit vault.
#[cfg(any(target_os = "ios", target_os = "android", test))]
mod legacy;
#[cfg(any(target_os = "ios", target_os = "android"))]
type ActiveRevision = legacy::Revision;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
type ActiveRevision = Revision;
use hagency_store::credential_vault::{Error, Vault, worker::Worker};
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub(in crate::console) struct CredentialIo {
    vault: Vault,
    worker: OnceLock<Arc<Worker>>,
    revocation_revision: Arc<Mutex<ActiveRevision>>,
}
impl CredentialIo {
    pub(super) fn new(vault: Vault) -> Self {
        Self {
            vault,
            worker: OnceLock::new(),
            revocation_revision: Arc::new(Mutex::new(ActiveRevision::default())),
        }
    }
    pub(in crate::console) async fn call<T: Send + 'static>(
        &self,
        operation: impl FnOnce(Vault) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let worker = if let Some(worker) = self.worker.get() {
            worker.clone()
        } else {
            // Share only the stateless bounded queue, never a backend or key.
            // Weak ownership lets the last real owner retire the original IO.
            static SHARED: Mutex<Weak<Worker>> = Mutex::new(Weak::new());
            let mut shared = SHARED.lock().map_err(|_| Error::Unavailable)?;
            let worker = match shared.upgrade() {
                Some(worker) => worker,
                None => {
                    let worker = Arc::new(Worker::new()?);
                    *shared = Arc::downgrade(&worker);
                    worker
                }
            };
            let _ = self.worker.set(worker.clone());
            self.worker.get().cloned().unwrap_or(worker)
        };
        let vault = self.vault.clone();
        worker.call(move || operation(vault)).await
    }
}

use super::Revocation;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use hagency_store::credential_vault::document::{self, Document, Published, Snapshot};
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use hagency_store::credential_vault::{DocumentKey, Envelope, Purpose, Scope, SecretRef};
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[cfg(not(any(target_os = "ios", target_os = "android")))]
const LIMIT: usize = 8 * 1024 * 1024;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeDocument {
    version: u32,
    envelope: Envelope,
}
#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[derive(Default)]
struct Revision {
    loaded: bool,
    snapshot: Option<Snapshot>,
    reference: Option<SecretRef>,
}

fn validate(rows: &[Revocation]) -> Result<(), Error> {
    if rows.len() > 64 {
        return Err(Error::Invalid);
    }
    for r in rows {
        super::origin(&r.origin).map_err(|_| Error::Invalid)?;
        if r.client_id.is_empty()
            || r.client_id.len() > 128
            || (!r.session_done && r.session_token.len() != 64)
            || (r.session_done && !r.session_token.is_empty() && r.session_token.len() != 64)
            || r.oauth_token.len() > 4096
            || (!r.oauth_done && r.oauth_token.is_empty())
            || !matches!(r.hint.as_str(), "access_token" | "refresh_token")
        {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn scope(path: &Path, allow: bool) -> Result<Scope, Error> {
    let root = path.parent().ok_or(Error::Invalid)?;
    if std::fs::symlink_metadata(root.join(".server-login-revocations.json.vault-journal")).is_ok()
        && !root.join("credentials-v1/installation").exists()
    {
        return Err(Error::Missing);
    }
    let installation = document::installation(root, allow)?;
    // The queue spans profiles. It is revoke-only, and never a session restore
    // document. Bind its complete physical data-root identity, not a display name.
    let identity = serde_json::to_vec(&(
        "standalone-server-revocations-v2",
        root.canonicalize().map_err(|_| Error::Storage)?,
    ))
    .map_err(|_| Error::Invalid)?;
    Scope::new(&installation, Purpose::Revocations, &identity)
}
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn decode(
    vault: &Vault,
    scope: &Scope,
    bytes: &[u8],
) -> Result<(SecretRef, Vec<Revocation>), Error> {
    let value: RevokeDocument = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    if value.version != 2 {
        return Err(Error::Invalid);
    }
    let key = vault.unlock(value.envelope.reference(), scope)?;
    let rows: Vec<Revocation> = key.open(&value.envelope)?;
    validate(&rows)?;
    Ok((key.reference().clone(), rows))
}
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn key_for(
    vault: &Vault,
    reference: &SecretRef,
    scope: &Scope,
    create: bool,
) -> Result<DocumentKey, Error> {
    match vault.unlock(reference, scope) {
        Ok(key) => Ok(key),
        Err(Error::Missing) if create => {
            let secret = DocumentKey::random_secret()?;
            match vault.create_at(reference, &secret) {
                Ok(()) => (),
                Err(Error::Conflict) => {
                    if vault.load(reference, scope)?.as_slice() != secret.as_slice() {
                        return Err(Error::Conflict);
                    }
                }
                Err(error) => return Err(error),
            }
            vault.unlock(reference, scope)
        }
        Err(error) => Err(error),
    }
}
#[cfg(not(any(target_os = "ios", target_os = "android")))]
impl Revision {
    fn load(&mut self, vault: &Vault, path: &Path) -> Result<Vec<Revocation>, Error> {
        let doc = Document::open(path, LIMIT)?;
        let snapshot = doc.read()?;
        // Even an absent canonical document may own an interrupted candidate.
        // Missing namespace cannot turn such a transaction into an empty queue.
        let legacy = snapshot.as_ref().is_some_and(|s| {
            s.bytes().iter().copied().find(|b| !b.is_ascii_whitespace()) == Some(b'[')
        });
        if snapshot.is_none() {
            let root = path.parent().ok_or(Error::Invalid)?;
            if !root.join("credentials-v1").exists() {
                let journal = root.join(".server-login-revocations.json.vault-journal");
                if std::fs::symlink_metadata(journal).is_ok() {
                    return Err(Error::Missing);
                }
                self.loaded = true;
                self.snapshot = None;
                return Ok(Vec::new());
            }
        }
        let scope = scope(path, legacy || snapshot.is_none())?;
        if let Some(source) = &snapshot {
            if !legacy {
                let (reference, mut rows) = decode(vault, &scope, source.bytes())?;
                if let Some(journal) = doc.pending(&scope)? {
                    if journal.candidate_digest() == Some(source.digest()) {
                        doc.finish_authenticated(&journal, std::slice::from_ref(&reference))?;
                    } else {
                        let journal = doc.begin(&scope, Some(source), vec![reference.clone()])?;
                        let (prepared, bytes) = doc.resume_candidate(&journal)?;
                        let (candidate_ref, candidate_rows) = decode(vault, &scope, &bytes)?;
                        if candidate_ref != reference {
                            return Err(Error::Integrity);
                        }
                        match doc.publish(prepared, Some(source), || true)? {
                            Published::Durable => (),
                            Published::DurabilityUnknown => return Err(Error::DurabilityUnknown),
                        }
                        rows = candidate_rows;
                    }
                }
                self.reference = Some(reference);
                self.snapshot = doc.read()?;
                self.loaded = true;
                return Ok(rows);
            }
            if source.bytes().len() > 1024 * 1024 {
                return Err(Error::Invalid);
            }
        }
        let rows: Vec<Revocation> = match &snapshot {
            Some(s) => serde_json::from_slice(s.bytes()).map_err(|_| Error::Invalid)?,
            None => Vec::new(),
        };
        validate(&rows)?;
        let pending = doc.pending(&scope)?;
        if snapshot.is_none() && pending.is_none() {
            self.snapshot = None;
            self.loaded = true;
            return Ok(rows);
        }
        let reference = match &pending {
            Some(j) if j.references.len() == 1 => j.references[0].clone(),
            Some(_) => return Err(Error::Invalid),
            None => Vault::fresh_reference(&scope)?,
        };
        let journal = doc.begin(&scope, snapshot.as_ref(), vec![reference.clone()])?;
        let key = key_for(
            vault,
            &reference,
            &scope,
            journal.candidate_digest().is_none(),
        )?;
        let prepared = if journal.candidate_digest().is_some() {
            let (prepared, bytes) = doc.resume_candidate(&journal)?;
            let (candidate_ref, candidate_rows) = decode(vault, &scope, &bytes)?;
            if candidate_ref != reference || (snapshot.is_some() && candidate_rows != rows) {
                return Err(Error::Integrity);
            }
            // With no old document the candidate is the only authenticated
            // recovery source. Never replace it with an empty in-memory queue.
            if snapshot.is_none() {
                match doc.publish(prepared, None, || true)? {
                    Published::Durable => (),
                    Published::DurabilityUnknown => return Err(Error::DurabilityUnknown),
                }
                self.reference = Some(reference);
                self.snapshot = doc.read()?;
                self.loaded = true;
                return Ok(candidate_rows);
            }
            prepared
        } else {
            let envelope = key.seal(&rows)?;
            let verified: Vec<Revocation> = key.open(&envelope)?;
            if verified != rows {
                return Err(Error::Integrity);
            }
            let bytes = zeroize::Zeroizing::new(
                serde_json::to_vec(&RevokeDocument {
                    version: 2,
                    envelope,
                })
                .map_err(|_| Error::Invalid)?,
            );
            doc.prepare(journal, &bytes)?
        };
        match doc.publish(prepared, snapshot.as_ref(), || true)? {
            Published::Durable => (),
            Published::DurabilityUnknown => return Err(Error::DurabilityUnknown),
        }
        self.reference = Some(reference);
        self.snapshot = doc.read()?;
        self.loaded = true;
        Ok(rows)
    }
    fn save(&mut self, vault: &Vault, path: &Path, rows: &[Revocation]) -> Result<(), Error> {
        if !self.loaded {
            return Err(Error::Conflict);
        }
        validate(rows)?;
        let scope = scope(path, self.snapshot.is_none())?;
        let doc = Document::open(path, LIMIT)?;
        // Keep the previously observed original file ID/digest. Reading a fresh
        // file here and overwriting it with stale memory loses another writer.
        if let Some(journal) = doc.pending(&scope)? {
            // Complete the exact earlier original transaction before processing
            // newer in-memory progress. Never reconstruct randomized ciphertext.
            let actual = doc.read()?;
            if actual
                .as_ref()
                .is_some_and(|s| journal.candidate_digest() == Some(s.digest()))
            {
                if journal.source_digest.as_deref() != self.snapshot.as_ref().map(Snapshot::digest)
                    || journal.source_identity.as_ref()
                        != self.snapshot.as_ref().map(Snapshot::physical_identity)
                {
                    return Err(Error::Conflict);
                }
                let (reference, _) = decode(
                    vault,
                    &scope,
                    actual.as_ref().ok_or(Error::Missing)?.bytes(),
                )?;
                doc.finish_authenticated(&journal, std::slice::from_ref(&reference))?;
                self.reference = Some(reference);
                self.snapshot = doc.read()?;
            } else if journal.candidate_digest().is_some() {
                let journal =
                    doc.begin(&scope, self.snapshot.as_ref(), journal.references.clone())?;
                let (prepared, bytes) = doc.resume_candidate(&journal)?;
                let (reference, _) = decode(vault, &scope, &bytes)?;
                if journal.references != vec![reference.clone()] {
                    return Err(Error::Integrity);
                }
                match doc.publish(prepared, self.snapshot.as_ref(), || true)? {
                    Published::Durable => (),
                    Published::DurabilityUnknown => return Err(Error::DurabilityUnknown),
                }
                self.reference = Some(reference);
                self.snapshot = doc.read()?;
            }
        }
        doc.check_original(self.snapshot.as_ref())?;
        let pending = doc.pending(&scope)?;
        let reference = match (&self.reference, &pending) {
            (Some(r), _) => r.clone(),
            (None, Some(j)) if j.references.len() == 1 => j.references[0].clone(),
            (None, Some(_)) => return Err(Error::Invalid),
            (None, None) => Vault::fresh_reference(&scope)?,
        };
        let journal = doc.begin(&scope, self.snapshot.as_ref(), vec![reference.clone()])?;
        let key = key_for(
            vault,
            &reference,
            &scope,
            self.reference.is_none() && journal.candidate_digest().is_none(),
        )?;
        let prepared = if journal.candidate_digest().is_some() {
            let (prepared, bytes) = doc.resume_candidate(&journal)?;
            let (candidate_ref, candidate_rows) = decode(vault, &scope, &bytes)?;
            if candidate_ref != reference || candidate_rows != rows {
                return Err(Error::Conflict);
            }
            prepared
        } else {
            let envelope = key.seal(&rows)?;
            let verified: Vec<Revocation> = key.open(&envelope)?;
            if verified != rows {
                return Err(Error::Integrity);
            }
            let bytes = zeroize::Zeroizing::new(
                serde_json::to_vec(&RevokeDocument {
                    version: 2,
                    envelope,
                })
                .map_err(|_| Error::Invalid)?,
            );
            doc.prepare(journal, &bytes)?
        };
        match doc.publish(prepared, self.snapshot.as_ref(), || true)? {
            Published::Durable => (),
            Published::DurabilityUnknown => return Err(Error::DurabilityUnknown),
        }
        // This runs inside the original worker, even if its observer canceled.
        self.snapshot = doc.read()?;
        self.reference = Some(reference);
        Ok(())
    }
}
impl CredentialIo {
    pub(super) async fn load_revocations(&self, path: PathBuf) -> Result<Vec<Revocation>, Error> {
        let revision = self.revocation_revision.clone();
        self.call(move |vault| {
            revision
                .lock()
                .map_err(|_| Error::Unavailable)?
                .load(&vault, &path)
        })
        .await
    }
    pub(super) async fn save_revocations(
        &self,
        path: PathBuf,
        rows: Vec<Revocation>,
    ) -> Result<(), Error> {
        let revision = self.revocation_revision.clone();
        self.call(move |vault| {
            revision
                .lock()
                .map_err(|_| Error::Unavailable)?
                .save(&vault, &path, &rows)
        })
        .await
    }
}

#[cfg(all(test, not(any(target_os = "ios", target_os = "android"))))]
mod recovery_tests {
    use super::*;
    use crate::console::server_login::credential_tests::Fake;
    fn rows() -> Vec<Revocation> {
        vec![Revocation {
            origin: "http://127.0.0.1:9/".into(),
            client_id: "client".into(),
            session_token: "S".repeat(64),
            oauth_token: "R".repeat(4096),
            hint: "refresh_token".into(),
            session_done: false,
            oauth_done: false,
        }]
    }
    #[test]
    fn credential_migration_revoke_recovery_authenticates_original_candidate_and_published_unknown()
    {
        for published in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("state");
            hagency_store::private::directory(&root).unwrap();
            let path = root.join("server-login-revocations.json");
            let rows = rows();
            hagency_store::private::write_new(&path, &serde_json::to_vec(&rows).unwrap()).unwrap();
            let vault = Vault::new(Arc::new(Fake::default()));
            let scope = scope(&path, true).unwrap();
            let doc = Document::open(&path, LIMIT).unwrap();
            let source = doc.read().unwrap().unwrap();
            let reference = Vault::fresh_reference(&scope).unwrap();
            let journal = doc
                .begin(&scope, Some(&source), vec![reference.clone()])
                .unwrap();
            let key = key_for(&vault, &reference, &scope, true).unwrap();
            let envelope = key.seal(&rows).unwrap();
            let candidate = serde_json::to_vec(&RevokeDocument {
                version: 2,
                envelope,
            })
            .unwrap();
            let _prepared = doc.prepare(journal, &candidate).unwrap();
            if published {
                // Actual rename, then simulate loss of the process before parent
                // sync/finish. Recovery still requires native key+AEAD+exact refs.
                let metadata: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(root.join(".server-login-revocations.json.vault-journal"))
                        .unwrap(),
                )
                .unwrap();
                std::fs::rename(root.join(metadata["candidate"].as_str().unwrap()), &path).unwrap();
            }
            drop(doc);
            drop(source);
            let mut revision = Revision::default();
            let recovered = revision.load(&vault, &path).unwrap();
            assert!(recovered == rows);
            assert!(
                !root
                    .join(".server-login-revocations.json.vault-journal")
                    .exists()
            );
            assert!(
                decode(&vault, &scope, &std::fs::read(&path).unwrap())
                    .unwrap()
                    .1
                    == rows
            );
        }
    }
    #[test]
    fn credential_migration_revoke_corrupt_candidate_keeps_plain_original_and_journal() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("state");
        hagency_store::private::directory(&root).unwrap();
        let path = root.join("server-login-revocations.json");
        let original = serde_json::to_vec(&rows()).unwrap();
        hagency_store::private::write_new(&path, &original).unwrap();
        let vault = Vault::new(Arc::new(Fake::default()));
        let scope = scope(&path, true).unwrap();
        let doc = Document::open(&path, LIMIT).unwrap();
        let source = doc.read().unwrap().unwrap();
        let reference = Vault::fresh_reference(&scope).unwrap();
        let journal = doc
            .begin(&scope, Some(&source), vec![reference.clone()])
            .unwrap();
        let key = key_for(&vault, &reference, &scope, true).unwrap();
        let bytes = serde_json::to_vec(&RevokeDocument {
            version: 2,
            envelope: key.seal(&rows()).unwrap(),
        })
        .unwrap();
        let _prepared = doc.prepare(journal, &bytes).unwrap();
        let journal_path = root.join(".server-login-revocations.json.vault-journal");
        let journal_bytes = std::fs::read(&journal_path).unwrap();
        let metadata: serde_json::Value = serde_json::from_slice(&journal_bytes).unwrap();
        let candidate_path = root.join(metadata["candidate"].as_str().unwrap());
        hagency_store::private::replace(&candidate_path, b"corrupt").unwrap();
        drop(doc);
        assert!(Revision::default().load(&vault, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(std::fs::read(journal_path).unwrap(), journal_bytes);
        assert_eq!(std::fs::read(candidate_path).unwrap(), b"corrupt");
    }
}
