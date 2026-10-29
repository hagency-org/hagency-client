//! Local account presentation and explicit, draft-only MCP configuration.
use super::*;

pub(super) fn parse_servers(text: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(if text.trim().is_empty() { "{}" } else { text })
        .map_err(|error| {
            crate::i18n::format(
                "Invalid JSON at line {line}, column {column}.",
                &[
                    ("line", error.line().to_string()),
                    ("column", error.column().to_string()),
                ],
            )
        })?;
    ::hagency::native_owner::validate_mcp_servers(&value).map_err(|error| {
        crate::i18n::format("Invalid MCP configuration: {error}", &[("error", error)])
    })?;
    // The runtime's transport restrictions still apply; also require a complete
    // URL here so a syntactically valid but incomplete HTTPS address is caught.
    for server in value
        .as_object()
        .into_iter()
        .flat_map(|object| object.values())
    {
        if server["transport"] == "http" {
            let url = server["url"]
                .as_str()
                .and_then(|url| url::Url::parse(url).ok());
            if url.is_none_or(|url| {
                url.host_str().is_none() || !url.username().is_empty() || url.password().is_some()
            }) {
                return Err(crate::i18n::tr(
                    "Enter a complete HTTPS URL or an HTTP loopback URL with a port.",
                )
                .into());
            }
        }
    }
    Ok(value)
}

impl HagencyPanel {
    pub(super) fn render_provider_controls(&self, cx: &mut Cx) {
        let runtime = self.provider_runtime(cx);
        let external = runtime != "codex";
        let remote = self.remote_agent(cx);
        let configurable = self.local_agent(cx) || (remote && self.configuration_online());
        let ready = self.provider_authenticated;
        let pending = !ready && self.provider_url.is_some();
        let checking = self.provider_state.is_null() && !ready && !pending;
        let shared = self.provider_state["shared"] == true;
        let failed = self.provider_state["checkFailed"] == true;
        for (id, show) in [
            (
                ids!(use_local_provider),
                !failed && !checking && !ready && !pending && (!shared || external),
            ),
            (ids!(provider_open), pending && !remote),
            (ids!(provider_cancel), pending && !remote),
            (
                ids!(provider_login),
                !failed && !checking && !external && !shared && !ready && !pending && !remote,
            ),
            (ids!(disconnect_provider), ready && shared && !remote),
            (
                ids!(provider_logout),
                ready && !shared && !external && !remote,
            ),
        ] {
            self.view.button(cx, id).set_visible(cx, show);
            self.view
                .button(cx, id)
                .set_enabled(cx, configurable && !self.busy);
        }
        self.view
            .view(cx, ids!(provider_controls))
            .set_visible(cx, !ready && !checking);
        self.view
            .widget(cx, ids!(provider_spinner))
            .set_visible(cx, !failed && (checking || pending));
        if remote {
            return;
        }
        let message = if self.provider_state["checkFailed"] == true {
            "Could not check the local account. Use refresh to retry."
        } else if checking {
            "Checking local account…"
        } else if pending {
            "Finish sign-in in your browser. Account status updates automatically."
        } else if ready && external {
            "Local runtime connected. Credentials remain managed by its own app."
        } else if ready && shared {
            "Using your signed-in local Codex account on this device."
        } else if ready {
            "Separate Codex account connected on this device."
        } else if external {
            "Sign in or configure profiles in the local runtime, then connect it here."
        } else if shared {
            "Local Codex is signed out. Sign in through Codex, then refresh here."
        } else {
            "No account connected. Connect your signed-in local account."
        };
        self.view
            .label(cx, ids!(provider_status))
            .set_text(cx, crate::i18n::tr(message));
    }

