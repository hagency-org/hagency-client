use super::*;
use makepad_widgets::event::{ScrollEvent, ScrollPhase};
use serde_json::json;
use makepad_widgets::makepad_draw::cx_draw::CxDraw;
pub(super) fn task(id: &str, state: &str) -> BoardTask {
    serde_json::from_value(task_value(id, state)).unwrap()
}
fn task_value(id: &str, state: &str) -> Value {
    json!({"id":id,"projectId":"p1","number":1,"title":"Design a clear, shared project board","description":"Acceptance criteria and context.","state":state,"rank":1024,"priority":"high","assigneeMxid":"@alice:test","labels":["design","desktop"],"dueDate":"2026-11-01","version":1,"createdBy":"@alice:test","createdAt":1,"updatedAt":1,"archivedAt":null})
}

pub(super) fn panel() -> (Cx, ProjectBoard) {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let board = cx.with_vm(|vm| {
        makepad_widgets::script_mod(vm);
        crate::app::register_widgets(vm);
        vm.bx.captured_errors = Some(Vec::new());
        let value = script_eval!(vm,{mod.widgets.ProjectBoard {}});
        let board = ProjectBoard::script_from_value(vm, value);
        let errors = vm.take_errors();
        assert!(errors.is_empty(), "{errors:?}");
        board
    });
    (cx, board)
}
fn populate(board: &mut ProjectBoard) {
    board.identity = "account".into();
    board.project = "p1".into();
    board.epoch = 1;
    board.members = vec!["@alice:test".into(), "@bob:test".into()];
    board.snapshot = Some(Snapshot {
        project_id: "p1".into(),
        sequence: 1,
        capability: CAPABILITY.into(),
        members: board.members.clone(),
        lanes: STATES
            .iter()
            .map(|state| ::hagency::project_board::Lane {
                state: (*state).into(),
                title: (*state).into(),
                wip_limit: None,
                is_done: *state == "done",
                task_count: 1,
                total: 1,
                tasks: vec![task(&format!("task_{state}"), state)],
                next_cursor: None,
                sequence: 1,
            })
            .collect(),
    });
}
pub(super) fn draw(cx: &mut Cx, board: &mut ProjectBoard, width: f64, height: f64) {
    cx.redraw_all();
    let pass = DrawPass::new(cx);
    pass.set_size(cx, dvec2(width, height));
    let mut dl = DrawList2d::new(cx);
    let overlay = Overlay {
        draw_list: DrawList::new(cx),
    };
    let event = DrawEvent::default();
    let mut draw = CxDraw::new(cx, &event);
    let mut cx2d = Cx2d::new(&mut draw);
    cx2d.begin_pass(&pass, None);
    dl.begin_always(&mut cx2d);
    overlay.begin(&mut cx2d);
    cx2d.begin_root_turtle(dvec2(width, height), Layout::flow_overlay());
    board.draw_walk_all(&mut cx2d, &mut Scope::empty(), Walk::fixed(width, height));
    overlay.end(&mut cx2d);
    cx2d.end_pass_sized_turtle();
    dl.end(&mut cx2d);
    cx2d.end_pass(&pass);
}
#[test]
fn kanban_dialog_centers_in_the_viewport_without_resizing_background_columns() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    board.sync_members(&mut cx, board.members.clone());
    for width in [600.0, 1100.0, 1500.0] {
        board
            .view
            .view(&cx, ids!(inspector))
            .set_visible(&mut cx, false);
        draw(&mut cx, &mut board, width, 760.0);
        let rect = board.view.view(&cx, ids!(board_scroll)).area().rect(&cx);
        assert!(rect.size.y > 500.0, "{rect:?}");
        for index in 0..2 {
            let column = board.view.view(&cx, column_path(index));
            let rect = column.area().rect(&cx);
            assert!((rect.size.x - 280.0).abs() < 1.0, "{rect:?}");
            assert!(rect.size.y > 500.0);
        }
        let card = board
            .view
            .view(&cx, ids!(todo))
            .portal_list(&cx, ids!(cards))
            .item(&mut cx, 0, id!(Card));
        assert!(card.area().rect(&cx).size.y > 90.0);
        board
            .view
            .view(&cx, ids!(inspector))
            .set_visible(&mut cx, true);
        board.fill_editor(&mut cx, task("task_todo", "todo").fields());
        draw(&mut cx, &mut board, width, 760.0);
        let panel = board.view.view(&cx, ids!(inspector)).area().rect(&cx);
        assert!(panel.pos.x + panel.size.x <= width + 1.0, "{panel:?}");
        assert!(board.view.view(&cx, ids!(board_scroll)).visible());
        assert!((panel.pos.x + panel.size.x * 0.5 - width * 0.5).abs() < 1.0, "{panel:?}");
        assert!((panel.pos.y + panel.size.y * 0.5 - 380.0).abs() < 1.0, "{panel:?}");
        assert!(panel.size.x <= 760.0 && panel.size.y < 760.0, "{panel:?}");
        assert_eq!(board.view.view(&cx, ids!(board_scroll)).area().rect(&cx).size, rect.size);
        let scrim = board.draw_dialog_bg.area().rect(&cx);
        assert_eq!(scrim.pos, dvec2(0.0, 0.0));
        assert_eq!(scrim.size, dvec2(width, 760.0));
        assert!(board.draw_dialog_bg.color.w > 0.3, "the backdrop must visibly dim the application");
    }
}
#[test]
fn kanban_old_account_project_and_request_replies_cannot_replace_current_content() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    board.reads.insert(11);
    let done = Completed {
        identity: "old-account".into(),
        project: "p1".into(),
        epoch: 1,
        request: 11,
        kind: Read::Snapshot,
        detail_generation: 0,
        result: Err("project_membership_required".into()),
    };
    board.apply_read(&mut cx, &done);
    assert!(board.snapshot.is_some());
    assert!(board.reads.contains(&11));
    let done = Completed {
        identity: "account".into(),
        project: "another-project".into(),
        ..done
    };
    board.apply_read(&mut cx, &done);
    assert!(board.snapshot.is_some());
    let done = Completed {
        project: "p1".into(),
        epoch: 0,
        ..done
    };
    board.apply_read(&mut cx, &done);
    assert!(board.snapshot.is_some());
    let done = Completed { epoch: 1, ..done };
    board.apply_read(&mut cx, &done);
    assert!(board.snapshot.is_none());
    assert!(board.members.is_empty());
}
#[test]
fn kanban_conflicts_preserve_draft_and_unknown_saves_preserve_the_original_intent() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    board.selected = Some(task("task_one", "todo"));
    board.fill_editor(
        &mut cx,
        Fields {
            title: "My unsaved edit".into(),
            ..Default::default()
        },
    );
    board.dirty = true;
    let action = BoardAction::Update {
        task: "task_one".into(),
        input: Update {
            idempotency_key: "original-intent".into(),
            expected_version: 1,
            fields: board.fields(&mut cx),
        },
    };
    board.pending = Some(action.clone());
    board.save_request = 12;
    let done = Saved {
        identity: "account".into(),
        project: "p1".into(),
        epoch: 1,
        request: 12,
        result: Err("server_unavailable".into()),
    };
    board.apply_saved(&mut cx, &done);
    assert_eq!(
        serde_json::to_value(&board.pending).unwrap(),
        serde_json::to_value(Some(&action)).unwrap()
    );
    assert!(board.dirty);
    let conflict = Saved {
        result: Err("board_version_conflict".into()),
        ..done
    };
    board.apply_saved(&mut cx, &conflict);
    assert!(board.pending.is_none());
    assert!(board.conflict);
    assert_eq!(
        board.view.text_input(&cx, ids!(title)).text(),
        "My unsaved edit"
    );
}
#[test]
fn kanban_large_lanes_virtualize_cards_and_resolve_real_drop_neighbours() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    for (index, lane) in board
        .snapshot
        .as_mut()
        .unwrap()
        .lanes
        .iter_mut()
        .enumerate()
    {
        lane.tasks = (0..200)
            .map(|i| {
                let mut value = task(&format!("task_{index}_{i}"), STATES[index]);
                value.rank = (i + 1) * 1024;
                value.number = i + 1;
                value
            })
            .collect();
        lane.total = 200;
    }
    draw(&mut cx, &mut board, 1500.0, 760.0);
    assert!(
        board.card_targets.len() < 70,
        "only visible cards should exist, got {}",
        board.card_targets.len()
    );
    assert!(!board.card_areas.contains_key("task_0_199"));
    let second = board.card_areas["task_0_1"].rect(&cx);
    let neighbours = board
        .drop_neighbours(
            &mut cx,
            0,
            dvec2(second.pos.x + 20.0, second.pos.y + 5.0),
            "task_1_0",
        )
        .unwrap();
    assert_eq!(
        neighbours,
        (Some("task_0_1".into()), Some("task_0_0".into()))
    );
}

