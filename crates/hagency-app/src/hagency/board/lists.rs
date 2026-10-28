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
        if self.view.view(cx, ids!(inspector)).visible()
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
