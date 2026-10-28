use super::*;
use crate::management::{
    HostContext, Permission, PrivateManagementJournal, ResourceAlertDetailQuery, Session,
    TriageResourceAlert,
};

fn capture(f: &Fixture) -> Result<AdoptionInventory, Error> {
    let mut d = Connection::open_with_flags(
        f.source.join("domain.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut p = Connection::open_with_flags(&f.personal, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    AdoptionInventory::inspect_with_operator_history(
        &mut d,
        &mut p,
        &f.profile.digest,
        &f.profile.account,
    )
}
fn populate(f: &Fixture) {
    let db = DomainRepository::open_managed(&f.source, &f.profile.digest).unwrap();
    for (i, status) in crate::ALERT_STATUSES.iter().enumerate() {
        db.db.execute("INSERT INTO ceiling_alerts(dedupe_key,resource_id,summary,detail,runbook,impact,recovery_condition,occurrences,first_seen_ms,last_seen_ms,resolved_at_ms,resolved_by,status,note,transitioned_at_ms,transitioned_by,assignee,suppress_until_ms,tags,owner) VALUES(?1,?2,'  Original 告警  ',?3,'Runbook','Impact','Recovery',3,100,200,?4,?5,?6,'Original latest note',150,'original actor','original assignee',900,'[\"原始\",\"budget\"]','original owner')",params![format!("legacy:{status}"),f.resource.id(),if i==0 {"{truncated original detail"} else {"{\"ceilingTokens\":1000,\"credential\":\"private-sentinel\"}"},if *status=="resolved" {Some(200)} else {None},if *status=="resolved" {Some("system")} else {None},status]).unwrap();
    }
    for (key, seq, text) in [
        ("legacy:open", 3, "  first 原文  "),
        ("legacy:open", 9, "second\nline"),
        ("legacy:assigned", 7, "assigned history"),
    ] {
        db.db
            .execute(
                "INSERT INTO ceiling_alert_notes VALUES(?1,?2,'Historical actor',?3,175)",
                params![key, seq, text],
            )
            .unwrap();
    }
}
async fn prepare_alerts(f: &Fixture, id: &str) -> Result<AdoptionSummary, Error> {
    f.registry
        .prepare_resource_adoption(
            authority(f),
            id.into(),
            provider(),
            capture(f)?,
            "c".repeat(64),
            700,
        )
        .await
}
async fn transfer(f: &Fixture, id: &str) -> AdoptionTransfer {
    f.registry
        .resource_adoption_transfer(authority(f), id.into())
        .await
        .unwrap()
}
async fn import(f: &Fixture, t: &AdoptionTransfer) -> Result<AdoptionSummary, Error> {
    f.registry
        .import_resource_adoption(authority(f), archive_proof(f, t)?)
        .await
}
async fn fence(f: &Fixture, t: &AdoptionTransfer) {
    fenced_connections(f, t);
    f.registry
        .record_adoption_sources(authority(f), archive_proof(f, t).unwrap())
        .await
        .unwrap();
}
async fn activate(f: &Fixture, t: &AdoptionTransfer) -> AdoptedManagementStore {
    let j =
        PrivateManagementJournal::open_existing(&f.source, t.archive_binding().unwrap()).unwrap();
    f.registry
        .record_adoption_custody(authority(f), t.verify_private_journal(&j).await.unwrap())
        .await
        .unwrap();
    f.registry
        .activate_resource_adoption(
            authority(f),
            t.verify_private_journal(&j).await.unwrap(),
            provider(),
        )
        .await
        .unwrap();
    t.adopted_management(j).await.unwrap()
}
fn context(f: &Fixture) -> HostContext {
    HostContext::new(
        f.profile.account.clone(),
        1,
        "a".repeat(64),
        1,
        "alert_transfer_device".into(),
        1,
    )
    .unwrap()
}
fn session(b: AdoptedManagementStore, c: &HostContext) -> Session {
    Session::new_adopted(
        b,
        c.clone(),
        [
            Permission::ReadAlerts,
            Permission::TriageAlerts,
            Permission::ReadTasks,
        ],
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap()
}
fn detail(f: &Fixture) -> ResourceAlertDetailQuery {
    ResourceAlertDetailQuery {
        resource_id: f.resource.id(),
        id: "legacy:open".into(),
        after_note: 0,
    }
}

#[tokio::test]
async fn alert_transfer_v3_preserves_raw_rows_then_uses_canonical_history_after_restart() {
    let mut f = fixture().await;
    populate(&f);
    assert!(matches!(inventory(&f), Err(Error::Quarantined)));
    let original = capture(&f).unwrap().2.unwrap();
    let summary = prepare_alerts(&f, "original_alerts").await.unwrap();
    assert_eq!(summary.transferred_alert_count, Some(5));
    assert_eq!(summary.transferred_alert_note_count, Some(3));
    let t = transfer(&f, "original_alerts").await;
    assert_eq!(t.plan.version, 3);
    assert!(t.includes_operator_tasks());
    fence(&f, &t).await;
    assert_eq!(import(&f, &t).await.unwrap().phase, "imported_inactive");
    let expected = serde_json::to_value(&original).unwrap();
    let resources = vec![f.resource.clone()];
    f.registry
        .0
        .store
        .call(128 * 1024, move |db| {
            let h = alert_transfer::capture(
                &db.db,
                &resources,
                Instant::now() + Duration::from_secs(2),
            )?;
            assert_eq!(serde_json::to_value(h)?, expected);
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        import(&f, &t).await.unwrap().transferred_alert_note_count,
        Some(3)
    );
    let b = activate(&f, &t).await;
    let c = context(&f);
    let s = session(b.clone(), &c);
    let before = s.resource_alert(&c, detail(&f)).await.unwrap();
    assert_eq!(
        before.notes.iter().map(|n| n.sequence).collect::<Vec<_>>(),
        vec![3, 9]
    );
    assert!(
        !serde_json::to_string(&before)
            .unwrap()
            .contains("truncated original detail")
    );
    assert!(s.operator_task_page(&c, Default::default()).await.is_ok());
    let input = TriageResourceAlert {
        command_id: "after_migration".into(),
        resource_id: f.resource.id(),
        id: before.alert.id.clone(),
        expected_revision: before.alert.revision.clone(),
        to: "acknowledged".into(),
        note: Some("Current authenticated note".into()),
    };
    let first = s.triage_resource_alert(&c, input.clone()).await.unwrap();
    assert_eq!(
        s.triage_resource_alert(&c, input.clone()).await.unwrap(),
        first
    );
    assert_eq!(
        s.resource_alert(&c, detail(&f)).await.unwrap().notes.len(),
        3
    );
    s.retire();
    drop(s);
    b.shutdown().await.unwrap();
    drop(b);
    f.registry.shutdown().await.unwrap();
    f.registry = OwnerResourceRegistry::open_root(&f._root.path().join("owner")).unwrap();
    let t = transfer(&f, "original_alerts").await;
    assert_eq!(
        serde_json::to_value(t.plan.alert_history.as_ref().unwrap()).unwrap(),
        serde_json::to_value(original).unwrap()
    );
    let j =
        PrivateManagementJournal::open_existing(&f.source, t.archive_binding().unwrap()).unwrap();
    let b = t.adopted_management(j).await.unwrap();
    let s = session(b.clone(), &c);
    assert_eq!(s.triage_resource_alert(&c, input).await.unwrap(), first);
    assert_eq!(
        s.resource_alert(&c, detail(&f)).await.unwrap().notes.len(),
        3
    );
    b.shutdown().await.unwrap();
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn alert_transfer_v3_populated_tasks_alerts_and_resource_survive_canonical_restart(){
    use crate::management::{OperatorTaskQuery,OperatorTaskDetailQuery,ResourceQuery,WebAlertsQuery};
    let mut f=fixture().await;populate(&f);
    let (originals, original_resource)={
        let db=DomainRepository::open_managed(&f.source,&f.profile.digest).unwrap();
        for i in 0..21 {
            db.db.execute("INSERT INTO operator_tasks(id,title,description,status,priority,granularity,created_by,created_at,updated_at,labels) VALUES(?1,?2,'Preserved 原文',?3,'p1','subtask','Historical author',1000,3000,'[\"旧任务\"]')",params![format!("legacy_task_{i:02}"),format!("Retained {i} 标题"),crate::TASK_STATUSES[0]]).unwrap();
        }
        for i in 0..19 {db.db.execute("INSERT INTO operator_task_comments(sequence,task_id,author,body,created_at) VALUES(?1,'legacy_task_00','Historical commenter',?2,4000)",params![i+1,format!("Comment 原文 {i}")]).unwrap();}
        ((0..21).map(|i|db.operator_task(&format!("legacy_task_{i:02}")).unwrap()).collect::<Vec<_>>(),db.resource_configuration(&f.resource.id()).unwrap())
    };
    let reviewed=prepare_alerts(&f,"combined_populated").await.unwrap();
    assert_eq!((reviewed.transferred_task_count,reviewed.transferred_comment_count,reviewed.transferred_alert_count,reviewed.transferred_alert_note_count),(Some(21),Some(19),Some(5),Some(3)));
    let t=transfer(&f,"combined_populated").await;assert_eq!(t.plan.version,3);fence(&f,&t).await;import(&f,&t).await.unwrap();
    let mut b=activate(&f,&t).await;let c=context(&f);
    let make=|backing|Session::new_adopted(backing,c.clone(),[Permission::ReadTasks,Permission::ReadAlerts,Permission::ReadResources,Permission::ReadAccounts,Permission::ReadWebCatalog],Instant::now()+Duration::from_secs(60)).unwrap();
    let mut s=make(b.clone());
    for pass in 0..2 {
        let mut actual=std::collections::BTreeSet::new();let mut offset=0;
        loop {let page=s.operator_task_page(&c,OperatorTaskQuery{offset,..Default::default()}).await.unwrap();assert!(page.supported);assert!(page.tasks.len()<=16);for task in page.tasks{assert!(actual.insert(task.id));}match page.next_offset{Some(next)=>offset=next,None=>break}}
        assert_eq!(actual.len(),21);
        for original in &originals {
            let mut offset=0;let mut merged=None;
            loop {let mut page=s.operator_task_detail(&c,OperatorTaskDetailQuery{id:original.id.clone(),comment_offset:offset}).await.unwrap();assert_eq!(page.revision,Some(1));let next=page.next_comment_offset;let task=page.task.take().unwrap();if let Some(existing)=merged.as_mut(){let existing:&mut crate::OperatorTask=existing;existing.comments.extend(task.comments);}else{merged=Some(task)}match next{Some(next)=>offset=next,None=>break}}
            assert_eq!(merged.as_ref(),Some(original));
        }
        assert_eq!(s.web_alerts(&c,WebAlertsQuery::default()).await.unwrap().counts.total,5);
        let resources=s.resources(&c,ResourceQuery::default()).await.unwrap();assert_eq!(resources.resources.len(),1);let resource=&resources.resources[0];
        assert_eq!(resource.id,f.resource.id());assert_eq!(resource.model,f.resource.model);assert_eq!(resource.reasoning,f.resource.reasoning);assert_eq!(resource.published,f.resource.published);
        let id=f.resource.id();let preserved=f.registry.0.store.call(16*1024,move|db|db.resource_configuration(&id)).await.unwrap();assert_eq!(serde_json::to_value(preserved).unwrap(),serde_json::to_value(&original_resource).unwrap(),"adoption/restart preserves the complete original resource configuration");
        let offers=s.web_offers(&c,resource.id.clone()).await.unwrap();assert!(offers.offers_editable);assert_eq!(offers.offer_editing_reason,None);
        let wrong=HostContext::new("@another:test".into(),1,"a".repeat(64),1,"alert_transfer_device".into(),1).unwrap();assert!(s.operator_task_page(&wrong,Default::default()).await.is_err());
        if pass==0 {
            s.retire();drop(s);b.shutdown().await.unwrap();f.registry.shutdown().await.unwrap();f.registry=OwnerResourceRegistry::open_root(&f._root.path().join("owner")).unwrap();
            let t=transfer(&f,"combined_populated").await;let j=PrivateManagementJournal::open_existing(&f.source,t.archive_binding().unwrap()).unwrap();b=t.adopted_management(j).await.unwrap();s=make(b.clone());
        }
    }
    s.retire();drop(s);b.shutdown().await.unwrap();f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn alert_transfer_v3_changed_review_and_invalid_history_refuse_without_staging() {
    for sql in [
        "UPDATE ceiling_alerts SET resource_id='other_resource' WHERE dedupe_key='legacy:open'",
        "UPDATE ceiling_alerts SET first_seen_ms=-1 WHERE dedupe_key='legacy:open'",
        "UPDATE ceiling_alerts SET tags='{}' WHERE dedupe_key='legacy:open'",
        "UPDATE ceiling_alerts SET summary=printf('%020000d',0) WHERE dedupe_key='legacy:open'",
        "WITH RECURSIVE n(v) AS(SELECT 1 UNION ALL SELECT v+1 FROM n WHERE v<350) INSERT INTO ceiling_alerts(dedupe_key,resource_id,summary,detail,runbook,impact,recovery_condition,occurrences,first_seen_ms,last_seen_ms) SELECT 'large:'||v,(SELECT resource_id FROM ceiling_alerts LIMIT 1),printf('%0512d',0),'{}','r','i','c',1,1,1 FROM n",
        "PRAGMA foreign_keys=OFF; INSERT INTO ceiling_alert_notes VALUES('missing',1,'old actor','orphan',1)",
    ] {
        let f = fixture().await;
        populate(&f);
        Connection::open(f.source.join("domain.sqlite3"))
            .unwrap()
            .execute_batch(sql)
            .unwrap();
        assert!(capture(&f).is_err(), "accepted {sql}");
        assert_eq!(
            f.registry
                .0
                .store
                .call(1, |db| Ok(db.db.query_row(
                    "SELECT count(*) FROM owner_resource_adoptions",
                    [],
                    |r| r.get::<_, u64>(0)
                )?))
                .await
                .unwrap(),
            0
        );
        f.registry.shutdown().await.unwrap();
    }
    let f = fixture().await;
    populate(&f);
    prepare_alerts(&f, "reviewed").await.unwrap();
    let t = transfer(&f, "reviewed").await;
    Connection::open(f.source.join("domain.sqlite3")).unwrap().execute("UPDATE ceiling_alert_notes SET text='Changed after review' WHERE dedupe_key='legacy:open' AND seq=3",[]).unwrap();
    assert!(matches!(
        prepare_alerts(&f, "reviewed").await,
        Err(Error::Conflict)
    ));
    let mut d = Connection::open_with_flags(
        f.source.join("domain.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut p = Connection::open_with_flags(&f.personal, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert!(
        t.inspect_source_state(&mut d, &mut p, &"c".repeat(64))
            .is_err()
    );
    f.registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn alert_transfer_v3_collision_and_import_failure_preserve_atomicity() {
    for collision in [true, false] {
        let f = fixture().await;
        populate(&f);
        prepare_alerts(&f, "atomic").await.unwrap();
        let t = transfer(&f, "atomic").await;
        fence(&f, &t).await;
        f.registry.0.store.call(4096,move|db|{if collision {db.db.execute("INSERT INTO ceiling_alerts(dedupe_key,resource_id,summary,detail,runbook,impact,recovery_condition,occurrences,first_seen_ms,last_seen_ms) VALUES('legacy:open','another_resource','Untouched','{}','r','i','c',1,1,1)",[])?;} else {db.db.execute_batch("CREATE TRIGGER reject_alert_import BEFORE INSERT ON ceiling_alert_notes BEGIN SELECT RAISE(ABORT,'fixture failure');END;")?;} Ok(())}).await.unwrap();
        assert!(import(&f, &t).await.is_err());
        f.registry
            .0
            .store
            .call(1, move |db| {
                assert_eq!(
                    db.db
                        .query_row("SELECT count(*) FROM resources", [], |r| r.get::<_, u64>(0))?,
                    0
                );
                assert_eq!(
                    db.db
                        .query_row("SELECT count(*) FROM ceiling_alert_notes", [], |r| r
                            .get::<_, u64>(0))?,
                    0
                );
                assert_eq!(
                    db.db
                        .query_row("SELECT count(*) FROM ceiling_alerts", [], |r| r
                            .get::<_, u64>(0))?,
                    u64::from(collision)
                );
                Ok(())
            })
            .await
            .unwrap();
        f.registry.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn alert_transfer_v3_completed_command_custody_is_closed_and_preserved() {
    let f = fixture().await;
    populate(&f);
    let db = DomainRepository::open_managed(&f.source, &f.profile.digest).unwrap();
    let receipt = db
        .management_resource_alert(&detail(&f), |_| Ok(()))
        .unwrap()
        .alert;
    let raw = serde_json::to_string(&receipt).unwrap();
    db.db.execute("INSERT INTO management_commands(actor_scope,command_id,operation,digest,result) VALUES(?1,'original_triage','alert.triage',?2,?3)",params!["a".repeat(64),"b".repeat(64),raw]).unwrap();
    drop(db);
    let result = prepare_alerts(&f, "with_custody").await.unwrap();
    assert_eq!(result.completed_private_records, 2); // Original enrollment plus original triage.
    let t = transfer(&f, "with_custody").await;
    fence(&f, &t).await;
    import(&f, &t).await.unwrap();
    let b = activate(&f, &t).await;
    b.shutdown().await.unwrap();
    f.registry.shutdown().await.unwrap();
    let f = fixture().await;
    populate(&f);
    let db = DomainRepository::open_managed(&f.source, &f.profile.digest).unwrap();
    let mut raw = serde_json::to_value(receipt).unwrap();
    raw["unknownAuthority"] = json!(true);
    db.db.execute("INSERT INTO management_commands(actor_scope,command_id,operation,digest,result) VALUES(?1,'forged','alert.triage',?2,?3)",params!["a".repeat(64),"b".repeat(64),serde_json::to_string(&raw).unwrap()]).unwrap();
    drop(db);
    assert!(capture(&f).is_err());
    f.registry.shutdown().await.unwrap();
}
