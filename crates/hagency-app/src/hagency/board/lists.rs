use super::*;

impl ProjectBoard {
    pub(super) fn append_column(&mut self, cx: &mut Cx) {
        let Some(columns) = self.columns_draft.as_mut().filter(|c| c.len() < 12) else {
            return;
        };
        let title = (1..)
            .map(|i| format!("{} {i}", tr("New column")))
            .find(|title| columns.iter().all(|c| c.title != *title))
            .unwrap();
        columns.push(ColumnDefinition {
            archived: false,
            id: format!("col_{}", uuid::Uuid::new_v4().simple()),
            title,
            wip_limit: None,
            is_done: false,
        });
        self.column_edit = columns.len() - 1;
        self.show_column_form(cx);
        cx.set_key_focus(self.view.text_input(cx, ids!(column_name)).area());
    }

    pub(super) fn reorder_column(&mut self, cx: &mut Cx, from: usize, to: usize) {
        if from == to || self.pending.is_some() || self.dirty || self.columns_draft.is_some() {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let mut columns: Vec<_> = snapshot
            .lanes
            .iter()
            .map(|lane| ColumnDefinition {
                id: lane.state.clone(),
                title: lane.title.clone(),
                wip_limit: lane.wip_limit,
                is_done: lane.is_done,
                archived: lane.archived,
            })
            .collect();
        if from >= columns.len() || to >= columns.len() {
            return;
        }
        let column = columns.remove(from);
        columns.insert(to, column);
        self.write(
            cx,
            BoardAction::ConfigureColumns {
                input: ConfigureColumns {
                    idempotency_key: uuid::Uuid::new_v4().to_string(),
                    expected_sequence: snapshot.sequence,
                    columns,
                },
            },
        );
    }

    pub(super) fn handle_column_drag(&mut self, cx: &mut Cx, event: &Event) {
        if matches!(
            event,
            Event::WindowLostFocus(_)
                | Event::KeyDown(KeyEvent {
                    key_code: KeyCode::Escape,
                    ..
                })
        ) {
            self.column_drag = None;
        }
        if self.modal_visible(cx)
            || self.columns_draft.is_some()
            || self.pending.is_some()
            || self.dirty
            || self.drag.is_some()
            || self.quick_create_state.is_some()
        {
            return;
        }
        for index in 0..self.column_count() {
            let column = self.view.view(cx, column_path(index));
            if !column.visible() {
                continue;
            }
            let grip = column.view(cx, ids!(column_grip)).area();
            match event.hits(cx, grip) {
                Hit::FingerDown(e) => self.column_drag = Some((index, e.abs)),
                Hit::FingerMove(e) if self.column_drag.is_some() => {
                    let rect = self.view.view(cx, ids!(board_scroll)).area().rect(cx);
                    let extent = (self
                        .snapshot
                        .as_ref()
                        .map_or(0, |s| s.lanes.iter().filter(|c| !c.archived).count())
                        as f64
                        * 292.0
                        - rect.size.x)
                        .max(0.0);
                    let step = if e.abs.x > rect.pos.x + rect.size.x - 40.0 {
                        24.0
                    } else if e.abs.x < rect.pos.x + 40.0 {
                        -24.0
                    } else {
                        0.0
                    };
                    self.scroll_x = (self.scroll_x + step).clamp(0.0, extent);
                    self.view
                        .view(cx, ids!(board_scroll))
                        .set_scroll_pos(cx, dvec2(self.scroll_x, 0.0));
                    cx.set_cursor(MouseCursor::Hand);
                }
                Hit::FingerUp(e) => {
                    if let Some((from, start)) = self.column_drag.take()
                        && (e.abs - start).length() > 8.0
                    {
                        let target = (0..self.column_count())
                            .filter(|i| self.view.view(cx, column_path(*i)).visible())
                            .min_by(|a, b| {
                                let distance = |i| {
                                    let r = self.view.view(cx, column_path(i)).area().rect(cx);
                                    (r.pos.x + r.size.x * 0.5 - e.abs.x).abs()
                                };
                                distance(*a).total_cmp(&distance(*b))
                            });
                        if let Some(to) = target {
                            self.reorder_column(cx, from, to);
                        }
                    }
                }
                _ => (),
            }
        }
    }
}

impl ProjectBoard {
    pub(super) fn columns_notice(&self, cx: &mut Cx, message: &str) {
        self.view.label(cx, ids!(columns_notice)).set_text(cx, message);
        self.view.label(cx, ids!(columns_notice)).set_visible(cx, !message.is_empty());
    }
    pub(super) fn sync_column_actions(&self, cx: &mut Cx) {
        let Some(columns) = &self.columns_draft else { return; };
        let Some(column) = columns.get(self.column_edit) else { return; };
        let stored = self.snapshot.as_ref().and_then(|s| s.lanes.iter().find(|l| l.state == column.id));
        let empty = stored.is_none_or(|l| l.all_task_count == Some(0));
        let unlocked = self.pending.is_none();
        self.view.button(cx, ids!(remove_column)).set_board_enabled(cx, unlocked && empty && columns.len() > 2);
        self.view.button(cx, ids!(add_column)).set_board_enabled(cx, unlocked && columns.len() < 12);
        self.view.button(cx, ids!(column_left)).set_board_enabled(cx, unlocked && self.column_edit > 0);
        self.view.button(cx, ids!(column_right)).set_board_enabled(cx, unlocked && self.column_edit + 1 < columns.len());
    }
    pub(super) fn request_close_columns(&mut self, cx: &mut Cx) {
        if self.pending.is_some() {
            self.columns_notice(cx, tr("A previous save is unconfirmed. Retry its original request to avoid duplicates."));
            return;
        }
        // Capture valid values, but an invalid field must never trap Cancel.
        let valid = self.capture_column_form(cx);
        let original = self.snapshot.as_ref().map(|s| s.lanes.iter().map(|l| ColumnDefinition {
            id:l.state.clone(), title:l.title.clone(), wip_limit:l.wip_limit, is_done:l.is_done, archived:l.archived
        }).collect::<Vec<_>>());
        let changed = !valid || serde_json::to_value(&self.columns_draft).ok() != serde_json::to_value(original).ok();
        if changed && !self.discard_armed {
            self.discard_armed = true;
            self.view.button(cx, ids!(cancel_columns)).set_text(cx, tr("Discard changes"));
            self.columns_notice(cx, tr("You have unsaved column changes. Save them, or choose Discard changes."));
        } else { self.close_column_settings(cx); }
    }
    pub(super) fn draw_column_list(&mut self, cx: &mut Cx2d, scope: &mut Scope, step: &WidgetRef) -> bool {
        if step.widget_uid() != self.view.portal_list(cx, ids!(column_list)).widget_uid() { return false; }
        let Some(mut list) = step.borrow_mut::<PortalList>() else { return true; };
        let Some(columns) = &self.columns_draft else { return true; };
        self.column_rows.clear();
        list.set_item_range(cx, 0, columns.len());
        while let Some(i) = list.next_visible_item(cx) {
            let Some(column) = columns.get(i) else { continue; };
            let row = list.item(cx, i, id!(Item));
            let mut button = row.button(cx, ids!(choose_column));
            let key = format!("{column:?}:{}:{}", i == self.column_edit, self.pending.is_some());
            if self.column_paint_keys.get(&button.widget_uid()) != Some(&key) {
                self.column_paint_keys.insert(button.widget_uid(), key);
                button.set_text(cx, &format!("{}. {}", i + 1, column_title(&column.title)));
                if i == self.column_edit {
                    script_apply_eval!(cx, button, {draw_text.color:mod.widgets.RINX_ACCENT draw_bg.color:mod.widgets.RINX_SELECTED});
                } else {
                    script_apply_eval!(cx, button, {draw_text.color:mod.widgets.RINX_INK draw_bg.color:#0000});
                }
                button.set_board_enabled(cx, self.pending.is_none());
                let mut parts = Vec::new();
                if column.archived { parts.push(tr("Archived").into()); }
                if column.is_done { parts.push(tr("Done").into()); }
                if let Some(limit) = column.wip_limit { parts.push(format!("{} {limit}", tr("Maximum tasks"))); }
                row.label(cx, ids!(column_summary)).set_text(cx, &parts.join(" · "));
            }
            row.draw_all(cx, scope);
            self.column_rows.insert(i, row);
        }
        true
    }
}
