//! Free-form labels share one parser and project-wide, bounded suggestions.
use super::*;

pub(super) fn parse(text: &str) -> Vec<String> {
    text.split([',', '，', '、', ';', '；'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}
fn valid(labels: &[String]) -> bool {
    labels.len() <= 12
        && labels
            .iter()
            .all(|s| s.chars().count() <= 32 && !s.chars().any(char::is_control))
}
impl ProjectBoard {
    pub(super) fn sync_label_options(&mut self, cx: &mut Cx) {
        let mut options: std::collections::BTreeSet<String> = self
            .snapshot
            .as_ref()
            .map(|s| s.label_suggestions.iter().cloned().collect())
            .unwrap_or_default();
        // Compatibility with older servers, plus labels in just-saved details.
        if let Some(snapshot) = &self.snapshot {
            options.extend(
                snapshot
                    .lanes
                    .iter()
                    .flat_map(|l| &l.tasks)
                    .flat_map(|t| t.labels.clone()),
            );
        }
        if let Some(task) = &self.selected {
            options.extend(task.labels.clone());
        }
        if let Some(label) = &self.query.label {
            options.insert(label.clone());
        }
        self.label_options = options
            .into_iter()
            .filter(|s| !s.is_empty() && s.chars().count() <= 32)
            .collect();
        for (id, placeholder) in [
            (ids!(label_choice), "Add existing label…"),
            (ids!(filter_label_choice), "Existing labels"),
        ] {
            let mut labels = vec![tr(placeholder).to_owned()];
            labels.extend(self.label_options.clone());
            self.view.drop_down(cx, id).set_labels(cx, labels);
            self.view.drop_down(cx, id).set_selected_item(cx, 0);
        }
        let help = if self.snapshot.as_ref().is_some_and(|s| s.has_more_labels) {
            "More labels are available. Enter a name to use or filter any label."
        } else {
            "Separate labels with commas. Remove a name to remove its label. Up to 12 labels, 32 characters each."
        };
        self.view.label(cx, ids!(label_help)).set_text(cx, tr(help));
        self.sync_label_feedback(cx);
    }
    pub(super) fn sync_label_feedback(&self, cx: &mut Cx) {
        let labels = parse(&self.view.text_input(cx, ids!(labels)).text());
        let message = if !valid(&labels) {
            tr("Use up to 12 labels, each with 1–32 characters and no control characters.").into()
        } else if labels.is_empty() {
            tr("No labels").into()
        } else {
            labels.join(" · ")
        };
        self.view
            .label(cx, ids!(label_feedback))
            .set_text(cx, &message);
        self.view.widget(cx, ids!(label_choice)).set_disabled(
            cx,
            self.pending.is_some()
                || self.conflict
                || self.selected_column_archived()
                || self
                    .selected
                    .as_ref()
                    .is_some_and(|t| t.archived_at.is_some())
                || (!self.creating && !self.detail_loaded),
        );
    }
    pub(super) fn handle_labels(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(index) = self.view.drop_down(cx, ids!(label_choice)).changed(actions)
            && let Some(label) = index
                .checked_sub(1)
                .and_then(|i| self.label_options.get(i))
                .cloned()
            && self.pending.is_none()
            && !self.conflict
            && !self.selected_column_archived()
            && !self
                .selected
                .as_ref()
                .is_some_and(|t| t.archived_at.is_some())
            && (self.creating || self.detail_loaded)
        {
            let mut labels = parse(&self.view.text_input(cx, ids!(labels)).text());
            if !labels.contains(&label) {
                labels.push(label);
            }
            if valid(&labels) {
                self.view
                    .text_input(cx, ids!(labels))
                    .set_text(cx, &labels.join(", "));
                self.dirty = true;
                self.discard_armed = false;
            } else {
                self.notice(
                    cx,
                    tr("Use up to 12 labels, each with 1–32 characters and no control characters."),
                );
            }
            self.view
                .drop_down(cx, ids!(label_choice))
                .set_selected_item(cx, 0);
            self.sync_controls(cx);
        }
        if self
            .view
            .text_input(cx, ids!(labels))
            .changed(actions)
            .is_some()
        {
            self.sync_label_feedback(cx);
        }
    }
}