#[test]
fn kanban_project_switch_retains_only_original_unconfirmed_intents_and_resets_view_state() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    board.active = true;
    board.offline = true;
    board.failed = true;
    board.retry_delay = 60.0;
    board.pending = Some(BoardAction::Create {
        input: Create {
            idempotency_key: "unconfirmed".into(),
            fields: Fields {
                title: "Keep original".into(),
                ..Default::default()
            },
            state: "todo".into(),
        },
    });
    board.bind(&mut cx, "account", Some("p2"), true);
    assert!(board.active);
    assert!(board.snapshot.is_none());
    assert!(!board.offline && !board.failed);
    assert_eq!(board.retry_delay, 5.0);
    assert!(board.pending.is_none());
    assert!(board.retained.contains_key("p1"));
    board.bind(&mut cx, "account", Some("p1"), true);
    assert!(
        matches!(board.pending, Some(BoardAction::Create { ref input }) if input.idempotency_key == "unconfirmed")
    );
    board.bind(&mut cx, "different-account", Some("p1"), true);
    assert!(board.pending.is_none() && board.retained.is_empty());
}

#[test]
fn kanban_read_only_preview_still_scrolls_the_actual_native_viewports() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    let lane = &mut board.snapshot.as_mut().unwrap().lanes[0];
    lane.tasks = (0..30)
        .map(|i| task(&format!("task_{i}"), "todo"))
        .collect();
    lane.total = 30;
    board.active = false;
    draw(&mut cx, &mut board, 700.0, 760.0);
    let rect = board.view.view(&cx, ids!(todo)).area().rect(&cx);
    let list = board
        .view
        .view(&cx, ids!(todo))
        .portal_list(&cx, ids!(cards));
    let before = (
        list.borrow().unwrap().first_id(),
        list.borrow().unwrap().first_scroll(),
    );
    let scroll = |delta| {
        Event::Scroll(ScrollEvent {
            window_id: WindowId(0, 0),
            scroll: delta,
            abs: rect.pos + dvec2(100.0, 150.0),
            modifiers: KeyModifiers::default(),
            handled_x: std::cell::Cell::new(false),
            handled_y: std::cell::Cell::new(false),
            is_mouse: false,
            time: 1.0,
            phase: ScrollPhase::Changed,
        })
    };
    board.handle_event(&mut cx, &scroll(dvec2(0.0, 350.0)), &mut Scope::empty());
    draw(&mut cx, &mut board, 700.0, 760.0);
    let after = (
        list.borrow().unwrap().first_id(),
        list.borrow().unwrap().first_scroll(),
    );
    assert_ne!(
        before, after,
        "read-only means no mutations, not a frozen event tree"
    );
    board.handle_event(&mut cx, &scroll(dvec2(300.0, 0.0)), &mut Scope::empty());
    draw(&mut cx, &mut board, 700.0, 760.0);
    let after = board.view.view(&cx, ids!(todo)).area().rect(&cx);
    assert!(
        after.pos.x < rect.pos.x - 100.0,
        "horizontal viewport must scroll: {rect:?} -> {after:?}"
    );
}

