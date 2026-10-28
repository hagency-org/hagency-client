use super::*;

impl Widget for ProjectBoard {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.active {
            // Read-only design previews must still permit viewport interaction.
            self.view.handle_event(cx, event, scope);
            return;
        }
        if !self
            .connection()
            .is_ok_and(|s| s.identity() == self.identity)
        {
            self.clear_content(cx);
            self.pending = None;
            self.retained.clear();
            self.active = false;
            self.epoch = nonce();
            cx.stop_timer(self.timer);
            return;
        }
        match event {
            Event::WindowLostFocus(_) => {
                self.cancel_drag(cx);
                self.foreground = false;
                cx.stop_timer(self.timer);
            }
            Event::WindowGotFocus(_) => {
                self.foreground = true;
                if !self.loading {
                    self.load(cx);
                }
            }
            Event::KeyDown(e) if e.key_code == KeyCode::Escape => {
                self.close_calendar(cx);
                if self.pending.is_none() {
                    self.clear_quick_create(cx);
                }
                self.cancel_drag(cx);
                self.view
                    .label(cx, ids!(drag_notice))
                    .set_visible(cx, false);
                self.view.redraw(cx);
            }
            _ => (),
        }
        if self.timer.is_event(event).is_some() {
            self.timer = Timer::default();
            if !self.loading && !self.saving && self.drag.is_none() {
                if let Some(snapshot) = &self.snapshot {
                    self.read(
                        cx,
                        Read::Changes,
                        BoardAction::Changes {
                            after: snapshot.sequence,
                        },
                    );
                } else {
                    self.load(cx);
                }
            } else {
                self.schedule(cx);
            }
        }
        if let Some(frame) = self.drag_frame.is_event(event) {
            self.advance_landing(cx, frame.time);
        }
        if self.drag_frame.is_event(event).is_some()
            && let Some(drag) = self.drag.clone()
            && drag.moved
        {
            let viewport = self.view.view(cx, ids!(board_scroll)).area().rect(cx);
            let step = if drag.at.x < viewport.pos.x + 36.0 {
                -12.0
            } else if drag.at.x > viewport.pos.x + viewport.size.x - 36.0 {
                12.0
            } else {
                0.0
            };
            if step != 0.0 {
                let visible: Vec<_> = (0..self.column_count())
                    .filter(|i| self.view.view(cx, column_path(*i)).visible())
                    .collect();
                let actual = visible
                    .first()
                    .map(|i| {
                        viewport.pos.x - self.view.view(cx, column_path(*i)).area().rect(cx).pos.x
                    })
                    .unwrap_or(0.0);
                let extent = (visible.len() as f64 * 292.0 - 12.0 - viewport.size.x).max(0.0);
                self.scroll_x = (actual + step).clamp(0.0, extent);
                self.view
                    .view(cx, ids!(board_scroll))
                    .set_scroll_pos(cx, dvec2(self.scroll_x, 0.0));
            }
            for index in 0..self.column_count() {
                let column = self.view.view(cx, column_path(index));
                let list = column.portal_list(cx, ids!(cards));
                let rect = list.area().rect(cx);
                if rect.contains(drag.at) {
                    let step = if drag.at.y < rect.pos.y + 40.0 {
                        10.0
                    } else if drag.at.y > rect.pos.y + rect.size.y - 40.0 {
                        -10.0
                    } else {
                        0.0
                    };
                    if step != 0.0
                        && let Some(mut inner) = list.borrow_mut()
                    {
                        let first = inner.first_id();
                        let scroll = inner.first_scroll();
                        inner.set_first_id_and_scroll(first, scroll + step);
                        inner.redraw(cx);
                    }
                }
            }
            self.drag_frame = cx.new_next_frame();
            self.view.redraw(cx);
        }
        self.media_event(cx, event);
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        self.handle_extensions(cx, actions);
        self.handle_markdown(cx, actions);
        self.handle_calendar(cx, actions);
        self.handle_quick_create(cx, actions);
        self.handle_collaboration(cx, actions);
        self.media_actions(cx, actions);
        for action in actions {
            if let Some(done) = action.downcast_ref::<Completed>() {
                self.apply_read(cx, done);
            }
            if let Some(done) = action.downcast_ref::<Saved>() {
                self.apply_saved(cx, done);
            }
            if let Some(action) = action.as_widget_action() {
                if let Some(task) = self.card_targets.get(&action.widget_uid).cloned() {
                    match action.cast::<CardAction>() {
                        CardAction::Open => self.select(cx, &task),
                        CardAction::Start(abs)
                            if self.pending.is_none()
                                && !self.dirty
                                && self.landing.is_none()
                                && self.columns_draft.is_none()
                                && !self.query.archived =>
                        {
                            let source = self
                                .card_areas
                                .get(&task.id)
                                .map(|a| a.rect(cx))
                                .unwrap_or_default();
                            self.drag = Some(Drag {
                                source,
                                task,
                                start: abs,
                                at: abs,
                                moved: false,
                            });
                        }
                        CardAction::Drag(abs) => {
                            if let Some(drag) = &mut self.drag {
                                drag.at = abs;
                                drag.moved |= (abs - drag.start).length() > 6.0;
                                if drag.moved {
                                    self.drag_frame = cx.new_next_frame();
                                    self.view.label(cx, ids!(drag_notice)).set_visible(cx, true);
                                    self.view.redraw(cx);
                                }
                            }
                        }
                        CardAction::Drop(abs) => {
                            if let Some(drag) = self.drag.take()
                                && drag.moved
                            {
                                // PortalList reuses the source row for another task after a move.
                                cx.set_key_focus(Area::Empty);
                                let mut drag = drag;
                                drag.at = abs;
                                self.landing = Some(Landing {
                                    target: drag.source.pos,
                                    drag: drag.clone(),
                                    started: None,
                                    progress: 0.0,
                                    awaiting: false,
                                    layout_ready: false,
                                });
                                self.drag_frame = cx.new_next_frame();
                                if let Some(index) = (0..self.column_count()).find(|i| {
                                    self.view
                                        .view(cx, column_path(*i))
                                        .area()
                                        .rect(cx)
                                        .contains(abs)
                                }) {
                                    self.landing.as_mut().unwrap().awaiting = true;
                                    let filtered = self.query.query.is_some()
                                        || self.query.label.is_some()
                                        || self.query.assignee.is_some()
                                        || self.query.overdue;
                                    let complete = self
                                        .snapshot
                                        .as_ref()
                                        .is_some_and(|s| s.lanes[index].next_cursor.is_none());
                                    let target =
                                        self.drop_neighbours(cx, index, abs, &drag.task.id);
                                    if !filtered
                                        && complete
                                        && let Some((before, after)) = target
                                    {
                                        self.move_task(
                                            cx,
                                            drag.task,
                                            &self.column_state(index),
                                            Position::Between,
                                            before,
                                            after,
                                        );
                                    } else {
                                        self.move_task(
                                            cx,
                                            drag.task,
                                            &self.column_state(index),
                                            Position::Append,
                                            None,
                                            None,
                                        );
                                    }
                                }
                            }
                            self.view
                                .label(cx, ids!(drag_notice))
                                .set_visible(cx, false);
                            self.view.redraw(cx);
                        }
                        _ => (),
                    }
                }
                if self.more_targets.contains_key(&action.widget_uid) {
                    if let Some(index) = self.more_targets.get(&action.widget_uid).copied()
                        && matches!(action.cast::<ButtonAction>(), ButtonAction::Clicked(..))
                    {
                        self.load_more(cx, index);
                    }
                }
            }
        }
        for id in [
            ids!(title).as_slice(),
            ids!(description_editor.source),
            ids!(labels),
            ids!(due_date),
        ] {
            if self.view.text_input(cx, id).changed(actions).is_some() {
                self.dirty = true;
                self.discard_armed = false;
            }
        }
        if self
            .view
            .drop_down(cx, ids!(priority))
            .changed(actions)
            .is_some()
            || self
                .view
                .drop_down(cx, ids!(assignee))
                .changed(actions)
                .is_some()
        {
            self.dirty = true;
            self.discard_armed = false;
        }
        if self
            .view
            .drop_down(cx, ids!(new_status))
            .changed(actions)
            .is_some()
            && self.creating
        {
            self.draft_state =
                self.column_state(self.view.drop_down(cx, ids!(new_status)).selected_item());
            self.dirty = true;
            self.discard_armed = false;
        }
        if self.view.button(cx, ids!(clear_filters)).clicked(actions) && !self.loading {
            self.query = Query::default();
            self.view.text_input(cx, ids!(search)).set_text(cx, "");
            self.view
                .text_input(cx, ids!(filter_label))
                .set_text(cx, "");
            self.view
                .drop_down(cx, ids!(filter_state))
                .set_selected_item(cx, 0);
            self.view
                .drop_down(cx, ids!(filter_assignee))
                .set_selected_item(cx, 0);
            self.view
                .check_box(cx, ids!(overdue))
                .set_active(cx, false, Animate::No);
            self.view
                .check_box(cx, ids!(archived))
                .set_active(cx, false, Animate::No);
            self.reset_viewports(cx);
            self.load(cx);
        }
        if self.view.button(cx, ids!(refresh)).clicked(actions) {
            self.failed = false;
            self.load(cx);
        }
        if self.view.button(cx, ids!(retry)).clicked(actions) {
            self.retry_write(cx);
        }
        if self.view.button(cx, ids!(new_task)).clicked(actions) {
            self.new_task(cx);
        }
        if self.view.button(cx, ids!(apply_filters)).clicked(actions)
            || self
                .view
                .text_input(cx, ids!(search))
                .returned(actions)
                .is_some()
            || self
                .view
                .check_box(cx, ids!(overdue))
                .changed(actions)
                .is_some()
            || self
                .view
                .check_box(cx, ids!(archived))
                .changed(actions)
                .is_some()
        {
            if !self.loading {
                let search = self.view.text_input(cx, ids!(search)).text();
                let label = self.view.text_input(cx, ids!(filter_label)).text();
                let member = self
                    .view
                    .drop_down(cx, ids!(filter_assignee))
                    .selected_item();
                self.query = Query {
                    state: self
                        .view
                        .drop_down(cx, ids!(filter_state))
                        .selected_item()
                        .checked_sub(1)
                        .and_then(|i| {
                            self.snapshot
                                .as_ref()?
                                .lanes
                                .get(i)
                                .map(|l| l.state.clone())
                        }),
                    query: if search.trim().is_empty() {
                        None
                    } else {
                        Some(search.trim().into())
                    },
                    label: if label.trim().is_empty() {
                        None
                    } else {
                        Some(label.trim().into())
                    },
                    assignee: member
                        .checked_sub(1)
                        .and_then(|i| self.members.get(i).cloned()),
                    overdue: self.view.check_box(cx, ids!(overdue)).active(cx),
                    archived: self.view.check_box(cx, ids!(archived)).active(cx),
                    ..Default::default()
                };
                if self.query.validate().is_ok() {
                    self.reset_viewports(cx);
                    self.load(cx);
                } else {
                    self.notice(
                        cx,
                        tr("Keep searches under 120 characters and labels under 32 characters."),
                    );
                }
            }
        }
        if self.view.button(cx, ids!(close_detail)).clicked(actions) {
            if self.pending.is_some() {
                self.notice(cx,tr("A previous save is unconfirmed. Retry its original request to avoid duplicates."));
            } else if self.dirty && !self.discard_armed {
                self.discard_armed = true;
                self.notice(
                    cx,
                    tr("You have unsaved changes. Save them, or close again to discard the draft."),
                );
            } else {
                self.remember_comment(cx);
                self.clear_media(cx);
                self.selected = None;
                self.creating = false;
                self.dirty = false;
                self.discard_armed = false;
                self.detail_request = nonce();
                self.view.view(cx, ids!(inspector)).set_visible(cx, false);
                self.notice(cx, "");
            }
        }
        if self.view.button(cx, ids!(reload_latest)).clicked(actions)
            && let Some(task) = self.selected.clone()
        {
            self.dirty = false;
            self.conflict = false;
            self.select(cx, &task);
        }
        if self.view.button(cx, ids!(save)).clicked(actions) && self.pending.is_none() {
            let mut fields = self.fields(cx);
            if let Err(e) = fields.validate() {
                self.notice(cx, error_message(e.0));
            } else if self.creating {
                self.write(
                    cx,
                    BoardAction::Create {
                        input: Create {
                            idempotency_key: uuid::Uuid::new_v4().to_string(),
                            fields,
                            state: self.draft_state.clone(),
                        },
                    },
                );
            } else if let Some(task) = &self.selected {
                self.write(
                    cx,
                    BoardAction::Update {
                        task: task.id.clone(),
                        input: Update {
                            idempotency_key: uuid::Uuid::new_v4().to_string(),
                            expected_version: task.version,
                            fields,
                        },
                    },
                );
            }
        }
        if self.view.button(cx, ids!(move_top)).clicked(actions)
            || self.view.button(cx, ids!(move_bottom)).clicked(actions)
        {
            if self.dirty {
                self.notice(
                    cx,
                    tr("Save or close your current draft before opening another task."),
                );
            } else if let Some(task) = self.selected.clone() {
                let state =
                    self.column_state(self.view.drop_down(cx, ids!(move_state)).selected_item());
                let position = if self.view.button(cx, ids!(move_top)).clicked(actions) {
                    Position::Prepend
                } else {
                    Position::Append
                };
                self.move_task(cx, task, &state, position, None, None);
            }
        }
        if self.view.button(cx, ids!(archive)).clicked(actions)
            && let Some(task) = &self.selected
        {
            let input = Versioned {
                idempotency_key: uuid::Uuid::new_v4().to_string(),
                expected_version: task.version,
            };
            let action = if task.archived_at.is_some() {
                BoardAction::Restore {
                    task: task.id.clone(),
                    input,
                }
            } else {
                BoardAction::Archive {
                    task: task.id.clone(),
                    input,
                }
            };
            self.write(cx, action);
        }
        if self.view.button(cx, ids!(add_comment)).clicked(actions)
            && let Some(task) = &self.selected
        {
            let body = self.view.text_input(cx, ids!(comment_editor.source)).text();
            if !body.trim().is_empty() && body.len() <= 8192 {
                self.write(
                    cx,
                    BoardAction::Comment {
                        task: task.id.clone(),
                        input: Comment {
                            idempotency_key: uuid::Uuid::new_v4().to_string(),
                            body,
                        },
                    },
                );
            } else {
                self.notice(cx, tr("Comments must contain 1–8192 bytes."));
            }
        }
        if self.view.button(cx, ids!(more_comments)).clicked(actions)
            && let Some(task) = &self.selected
        {
            let id = task.id.clone();
            self.read(
                cx,
                Read::Comments(id.clone(), true),
                BoardAction::Comments {
                    task: id,
                    after: self.comments_after,
                },
            );
        }
        if self.view.button(cx, ids!(more_activity)).clicked(actions)
            && let Some(task) = &self.selected
        {
            let id = task.id.clone();
            self.read(
                cx,
                Read::Activity(id.clone(), true),
                BoardAction::Activity {
                    task: id,
                    after: self.activity_after,
                },
            );
        }
        if let Some(task) = &self.selected {
            let id = task.id.clone();
            if self.view.button(cx, ids!(first_comments)).clicked(actions) {
                self.read(
                    cx,
                    Read::Comments(id.clone(), false),
                    BoardAction::Comments {
                        task: id.clone(),
                        after: 0,
                    },
                );
            }
            if self.view.button(cx, ids!(first_activity)).clicked(actions) {
                self.read(
                    cx,
                    Read::Activity(id.clone(), false),
                    BoardAction::Activity { task: id, after: 0 },
                );
            }
        }
        self.sync_controls(cx);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_drag_preview(cx);
        self.card_targets.clear();
        self.card_areas.clear();
        self.more_targets.clear();

