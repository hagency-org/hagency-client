use super::*;
use ::hagency::project_board::BoardAction;

/// Only authenticated Agent ownership metadata establishes nesting. Unknown
/// identities retain their own row, never a guessed owner based on the MXID.
pub(super) fn ordered_members(members: Vec<Value>) -> Vec<Value> {
    let mut people: Vec<_> = members.iter().filter(|m| !m["agentOwner"].is_string()).cloned().collect();
    people.sort_by_key(|m| (m["service"]==true,string(m,"name").to_lowercase(),string(m,"userId").to_owned()));
    let mut result=Vec::new();
    let mut used=HashSet::new();
    for person in people {
        let owner=string(&person,"userId").to_owned();
        used.insert(owner.clone());
        result.push(person);
        let mut agents:Vec<_>=members.iter().filter(|m|m["agentOwner"]==owner).cloned().collect();
        agents.sort_by_key(|m|(string(m,"name").to_lowercase(),string(m,"userId").to_owned()));
        for agent in agents {used.insert(string(&agent,"userId").to_owned());result.push(agent);}
    }
    // Owners outside this membership do not disappear along with their Agents.
    result.extend(members.into_iter().filter(|m|!used.contains(string(m,"userId"))));
    result
}
impl HagencyPanel {
    fn board_controls(&self,cx:&Cx)->WidgetRef {self.view.project_board(cx,ids!(board)).controls(cx)}
    fn board_creation_action(&mut self,name:&str)->BoardAction {
        let (idempotency_key,name)=self.board_creation.get_or_insert_with(||(uuid::Uuid::new_v4().to_string(),name.into())).clone();
        BoardAction::CreateBoard {idempotency_key,name}
    }
    pub(super) fn project_toolbar_tooltips(&self,cx:&mut Cx,event:&Event) {
        if !matches!(event,Event::MouseMove(_)) || !self.active {return;}
        for (id,text) in [(ids!(back).as_slice(),"Back"),(ids!(toggle_board_creation).as_slice(),"New Board"),(ids!(archive_project).as_slice(),if self.project_home["archived"]==true {"Restore Project"}else{"Archive Project"}),(ids!(refresh_members).as_slice(),"Refresh members"),(ids!(invite_member).as_slice(),if self.members_room.as_ref().is_some_and(|(_,space)|*space) {"Invite to Project"} else {"Invite to Room"})] {
            let button=if id==ids!(toggle_board_creation).as_slice() {self.board_controls(cx).button(cx,id)}else{self.view.button(cx,id)};
            if !button.visible() || button.area().rect(cx).size.x<=0.0 {continue;}
            match event.hits(cx,button.area()) {
                Hit::FingerHoverIn(_) => cx.widget_action(button.widget_uid(),TooltipAction::HoverIn {widget_rect:button.area().rect(cx),text:crate::i18n::tr(text).into(),options:CalloutTooltipOptions {position:TooltipPosition::Bottom,..Default::default()}}),
                Hit::FingerHoverOut(_) => cx.widget_action(button.widget_uid(),TooltipAction::HoverOut),
                _=>(),
            }
        }
    }