#[test]
fn kanban_mouse_drag_can_start_from_the_card_body() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    draw(&mut cx, &mut board, 1100.0, 760.0);
    let card = board
        .view
        .view(&cx, ids!(todo))
        .portal_list(&cx, ids!(cards))
        .item(&mut cx, 0, id!(Card));
    let rect = card.area().rect(&cx);
    let point = rect.pos + dvec2(40.0, rect.size.y * 0.5);
    let actions = cx.capture_actions(|cx| {
        card.handle_event(
            cx,
            &Event::MouseDown(MouseDownEvent {
                abs: point,
                button: MouseButton::PRIMARY,
                window_id: WindowId(0, 0),
                modifiers: KeyModifiers::default(),
                handled: std::cell::Cell::new(Area::Empty),
                time: 1.0,
            }),
            &mut Scope::empty(),
        )
    });
    assert!(
        actions
            .iter()
            .filter_map(|a| a.as_widget_action())
            .any(|a| matches!(a.cast::<CardAction>(), CardAction::Start(_))),
        "the card body must capture mouse dragging, not only the tiny handle"
    );
}

#[test]
fn kanban_empty_filtered_lane_resets_an_old_scroll_anchor() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    let lane = &mut board.snapshot.as_mut().unwrap().lanes[0];
    lane.tasks = (0..30)
        .map(|i| task(&format!("task_{i}"), "todo"))
        .collect();
    lane.total = 30;
    draw(&mut cx, &mut board, 1100.0, 760.0);
    let list = board
        .view
        .view(&cx, ids!(todo))
        .portal_list(&cx, ids!(cards));
    list.set_first_id_and_scroll(25, -40.0);
    let lane = &mut board.snapshot.as_mut().unwrap().lanes[0];
    lane.tasks.clear();
    lane.total = 0;
    draw(&mut cx, &mut board, 1100.0, 760.0);
    assert_eq!(list.first_id(), 0);
    let empty = list.item(&mut cx, 0, id!(Empty));
    assert!(
        empty.area().rect(&cx).size.y > 50.0,
        "an empty filtered lane must show its empty state rather than an old scroll offset"
    );
}