        let width = cx.turtle().rect().size.x;
        let inspector = self.view.view(cx, ids!(inspector)).visible();
        let layout_key = format!(
            "{:?}",
            (
                (
                    &self.snapshot,
                    &self.query,
                    width,
                    inspector,
                    self.dirty,
                    self.pending.is_some(),
                    self.creating
                ),
                (
                    &self.draft_checklist,
                    &self.subtasks,
                    &self.draft_parent,
                    &self.parent_task,
                    self.selected.as_ref().map(|t| (
                        t.id.as_str(),
                        t.subtask_done,
                        t.subtask_total
                    )),
                    self.drag.as_ref().map(|d| (d.at.x, d.at.y, d.moved)),
                    crate::i18n::language()
                )
            )
        );
        if self.layout_key != layout_key {
            self.layout_key = layout_key;
            self.view
                .view(cx, ids!(board_scroll))
                .set_visible(cx, !inspector || width >= 950.0);
            let mut panel = self.view.view(cx, ids!(inspector));
            let size = if width < 950.0 {
                Size::fill()
            } else {
                Size::Fixed(420.0)
            };
            script_apply_eval!(cx,panel,{width:#(size)});
            self.prepare_extension_draw(cx);
            for index in 0..12 {
                let lane = self.snapshot.as_ref().and_then(|s| s.lanes.get(index));
                let state = lane.map(|l| l.state.as_str()).unwrap_or("");
                let column = self.view.view(cx, column_path(index));
                column.set_visible(
                    cx,
                    self.query
                        .state
                        .as_ref()
                        .is_none_or(|filter| filter == state)
                        && lane.is_some(),
                );
                column.label(cx, ids!(heading)).set_text(
                    cx,
                    &lane.map(|l| column_title(&l.title)).unwrap_or_default(),
                );
                let mut marker = column.view(cx, ids!(marker));
                if lane.is_some_and(|l| l.is_done) {
                    script_apply_eval!(cx,marker,{draw_bg +: {color:mod.widgets.RINX_SUCCESS_FG}});
                } else {
                    match state {
                        "in_progress" => {
                            script_apply_eval!(cx,marker,{draw_bg +: {color:mod.widgets.RINX_ACCENT}});
                        }
                        "blocked" => {
                            script_apply_eval!(cx,marker,{draw_bg +: {color:mod.widgets.RINX_WARNING_FG}});
                        }
                        "in_review" => {
                            script_apply_eval!(cx,marker,{draw_bg +: {color:mod.widgets.RINX_INFO_FG}});
                        }
                        "todo" => {
                            script_apply_eval!(cx,marker,{draw_bg +: {color:mod.widgets.RINX_MUTED}});
                        }
                        _ => {
                            script_apply_eval!(cx,marker,{draw_bg +: {color:mod.widgets.RINX_ACCENT}});
                        }
                    }
                }

                column.button(cx, ids!(add)).set_board_enabled(
                    cx,
                    self.snapshot.is_some()
                        && self.pending.is_none()
                        && !self.dirty
                        && !self.query.archived,
                );
                let lane = self.snapshot.as_ref().and_then(|s| s.lanes.get(index));
                column.label(cx, ids!(count)).set_text(
                    cx,
                    &lane
                        .map(|l| {
                            l.wip_limit.map_or_else(
                                || l.total.to_string(),
                                |limit| format!("WIP {}/{}", l.task_count, limit),
                            )
                        })
                        .unwrap_or_else(|| "—".into()),
                );
                let full =
                    lane.is_some_and(|l| l.wip_limit.is_some_and(|limit| l.task_count >= limit));
                let mut count = column.label(cx, ids!(count));
                if full {
                    script_apply_eval!(cx,count,{draw_text.color:mod.widgets.RINX_WARNING_FG});
                } else {
                    script_apply_eval!(cx,count,{draw_text.color:mod.widgets.RINX_MUTED});
                }
                let mut column = column;
                let highlighted = self
                    .drag
                    .as_ref()
                    .is_some_and(|d| d.moved && column.area().rect(cx).contains(d.at));
                let blocked = highlighted
                    && self.drag.as_ref().is_some_and(|drag| {
                        lane.is_some_and(|lane| {
                            lane.state != drag.task.state
                                && lane.wip_limit.is_some_and(|limit| lane.task_count >= limit)
                        })
                    });
                if highlighted {
                    script_apply_eval!(cx,column,{draw_bg +: {color:mod.widgets.RINX_SELECTED border_size:2.0}});
                } else {
                    script_apply_eval!(cx,column,{draw_bg +: {color:mod.widgets.RINX_PANEL_BG border_size:0.0}});
                }
                if blocked {
                    script_apply_eval!(cx,column,{draw_bg.border_color:mod.widgets.RINX_WARNING_FG});
                } else {
                    script_apply_eval!(cx,column,{draw_bg.border_color:mod.widgets.RINX_ACCENT});
                }
            }
        }
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if self.draw_collaboration(cx, scope, &step) {
                continue;
            }
            if self.draw_extension_list(cx, scope, &step) {
                continue;
            }
            let index = (0..self.column_count()).find(|i| {
                step.widget_uid()
                    == self
                        .view
                        .view(cx, column_path(*i))
                        .portal_list(cx, ids!(cards))
                        .widget_uid()
            });
            let Some(index) = index else {
                continue;
            };
            let Some(mut list) = step.borrow_mut::<PortalList>() else {
                continue;
            };
            let lane = self.snapshot.as_ref().and_then(|s| s.lanes.get(index));
            let total = lane.map_or(0, |l| l.tasks.len());
            let more = lane.is_some_and(|l| l.next_cursor.is_some());
            if total == 0 || list.first_id() >= total {
                list.set_first_id_and_scroll(0, 0.0);
            }
            list.set_item_range(cx, 0, total.max(1) + usize::from(more));
            while let Some(row_index) = list.next_visible_item(cx) {
                if let Some(task) = lane.and_then(|l| l.tasks.get(row_index)) {
                    let card = list.item(cx, row_index, id!(Card));
                    self.card_targets.insert(card.widget_uid(), task.clone());
                    let lifted = self
                        .drag
                        .as_ref()
                        .is_some_and(|d| d.moved && d.task.id == task.id)
                        || self
                            .landing
                            .as_ref()
                            .is_some_and(|l| l.drag.task.id == task.id);
                    fill_card(
                        cx,
                        &card,
                        task,
                        lane.is_some_and(|l| l.is_done),
                        self.selected.as_ref().is_some_and(|t| t.id == task.id),
                        lifted,
                    );
                    card.draw_all(cx, scope);
                    self.card_areas.insert(task.id.clone(), card.area());
                } else if more && row_index == total {
                    let row = list.item(cx, row_index, id!(More));
                    self.more_targets
                        .insert(row.button(cx, ids!(more)).widget_uid(), index);
                    row.button(cx, ids!(more))
                        .set_board_enabled(cx, !self.loading);
                    row.draw_all(cx, scope);
                } else {
                    list.item(
                        cx,
                        row_index,
                        if row_index == 0 && self.snapshot.is_some() {
                            id!(Empty)
                        } else {
                            id!(Filler)
                        },
                    )
                    .draw_all(cx, scope);
                }
            }
        }
        self.resolve_landing_layout(cx);
        self.focus_quick_create(cx);
        DrawStep::done()
    }
}
impl ProjectBoard {
    pub(super) fn load_more(&mut self, cx: &mut Cx, index: usize) {
        if let Some(snapshot) = &self.snapshot
            && let Some(cursor) = snapshot.lanes[index].next_cursor.clone()
        {
            let mut query = self.query.clone();
            query.state = Some(snapshot.lanes[index].state.clone());
            query.sequence = Some(snapshot.sequence);
            query.cursor = Some(cursor);
            self.read(cx, Read::Page(index), BoardAction::Page { query });
        }
    }
    pub(super) fn drop_neighbours(
        &self,
        cx: &mut Cx,
        index: usize,
        abs: Vec2d,
        moving: &str,
    ) -> Option<(Option<String>, Option<String>)> {
        let lane = self.snapshot.as_ref()?.lanes.get(index)?;
        let tasks: Vec<_> = lane.tasks.iter().filter(|t| t.id != moving).collect();
        let next = tasks.iter().position(|t| {
            self.card_areas.get(&t.id).is_some_and(|area| {
                let rect = area.rect(cx);
                rect.size.y > 0.0 && abs.y < rect.pos.y + rect.size.y / 2.0
            })
        });
        match next {
            Some(i) => Some((
                Some(tasks[i].id.clone()),
                i.checked_sub(1).map(|j| tasks[j].id.clone()),
            )),
            None => {
                let viewport = self
                    .view
                    .view(cx, column_path(index))
                    .portal_list(cx, ids!(cards))
                    .area()
                    .rect(cx);
                let last = tasks
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| {
                        self.card_areas.get(&t.id).is_some_and(|area| {
                            let rect = area.rect(cx);
                            rect.size.y > 0.0
                                && rect.pos.y < viewport.pos.y + viewport.size.y
                                && rect.pos.y + rect.size.y > viewport.pos.y
                        })
                    })
                    .map(|(i, _)| i)
                    .max();
                last.map(|i| {
                    (
                        tasks.get(i + 1).map(|t| t.id.clone()),
                        Some(tasks[i].id.clone()),
                    )
                })
            }
        }
    }
}

