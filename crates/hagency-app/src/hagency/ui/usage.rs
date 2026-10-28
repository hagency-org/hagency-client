use super::*;

fn utc_date(value: &str) -> Option<i64> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()?
        .and_hms_opt(0, 0, 0)
        .map(|d| d.and_utc().timestamp())
}
fn utc_time(value: &Value) -> String {
    value
        .as_i64()
        .and_then(|t| chrono::DateTime::<chrono::Utc>::from_timestamp(t, 0))
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}
fn total_text(value: &Value) -> String {
    crate::i18n::format(
        "Calls {calls} · input {input} · output {output} · cached {cached} · reasoning {reasoning} · pending {pending} · reserved {reserved} · unreported {unreported}",
        &[
            ("calls", value["calls"].as_u64().unwrap_or(0).to_string()),
            ("input", value["input"].as_u64().unwrap_or(0).to_string()),
            ("output", value["output"].as_u64().unwrap_or(0).to_string()),
            (
                "cached",
                value["cachedInput"].as_u64().unwrap_or(0).to_string(),
            ),
            (
                "reasoning",
                value["reasoningOutput"].as_u64().unwrap_or(0).to_string(),
            ),
            (
                "pending",
                value["pendingCalls"].as_u64().unwrap_or(0).to_string(),
            ),
            (
                "reserved",
                value["reserved"].as_u64().unwrap_or(0).to_string(),
            ),
            (
                "unreported",
                value["unreportedCalls"].as_u64().unwrap_or(0).to_string(),
            ),
        ],
    )
}

