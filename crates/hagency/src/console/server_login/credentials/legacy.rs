//! Compile-time mobile compatibility only. Desktop native errors never select
//! this writer; mobile native secret protection remains unqualified.
use super::{Error, Path, Revocation, validate};
use hagency_store::credential_vault::{
    Vault,
    document::{Document, Snapshot},
};

#[derive(Default)]
pub(super) struct Revision {
    loaded: bool,
    snapshot: Option<Snapshot>,
}
const LIMIT: usize = 1024 * 1024;
fn decode(bytes: &[u8]) -> Result<Vec<Revocation>, Error> {
    // A mobile legacy writer must not reinterpret a desktop envelope, unknown
    // schema or interrupted native migration as permission to downgrade it.
    let rows: Vec<Revocation> = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    validate(&rows)?;
    Ok(rows)
}
fn document(path: &Path) -> Result<Document, Error> {
    let parent = path.parent().ok_or(Error::Invalid)?;
    match std::fs::symlink_metadata(parent.join(".server-login-revocations.json.vault-journal")) {
        Ok(_) => return Err(Error::Invalid),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err(Error::Storage),
    }
    Document::open(path, LIMIT)
}
impl Revision {
    pub(super) fn load(&mut self, _vault: &Vault, path: &Path) -> Result<Vec<Revocation>, Error> {
        let doc = document(path)?;
        let snapshot = doc.read()?;
        let rows = snapshot
            .as_ref()
            .map(|s| decode(s.bytes()))
            .transpose()?
            .unwrap_or_default();
        self.snapshot = snapshot;
        self.loaded = true;
        Ok(rows)
    }
    pub(super) fn save(
        &mut self,
        _vault: &Vault,
        path: &Path,
        rows: &[Revocation],
    ) -> Result<(), Error> {
        if !self.loaded {
            return Err(Error::Conflict);
        }
        validate(rows)?;
        let doc = document(path)?;
        let actual = doc.read()?;
        if let Some(actual) = &actual {
            decode(actual.bytes())?;
        }
        doc.check_original(self.snapshot.as_ref())?;
        let bytes = zeroize::Zeroizing::new(serde_json::to_vec(rows).map_err(|_| Error::Invalid)?);
        if bytes.len() > LIMIT {
            return Err(Error::Invalid);
        }
        hagency_store::private::replace(path, &bytes).map_err(|_| Error::DurabilityUnknown)?;
        self.snapshot = doc.read()?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credential_migration_mobile_legacy_revoke_keeps_v1_and_refuses_v2_or_unknown_write() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("state");
        hagency_store::private::directory(&root).unwrap();
        let path = root.join("server-login-revocations.json");
        let vault = crate::console::server_login::credential_tests::vault();
        let rows = vec![Revocation {
            origin: "http://127.0.0.1:9/".into(),
            client_id: "client".into(),
            session_token: "S".repeat(64),
            oauth_token: "R".repeat(4096),
            hint: "refresh_token".into(),
            session_done: false,
            oauth_done: false,
        }];
        hagency_store::private::write_new(&path, &serde_json::to_vec(&rows).unwrap()).unwrap();
        let mut original = Revision::default();
        assert!(original.load(&vault, &path).unwrap() == rows);
        original.save(&vault, &path, &rows).unwrap();
        assert!(decode(&std::fs::read(&path).unwrap()).unwrap() == rows);
        for bytes in [
            b"{\"version\":2,\"envelope\":{}}".as_slice(),
            b"{\"version\":99}".as_slice(),
        ] {
            hagency_store::private::replace(&path, bytes).unwrap();
            assert!(original.save(&vault, &path, &rows).is_err());
            assert!(Revision::default().load(&vault, &path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
}
