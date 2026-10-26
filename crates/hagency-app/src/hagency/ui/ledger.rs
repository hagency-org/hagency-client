use super::*;

#[derive(Debug)]
pub(super) struct Selected {identity:String, request:u64, result:Result<Option<String>,String>, custody:Option<robius_file_picker::LocalFile>}
impl HagencyPanel {
    pub(super) fn render_ledger(&self,cx:&mut Cx) {
        let path=self.input(cx,ids!(ledger_path));
        let inspected=self.ledger_preview["operation"]=="inspected"
            && self.ledger_preview["path"].as_str()==Some(path.as_str())
            && self.ledger_preview["digest"].as_str().is_some_and(|s|s.len()==64);
        self.view.button(cx,ids!(ledger_restore)).set_enabled(cx,!self.busy && inspected && self.view.check_box(cx,ids!(ledger_confirm)).active(cx));
        for id in [ids!(ledger_export),ids!(ledger_inspect)] {self.view.button(cx,id).set_enabled(cx,!self.busy && !path.is_empty());}
    }
    pub(super) fn apply_ledger(&mut self,cx:&mut Cx,value:&Value) {
        self.ledger_preview=value.clone();
        self.view.check_box(cx,ids!(ledger_confirm)).set_active(cx,false,Animate::No);
        let snapshot=&value["snapshot"];
        let created=snapshot["createdAt"].as_i64().and_then(|t|chrono::DateTime::<chrono::Utc>::from_timestamp(t,0)).map(|t|t.to_rfc3339()).unwrap_or_default();
        let summary=crate::i18n::format("Owner: {owner}\nSaved: {time}\nRequests: {requests} · Pending calls: {pending} · Pending replies: {replies}",&[
            ("owner",string(snapshot,"owner").to_owned()),("time",created),("requests",snapshot["inboxRecords"].to_string()),("pending",snapshot["pendingCalls"].to_string()),("replies",snapshot["pendingReplies"].to_string())]);
        self.view.label(cx,ids!(ledger_summary)).set_text(cx,&summary);
        self.status(cx,match string(value,"operation") {
            "exported"=>"Owner ledger backup exported.",
            "restored"=>"Owner ledger restored. Services remain stopped; start them explicitly after checking their settings.",
            _=>"Backup checked. Review the owner and date before restoring.",
        });
        self.render_ledger(cx);
    }
    pub(super) fn accept_ledger_selection(&mut self,cx:&mut Cx,done:&Selected) {
        if !self.active || self.busy || self.identity!=done.identity || self.request!=done.request || !self.ledger_open {return;}
        match &done.result {
            Ok(Some(path))=>{
                self.ledger_file=done.custody.clone();
                self.view.text_input(cx,ids!(ledger_path)).set_text(cx,path);
                self.ledger_preview=Value::Null;
                self.view.label(cx,ids!(ledger_summary)).set_text(cx,"");
                self.view.check_box(cx,ids!(ledger_confirm)).set_active(cx,false,Animate::No);
                self.render_ledger(cx);
            }
            Ok(None)=>{},Err(_)=>self.status(cx,"Could not open the backup file picker."),
        }
    }
    pub(super) fn ledger_actions(&mut self,cx:&mut Cx,actions:&Actions)->bool {
        if self.page!=Page::Agents {return false;}
        if self.view.button(cx,ids!(ledger_toggle)).clicked(actions) {
            self.ledger_open = !self.ledger_open;self.sync_navigation(cx);return true;
        }
        if !self.view.view(cx,ids!(ledger_panel)).visible() {return false;}
        if self.view.text_input(cx,ids!(ledger_path)).changed(actions).is_some() {
            self.ledger_file=None;
            self.ledger_preview=Value::Null;
            self.view.label(cx,ids!(ledger_summary)).set_text(cx,"");
            self.view.check_box(cx,ids!(ledger_confirm)).set_active(cx,false,Animate::No);
            self.render_ledger(cx);
        }
        if self.view.check_box(cx,ids!(ledger_confirm)).changed(actions).is_some() {self.render_ledger(cx);}
        if self.view.button(cx,ids!(ledger_choose)).clicked(actions) {
            let identity=self.identity.clone();let request=self.request;
            if robius_file_picker::FileDialog::new().pick_file(move|picked| {
                let mut custody=None;
                let result=(||{let Some(file)=picked.map_err(|e|e.to_string())? else{return Ok(None);};
                    let local=file.into_local_file().map_err(|e|e.to_string())?;
                    let path=local.path().to_str().ok_or_else(||"invalid path".to_owned())?.to_owned();
                    custody=Some(local);Ok(Some(path))})();
                Cx::post_action(Selected{identity,request,result,custody});
            }).is_err() {self.status(cx,"Could not open the backup file picker.");}
            return true;
        }
        let path=self.input(cx,ids!(ledger_path));
        let action=if self.view.button(cx,ids!(ledger_export)).clicked(actions) {Some(LedgerAction::Export{path})}
        else if self.view.button(cx,ids!(ledger_inspect)).clicked(actions) {Some(LedgerAction::Inspect{path})}
        else if self.view.button(cx,ids!(ledger_restore)).clicked(actions)
            && self.view.check_box(cx,ids!(ledger_confirm)).active(cx)
            && self.ledger_preview["operation"]=="inspected"
            && self.ledger_preview["path"].as_str()==Some(path.as_str()) {
                Some(LedgerAction::Restore{path,digest:string(&self.ledger_preview,"digest").into()})
        } else {None};
        if let Some(action)=action {self.run(cx,Target::Ledger,Command::Ledger{action});return true;}
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restore_requires_inspected_exact_path_fresh_confirmation_and_idle_ui() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);vm.bx.captured_errors=Some(Vec::new());let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});let panel=HagencyPanel::script_from_value(vm,value);assert!(vm.take_errors().is_empty());panel});
        panel.active=true;panel.page=Page::Agents;panel.ledger_open=true;
        panel.view.text_input(&cx,ids!(ledger_path)).set_text(&mut cx,"/backup.hagency-ledger");
        panel.apply(&mut cx,Target::Ledger,&json!({"operation":"inspected","path":"/backup.hagency-ledger","digest":"a".repeat(64),"snapshot":{"owner":"@alice:example.org","createdAt":123,"inboxRecords":7,"pendingCalls":2,"pendingReplies":1}}));
        assert!(!panel.view.button(&cx,ids!(ledger_restore)).borrow().unwrap().enabled());
        assert!(panel.view.label(&cx,ids!(ledger_summary)).text().contains("@alice:example.org"));
        panel.view.check_box(&cx,ids!(ledger_confirm)).set_active(&mut cx,true,Animate::No);panel.render_ledger(&mut cx);
        assert!(panel.view.button(&cx,ids!(ledger_restore)).borrow().unwrap().enabled());
        panel.busy=true;panel.render_ledger(&mut cx);
        assert!(!panel.view.button(&cx,ids!(ledger_restore)).borrow().unwrap().enabled());
        panel.busy=false;panel.view.text_input(&cx,ids!(ledger_path)).set_text(&mut cx,"/different.hagency-ledger");panel.render_ledger(&mut cx);
        assert!(!panel.view.button(&cx,ids!(ledger_restore)).borrow().unwrap().enabled());
    }
    #[test]
    fn late_backup_selection_cannot_cross_account_or_request_fences() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});HagencyPanel::script_from_value(vm,value)});
        panel.active=true;panel.ledger_open=true;panel.identity="current".into();panel.request=7;
        for (identity,request) in [("old",7),("current",6)] {
            panel.accept_ledger_selection(&mut cx,&Selected{identity:identity.into(),request,result:Ok(Some("/private/old.hagency-ledger".into())),custody:None});
            assert!(panel.input(&mut cx,ids!(ledger_path)).is_empty());
        }
        panel.accept_ledger_selection(&mut cx,&Selected{identity:"current".into(),request:7,result:Ok(Some("/current.hagency-ledger".into())),custody:None});
        assert_eq!(panel.input(&mut cx,ids!(ledger_path)),"/current.hagency-ledger");
    }
}
