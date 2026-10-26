use super::*;
fn file_state(entry: &Value) -> &'static str {
    let record = &entry["record"];
    if !record["absence"].is_null() {
        return "Never prepared; original execution closed";
    }
    match record["remote"]["state"].as_str() {
        Some("sent") => "Delivered with Matrix receipt",
        Some("cancelled") => "Original delivery cancelled",
        Some("staging") => "Preparing; delivery unconfirmed",
        Some("queued") => "Queued; delivery unconfirmed",
        Some("uploading") => "Uploading; delivery unconfirmed",
        Some("uploaded") => "Uploaded; delivery unconfirmed",
        Some("sending") => "Sending; delivery unconfirmed",
        _ => "Delivery result unknown",
    }
}
fn file_details(entry: &Value) -> String {
    let r = &entry["record"];
    let f = &r["manifest"];
    let scope = &entry["scope"];
    let details = crate::i18n::format(
        "{state}\nFile: {filename}\nBytes: {bytes}\nCaption: {caption}\nSHA256: {hash}\nRoom: {room}\nRequester: {requester}\nThread: {thread}\nDispatch: {dispatch}\nExecution: {execution}\nOriginal request: {request}\nMatrix receipt: {event}\nApproval record: {audit}",
        &[
            ("state", crate::i18n::tr(file_state(entry)).into()),
            ("filename", string(f, "filename").into()),
            ("bytes", f["bytes"].to_string()),
            ("caption", string(f, "caption").into()),
            ("hash", string(f, "sha256").into()),
            ("room", string(scope, "room").into()),
            ("requester", string(scope, "requester").into()),
            ("thread", string(scope, "thread").into()),
            ("dispatch", string(entry, "dispatchId").into()),
            ("execution", string(entry, "executionId").into()),
            ("request", string(r, "requestId").into()),
            ("event", string(&r["remote"], "matrixEventId").into()),
            ("audit", string(r, "audit").into()),
        ],
    );
    if r["absence"].is_null() {
        details
    } else {
        format!(
            "{details}\n{}",
            crate::i18n::tr(
                "Server confirmed no preparation for the closed original execution. Capacity is released; original history and approvals are retained."
            )
        )
    }
}
impl HagencyPanel {
    pub(super) fn reset_file_history(&mut self, cx: &mut Cx) {
        self.file_page = Value::Null;
        self.file_detail = Value::Null;
        self.file_selected.clear();
        self.file_cursor = None;
        self.view
            .drop_down(cx, ids!(file_records))
            .set_labels(cx, vec![crate::i18n::tr("Select a file delivery").into()]);
        self.view
            .drop_down(cx, ids!(file_records))
            .set_selected_item(cx, 0);
        self.view.label(cx, ids!(file_details)).set_text(cx, "");
    }