#[test]
fn kanban_older_history_pages_cannot_replace_the_latest_page() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    board.selected = Some(task("task_one", "todo"));
    board.detail_request = 7;
    board.comments_request = 12;
    board.reads.extend([11, 12]);
    let old = Completed {
        identity: "account".into(),
        project: "p1".into(),
        epoch: 1,
        request: 11,
        kind: Read::Comments("task_one".into(), true),
        detail_generation: 7,
        result: Ok(
            json!({"items":[{"author":"@alice:test","body":"old page","createdAt":1}],"next":1,"hasMore":false}),
        ),
    };
    board.apply_read(&mut cx, &old);
    assert!(board.comments.is_empty());
    let current = Completed {
        request: 12,
        result: Ok(
            json!({"items":[{"author":"@bob:test","body":"current page","createdAt":2}],"next":2,"hasMore":false}),
        ),
        ..old
    };
    board.apply_read(&mut cx, &current);
    assert_eq!(board.comments[0]["body"], "current page");
}

#[test]
fn kanban_dynamic_columns_and_nested_item_drafts_survive_conflicts() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    let mut lane = board.snapshot.as_ref().unwrap().lanes[0].clone();
    lane.state = "verification".into();
    lane.title = "验收".into();
    lane.tasks = vec![task("task_custom", "verification")];
    lane.wip_limit = Some(1);
    lane.task_count = 1;
    board.snapshot.as_mut().unwrap().lanes.insert(0, lane);
    board.sync_column_labels(&mut cx);
    assert_eq!(board.column_state(0), "verification");
    let mut fields = task("child", "todo").fields();
    fields.parent_id = Some("parent".into());
    fields.checklist = vec![ChecklistItem {
        id: "one".into(),
        title: "Verify persistence".into(),
        checked: true,
    }];
    board.selected = Some(task("child", "todo"));
    board.detail_loaded = true;
    board.fill_editor(&mut cx, fields.clone());
    board.dirty = true;
    board
        .view
        .view(&cx, ids!(inspector))
        .set_visible(&mut cx, true);
    draw(&mut cx, &mut board, 1440.0, 900.0);
    let wire = board.fields(&mut cx);
    assert_eq!(wire.parent_id, fields.parent_id);
    assert_eq!(wire.checklist, fields.checklist);
    board.pending = Some(BoardAction::Update {
        task: "child".into(),
        input: Update {
            idempotency_key: "checklist-original".into(),
            expected_version: 1,
            fields: wire,
        },
    });
    board.save_request = 33;
    board.apply_saved(
        &mut cx,
        &Saved {
            identity: board.identity.clone(),
            project: board.project.clone(),
            epoch: board.epoch,
            request: 33,
            result: Err("board_version_conflict".into()),
        },
    );
    assert_eq!(board.draft_checklist, fields.checklist);
    assert_eq!(board.draft_parent, fields.parent_id);
    assert!(board.dirty && board.conflict);
    assert!(!uncertain("board_wip_limit_reached"));
}

#[test]
fn kanban_new_task_keeps_its_column_id_when_columns_are_reordered() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    board.new_task(&mut cx);
    assert_eq!(board.draft_state, "todo");
    board.snapshot.as_mut().unwrap().lanes.swap(0, 1);
    board.sync_column_labels(&mut cx);
    assert_eq!(board.draft_state, "todo");
    assert_eq!(
        board.view.drop_down(&cx, ids!(new_status)).selected_item(),
        1
    );
}

#[test]
fn kanban_malformed_child_payload_cannot_become_an_empty_success() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    let selected = task("parent", "todo");
    board.selected = Some(selected.clone());
    board.detail_request = 7;
    board.reads.insert(7);
    board.fill_editor(&mut cx, selected.fields());
    let raw = serde_json::json!({"task":{"id":"parent","projectId":"p1","number":1,"title":"Must not replace editor","state":"todo","rank":1024,"priority":"normal","labels":[],"version":2,"createdBy":"@alice:test","createdAt":1,"updatedAt":2},"parent":null,"subtasks":"invalid"});
    board.apply_read(
        &mut cx,
        &Completed {
            identity: "account".into(),
            project: "p1".into(),
            epoch: 1,
            request: 7,
            kind: Read::Detail("parent".into()),
            detail_generation: 7,
            result: Ok(raw),
        },
    );
    assert!(!board.detail_loaded);
    assert_eq!(board.fields(&mut cx).title, selected.title);
}

