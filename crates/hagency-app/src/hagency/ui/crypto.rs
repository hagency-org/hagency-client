use super::*;
use ::hagency::native_owner::AgentCryptoAction;

impl HagencyPanel {
    pub(super) fn apply_crypto(&mut self, cx: &mut Cx, value: &Value) {
        if self.agent(cx).as_deref() != value["agentId"].as_str() {
            return;
        }
        let crypto = &value["crypto"];
        if !crypto.is_object()
            || crypto["agentGeneration"]
                .as_i64()
                .is_some_and(|g| self.execution_agent_record["generation"].as_i64() != Some(g))
        {
            return;
        }
        self.agent_crypto = crypto.clone();
        self.render_crypto(cx);
        self.sync_crypto(cx);
    }
    pub(super) fn render_crypto(&self, cx: &mut Cx) {
        let status = match string(&self.agent_crypto, "phase") {
            "initializing" => "Initializing Agent identity. Refresh to check progress.",
            "ready" => "Identity keys are available. Owner verification is pending.",
            "stopping" => "Stopping identity session. Waiting for original requests.",
            "stopped" => "Identity session stopped. Original keys are retained.",
            "close_unconfirmed" => {
                "Identity shutdown is unconfirmed. Original work is still retained."
            }
            "stale" => "Execution device changed. Reopen this Agent.",
            "failed" | "unavailable" => {
                "Identity unavailable. Refresh status or initialize the original store again."
            }
            _ => "Identity status has not been loaded on this device.",
        };
        self.view
            .label(cx, ids!(crypto_status))
            .set_text(cx, crate::i18n::tr(status));
        let identity = &self.agent_crypto["identity"];
        let public = if identity.is_object() {
            format!(
                "{}\n{}\nEd25519: {}\nCurve25519: {}",
                string(identity, "matrixUserId"),
                string(identity, "matrixDeviceId"),
                string(identity, "ed25519"),
                string(identity, "curve25519")
            )
        } else {
            String::new()
        };
        self.view
            .label(cx, ids!(crypto_identity))
            .set_text(cx, &public);
    }
    pub(super) fn sync_crypto(&self, cx: &mut Cx) {
        let local = self.local_agent(cx) && !self.busy;
        let phase = string(&self.agent_crypto, "phase");
        for (id, enabled) in [
            (ids!(crypto_refresh), local),
            (
                ids!(crypto_prepare),
                local
                    && !matches!(
                        phase,
                        "initializing" | "ready" | "stopping" | "close_unconfirmed"
                    ),
            ),
            (
                ids!(crypto_stop),
                local && matches!(phase, "initializing" | "ready" | "unavailable" | "stopping"),
            ),
            (
                ids!(crypto_copy),
                local && self.agent_crypto["identity"].is_object(),
            ),
        ] {
            self.view.button(cx, id).set_enabled(cx, enabled);
            self.view.widget(cx, id).set_disabled(cx, !enabled);
        }
    }
    pub(super) fn handle_crypto(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        let refresh = self.view.button(cx, ids!(crypto_refresh)).clicked(actions);
        let prepare = self.view.button(cx, ids!(crypto_prepare)).clicked(actions);
        let stop = self.view.button(cx, ids!(crypto_stop)).clicked(actions);
        let copy = self.view.button(cx, ids!(crypto_copy)).clicked(actions);
        if !(refresh || prepare || stop || copy) {
            return false;
        }
        if !self.local_agent(cx) || self.busy {
            return true;
        }
        if copy {
            if self.agent_crypto["identity"].is_object() {
                cx.copy_to_clipboard(
                    &serde_json::to_string_pretty(&self.agent_crypto["identity"])
                        .unwrap_or_default(),
                );
            }
            return true;
        }
        if let Some(agent) = self.agent(cx) {
            let action = if stop {
                AgentCryptoAction::Stop { agent }
            } else if prepare {
                AgentCryptoAction::Prepare { agent }
            } else {
                AgentCryptoAction::Status { agent }
            };
            self.run(cx, Target::Crypto, Command::AgentCrypto { action });
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agent_crypto_ui_keeps_unverified_identity_scoped_and_disables_foreign_device_actions() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let v = script_eval!(vm,{mod.widgets.HagencyPanel {}});
            let p = HagencyPanel::script_from_value(vm, v);
            assert!(vm.take_errors().is_empty());
            p
        });
        panel.agents =
            vec![json!({"id":"a","state":"active","generation":1,"executionDeviceId":"here"})];
        panel.execution_agent_record = panel.agents[0].clone();
        panel.current_device_id = "here".into();
        panel.execution_devices = vec![json!({"id":"here","revoked":false})];
        panel.devices_loaded = true;
        panel.labels(
            &mut cx,
            ids!(agents),
            "Select my Agent",
            &panel.agents,
            "id",
        );
        panel
            .view
            .drop_down(&cx, ids!(agents))
            .set_selected_item(&mut cx, 1);
        let identity = json!({"agentId":"a","agentGeneration":1,"matrixUserId":"@_hagency_a:test","matrixDeviceId":"HAGENCY_public","ed25519":"E".repeat(43),"curve25519":"C".repeat(43)});
        panel.apply_crypto(&mut cx,&json!({"agentId":"other","crypto":{"phase":"ready","agentGeneration":1,"identity":identity}}));
        assert!(panel.agent_crypto.is_null());
        panel.apply_crypto(&mut cx,&json!({"agentId":"a","crypto":{"phase":"ready","agentGeneration":2,"identity":identity}}));
        assert!(panel.agent_crypto.is_null());
        panel.apply_crypto(&mut cx,&json!({"agentId":"a","crypto":{"phase":"ready","agentGeneration":1,"identity":identity,"ownerVerified":false}}));
        assert!(
            panel
                .view
                .label(&cx, ids!(crypto_identity))
                .text()
                .contains(&"E".repeat(43))
        );
        assert_eq!(
            panel.view.label(&cx, ids!(crypto_status)).text(),
            crate::i18n::tr("Identity keys are available. Owner verification is pending.")
        );
        assert!(
            !panel
                .view
                .button(&cx, ids!(crypto_prepare))
                .borrow()
                .unwrap()
                .enabled()
        );
        assert!(
            panel
                .view
                .button(&cx, ids!(crypto_stop))
                .borrow()
                .unwrap()
                .enabled()
        );
        panel.execution_agent_record["executionDeviceId"] = json!("other");
        panel.sync_crypto(&mut cx);
        for id in [
            ids!(crypto_refresh),
            ids!(crypto_prepare),
            ids!(crypto_stop),
            ids!(crypto_copy),
        ] {
            assert!(!panel.view.button(&cx, id).borrow().unwrap().enabled());
        }
        panel.agent_crypto = Value::Null;
        panel.render_crypto(&mut cx);
        assert!(
            panel
                .view
                .label(&cx, ids!(crypto_identity))
                .text()
                .is_empty()
        );
    }
}
