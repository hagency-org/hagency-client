//! Column-local title-only creation uses the same validated, idempotent write path.
use super::*;

fn minimal_fields(title: String) -> Result<Fields, &'static str> {
    let mut fields = Fields {
        title,
        ..Fields::default()
    };
    fields.validate().map_err(|e| e.0)?;
    Ok(fields)
}
fn title_only(title: String, state: String) -> Result<Create, &'static str> {
    Ok(Create {
        idempotency_key: uuid::Uuid::new_v4().to_string(),
        fields: minimal_fields(title)?,
        state,
    })
}

impl ProjectBoard {
    pub(super) fn clear_quick_create(&mut self, cx: &mut Cx) {
        self.quick_create_state = None;
        self.quick_save_key = None;
        self.quick_focus = false;
        for index in 0 .. 12 {
            let column = self.view.view(cx, column_path(index));
            column.text_input(cx, ids!(quick_title)).set_text(cx, "");
            column.view(cx, ids!(quick_form)).set_visible(cx, false);
            column.button(cx, ids!(quick_add)).set_visible(cx, true);
        }
    }

    fn begin_quick_create(&mut self, cx: &mut Cx, index: usize) {
        if !self.can_quick_create() || self.quick_create_state.is_some() {
            return;
        }
        self.quick_create_state = Some(self.column_state(index));
        self.quick_focus = true;
        let column = self.view.view(cx, column_path(index));
        column.view(cx, ids!(quick_form)).set_visible(cx, true);
        self.notice(cx, "");
    }

    fn can_quick_create(&self) -> bool {
        self.snapshot.is_some()
            && self.pending.is_none()
            && !self.failed
            && !self.dirty
            && self.columns_draft.is_none()
            && !self.query.archived
    }

    pub(super) fn sync_quick_create(&self, cx: &mut Cx) {
        for index in 0 .. self.column_count() {
            let column = self.view.view(cx, column_path(index));
            let open = self.quick_create_state.as_deref() == Some(&self.column_state(index));
            column.view(cx, ids!(quick_form)).set_visible(cx, open);
            column.button(cx, ids!(quick_add)).set_visible(cx, !open);
            column.button(cx, ids!(quick_add)).set_board_enabled(
                cx,
                self.can_quick_create() && self.quick_create_state.is_none(),
            );
            column
                .button(cx, ids!(quick_cancel))
                .set_board_enabled(cx, self.pending.is_none());
            let input = column.text_input(cx, ids!(quick_title));
            let enabled = self.can_quick_create();
            if input.is_read_only() == enabled {
                input.set_is_read_only(cx, !enabled);
            }
            let valid = minimal_fields(input.text()).is_ok();
            column
                .button(cx, ids!(quick_save))
                .set_board_enabled(cx, enabled && valid);
        }
    }

    pub(super) fn handle_quick_create(&mut self, cx: &mut Cx, actions: &Actions) {
        for index in 0 .. self.column_count() {
            let column = self.view.view(cx, column_path(index));
            if column.button(cx, ids!(quick_add)).clicked(actions)
                || column.button(cx, ids!(add)).clicked(actions)
            {
                self.begin_quick_create(cx, index);
            }
            if self.quick_create_state.as_deref() != Some(&self.column_state(index)) {
                continue;
            }
            if column.button(cx, ids!(quick_cancel)).clicked(actions) && self.pending.is_none() {
                self.clear_quick_create(cx);
                continue;
            }
            let input = column.text_input(cx, ids!(quick_title));
            if self.can_quick_create()
                && !input.is_composing()
                && (column.button(cx, ids!(quick_save)).clicked(actions)
                    || input.returned(actions).is_some())
            {
                match title_only(input.text(), self.column_state(index)) {
                    Ok(input) => {
                        self.quick_save_key = Some(input.idempotency_key.clone());
                        self.write(cx, BoardAction::Create { input });
                    }
                    Err(code) => self.notice(cx, error_message(code)),
                }
            }
        }
    }

    pub(super) fn is_quick_save(&self) -> bool {
        matches!(&self.pending, Some(BoardAction::Create { input })
            if self.quick_save_key.as_deref() == Some(&input.idempotency_key))
    }

    pub(super) fn focus_quick_create(&mut self, cx: &mut Cx) {
        if self.quick_focus
            && let Some(index) = (0 .. self.column_count())
                .find(|i| self.quick_create_state.as_deref() == Some(&self.column_state(*i)))
        {
            let input = self
                .view
                .view(cx, column_path(index))
                .text_input(cx, ids!(quick_title));
            if !input.area().is_empty() {
                self.quick_focus = false;
                input.take_key_focus(cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quick_card_has_only_title_and_column_with_no_optional_metadata() {
        let card = title_only("  简单卡片  ".into(), "custom_lane".into()).unwrap();
        assert_eq!(card.state, "custom_lane");
        assert_eq!(card.fields.title, "简单卡片");
        assert!(card.fields.description.is_empty() && card.fields.labels.is_empty());
        assert!(card.fields.assignee_mxid.is_none() && card.fields.due_date.is_none());
        assert!(card.fields.parent_id.is_none() && card.fields.checklist.is_empty());
        assert_eq!(card.fields.priority, "normal");
        assert!(title_only("   ".into(), "todo".into()).is_err());
        assert!(title_only("a".repeat(241), "todo".into()).is_err());
    }
}