#[test]
fn kanban_lift_preserves_geometry_and_leaves_a_scrollbar_gutter() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    draw(&mut cx, &mut board, 1100.0, 760.0);
    let list = board
        .view
        .view(&cx, ids!(todo))
        .portal_list(&cx, ids!(cards));
    let card = list.item(&mut cx, 0, id!(Card));
    let rect = card.area().rect(&cx);
    let viewport = list.area().rect(&cx);
    assert!(viewport.pos.x + viewport.size.x - rect.pos.x - rect.size.x >= 13.0);
    let start = rect.pos + dvec2(70.0, 50.0);
    board.drag = Some(Drag {
        task: task("task_todo", "todo"),
        start,
        at: start + dvec2(280.0, 35.0),
        moved: true,
        source: rect,
    });
    draw(&mut cx, &mut board, 1100.0, 760.0);
    let floating = board.view.widget(&cx, ids!(drag_preview.content.floating));
    let ghost = floating.area().rect(&cx);
    assert!(
        (ghost.pos - rect.pos - dvec2(280.0, 35.0)).length() < 1.0,
        "{ghost:?} {rect:?}"
    );
    assert!(
        (ghost.size - rect.size).length() < 1.0,
        "full card dimensions must match: {ghost:?} {rect:?}"
    );
    assert_eq!(floating.label(&cx, ids!(assignee)).text(), "alice");
    assert_eq!(floating.label(&cx, ids!(tags)).text(), "design · desktop");
    assert_eq!(
        card.label(&cx, ids!(title))
            .borrow()
            .unwrap()
            .draw_text
            .color
            .w,
        0.0
    );
    assert!(
        floating
            .label(&cx, ids!(title))
            .borrow()
            .unwrap()
            .draw_text
            .color
            .w
            > 0.0
    );
    assert_eq!(
        board.card_areas["task_todo"].rect(&cx),
        rect,
        "lifting must not collapse the captured list row"
    );
    board.cancel_drag(&mut cx);
    draw(&mut cx, &mut board, 1100.0, 760.0);
    board.advance_landing(&mut cx, 1.0);
    board.advance_landing(&mut cx, 1.11);
    let middle = board.landing.as_ref().unwrap().position();
    assert!(middle.x > rect.pos.x && middle.x < ghost.pos.x);
    board.advance_landing(&mut cx, 1.23);
    draw(&mut cx, &mut board, 1100.0, 760.0);
    assert!(board.landing.is_none());
    assert!(
        card.label(&cx, ids!(title))
            .borrow()
            .unwrap()
            .draw_text
            .color
            .w
            > 0.0
    );
}

#[test]
fn kanban_drop_waits_for_ack_and_animates_to_the_authoritative_slot() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    draw(&mut cx, &mut board, 1100.0, 760.0);
    let source = board.card_areas["task_todo"].rect(&cx);
    let drag = Drag {
        task: task("task_todo", "todo"),
        source,
        start: source.pos + dvec2(30.0, 30.0),
        at: source.pos + dvec2(320.0, 70.0),
        moved: true,
    };
    board.landing = Some(Landing {
        drag,
        target: source.pos,
        awaiting: true,
        layout_ready: false,
        started: None,
        progress: 0.0,
    });
    board.advance_landing(&mut cx, 1.0);
    board.advance_landing(&mut cx, 3.0);
    assert!(board.landing.as_ref().unwrap().started.is_none());
    assert_eq!(board.snapshot.as_ref().unwrap().lanes[0].tasks.len(), 1);
    // Model the snapshot returned after server acknowledgement, including rank.
    let lanes = &mut board.snapshot.as_mut().unwrap().lanes;
    let mut moved = lanes[0].tasks.remove(0);
    moved.state = "in_progress".into();
    lanes[1].tasks.insert(0, moved);
    board.landing.as_mut().unwrap().awaiting = false;
    board.advance_landing(&mut cx, 3.9);
    assert!(
        board.landing.as_ref().unwrap().started.is_none(),
        "do not animate to the stale pre-reply geometry"
    );
    draw(&mut cx, &mut board, 1100.0, 760.0);
    let target = board.card_areas["task_todo"].rect(&cx).pos;
    board.advance_landing(&mut cx, 4.0);
    assert_eq!(board.landing.as_ref().unwrap().target, target);
    board.advance_landing(&mut cx, 4.23);
    assert!(board.landing.is_none());
    board.clear_content(&mut cx);
    assert!(board.drag.is_none() && board.landing.is_none());
}

