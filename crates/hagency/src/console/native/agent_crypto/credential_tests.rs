use super::*;
use crate::console::server_login::credential_tests::Fake;
use hagency_store::credential_vault::{self as cv, Vault};

async fn owner(root: &Path, vault: Vault) -> NativeOwner {
    NativeOwner::open_inner_with_credentials(
        root,
        "http://127.0.0.1:9",
        "@owner:example.test",
        false,
        vault,
    )
    .await
    .unwrap()
}
fn device() -> AuthorizedDevice {
    crate::console::server_login::fixture_device(
        "http://127.0.0.1:9/",
        "owner",
        "@owner:example.test",
    )
    .0
}
fn authority(owner: &NativeOwner, device: &AuthorizedDevice) -> AgentCryptoAuthority {
    AgentCryptoAuthority {
        owner: Arc::downgrade(&owner.0),
        pin: DeviceIdentity::from(device),
        retired: Arc::new(AtomicBool::new(false)),
        publication: owner
            .0
            .host
            .console
            .0
            .server_login
            .credential_publication
            .clone(),
    }
}
fn legacy_path(root: &Path, device: &AuthorizedDevice, agent: &str) -> PathBuf {
    let ledger = owned_agents::ledger_path(
        root,
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .unwrap();
    let profile = owned_agents::profile_identity(
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .unwrap();
    use sha2::{Digest, Sha256};
    let digest =
        Sha256::digest(serde_json::to_vec(&(&profile, agent, device.device_id())).unwrap());
    ledger
        .parent()
        .unwrap()
        .join("agent-crypto")
        .join(format!("agent_{digest:x}"))
        .join("credential.json")
}
fn seed(root: &Path, device: &AuthorizedDevice, agent: &str) -> PathBuf {
    let path = legacy_path(root, device, agent);
    hagency_store::private::directory(path.parent().unwrap().parent().unwrap()).unwrap();
    hagency_store::private::directory(path.parent().unwrap()).unwrap();
    let profile = owned_agents::profile_identity(
        device.origin(),
        device.issuer(),
        device.subject(),
        device.owner_mxid(),
    )
    .unwrap();
    let value = Credential {
        version: 1,
        profile,
        agent: agent.into(),
        device: device.device_id().into(),
        passphrase: "K".repeat(64),
    };
    hagency_store::private::write_new(&path, &serde_json::to_vec(&value).unwrap()).unwrap();
    path
}
#[tokio::test]
async fn credential_migration_agent_keeps_original_legacy_passphrase_across_auth_epochs_and_restart()
 {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let fake = Arc::new(Fake::default());
    let vault = Vault::new(fake.clone());
    let owner = owner(&state, vault.clone()).await;
    let d = device();
    let path = seed(&state, &d, "agent-a");
    let original = material(&owner, &d, "agent-a", 1, authority(&owner, &d))
        .await
        .unwrap();
    assert_eq!(original.passphrase(), "K".repeat(64));
    let bytes = std::fs::read(&path).unwrap();
    assert!(!bytes.windows(64).any(|s| s == "K".repeat(64).as_bytes()));
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["version"],
        2
    );
    let again = material(&owner, &d, "agent-a", 9, authority(&owner, &d))
        .await
        .unwrap();
    assert_eq!(again.passphrase(), original.passphrase());
    assert_eq!(again.root(), original.root());
    owner.shutdown_checked().await.unwrap();
    drop(original);
    drop(again);
    drop(owner);
    let reopened = self::owner(&state, vault).await;
    let recovered = material(&reopened, &d, "agent-a", 10, authority(&reopened, &d))
        .await
        .unwrap();
    assert_eq!(recovered.passphrase(), "K".repeat(64));
    assert_eq!(fake.puts.lock().unwrap().len(), 1);
    reopened.shutdown_checked().await.unwrap();
}
#[tokio::test]
async fn credential_migration_agent_locked_missing_key_and_namespace_preserve_original() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let fake = Arc::new(Fake::default());
    let vault = Vault::new(fake.clone());
    let owner = owner(&state, vault).await;
    let d = device();
    let path = seed(&state, &d, "agent-a");
    let original = std::fs::read(&path).unwrap();
    *fake.failure.lock().unwrap() = Some(cv::Error::Locked);
    assert!(
        material(&owner, &d, "agent-a", 1, authority(&owner, &d))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    *fake.failure.lock().unwrap() = None;
    let context = material(&owner, &d, "agent-a", 1, authority(&owner, &d))
        .await
        .unwrap();
    let v2 = std::fs::read(&path).unwrap();
    fake.values.lock().unwrap().clear();
    let put_count = fake.puts.lock().unwrap().len();
    assert!(
        material(&owner, &d, "agent-a", 2, authority(&owner, &d))
            .await
            .is_err()
    );
    assert_eq!(fake.puts.lock().unwrap().len(), put_count);
    assert_eq!(std::fs::read(&path).unwrap(), v2);
    std::fs::remove_file(state.join("credentials-v1/installation")).unwrap();
    assert!(
        material(&owner, &d, "agent-a", 2, authority(&owner, &d))
            .await
            .is_err()
    );
    assert!(!state.join("credentials-v1/installation").exists());
    assert!(context.root().exists());
    owner.shutdown_checked().await.unwrap();
}
#[tokio::test]
async fn credential_migration_agent_pending_candidate_recovers_exact_ref_and_rejects_retired_publication()
 {
    use hagency_store::credential_vault::SecretStore;
    struct Held {
        fake: Fake,
        entered: tokio::sync::Notify,
        gate: (StdMutex<bool>, std::sync::Condvar),
        first: AtomicBool,
    }
    impl SecretStore for Held {
        fn get(&self, r: &cv::SecretRef) -> Result<Zeroizing<Vec<u8>>, cv::Error> {
            self.fake.get(r)
        }
        fn put(&self, r: &cv::SecretRef, value: &[u8]) -> Result<(), cv::Error> {
            if !self.first.swap(true, Ordering::AcqRel) {
                self.entered.notify_one();
                let (lock, signal) = &self.gate;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = signal.wait(released).unwrap();
                }
            }
            self.fake.put(r, value)
        }
        fn remove(&self, r: &cv::SecretRef) -> Result<(), cv::Error> {
            self.fake.remove(r)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let held = Arc::new(Held {
        fake: Fake::default(),
        entered: Default::default(),
        gate: (StdMutex::new(false), Default::default()),
        first: AtomicBool::new(false),
    });
    let vault = Vault::new(held.clone());
    let owner = owner(&state, vault).await;
    let d = device();
    let path = seed(&state, &d, "agent-a");
    let original = std::fs::read(&path).unwrap();
    let runner = owner.clone();
    let next = d.clone();
    let auth = authority(&owner, &d);
    let task_auth = auth.clone();
    let task = tokio::spawn(async move { material(&runner, &next, "agent-a", 1, task_auth).await });
    held.entered.notified().await;
    auth.retire();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), async {
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_err()
    );
    {
        let (lock, signal) = &held.gate;
        *lock.lock().unwrap() = true;
        signal.notify_one();
    }
    assert!(task.await.unwrap().is_err());
    auth.finish_retire().await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(held.fake.puts.lock().unwrap().len(), 1);
    // The original journal/native reference can be recovered by a fresh live
    // authority; no replacement native secret is issued after retirement.
    let journal_path = path
        .parent()
        .unwrap()
        .join(".credential.json.vault-journal");
    let journal: Value = serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
    let candidate = journal["candidate"].as_str().unwrap();
    std::fs::remove_file(path.parent().unwrap().join(candidate)).unwrap();
    let recovered = material(&owner, &d, "agent-a", 2, authority(&owner, &d))
        .await
        .unwrap();
    assert_eq!(recovered.passphrase(), "K".repeat(64));
    assert_eq!(held.fake.puts.lock().unwrap().len(), 1);
    owner.shutdown_checked().await.unwrap();
}