pub(super) fn fill_card(
    cx: &mut Cx,
    card: &WidgetRef,
    task: &BoardTask,
    is_done: bool,
    selected: bool,
    lifted: bool,
) {
    if let Some(mut inner) = card.borrow_mut::<KanbanCard>() {
        let key = format!(
            "{task:?}:{is_done}:{selected}:{lifted}:{}:{:?}:{}",
            inner.focused,
            crate::i18n::language(),
            chrono::Utc::now().date_naive()
        );
        if inner.paint_key == key {
            return;
        }
        inner.paint_key = key;
    }
    for id in [
        ids!(number),
        ids!(priority),
        ids!(tags),
        ids!(assignee),
        ids!(due),
        ids!(grip),
    ] {
        let mut label = card.label(cx, id);
        script_apply_eval!(cx,label,{draw_text.color:mod.widgets.RINX_MUTED});
    }
    let mut title = card.label(cx, ids!(title));
    script_apply_eval!(cx,title,{draw_text.color:mod.widgets.RINX_INK});
    card.label(cx, ids!(number))
        .set_text(cx, &format!("#{}", task.number));
    card.label(cx, ids!(title)).set_text(cx, &task.title);
    card.label(cx, ids!(priority))
        .set_text(cx, priority_label(&task.priority));
    let mut tags = task.labels.clone();
    if task.attachment_count > 0 {
        tags.push(format!("📎 {}", task.attachment_count));
    }
    if task.comment_count > 0 {
        tags.push(format!("▤ {}", task.comment_count));
    }
    if task.checklist_total > 0 {
        tags.push(format!(
            "☑ {}/{}",
            task.checklist_done, task.checklist_total
        ));
    }
    if task.subtask_total > 0 {
        tags.push(format!("↳ {}/{}", task.subtask_done, task.subtask_total));
    }
    if let Some(n) = task.parent_number {
        tags.push(format!("↳ #{}", n));
    }
    card.label(cx, ids!(tags)).set_text(cx, &tags.join(" · "));
    card.label(cx, ids!(tags)).set_visible(cx, !tags.is_empty());
    card.label(cx, ids!(assignee)).set_text(
        cx,
        task.assignee_mxid
            .as_deref()
            .map(member_label)
            .unwrap_or_else(|| tr("Unassigned")),
    );
    let overdue = task
        .due_date
        .as_deref()
        .is_some_and(|date| date < chrono::Utc::now().format("%Y-%m-%d").to_string().as_str())
        && !is_done
        && task.archived_at.is_none();
    let mut due = card.label(cx, ids!(due));
    if overdue {
        script_apply_eval!(cx,due,{draw_text.color:mod.widgets.RINX_WARNING_FG});
        due.set_text(
            cx,
            &format!(
                "{} · {}",
                tr("Overdue"),
                task.due_date.as_deref().unwrap_or("")
            ),
        );
    } else {
        due.set_text(cx, task.due_date.as_deref().unwrap_or(""));
        script_apply_eval!(cx,due,{draw_text.color:mod.widgets.RINX_MUTED});
    }
    let mut priority = card.label(cx, ids!(priority));
    let mut priority_badge = card.view(cx, ids!(priority_badge));
    priority_badge.set_visible(cx, task.priority != "normal");
    if task.priority == "urgent" {
        script_apply_eval!(cx,priority,{draw_text.color:mod.widgets.RINX_WARNING_FG});
        script_apply_eval!(cx,priority_badge,{draw_bg.color:mod.widgets.RINX_SELECTED});
    } else if task.priority == "high" {
        script_apply_eval!(cx,priority,{draw_text.color:mod.widgets.RINX_ACCENT});
        script_apply_eval!(cx,priority_badge,{draw_bg.color:mod.widgets.RINX_SELECTED});
    } else {
        script_apply_eval!(cx,priority,{draw_text.color:mod.widgets.RINX_MUTED});
        script_apply_eval!(cx,priority_badge,{draw_bg.color:mod.widgets.RINX_PANEL_BG});
    }
    let mut card_bg = card.clone();

    if selected {
        script_apply_eval!(cx,card_bg,{draw_bg +: {border_color:mod.widgets.RINX_ACCENT}});
    } else {
        script_apply_eval!(cx,card_bg,{draw_bg +: {border_color:mod.widgets.RINX_BORDER}});
    }

    let mut bg = card.clone();
    script_apply_eval!(cx,bg,{draw_bg.lifted:#(if lifted {1.0} else {0.0})});
    if lifted {
        script_apply_eval!(cx,priority_badge,{draw_bg.color:#0000});
        for id in [
            ids!(number),
            ids!(title),
            ids!(priority),
            ids!(tags),
            ids!(assignee),
            ids!(due),
            ids!(grip),
        ] {
            let mut label = card.label(cx, id);
            script_apply_eval!(cx,label,{draw_text.color:#0000});
        }
    }
}
