//! Account-wide discovery observes only existing local work. Review opens the
//! original scoped page; neither discovery nor cached rows can authorize tools.
use super::*;
use serde::Deserialize;
use std::collections::HashMap;
const INTERVAL: f64 = 5.0;
const MAX_PAGE: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Summary {
    agent_id: String, binding_id: String, room_id: String, requester: String,
    proposal_id: String, args_digest: String, tool: String, expires: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalTarget {
    pub identity: String,
    pub agent: String,
    pub binding: String,
    pub room: String,
    pub proposal: String,
    pub digest: String,
}
impl ApprovalTarget {
    fn from_summary(identity: &str, row: &Summary) -> Self {
        Self {identity:identity.into(),agent:row.agent_id.clone(),binding:row.binding_id.clone(),
            room:row.room_id.clone(),proposal:row.proposal_id.clone(),digest:row.args_digest.clone()}
    }
    pub(super) fn matches(&self, identity: &str, row: &Value, at: i64) -> bool {
        self.identity == identity && row["proposalId"] == self.proposal && row["argsDigest"] == self.digest
            && super::super::approvals::matches(row,&self.agent,&self.binding,&self.room,at)
    }
}
impl HagencyPanel {
    pub(super) fn reject_discovered_approval(&mut self,cx:&mut Cx) {
        self.wanted_approval=None;self.wanted_run=None;
        self.select_policy_room(cx,0);
        let message=crate::i18n::tr("This exact approval is no longer pending. No decision was submitted.");
        self.view.label(cx,ids!(approval_feedback)).set_text(cx,message);
        self.status(cx,message);
    }
    pub(super) fn open_discovered_binding(&mut self,cx:&mut Cx)->bool {
        let Some(target)=self.wanted_approval.clone() else {return false;};
        self.wanted_run=None;
        let index=self.bindings.iter().position(|binding|string(binding,"id")==target.binding
            && string(binding,"roomId")==target.room && string(binding,"state")=="active");
        if target.identity!=self.identity || self.agent(cx).as_deref()!=Some(target.agent.as_str())
            || !self.local_agent(cx) || index.is_none() {
            self.reject_discovered_approval(cx);return true;
        }
        self.select_policy_room(cx,index.unwrap()+1);
        self.section(cx,ids!(runtime_section));
        self.refresh_approvals(cx);
        true
    }
}
#[derive(Debug)]
struct Discovered {
    widget: WidgetUid, identity: String, request: u64, cursor: Option<String>, result: Result<Value,String>,
}
struct Page {rows:Vec<Summary>,total:usize,next:Option<String>}
fn page(value: &Value, at: i64) -> Option<Page> {
    let rows:Vec<Summary>=serde_json::from_value(value["approvals"].clone()).ok()?;
    let total=usize::try_from(value["total"].as_u64()?).ok()?;
    if rows.len()>MAX_PAGE || total<rows.len() || rows.iter().any(|row| {
        [&row.agent_id,&row.binding_id,&row.room_id,&row.requester,&row.proposal_id,&row.tool]
            .iter().any(|s|s.is_empty() || s.len()>512 || s.chars().any(char::is_control))
            || !row.room_id.starts_with('!') || row.args_digest.len()!=64
            || !row.args_digest.bytes().all(|b|b.is_ascii_hexdigit())
    }) {return None;}
    let next=match value.get("nextCursor")? {
        Value::Null=>None, Value::String(s) if s.len()==64 && s.bytes().all(|b|b.is_ascii_hexdigit())=>Some(s.clone()), _=>return None,
    };
    Some(Page {rows:rows.into_iter().filter(|row|row.expires>at).collect(),total,next})
}

/// Modal draws its content with draw_all, so the virtual list must own its
/// draw loop here. An outer widget never receives the modal's PortalList steps.
#[derive(Script, ScriptHook, Widget)]
pub struct ApprovalRows {
    #[deref] list: PortalList,
    #[rust] rows: Vec<Summary>,
    #[rust] identity: String,
    #[rust] fresh: Option<Instant>,
    #[rust] failed: bool,
    #[rust] pending: bool,
    #[rust] painted: HashMap<WidgetUid,ApprovalTarget>,
}
impl ApprovalRows {
    fn clicked_target(&self,cx:&Cx,actions:&Actions)->Option<ApprovalTarget> {
        for index in 0..self.rows.len() {
            let Some((_,row))=self.list.get_item(index) else {continue;};
            if row.button(cx,ids!(review_approval)).clicked(actions) {
                return self.painted.get(&row.widget_uid()).cloned();
            }
        }
        None
    }
}
impl Widget for ApprovalRows {
    fn handle_event(&mut self,cx:&mut Cx,event:&Event,scope:&mut Scope) {
        self.list.handle_event(cx,event,scope);
    }
    fn draw_walk(&mut self,cx:&mut Cx2d,scope:&mut Scope,walk:Walk)->DrawStep {
        let actionable=!self.failed && self.fresh.is_some_and(|at|at.elapsed().as_secs()<15);
        self.list.set_item_range(cx,0,self.rows.len().max(1));
        while self.list.draw_walk(cx,scope,walk).step().is_some() {
            while let Some(index)=self.list.next_visible_item(cx) {
                let Some(row)=self.rows.get(index) else {
                    let empty=self.list.item(cx,index,id!(Empty));
                    empty.set_text(cx,crate::i18n::tr(if self.failed {"Pending approvals are unavailable."} else if self.pending {"Checking pending approvals on this device…"} else {"No pending approvals on this device."}));
                    empty.draw_all(cx,scope);continue;
                };
                let widget=self.list.item(cx,index,id!(Approval));
                widget.label(cx,ids!(approval_summary)).set_text(cx,&format!("{} · {}\n{} · {}",row.agent_id,row.tool,row.room_id,row.requester));
                widget.button(cx,ids!(review_approval)).set_enabled(cx,actionable && row.expires>super::super::approvals::now());
                self.painted.insert(widget.widget_uid(),ApprovalTarget::from_summary(&self.identity,row));
                widget.draw_all(cx,scope);
            }
        }
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct GlobalApprovalInbox {
    #[deref] view: View,
    #[rust] identity: String,
    #[rust] request: u64,
    #[rust] pending: bool,
    #[rust] timer: Timer,
    #[rust] task: Option<tokio::task::AbortHandle>,
    #[rust] rows: Vec<Summary>,
    #[rust] total: usize,
    #[rust] next: Option<String>,
    #[rust] cursor: Option<String>,
    #[rust] back: Vec<Option<String>>,
    #[rust] fresh: Option<Instant>,
    #[rust] failed: bool,
    #[rust] dialog_size: Option<Vec2d>,
    #[rust] compact: bool,
}
impl Drop for GlobalApprovalInbox {
    fn drop(&mut self) {if let Some(task)=self.task.take() {task.abort();}}
}
impl GlobalApprovalInbox {
    fn reset(&mut self,cx:&mut Cx,identity:String) {
        if let Some(task)=self.task.take() {task.abort();}
        cx.stop_timer(self.timer);self.timer=Timer::default();self.request=nonce();self.pending=false;
        self.rows.clear();self.total=0;self.cursor=None;self.next=None;self.back.clear();
        self.fresh=None;self.failed=false;self.identity=identity;
        self.view.modal(cx,ids!(inbox_popup)).close(cx);
        self.render(cx);
    }
    fn poll(&mut self,cx:&mut Cx) {
        if self.pending || self.identity.is_empty() {return;}
        let Some(service)=Service::existing_current() else {return;};
        if service.identity()!=self.identity {return;}
        self.request=nonce();self.pending=true;self.render(cx);
        let request=self.request;let identity=self.identity.clone();let widget=self.widget_uid();let cursor=self.cursor.clone();
        let handle=crate::account_session::spawn(async move {
            let result=tokio::time::timeout(std::time::Duration::from_secs(10),service.execute(Command::PendingApprovals {cursor:cursor.clone()}))
                .await.unwrap_or_else(|_|Err("approvals_unavailable".into()));
            Cx::post_action(Discovered {widget,identity,request,cursor,result});
        });
        self.task=Some(handle.abort_handle());
    }
    fn accept(&mut self,cx:&mut Cx,done:&Discovered) {
        if done.widget!=self.widget_uid() || done.identity!=self.identity || done.request!=self.request
            || done.cursor!=self.cursor || !self.pending {return;}
        self.pending=false;self.task=None;
        match done.result.as_ref().ok().and_then(|value|page(value,super::super::approvals::now())) {
            Some(page)=>{
                self.rows=page.rows;self.total=page.total;self.next=page.next;self.fresh=Some(Instant::now());self.failed=false;
            }
            _=>{
                self.failed=true;self.fresh=None;
                if matches!(&done.result,Err(code) if code=="approval_cursor_expired") {
                    self.cursor=None;self.next=None;self.back.clear();self.rows.clear();
                }
            }
        }
        cx.stop_timer(self.timer);self.timer=cx.start_timeout(INTERVAL);self.render(cx);
    }
    fn actionable(&self)->bool { !self.failed && self.fresh.is_some_and(|at|at.elapsed().as_secs()<15) }
    fn render(&mut self,cx:&mut Cx) {
        let compact=self.view.area().rect(cx).size.x<120.0;
        let count=if self.identity.is_empty() {"0".into()} else if self.failed {"?".into()} else {self.total.to_string()};
        self.view.button(cx,ids!(open_approval_inbox)).set_text(cx,&if compact {count.clone()} else {
            crate::i18n::format("Approvals · {count}",&[("count",count)])
        });
        self.view.button(cx,ids!(open_approval_inbox)).set_enabled(cx,!self.identity.is_empty());
        let status=if self.failed {crate::i18n::tr("Approvals could not be refreshed. Review is disabled until a fresh check succeeds.").to_owned()}
            else if self.pending && self.fresh.is_none() {crate::i18n::tr("Checking pending approvals on this device…").into()}
            else {crate::i18n::format("{count} pending approvals across this device. Open a request to review its exact action.",&[("count",self.total.to_string())])};
        self.view.label(cx,ids!(inbox_status)).set_text(cx,&status);
        self.view.button(cx,ids!(inbox_next)).set_enabled(cx,!self.pending && self.next.is_some() && self.actionable());
        self.view.button(cx,ids!(inbox_previous)).set_enabled(cx,!self.pending && !self.back.is_empty());
        self.view.button(cx,ids!(inbox_refresh)).set_enabled(cx,!self.pending);
        if let Some(mut rows)=self.view.approval_rows(cx,ids!(inbox_rows)).borrow_mut() {
            rows.rows=self.rows.clone();rows.identity=self.identity.clone();rows.fresh=self.fresh;
            rows.failed=self.failed;rows.pending=self.pending;rows.painted.clear();
            rows.list.redraw(cx);
        }
        self.view.redraw(cx);
    }
    fn review(&mut self,cx:&mut Cx,target:&ApprovalTarget) {
        let current=Service::existing_current().map(|service|service.identity());
        self.review_for_identity(cx,target,current.as_deref());
    }
    fn clicked_target(&self,cx:&Cx,actions:&Actions)->Option<ApprovalTarget> {
        self.view.approval_rows(cx,ids!(inbox_rows)).borrow()?.clicked_target(cx,actions)
    }
    fn review_for_identity(&mut self,cx:&mut Cx,target:&ApprovalTarget,current:Option<&str>) {
        if current!=Some(self.identity.as_str()) {return;}
        if !self.actionable() || !self.rows.iter().any(|r|r.expires>super::super::approvals::now()
            && ApprovalTarget::from_summary(&self.identity,r)==*target) {self.poll(cx);return;}
        self.view.modal(cx,ids!(inbox_popup)).close(cx);
        cx.action(HagencyAction::ReviewApproval(target.clone()));
    }
}
impl Widget for GlobalApprovalInbox {
    fn handle_event(&mut self,cx:&mut Cx,event:&Event,scope:&mut Scope) {
        let identity=Service::existing_current().map(|s|s.identity()).unwrap_or_default();
        if identity!=self.identity {self.reset(cx,identity);}
        let due=self.timer.is_event(event).is_some();
        if !self.identity.is_empty() && (due || self.timer.is_empty() && !self.pending) {
            self.timer=cx.start_timeout(INTERVAL);self.poll(cx);
        }
        self.view.handle_event(cx,event,scope);
        if let Event::Actions(actions)=event {
            for action in actions {if let Some(done)=action.downcast_ref::<Discovered>() {self.accept(cx,done);}}
            if self.view.button(cx,ids!(open_approval_inbox)).clicked(actions) {
                self.view.modal(cx,ids!(inbox_popup)).open(cx);self.poll(cx);self.render(cx);
            }
            if self.view.button(cx,ids!(inbox_close)).clicked(actions) || self.view.modal(cx,ids!(inbox_popup)).dismissed(actions) {
                self.view.modal(cx,ids!(inbox_popup)).close(cx);
            }
            if self.view.button(cx,ids!(inbox_refresh)).clicked(actions) && !self.pending {
                self.cursor=None;self.back.clear();self.poll(cx);
            }
            if self.view.button(cx,ids!(inbox_next)).clicked(actions) && !self.pending && self.actionable()
                && let Some(next)=self.next.clone() {
                if self.back.len()==128 {self.back.remove(0);}
                self.back.push(self.cursor.clone());self.cursor=Some(next);self.rows.clear();self.fresh=None;self.poll(cx);
            }
            if self.view.button(cx,ids!(inbox_previous)).clicked(actions) && !self.pending
                && let Some(previous)=self.back.pop() {
                self.cursor=previous;self.rows.clear();self.fresh=None;self.poll(cx);
            }
            if let Some(target)=self.clicked_target(cx,actions) {self.review(cx,&target);}
        }
    }
    fn draw_walk(&mut self,cx:&mut Cx2d,scope:&mut Scope,walk:Walk)->DrawStep {
        // The modal uses the current window bounds, including narrow/short views.
        let size=cx.current_pass_size();
        let width=(size.x-32.0).clamp(200.0,580.0);let height=(size.y-48.0).clamp(200.0,560.0);
        if self.dialog_size!=Some(dvec2(width,height)) {
            let mut content=self.view.view(cx,ids!(inbox_popup.content));
            script_apply_eval!(cx,content,{width:#(width) height:#(height)});
            self.dialog_size=Some(dvec2(width,height));
        }
        let compact=self.view.area().rect(cx).size.x<120.0;
        if compact!=self.compact {self.compact=compact;self.render(cx);}
        self.view.draw_walk(cx,scope,walk)
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_draw::cx_draw::CxDraw;
    fn summary(agent:&str,binding:&str,id:&str)->Value {
        json!({"agentId":agent,"bindingId":binding,"roomId":format!("!{binding}:test"),
            "requester":"@owner:test","proposalId":id,"argsDigest":"a".repeat(64),"tool":"model.request",
            "expires":super::super::super::approvals::now()+300})
    }
    fn widget()->(Cx,GlobalApprovalInbox) {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let widget=cx.with_vm(|vm|{
            makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);vm.bx.captured_errors=Some(Vec::new());
            let value=script_eval!(vm,{mod.widgets.GlobalApprovalInbox {}});
            let widget=GlobalApprovalInbox::script_from_value(vm,value);
            assert!(vm.take_errors().is_empty());widget
        });
        (cx,widget)
    }
    fn finish(cx:&mut Cx,inbox:&mut GlobalApprovalInbox,value:Value) {
        inbox.request=nonce();inbox.pending=true;
        let done=Discovered {widget:inbox.widget_uid(),identity:inbox.identity.clone(),request:inbox.request,
            cursor:inbox.cursor.clone(),result:Ok(value)};
        inbox.accept(cx,&done);
    }
    fn draw(cx:&mut Cx,inbox:&mut GlobalApprovalInbox,width:f64,height:f64) {
        cx.redraw_all();let pass=DrawPass::new(cx);pass.set_size(cx,dvec2(width,height));
        let mut list=DrawList2d::new(cx);let overlay=Overlay {draw_list:DrawList::new(cx)};
        let event=DrawEvent::default();let mut draw=CxDraw::new(cx,&event);let mut cx2d=Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass,None);list.begin_always(&mut cx2d);overlay.begin(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(width,height),Layout::flow_overlay());
        inbox.draw_walk_all(&mut cx2d,&mut Scope::empty(),Walk::fixed(250.0,36.0));
        overlay.end(&mut cx2d);cx2d.end_pass_sized_turtle();list.end(&mut cx2d);cx2d.end_pass(&pass);
    }
    #[test]
    fn discovery_page_is_bounded_and_rejects_malformed_targets() {
        let at=super::super::super::approvals::now();
        let mut value=json!({"approvals":[summary("a","one","p1"),summary("b","two","p2")],"total":80,"nextCursor":"b".repeat(64)});
        let parsed=page(&value,at).unwrap();assert_eq!(parsed.rows.len(),2);assert_eq!(parsed.total,80);
        value["approvals"][0]["expires"]=json!(at);
        assert_eq!(page(&value,at).unwrap().rows.len(),1);
        value["approvals"][1]["argsDigest"]=json!("forged");assert!(page(&value,at).is_none());
        value["approvals"]=json!((0..33).map(|i|summary("a","one",&i.to_string())).collect::<Vec<_>>());
        assert!(page(&value,at).is_none());
    }
    #[test]
    fn discovery_clears_cross_account_state_and_ignores_old_requests_without_authorizing_work() {
        let (mut cx,mut inbox)=widget();inbox.identity="account-a".into();
        finish(&mut cx,&mut inbox,json!({"approvals":[summary("a","one","p1"),summary("b","two","p2")],"total":2,"nextCursor":null}));
        assert_eq!(inbox.rows.len(),2);assert!(inbox.actionable());
        inbox.view.modal(&cx,ids!(inbox_popup)).open(&mut cx);
        inbox.reset(&mut cx,"account-b".into());
        let late=Discovered {widget:inbox.widget_uid(),identity:"account-a".into(),request:inbox.request,cursor:None,
            result:Ok(json!({"approvals":[summary("a","one","p1")],"total":1,"nextCursor":null}))};
        inbox.pending=true;inbox.accept(&mut cx,&late);
        assert!(inbox.rows.is_empty() && inbox.fresh.is_none() && inbox.next.is_none());
        assert!(!inbox.view.modal(&cx,ids!(inbox_popup)).is_open());
        finish(&mut cx,&mut inbox,json!({"approvals":[summary("b","two","p2")],"total":1,"nextCursor":null}));
        let target=ApprovalTarget::from_summary(&inbox.identity,&inbox.rows[0]);
        let actions=cx.capture_actions(|cx|inbox.review_for_identity(cx,&target,Some("account-a")));
        assert!(actions.is_empty());
        inbox.failed=true;
        let actions=cx.capture_actions(|cx|inbox.review_for_identity(cx,&target,Some("account-b")));
        assert!(actions.is_empty());
    }
    #[test]
    fn global_inbox_widget_fits_window_and_review_opens_only_the_painted_exact_target() {
        let (mut cx,mut inbox)=widget();inbox.identity="account".into();
        finish(&mut cx,&mut inbox,json!({"approvals":[summary("agent-a","one","p1"),summary("agent-b","two","p2")],"total":80,"nextCursor":"b".repeat(64)}));
        inbox.view.modal(&cx,ids!(inbox_popup)).open(&mut cx);
        for (width,height) in [(375.0,640.0),(1100.0,760.0)] {
            draw(&mut cx,&mut inbox,width,height);
            let bounds=inbox.view.view(&cx,ids!(inbox_popup.content)).area().rect(&cx);
            assert!(bounds.size.x>200.0 && bounds.size.y>200.0,"{bounds:?}");
            assert!(bounds.pos.x>=0.0 && bounds.pos.y>=0.0 && bounds.pos.x+bounds.size.x<=width+1.0 && bounds.pos.y+bounds.size.y<=height+1.0,"{bounds:?}");
        }
        let list=inbox.view.approval_rows(&cx,ids!(inbox_rows));
        let row=list.borrow_mut().unwrap().list.item(&mut cx,1,id!(Approval));
        assert!(row.label(&cx,ids!(approval_summary)).text().contains("agent-b"));
        let button=row.button(&cx,ids!(review_approval));
        let clicks=cx.capture_actions(|cx|cx.widget_action(button.widget_uid(),ButtonAction::Clicked(KeyModifiers::default())));
        let target=inbox.clicked_target(&cx,&clicks).expect("painted row target");
        assert_eq!(target.agent,"agent-b");assert_eq!(target.binding,"two");assert_eq!(target.proposal,"p2");
        let actions=cx.capture_actions(|cx|inbox.review_for_identity(cx,&target,Some("account")));
        assert!(matches!(actions.iter().find_map(|action|action.downcast_ref::<HagencyAction>()),Some(HagencyAction::ReviewApproval(actual)) if actual==&target));
        assert_eq!(actions.len(),1,"review only navigates; it cannot submit a decision");
        finish(&mut cx,&mut inbox,json!({"approvals":[],"total":0,"nextCursor":null}));
        assert!(inbox.clicked_target(&cx,&clicks).is_none());
        assert!(inbox.rows.is_empty() && inbox.total==0);
    }
    #[test]
    fn discovered_target_is_selected_only_after_matching_fresh_scope_id_and_digest() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);
            let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        panel.identity="account".into();panel.agents=vec![json!({"id":"a"})];
        panel.labels(&mut cx,ids!(agents),"Select",&panel.agents,"id");
        panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
        panel.bindings=vec![json!({"id":"one","roomId":"!one:test"})];
        panel.label_bindings(&mut cx,ids!(bindings));panel.view.drop_down(&cx,ids!(bindings)).set_selected_item(&mut cx,1);
        let target=ApprovalTarget::from_summary("account",&serde_json::from_value(summary("a","one","p1")).unwrap());
        let row=json!({"proposalId":"p1","argsDigest":"a".repeat(64),"proposal":{"scope":{"agent":"a","binding":"one","room":"!one:test","requester":"@owner:test"},"expires":super::super::super::approvals::now()+300,"tool":"model.request","arguments":{}}});
        panel.wanted_approval=Some(target.clone());
        let emitted=cx.capture_actions(|cx|panel.apply_approvals(cx,&json!({"approvals":[row.clone()]})));
        assert!(emitted.is_empty());assert_eq!(panel.view.drop_down(&cx,ids!(approvals)).selected_item(),1);
        panel.wanted_approval=Some(target);
        let mut changed=row;changed["argsDigest"]=json!("b".repeat(64));
        panel.apply_approvals(&mut cx,&json!({"approvals":[changed]}));
        assert_eq!(panel.view.drop_down(&cx,ids!(approvals)).selected_item(),0);
        assert!(panel.view.label(&cx,ids!(approval_feedback)).text().contains("no longer pending"));
    }
    #[test]
    fn discovered_binding_loss_room_change_or_device_change_never_selects_a_replacement_room() {
        for (missing,changed_room,changed_device) in [(true,false,false),(false,true,false),(false,false,true)] {
            let mut cx=Cx::new(Box::new(|_,_|{}));
            let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);
                let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
            panel.identity="account".into();panel.agents=vec![json!({"id":"a"})];
            panel.labels(&mut cx,ids!(agents),"Select",&panel.agents,"id");panel.view.drop_down(&cx,ids!(agents)).set_selected_item(&mut cx,1);
            panel.current_device_id="here".into();panel.devices_loaded=true;
            panel.execution_devices=vec![json!({"id":"here","revoked":false}),json!({"id":"there","revoked":false})];
            panel.execution_agent_record=json!({"id":"a","generation":1,"executionDeviceId":if changed_device {"there"}else{"here"}});
            let target=ApprovalTarget::from_summary("account",&serde_json::from_value(summary("a","one","p1")).unwrap());
            panel.wanted_approval=Some(target);panel.wanted_run=Some(("a".into(),"one".into()));
            let mut bindings=vec![json!({"id":"default","roomId":"!private:test","scopeKind":"owner_direct","state":"active"})];
            if !missing {bindings.push(json!({"id":"one","roomId":if changed_room {"!changed:test"}else{"!one:test"},"state":"active"}));}
            let request=panel.request;
            let emitted=cx.capture_actions(|cx|panel.apply(cx,Target::Bindings,&json!({"bindings":bindings})));
            assert!(emitted.is_empty());assert_eq!(panel.request,request,"a lost review target must not issue a substitute resource read");
            assert_eq!(panel.view.drop_down(&cx,ids!(bindings)).selected_item(),0);
            assert!(panel.wanted_approval.is_none() && panel.wanted_run.is_none() && !panel.approval_checked);
            assert!(panel.view.label(&cx,ids!(approval_feedback)).text().contains("no longer pending"));
        }
    }
    #[test]
    fn discovery_widget_startup_observes_a_real_sdk_account_without_bootstrapping_an_owner() {
        const CHILD:&str="HAGENCY_APPROVAL_OBSERVER_CHILD";
        if std::env::var(CHILD).as_deref()!=Ok("1") {
            let directory=tempfile::tempdir().unwrap();
            let output=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","hagency::ui::approval_discovery::tests::discovery_widget_startup_observes_a_real_sdk_account_without_bootstrapping_an_owner","--nocapture"])
                .env(CHILD,"1").env("HAGENCY_APP_DATA_DIR",directory.path()).output().unwrap();
            assert!(output.status.success(),"isolated SDK observer regression failed: {} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"),"the isolated test must actually run");
            return;
        }
        // Process isolation keeps the real CLIENT/CURRENT registries away from
        // parallel tests. There is no mock Service lookup or Pending command.
        crate::sliding_sync::backend_runtime().block_on(async {
            use tokio::io::{AsyncReadExt,AsyncWriteExt};
            use std::sync::{Arc,Mutex};
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin=format!("http://{}",listener.local_addr().unwrap());
            let requests=Arc::new(Mutex::new(Vec::new()));let observed=requests.clone();
            let server=tokio::spawn(async move {
                loop {
                    let Ok((mut socket,_))=listener.accept().await else {break;};
                    let mut request=Vec::new();let mut bytes=[0u8;1024];
                    while !request.ends_with(b"\r\n\r\n") && request.len()<8192 {
                        let count=socket.read(&mut bytes).await.unwrap();if count==0 {break;}
                        request.extend_from_slice(&bytes[..count]);
                    }
                    observed.lock().unwrap().push(String::from_utf8_lossy(&request).lines().next().unwrap_or("").to_owned());
                    let _=socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await;
                }
            });
            let client=matrix_sdk::Client::builder().homeserver_url(&origin)
                .server_versions([matrix_sdk::ruma::api::MatrixVersion::V1_0]).build().await.unwrap();
            client.matrix_auth().restore_session(serde_json::from_value(json!({"access_token":"isolated-sdk-observer","user_id":"@observer:test","device_id":"OBSERVER"})).unwrap(),Default::default()).await.unwrap();
            // restore_session spawns the SDK's own backup/recovery setup. Join
            // that original task before observing the UI, rather than attributing
            // its asynchronous account-data reads to an owner startup.
            tokio::time::timeout(std::time::Duration::from_secs(5),client.encryption().wait_for_e2ee_initialization_tasks())
                .await.expect("the SDK's original encryption setup must finish before observation");
            let setup_requests=requests.lock().unwrap().clone();
            let secret_storage_read="GET /_matrix/client/r0/user/@observer:test/account_data/m.secret_storage.default_key HTTP/1.1";
            assert_eq!(setup_requests,vec![secret_storage_read.to_owned(),secret_storage_read.to_owned()],
                "SDK setup permits only its two backup/recovery secret-storage reads; no management/login/lease/provider request is permitted");
            assert!(crate::sliding_sync::replace_client(Some(client)).is_none());
            assert!(crate::sliding_sync::get_client().unwrap().user_id().is_some());
            assert_eq!(Service::startup_bootstrap_count(),0);
            eprintln!("SDK setup requests before observer: {setup_requests:?}");
            let (mut cx,mut inbox)=widget();
            inbox.handle_event(&mut cx,&Event::Startup,&mut Scope::empty());
            inbox.handle_event(&mut cx,&Event::Signal,&mut Scope::empty());
            draw(&mut cx,&mut inbox,1100.0,760.0);
            inbox.poll(&mut cx);
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert!(Service::existing_current().is_none());
            assert_eq!(Service::startup_bootstrap_count(),0,"observation must never invoke the LoginStatus startup hook");
            eprintln!("Requests after observer: {:?}",requests.lock().unwrap());
            let observed=requests.lock().unwrap().clone();
            assert_eq!(observed,setup_requests,"observer must add zero HTTP requests, including discovery/login/lease/provider requests; actual paths: {observed:?}");
            assert!(!crate::app_data_dir().join("native-owner").exists(),"no owner directory or recovery ledger may be created");
            assert!(inbox.rows.is_empty() && !inbox.pending);
            crate::sliding_sync::replace_client(None);server.abort();
        });
    }

}