    pub(super) fn load_project_home(&mut self,cx:&mut Cx) {
        if let Some(project_id)=self.project(cx) {
            self.run(cx,Target::ProjectHome,Command::ProjectBoard {project_id,action:BoardAction::Overview});
        }
    }
    pub(super) fn apply_project_home(&mut self,cx:&mut Cx,target:Target,value:&Value) {
        if value["projectId"].as_str()!=self.project(cx).as_deref() {return;}
        self.board_catalog=array(value,"boards");
        let wanted=value["createdBoardId"].as_str().map(str::to_owned).or_else(||self.wanted_board.take()).or_else(||self.selected_board.clone());
        let index=wanted.as_deref().and_then(|id|self.board_catalog.iter().position(|b|b["id"]==id)).unwrap_or(0);
        self.selected_board=self.board_catalog.get(index).and_then(|b|b["id"].as_str()).map(str::to_owned);
        self.board_controls(cx).drop_down(cx,ids!(boards)).set_labels(cx,self.board_catalog.iter().map(|b|{
            let name=string(b,"name");if name=="Main Board" {crate::i18n::tr(name).into()}else{name.into()}
        }).collect());
        self.board_controls(cx).drop_down(cx,ids!(boards)).set_selected_item(cx,index);
        if matches!(target,Target::BoardCreated) {
            self.board_creation=None;
            self.board_creation_open=false;
            self.board_controls(cx).text_input(cx,ids!(board_name)).set_text(cx,"");
            cx.action(crate::home::project_tree::ProjectTreeChanged {identity:self.identity.clone()});
        } else {
            self.project_home=value.clone();
            self.render_overview_stats(cx);
            let empty=array(value,"activity").is_empty();
            self.view.label(cx,ids!(overview_activity)).set_text(cx,crate::i18n::tr("No recent activity"));
            self.view.label(cx,ids!(overview_activity)).set_visible(cx,empty);

        }
        self.render_project_controls(cx);
        if self.wanted_access && matches!(target,Target::ProjectHome) {
            self.wanted_access=false;
            if self.project_home["canManagePolicy"]==true {self.load_permissions(cx,false);}
        }
    }
    pub(super) fn render_overview_stats(&self,cx:&mut Cx) {
        let stats=&self.project_home["stats"];
        let known=stats.is_object();let tasks=stats["tasks"].as_u64().unwrap_or(0);let done=stats["done"].as_u64().unwrap_or(0);
        for (id,value) in [(ids!(metric_tasks).as_slice(),stats["tasks"].as_u64().unwrap_or(0).to_string()),(ids!(metric_done).as_slice(),done.to_string()),(ids!(metric_overdue).as_slice(),stats["overdue"].as_u64().unwrap_or(0).to_string()),(ids!(metric_boards).as_slice(),self.board_catalog.len().to_string()),(ids!(metric_members).as_slice(),stats["members"].as_u64().unwrap_or(0).to_string()),(ids!(metric_progress).as_slice(),format!("{}%",done.saturating_mul(100).checked_div(tasks).unwrap_or(0).min(100)))] {
            self.view.view(cx,id).label(cx,ids!(value)).set_text(cx,if known {&value}else{"—"});
        }
        self.view.label(cx,ids!(overview_archived)).set_visible(cx,self.project_home["archived"]==true);
        self.view.view(cx,ids!(overview_boards_section)).set_visible(cx,known);
    }
    pub(super) fn render_project_controls(&self,cx:&mut Cx) {
        let detail=self.page==Page::Projects && self.view.view(cx,ids!(project_detail)).visible();
        self.board_controls(cx).view(cx,ids!(board_navigation)).set_visible(cx,detail);
        self.view.check_box(cx,ids!(show_archived_projects)).set_visible(cx,self.page==Page::Projects && !detail && !self.view.view(cx,ids!(project_form)).visible());
        self.view.view(cx,ids!(project_tabs)).set_visible(cx,detail);
        let admin=detail && self.project_home["canManagePolicy"]==true;
        let archived=self.project_home["archived"]==true || self.project(cx).is_some_and(|project|self.view.project_board(cx,ids!(board)).read_only_for(&project));
        self.view.button(cx,ids!(permissions_tab)).set_visible(cx,admin);
        self.view.button(cx,ids!(archive_project)).set_visible(cx,admin);
        let mut archive=self.view.button(cx,ids!(archive_project));
        if archived {script_apply_eval!(cx,archive,{draw_icon.svg:mod.widgets.ICON_JUMP});}
        else {script_apply_eval!(cx,archive,{draw_icon.svg:mod.widgets.ICON_PROJECT_ARCHIVE});}
        self.view.button(cx,ids!(archive_project)).set_enabled(cx,admin && !self.busy);
        for (id,disabled) in [(ids!(boards).as_slice(),self.busy),(ids!(board_name).as_slice(),self.busy || self.board_creation.is_some() || archived)] {
            let widget=self.board_controls(cx).widget(cx,id);
            if widget.disabled(cx)!=disabled {widget.set_disabled(cx,disabled);}
        }
        self.board_controls(cx).button(cx,ids!(toggle_board_creation)).set_visible(cx,detail && !archived);
        self.board_controls(cx).view(cx,ids!(board_creation_controls)).set_visible(cx,detail && !archived && (self.board_creation_open || self.board_creation.is_some()));
        self.board_controls(cx).button(cx,ids!(create_board)).set_enabled(cx,!self.busy);
        self.board_controls(cx).button(cx,ids!(create_board)).set_text(cx,crate::i18n::tr(if self.board_creation.is_some() {"Retry request"}else{"New Board"}));
        self.board_controls(cx).button(cx,ids!(create_board)).set_visible(cx,detail && !archived);

        self.view.button(cx,ids!(new_room)).set_visible(cx,detail && self.project_home["canCreateRoom"]==true && !archived);
        let room_selected=self.selected(cx,ids!(rooms),&self.rooms).is_some();
        self.view.button(cx,ids!(room_permissions)).set_visible(cx,self.room_can_manage && room_selected);
    }
    pub(super) fn handle_project_navigation(&mut self,cx:&mut Cx,actions:&Actions)->bool {
        for (key,row) in self.view.flat_list(cx,ids!(overview_boards)).items_with_actions(actions) {
            if row.button(cx,ids!(open_board)).clicked(actions) && let Some(board)=self.board_catalog.iter().find(|b|LiveId::from_str(string(b,"id"))==key) {
                self.selected_board=Some(string(board,"id").into());self.project_section(cx,ids!(board_section));return true;
            }
        }

        if self.board_controls(cx).button(cx,ids!(toggle_board_creation)).clicked(actions) && !self.busy && self.project_home["archived"]!=true {
            self.board_creation_open = !self.board_creation_open;
            self.render_project_controls(cx);
            if self.board_creation_open {self.board_controls(cx).text_input(cx,ids!(board_name)).set_key_focus(cx);}
            return true;
        }
        if let Some(show)=self.view.check_box(cx,ids!(show_archived_projects)).changed(actions) {
            self.show_archived_projects=show;
            cx.action(crate::home::project_tree::ArchivedProjectsVisibility(show));
            self.run(cx,Target::Projects,Command::Projects);return true;
        }
        if let Some(index)=self.board_controls(cx).drop_down(cx,ids!(boards)).selected(actions) {
            let board=self.view.project_board(cx,ids!(board));
            if !board.can_switch(cx) {
                self.status(cx,"Save or close the current task before switching boards.");
                let old=self.board_catalog.iter().position(|b|b["id"].as_str()==self.selected_board.as_deref()).unwrap_or(0);
                self.board_controls(cx).drop_down(cx,ids!(boards)).set_selected_item(cx,old);
            } else {self.selected_board=self.board_catalog.get(index).and_then(|b|b["id"].as_str()).map(str::to_owned);}
            return true;
        }
        if self.board_controls(cx).button(cx,ids!(create_board)).clicked(actions) {
            if self.busy || self.project_home["archived"]==true || !self.view.project_board(cx,ids!(board)).can_switch(cx) {return true;}
            let Some(project_id)=self.project(cx) else {return true;};
            let name=self.board_controls(cx).text_input(cx,ids!(board_name)).text();let name=name.trim();
            if name.is_empty() || name.chars().count()>80 || name.chars().any(char::is_control) {self.status(cx,"Enter a board name (1–80 characters).");return true;}
            let action=self.board_creation_action(name);
            self.run(cx,Target::BoardCreated,Command::ProjectBoard {project_id,action});return true;
        }
        if self.view.button(cx,ids!(archive_project)).clicked(actions) {
            if self.busy || self.project_home["canManagePolicy"]!=true {return true;}
            if let Some(project_id)=self.project(cx) && let Some(expected_revision)=self.project_home["revision"].as_i64() {
                self.run(cx,Target::ProjectArchived,Command::ProjectBoard {project_id,action:BoardAction::SetProjectArchived {archived:self.project_home["archived"]!=true,expected_revision}});
            }
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn panel(cx:&mut Cx)->HagencyPanel {
        cx.with_vm(|vm| {makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);vm.bx.captured_errors=Some(Vec::new());
            let value=script_eval!(vm,{mod.widgets.HagencyPanel {}});
            let panel=HagencyPanel::script_from_value(vm,value);assert!(vm.take_errors().is_empty());panel})
    }
    #[test]
    fn archive_icon_switching_uses_registered_resources_without_script_errors() {
        let mut cx=Cx::new(Box::new(|_,_|{}));let mut p=panel(&mut cx);
        for archived in [false,true,false] {
            p.project_home=json!({"archived":archived});
            p.render_project_controls(&mut cx);
            cx.with_vm(|vm|assert!(vm.take_errors().is_empty(),"archive/restore icon changes must use registered resources"));
        }
    }
    #[test]
    fn board_creation_retries_original_intent_but_recreated_panels_never_reuse_its_key() {
        let mut cx=Cx::new(Box::new(|_,_|{}));let mut first=panel(&mut cx);
        let original=first.board_creation_action("Research");
        assert_eq!(serde_json::to_value(&original).unwrap(),serde_json::to_value(first.board_creation_action("Changed while pending")).unwrap());
        let mut recreated=panel(&mut cx);
        let fresh=recreated.board_creation_action("Research");
        assert_ne!(serde_json::to_value(original).unwrap(),serde_json::to_value(&fresh).unwrap(),"A process-local request counter can collide after restart");
        if let BoardAction::CreateBoard {idempotency_key,name}=fresh {
            assert_eq!(name,"Research");assert_eq!(uuid::Uuid::parse_str(&idempotency_key).unwrap().get_version_num(),4);
        }else{panic!("Expected a board creation command");}
    }
    #[test]
    fn project_permissions_and_member_invites_are_independent_and_stale_results_are_ignored() {
        let mut cx=Cx::new(Box::new(|_,_|{}));let mut p=panel(&mut cx);
        p.projects=vec![json!({"id":"p","name":"Project","spaceId":"!p:test","active":true})];
        p.labels(&mut cx,ids!(projects),"Select a Project",&p.projects,"id");
        p.view.drop_down(&cx,ids!(projects)).set_selected_item(&mut cx,1);
        p.view.view(&cx,ids!(project_detail)).set_visible(&mut cx,true);
        for admin in [false,true,false] {
            p.apply_project_home(&mut cx,Target::ProjectHome,&json!({"projectId":"p","boards":[{"id":"p","name":"Main Board"}],"stats":{"tasks":3,"done":1,"overdue":0,"members":2},"activity":[],"canManagePolicy":admin,"canCreateRoom":true,"archived":false,"revision":1}));
            assert_eq!(p.view.button(&cx,ids!(permissions_tab)).visible(),admin);
            assert_eq!(p.view.button(&cx,ids!(archive_project)).visible(),admin);
            assert!(p.view.button(&cx,ids!(new_room)).visible(),"Room creation follows the Space child permission independently");
        }
        p.members_room=Some(("!p:test".into(),true));
        for can_invite in [true,false] {
            p.apply(&mut cx,Target::Members,&json!({"roomId":"!p:test","space":true,"canInvite":can_invite,"members":[]}));
            assert_eq!(p.view.button(&cx,ids!(invite_member)).visible(),can_invite);
            assert!(!p.view.button(&cx,ids!(permissions_tab)).visible());
        }
        p.apply_project_home(&mut cx,Target::ProjectHome,&json!({"projectId":"other","canManagePolicy":true}));
        assert!(!p.view.button(&cx,ids!(permissions_tab)).visible());
    }
    #[test]
    fn switching_projects_clears_overview_and_board_scope_before_starting_any_new_reads() {
        let mut cx=Cx::new(Box::new(|_,_|{}));let mut p=panel(&mut cx);
        p.projects=vec![json!({"id":"a","name":"First","spaceId":"!a:test","active":true}),json!({"id":"b","name":"Second","spaceId":"!b:test","active":true})];
        p.labels(&mut cx,ids!(projects),"Select a Project",&p.projects,"id");p.view.drop_down(&cx,ids!(projects)).set_selected_item(&mut cx,1);
        p.selected_board=Some("a".into());p.project_home=json!({"projectId":"a"});
        p.view.view(&cx,ids!(metric_tasks)).label(&cx,ids!(value)).set_text(&mut cx,"999");p.view.label(&cx,ids!(overview_activity)).set_text(&mut cx,"Old activity");
        p.wanted_project_section=Some(("b".into(),ProjectDetailSection::Board));p.project_detail(&mut cx,1);
        assert_eq!(p.project(&mut cx).as_deref(),Some("b"));assert!(p.selected_board.is_none());assert!(p.project_home.is_null());
        assert_eq!(p.view.view(&cx,ids!(metric_tasks)).label(&cx,ids!(value)).text(),"—");assert_eq!(p.view.label(&cx,ids!(overview_activity)).text(),crate::i18n::tr("Loading…"));
    }
    #[test]
    fn project_creation_never_shows_or_uses_the_existing_space_selector() {
        let mut cx=Cx::new(Box::new(|_,_|{}));let mut p=panel(&mut cx);
        p.create_room=false;
        p.view.drop_down(&cx,ids!(creation_source)).set_labels(&mut cx,vec!["new".into(),"old".into()]);
        p.view.drop_down(&cx,ids!(creation_source)).set_selected_item(&mut cx,1);
        p.creation_source(&mut cx);
        assert!(!p.view.view(&cx,ids!(creation_source_holder)).visible());
        assert!(!p.view.view(&cx,ids!(candidates_holder)).visible());
        assert!(p.view.view(&cx,ids!(creation_name_holder)).visible());
        assert_eq!(p.view.button(&cx,ids!(create_project_room)).text(),crate::i18n::tr("Create Project"));
    }
    #[test]
    fn agents_follow_their_actual_owner_with_roles_and_invitations_preserved() {
        let rows=ordered_members(vec![json!({"userId":"agent-b","name":"B bot","agentOwner":"bob","role":"Member"}),json!({"userId":"bob","name":"Bob","role":"Administrator"}),json!({"userId":"agent-a","name":"A bot","agentOwner":"alice","membership":"invite"}),json!({"userId":"alice","name":"Alice"}),json!({"userId":"orphan","name":"Agent","agentOwner":"outside"})]);
        assert_eq!(rows.iter().map(|r|string(r,"userId")).collect::<Vec<_>>(),vec!["alice","agent-a","bob","agent-b","orphan"]);
        assert_eq!(rows[1]["membership"],"invite");assert_eq!(rows[2]["role"],"Administrator");
    }
    #[test]
    fn overview_uses_actual_metrics_and_never_exposes_a_project_space_shortcut() {
        let mut cx=Cx::new(Box::new(|_,_|{}));let mut panel=panel(&mut cx);
        panel.project_home=json!({"stats":{"tasks":8,"done":3,"overdue":1,"members":4}});
        panel.board_catalog=vec![json!({"id":"main"}),json!({"id":"next"})];panel.render_overview_stats(&mut cx);
        assert_eq!(panel.view.view(&cx,ids!(metric_tasks)).label(&cx,ids!(value)).text(),"8");
        assert_eq!(panel.view.view(&cx,ids!(metric_progress)).label(&cx,ids!(value)).text(),"37%");
        assert_eq!(panel.view.view(&cx,ids!(metric_boards)).label(&cx,ids!(value)).text(),"2");
        assert!(panel.view.button(&cx,ids!(open_space)).is_empty());
    }

}
