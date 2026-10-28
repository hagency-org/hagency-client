mod common;
use hagency_store::{DomainRepository, Error, TaskFilters};
use serde_json::json;

/// The retained operator flow end to end (`lib/task-store.js`, board #23):
/// create, list with the retained filters, edit, comment, the ten-step
/// status walk, and delete — asserting the RETAINED user-visible outcome of
/// each (the field values, the ISO timestamps, the error word), not an
/// internal invariant.
#[test]
fn native_operator_task_lifecycle_matches_retained_store() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let mut db = DomainRepository::open(&root.path().join("state")).unwrap();

    // createTask: the defaults are p2/task/`created`, and the id is the
    // retained `task_<seconds>_<6>` spelling.
    let created = db
        .create_operator_task(
            &json!({"title":"Wire the board","assignee":"Octos","labels":["a","a","b"]}),
            1_700_000_000_000,
        )
        .unwrap();
    assert!(created.id.starts_with("task_1700000000_"), "{}", created.id);
    assert_eq!(created.status, "created");
    assert_eq!(created.priority, "p2");
    assert_eq!(created.granularity, "task");
    assert_eq!(created.assignee.as_deref(), Some("Octos"));
    assert_eq!(created.labels, ["a", "b"], "duplicates dropped");
    assert_eq!(created.created_at, "2023-11-14T22:13:20.000Z");
    assert_eq!(created.created_at, created.updated_at);
    assert!(created.started_at.is_none() && created.completed_at.is_none());
    assert!(created.comments.is_empty());

    // Title is required, and the refusal is the retained store's own word.
    assert!(matches!(
        db.create_operator_task(&json!({"title":"   "}), 1),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        db.create_operator_task(&json!({"title":"x","priority":"p9"}), 1),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        db.create_operator_task(&json!({"title":"x","granularity":"epics"}), 1),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        db.create_operator_task(&json!({"title":"x","parent_id":"task_missing"}), 1),
        Err(Error::Invalid(_))
    ));

    // A second task for the list filters, with a parent that exists.
    let child = db
        .create_operator_task(
            &json!({"title":"Ship it","assignee":"Aria","priority":"p1","parent_id":created.id}),
            1_700_000_001_000,
        )
        .unwrap();
    assert_eq!(child.parent_id.as_deref(), Some(created.id.as_str()));

    // listTasks: each retained filter, one at a time.
    let list = |filters: TaskFilters| db.operator_tasks(&filters).unwrap();
    assert_eq!(
        list(TaskFilters {
            assignee: Some("Octos".into()),
            ..TaskFilters::default()
        })
        .len(),
        1
    );
    assert_eq!(
        list(TaskFilters {
            status: Some("created".into()),
            ..TaskFilters::default()
        })
        .len(),
        2
    );
    assert_eq!(
        list(TaskFilters {
            priority: Some("p1".into()),
            ..TaskFilters::default()
        })
        .len(),
        1
    );
    assert_eq!(
        list(TaskFilters {
            label: Some("b".into()),
            ..TaskFilters::default()
        })
        .len(),
        1
    );
    assert_eq!(
        list(TaskFilters {
            status: Some("created,accepted".into()),
            ..TaskFilters::default()
        })
        .len(),
        2
    );
    // The page shape: offset/limit slice the same ordered list.
    assert_eq!(
        list(TaskFilters {
            offset: 1,
            limit: Some(1),
            ..TaskFilters::default()
        })
        .len(),
        1
    );
    // A page beyond the bound is refused, never clamped silently.
    assert!(
        db.operator_tasks(&TaskFilters {
            limit: Some(501),
            ..TaskFilters::default()
        })
        .is_err()
    );

    // updateTask: the operator's full-field edit; `status` is NOT editable
    // here (only `/transition` moves it), and updated_at moves.
    let edited = db
        .update_operator_task(
            &created.id,
            &json!({"title":"Wire the board properly","priority":"p0","assignee":null,
                    "labels":["x"],"description":"now with detail","status":"done"}),
        )
        .unwrap();
    assert_eq!(edited.title, "Wire the board properly");
    assert_eq!(edited.priority, "p0");
    assert!(edited.assignee.is_none(), "a null assignee clears it");
    assert_eq!(
        edited.labels,
        ["x"],
        "a non-array clears and a list replaces"
    );
    assert_eq!(edited.description, "now with detail");
    assert_eq!(edited.status, "created", "PATCH cannot move the status");
    assert_eq!(edited.created_at, "2023-11-14T22:13:20.000Z");
    // A non-string title is IGNORED rather than refused (normalizeText).
    let ignored = db
        .update_operator_task(&created.id, &json!({"title":7}))
        .unwrap();
    assert_eq!(ignored.title, "Wire the board properly");
    // A bad priority IS refused, and the write is rolled back.
    assert!(matches!(
        db.update_operator_task(&created.id, &json!({"priority":"p9"})),
        Err(Error::Invalid(_))
    ));
    assert_eq!(db.operator_task(&created.id).unwrap().priority, "p0");
    assert!(matches!(
        db.update_operator_task("task_missing", &json!({"title":"x"})),
        Err(Error::NotFound)
    ));

    // addComment: the retained entry shape and the author default.
    let commented = db
        .comment_operator_task(&created.id, &json!({"text":"started"}), 1_700_000_010_000)
        .unwrap();
    assert_eq!(commented.comments.len(), 1);
    assert_eq!(commented.comments[0].author, "anonymous");
    assert_eq!(commented.comments[0].text, "started");
    assert_eq!(commented.comments[0].ts, "2023-11-14T22:13:30.000Z");
    assert_eq!(commented.updated_at, commented.comments[0].ts);
    assert!(matches!(
        db.comment_operator_task(&created.id, &json!({"text":" "}), 1),
        Err(Error::Invalid(_))
    ));

    // transitionTask: the retained ten-step walk and its refusing pairs.
    assert!(matches!(
        db.transition_operator_task(&created.id, "in_progress", &json!({}), 2),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        db.transition_operator_task(&created.id, "nonsense", &json!({}), 2),
        Err(Error::Invalid(_))
    ));
    let accepted = db
        .transition_operator_task(&created.id, "accepted", &json!({}), 1_700_000_020_000)
        .unwrap();
    assert_eq!(accepted.status, "accepted");
    assert_eq!(
        accepted.started_at.as_deref(),
        Some("2023-11-14T22:13:40.000Z")
    );
    let running = db
        .transition_operator_task(&created.id, "in_progress", &json!({}), 1_700_000_030_000)
        .unwrap();
    assert_eq!(
        running.started_at, accepted.started_at,
        "started_at is kept"
    );
    // blocked requires both metadata fields, and refuses without them.
    assert!(matches!(
        db.transition_operator_task(&created.id, "blocked", &json!({}), 4),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        db.transition_operator_task(
            &created.id,
            "blocked",
            &json!({"waiting_reason":"needs a decision"}),
            4
        ),
        Err(Error::Invalid(_))
    ));
    let blocked = db
        .transition_operator_task(
            &created.id,
            "blocked",
            &json!({"waiting_reason":"needs a decision","waiting_until":"2026-01-01T00:00:00.000Z"}),
            1_700_000_040_000,
        )
        .unwrap();
    assert_eq!(blocked.waiting_reason.as_deref(), Some("needs a decision"));
    let resumed = db
        .transition_operator_task(&created.id, "in_progress", &json!({}), 1_700_000_050_000)
        .unwrap();
    assert!(
        resumed.waiting_reason.is_none() && resumed.waiting_until.is_none(),
        "in_progress clears the waiting metadata"
    );
    let done = db
        .transition_operator_task(&created.id, "done", &json!({}), 1_700_000_060_000)
        .unwrap();
    assert_eq!(done.status, "done");
    assert_eq!(
        done.completed_at.as_deref(),
        Some("2023-11-14T22:14:20.000Z")
    );
    assert!(matches!(
        db.transition_operator_task(&created.id, "in_progress", &json!({}), 7),
        Err(Error::Invalid(_))
    ));

    // deleteTask: the row and its comments go; a missing id is None.
    let removed = db.delete_operator_task(&created.id).unwrap().unwrap();
    assert_eq!(removed.id, created.id);
    assert_eq!(
        removed.comments.len(),
        1,
        "the reply carries the last state"
    );
    assert!(matches!(
        db.operator_task(&created.id),
        Err(Error::NotFound)
    ));
    assert!(db.delete_operator_task(&created.id).unwrap().is_none());

    // The project board: the retained envelope, with every column native
    // cannot source NAMED rather than zeroed.
    let board = db.operator_project_board(1_700_000_070_000, 20).unwrap();
    assert_eq!(board["generatedAt"], "2023-11-14T22:14:30.000Z");
    assert_eq!(board["staleAfterMs"], 300_000);
    assert_eq!(board["activityLimit"], 20);
    assert_eq!(
        board["totals"]["projects"], 0,
        "no project is registered here"
    );
    for status in ["created", "accepted", "in_progress", "blocked", "done"] {
        assert!(
            board["totals"]["tasks"][status].is_u64(),
            "every lane total is served: {status}"
        );
    }
    let unavailable = board["unavailable"].as_array().unwrap();
    for column in ["health", "repositories", "worktrees", "activity"] {
        assert!(
            unavailable.iter().any(|v| v == column),
            "{column} is named unavailable rather than zeroed"
        );
    }
    assert!(board["projects"].as_array().unwrap().is_empty());
}

/// The waiting metadata survives a restart: it is stored, not held in memory
/// the way the retained JSON-backed map holds it between saves.
#[test]
fn native_operator_task_blocked_metadata_is_durable() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = root.path().join("state");
    let id = {
        let mut db = DomainRepository::open(&state).unwrap();
        let task = db
            .create_operator_task(&json!({"title":"Durable"}), 1)
            .unwrap();
        db.transition_operator_task(&task.id, "accepted", &json!({}), 1_700_000_000_000)
            .unwrap();
        let blocked = db
            .transition_operator_task(&task.id, "in_progress", &json!({}), 1_700_000_001_000)
            .unwrap();
        db.transition_operator_task(
            &blocked.id,
            "blocked",
            &json!({"waiting_reason":"parked","waiting_until":"2026-02-01T00:00:00.000Z"}),
            1_700_000_002_000,
        )
        .unwrap();
        task.id
    };
    let db = DomainRepository::open(&state).unwrap();
    let task = db.operator_task(&id).unwrap();
    assert_eq!(task.status, "blocked");
    assert_eq!(task.waiting_reason.as_deref(), Some("parked"));
    assert_eq!(
        task.waiting_until.as_deref(),
        Some("2026-02-01T00:00:00.000Z")
    );
}
