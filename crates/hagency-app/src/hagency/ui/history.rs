use super::*;
fn phase_label(phase: &str) -> &'static str {
    match phase {
        "offered" => "Awaiting owner decision",
        "owner_allowed" => "Owner allowed",
        "owner_denied" => "Owner denied",
        "policy_allowed" => "Policy allowed",
        "policy_denied" => "Policy denied",
        "applying" => "Applying permission",
        "submitted" => "Submitted to provider",
        "completed" => "Matching completion observed",
        "failed" => "Tool returned failure",
        "expired" => "Expired or no longer applicable",
        _ => "Execution result unknown",
    }
}

fn observed(value: &Value) -> String {
    value
        .as_i64()
        .and_then(|t| chrono::DateTime::<chrono::Utc>::from_timestamp(t, 0))
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}
impl HagencyPanel {
    pub(super) fn render_history_controls(&self, cx: &mut Cx) {
        let current = self
            .agent(cx)
            .is_some_and(|a| self.history_page["agentId"] == a);
        self.view.button(cx, ids!(history_older)).set_enabled(
            cx,
            !self.busy
                && current
                && self.history_page["approvals"]["nextCursor"]
                    .as_str()
                    .is_some(),
        );
        let selected = self.history_detail["approval"]["summary"]["id"].as_str()
            == Some(self.history_selected.as_str())
            && self
                .agent(cx)
                .is_some_and(|a| self.history_detail["agentId"] == a);
        self.view.button(cx, ids!(history_copy)).set_enabled(
            cx,
            !self.busy && selected && !self.history_selected.is_empty(),
        );
    }
    fn load_history(&mut self, cx: &mut Cx) {
        let Some(agent) = self.agent(cx) else {
            return;
        };
        self.history_selected.clear();
        self.history_detail = Value::Null;
        self.view.label(cx, ids!(history_details)).set_text(cx, "");
        self.view.label(cx, ids!(history_proposal)).set_text(cx, "");
        self.run(
            cx,
            Target::History,
            Command::AgentHistory {
                action: HistoryAction::Approvals {
                    agent,
                    cursor: self.history_cursor.clone(),
                },
            },
        );
    }
    pub(super) fn apply_history(&mut self, cx: &mut Cx, target: Target, value: &Value) {
        if !self.agent(cx).is_some_and(|a| value["agentId"] == a) {
            return;
        }
        if matches!(target, Target::History) {
            self.history_page = value.clone();
            self.history_detail = Value::Null;
            self.history_selected.clear();
            let records = array(&value["approvals"], "records");
            let mut labels = vec![crate::i18n::tr("Select an approval record").to_owned()];
            labels.extend(records.iter().map(|r| {
                format!(
                    "{} · {} · {}",
                    observed(&r["createdAt"]),
                    string(r, "tool"),
                    crate::i18n::tr(phase_label(string(r, "phase")))
                )
            }));
            self.view
                .drop_down(cx, ids!(history_records))
                .set_labels(cx, labels);
            self.view
                .drop_down(cx, ids!(history_records))
                .set_selected_item(cx, 0);
            self.view.label(cx, ids!(history_details)).set_text(
                cx,
                if records.is_empty() {
                    crate::i18n::tr("No approval evidence is recorded on this device.")
                } else {
                    ""
                },
            );
        } else {
            let record = &value["approval"];
            if record["summary"]["id"].as_str() != Some(self.history_selected.as_str()) {
                return;
            }
            self.history_detail = value.clone();
            let proposal = &record["proposal"];
            let scope = &proposal["scope"];
            let heading = crate::i18n::format(
                "{phase}\nRoom: {room}\nRequester: {requester}\nThread: {thread}\nDispatch: {dispatch}\nDirectory: {directory}\nProposal SHA256: {digest}",
                &[
                    (
                        "phase",
                        crate::i18n::tr(phase_label(string(&record["summary"], "phase"))).into(),
                    ),
                    ("room", string(scope, "room").into()),
                    ("requester", string(scope, "requester").into()),
                    ("thread", string(scope, "thread").into()),
                    ("dispatch", string(proposal, "dispatch").into()),
                    ("directory", string(proposal, "canonical_directory").into()),
                    (
                        "digest",
                        string(&record["summary"], "proposalDigest").into(),
                    ),
                ],
            );
            let events = array(record, "events")
                .iter()
                .map(|e| {
                    format!(
                        "{} · {}\n{}",
                        observed(&e["observedAt"]),
                        crate::i18n::tr(phase_label(string(e, "phase"))),
                        e["detail"]
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            self.view
                .label(cx, ids!(history_details))
                .set_text(cx, &format!("{heading}\n\n{events}"));
            let encoded = serde_json::to_string_pretty(proposal).unwrap_or_default();
            self.view
                .label(cx, ids!(history_proposal))
                .set_text(cx, &encoded.chars().take(16384).collect::<String>());
        }
        self.status(cx, "");
        self.render_history_controls(cx);
    }
    pub(super) fn history_actions(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        if self.page != Page::Agents || !self.view.view(cx, ids!(agent_detail)).visible() {
            return false;
        }
        if self.view.button(cx, ids!(history_tab)).clicked(actions) {
            self.section(cx, ids!(history_section));
            self.history_cursor = None;
            self.load_history(cx);
            return true;
        }
        if !self.view.view(cx, ids!(history_section)).visible() {
            return false;
        }
        if self.view.button(cx, ids!(history_refresh)).clicked(actions) {
            self.history_cursor = None;
            self.load_history(cx);
            return true;
        }
        if self.view.button(cx, ids!(history_older)).clicked(actions)
            && self
                .agent(cx)
                .is_some_and(|a| self.history_page["agentId"] == a)
        {
            self.history_cursor = self.history_page["approvals"]["nextCursor"]
                .as_str()
                .map(str::to_owned);
            self.load_history(cx);
            return true;
        }
        if self
            .view
            .drop_down(cx, ids!(history_records))
            .selected(actions)
            .is_some()
        {
            self.history_selected.clear();
            self.history_detail = Value::Null;
            self.view.label(cx, ids!(history_details)).set_text(cx, "");
            self.view.label(cx, ids!(history_proposal)).set_text(cx, "");
            if self
                .agent(cx)
                .is_some_and(|a| self.history_page["agentId"] == a)
                && let Some(record) = self.selected(
                    cx,
                    ids!(history_records),
                    &array(&self.history_page["approvals"], "records"),
                )
                && let Some(agent) = self.agent(cx)
            {
                self.history_selected = string(&record, "id").into();
                self.run(
                    cx,
                    Target::HistoryDetail,
                    Command::AgentHistory {
                        action: HistoryAction::Approval {
                            agent,
                            id: self.history_selected.clone(),
                        },
                    },
                );
            }
            self.render_history_controls(cx);
            return true;
        }
        if self.view.button(cx, ids!(history_copy)).clicked(actions)
            && self.history_detail["approval"]["summary"]["id"].as_str()
                == Some(self.history_selected.as_str())
            && self
                .agent(cx)
                .is_some_and(|a| self.history_detail["agentId"] == a)
        {
            if let Ok(proposal) =
                serde_json::to_string_pretty(&self.history_detail["approval"]["proposal"])
            {
                cx.copy_to_clipboard(&proposal);
            }
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_agent_and_record_fence_history_and_full_proposal_copy() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut p = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm,{mod.widgets.HagencyPanel {}});
            let p = HagencyPanel::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty());
            p
        });
        p.set_page(&mut cx, Page::Agents);
        p.agents = vec![json!({"id":"agent"}), json!({"id":"other"})];
        p.labels(&mut cx, ids!(agents), "Select", &p.agents, "id");
        p.view
            .drop_down(&cx, ids!(agents))
            .set_selected_item(&mut cx, 1);
        let page = json!({"agentId":"agent","approvals":{"records":[{"id":"approval","phase":"submitted","tool":"codex.command","createdAt":100}],"nextCursor":"older"}});
        p.apply_history(&mut cx, Target::History, &page);
        assert!(
            p.view
                .button(&cx, ids!(history_older))
                .borrow()
                .unwrap()
                .enabled()
        );
        p.history_selected = "approval".into();
        let detail = json!({"agentId":"agent","approval":{"summary":{"id":"approval","phase":"submitted","proposalDigest":"digest"},"proposal":{"scope":{"room":"!r:test","requester":"@a:test","thread":"thread"},"dispatch":"dispatch","canonical_directory":"/workspace","arguments":{"command":"汉".repeat(20000)}},"events":[{"phase":"owner_allowed","observedAt":100,"detail":{"deviceId":"original-device"}},{"phase":"submitted","observedAt":101,"detail":{}}]}});
        p.apply_history(&mut cx, Target::HistoryDetail, &detail);
        assert_eq!(p.history_detail, detail);
        assert_eq!(
            p.view
                .label(&cx, ids!(history_proposal))
                .text()
                .chars()
                .count(),
            16384
        );
        assert!(
            p.view
                .label(&cx, ids!(history_details))
                .text()
                .contains("original-device")
        );
        assert!(
            !p.view
                .label(&cx, ids!(history_details))
                .text()
                .contains(crate::i18n::tr(phase_label("completed")))
        );
        assert!(
            p.view
                .button(&cx, ids!(history_copy))
                .borrow()
                .unwrap()
                .enabled()
        );
        let mut stale = detail.clone();
        stale["approval"]["summary"]["id"] = "old-selection".into();
        p.apply_history(&mut cx, Target::HistoryDetail, &stale);
        assert_eq!(p.history_detail, detail);
        p.view
            .drop_down(&cx, ids!(agents))
            .set_selected_item(&mut cx, 2);
        p.render_history_controls(&mut cx);
        assert!(
            !p.view
                .button(&cx, ids!(history_copy))
                .borrow()
                .unwrap()
                .enabled()
        );
        assert!(
            !p.view
                .button(&cx, ids!(history_older))
                .borrow()
                .unwrap()
                .enabled()
        );
        p.apply_history(&mut cx, Target::History, &page);
        assert_eq!(p.history_detail, detail);
        assert_eq!(phase_label("future_unknown_phase"), phase_label("unknown"));
    }
}
