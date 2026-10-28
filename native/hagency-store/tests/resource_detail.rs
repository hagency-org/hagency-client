use hagency_core::project::Resource;
use hagency_store::{DomainRepository, DomainStore, Error, management::*};
use serde_json::{Value, json};
use std::{
    future::Future,
    path::Path,
    time::{Duration, Instant},
};

fn context(account: &str, epoch: u64) -> HostContext {
    HostContext::new(account.into(), epoch, "a".repeat(64), 1, "device".into(), 1).unwrap()
}
fn session(store: &DomainStore, context: &HostContext, grants: &[Permission]) -> Session {
    Session::new(
        store.clone(),
        context.clone(),
        grants.iter().copied(),
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap()
}
fn resource(preset: &str, ceiling: Value) -> Resource {
    serde_json::from_value(json!({"presetId":preset,"seatId":"private_seat_reference",
        "framework":"codex","model":"gpt-5.6-sol","reasoning":"high",
        "provider":"private_provider_reference","ceiling":ceiling,"published":false}))
    .unwrap()
}
fn query(r: &Resource) -> ResourceDetailQuery {
    ResourceDetailQuery { id: r.id() }
}
fn bytes(path: &Path) -> (Vec<u8>, Option<Vec<u8>>) {
    (
        std::fs::read(path.join("domain.sqlite3")).unwrap(),
        std::fs::read(path.join("domain.sqlite3-wal")).ok(),
    )
}

#[tokio::test]
async fn resource_detail_projects_exact_native_snapshot_without_private_binding_or_writes() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let path = root.path().join("domain");
    let mut db = DomainRepository::open(&path).unwrap();
    let r = resource("public_resource", json!({"tokens":5000,"period":"monthly"}));
    db.put_resource(&r).unwrap();
    let store = DomainStore::start(db, 8).unwrap();
    let ctx = context("@owner:test", 1);
    let reader = session(&store, &ctx, &[Permission::ReadResources]);
    let before = bytes(&path);
    let detail = reader.resource_detail(&ctx, query(&r)).await.unwrap();
    assert_eq!(bytes(&path), before);
    assert_eq!(detail.protocol, PROTOCOL);
    assert!(detail.observed_at_ms > 0);
    assert_eq!(detail.resource.id, r.id());
    assert_eq!(
        detail.resource.revision,
        hagency_store::resource_publication_revision(&r).unwrap()
    );
    assert!(!detail.resource.published);
    assert!(!detail.editing.allowed);
    assert_eq!(
        detail.editing.reason,
        Some(ResourceEditBlocker::ContributionHistoryNotProven)
    );
    let expected = hagency_core::qualification::configuration_choices(&r.profile()).unwrap();
    assert_eq!(
        serde_json::to_value(&detail.choices).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    let encoded = serde_json::to_value(detail).unwrap();
    assert_eq!(
        encoded["ceiling"],
        json!({"tokens":5000,"period":"monthly"})
    );
    assert_eq!(encoded["resource"]["budget"]["pool"]["ceiling"], 5000);
    assert!(encoded["resource"]["usage"]["spent"].is_null());
    assert!(encoded["resource"]["usage"]["spend_period_key"].is_null());
    let text = encoded.to_string();
    for private in [
        "private_seat_reference",
        "private_provider_reference",
        "accountHome",
        "credential",
        "accessToken",
    ] {
        assert!(!text.contains(private), "private value leaked: {private}");
    }
    assert!(text.len() <= 64 * 1024);
    reader.retire();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn resource_detail_preserves_unknown_and_explicit_zero_limits() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&root.path().join("domain")).unwrap();
    let missing = resource("unknown_limit", Value::Null);
    let zero = resource("zero_limit", json!({"tokens":0,"period":"monthly"}));
    db.put_resource(&missing).unwrap();
    db.put_resource(&zero).unwrap();
    let store = DomainStore::start(db, 8).unwrap();
    let ctx = context("@owner:test", 1);
    let reader = session(&store, &ctx, &[Permission::ReadResources]);
    let unknown =
        serde_json::to_value(reader.resource_detail(&ctx, query(&missing)).await.unwrap()).unwrap();
    let explicit =
        serde_json::to_value(reader.resource_detail(&ctx, query(&zero)).await.unwrap()).unwrap();
    assert!(unknown["ceiling"].is_null());
    assert!(unknown["resource"]["budget"]["pool"]["ceiling"].is_null());
    assert!(unknown["resource"]["budget"]["remainingTokens"].is_null());
    assert_eq!(explicit["ceiling"]["tokens"], 0);
    assert_eq!(explicit["resource"]["budget"]["pool"]["ceiling"], 0);
    for value in [&unknown, &explicit] {
        assert!(value["resource"]["usage"]["spent"].is_null());
        assert_eq!(value["editing"]["allowed"], false);
    }
    reader.retire();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn resource_detail_requires_exact_scope_read_permission_and_current_session() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&root.path().join("domain")).unwrap();
    let r = resource("scope", Value::Null);
    db.put_resource(&r).unwrap();
    let store = DomainStore::start(db, 8).unwrap();
    let ctx = context("@owner:test", 1);
    let writer_only = session(&store, &ctx, &[Permission::ConfigureResources]);
    assert!(matches!(
        writer_only.resource_detail(&ctx, query(&r)).await,
        Err(Error::LocalAuthority)
    ));
    let reader = session(&store, &ctx, &[Permission::ReadResources]);
    for wrong in [context("@other:test", 1), context("@owner:test", 2)] {
        assert!(matches!(
            reader.resource_detail(&wrong, query(&r)).await,
            Err(Error::LocalAuthority)
        ));
    }
    assert!(matches!(
        reader
            .resource_detail(
                &ctx,
                ResourceDetailQuery {
                    id: "absent".into()
                }
            )
            .await,
        Err(Error::NotFound)
    ));
    reader.retire();
    assert!(matches!(
        reader.resource_detail(&ctx, query(&r)).await,
        Err(Error::LocalAuthority)
    ));
    writer_only.retire();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn resource_detail_rechecks_retirement_after_real_queued_read() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let path = root.path().join("domain");
    let mut db = DomainRepository::open(&path).unwrap();
    let r = resource("queued", Value::Null);
    db.put_resource(&r).unwrap();
    let store = DomainStore::start(db, 8).unwrap();
    let ctx = context("@owner:test", 1);
    let reader = session(&store, &ctx, &[Permission::ReadResources]);
    let lock = rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut ahead = Box::pin(store.put_resource(r.clone()));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(ahead.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    let mut reading = Box::pin(reader.resource_detail(&ctx, query(&r)));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(reading.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    reader.retire();
    lock.execute_batch("ROLLBACK").unwrap();
    ahead.await.unwrap();
    assert!(matches!(reading.await, Err(Error::LocalAuthority)));
    store.shutdown().await.unwrap();
}

#[test]
fn resource_detail_query_is_closed_and_bounded_before_host_initialization() {
    let valid = json!({"id":"resource_1"});
    serde_json::from_value::<ResourceDetailQuery>(valid.clone())
        .unwrap()
        .validate()
        .unwrap();
    for field in [
        "owner",
        "device",
        "path",
        "url",
        "provider",
        "canEdit",
        "create",
        "commandId",
    ] {
        let mut injected = valid.clone();
        injected[field] = json!("forged");
        assert!(serde_json::from_value::<ResourceDetailQuery>(injected).is_err());
    }
    for id in ["".into(), "x".repeat(129), "../resource".into()] {
        assert!(ResourceDetailQuery { id }.validate().is_err());
    }
}
