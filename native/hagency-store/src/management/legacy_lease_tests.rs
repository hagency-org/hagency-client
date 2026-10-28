use super::*;
use crate::installation_authority::{AdoptionExclusiveLease, LegacyResourceLease};
use hagency_core::project::Resource;
use std::{future::Future, path::Path, pin::Pin};

fn expires() -> Instant {
    Instant::now() + Duration::from_secs(60)
}
fn exclusive(root: &Path) -> Result<AdoptionExclusiveLease, Error> {
    AdoptionExclusiveLease::isolated_for_tests(root, "a".repeat(64), expires(), || true)
}

// Exercise the real Session -> bounded worker -> SQLite transaction, with a
// second SQLite connection holding the writer lock. No mock queue or receipt.
async fn queued_case(operation: &str, retirement: &str, cancel_waiter: bool) {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("domain");
    let installation = root.path().join("installation");
    let mut db = DomainRepository::open(&state).unwrap();
    let resource: Resource = serde_json::from_value(serde_json::json!({
        "presetId":"lease_resource", "seatId":"lease_seat", "framework":"codex",
        "model":"gpt-5.6-sol", "reasoning":"high", "published":false
    }))
    .unwrap();
    db.put_resource(&resource).unwrap();
    let store = DomainStore::start(db, 8).unwrap();
    let context = HostContext::new(
        "@owner:test".into(),
        1,
        "b".repeat(64),
        1,
        "device".into(),
        1,
    )
    .unwrap();
    let session = Session::new(
        store.clone(),
        context.clone(),
        [
            Permission::ConfigureResources,
            Permission::PublishResources,
            Permission::EnrollResources,
            Permission::ReadResources,
        ],
        expires(),
    )
    .unwrap();
    let lease = LegacyResourceLease::isolated_for_tests(&installation, expires(), || true).unwrap();
    let inspect = Connection::open(state.join("domain.sqlite3")).unwrap();
    inspect.busy_timeout(Duration::ZERO).unwrap();
    inspect.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut ahead = Box::pin(store.put_resource(resource.clone()));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(ahead.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    let revision = resource_publication_revision(&resource).unwrap();
    let mut pending: Pin<Box<dyn Future<Output = Result<serde_json::Value, Error>>>> =
        match operation {
            "configure" => Box::pin(async {
                session
                    .configure_resource_with_legacy_lease(
                        &context,
                        ConfigureResource {
                            command_id: "original_configure".into(),
                            resource_id: resource.id(),
                            expected_revision: revision,
                            create: true,
                            profile: ProfileChange::Preserve {},
                            ceiling: CeilingChange::Monthly {
                                tokens: 100_u64.try_into().unwrap(),
                            },
                            engagement: None,
                        },
                        lease.clone(),
                    )
                    .await
                    .and_then(|value| Ok(serde_json::to_value(value)?))
            }),
            "publish" => Box::pin(async {
                session
                    .publish_resource_with_legacy_lease(
                        &context,
                        PublishResource {
                            command_id: "original_publish".into(),
                            resource_id: resource.id(),
                            expected_revision: revision,
                            published: true,
                        },
                        lease.clone(),
                    )
                    .await
                    .and_then(|value| Ok(serde_json::to_value(value)?))
            }),
            "enroll" => Box::pin(async {
                let input = EnrollResource {
                    command_id: "original_enroll".into(),
                    runtime: "codex".into(),
                    model_id: "sol".into(),
                    reasoning: Some("high".into()),
                    monthly_tokens: 100,
                };
                let proof = ProviderSelection::new(
                    context.clone(),
                    &input,
                    "fixture_account",
                    "gpt-5.6-sol".into(),
                    None,
                    Instant::now() + Duration::from_secs(30),
                )
                .unwrap();
                session
                    .enroll_resource_with_legacy_lease(&context, input, proof, lease.clone())
                    .await
                    .and_then(|value| Ok(serde_json::to_value(value)?))
            }),
            _ => unreachable!(),
        };
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(pending.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    match retirement {
        "lease" => lease.retire(),
        "session" => session.retire(),
        "none" => (),
        _ => unreachable!(),
    }
    if cancel_waiter {
        // A queued abandoned operation is skipped by the existing domain
        // worker. Its command must still retain exclusion until that dequeue;
        // cancellation is not evidence the closure has already been dropped.
        drop(pending);
        drop(lease);
        assert!(matches!(exclusive(&installation), Err(Error::Locked)));
        inspect.execute_batch("COMMIT").unwrap();
        ahead.await.unwrap();
    } else {
        assert!(matches!(exclusive(&installation), Err(Error::Locked)));
        inspect.execute_batch("COMMIT").unwrap();
        ahead.await.unwrap();
        let result = pending.await;
        if retirement == "none" {
            result.unwrap();
        } else {
            assert!(
                matches!(result, Err(Error::LocalAuthority)),
                "{operation} / {retirement}: {result:?}"
            );
        }
        drop(lease);
    }
    // FIFO read is a completion barrier for the original queued command.
    let _ = store
        .management_resources(ResourceQuery::default())
        .await
        .unwrap();
    exclusive(&installation).unwrap().check().unwrap();
    let commands: i64 = inspect
        .query_row("SELECT COUNT(*) FROM management_commands", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        commands,
        i64::from(retirement == "none" && !cancel_waiter),
        "{operation} / {retirement}"
    );
    let count: i64 = inspect
        .query_row("SELECT COUNT(*) FROM resources", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count,
        if retirement == "none" && !cancel_waiter && operation != "publish" {
            2
        } else {
            1
        }
    );
    let published = store
        .resource_configuration(resource.id())
        .await
        .unwrap()
        .published;
    assert_eq!(
        published,
        retirement == "none" && !cancel_waiter && operation == "publish"
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn retired_installation_lease_fences_all_three_queued_definition_writes() {
    for operation in ["configure", "publish", "enroll"] {
        queued_case(operation, "lease", false).await;
    }
}
#[tokio::test]
async fn installation_lease_does_not_replace_original_session_retirement() {
    for operation in ["configure", "publish", "enroll"] {
        queued_case(operation, "session", false).await;
    }
}
#[tokio::test]
async fn cancelled_waiter_retains_exclusion_until_original_command_is_dequeued() {
    for operation in ["configure", "publish", "enroll"] {
        queued_case(operation, "none", true).await;
    }
}

#[tokio::test]
async fn live_installation_lease_preserves_original_mutations_and_receipts() {
    for operation in ["configure", "publish", "enroll"] {
        queued_case(operation, "none", false).await;
    }
}
