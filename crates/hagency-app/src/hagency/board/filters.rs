//! Keep applied filters visible and newly created cards discoverable.
use super::*;
impl ProjectBoard {
    pub(super) fn clear_filters(&mut self, cx: &mut Cx) {
        self.query = Query::default();
        for id in [ids!(search), ids!(filter_label)] {
            self.view.text_input(cx, id).set_text(cx, "");
        }
        for id in [
            ids!(filter_state),
            ids!(filter_assignee),
            ids!(filter_label_choice),
        ] {
            self.view.drop_down(cx, id).set_selected_item(cx, 0);
        }
        for id in [ids!(overdue), ids!(archived)] {
            self.view
                .check_box(cx, id)
                .set_active(cx, false, Animate::No);
        }
        self.reset_viewports(cx);
    }
    pub(super) fn matches_filters(&self, task: &BoardTask) -> bool {
        self.query.state.as_ref().is_none_or(|s| *s == task.state)
            && self
                .query
                .assignee
                .as_ref()
                .is_none_or(|s| task.assignee_mxid.as_ref() == Some(s))
            && self
                .query
                .label
                .as_ref()
                .is_none_or(|s| task.labels.contains(s))
            && self
                .query
                .query
                .as_ref()
                .is_none_or(|q| task.title.to_lowercase().contains(&q.to_lowercase()))
            && (!self.query.overdue
                || (task
                    .due_date
                    .as_ref()
                    .is_some_and(|d| d < &chrono::Utc::now().format("%Y-%m-%d").to_string())
                    && !self.snapshot.as_ref().is_some_and(|s| {
                        s.lanes.iter().any(|l| l.state == task.state && l.is_done)
                    })))
            && self.query.archived == task.archived_at.is_some()
    }
    pub(super) fn sync_filter_feedback(&self, cx: &mut Cx) {
        let mut parts = Vec::new();
        if let Some(state) = &self.query.state {
            parts.push(
                self.snapshot
                    .as_ref()
                    .and_then(|s| s.lanes.iter().find(|l| l.state == *state))
                    .map(|l| column_title(&l.title))
                    .unwrap_or_else(|| state.clone()),
            );
        }
        if let Some(member) = &self.query.assignee {
            parts.push(format!("{}: {}", tr("Assignee"), self.member_name(member)));
        }
        if let Some(label) = &self.query.label {
            parts.push(format!("{}: {label}", tr("Label")));
        }
        if let Some(query) = &self.query.query {
            parts.push(format!("{}: {query}", tr("Search")));
        }
        if self.query.overdue {
            parts.push(tr("Overdue").into());
        }
        if self.query.archived {
            parts.push(tr("Archived tasks").into());
        }
        self.view.label(cx, ids!(filter_summary)).set_text(
            cx,
            &format!("{} · {}", tr("Active filters"), parts.join(" · ")),
        );
        self.view
            .label(cx, ids!(filter_summary))
            .set_visible(cx, !parts.is_empty());
        self.view
            .view(cx, ids!(created_feedback))
            .set_visible(cx, self.created_card.is_some());
        if let Some(task) = &self.created_card {
            let suffix = if self.created_filters_cleared {
                tr("Filters cleared to show the new card.")
            } else {
                ""
            };
            self.view.label(cx, ids!(created_message)).set_text(
                cx,
                &format!(
                    "{} #{} · {} {suffix}",
                    tr("Card created"),
                    task.number,
                    task.title
                ),
            );
        }
    }
}

