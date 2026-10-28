use super::*;

impl ProjectBoard {
    pub(super) fn notice(&mut self, cx: &mut Cx, message: &str) {
        self.view
            .label(cx, ids!(dialog_notice))
            .set_text(cx, message);
        self.view
            .label(cx, ids!(dialog_notice))
            .set_visible(cx, !message.is_empty());
        self.view.label(cx, ids!(notice)).set_text(cx, message);
        self.view.label(cx, ids!(notice)).set_visible(
            cx,
            !message.is_empty() && !self.view.view(cx, ids!(inspector)).visible(),
        );
        self.view.redraw(cx);
    }
    pub(super) fn clear_content(&mut self, cx: &mut Cx) {
        self.close_calendar(cx);
        self.clear_quick_create(cx);
        self.subtask_after_save = None;
        self.clear_media(cx);
        self.layout_key.clear();
        self.notifications.clear();
        self.notification_targets.clear();
        self.attachment_targets.clear();
        self.notification_before = 0;
        self.notification_latest = 0;
        self.view
            .view(cx, ids!(notifications_panel))
            .set_visible(cx, false);
        self.view
            .button(cx, ids!(notifications_button))
            .set_text(cx, tr("Notifications"));
        self.columns_draft = None;
        self.draft_checklist.clear();
        self.draft_parent = None;
        self.subtasks.clear();
        self.parent_task = None;
        self.view
            .view(cx, ids!(column_settings))
            .set_visible(cx, false);
        self.snapshot = None;
        self.detail_loaded = false;
        self.selected = None;
        self.creating = false;
        self.dirty = false;
        self.conflict = false;
        self.attachments.clear();
        self.view
            .widget(cx, ids!(attachment_preview))
            .set_visible(cx, false);
        self.comments.clear();
        self.members.clear();
        self.member_names.clear();
        self.card_targets.clear();
        self.more_targets.clear();
        self.drag = None;
        self.column_drag = None;
        self.landing = None;
        for id in [
            ids!(title).as_slice(),
            ids!(description_editor.source),
            ids!(labels),
            ids!(due_date),
            ids!(comment_editor.source),
        ] {
            self.view.text_input(cx, id).set_text(cx, "");
        }
        self.view.widget(cx, ids!(comments)).set_text(cx, "");
        self.hide_inspector(cx);
        self.view.redraw(cx);
    }
    pub(super) fn bind(
        &mut self,
        cx: &mut Cx,
        identity: &str,
        project: Option<&str>,
        active: bool,
    ) {
        let project = project.unwrap_or("");
        let context_changed = self.identity != identity || self.project != project;
        // The parent binds on every event, including Draw and while this board
        // is hidden behind a room. Only a changed binding needs control sync;
        // otherwise its redraw creates an endless event/draw/bind cycle.
        if !context_changed && active == self.active { return; }
        if context_changed {
            self.remember_comment(cx);
            if !self.project.is_empty() && self.identity == identity {
                if let Some(pending) = self.pending.take() {
                    self.retained.insert(self.project.clone(), pending);
                }
            }
            if self.identity != identity {
                self.retained.clear();
                self.comment_drafts.clear();
            }
            self.clear_content(cx);
            self.identity = identity.into();
            self.project = project.into();
            self.query = Query::default();
            self.reset_viewports(cx);
            self.pending = self.retained.remove(project);
            self.view.text_input(cx, ids!(search)).set_text(cx, "");
            self.view
                .text_input(cx, ids!(filter_label))
                .set_text(cx, "");
            self.view
                .check_box(cx, ids!(overdue))
                .set_active(cx, false, Animate::No);
            self.view
                .check_box(cx, ids!(archived))
                .set_active(cx, false, Animate::No);
            self.view
                .drop_down(cx, ids!(filter_assignee))
                .set_selected_item(cx, 0);
            self.view
                .drop_down(cx, ids!(filter_state))
                .set_selected_item(cx, 0);
            self.epoch = nonce();
            self.reads.clear();
            self.loading = false;
            self.saving = false;
            self.failed = false;
            self.offline = false;
            self.retry_delay = 5.0;
            self.discard_armed = false;
        }
        if context_changed || active != self.active {
            if !active {
                self.hide_inspector(cx);
            }
            self.active = active;
            cx.stop_timer(self.timer);
            self.timer = Timer::default();
            self.epoch = nonce();
            self.reads.clear();
            self.loading = false;
            self.saving = false;
            self.drag = None;
            self.landing = None;
            if active && !project.is_empty() {
                self.load(cx);
            }
        }
        self.sync_controls(cx);
    }
    pub(super) fn read(&mut self, cx: &mut Cx, kind: Read, action: BoardAction) {
        if !self.active {
            return;
        }
        let Ok(service) = self.connection() else {
            return;
        };
        if service.identity() != self.identity {
            return;
        }
        let identity = self.identity.clone();
        let project = self.project.clone();
        let epoch = self.epoch;
        let request = nonce();
        if matches!(kind, Read::Snapshot | Read::Changes | Read::Page(_)) {
            if self.loading {
                return;
            }
            self.loading = true;
        }
        if matches!(kind, Read::Detail(_)) {
            self.detail_request = request;
        }
        match kind {
            Read::OpenTask(..) => self.open_request = request,
            Read::Attachment(..) => self.attachment_request = request,
            Read::Notifications(_) => self.notification_request = request,
            Read::Comments(..) => self.comments_request = request,
            _ => (),
        }
        self.reads.insert(request);
        let detail_generation = self.detail_request;
        crate::sliding_sync::spawn_async_task(async move {
            let result = service
                .execute(Command::ProjectBoard {
                    project_id: project.clone(),
                    action,
                })
                .await;
            Cx::post_action(Completed {
                identity,
                project,
                epoch,
                request,
                kind,
                detail_generation,
                result,
            });
        });
        self.sync_controls(cx);
    }
    pub(super) fn reset_viewports(&mut self, cx: &mut Cx) {
        for index in 0..12 {
            self.view
                .view(cx, column_path(index))
                .portal_list(cx, ids!(cards))
                .set_first_id_and_scroll(0, 0.0);
        }
        self.scroll_x = 0.0;
        self.view
            .view(cx, ids!(board_scroll))
            .set_scroll_pos(cx, dvec2(0.0, 0.0));
    }
    pub(super) fn load(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.timer);
        self.timer = Timer::default();
        self.read(
            cx,
            Read::Snapshot,
            BoardAction::Snapshot {
                query: self.query.clone(),
            },
        );
    }
    pub(super) fn schedule(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.timer);
        if self.active && self.foreground && !self.failed {
            self.timer = cx.start_timeout(self.retry_delay.max(5.0));
        } else {
            self.timer = Timer::default();
        }
    }
    pub(super) fn write(&mut self, cx: &mut Cx, action: BoardAction) {
        if self.pending.is_some() || self.saving {
            return;
        }
        self.pending = Some(action);
        self.retry_write(cx);
    }
    pub(super) fn save_task(&mut self, cx: &mut Cx) {
        if self.pending.is_some()
            || self.conflict
            || self.selected_column_archived()
            || (!self.creating && (!self.detail_loaded || self.selected.is_none()))
            || self
                .selected
                .as_ref()
                .is_some_and(|t| t.archived_at.is_some())
        {
            return;
        }
        if !self.creating && !self.dirty {
            self.request_close_inspector(cx);
            return;
        }
        let mut fields = self.fields(cx);
        if let Err(e) = fields.validate() {
            self.notice(cx, error_message(e.0));
            return;
        }
        self.subtask_after_save = None;
        let idempotency_key = uuid::Uuid::new_v4().to_string();
        if self.creating {
            self.write(
                cx,
                BoardAction::Create {
                    input: Create {
                        idempotency_key,
                        fields,
                        state: self.draft_state.clone(),
                    },
                },
            );
        } else if let Some(task) = self.selected.clone() {
            self.write(
                cx,
                BoardAction::Update {
                    task: task.id,
                    input: Update {
                        state: Some(self.draft_state.clone()),
                        idempotency_key,
                        fields,
                        expected_version: task.version,
                    },
                },
            );
        }
    }
    pub(super) fn retry_write(&mut self, cx: &mut Cx) {
        if self.saving || !self.active {
            return;
        }
        let Some(action) = self.pending.clone() else {
            return;
        };
        let Ok(service) = self.connection() else {
            return;
        };
        if service.identity() != self.identity {
            return;
        }
        let identity = self.identity.clone();
        let project = self.project.clone();
        let epoch = self.epoch;
        let request = nonce();
        self.save_request = request;
        self.saving = true;
        self.notice(cx, tr("Saving changes…"));
        self.sync_controls(cx);
        crate::sliding_sync::spawn_async_task(async move {
            let result = service
                .execute(Command::ProjectBoard {
                    project_id: project.clone(),
                    action,
                })
                .await;
            Cx::post_action(Saved {
                identity,
                project,
                epoch,
                request,
                result,
            });
        });
    }
    pub(super) fn fields(&self, cx: &mut Cx) -> Fields {
        let due = self.view.text_input(cx, ids!(due_date)).text();
        let assignee = self.view.drop_down(cx, ids!(assignee)).selected_item();
        Fields {
            title: self.view.text_input(cx, ids!(title)).text(),
            description: self
                .view
                .text_input(cx, ids!(description_editor.source))
                .text(),
            priority: PRIORITIES[self
                .view
                .drop_down(cx, ids!(priority))
                .selected_item()
                .min(3)]
            .into(),
            assignee_mxid: assignee
                .checked_sub(1)
                .and_then(|i| self.members.get(i).cloned()),
            labels: self
                .view
                .text_input(cx, ids!(labels))
                .text()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            parent_id: self.draft_parent.clone(),
            checklist: self.draft_checklist.clone(),
            due_date: if due.trim().is_empty() {
                None
            } else {
                Some(due.trim().into())
            },
        }
    }
    pub(super) fn fill_editor(&mut self, cx: &mut Cx, fields: Fields) {
        self.close_calendar(cx);
        self.view
            .text_input(cx, ids!(checklist_new))
            .set_text(cx, "");
        self.draft_checklist = fields.checklist.clone();
        self.draft_parent = fields.parent_id.clone();
        self.checklist_rows.clear();
        self.view
            .text_input(cx, ids!(title))
            .set_text(cx, &fields.title);
        self.view
            .text_input(cx, ids!(description_editor.source))
            .set_text(cx, &fields.description);
        self.view
            .text_input(cx, ids!(labels))
            .set_text(cx, &fields.labels.join(", "));
        self.view
            .text_input(cx, ids!(due_date))
            .set_text(cx, fields.due_date.as_deref().unwrap_or(""));
        self.view.drop_down(cx, ids!(priority)).set_selected_item(
            cx,
            PRIORITIES
                .iter()
                .position(|p| *p == fields.priority)
                .unwrap_or(1),
        );
        self.view.drop_down(cx, ids!(assignee)).set_selected_item(
            cx,
            fields
                .assignee_mxid
                .as_ref()
                .and_then(|s| self.members.iter().position(|m| m == s))
                .map_or(0, |i| i + 1),
        );
        self.dirty = false;
        self.conflict = false;
        self.discard_armed = false;
        self.description_preview = !self.creating && !fields.description.is_empty();
        self.comment_preview = false;
        self.sync_markdown_views(cx);
        self.sync_controls(cx);
    }
    pub(super) fn select(&mut self, cx: &mut Cx, task: &BoardTask) {
        if self.pending.is_some()
            || self.dirty
            || self.columns_draft.is_some()
            || self.quick_create_state.is_some()
        {
            self.notice(
                cx,
                tr("Save or close your current draft before opening another task."),
            );
            return;
        }
        self.remember_comment(cx);
        self.clear_media(cx);
        self.attachments.clear();
        self.view
            .widget(cx, ids!(attachment_preview))
            .set_visible(cx, false);
        self.restore_comment(cx, Some(&task.id));
        self.comments.clear();
        self.view
            .widget(cx, ids!(comments))
            .set_text(cx, tr("Loading…"));
        self.creating = false;
        self.discussion = false;
        self.detail_loaded = false;
        self.subtasks.clear();
        self.parent_task = None;
        self.view
            .view(cx, ids!(editor_scroll))
            .set_scroll_pos(cx, dvec2(0.0, 0.0));
        self.fill_editor(cx, task.fields());
        self.selected = Some(task.clone());
        self.draft_state = task.state.clone();
        self.sync_column_labels(cx);
        self.notice(cx, "");
        self.open_inspector(cx);
        self.view
            .label(cx, ids!(detail_heading))
            .set_text(cx, &format!("#{}", task.number));
        self.view
            .label(cx, ids!(detail_notice))
            .set_text(cx, tr("Loading task…"));
        self.sync_controls(cx);
        self.read(
            cx,
            Read::Detail(task.id.clone()),
            BoardAction::Detail {
                task: task.id.clone(),
            },
        );
    }
    pub(super) fn new_task(&mut self, cx: &mut Cx) {
        if self.pending.is_some()
            || self.dirty
            || self.columns_draft.is_some()
            || self.quick_create_state.is_some()
        {
            self.notice(
                cx,
                tr("Save or close your current draft before opening another task."),
            );
            return;
        }
        self.subtasks.clear();
        self.parent_task = None;
        self.view
            .view(cx, ids!(editor_scroll))
            .set_scroll_pos(cx, dvec2(0.0, 0.0));
        self.remember_comment(cx);
        self.restore_comment(cx, None);
        self.creating = true;
        self.discussion = false;
        let state = self
            .query
            .state
            .as_ref()
            .and_then(|s| self.status_index(s))
            .unwrap_or(0);
        self.view
            .drop_down(cx, ids!(new_status))
            .set_selected_item(cx, state);
        self.draft_state = self.status_state(state);
        self.selected = None;
        self.comments.clear();
        self.open_inspector(cx);
        self.view
            .label(cx, ids!(detail_heading))
            .set_text(cx, tr("New task"));
        self.view
            .label(cx, ids!(detail_notice))
            .set_text(cx, tr("Visible to current Project members."));
        self.fill_editor(cx, Fields::default());
        self.notice(cx, "");
    }
    pub(super) fn sync_members(&mut self, cx: &mut Cx, next: Vec<String>) {
        let selected = if self.dirty {
            self.fields(cx).assignee_mxid
        } else {
            self.selected.as_ref().and_then(|t| t.assignee_mxid.clone())
        };
        let joined: HashSet<_> = next.iter().cloned().collect();
        self.members = next;
        for member in [selected.clone(), self.query.assignee.clone()]
            .into_iter()
            .flatten()
        {
            if !self.members.contains(&member) {
                self.members.push(member);
            }
        }
        let previous_filter = self.query.assignee.clone();
        let mut labels = vec![tr("All members").to_owned()];
        labels.extend(self.members.iter().map(|id| self.member_choice(id)));
        self.view
            .drop_down(cx, ids!(filter_assignee))
            .set_labels(cx, labels);
        self.view
            .drop_down(cx, ids!(filter_assignee))
            .set_selected_item(
                cx,
                previous_filter
                    .as_ref()
                    .and_then(|s| self.members.iter().position(|m| m == s))
                    .map_or(0, |i| i + 1),
            );
        let mut labels = vec![tr("Unassigned").to_owned()];
        labels.extend(self.members.iter().cloned());
        let labels = labels
            .into_iter()
            .map(|label| {
                if label.starts_with('@') && !joined.contains(&label) {
                    format!("{} · {}", label, tr("Former member"))
                } else if label.starts_with('@') {
                    self.member_choice(&label)
                } else {
                    label
                }
            })
            .collect();
        self.view
            .drop_down(cx, ids!(assignee))
            .set_labels(cx, labels);
        self.view.drop_down(cx, ids!(assignee)).set_selected_item(
            cx,
            selected
                .as_ref()
                .and_then(|s| self.members.iter().position(|m| m == s))
                .map_or(0, |i| i + 1),
        );
    }
    pub(super) fn sync_controls(&mut self, cx: &mut Cx) {
        self.sync_quick_create(cx);
        self.view
            .view(cx, ids!(details_body))
            .set_visible(cx, !self.discussion);
        for (id, selected) in [
            (ids!(detail_tab).as_slice(), !self.discussion),
            (ids!(discussion_tab).as_slice(), self.discussion),
        ] {
            let mut button = self.view.button(cx, id);
            if selected {
                script_apply_eval!(cx, button, {draw_bg +: {color:mod.widgets.RINX_SURFACE color_hover:mod.widgets.RINX_SURFACE color_focus:mod.widgets.RINX_SURFACE} draw_text.color:mod.widgets.RINX_ACCENT});
            } else {
                script_apply_eval!(cx, button, {draw_bg +: {color:#0000 color_hover:mod.widgets.RINX_HOVER color_focus:#0000} draw_text.color:mod.widgets.RINX_MUTED});
            }
        }
        self.view
            .button(cx, ids!(discussion_tab))
            .set_board_enabled(cx, self.selected.is_some() && !self.creating);
        self.view
            .button(cx, ids!(archive))
            .set_visible(cx, self.selected.is_some() && !self.creating);
        let mut attachments = self.view.portal_list(cx, ids!(attachment_list));
        script_apply_eval!(cx, attachments, {height: #(self.attachments.len().max(1) as f64 * 74.0)});
        self.view
            .view(cx, ids!(attachments_section))
            .set_visible(cx, self.selected.is_some() && !self.creating);
        self.view
            .button(cx, ids!(upload_attachment))
            .set_board_enabled(
                cx,
                self.detail_loaded
                    && self.pending.is_none()
                    && !self
                        .selected
                        .as_ref()
                        .is_some_and(|t| t.archived_at.is_some()),
            );
        self.sync_extension_controls(cx);
        self.view
            .view(cx, ids!(create_status))
            .set_visible(cx, self.creating);
        let filtered = self.query.state.is_some()
            || self.query.query.is_some()
            || self.query.label.is_some()
            || self.query.assignee.is_some()
            || self.query.overdue
            || self.query.archived;
        self.view
            .button(cx, ids!(clear_filters))
            .set_visible(cx, filtered);
        self.view
            .button(cx, ids!(clear_filters))
            .set_board_enabled(cx, !self.loading);
        for (id, pending) in [
            (
                ids!(more_comments),
                self.reads.contains(&self.comments_request),
            ),
            (
                ids!(first_comments),
                self.reads.contains(&self.comments_request),
            ),
        ] {
            self.view.button(cx, id).set_board_enabled(cx, !pending);
        }
        let archived = self
            .selected
            .as_ref()
            .is_some_and(|t| t.archived_at.is_some())
            || self.selected_column_archived();
        let writing = self.pending.is_some();
        let editing = (self.creating || self.selected.is_some() && self.detail_loaded)
            && !writing
            && !self.conflict
            && !archived;
        for id in [
            ids!(title).as_slice(),
            ids!(new_status),
            ids!(description_editor.source),
            ids!(priority),
            ids!(assignee),
            ids!(labels),
            ids!(due_date),
        ] {
            self.view.widget(cx, id).set_disabled(cx, !editing);
        }
        self.view
            .button(cx, ids!(save))
            .set_board_enabled(cx, editing);
        self.view
            .button(cx, ids!(open_calendar))
            .set_board_enabled(cx, editing);
        if !editing {
            self.close_calendar(cx);
        }
        self.view.widget(cx, ids!(save)).set_disabled(cx, !editing);
        self.view.button(cx, ids!(new_task)).set_board_enabled(
            cx,
            self.snapshot.is_some() && !writing && !self.failed && self.columns_draft.is_none(),
        );
        self.view
            .button(cx, ids!(refresh))
            .set_board_enabled(cx, !self.loading);
        self.view
            .button(cx, ids!(apply_filters))
            .set_board_enabled(cx, !self.loading);
        self.view
            .button(cx, ids!(retry))
            .set_visible(cx, writing && !self.saving);
        self.view
            .button(cx, ids!(retry))
            .set_board_enabled(cx, !self.saving);
        self.view
            .button(cx, ids!(dialog_retry))
            .set_visible(cx, writing && !self.saving);
        self.view
            .button(cx, ids!(dialog_retry))
            .set_board_enabled(cx, !self.saving);
        self.view.button(cx, ids!(reload_latest)).set_visible(
            cx,
            self.conflict || self.selected.is_some() && !self.detail_loaded,
        );
        self.view.view(cx, ids!(existing_actions)).set_visible(
            cx,
            !self.creating && self.selected.is_some() && self.discussion,
        );
        for id in [ids!(move_state).as_slice(), ids!(comment_editor.source)] {
            self.view
                .widget(cx, id)
                .set_disabled(cx, writing || self.conflict || archived);
        }
        for (id, enabled) in [
            (
                ids!(move_top),
                !writing && !self.conflict && !archived && !self.dirty && self.detail_loaded,
            ),
            (
                ids!(move_bottom),
                !writing && !self.conflict && !archived && !self.dirty && self.detail_loaded,
            ),
            (
                ids!(add_comment),
                !writing && !self.conflict && !archived && self.detail_loaded,
            ),
        ] {
            self.view.button(cx, id).set_board_enabled(cx, enabled);
            self.view.widget(cx, id).set_disabled(cx, !enabled);
        }
        self.view.button(cx, ids!(archive)).set_board_enabled(
            cx,
            !writing && !self.dirty && !self.conflict && !self.selected_column_archived(),
        );
        self.view
            .button(cx, ids!(archive))
            .set_text(cx, tr(if archived { "Restore" } else { "Archive" }));
        self.view
            .button(cx, ids!(save))
            .set_text(cx, tr(if self.saving { "Saving…" } else { "Save" }));
        self.view.button(cx, ids!(close_detail)).set_text(
            cx,
            tr(if self.discard_armed {
                "Discard draft"
            } else {
                "Close"
            }),
        );
        self.view
            .widget(cx, ids!(board_loading))
            .set_visible(cx, self.loading);
        self.view.label(cx, ids!(sync_status)).set_text(
            cx,
            tr(if self.loading {
                "Refreshing…"
            } else if self.saving {
                "Saving…"
            } else if self.offline {
                "Offline · showing last confirmed board"
            } else if self.snapshot.is_some() {
                "Up to date"
            } else {
                ""
            }),
        );
        self.view
            .view(cx, ids!(task_status))
            .set_visible(cx, !self.creating && self.selected.is_some());
        let can_assign = editing
            && self
                .current_mxid()
                .is_some_and(|id| self.members.contains(&id));
        self.view
            .button(cx, ids!(assign_me))
            .set_board_enabled(cx, can_assign);
        self.view
            .widget(cx, ids!(assign_me))
            .set_disabled(cx, !can_assign);
        self.sync_markdown_controls(
            cx,
            editing,
            !writing && !self.conflict && !archived && self.detail_loaded,
        );
        self.view.redraw(cx);
    }
    pub(super) fn detail_history(&mut self, cx: &mut Cx, id: &str) {
        // Keep same-task history visible during refresh. Clearing it shrinks the
        // inspector and clamps scroll to the top while an image is uploading.
        // Task selection clears the old task before requesting its detail.
        self.comments_after = 0;
        self.read(
            cx,
            Read::Comments(id.into(), false),
            BoardAction::Comments {
                task: id.into(),
                after: 0,
            },
        );
    }
    pub(super) fn apply_read(&mut self, cx: &mut Cx, done: &Completed) {
        if done.identity != self.identity
            || done.project != self.project
            || done.epoch != self.epoch
            || !self.reads.remove(&done.request)
        {
            return;
        }
        if matches!(done.kind, Read::Comments(..)) && done.detail_generation != self.detail_request
        {
            return;
        }
        if matches!(done.kind, Read::Comments(..)) && done.request != self.comments_request {
            return;
        }
        if matches!(done.kind, Read::Snapshot | Read::Changes | Read::Page(_)) {
            self.loading = false;
        }
        if matches!(done.kind, Read::OpenTask(..))
            && (done.request != self.open_request || done.detail_generation != self.detail_request)
        {
            return;
        }
        if matches!(done.kind, Read::Notifications(_)) && done.request != self.notification_request
        {
            return;
        }
        if let Read::Notifications(before) = done.kind
            && before != self.notification_before
        {
            self.refresh_notifications(cx);
            return;
        }
        if matches!(done.kind, Read::Attachment(..))
            && (done.detail_generation != self.detail_request
                || done.request != self.attachment_request)
        {
            return;
        }
        match &done.result {
            Err(code) => {
                if let Read::InlineImage(task, id, scope) = &done.kind {
                    self.inline_image(cx, task, id, *scope, &Value::Null);
                }
                self.offline = uncertain(code);
                self.retry_delay = (self.retry_delay.max(5.0) * 2.0).min(60.0);
                self.notice(cx, error_message(code));
                if denied(code) || code == "board_unsupported" {
                    self.clear_content(cx);
                    self.failed = true;
                } else if code == "board_snapshot_changed" || code == "board_cursor_reset" {
                    self.load(cx);
                } else {
                    self.failed = false;
                }
            }
            Ok(value) => {
                self.offline = false;
                self.retry_delay = 5.0;
                match &done.kind {
                    Read::Notifications(_)
                    | Read::NotificationAck
                    | Read::OpenTask(_)
                    | Read::Attachment(..) => self.apply_collaboration(cx, &done.kind, value),
                    Read::Snapshot => match serde_json::from_value::<Snapshot>(value.clone()) {
                        Ok(snapshot)
                            if snapshot.project_id == self.project
                                && snapshot.capability == CAPABILITY
                                && (2..=12).contains(&snapshot.lanes.len())
                                && snapshot.lanes.iter().all(|l| {
                                    valid_state(&l.state)
                                        && l.tasks.len() <= 100
                                        && l.tasks.iter().all(|t| {
                                            t.project_id == self.project && t.state == l.state
                                        })
                                }) =>
                        {
                            let changed = self
                                .snapshot
                                .as_ref()
                                .is_some_and(|old| old.sequence != snapshot.sequence);
                            let refresh_detail = self.selected.clone().filter(|_| {
                                changed
                                    && self.detail_loaded
                                    && !self.dirty
                                    && self.pending.is_none()
                            });
                            let members = snapshot.members.clone();
                            self.member_names = snapshot.member_names.clone();
                            self.snapshot = Some(snapshot);
                            self.refresh_notifications(cx);
                            self.failed = false;
                            self.sync_members(cx, members);
                            self.sync_column_labels(cx);
                            if let Some(task) = refresh_detail {
                                self.read(
                                    cx,
                                    Read::Detail(task.id.clone()),
                                    BoardAction::Detail { task: task.id },
                                );
                            }
                            if !self.dirty && !self.conflict && self.pending.is_none() {
                                self.notice(cx, "");
                            }
                            if self.pending.is_some() && !self.saving {
                                self.notice(cx,tr("A previous save is unconfirmed. Retry its original request to avoid duplicates."));
                            }
                        }
                        _ => self.notice(cx, error_message("invalid_server_response")),
                    },
                    Read::Changes => {
                        self.refresh_notifications(cx);
                        let seq = value["sequence"].as_i64().unwrap_or(-1);
                        if self.snapshot.as_ref().is_some_and(|s| s.sequence != seq) {
                            self.load(cx);
                        }
                    }
                    Read::Page(index) => {
                        if let Some(snapshot) = &mut self.snapshot {
                            if value["sequence"].as_i64() != Some(snapshot.sequence) {
                                self.load(cx);
                                return;
                            }
                            if let Ok(tasks) =
                                serde_json::from_value::<Vec<BoardTask>>(value["tasks"].clone())
                                && tasks.len() <= 100
                                && tasks.iter().all(|t| {
                                    t.project_id == self.project
                                        && t.state == snapshot.lanes[*index].state
                                })
                            {
                                let lane = &mut snapshot.lanes[*index];
                                let ids: HashSet<_> =
                                    lane.tasks.iter().map(|t| t.id.clone()).collect();
                                lane.tasks
                                    .extend(tasks.into_iter().filter(|t| !ids.contains(&t.id)));
                                lane.next_cursor = value["nextCursor"].as_str().map(str::to_owned);
                            }
                        }
                    }
                    Read::Detail(id) => {
                        if self.detail_request != done.request
                            || !self.selected.as_ref().is_some_and(|t| t.id == *id)
                        {
                            return;
                        }
                        if let Ok(task) = serde_json::from_value::<BoardTask>(value["task"].clone())
                            && task.project_id == self.project
                            && task.id == *id
                        {
                            let related =
                                serde_json::from_value::<Vec<BoardTask>>(value["subtasks"].clone())
                                    .ok()
                                    .filter(|children| {
                                        children.len() <= 100
                                            && children.iter().all(|child| {
                                                child.project_id == self.project
                                                    && child.parent_id.as_deref()
                                                        == Some(id.as_str())
                                            })
                                    });
                            let parent = if value["parent"].is_null() {
                                Some(None)
                            } else {
                                serde_json::from_value::<BoardTask>(value["parent"].clone())
                                    .ok()
                                    .map(Some)
                            };
                            let (Some(children), Some(parent)) = (related, parent) else {
                                self.detail_loaded = false;
                                self.notice(cx, error_message("invalid_server_response"));
                                self.sync_controls(cx);
                                return;
                            };
                            if parent.as_ref().map(|p| p.id.as_str()) != task.parent_id.as_deref()
                                || parent
                                    .as_ref()
                                    .is_some_and(|p| p.project_id != self.project)
                            {
                                self.detail_loaded = false;
                                self.notice(cx, error_message("invalid_server_response"));
                                self.sync_controls(cx);
                                return;
                            }
                            self.attachments =
                                value["attachments"].as_array().cloned().unwrap_or_default();
                            self.subtasks = children;
                            self.parent_task = parent;
                            let keep_modes = self.detail_loaded;
                            let modes = (self.description_preview, self.comment_preview);
                            self.detail_loaded = true;
                            self.selected = Some(task.clone());
                            if !self.dirty && !self.conflict {
                                self.fill_editor(cx, task.fields());
                                if keep_modes {
                                    (self.description_preview, self.comment_preview) = modes;
                                    self.sync_markdown_views(cx);
                                }
                            }
                            self.view.label(cx, ids!(detail_notice)).set_text(
                                cx,
                                tr(if self.selected_column_archived() {
                                    "Archived column · restore the column to edit its cards."
                                } else if task.archived_at.is_some() {
                                    "Archived · restore to edit this task."
                                } else {
                                    "Visible to current Project members."
                                }),
                            );
                            if !self.dirty && !self.conflict {
                                self.draft_state = task.state.clone();
                                self.view.drop_down(cx, ids!(move_state)).set_selected_item(
                                    cx,
                                    self.status_index(&task.state).unwrap_or(0),
                                );
                            }
                            self.detail_history(cx, id);
                        }
                    }
                    Read::InlineImage(task, id, scope) => {
                        self.inline_image(cx, task, id, *scope, value)
                    }
                    Read::Comments(id, append) => {
                        if !self.selected.as_ref().is_some_and(|t| t.id == *id) {
                            return;
                        }
                        self.view
                            .button(cx, ids!(first_comments))
                            .set_visible(cx, *append);
                        self.comments = value["items"].as_array().cloned().unwrap_or_default();
                        self.comments_after = value["next"].as_i64().unwrap_or(0);
                        let media_scope = self.media_scope();
                        let text = self
                            .comments
                            .iter()
                            .map(|v| {
                                let author = markdown::escape_html(
                                    self.member_name(v["author"].as_str().unwrap_or("")),
                                );
                                let time = timestamp(&v["createdAt"]);
                                format!(
                                    "<p><strong>{author}</strong> · {time}</p>{}<hr/>",
                                    markdown::render_markdown_with_media(
                                        v["body"].as_str().unwrap_or(""),
                                        media_scope
                                    )
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("");
                        self.view.widget(cx, ids!(comments)).set_text(
                            cx,
                            if text.is_empty() {
                                tr("No comments yet")
                            } else {
                                &text
                            },
                        );
                        self.view
                            .button(cx, ids!(more_comments))
                            .set_visible(cx, value["hasMore"] == true);
                    }
                }
            }
        }
        self.sync_inline_images(cx);
        self.sync_controls(cx);
        if !self.loading {
            self.schedule(cx);
        }
    }
    pub(super) fn apply_saved(&mut self, cx: &mut Cx, done: &Saved) {
        if done.identity != self.identity
            || done.project != self.project
            || done.epoch != self.epoch
            || done.request != self.save_request
        {
            return;
        }
        self.saving = false;
        if let Some(landing) = &mut self.landing {
            landing.awaiting = false;
        }
        match &done.result {
            Ok(value) => {
                let was_quick = self.is_quick_save();
                let task_save = matches!(
                    self.pending,
                    Some(BoardAction::Create { .. } | BoardAction::Update { .. })
                );
                let confirmed = serde_json::from_value::<BoardTask>(value["task"].clone())
                    .ok()
                    .filter(|task| {
                        task.project_id == self.project
                            && match &self.pending {
                                Some(BoardAction::Update { task: id, input }) => {
                                    task.id == *id
                                        && input
                                            .state
                                            .as_ref()
                                            .is_none_or(|state| task.state == *state)
                                }
                                Some(BoardAction::Create { input }) => task.state == input.state,
                                _ => true,
                            }
                    });
                if task_save && confirmed.is_none() {
                    self.notice(cx, error_message("invalid_board_response"));
                    self.sync_controls(cx);
                    return;
                }
                let next_subtask = self.subtask_after_save.take().filter(|(key, parent)| {
                    matches!(&self.pending, Some(BoardAction::Update {task, input})
                        if task == parent && input.idempotency_key == *key)
                });
                let close_after_save = task_save && !was_quick && next_subtask.is_none();
                let was_columns =
                    matches!(self.pending, Some(BoardAction::ConfigureColumns { .. }));
                let was_comment = matches!(self.pending, Some(BoardAction::Comment { .. }));
                let was_attachment = matches!(
                    self.pending,
                    Some(
                        BoardAction::UploadAttachment { .. } | BoardAction::RemoveAttachment { .. }
                    )
                );
                self.epoch = nonce();
                self.reads.clear();
                self.loading = false;
                let saved_action = self.pending.take();
                self.media_saved(cx, &saved_action, value);
                self.retained.remove(&self.project);
                if !was_comment && !was_attachment {
                    self.dirty = false;
                }
                self.conflict = false;
                if !was_quick
                    && let Ok(task) = serde_json::from_value::<BoardTask>(value["task"].clone())
                    && task.project_id == self.project
                {
                    self.detail_loaded = true;
                    self.selected = Some(task.clone());
                    self.creating = false;
                    self.view
                        .label(cx, ids!(detail_heading))
                        .set_text(cx, &format!("#{}", task.number));
                    if !was_comment && !was_attachment {
                        self.fill_editor(cx, task.fields());
                    } else if was_comment {
                        self.view
                            .label(cx, ids!(comment_draft_hint))
                            .set_text(cx, "");
                        self.view
                            .text_input(cx, ids!(comment_editor.source))
                            .set_text(cx, "");
                        self.comment_drafts
                            .remove(&(self.project.clone(), task.id.clone()));
                        self.comment_preview = false;
                        self.sync_markdown_views(cx);
                    }
                    if !task_save {
                        self.detail_history(cx, &task.id);
                    }
                    if was_attachment {
                        self.read(
                            cx,
                            Read::Detail(task.id.clone()),
                            BoardAction::Detail {
                                task: task.id.clone(),
                            },
                        );
                    }
                }
                if was_columns {
                    self.columns_draft = None;
                    self.view
                        .view(cx, ids!(column_settings))
                        .set_visible(cx, false);
                    self.query.state = None;
                    self.reset_viewports(cx);
                }
                if was_quick {
                    self.clear_quick_create(cx);
                }
                if let Some((_, parent)) = next_subtask {
                    if let Some(task) = confirmed.filter(|task| task.id == parent) {
                        self.begin_subtask(cx, task);
                    }
                } else if close_after_save {
                    self.request_close_inspector(cx);
                }
                self.notice(
                    cx,
                    if was_attachment {
                        tr("Attachment updated. Save or post your draft when ready.")
                    } else {
                        tr("Changes saved")
                    },
                );
                self.load(cx);
            }
            Err(code) => {
                if matches!(self.pending, Some(BoardAction::ConfigureColumns { .. })) {
                    self.view
                        .label(cx, ids!(columns_notice))
                        .set_text(cx, error_message(code));
                } else {
                    self.notice(cx, error_message(code));
                }
                if !uncertain(code) {
                    self.cancel_media_uploads();
                    self.pending = None;
                    self.subtask_after_save = None;
                }
                if denied(code) {
                    self.clear_content(cx);
                    self.failed = true;
                } else if code == "board_version_conflict" {
                    self.conflict = true;
                    self.load(cx);
                } else if code == "board_neighbours_changed"
                    || code == "board_snapshot_changed"
                    || code == "board_archive_conflict"
                {
                    self.load(cx);
                }
            }
        }
        self.sync_controls(cx);
    }
    pub(super) fn move_task(
        &mut self,
        cx: &mut Cx,
        task: BoardTask,
        state: &str,
        position: Position,
        before: Option<String>,
        after: Option<String>,
    ) {
        let expected_sequence = self.snapshot.as_ref().map(|s| s.sequence);
        self.write(
            cx,
            BoardAction::Move {
                task: task.id,
                input: Move {
                    idempotency_key: uuid::Uuid::new_v4().to_string(),
                    expected_version: task.version,
                    state: state.into(),
                    expected_sequence,
                    before,
                    after,
                    position,
                },
            },
        );
    }
}
