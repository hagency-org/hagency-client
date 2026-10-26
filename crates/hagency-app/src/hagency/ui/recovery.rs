use super::*;
impl HagencyPanel {
    fn recovery_paths(&self, cx: &mut Cx) -> Vec<String> {
        self.input(cx, ids!(recovery_paths))
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    }
    fn recovery_matches(&self, cx: &mut Cx) -> bool {
        self.agent(cx)
            .is_some_and(|a| self.recovery_review["agentId"] == a)
            && self.recovery_review["review"]["evidence"]["dispatchId"].as_str()
                == Some(self.recovery_selected.as_str())
            && self.recovery_review["review"]["evidence"]["files"]
                .as_array()
                .is_some_and(|files| {
                    files
                        .iter()
                        .filter_map(|f| f["path"].as_str())
                        .collect::<Vec<_>>()
                        == self.recovery_paths(cx)
                })
            && self.recovery_review["runtimeActive"] == false
    }
    fn recovery_input(&self, cx: &mut Cx, choice: usize) -> Option<RecoveryInput> {
        if !self.recovery_matches(cx) {
            return None;
        }
        let note = self.input(cx, ids!(recovery_note));
        let instruction = self.input(cx, ids!(recovery_instruction));
        if note.trim().is_empty()
            || note.len() > 8192
            || choice == 2 && (instruction.trim().is_empty() || instruction.len() > 32768)
        {
            return None;
        }
        let evidence = &self.recovery_review["review"]["evidence"];
        if choice == 2
            && !(evidence["stopScope"] == "whole_tree"
                || evidence["requiresTree"] == false && evidence["stopScope"] == "leader")
        {
            return None;
        }
        let disposition = match choice {
            0 => RecoveryDisposition::KeepBlocked { note },
            1 => RecoveryDisposition::OwnerConfirmed { note },
            _ => RecoveryDisposition::Continue { note, instruction },
        };
        Some(RecoveryInput {
            review_id: self.recovery_review["review"]["id"].as_str()?.into(),
            review_digest: self.recovery_review["review"]["digest"].as_str()?.into(),
            disposition,
        })
    }
    pub(super) fn render_recovery_controls(&self, cx: &mut Cx) {
        let current = self
            .agent(cx)
            .is_some_and(|a| self.stopped_page["agentId"] == a);
        self.view.button(cx, ids!(recovery_older)).set_enabled(
            cx,
            !self.busy
                && current
                && self.stopped_page["stopped"]["nextCursor"]
                    .as_str()
                    .is_some(),
        );
        self.view.button(cx, ids!(recovery_inspect)).set_enabled(
            cx,
            !self.busy
                && current
                && self.stopped_page["runtimeActive"] == false
                && !self.recovery_selected.is_empty(),
        );
        let fresh = self.recovery_review["review"]["expires"]
            .as_i64()
            .is_some_and(|t| t > chrono::Utc::now().timestamp());
        for (id, choice) in [
            (ids!(recovery_keep), 0),
            (ids!(recovery_accept), 1),
            (ids!(recovery_continue), 2),
        ] {
            let enabled = !self.busy && fresh && self.recovery_input(cx, choice).is_some();
            self.view.button(cx, id).set_enabled(cx, enabled);
        }
        let copy = !self.busy
            && self.recovery_matches(cx)
            && !string(&self.recovery_review["review"]["evidence"], "input").is_empty();
        self.view
            .button(cx, ids!(recovery_copy))
            .set_enabled(cx, copy);
        let confirm = !self.busy && self.recovery_matches(cx) && self.recovery_pending.is_some();
        self.view
            .button(cx, ids!(recovery_confirm))
            .set_enabled(cx, confirm);
    }
    fn clear_recovery_review(&mut self, cx: &mut Cx) {
        self.recovery_review = Value::Null;
        self.recovery_pending = None;
        self.view
            .view(cx, ids!(recovery_confirmation))
            .set_visible(cx, false);
        self.view.label(cx, ids!(recovery_details)).set_text(cx, "");
    }
    fn load_stopped(&mut self, cx: &mut Cx) {
        let Some(agent) = self.agent(cx) else {
            return;
        };
        self.recovery_selected.clear();
        self.clear_recovery_review(cx);
        self.run(
            cx,
            Target::Stopped,
            Command::AgentHistory {
                action: HistoryAction::Stopped {
                    agent,
                    cursor: self.recovery_cursor.clone(),
                },
            },
        );
    }
    pub(super) fn apply_recovery(&mut self, cx: &mut Cx, target: Target, value: &Value) {
        if !self.agent(cx).is_some_and(|a| value["agentId"] == a) {
            return;
        }
        match target {
            Target::Stopped => {
                self.stopped_page = value.clone();
                self.recovery_selected.clear();
                self.clear_recovery_review(cx);
                let records = array(&value["stopped"], "records");
                let mut labels = vec![crate::i18n::tr("Select a stopped request").into()];
                labels.extend(records.iter().map(|r| {
                    format!(
                        "{} · {} · {}",
                        string(r, "id"),
                        string(&r["scope"], "room"),
                        string(r, "inputPreview")
                    )
                }));
                self.view
                    .drop_down(cx, ids!(recovery_records))
                    .set_labels(cx, labels);
                self.view
                    .drop_down(cx, ids!(recovery_records))
                    .set_selected_item(cx, 0);
                self.status(cx,if value["runtimeActive"]==true{"Stop this Agent's services before inspecting or deciding a stopped request."}else if records.is_empty(){"No stopped requests are recorded on this device."}else{""});
            }
            Target::RecoveryReview => {
                if value["review"]["evidence"]["dispatchId"].as_str()
                    != Some(self.recovery_selected.as_str())
                {
                    return;
                }
                self.recovery_review = value.clone();
                self.recovery_pending = None;
                let review = &value["review"];
                let e = &review["evidence"];
                let input = if string(e, "input").is_empty() {
                    crate::i18n::tr("The original input was compacted in an older ledger. Its identity digest is retained.").to_owned()
                } else {
                    string(e, "input").chars().take(16384).collect()
                };
                let workspace = if e["workspace"].is_null() {
                    crate::i18n::tr(
                        "The original workspace was not recorded. No file digest can be inferred.",
                    )
                } else {
                    string(e, "workspace")
                };
                let details = crate::i18n::format(
                    "Original result: {state}\nRoom: {room}\nRequester: {requester}\nThread: {thread}\nExecution: {execution}\nIdentity SHA256: {digest}\nWorkspace: {workspace}\nStop evidence: {stop}\n\nOriginal input (up to 16,384 characters):\n{input}\n\nRetained reply: {reply}\n\nCurrent file observations:\n{files}\n\nOwner dispositions (newest 100):\n{actions}",
                    &[
                        ("state", string(e, "state").into()),
                        ("room", string(&e["scope"], "room").into()),
                        ("requester", string(&e["scope"], "requester").into()),
                        ("thread", string(&e["scope"], "thread").into()),
                        ("execution", string(e, "executionId").into()),
                        ("digest", string(e, "immutableDigest").into()),
                        ("workspace", workspace.into()),
                    ("stop",e["stopScope"].as_str().map(str::to_owned).unwrap_or_else(||crate::i18n::tr("No observed provider stop is recorded. Continuation is unavailable.").into())),
                        ("input", input),
                        ("reply", e["reply"].to_string()),
                        (
                            "files",
                            serde_json::to_string_pretty(&e["files"]).unwrap_or_default(),
                        ),
                        (
                            "actions",
                            serde_json::to_string_pretty(&review["actions"]).unwrap_or_default(),
                        ),
                    ],
                );
                self.view
                    .label(cx, ids!(recovery_details))
                    .set_text(cx, &details);
                self.status(cx, "");
            }
            Target::RecoveryDecision => {
                if self.recovery_pending.as_ref().is_none_or(|(id, input)| {
                    value["decision"]["id"] != *id
                        || serde_json::to_value(input).ok().as_ref()
                            != Some(&value["decision"]["input"])
                }) {
                    return;
                }
                self.recovery_pending = None;
                self.recovery_review = Value::Null;
                self.view
                    .view(cx, ids!(recovery_confirmation))
                    .set_visible(cx, false);
                self.status(cx,match value["decision"]["phase"].as_str(){Some("sent")=>"The new continuation message was accepted. The original result is unchanged. Start the Agent service to process the new request.",Some("recorded")=>"Your disposition was recorded. The original execution result is unchanged.",_=>"Continuation delivery is unknown. Inspect the Room and the recorded action before creating another request. This action will not resend."});
            }
            _ => return,
        }
        self.render_recovery_controls(cx);
    }
    pub(super) fn recovery_actions(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        if self.page != Page::Agents || !self.view.view(cx, ids!(agent_detail)).visible() {
            return false;
        }
        if self.view.button(cx, ids!(recovery_tab)).clicked(actions) {
            self.section(cx, ids!(recovery_section));
            self.recovery_cursor = None;
            self.load_stopped(cx);
            return true;
        }
        if !self.view.view(cx, ids!(recovery_section)).visible() {
            return false;
        }
        if self
            .view
            .button(cx, ids!(recovery_refresh))
            .clicked(actions)
        {
            self.recovery_cursor = None;
            self.load_stopped(cx);
            return true;
        }
        if self.view.button(cx, ids!(recovery_older)).clicked(actions)
            && self
                .agent(cx)
                .is_some_and(|a| self.stopped_page["agentId"] == a)
        {
            self.recovery_cursor = self.stopped_page["stopped"]["nextCursor"]
                .as_str()
                .map(str::to_owned);
            self.load_stopped(cx);
            return true;
        }
        if self
            .view
            .drop_down(cx, ids!(recovery_records))
            .selected(actions)
            .is_some()
        {
            self.clear_recovery_review(cx);
            self.recovery_selected = if self
                .agent(cx)
                .is_some_and(|a| self.stopped_page["agentId"] == a)
            {
                self.selected(
                    cx,
                    ids!(recovery_records),
                    &array(&self.stopped_page["stopped"], "records"),
                )
                .map(|r| string(&r, "id").into())
                .unwrap_or_default()
            } else {
                String::new()
            };
            self.render_recovery_controls(cx);
            return true;
        }
        if self
            .view
            .button(cx, ids!(recovery_inspect))
            .clicked(actions)
            && let Some(agent) = self.agent(cx)
            && !self.recovery_selected.is_empty()
        {
            let paths = self.recovery_paths(cx);
            self.run(
                cx,
                Target::RecoveryReview,
                Command::AgentHistory {
                    action: HistoryAction::InspectStopped {
                        agent,
                        dispatch: self.recovery_selected.clone(),
                        paths,
                    },
                },
            );
            return true;
        }
        if self.view.button(cx, ids!(recovery_copy)).clicked(actions) && self.recovery_matches(cx) {
            cx.copy_to_clipboard(string(&self.recovery_review["review"]["evidence"], "input"));
            return true;
        }
        for (id, choice) in [
            (ids!(recovery_keep), 0),
            (ids!(recovery_accept), 1),
            (ids!(recovery_continue), 2),
        ] {
            if self.view.button(cx, id).clicked(actions) {
                if let Some(input) = self.recovery_input(cx, choice) {
                    self.view
                        .label(cx, ids!(recovery_confirm_details))
                        .set_text(
                            cx,
                            &serde_json::to_string_pretty(&input.disposition).unwrap_or_default(),
                        );
                    self.recovery_pending = Some((uuid::Uuid::new_v4().to_string(), input));
                    self.view
                        .view(cx, ids!(recovery_confirmation))
                        .set_visible(cx, true);
                }
                self.render_recovery_controls(cx);
                return true;
            }
        }
        if self
            .view
            .button(cx, ids!(recovery_confirm))
            .clicked(actions)
            && self.recovery_matches(cx)
            && let Some((id, input)) = self.recovery_pending.clone()
            && let Some(agent) = self.agent(cx)
        {
            self.run(
                cx,
                Target::RecoveryDecision,
                Command::AgentHistory {
                    action: HistoryAction::DecideStopped { agent, id, input },
                },
            );
            return true;
        }
        if self.view.button(cx, ids!(recovery_cancel)).clicked(actions) {
            self.recovery_pending = None;
            self.view
                .view(cx, ids!(recovery_confirmation))
                .set_visible(cx, false);
            self.render_recovery_controls(cx);
            return true;
        }
        for id in [
            ids!(recovery_paths),
            ids!(recovery_note),
            ids!(recovery_instruction),
        ] {
            if self.view.text_input(cx, id).changed(actions).is_some() {
                self.recovery_pending = None;
                self.recovery_review = Value::Null;
                self.view
                    .view(cx, ids!(recovery_confirmation))
                    .set_visible(cx, false);
                self.render_recovery_controls(cx);
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_dispositions_require_current_agent_review_paths_and_explicit_note() {
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
        let page = json!({"agentId":"agent","runtimeActive":false,"stopped":{"records":[{"id":"original","scope":{"room":"!r:test"},"inputPreview":"original"}],"nextCursor":"older"}});
        p.apply_recovery(&mut cx, Target::Stopped, &page);
        p.recovery_selected = "original".into();
        let review = json!({"agentId":"agent","runtimeActive":false,"review":{"id":"review","digest":"digest","expires":chrono::Utc::now().timestamp()+300,"evidence":{"dispatchId":"original","state":"unknown","input":"original request","scope":{"room":"!r:test","requester":"@bob:test"},"workspace":"/original","requiresTree":true,"stopScope":"whole_tree","files":[]},"actions":[]}});
        p.apply_recovery(&mut cx, Target::RecoveryReview, &review);
        assert!(p.recovery_matches(&mut cx));
        assert!(p.recovery_input(&mut cx, 0).is_none());
        p.view
            .text_input(&cx, ids!(recovery_note))
            .set_text(&mut cx, "I reviewed the original effects.");
        assert!(p.recovery_input(&mut cx, 0).is_some());
        assert!(p.recovery_input(&mut cx, 2).is_none());
        p.view
            .text_input(&cx, ids!(recovery_instruction))
            .set_text(&mut cx, "Only finish the remaining part.");
        let input = p.recovery_input(&mut cx, 2).unwrap();
        p.recovery_pending = Some(("same-original-action".into(), input.clone()));
        let wrong = json!({"agentId":"agent","decision":{"id":"another-action","input":input,"phase":"sent"}});
        p.apply_recovery(&mut cx, Target::RecoveryDecision, &wrong);
        assert!(p.recovery_pending.is_some());
        p.view
            .text_input(&cx, ids!(recovery_paths))
            .set_text(&mut cx, "changed.txt");
        p.render_recovery_controls(&mut cx);
        assert!(!p.recovery_matches(&mut cx));
        assert!(
            !p.view
                .button(&cx, ids!(recovery_confirm))
                .borrow()
                .unwrap()
                .enabled()
        );
        p.view
            .text_input(&cx, ids!(recovery_paths))
            .set_text(&mut cx, "");
        p.view
            .drop_down(&cx, ids!(agents))
            .set_selected_item(&mut cx, 2);
        let before = p.recovery_review.clone();
        p.apply_recovery(&mut cx, Target::RecoveryReview, &review);
        assert_eq!(p.recovery_review, before);
        assert!(!p.recovery_matches(&mut cx));
    }
}