impl ProjectBoard {
    pub(super) fn handle_filters(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.view.button(cx,ids!(filters_toggle)).clicked(actions) {
            let panel=self.view.view(cx,ids!(filters_panel));
            panel.set_visible(cx,!panel.visible());
            if panel.visible() {self.view.text_input(cx,ids!(search)).set_key_focus(cx);}
        }
        if self.view.button(cx,ids!(filters_close)).clicked(actions) {self.view.view(cx,ids!(filters_panel)).set_visible(cx,false);}

        if self.view.button(cx, ids!(clear_filters)).clicked(actions) && !self.loading {
            self.clear_filters(cx);
            self.load(cx);
        }
        if self.view.button(cx, ids!(open_created)).clicked(actions)
            && let Some(task) = self.created_card.clone()
        {
            self.select(cx, &task);
            if self
                .selected
                .as_ref()
                .is_some_and(|selected| selected.id == task.id)
            {
                self.created_card = None;
                self.created_reveal_pending = false;
            }
        }
        let label_choice = self
            .view
            .drop_down(cx, ids!(filter_label_choice))
            .changed(actions);
        if let Some(label) = label_choice
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| self.label_options.get(i))
            .cloned()
        {
            self.view
                .text_input(cx, ids!(filter_label))
                .set_text(cx, &label);
        }
        if self.view.button(cx, ids!(refresh)).clicked(actions) {
            self.failed = false;
            self.load(cx);
        }
        if self.view.button(cx, ids!(retry)).clicked(actions)
            || self.view.button(cx, ids!(dialog_retry)).clicked(actions)
            || self.view.button(cx, ids!(columns_retry)).clicked(actions)
        {
            self.retry_write(cx);
        }
        if self.view.button(cx, ids!(apply_filters)).clicked(actions)
            || label_choice.is_some_and(|i| i > 0)
            || self
                .view
                .text_input(cx, ids!(filter_label))
                .returned(actions)
                .is_some()
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
                let query = Query {
                    state: self
                        .view
                        .drop_down(cx, ids!(filter_state))
                        .selected_item()
                        .checked_sub(1)
                        .map(|i| self.status_state(i)),
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
                if query.validate().is_ok() {
                    self.query = query;
                    if self.view.button(cx,ids!(apply_filters)).clicked(actions) || self.view.text_input(cx,ids!(search)).returned(actions).is_some() {self.view.view(cx,ids!(filters_panel)).set_visible(cx,false);}
                    self.created_card = None;
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
    }
}

impl ProjectBoard {
    pub(super) fn reveal_created_card(&mut self, cx: &mut Cx) {
        if !self.created_reveal_pending {
            return;
        }
        let Some(task) = &self.created_card else {
            return;
        };
        let target = self.snapshot.as_ref().and_then(|s| {
            s.lanes.iter().enumerate().find_map(|(lane, l)| {
                l.tasks
                    .iter()
                    .position(|t| t.id == task.id)
                    .map(|index| (lane, index))
            })
        });
        // Off-page cards remain directly accessible through the confirmed receipt.
        self.created_reveal_pending = false;
        if let Some((lane, index)) = target {
            self.view
                .view(cx, column_path(lane))
                .portal_list(cx, ids!(cards))
                .set_first_id_and_scroll(index, 0.0);
            let visible = self.snapshot.as_ref().map_or(0, |s| {
                s.lanes[..lane].iter().filter(|l| !l.archived).count()
            });
            let left = visible as f64 * 292.0;
            let viewport = self.view.view(cx, ids!(board_scroll)).area().rect(cx);
            let column = self.view.view(cx, column_path(lane)).area().rect(cx);
            if viewport.size.x > 0.0 && column.size.x > 0.0 {
                // Keep a visible destination in place. Only reveal the clipped edge;
                // aligning every new card to the left makes columns jump after Add.
                if column.pos.x < viewport.pos.x {
                    self.scroll_x = left;
                } else if column.pos.x + column.size.x > viewport.pos.x + viewport.size.x {
                    self.scroll_x = (left + column.size.x - viewport.size.x).max(0.0);
                } else {
                    return;
                }
            } else {
                self.scroll_x = left;
            }
            self.view
                .view(cx, ids!(board_scroll))
                .set_scroll_pos(cx, dvec2(self.scroll_x, 0.0));
        }
    }
}

impl ProjectBoard {
    pub(super) fn toolbar_tooltips(&self,cx:&mut Cx,event:&Event) {
        if !matches!(event,Event::MouseMove(_)) {return;}
        for (id,text) in [(ids!(filters_toggle).as_slice(),"Search and filter"),(ids!(filters_close).as_slice(),"Close filters"),(ids!(notifications_button).as_slice(),"Notifications"),(ids!(refresh).as_slice(),"Refresh"),(ids!(manage_columns).as_slice(),"Manage columns")] {
            let button=self.view.button(cx,id);
            if !button.visible() || button.area().rect(cx).size.x<=0.0 {continue;}
            match event.hits(cx,button.area()) {
                Hit::FingerHoverIn(_)=>cx.widget_action(button.widget_uid(),TooltipAction::HoverIn {widget_rect:button.area().rect(cx),text:tr(text).into(),options:CalloutTooltipOptions {position:TooltipPosition::Bottom,..Default::default()}}),
                Hit::FingerHoverOut(_)=>cx.widget_action(button.widget_uid(),TooltipAction::HoverOut),_=>()
            }
        }
    }
}
