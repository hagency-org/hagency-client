//! Explicit disposable credential backend for integration fixtures only.
//! No environment switch, native adapter, or production fallback is selected.
use hagency_store::credential_vault::{
    self as cv, Envelope, Purpose, Scope, SecretRef, SecretStore, Vault, document,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

#[derive(Default)]
struct FakeStore(Mutex<BTreeMap<String, Zeroizing<Vec<u8>>>>);
impl SecretStore for FakeStore {
    fn get(&self, reference: &SecretRef) -> Result<Zeroizing<Vec<u8>>, cv::Error> {
        self.0
            .lock()
            .unwrap()
            .get(&serde_json::to_string(reference).map_err(|_| cv::Error::Invalid)?)
            .cloned()
            .ok_or(cv::Error::Missing)
    }
    fn put(&self, reference: &SecretRef, bytes: &[u8]) -> Result<(), cv::Error> {
        self.0.lock().unwrap().insert(
            serde_json::to_string(reference).map_err(|_| cv::Error::Invalid)?,
            Zeroizing::new(bytes.to_vec()),
        );
        Ok(())
    }
    fn remove(&self, reference: &SecretRef) -> Result<(), cv::Error> {
        self.0
            .lock()
            .unwrap()
            .remove(&serde_json::to_string(reference).map_err(|_| cv::Error::Invalid)?)
            .map(|_| ())
            .ok_or(cv::Error::Missing)
    }
}

#[derive(Clone)]
pub struct CredentialFixture(Vault);
impl Default for CredentialFixture {
    fn default() -> Self {
        Self(Vault::new(Arc::new(FakeStore::default())))
    }
}
impl CredentialFixture {
    pub fn vault(&self) -> Vault {
        self.0.clone()
    }

    /// Read the actual persisted queue, verify its production scope and AEAD,
    /// then return the original rows for the existing HTTP/progress assertions.
    pub fn revocations(&self, path: &Path) -> Value {
        let bytes = hagency_store::private::read_secret(path).unwrap();
        let encoded = std::str::from_utf8(&bytes).unwrap();
        for secret in [
            "oauth-user-secret-0123456789",
            "oauth-refresh-secret",
            "rotated-oauth-secret",
            &"e".repeat(64),
        ] {
            assert!(
                !encoded.contains(secret),
                "revocation bytes must not contain fixture tokens"
            );
        }
        let stored: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(stored["version"], 2);
        let envelope: Envelope = serde_json::from_value(stored["envelope"].clone()).unwrap();
        let root = path.parent().unwrap();
        let installation = document::installation(root, false).unwrap();
        let identity = serde_json::to_vec(&(
            "standalone-server-revocations-v2",
            root.canonicalize().unwrap(),
        ))
        .unwrap();
        let scope = Scope::new(&installation, Purpose::Revocations, &identity).unwrap();
        let key = self.0.unlock(envelope.reference(), &scope).unwrap();
        let rows: Value = key.open(&envelope).unwrap();
        assert!(
            rows.is_array(),
            "the authenticated payload remains a revocation array"
        );
        rows
    }
}