    fn file_matches(&self, cx: &mut Cx) -> bool {
        !self.file_selected.is_empty()
            && self
                .agent(cx)
                .is_some_and(|a| self.file_detail["agentId"] == a)
            && self.file_detail["file"]["record"]["id"].as_str()
                == Some(self.file_selected.as_str())
    }
    pub(super) fn render_file_controls(&self, cx: &mut Cx) {
        let current = self
            .agent(cx)
            .is_some_and(|a| self.file_page["agentId"] == a);
        self.view.button(cx, ids!(file_older)).set_enabled(
            cx,
            !self.busy && current && self.file_page["files"]["nextCursor"].is_string(),
        );
        if !current {
            self.view.label(cx, ids!(file_details)).set_text(cx, "");
        }
        let can_refresh = !self.busy && self.file_matches(cx);
        self.view
            .button(cx, ids!(file_refresh))
            .set_enabled(cx, can_refresh);
    }
    fn load_files(&mut self, cx: &mut Cx) {
        let Some(agent) = self.agent(cx) else { return };
        self.file_selected.clear();
        self.file_detail = Value::Null;
        self.view.label(cx, ids!(file_details)).set_text(cx, "");
        self.run(
            cx,
            Target::Files,
            Command::AgentHistory {
                action: HistoryAction::Files {
                    agent,
                    cursor: self.file_cursor.clone(),
                },
            },
        );
    }
    pub(super) fn apply_file_history(&mut self, cx: &mut Cx, target: Target, value: &Value) {
        if !self.agent(cx).is_some_and(|a| value["agentId"] == a) {
            return;
        }
        if matches!(target, Target::Files) {
            self.file_page = value.clone();
            self.file_detail = Value::Null;
            self.file_selected.clear();
            let records = array(&value["files"], "records");
            let mut labels = vec![crate::i18n::tr("Select a file delivery").to_owned()];
            labels.extend(records.iter().map(|r| {
                format!(
                    "{} · {}",
                    string(&r["record"]["manifest"], "filename"),
                    crate::i18n::tr(file_state(r))
                )
            }));
            self.view
                .drop_down(cx, ids!(file_records))
                .set_labels(cx, labels);
            self.view
                .drop_down(cx, ids!(file_records))
                .set_selected_item(cx, 0);
            self.view.label(cx, ids!(file_details)).set_text(
                cx,
                if records.is_empty() {
                    crate::i18n::tr("No file delivery evidence is recorded on this device.")
                } else {
                    ""
                },
            );
        } else {
            if value["file"]["record"]["id"].as_str() != Some(self.file_selected.as_str()) {
                return;
            }
            self.file_detail = value.clone();
            self.view
                .label(cx, ids!(file_details))
                .set_text(cx, &file_details(&value["file"]));
        }
        self.render_file_controls(cx);
    }
    pub(super) fn file_history_actions(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        if self.page != Page::Agents || !self.view.view(cx, ids!(agent_detail)).visible() {
            return false;
        }
        if self.view.button(cx, ids!(files_tab)).clicked(actions) {
            self.section(cx, ids!(files_section));
            self.file_cursor = None;
            self.load_files(cx);
            return true;
        }
        if !self.view.view(cx, ids!(files_section)).visible() {
            return false;
        }
        if self.view.button(cx, ids!(file_newest)).clicked(actions) {
            self.file_cursor = None;
            self.load_files(cx);
            return true;
        }
        if self.view.button(cx, ids!(file_older)).clicked(actions)
            && self
                .agent(cx)
                .is_some_and(|a| self.file_page["agentId"] == a)
        {
            self.file_cursor = self.file_page["files"]["nextCursor"]
                .as_str()
                .map(str::to_owned);
            self.load_files(cx);
            return true;
        }
        if self
            .view
            .drop_down(cx, ids!(file_records))
            .selected(actions)
            .is_some()
        {
            let records = array(&self.file_page["files"], "records");
            let index = self.view.drop_down(cx, ids!(file_records)).selected_item();
            self.file_selected = index
                .checked_sub(1)
                .and_then(|i| records.get(i))
                .and_then(|r| r["record"]["id"].as_str())
                .unwrap_or("")
                .into();
            self.file_detail = Value::Null;
            self.view.label(cx, ids!(file_details)).set_text(cx, "");
            if !self.file_selected.is_empty()
                && let Some(agent) = self.agent(cx).filter(|a| self.file_page["agentId"] == *a)
            {
                self.run(
                    cx,
                    Target::FileDetail,
                    Command::AgentHistory {
                        action: HistoryAction::File {
                            agent,
                            id: self.file_selected.clone(),
                        },
                    },
                );
            }
            self.render_file_controls(cx);
            return true;
        }
        if self.view.button(cx, ids!(file_refresh)).clicked(actions)
            && self.file_matches(cx)
            && let Some(agent) = self.agent(cx)
        {
            self.run(
                cx,
                Target::FileDetail,
                Command::AgentHistory {
                    action: HistoryAction::RefreshFile {
                        agent,
                        id: self.file_selected.clone(),
                    },
                },
            );
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_delivery_controls_preserve_unknown_and_fence_agent_and_selection() {
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
        p.section(&mut cx, ids!(files_section));
        assert!(p.view.view(&cx, ids!(files_section)).visible());
        assert!(!p.view.view(&cx, ids!(history_section)).visible());
        p.section(&mut cx, ids!(history_section));
        assert!(!p.view.view(&cx, ids!(files_section)).visible());
        p.section(&mut cx, ids!(files_section));
        p.agents = vec![json!({"id":"agent"}), json!({"id":"other"})];
        p.labels(&mut cx, ids!(agents), "Select", &p.agents, "id");
        p.view
            .drop_down(&cx, ids!(agents))
            .set_selected_item(&mut cx, 1);
        let entry = json!({"scope":{"room":"!room:test","requester":"@bob:test","thread":"$root"},"dispatchId":"original-dispatch","executionId":"original-execution","record":{"id":"original-file","requestId":"original-request","manifest":{"filename":"report.txt","caption":"original caption","bytes":5,"sha256":"original-digest"},"remote":{"state":"unknown","matrixEventId":null},"audit":"original-approval","absence":null}});
        let page = json!({"agentId":"agent","files":{"records":[entry],"nextCursor":"older"}});
        p.apply_file_history(&mut cx, Target::Files, &page);
        assert!(
            p.view
                .button(&cx, ids!(file_older))
                .borrow()
                .unwrap()
                .enabled()
        );
        p.file_selected = "original-file".into();
        let mut detail = json!({"agentId":"agent","file":entry});
        p.apply_file_history(&mut cx, Target::FileDetail, &detail);
        assert!(
            p.view
                .button(&cx, ids!(file_refresh))
                .borrow()
                .unwrap()
                .enabled()
        );
        let text = p.view.label(&cx, ids!(file_details)).text();
        assert!(text.contains(crate::i18n::tr("Delivery result unknown")));
        assert!(text.contains("original-dispatch"));
        assert!(!text.contains(crate::i18n::tr("Delivered with Matrix receipt")));
        let mut foreign = detail.clone();
        foreign["agentId"] = "other".into();
        p.apply_file_history(&mut cx, Target::FileDetail, &foreign);
        assert_eq!(p.file_detail, detail);
        foreign = detail.clone();
        foreign["file"]["record"]["id"] = "other-file".into();
        p.apply_file_history(&mut cx, Target::FileDetail, &foreign);
        assert_eq!(p.file_detail, detail);
        detail["file"]["record"]["remote"] =
            json!({"state":"sent","matrixEventId":"$originalReceipt"});
        p.apply_file_history(&mut cx, Target::FileDetail, &detail);
        assert!(
            p.view
                .label(&cx, ids!(file_details))
                .text()
                .contains("$originalReceipt")
        );
        p.busy = true;
        p.render_file_controls(&mut cx);
        assert!(
            !p.view
                .button(&cx, ids!(file_refresh))
                .borrow()
                .unwrap()
                .enabled()
        );
        p.busy = false;
        p.view
            .drop_down(&cx, ids!(agents))
            .set_selected_item(&mut cx, 2);
        p.reset_file_history(&mut cx);
        p.render_file_controls(&mut cx);
        assert!(p.file_selected.is_empty());
        assert!(p.file_detail.is_null());
        assert!(p.view.label(&cx, ids!(file_details)).text().is_empty());
        assert!(
            !p.view
                .button(&cx, ids!(file_refresh))
                .borrow()
                .unwrap()
                .enabled()
        );
    }
    #[test]
    fn closed_preparation_is_separate_from_delivered_or_cancelled() {
        let entry = json!({"record":{"absence":{"observation":{"preparationClosed":true,"delivery":null}},"remote":null}});
        assert_eq!(
            file_state(&entry),
            "Never prepared; original execution closed"
        );
        assert!(file_details(&entry).contains(crate::i18n::tr("Server confirmed no preparation for the closed original execution. Capacity is released; original history and approvals are retained.")));
    }
}
