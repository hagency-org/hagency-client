use super::*;

pub(super) fn column_title(title: &str) -> String {
    match title {
        "To do" | "In progress" | "Blocked" | "In review" | "Done" => tr(title).into(),
        _ => title.into(),
    }
}
impl ProjectBoard {
    pub(super) fn begin_subtask(&mut self, cx: &mut Cx, parent: BoardTask) {
        self.new_task(cx);
        if !self.creating || self.selected.is_some() {
            return;
        }
        self.draft_parent = Some(parent.id.clone());
        self.parent_task = Some(parent);
        self.dirty = true;
        self.dialog_focus_pending = true;
        self.view
            .label(cx, ids!(detail_heading))
            .set_text(cx, tr("New subtask"));
        self.sync_controls(cx);
    }

    pub(super) fn add_subtask(&mut self, cx: &mut Cx) {
        if self.pending.is_some()
            || self.conflict
            || !self.detail_loaded
            || self.subtasks.len() >= 100
        {
            return;
        }
        let Some(parent) = self
            .selected
            .clone()
            .filter(|t| t.parent_id.is_none() && t.archived_at.is_none())
        else {
            return;
        };
        if !self.dirty {
            self.begin_subtask(cx, parent);
            return;
        }
        let mut fields = self.fields(cx);
        if let Err(e) = fields.validate() {
            self.notice(cx, error_message(e.0));
            return;
        }
        let idempotency_key = uuid::Uuid::new_v4().to_string();
        self.subtask_after_save = Some((idempotency_key.clone(), parent.id.clone()));
        self.write(
            cx,
            BoardAction::Update {
                task: parent.id,
                input: Update {
                    state: Some(self.draft_state.clone()),
                    idempotency_key,
                    fields,
                    expected_version: parent.version,
                },
            },
        );
    }
    pub(super) fn column_count(&self) -> usize {
        self.snapshot.as_ref().map_or(0, |s| s.lanes.len())
    }
    pub(super) fn column_state(&self, index: usize) -> String {
        self.snapshot
            .as_ref()
            .and_then(|s| s.lanes.get(index))
            .map(|l| l.state.clone())
            .unwrap_or_else(|| "todo".into())
    }
    pub(super) fn status_state(&self, index: usize) -> String {
        self.snapshot
            .as_ref()
            .and_then(|s| s.lanes.iter().filter(|l| !l.archived).nth(index))
            .map(|l| l.state.clone())
            .unwrap_or_default()
    }
    pub(super) fn status_index(&self, state: &str) -> Option<usize> {
        self.snapshot
            .as_ref()?
            .lanes
            .iter()
            .filter(|l| !l.archived)
            .position(|l| l.state == state)
    }
    pub(super) fn selected_column_archived(&self) -> bool {
        self.selected.as_ref().is_some_and(|task| {
            self.snapshot
                .as_ref()
                .is_some_and(|s| s.lanes.iter().any(|l| l.state == task.state && l.archived))
        })
    }
    pub(super) fn sync_column_labels(&mut self, cx: &mut Cx) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let labels: Vec<_> = snapshot
            .lanes
            .iter()
            .filter(|l| !l.archived)
            .map(|l| column_title(&l.title))
            .collect();
        for id in [ids!(new_status), ids!(move_state)] {
            self.view.drop_down(cx, id).set_labels(cx, labels.clone());
            self.view
                .drop_down(cx, id)
                .set_selected_item(cx, self.status_index(&self.draft_state).unwrap_or(0));
        }
        if self.selected_column_archived()
            && let Some(lane) = self.selected.as_ref().and_then(|task| snapshot.lanes.iter().find(|l| l.state == task.state))
        {
            self.view.drop_down(cx, ids!(move_state)).set_labels(cx, vec![format!("{} · {}", column_title(&lane.title), tr("Archived"))]);
            self.view.drop_down(cx, ids!(move_state)).set_selected_item(cx, 0);
        }
        let mut filter = vec![tr("All statuses").into()];
        filter.extend(labels);
        self.view
            .drop_down(cx, ids!(filter_state))
            .set_labels(cx, filter);
        let index = self.query.state.as_ref().and_then(|q| self.status_index(q));
        if self.query.state.is_some() && index.is_none() {
            self.query.state = None;
        }
        self.view
            .drop_down(cx, ids!(filter_state))
            .set_selected_item(cx, index.map_or(0, |i| i + 1));
    }
    pub(super) fn show_column_form(&mut self, cx: &mut Cx) {
        let Some(columns) = &self.columns_draft else {
            return;
        };
        self.column_edit = self.column_edit.min(columns.len().saturating_sub(1));
        let Some(column) = columns.get(self.column_edit) else {
            return;
        };
        self.view.drop_down(cx, ids!(edit_column)).set_labels(
            cx,
            columns
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    format!(
                        "{}. {}{}",
                        i + 1,
                        column_title(&c.title),
                        if c.archived {
                            format!(" · {}", tr("Archived"))
                        } else {
                            String::new()
                        }
                    )
                })
                .collect(),
        );
        self.view
            .drop_down(cx, ids!(edit_column))
            .set_selected_item(cx, self.column_edit);
        self.view
            .text_input(cx, ids!(column_name))
            .set_text(cx, &column_title(&column.title));
        self.view.text_input(cx, ids!(column_wip)).set_text(
            cx,
            &column.wip_limit.map(|n| n.to_string()).unwrap_or_default(),
        );
        self.view
            .check_box(cx, ids!(column_done))
            .set_active(cx, column.is_done, Animate::No);
        self.view.button(cx, ids!(archive_column)).set_text(
            cx,
            tr(if column.archived {
                "Restore column"
            } else {
                "Archive column"
            }),
        );
        self.view.check_box(cx, ids!(column_limit_enabled)).set_active(cx, column.wip_limit.is_some(), Animate::No);
        self.view.view(cx, ids!(column_limit_form)).set_visible(cx, column.wip_limit.is_some());
        self.discard_armed = false;
        self.view.button(cx, ids!(cancel_columns)).set_text(cx, tr("Cancel"));
        self.columns_notice(cx, "");
        self.sync_column_actions(cx);
    }
    pub(super) fn capture_column_form(&mut self, cx: &mut Cx) -> bool {
        let Some(column) = self
            .columns_draft
            .as_mut()
            .and_then(|v| v.get_mut(self.column_edit))
        else {
            return false;
        };
        let raw = self.view.text_input(cx, ids!(column_wip)).text();
        let limit = if !self.view.check_box(cx, ids!(column_limit_enabled)).active(cx) {
            None
        } else {
            match raw.trim().parse::<i64>() {
                Ok(n) if (1..=10000).contains(&n) => Some(n),
                _ => {
                    self.columns_notice(cx, tr("Enter a maximum between 1 and 10000, or turn off the limit."));
                    return false;
                }
            }
        };
        let name = self.view.text_input(cx, ids!(column_name)).text();
        // Keep default translation as a presentation detail until renamed.
        if name != column_title(&column.title) {
            column.title = name;
        }
        column.wip_limit = limit;
        column.is_done = self.view.check_box(cx, ids!(column_done)).active(cx);
        true
    }
    pub(super) fn handle_extensions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.view.button(cx, ids!(manage_columns)).clicked(actions)
            && !self.read_only && self.pending.is_none()
        {
            if self.dirty || self.quick_create_state.is_some() {
                self.notice(
                    cx,
                    tr("Save or close your current draft before opening another task."),
                );
            } else if let Some(snapshot) = &self.snapshot {
                self.selected = None;
                self.columns_draft = Some(
                    snapshot
                        .lanes
                        .iter()
                        .map(|l| ColumnDefinition {
                            archived: l.archived,
                            id: l.state.clone(),
                            title: l.title.clone(),
                            wip_limit: l.wip_limit,
                            is_done: l.is_done,
                        })
                        .collect(),
                );
                self.columns_sequence = snapshot.sequence;
                self.hide_inspector(cx);
                self.column_edit = 0;
                self.open_column_settings(cx);
                self.show_column_form(cx);
            }
        }
        if self.columns_draft.is_some() && self.pending.is_none() {
            if self.view.check_box(cx, ids!(column_limit_enabled)).changed(actions).is_some() {
                let enabled = self.view.check_box(cx, ids!(column_limit_enabled)).active(cx);
                self.view.view(cx, ids!(column_limit_form)).set_visible(cx, enabled);
                if enabled && self.view.text_input(cx, ids!(column_wip)).text().trim().is_empty() {
                    self.view.text_input(cx, ids!(column_wip)).set_text(cx, "3");
                }
            }
            let chosen = self.column_rows.iter().find(|(_, row)| row.button(cx, ids!(choose_column)).clicked(actions)).map(|(i, _)| *i);
            if let Some(index) = chosen && self.capture_column_form(cx) {
                self.column_edit = index;
                self.show_column_form(cx);
            }
            if self.view.button(cx, ids!(archive_column)).clicked(actions)
                && self.capture_column_form(cx)
            {
                if let Some(c) = self
                    .columns_draft
                    .as_mut()
                    .and_then(|c| c.get_mut(self.column_edit))
                {
                    c.archived = !c.archived;
                }
                self.show_column_form(cx);
            }
            if self.view.button(cx, ids!(rebase_columns)).clicked(actions) {
                if let Some(snapshot) = &self.snapshot {
                    self.columns_sequence = snapshot.sequence;
                    self.columns_notice(cx, tr("Review your column changes, then save again."));
                }
            } else if self.view.button(cx, ids!(cancel_columns)).clicked(actions) {
                self.request_close_columns(cx);
            } else if let Some(index) = self.view.drop_down(cx, ids!(edit_column)).changed(actions)
            {
                if self.capture_column_form(cx) {
                    self.column_edit = index;
                    self.show_column_form(cx);
                } else {
                    self.view
                        .drop_down(cx, ids!(edit_column))
                        .set_selected_item(cx, self.column_edit);
                }
            } else if self.view.button(cx, ids!(save_columns)).clicked(actions) {
                if self.capture_column_form(cx) {
                    let mut input = ConfigureColumns {
                        idempotency_key: uuid::Uuid::new_v4().to_string(),
                        expected_sequence: self.columns_sequence,
                        columns: self.columns_draft.clone().unwrap_or_default(),
                    };
                    match input.validate() {
                        Ok(()) => self.write(cx, BoardAction::ConfigureColumns { input }),
                        Err(e) => self.columns_notice(cx, error_message(e.0)),
                    }
                }
            } else if self.view.button(cx, ids!(add_column)).clicked(actions)
                && self.capture_column_form(cx)
            {
                self.append_column(cx);
            } else if self.view.button(cx, ids!(remove_column)).clicked(actions)
                && self.capture_column_form(cx)
            {
                let columns = self.columns_draft.as_mut().unwrap();
                if columns.len() > 2 && self.view.button(cx, ids!(remove_column)).borrow().is_some_and(|b| b.enabled()) {
                    columns.remove(self.column_edit);
                    self.column_edit = self.column_edit.saturating_sub(1);
                    self.show_column_form(cx);
                }
            } else if (self.view.button(cx, ids!(column_left)).clicked(actions)
                || self.view.button(cx, ids!(column_right)).clicked(actions))
                && self.capture_column_form(cx)
            {
                let columns = self.columns_draft.as_mut().unwrap();
                let next = if self.view.button(cx, ids!(column_left)).clicked(actions) {
                    self.column_edit.saturating_sub(1)
                } else {
                    (self.column_edit + 1).min(columns.len() - 1)
                };
                columns.swap(self.column_edit, next);
                self.column_edit = next;
                self.show_column_form(cx);
            }
        }
        let editable = (self.creating || self.detail_loaded)
            && !self.conflict
            && self.pending.is_none()
            && !self
                .selected
                .as_ref()
                .is_some_and(|t| t.archived_at.is_some());
        if editable {
            let mut changed = false;
            let mut remove = None;
            let mut up = None;
            for (&i, row) in &self.checklist_rows {
                if let Some(checked) = row.check_box(cx, ids!(checked)).changed(actions)
                    && let Some(item) = self.draft_checklist.get_mut(i)
                {
                    item.checked = checked;
                    changed = true;
                }
                if let Some(title) = row.text_input(cx, ids!(item_title)).changed(actions)
                    && let Some(item) = self.draft_checklist.get_mut(i)
                {
                    item.title = title;
                    changed = true;
                }
                if row.button(cx, ids!(item_remove)).clicked(actions) {
                    remove = Some(i);
                }
                if row.button(cx, ids!(item_up)).clicked(actions) {
                    up = Some(i);
                }
            }
            if let Some(i) = remove
                && i < self.draft_checklist.len()
            {
                self.draft_checklist.remove(i);
                self.checklist_rows.clear();
                changed = true;
            }
            if let Some(i) = up
                && i > 0
                && i < self.draft_checklist.len()
            {
                self.draft_checklist.swap(i, i - 1);
                self.checklist_rows.clear();
                changed = true;
            }
            if self.view.button(cx, ids!(checklist_add)).clicked(actions)
                || self
                    .view
                    .text_input(cx, ids!(checklist_new))
                    .returned(actions)
                    .is_some()
            {
                let title = self.view.text_input(cx, ids!(checklist_new)).text();
                if !title.trim().is_empty()
                    && title.trim().chars().count() <= 160
                    && !title.chars().any(char::is_control)
                    && self.draft_checklist.len() < 100
                {
                    self.draft_checklist.push(ChecklistItem {
                        id: uuid::Uuid::new_v4().to_string(),
                        title: title.trim().into(),
                        checked: false,
                    });
                    self.view
                        .text_input(cx, ids!(checklist_new))
                        .set_text(cx, "");
                    changed = true;
                } else {
                    self.notice(cx, error_message("invalid_board_checklist"));
                }
            }
            if self.view.button(cx, ids!(unlink_parent)).clicked(actions) {
                self.draft_parent = None;
                changed = true;
            }
            if changed {
                self.dirty = true;
                self.discard_armed = false;
                self.sync_controls(cx);
                self.view.redraw(cx);
            }
        }
        if self.view.button(cx, ids!(add_subtask)).clicked(actions) {
            self.add_subtask(cx);
        }
        if self.view.button(cx, ids!(open_parent)).clicked(actions)
            && self.pending.is_none()
            && !self.dirty
            && let Some(id) = self.draft_parent.clone()
        {
            if let Some(parent) = self.parent_task.clone().or_else(|| {
                self.snapshot
                    .as_ref()?
                    .lanes
                    .iter()
                    .flat_map(|l| &l.tasks)
                    .find(|t| t.id == id)
                    .cloned()
            }) {
                self.select(cx, &parent);
            }
        }
        let child = self
            .subtask_rows
            .iter()
            .find(|(_, row)| row.button(cx, ids!(open_child)).clicked(actions))
            .and_then(|(i, _)| self.subtasks.get(*i))
            .cloned();
        if let Some(child) = child {
            self.select(cx, &child);
        }
        self.sync_extension_controls(cx);
    }
    pub(super) fn sync_extension_controls(&mut self, cx: &mut Cx) {
        self.view.button(cx,ids!(manage_columns)).set_visible(cx,!self.read_only);
        let writing = self.pending.is_some() || self.read_only;
        let editable = (self.creating || self.selected.is_some() && self.detail_loaded)
            && !writing
            && !self.conflict
            && !self
                .selected
                .as_ref()
                .is_some_and(|t| t.archived_at.is_some());
        self.view
            .button(cx, ids!(manage_columns))
            .set_board_enabled(
                cx,
                self.snapshot.is_some()
                    && !writing
                    && !self.dirty
                    && self.columns_draft.is_none()
                    && self.quick_create_state.is_none(),
            );
        self.view.button(cx, ids!(rebase_columns)).set_visible(
            cx,
            self.columns_draft.is_some()
                && self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.sequence != self.columns_sequence),
        );
        self.view
            .button(cx, ids!(rebase_columns))
            .set_board_enabled(cx, !writing);
        for id in [
            ids!(save_columns),
            ids!(cancel_columns),
            ids!(add_column),
            ids!(column_left),
            ids!(column_right),
            ids!(archive_column),
        ] {
            self.view.button(cx, id).set_board_enabled(cx, !writing);
        }
        for id in [
            ids!(edit_column),
            ids!(column_name),
            ids!(column_wip),
            ids!(column_done),
            ids!(column_limit_enabled),
        ] {
            self.view.widget(cx, id).set_disabled(cx, writing);
        }
        self.sync_column_actions(cx);
        for id in [
            ids!(checklist_new),
            ids!(checklist_add),
            ids!(unlink_parent),
        ] {
            self.view.widget(cx, id).set_disabled(cx, !editable);
        }
        self.view
            .button(cx, ids!(checklist_add))
            .set_board_enabled(cx, editable);
        self.view
            .button(cx, ids!(unlink_parent))
            .set_board_enabled(cx, editable);
        self.view
            .button(cx, ids!(add_subtask))
            .set_board_enabled(cx, editable && self.subtasks.len() < 100);
        self.view.button(cx, ids!(add_subtask)).set_text(
            cx,
            tr(if self.dirty {
                "Save and add subtask"
            } else {
                "Add subtask"
            }),
        );
        self.view
            .button(cx, ids!(open_parent))
            .set_board_enabled(cx, !writing && !self.dirty);
    }
    pub(super) fn prepare_extension_draw(&mut self, cx: &mut Cx) {
        let count = self.draft_checklist.len();
        let done = self.draft_checklist.iter().filter(|i| i.checked).count();
        self.view
            .label(cx, ids!(checklist_progress))
            .set_text(cx, &format!("{done}/{count}"));
        let mut list = self.view.portal_list(cx, ids!(checklist_items));
        script_apply_eval!(cx,list,{height:#(Size::Fixed((count.min(5)*46).max(1) as f64))});
        let mut list = self.view.portal_list(cx, ids!(subtask_items));
        script_apply_eval!(cx,list,{height:#(Size::Fixed((self.subtasks.len().min(4)*62).max(1) as f64))});
        self.view.view(cx, ids!(subtasks_section)).set_visible(
            cx,
            !self.creating
                && self
                    .selected
                    .as_ref()
                    .is_some_and(|t| t.parent_id.is_none()),
        );
        let done = self.selected.as_ref().map_or(0, |t| t.subtask_done);
        let total = self.selected.as_ref().map_or(0, |t| t.subtask_total);
        self.view
            .label(cx, ids!(subtask_progress))
            .set_text(cx, &format!("{done}/{total}"));
        self.view
            .view(cx, ids!(parent_link))
            .set_visible(cx, self.draft_parent.is_some());
        let number = self
            .parent_task
            .as_ref()
            .map(|t| t.number)
            .or_else(|| self.selected.as_ref().and_then(|t| t.parent_number));
        self.view.button(cx, ids!(open_parent)).set_text(
            cx,
            &format!(
                "{} #{}",
                tr("Parent task"),
                number.map(|n| n.to_string()).unwrap_or_default()
            ),
        );
    }
    pub(super) fn draw_extension_list(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        step: &WidgetRef,
    ) -> bool {
        if self.draw_column_list(cx, scope, step) { return true; }
        let checklist = step.widget_uid()
            == self
                .view
                .portal_list(cx, ids!(checklist_items))
                .widget_uid();
        let children =
            step.widget_uid() == self.view.portal_list(cx, ids!(subtask_items)).widget_uid();
        if !checklist && !children {
            return false;
        }
        let Some(mut list) = step.borrow_mut::<PortalList>() else {
            return true;
        };
        let editable = (self.creating || self.detail_loaded)
            && self.pending.is_none()
            && !self.conflict
            && !self
                .selected
                .as_ref()
                .is_some_and(|t| t.archived_at.is_some());
        let count = if checklist {
            self.draft_checklist.len()
        } else {
            self.subtasks.len()
        };
        if list.first_id() >= count {
            list.set_first_id_and_scroll(0, 0.0);
        }
        list.set_item_range(cx, 0, count);
        if checklist {
            self.checklist_rows.clear();
        } else {
            self.subtask_rows.clear();
        }
        while let Some(i) = list.next_visible_item(cx) {
            let row = list.item(cx, i, id!(Item));
            if checklist {
                if let Some(item) = self.draft_checklist.get(i) {
                    row.check_box(cx, ids!(checked))
                        .set_active(cx, item.checked, Animate::No);
                    let input = row.text_input(cx, ids!(item_title));
                    if input.text() != item.title {
                        input.set_text(cx, &item.title);
                    }
                    for id in [
                        ids!(checked),
                        ids!(item_title),
                        ids!(item_up),
                        ids!(item_remove),
                    ] {
                        row.widget(cx, id).set_disabled(cx, !editable);
                    }
                    row.button(cx, ids!(item_up))
                        .set_board_enabled(cx, editable && i > 0);
                    row.button(cx, ids!(item_remove))
                        .set_board_enabled(cx, editable);
                    self.checklist_rows.insert(i, row.clone());
                }
            } else if let Some(task) = self.subtasks.get(i) {
                row.button(cx, ids!(open_child))
                    .set_text(cx, &format!("#{} {}", task.number, task.title));
                row.button(cx, ids!(open_child))
                    .set_board_enabled(cx, self.pending.is_none() && !self.dirty);
                let state = self
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.lanes.iter().find(|l| l.state == task.state))
                    .map(|l| column_title(&l.title))
                    .unwrap_or_default();
                let suffix = if task.archived_at.is_some() {
                    tr("Archived tasks")
                } else {
                    task.assignee_mxid
                        .as_deref()
                        .map(|id| self.member_name(id))
                        .unwrap_or_else(|| tr("Unassigned"))
                };
                row.label(cx, ids!(child_status))
                    .set_text(cx, &format!("{state} · {suffix}"));
                self.subtask_rows.insert(i, row.clone());
            }
            row.draw_all(cx, scope);
        }
        true
    }
}