    pub(super) fn schedule_provider_check(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.provider_timer);
        self.provider_timer = Timer::default();
        if self.provider_url.is_some()
            && !self.provider_authenticated
            && self.provider_poll_count < 200
        {
            self.provider_timer = cx.start_timeout(3.0);
        }
    }

    fn mcp_feedback(&self, cx: &mut Cx, result: Result<(), String>) {
        self.view.inline_notice(cx, ids!(mcp_feedback)).show(
            cx,
            if result.is_ok() {
                PopupKind::Success
            } else {
                PopupKind::Error
            },
            if result.is_ok() {
                "Configuration valid"
            } else {
                "Check MCP configuration"
            },
            result
                .as_ref()
                .err()
                .map(String::as_str)
                .unwrap_or(crate::i18n::tr(
                    "Configuration is valid. No server was started or contacted.",
                )),
            "",
            false,
        );
    }

    pub(super) fn render_mcp(&self, cx: &mut Cx) {
        self.view
            .view(cx, ids!(mcp_content))
            .set_visible(cx, self.view.check_box(cx, ids!(programming)).active(cx));
        self.view
            .view(cx, ids!(mcp_json_panel))
            .set_visible(cx, self.mcp_json_open);
        self.view.button(cx, ids!(mcp_json_toggle)).set_text(
            cx,
            crate::i18n::tr(if self.mcp_json_open {
                "Hide JSON"
            } else {
                "Edit JSON"
            }),
        );
        let value = serde_json::from_str::<Value>(&self.input(cx, ids!(mcp_servers))).ok();
        let empty = value
            .as_ref()
            .and_then(Value::as_object)
            .is_none_or(serde_json::Map::is_empty);
        self.view.label(cx, ids!(mcp_empty)).set_visible(cx, empty);
        self.view.label(cx, ids!(mcp_empty)).set_text(
            cx,
            crate::i18n::tr(if value.as_ref().is_some_and(Value::is_object) {
                "No MCP servers configured."
            } else {
                "JSON draft is invalid. Validate it for details."
            }),
        );
    }

    fn mcp_value(&self, cx: &mut Cx) -> Result<Value, String> {
        parse_servers(&self.input(cx, ids!(mcp_servers)))
    }

    fn set_mcp_value(&mut self, cx: &mut Cx, value: &Value) {
        self.view
            .text_input(cx, ids!(mcp_servers))
            .set_text(cx, &serde_json::to_string_pretty(value).unwrap());
        self.view.inline_notice(cx, ids!(mcp_feedback)).clear(cx);
        self.clear_model_save_feedback(cx);
        self.render_mcp(cx);
    }

    pub(super) fn validate_mcp(&mut self, cx: &mut Cx) -> bool {
        let result = self.mcp_value(cx).map(|_| ());
        let valid = result.is_ok();
        if !valid {
            self.mcp_json_open = true;
        }
        self.mcp_feedback(cx, result);
        self.render_mcp(cx);
        valid
    }

    fn edit_mcp(&mut self, cx: &mut Cx, name: Option<String>, value: &Value) {
        self.mcp_editing = name.clone();
        self.view
            .text_input(cx, ids!(mcp_name))
            .set_text(cx, name.as_deref().unwrap_or(""));
        self.view
            .drop_down(cx, ids!(mcp_transport))
            .set_selected_item(cx, usize::from(value["transport"] == "stdio"));
        self.view
            .text_input(cx, ids!(mcp_url))
            .set_text(cx, string(value, "url"));
        self.view
            .text_input(cx, ids!(mcp_command))
            .set_text(cx, string(value, "command"));
        self.view.text_input(cx, ids!(mcp_args)).set_text(
            cx,
            &serde_json::to_string(value.get("args").unwrap_or(&json!([]))).unwrap(),
        );
        self.view.view(cx, ids!(mcp_form)).set_visible(cx, true);
        self.sync_mcp_transport(cx);
        self.view.inline_notice(cx, ids!(mcp_feedback)).clear(cx);
    }

    fn sync_mcp_transport(&self, cx: &mut Cx) {
        let stdio = self.view.drop_down(cx, ids!(mcp_transport)).selected_item() == 1;
        self.view.view(cx, ids!(mcp_stdio)).set_visible(cx, stdio);
        self.view.view(cx, ids!(mcp_http)).set_visible(cx, !stdio);
    }

    pub(super) fn handle_mcp(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        if self
            .view
            .text_input(cx, ids!(mcp_servers))
            .changed(actions)
            .is_some()
        {
            self.view.inline_notice(cx, ids!(mcp_feedback)).clear(cx);
            self.render_mcp(cx);
        }
        if self
            .view
            .drop_down(cx, ids!(mcp_transport))
            .selected(actions)
            .is_some()
        {
            self.sync_mcp_transport(cx);
            return true;
        }
        if self.view.button(cx, ids!(mcp_json_toggle)).clicked(actions) {
            self.mcp_json_open = !self.mcp_json_open;
            self.render_mcp(cx);
            return true;
        }
        if self.view.button(cx, ids!(mcp_cancel)).clicked(actions) {
            self.mcp_editing = None;
            self.view.view(cx, ids!(mcp_form)).set_visible(cx, false);
            return true;
        }
        if self.view.button(cx, ids!(mcp_validate)).clicked(actions) {
            self.validate_mcp(cx);
            return true;
        }
        if self.view.button(cx, ids!(mcp_format)).clicked(actions) {
            match self.mcp_value(cx) {
                Ok(value) => {
                    self.set_mcp_value(cx, &value);
                    self.mcp_feedback(cx, Ok(()));
                }
                Err(error) => self.mcp_feedback(cx, Err(error)),
            }
            return true;
        }
        if self.view.button(cx, ids!(mcp_add)).clicked(actions) {
            match self.mcp_value(cx) {
                Ok(_) => self.edit_mcp(cx, None, &Value::Null),
                Err(error) => {
                    self.mcp_json_open = true;
                    self.render_mcp(cx);
                    self.mcp_feedback(cx, Err(error));
                }
            }
            return true;
        }
        if self.view.button(cx, ids!(mcp_apply)).clicked(actions) {
            let result = (|| {
                let mut servers = self.mcp_value(cx)?;
                let name = self.input(cx, ids!(mcp_name));
                if self.mcp_editing.as_deref() != Some(&name) && servers.get(&name).is_some() {
                    return Err(crate::i18n::tr("A server with this name already exists.").into());
                }
                let server = if self.view.drop_down(cx, ids!(mcp_transport)).selected_item() == 1 {
                    let text = self.input(cx, ids!(mcp_args));
                    let args: Vec<String> =
                        serde_json::from_str(if text.trim().is_empty() { "[]" } else { &text })
                            .map_err(|_| {
                                crate::i18n::tr("Arguments must be a JSON array of strings.")
                                    .to_owned()
                            })?;
                    json!({"transport":"stdio","command":self.input(cx,ids!(mcp_command)),"args":args})
                } else {
                    json!({"transport":"http","url":self.input(cx,ids!(mcp_url))})
                };
                if let Some(old) = &self.mcp_editing {
                    servers.as_object_mut().unwrap().remove(old);
                }
                servers.as_object_mut().unwrap().insert(name, server);
                parse_servers(&servers.to_string())?;
                Ok(servers)
            })();
            match result {
                Ok(value) => {
                    self.set_mcp_value(cx, &value);
                    self.mcp_editing = None;
                    self.view.view(cx, ids!(mcp_form)).set_visible(cx, false);
                }
                Err(error) => self.mcp_feedback(cx, Err(error)),
            }
            return true;
        }
        for (key, row) in self
            .view
            .flat_list(cx, ids!(mcp_list))
            .items_with_actions(actions)
        {
            let edit = row.button(cx, ids!(edit)).clicked(actions);
            let remove = row.button(cx, ids!(remove)).clicked(actions);
            if !edit && !remove {
                continue;
            }
            match self.mcp_value(cx) {
                Ok(mut servers) => {
                    let Some(name) = servers
                        .as_object()
                        .unwrap()
                        .keys()
                        .find(|name| LiveId::from_str(name) == key)
                        .cloned()
                    else {
                        continue;
                    };
                    if edit {
                        self.edit_mcp(cx, Some(name.clone()), &servers[&name]);
                    } else {
                        servers.as_object_mut().unwrap().remove(&name);
                        self.set_mcp_value(cx, &servers);
                    }
                }
                Err(error) => {
                    self.mcp_json_open = true;
                    self.render_mcp(cx);
                    self.mcp_feedback(cx, Err(error));
                }
            }
            return true;
        }
        false
    }

    pub(super) fn draw_mcp(&self, cx: &mut Cx2d, scope: &mut Scope, step: &WidgetRef) -> bool {
        if step.widget_uid() != self.view.flat_list(cx, ids!(mcp_list)).widget_uid() {
            return false;
        }
        let Some(mut list) = step.borrow_mut::<FlatList>() else {
            return true;
        };
        let value =
            serde_json::from_str::<Value>(&self.input(cx, ids!(mcp_servers))).unwrap_or_default();
        for (name, server) in value
            .as_object()
            .into_iter()
            .flat_map(|object| object.iter())
            .take(16)
        {
            let Some(row) = list.item(cx, LiveId::from_str(name), id!(Server)) else {
                continue;
            };
            row.label(cx, ids!(name)).set_text(cx, name);
            let detail = if server["transport"] == "stdio" {
                format!("Stdio · {}", string(server, "command"))
            } else {
                format!("HTTP · {}", string(server, "url"))
            };
            row.label(cx, ids!(details)).set_text(cx, &detail);
            row.draw_all(cx, scope);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn panel() -> (Cx, HagencyPanel) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm,{mod.widgets.HagencyPanel {}});
            let panel = HagencyPanel::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty());
            panel
        });
        panel.page = Page::Agents;
        panel.agents = vec![json!({"id":"a"})];
        panel.labels(&mut cx, ids!(agents), "Select", &panel.agents, "id");
        panel
            .view
            .drop_down(&cx, ids!(agents))
            .set_selected_item(&mut cx, 1);
        panel.execution_agent_record =
            json!({"id":"a","executionDeviceId":"here","generation":1,"state":"active"});
        panel.current_device_id = "here".into();
        panel.devices_loaded = true;
        panel.execution_devices = vec![json!({"id":"here","revoked":false})];
        panel
            .view
            .view(&cx, ids!(agents_page))
            .set_visible(&mut cx, true);
        panel
            .view
            .view(&cx, ids!(agent_detail))
            .set_visible(&mut cx, true);
        panel
            .view
            .view(&cx, ids!(agent_list_holder))
            .set_visible(&mut cx, false);
        panel.section(&mut cx, ids!(model_section));
        panel.policy = json!({"agentId":"a","scope":"agent","modelLoaded":true,"modelProfile":{"model":"test","programming":{"enabled":true,"mcp_servers":{}}}});
        panel.render_policy(&mut cx);
        (cx, panel)
    }
    fn click(cx: &mut Cx, p: &mut HagencyPanel, id: &[LiveId]) {
        let uid = p.view.button(cx, id).widget_uid();
        let actions = cx.capture_actions(|cx| {
            cx.widget_action(uid, ButtonAction::Clicked(KeyModifiers::default()))
        });
        assert!(p.handle_mcp(cx, &actions));
    }
    fn draw(cx: &mut Cx, p: &mut HagencyPanel, width: f64) {
        cx.redraw_all();
        let pass = DrawPass::new(cx);
        pass.set_size(cx, dvec2(width, 1800.0));
        let mut list = DrawList2d::new(cx);
        let event = DrawEvent::default();
        let mut draw = makepad_widgets::makepad_draw::cx_draw::CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(width, 1800.0), Layout::flow_overlay());
        p.draw_walk_all(&mut cx2d, &mut Scope::empty(), Walk::fill());
        cx2d.end_pass_sized_turtle();
        list.end(&mut cx2d);
        cx2d.end_pass(&pass);
    }
    #[test]
    fn account_states_show_only_relevant_actions_and_pending_checks_stop_on_success() {
        let (mut cx, mut p) = panel();
        p.render_provider_controls(&mut cx);
        assert!(p.view.widget(&cx, ids!(provider_spinner)).visible());
        assert!(!p.view.button(&cx, ids!(use_local_provider)).visible());
        p.apply(
            &mut cx,
            Target::Provider,
            &json!({"runtime":"codex","state":"signed_out","authenticated":false}),
        );
        assert!(p.view.button(&cx, ids!(use_local_provider)).visible());
        p.apply(&mut cx,Target::Provider,&json!({"runtime":"codex","authenticated":false,"authUrl":"https://auth.openai.com/test"}));
        p.model_advanced = true;
        p.render_model_advanced(&mut cx);
        p.sync_navigation(&mut cx);
        assert!(p.view.button(&cx, ids!(provider_open)).visible());
        assert!(p.view.button(&cx, ids!(provider_cancel)).visible());
        assert!(!p.view.button(&cx, ids!(use_local_provider)).visible());
        assert!(!p.view.button(&cx, ids!(provider_login)).visible());
        assert!(
            !p.provider_timer.is_empty(),
            "pending login schedules an automatic read"
        );
        p.apply(&mut cx,Target::Provider,&json!({"runtime":"codex","authenticated":true,"shared":true,"credentialSource":"local_codex"}));
        assert!(p.provider_timer.is_empty());
        assert!(!p.view.view(&cx, ids!(provider_controls)).visible());
        assert!(p.view.button(&cx, ids!(disconnect_provider)).visible());
        for id in [
            ids!(use_local_provider),
            ids!(provider_login),
            ids!(provider_cancel),
            ids!(provider_open),
            ids!(provider_logout),
        ] {
            assert!(!p.view.button(&cx, id).visible());
        }
        p.apply(
            &mut cx,
            Target::Provider,
            &json!({"runtime":"codex","authenticated":false,"shared":true}),
        );
        assert!(
            !p.view.button(&cx, ids!(provider_login)).visible(),
            "shared login is managed by Codex itself"
        );
    }
    #[test]
    fn model_refresh_is_next_to_its_dropdown_and_octos_uses_only_profile_controls() {
        let (mut cx, mut p) = panel();
        p.render_models(&mut cx);
        for width in [1100.0, 560.0] {
            draw(&mut cx, &mut p, width);
            let model = p.view.drop_down(&cx, ids!(model)).area().rect(&cx);
            let refresh = p.view.button(&cx, ids!(refresh_models)).area().rect(&cx);
            assert!(model.size.x > 100.0 && refresh.size.x == 36.0);
            assert!(
                (model.pos.y + model.size.y / 2.0 - refresh.pos.y - refresh.size.y / 2.0).abs()
                    < 1.0,
                "{model:?} {refresh:?}"
            );
            assert!(refresh.pos.x >= model.pos.x + model.size.x);
            let header = p.view.view(&cx, ids!(mcp_header)).area().rect(&cx);
            let add = p.view.button(&cx, ids!(mcp_add)).area().rect(&cx);
            assert!(
                header.size.y <= 40.0 && add.size.x > 0.0,
                "the MCP title and add action stay on one compact row: {header:?} {add:?}"
            );
            assert!(add.pos.x + add.size.x <= header.pos.x + header.size.x + 0.5);
        }
        p.view
            .drop_down(&cx, ids!(runtime_kind))
            .set_selected_item(&mut cx, 2);
        p.render_models(&mut cx);
        assert_eq!(p.view.label(&cx, ids!(model_label)).text(), "Octos profile");
        assert!(!p.view.view(&cx, ids!(effort_holder)).visible());
        assert!(!p.view.view(&cx, ids!(programming_holder)).visible());
        p.view
            .drop_down(&cx, ids!(runtime_kind))
            .set_selected_item(&mut cx, 1);
        p.render_models(&mut cx);
        assert_eq!(p.view.label(&cx, ids!(model_label)).text(), "Local model");
        assert!(p.view.view(&cx, ids!(effort_holder)).visible());
        cx.with_vm(|vm| assert!(vm.take_errors().is_empty()));
    }
    #[test]
    fn mcp_validation_reuses_runtime_rules_and_reports_json_locations_without_running_servers() {
        assert!(parse_servers("{}").is_ok());
        assert!(
            parse_servers(r#"{"docs":{"transport":"http","url":"https://example.test/mcp"}}"#)
                .is_ok()
        );
        for invalid in [
            r#"[]"#,
            r#"{"bad name":{"transport":"http","url":"https://example.test/mcp"}}"#,
            r#"{"docs":{"transport":"http","url":"https://"}}"#,
            r#"{"docs":{"transport":"http","url":"http://example.test/mcp"}}"#,
            r#"{"docs":{"transport":"http","url":"https://example.test/mcp","headers":{"Authorization":"hidden"}}}"#,
            r#"{"local":{"transport":"stdio","command":"relative-command"}}"#,
        ] {
            assert!(parse_servers(invalid).is_err(), "{invalid}");
        }
        assert!(
            parse_servers("{\n invalid}")
                .unwrap_err()
                .contains("line 2")
        );
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("mcp-helper");
        std::fs::write(&executable, "this is deliberately not an executable").unwrap();
        let executable = executable.canonicalize().unwrap();
        assert!(parse_servers(&json!({"local":{"transport":"stdio","command":executable,"args":["--example","with spaces"]}}).to_string()).is_ok(),"validation never tries to execute the file");
    }
    #[test]
    fn visual_mcp_edits_preserve_other_servers_and_invalid_json_cannot_be_saved_or_replaced() {
        let (mut cx, mut p) = panel();
        p.set_mcp_value(
            &mut cx,
            &json!({"existing":{"transport":"http","url":"https://existing.test/mcp"}}),
        );
        click(&mut cx, &mut p, ids!(mcp_add));
        p.view
            .text_input(&cx, ids!(mcp_name))
            .set_text(&mut cx, "docs");
        p.view
            .text_input(&cx, ids!(mcp_url))
            .set_text(&mut cx, "https://docs.test/mcp");
        click(&mut cx, &mut p, ids!(mcp_apply));
        let original = p.mcp_value(&mut cx).unwrap();
        assert_eq!(original.as_object().unwrap().len(), 2);
        p.edit_mcp(&mut cx, Some("docs".into()), &original["docs"]);
        p.view
            .text_input(&cx, ids!(mcp_name))
            .set_text(&mut cx, "existing");
        click(&mut cx, &mut p, ids!(mcp_apply));
        assert_eq!(
            p.mcp_value(&mut cx).unwrap(),
            original,
            "duplicate rename must not overwrite either server"
        );
        p.view
            .text_input(&cx, ids!(mcp_name))
            .set_text(&mut cx, "renamed");
        click(&mut cx, &mut p, ids!(mcp_apply));
        let value = p.mcp_value(&mut cx).unwrap();
        assert!(value.get("docs").is_none());
        assert_eq!(value["renamed"], original["docs"]);
        assert_eq!(value["existing"], original["existing"]);
        p.view
            .text_input(&cx, ids!(mcp_servers))
            .set_text(&mut cx, "{broken");
        assert!(!p.validate_mcp(&mut cx));
        assert!(p.mcp_json_open);
        click(&mut cx, &mut p, ids!(mcp_add));
        assert_eq!(
            p.input(&mut cx, ids!(mcp_servers)),
            "{broken",
            "visual actions never replace invalid draft JSON with an empty object"
        );
        p.render_policy(&mut cx);
        assert_eq!(
            p.mcp_value(&mut cx).unwrap(),
            json!({}),
            "reopening resources restores the saved profile"
        );
        assert!(
            !p.view.inline_notice(&cx, ids!(mcp_feedback)).is_visible(),
            "discarding a draft also clears its old validation error"
        );
        assert!(!p.view.view(&cx, ids!(mcp_form)).visible());
    }
}