#[test]
fn kanban_markdown_toolbar_preserves_source_selection_and_read_only_state() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    board.creating = true;
    board
        .view
        .view(&cx, ids!(inspector))
        .set_visible(&mut cx, true);
    board.fill_editor(
        &mut cx,
        Fields {
            description: "重点".into(),
            ..Default::default()
        },
    );
    let input = board.view.text_input(&cx, ids!(description_editor.source));
    let mut selection = input.selection();
    selection.anchor.index = 0;
    selection.cursor.index = 6;
    input.set_selection(&mut cx, selection);
    let click = |cx: &mut Cx, board: &ProjectBoard, path: &[LiveId]| {
        let uid = board.view.button(cx, path).widget_uid();
        cx.capture_actions(|cx| {
            cx.widget_action(uid, ButtonAction::Clicked(KeyModifiers::default()))
        })
    };
    let actions = click(&mut cx, &board, ids!(description_editor.tools.bold));
    board.handle_markdown(&mut cx, &actions);
    assert_eq!(input.text(), "**重点**");
    assert_eq!(input.selection().anchor.index, 2);
    assert_eq!(input.selection().cursor.index, 8);
    assert!(board.dirty);
    let actions = click(&mut cx, &board, ids!(description_editor.preview));
    board.handle_markdown(&mut cx, &actions);
    draw(&mut cx, &mut board, 1100.0, 760.0);
    assert!(board.description_preview);
    assert!(
        !board
            .view
            .view(&cx, ids!(description_editor.source_wrap))
            .visible()
    );
    assert!(
        !board
            .view
            .view(&cx, ids!(description_editor.tools))
            .visible()
    );
    assert_eq!(board.fields(&mut cx).description, "**重点**");
    assert!(
        board
            .view
            .widget(&cx, ids!(description_editor.rendered))
            .text()
            .contains("<strong>重点</strong>")
    );
    let editor = board
        .view
        .view(&cx, ids!(description_editor))
        .area()
        .rect(&cx);
    let preview = board
        .view
        .widget(&cx, ids!(description_editor.rendered))
        .area()
        .rect(&cx);
    assert!(
        preview.size.y > 0.0
            && preview.pos.x + preview.size.x <= editor.pos.x + editor.size.x + 1.0
    );
    board.conflict = true;
    board.sync_controls(&mut cx);
    let actions = click(&mut cx, &board, ids!(description_editor.tools.italic));
    board.handle_markdown(&mut cx, &actions);
    assert_eq!(
        input.text(),
        "**重点**",
        "conflict disables formatting just like typing"
    );
}

#[test]
fn kanban_priority_badges_and_loading_feedback_follow_visual_state() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    draw(&mut cx, &mut board, 1100.0, 760.0);
    let card = board
        .view
        .view(&cx, ids!(todo))
        .portal_list(&cx, ids!(cards))
        .item(&mut cx, 0, id!(Card));
    let mut task = task("task_todo", "todo");
    interaction::fill_card(&mut cx, &card, &task, false, false, false);
    assert!(card.view(&cx, ids!(priority_badge)).visible());

    task.priority = "normal".into();
    interaction::fill_card(&mut cx, &card, &task, false, false, false);
    assert!(!card.view(&cx, ids!(priority_badge)).visible());

    task.priority = "urgent".into();
    interaction::fill_card(&mut cx, &card, &task, false, false, false);
    assert!(card.view(&cx, ids!(priority_badge)).visible());
    interaction::fill_card(&mut cx, &card, &task, false, false, true);
    assert!(card.view(&cx, ids!(priority_badge)).visible());
    let badge = card.view(&cx, ids!(priority_badge));
    let mut color = [0.0; 4];
    badge
        .borrow()
        .unwrap()
        .draw_bg
        .get_instance(&mut cx, id!(color), &mut color);
    assert_eq!(color[3], 0.0, "the lifted source badge must be transparent");
    interaction::fill_card(&mut cx, &card, &task, false, false, false);
    badge
        .borrow()
        .unwrap()
        .draw_bg
        .get_instance(&mut cx, id!(color), &mut color);
    assert!(color[3] > 0.0, "the badge must return after the drag ends");

    board.loading = true;
    board.sync_controls(&mut cx);
    assert!(board.view.widget(&cx, ids!(board_loading)).visible());
    board.loading = false;
    board.sync_controls(&mut cx);
    assert!(!board.view.widget(&cx, ids!(board_loading)).visible());
}

#[test]
fn kanban_unsupported_server_is_not_mislabeled_offline() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    board.reads.insert(21);
    board.apply_read(
        &mut cx,
        &Completed {
            identity: "account".into(),
            project: "p1".into(),
            epoch: 1,
            request: 21,
            kind: Read::Snapshot,
            detail_generation: 0,
            result: Err("board_unsupported".into()),
        },
    );
    assert!(board.failed && board.snapshot.is_none());
    assert!(!board.offline);
    assert!(
        !board
            .view
            .label(&cx, ids!(sync_status))
            .text()
            .contains("Offline")
    );
}