impl HagencyPanel {
    fn usage_query(&self, cx: &mut Cx) -> Option<UsageQuery> {
        let level = self.view.drop_down(cx, ids!(usage_scope)).selected_item();
        let binding = if level > 0 {
            Some(string(&self.selected(cx, ids!(usage_room), &self.bindings)?, "id").to_owned())
        } else {
            None
        };
        let requester = if level == 2 {
            Some(self.input(cx, ids!(usage_requester)))
        } else {
            None
        };
        let query = UsageQuery {
            agent: self.agent(cx)?,
            binding,
            requester,
            from: utc_date(&self.input(cx, ids!(usage_from)))?,
            until: utc_date(&self.input(cx, ids!(usage_until)))?,
            group: if self.view.drop_down(cx, ids!(usage_group)).selected_item() == 0 {
                UsageGroup::Day
            } else {
                UsageGroup::Month
            },
            alerts_before: self.usage_cursor.clone(),
        };
        query.validate().ok()?;
        Some(query)
    }
    fn usage_matches(&self, cx: &mut Cx) -> bool {
        self.usage_query(cx)
            .and_then(|q| serde_json::to_value(q).ok())
            .is_some_and(|q| q == self.usage_report["query"])
    }
    fn selected_usage_alert(&self, cx: &mut Cx) -> Option<Value> {
        self.selected(cx, ids!(usage_alerts), &array(&self.usage_report, "alerts"))
    }
    pub(super) fn render_usage_controls(&self, cx: &mut Cx) {
        let scope = self.view.drop_down(cx, ids!(usage_scope)).selected_item();
        self.view
            .view(cx, ids!(usage_room_holder))
            .set_visible(cx, scope > 0);
        self.view
            .view(cx, ids!(usage_requester_holder))
            .set_visible(cx, scope == 2);
        let matches = self.usage_matches(cx);
        let selected = self.selected_usage_alert(cx);
        self.view.button(cx, ids!(usage_ack)).set_enabled(
            cx,
            !self.busy
                && matches
                && selected
                    .as_ref()
                    .is_some_and(|a| a["acknowledgedAt"].is_null()),
        );
        self.view.button(cx, ids!(usage_older)).set_enabled(
            cx,
            !self.busy && matches && self.usage_report["alertsNext"].as_str().is_some(),
        );
        self.view
            .button(cx, ids!(usage_latest))
            .set_enabled(cx, !self.busy && self.usage_cursor.is_some());
    }
    fn load_usage(&mut self, cx: &mut Cx) {
        let Some(query) = self.usage_query(cx) else {
            self.status(cx,"Select the scope and enter valid UTC dates (YYYY-MM-DD). Daily reports support up to 366 days; monthly reports support up to 10 years.");
            return;
        };
        self.run(
            cx,
            Target::Usage,
            Command::AgentUsage {
                action: UsageAction::Report { query },
            },
        );
    }
    fn clear_usage_result(&mut self, cx: &mut Cx) {
        self.usage_report = Value::Null;
        self.usage_cursor = None;
        for id in [
            ids!(usage_totals),
            ids!(usage_undated),
            ids!(usage_rows),
            ids!(usage_alert_details),
        ] {
            self.view.label(cx, id).set_text(cx, "");
        }
        self.view
            .drop_down(cx, ids!(usage_alerts))
            .set_labels(cx, vec![crate::i18n::tr("Select a budget alert").into()]);
        self.render_usage_controls(cx);
    }
    fn open_usage(&mut self, cx: &mut Cx) {
        let Some(agent) = self.agent(cx) else {
            return;
        };
        self.section(cx, ids!(usage_section));
        if self.usage_agent != agent {
            self.usage_agent = agent;
            self.clear_usage_result(cx);
            let today = chrono::Utc::now().date_naive();
            self.view.text_input(cx, ids!(usage_from)).set_text(
                cx,
                &(today - chrono::Duration::days(29))
                    .format("%Y-%m-%d")
                    .to_string(),
            );
            self.view.text_input(cx, ids!(usage_until)).set_text(
                cx,
                &(today + chrono::Duration::days(1))
                    .format("%Y-%m-%d")
                    .to_string(),
            );
            self.view
                .drop_down(cx, ids!(usage_scope))
                .set_selected_item(cx, 0);
            self.view
                .drop_down(cx, ids!(usage_group))
                .set_selected_item(cx, 0);
        }
        let old = self
            .selected(cx, ids!(usage_room), &self.bindings)
            .map(|b| string(&b, "id").to_owned());
        let labels = std::iter::once(crate::i18n::tr("Select a Room binding").to_owned())
            .chain(self.bindings.iter().map(|b| self.binding_label(cx, b)))
            .collect();
        self.view
            .drop_down(cx, ids!(usage_room))
            .set_labels(cx, labels);
        self.view.drop_down(cx, ids!(usage_room)).set_selected_item(
            cx,
            old.and_then(|id| self.bindings.iter().position(|b| string(b, "id") == id))
                .map(|i| i + 1)
                .unwrap_or(0),
        );
        self.render_usage_controls(cx);
        self.load_usage(cx);
    }
    pub(super) fn apply_usage(&mut self, cx: &mut Cx, value: &Value) {
        let Some(query) = self
            .usage_query(cx)
            .and_then(|q| serde_json::to_value(q).ok())
        else {
            return;
        };
        if value["query"] != query {
            return;
        }
        self.usage_report = value.clone();
        self.view
            .label(cx, ids!(usage_totals))
            .set_text(cx, &total_text(&value["totals"]));
        self.view.label(cx, ids!(usage_undated)).set_text(
            cx,
            &format!(
                "{}\n{}",
                crate::i18n::tr("Calls with unknown dates (outside the selected date totals)"),
                total_text(&value["undated"])
            ),
        );
        let rows = array(value, "buckets");
        self.view.label(cx, ids!(usage_rows)).set_text(
            cx,
            &if rows.is_empty() {
                crate::i18n::tr("No calls in this date range.").into()
            } else {
                rows.iter()
                    .map(|r| format!("{}\n{}", string(r, "window"), total_text(&r["totals"])))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            },
        );
        let alerts = array(value, "alerts");
        let mut labels = vec![crate::i18n::tr("Select a budget alert").to_owned()];
        labels.extend(alerts.iter().map(|a| {
            format!(
                "{}% · {} · {} · {}",
                a["threshold"],
                string(a, "window"),
                utc_time(&a["observedAt"]),
                crate::i18n::tr(if a["acknowledgedAt"].is_null() {
                    "Unacknowledged"
                } else {
                    "Acknowledged"
                })
            )
        }));
        self.view
            .drop_down(cx, ids!(usage_alerts))
            .set_labels(cx, labels);
        self.view
            .drop_down(cx, ids!(usage_alerts))
            .set_selected_item(cx, 0);
        self.view.label(cx, ids!(usage_alert_details)).set_text(
            cx,
            if alerts.is_empty() {
                crate::i18n::tr("No budget alerts in this scope.")
            } else {
                ""
            },
        );
        self.status(cx, "");
        self.render_usage_controls(cx);
    }
    fn usage_alert_details(&self, cx: &mut Cx) {
        let Some(alert) = self.selected_usage_alert(cx) else {
            return;
        };
        let scope = alert["scope"].as_array().cloned().unwrap_or_default();
        let level = scope.first().and_then(Value::as_str).unwrap_or("");
        let mut target = crate::i18n::tr(match level {
            "room" => "Selected Room",
            "requester" => "Selected requester in Room",
            _ => "Entire Agent",
        })
        .to_owned();
        if let Some(id) = scope.get(2).and_then(Value::as_str) {
            target.push_str(&format!(
                " · {}",
                self.bindings
                    .iter()
                    .find(|b| string(b, "id") == id)
                    .map(|b| self.binding_label(cx, b))
                    .unwrap_or_else(|| id.into())
            ));
        }
        if let Some(user) = scope.get(3).and_then(Value::as_str) {
            target.push_str(&format!(" · {user}"));
        }
        let details = crate::i18n::format(
            "{scope}\nObserved {time} · budget revision {revision}\nAt observation: spent {spent} + reserved {held} / limit {limit}\n{acknowledgement}",
            &[
                ("scope", target),
                ("time", utc_time(&alert["observedAt"])),
                ("revision", alert["revision"].to_string()),
                ("spent", alert["spent"].to_string()),
                ("held", alert["held"].to_string()),
                ("limit", alert["tokenLimit"].to_string()),
                (
                    "acknowledgement",
                    if alert["acknowledgedAt"].is_null() {
                        crate::i18n::tr("Unacknowledged").into()
                    } else {
                        format!(
                            "{} {}",
                            crate::i18n::tr("Acknowledged"),
                            utc_time(&alert["acknowledgedAt"])
                        )
                    },
                ),
            ],
        );
        self.view
            .label(cx, ids!(usage_alert_details))
            .set_text(cx, &details);
        self.render_usage_controls(cx);
    }
    pub(super) fn usage_actions(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        if self.page != Page::Agents || !self.view.view(cx, ids!(agent_detail)).visible() {
            return false;
        }
        if self.view.button(cx, ids!(usage_tab)).clicked(actions) {
            self.open_usage(cx);
            return true;
        }
        if !self.view.view(cx, ids!(usage_section)).visible() {
            return false;
        }
        if self
            .view
            .drop_down(cx, ids!(usage_alerts))
            .selected(actions)
            .is_some()
        {
            self.usage_alert_details(cx);
            return true;
        }
        let changed = [ids!(usage_scope), ids!(usage_room), ids!(usage_group)]
            .into_iter()
            .any(|id| self.view.drop_down(cx, id).selected(actions).is_some())
            || [ids!(usage_requester), ids!(usage_from), ids!(usage_until)]
                .into_iter()
                .any(|id| self.view.text_input(cx, id).changed(actions).is_some());
        if changed {
            self.clear_usage_result(cx);
            self.render_usage_controls(cx);
            return true;
        }
        if self.view.button(cx, ids!(usage_refresh)).clicked(actions) {
            self.usage_cursor = None;
            self.load_usage(cx);
            return true;
        }
        if self.view.button(cx, ids!(usage_latest)).clicked(actions) {
            self.usage_cursor = None;
            self.load_usage(cx);
            return true;
        }
        if self.view.button(cx, ids!(usage_older)).clicked(actions) && self.usage_matches(cx) {
            self.usage_cursor = self.usage_report["alertsNext"].as_str().map(str::to_owned);
            self.load_usage(cx);
            return true;
        }
        if self.view.button(cx, ids!(usage_ack)).clicked(actions)
            && self.usage_matches(cx)
            && let Some(alert) = self.selected_usage_alert(cx)
            && alert["acknowledgedAt"].is_null()
            && let Some(query) = self.usage_query(cx)
        {
            self.run(
                cx,
                Target::Usage,
                Command::AgentUsage {
                    action: UsageAction::Acknowledge {
                        query,
                        id: string(&alert, "id").into(),
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
    fn panel(cx: &mut Cx) -> HagencyPanel {
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm,{mod.widgets.HagencyPanel {}});
            let panel = HagencyPanel::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty());
            panel
        });
        panel.set_page(cx, Page::Agents);
        panel.agents = vec![json!({"id":"agent"})];
        panel.labels(cx, ids!(agents), "Select", &panel.agents, "id");
        panel
            .view
            .drop_down(cx, ids!(agents))
            .set_selected_item(cx, 1);
        panel.bindings = vec![json!({"id":"binding","roomId":"!r:example.org"})];
        panel.labels(cx, ids!(usage_room), "Select", &panel.bindings, "id");
        panel
            .view
            .view(cx, ids!(agent_detail))
            .set_visible(cx, true);
        panel.section(cx, ids!(usage_section));
        panel
            .view
            .text_input(cx, ids!(usage_from))
            .set_text(cx, "2026-10-01");
        panel
            .view
            .text_input(cx, ids!(usage_until))
            .set_text(cx, "2026-10-03");
        panel
    }
    #[test]
    fn dates_scope_and_report_witness_control_alert_acknowledgement() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut p = panel(&mut cx);
        let query = p.usage_query(&mut cx).unwrap();
        assert_eq!(query.until - query.from, 2 * 86400);
        assert!(query.binding.is_none());
        assert!(query.requester.is_none());
        p.view
            .drop_down(&cx, ids!(usage_scope))
            .set_selected_item(&mut cx, 2);
        assert!(p.usage_query(&mut cx).is_none());
        p.view
            .drop_down(&cx, ids!(usage_room))
            .set_selected_item(&mut cx, 1);
        p.view
            .text_input(&cx, ids!(usage_requester))
            .set_text(&mut cx, "@alice:example.org");
        let query = p.usage_query(&mut cx).unwrap();
        assert_eq!(query.binding.as_deref(), Some("binding"));
        assert_eq!(query.requester.as_deref(), Some("@alice:example.org"));
        p.render_usage_controls(&mut cx);
        assert!(p.view.view(&cx, ids!(usage_room_holder)).visible());
        assert!(p.view.view(&cx, ids!(usage_requester_holder)).visible());
        let value = json!({"query":query,"totals":{"calls":2,"input":60,"output":20,"cachedInput":40,"reasoningOutput":10},"undated":{"calls":1},"buckets":[],"alerts":[{"id":"original","scope":["requester","agent","binding","@alice:example.org"],"window":"lifetime","revision":4,"threshold":80,"spent":60,"held":20,"tokenLimit":100,"observedAt":123,"acknowledgedAt":null}],"alertsNext":null});
        p.apply_usage(&mut cx, &value);
        p.view
            .drop_down(&cx, ids!(usage_alerts))
            .set_selected_item(&mut cx, 1);
        p.usage_alert_details(&mut cx);
        assert!(
            p.view
                .button(&cx, ids!(usage_ack))
                .borrow()
                .unwrap()
                .enabled()
        );
        assert!(p.view.label(&cx, ids!(usage_totals)).text().contains("60"));
        assert!(p.view.label(&cx, ids!(usage_undated)).text().contains("1"));
        p.view
            .text_input(&cx, ids!(usage_until))
            .set_text(&mut cx, "2026-02-30");
        p.render_usage_controls(&mut cx);
        assert!(p.usage_query(&mut cx).is_none());
        assert!(
            !p.view
                .button(&cx, ids!(usage_ack))
                .borrow()
                .unwrap()
                .enabled()
        );
        let before = p.usage_report.clone();
        let mut stale = value.clone();
        stale["query"]["agent"] = "other".into();
        p.apply_usage(&mut cx, &stale);
        assert_eq!(p.usage_report, before);
        p.clear_usage_result(&mut cx);
        assert!(p.usage_report.is_null());
        assert!(
            !p.view
                .button(&cx, ids!(usage_ack))
                .borrow()
                .unwrap()
                .enabled()
        );
    }
    #[test]
    fn usage_view_keeps_filters_and_actions_inside_native_scroll_viewport() {
        use makepad_widgets::makepad_draw::cx_draw::CxDraw;
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = panel(&mut cx);
        panel.sync_navigation(&mut cx);
        for width in [600.0, 980.0] {
            {
                cx.redraw_all();
                let pass = DrawPass::new(&mut cx);
                pass.set_size(&mut cx, dvec2(width, 800.0));
                let mut list = DrawList2d::new(&mut cx);
                let overlay = Overlay {
                    draw_list: DrawList::new(&mut cx),
                };
                let event = DrawEvent {
                    redraw_all: true,
                    ..Default::default()
                };
                let mut draw = CxDraw::new(&mut cx, &event);
                let mut cx2d = Cx2d::new(&mut draw);
                cx2d.begin_pass(&pass, None);
                list.begin_always(&mut cx2d);
                overlay.begin(&mut cx2d);
                cx2d.begin_root_turtle(dvec2(width, 800.0), Layout::flow_overlay());
                panel.draw_walk_all(&mut cx2d, &mut Scope::empty(), Walk::fixed(width, 800.0));
                overlay.end(&mut cx2d);
                cx2d.end_pass_sized_turtle();
                list.end(&mut cx2d);
                cx2d.end_pass(&pass);
            }
            for id in [
                ids!(usage_from),
                ids!(usage_until),
                ids!(usage_alerts),
                ids!(usage_ack),
            ] {
                let rect = panel.view.widget(&cx, id).area().rect(&cx);
                assert!(rect.size.x > 0.0 && rect.size.y > 0.0, "{id:?}");
                assert!(
                    rect.pos.x >= 0.0 && rect.pos.x + rect.size.x <= width,
                    "overflow: {rect:?}"
                );
            }
            let viewport = panel.view.view(&cx, ids!(details_scroll)).area().rect(&cx);
            assert!(viewport.size.y > 300.0 && viewport.pos.y + viewport.size.y <= 800.0);
        }
    }
}