#[test]
fn comment_drafts_restore_per_task_without_cross_card_leaks() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    let a = task("a", "todo");
    let b = task("b", "todo");
    board.select(&mut cx, &a);
    board
        .view
        .text_input(&cx, ids!(comment_editor.source))
        .set_text(&mut cx, "Draft for A");
    board.select(&mut cx, &b);
    assert_eq!(
        board
            .view
            .text_input(&cx, ids!(comment_editor.source))
            .text(),
        ""
    );
    board
        .view
        .text_input(&cx, ids!(comment_editor.source))
        .set_text(&mut cx, "Draft for B");
    board.select(&mut cx, &a);
    assert_eq!(
        board
            .view
            .text_input(&cx, ids!(comment_editor.source))
            .text(),
        "Draft for A"
    );
    board.select(&mut cx, &b);
    assert_eq!(
        board
            .view
            .text_input(&cx, ids!(comment_editor.source))
            .text(),
        "Draft for B"
    );
    board.remember_comment(&cx);
    board.project = "another_project".into();
    board.restore_comment(&mut cx, Some("a"));
    assert_eq!(
        board
            .view
            .text_input(&cx, ids!(comment_editor.source))
            .text(),
        ""
    );
}

#[test]
fn repeated_control_sync_preserves_markdown_hit_layout_and_native_selection() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    board.new_task(&mut cx);
    let input = board.view.text_input(&cx, ids!(description_editor.source));
    input.set_text(&mut cx, "Keep 需要删除 keep\nSecond line");
    draw(&mut cx, &mut board, 1200.0, 1800.0);
    input.set_selection(
        &mut cx,
        makepad_widgets::text::selection::Selection {
            anchor: makepad_widgets::text::selection::Cursor {
                index: 5,
                prefer_next_row: true,
            },
            cursor: makepad_widgets::text::selection::Cursor {
                index: 17,
                prefer_next_row: true,
            },
        },
    );
    // Hover/action batches resync controls without guaranteeing an intervening
    // draw. The read-only setter previously discarded the hit-test layout here.
    board.sync_controls(&mut cx);
    board.sync_controls(&mut cx);
    assert_eq!(input.selection().start().index, 5);
    assert_eq!(input.selection().end().index, 17);
    let mut native = input.borrow_mut().unwrap();
    assert!(native.move_cursor_line_end(&mut cx, true).is_ok());
    drop(native);
    input
        .replace_range(
            &mut cx,
            5..17,
            "",
            makepad_widgets::text_input::UndoGroup::New,
        )
        .unwrap();
    assert_eq!(input.text(), "Keep  keep\nSecond line");
    board.pending = Some(BoardAction::Create {
        input: Create {
            idempotency_key: "pending-markdown-lock".into(),
            fields: Fields {
                title: "Draft".into(),
                ..Default::default()
            },
            state: "todo".into(),
        },
    });
    board.sync_controls(&mut cx);
    assert!(input.is_read_only());
    assert!(
        input
            .replace_range(
                &mut cx,
                0..0,
                "blocked",
                makepad_widgets::text_input::UndoGroup::New
            )
            .is_err()
    );
}

#[test]
fn quick_card_retries_original_intent_and_does_not_replace_open_detail() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    board.selected = Some(task("original", "todo"));
    board.detail_loaded = true;
    let column = board.view.view(&cx, ids!(in_progress));
    let click = |cx: &mut Cx, id: &[LiveId]| {
        let uid = column.button(cx, id).widget_uid();
        cx.capture_actions(|cx| {
            cx.widget_action(uid, ButtonAction::Clicked(KeyModifiers::default()))
        })
    };
    let actions = click(&mut cx, ids!(quick_add));
    board.handle_quick_create(&mut cx, &actions);
    assert_eq!(board.quick_create_state.as_deref(), Some("in_progress"));
    column
        .text_input(&cx, ids!(quick_title))
        .set_text(&mut cx, "Only a title");
    let actions = click(&mut cx, ids!(quick_save));
    board.handle_quick_create(&mut cx, &actions);
    let original = serde_json::to_value(&board.pending).unwrap();
    assert!(board.is_quick_save());
    assert_eq!(original["input"]["state"], "in_progress");
    let done = Saved {
        identity: "account".into(),
        project: "p1".into(),
        epoch: 1,
        request: 0,
        result: Err("server_unavailable".into()),
    };
    board.apply_saved(&mut cx, &done);
    assert_eq!(serde_json::to_value(&board.pending).unwrap(), original);
    assert_eq!(
        column.text_input(&cx, ids!(quick_title)).text(),
        "Only a title"
    );
    board.apply_saved(
        &mut cx,
        &Saved {
            result: Ok(json!({"task":task_value("new", "in_progress")})),
            ..done
        },
    );
    assert!(board.pending.is_none() && board.quick_create_state.is_none());
    assert_eq!(board.selected.as_ref().unwrap().id, "original");
    assert_eq!(column.text_input(&cx, ids!(quick_title)).text(), "");
    assert!(!column.view(&cx, ids!(quick_form)).visible());
}

#[test]
fn activity_describes_assignment_and_attachment_changes() {
    let text = collaboration::activity_text(
        &json!({"actor":"@alice:test","kind":"edited","details":{"fields":[{"field":"assigneeMxid","before":null,"after":"@bob:test"}]}}),
    );
    assert!(text.contains("bob") && text.contains('→'));
    let text = collaboration::activity_text(
        &json!({"kind":"attachment_added","details":{"filename":"design.png"}}),
    );
    assert!(text.contains("design.png"));
}

#[test]
fn late_attachment_and_notification_navigation_cannot_reopen_another_task() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    board.select(&mut cx, &task("b", "todo"));
    board.detail_request = 9;
    board.attachment_request = 12;
    board.open_request = 13;
    for (request, kind) in [
        (11, Read::Attachment("a".into(), false)),
        (13, Read::OpenTask("a".into())),
    ] {
        board.reads.insert(request);
        board.apply_read(
            &mut cx,
            &Completed {
                identity: "account".into(),
                project: "p1".into(),
                epoch: 1,
                request,
                kind,
                detail_generation: 8,
                result: Ok(json!({"task":task_value("a","todo")})),
            },
        );
        assert_eq!(board.selected.as_ref().unwrap().id, "b");
    }
    assert_eq!(
        collaboration::member_choice("@alice:test", &["@alice:test".into()]),
        "alice"
    );
    assert_eq!(
        collaboration::member_choice(
            "@alice:test",
            &["@alice:test".into(), "@alice:elsewhere".into()]
        ),
        "@alice:test"
    );
}

#[test]
fn unchanged_overdue_card_does_not_schedule_another_frame() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    let task = &mut board.snapshot.as_mut().unwrap().lanes[0].tasks[0];
    task.due_date = Some("2000-01-01".into());
    let task = task.clone();
    draw(&mut cx, &mut board, 1500.0, 760.0);
    let card = board
        .view
        .view(&cx, column_path(0))
        .portal_list(&cx, ids!(cards))
        .item(&mut cx, 0, id!(Card));
    interaction::fill_card(&mut cx, &card, &task, false, false, false);
    cx.new_draw_event = DrawEvent::default();
    interaction::fill_card(&mut cx, &card, &task, false, false, false);
    assert!(
        !cx.new_draw_event.redraw_all && cx.new_draw_event.draw_lists.is_empty(),
        "An unchanged card must not redraw itself forever"
    );
}

#[test]
fn idle_board_has_no_drag_overlay_redraw_loop() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    draw(&mut cx, &mut board, 1500.0, 760.0);
    cx.new_draw_event = DrawEvent::default();
    let event = DrawEvent::default();
    let mut draw = CxDraw::new(&mut cx, &event);
    let mut cx2d = Cx2d::new(&mut draw);
    board.draw_drag_preview(&mut cx2d);
    assert!(!cx2d.new_draw_event.redraw_all && cx2d.new_draw_event.draw_lists.is_empty());
}

#[test]
fn same_task_history_refresh_preserves_content_and_selected_preview() {
    let (mut cx, mut board) = panel();
    populate(&mut board);
    let current = task("a", "todo");
    board.selected = Some(current.clone());
    board.detail_loaded = true;
    board.detail_request = 7;
    board.reads.insert(7);
    board.comment_preview = true;
    board.comments = vec![json!({"body":"Keep this visible"})];
    board
        .view
        .widget(&cx, ids!(comments))
        .set_text(&mut cx, "Keep this visible");
    board.apply_read(
        &mut cx,
        &Completed {
            identity: "account".into(),
            project: "p1".into(),
            epoch: 1,
            request: 7,
            kind: Read::Detail("a".into()),
            detail_generation: 7,
            result: Ok(json!({"task":task_value("a", "todo"),"subtasks":[],"parent":null,"attachments":[]})),
        },
    );
    assert!(board.comment_preview);
    assert_eq!(board.comments[0]["body"], "Keep this visible");
    assert_eq!(
        board.view.widget(&cx, ids!(comments)).text(),
        "Keep this visible"
    );
    board.select(&mut cx, &task("b", "todo"));
    assert!(board.comments.is_empty());
    assert_ne!(
        board.view.widget(&cx, ids!(comments)).text(),
        "Keep this visible"
    );
}
