//! Workspace navigation for shared Rooms; links never grant membership.
use std::collections::HashSet;
use makepad_widgets::*;
use matrix_sdk::{Client, RoomState, ruma::{OwnedRoomId, OwnedUserId}};
use crate::{hagency::{backend::Service, model::Command, ui::{HagencyAction, ProjectDetailSection}}, sliding_sync::{get_client, spawn_async_task}, utils::RoomNameId};
use crate::shared::navigation_bar_button::NavigationBarButtonWidgetExt;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    let Tab = RobrixNeutralIconButton {height:32 padding:Inset{left:10 right:10} icon_walk:Walk{width:0 height:0} spacing:0}
    mod.widgets.RoomNavigation = #(RoomNavigation::register_widget(vm)) {
        width:Fill height:Fit flow:Down padding:Inset{top:8 left:12 right:20 bottom:6} spacing:6
        header := View {width:Fill height:Fit spacing:6 align:Align{y:0.5}
            title_row := View {width:Fill height:32 spacing:8 align:Align{y:0.5}
                back := RinxIconButton {width:32 height:32 draw_icon.svg:ICON_ARROW_BACK}
                heading := RinxLabel {width:Fill max_lines:1 text_overflow:Ellipsis draw_text.text_style:theme.font_bold{font_size:mod.widgets.RINX_TITLE_SIZE}}
                loading := LoadingSpinner {visible:false width:18 height:18 draw_bg.color:mod.widgets.RINX_MUTED}
            }
            tabs := View {width:Fit height:Fit flow:Flow.Right{wrap:true} spacing:4
                overview := Tab {text:#(crate::i18n::tr("Overview")) i18n_text:"Overview"}
                boards := Tab {visible:false text:#(crate::i18n::tr("Boards")) i18n_text:"Boards"}
                rooms_tab := Tab {text:#(crate::i18n::tr("Rooms")) i18n_text:"Rooms"
                    draw_bg +: {color:mod.widgets.RINX_ACCENT color_hover:mod.widgets.RINX_ACCENT_HOVER color_down:mod.widgets.RINX_ACCENT_DOWN}
                    draw_text +: {color:mod.widgets.RINX_ON_ACCENT color_hover:mod.widgets.RINX_ON_ACCENT color_down:mod.widgets.RINX_ON_ACCENT}}
                members := Tab {text:#(crate::i18n::tr("Members")) i18n_text:"Members"}
                access := Tab {visible:false text:#(crate::i18n::tr("Access")) i18n_text:"Access"}
            }
        }
        toolbar := View {width:Fill height:Fit spacing:6 align:Align{y:0.5}
            navigation := View {width:Fit height:32 spacing:6 align:Align{y:0.5}
                rooms := RobrixSettingsDropDown {width:220 height:32 margin:0 labels:[#(crate::i18n::tr("Select a Room"))]}
                add_holder := View {visible:false width:32 height:32
                    add := NavigationBarButton {width:32 height:32 padding:8 tooltip_below:true tooltip_text:#(crate::i18n::tr("Add discussion Room")) Icon {icon_walk:Walk{width:16 height:16} draw_icon +: {svg:ICON_ADD color:mod.widgets.RINX_INK}}}
                }
            }
            tools := View {width:Fill height:32 spacing:6 align:Align{x:1.0 y:0.5}
                default_badge := mod.widgets.RinxDefaultBadge {visible:false label +: {i18n_text:"default"}}
                set_default := RinxGhostButton {visible:false height:32 text:#(crate::i18n::tr("Set as default")) i18n_text:"Set as default"}
                search := NavigationBarButton {width:32 height:32 padding:8 tooltip_below:true tooltip_text:#(crate::i18n::tr("Search Chat History")) Icon {icon_walk:Walk{width:16 height:16} draw_icon +: {svg:ICON_SEARCH color:mod.widgets.RINX_INK}}}
                media := NavigationBarButton {width:32 height:32 padding:8 tooltip_below:true tooltip_text:#(crate::i18n::tr("Shared Attachments")) Icon {icon_walk:Walk{width:16 height:16} draw_icon +: {svg:ICON_ADD_ATTACHMENT color:mod.widgets.RINX_INK}}}
                refresh := RinxIconButton {width:32 height:32 draw_icon.svg:crate_resource("self://resources/icons/article_refresh.svg")}
                info := RinxRoomInfoToggleButton {}
            }
        }
        status := RinxHint {visible:false width:Fill height:Fit flow:Flow.Right{wrap:true}}
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Pin {room:OwnedRoomId, owner:OwnedUserId, origin:String, epoch:u64}
impl Pin {
    fn current(&self)->bool {
        !crate::sliding_sync::account_changing() && self.epoch==crate::account_session::epoch()
            && get_client().is_some_and(|c|c.user_id()==Some(self.owner.as_ref()) && c.homeserver().as_str()==self.origin)
    }
}
#[derive(Clone, Debug)]
struct Navigation {title:String, project:Option<String>, parent:Option<RoomNameId>, rooms:Vec<RoomNameId>, can_add:bool, can_manage:bool, default_room:Option<OwnedRoomId>, revision:i64, archived:bool}
#[derive(Debug)]
struct Loaded {pin:Pin, request:u64, identity:String, mutation:bool, result:Result<Navigation,String>}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RoomNavigationAction {Search, Media, Info, Members}

async fn load(client:&Client, service:&Service, current:&OwnedRoomId)->Result<Navigation,String> {
    let value=service.execute(Command::Projects).await?;
    let projects=value["projects"].as_array().ok_or("invalid_project_response")?;
    if projects.len()>128 {return Err("room_discovery_incomplete".into());}
    let mut project_rooms=HashSet::new();let mut project_spaces=HashSet::new();let mut context=None;
    for project in projects.iter().filter(|p|p["active"]==true) {
        let id=project["id"].as_str().ok_or("invalid_project_response")?;
        project_spaces.insert(project["spaceId"].as_str().ok_or("invalid_project_response")?.to_owned());
        let value=service.execute(Command::ProjectRooms {project_id:id.into()}).await?;
        let records=value["rooms"].as_array().ok_or("invalid_project_response")?;
        let ids=records.iter().filter(|r|r["active"]==true).map(|r|r["roomId"].as_str().ok_or("invalid_project_response")).collect::<Result<Vec<_>,_>>()?;
        project_rooms.extend(ids.iter().map(|id|(*id).to_owned()));
        if ids.contains(&current.as_str()) {
            if context.is_some() {return Err("ambiguous_room_project".into());}
            let default_room=records.iter().find(|r|r["isDefault"]==true).and_then(|r|r["roomId"].as_str()).and_then(|id|id.parse().ok());
            let overview=service.execute(Command::ProjectBoard {project_id:id.into(),action: ::hagency::project_board::BoardAction::Overview}).await?;
            context=Some(Navigation {title:project["name"].as_str().unwrap_or(id).into(),project:Some(id.into()),parent:None,
                rooms:project_room_names(records,default_room.as_ref()).into_iter().filter(|name|client.get_room(name.room_id()).is_none_or(|room|!room.is_space() && room.direct_targets().is_empty())).collect(),
                default_room,revision:value["navigationRevision"].as_i64().unwrap_or(-1),
                can_add:overview["canCreateRoom"]==true && overview["archived"]!=true,can_manage:overview["canManagePolicy"]==true,archived:overview["archived"]==true});
        }
    }
    if let Some(context)=context {return Ok(context);}
    let mut parents=Vec::new();
    for room in client.joined_rooms().into_iter().filter(|r|r.is_space() && !project_spaces.contains(r.room_id().as_str())) {
        use matrix_sdk::{deserialized_responses::SyncOrStrippedState,ruma::events::{space::child::SpaceChildEventContent,SyncStateEvent}};
        let children=room.get_state_events_static::<SpaceChildEventContent>().await.map_err(|e|e.to_string())?.into_iter().filter_map(|raw|match raw.deserialize() {
            Ok(SyncOrStrippedState::Sync(SyncStateEvent::Original(e))) if !e.content.via.is_empty()=>Some(e.state_key.to_string()),_=>None,
        }).collect::<Vec<_>>();
        if children.iter().any(|id|id==current.as_str()) {parents.push((room,children));}
    }
    parents.sort_by_key(|(room,_)|room.room_id().to_owned());
    let (parent, candidates, can_add)=if let Some((parent,children))=parents.into_iter().next() {
        let can_add=super::space_management::can_add_room(client,parent.room_id()).await;
        (Some(RoomNameId::from_room(&parent).await),children,can_add)
    }else {(None,client.joined_rooms().into_iter().filter(|r|!r.is_space() && r.direct_targets().is_empty()).map(|r|r.room_id().to_string()).collect(),true)};
    let rooms=room_names(client,candidates.into_iter().filter(|id|!project_rooms.contains(id)),None).await;
    Ok(Navigation {title:parent.as_ref().map_or_else(||crate::i18n::tr("Group Chats").into(),|p|p.display().into_owned()),project:None,parent,rooms,can_add,can_manage:false,default_room:None,revision:-1,archived:false})
}
fn project_room_names(records:&[serde_json::Value],default_room:Option<&OwnedRoomId>)->Vec<RoomNameId> {
    let mut seen=HashSet::new();let mut rooms=Vec::new();
    for record in records.iter().filter(|r|r["active"]==true && matches!(r["membership"].as_str(),Some("join"|"invite"))) {
        let Some(id)=record["roomId"].as_str().and_then(|id|id.parse::<OwnedRoomId>().ok()).filter(|id|seen.insert(id.clone())) else {continue;};
        let room=record["name"].as_str().filter(|name|!name.trim().is_empty()).map(|name|RoomNameId::new(matrix_sdk::RoomDisplayName::Named(name.into()),id.clone())).unwrap_or_else(||RoomNameId::empty(id));
        rooms.push(room);
    }
    rooms.sort_by_key(|r|(default_room!=Some(r.room_id()),r.display().to_lowercase(),r.room_id().clone()));rooms
}
async fn room_names(client:&Client, ids:impl IntoIterator<Item=String>, default_room:Option<&OwnedRoomId>)->Vec<RoomNameId> {
    let mut seen=HashSet::new();let mut rooms=Vec::new();
    for id in ids {
        if let Ok(id)=matrix_sdk::ruma::RoomId::parse(id) && seen.insert(id.clone())
            && let Some(room)=client.get_room(&id)
            && !room.is_space() && room.direct_targets().is_empty() && matches!(room.state(),RoomState::Joined|RoomState::Invited) {
            rooms.push(RoomNameId::from_room(&room).await);
        }
    }
    rooms.sort_by_key(|r|(default_room!=Some(r.room_id()),r.display().to_lowercase(),r.room_id().clone()));rooms
}

#[derive(Script, ScriptHook, Widget)]
pub struct RoomNavigation {
    #[deref] view:View,
    #[rust] pin:Option<Pin>,
    #[rust] identity:String,
    #[rust] request:u64,
    #[rust] data:Option<Navigation>,
    #[rust] busy:bool,
    #[rust] announced:bool,
}
impl RoomNavigation {
    fn current(&self)->bool {self.pin.as_ref().is_some_and(Pin::current) && Service::for_current().is_ok_and(|s|s.identity()==self.identity)}
    fn request(&mut self,cx:&mut Cx,command:Option<crate::hagency::model::Command>) {
        let (Some(pin),Some(client),Ok(service))=(self.pin.clone(),get_client(),Service::for_current()) else {return;};
        if !pin.current() {return;}
        static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(1);
        self.request=NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed);let request=self.request;
        self.identity=service.identity();let identity=self.identity.clone();self.busy=true;
        self.view.widget(cx,ids!(loading)).set_visible(cx,true);
        spawn_async_task(async move {
            let mutation=command.is_some();
            let result=tokio::time::timeout(std::time::Duration::from_secs(30),async {
                if let Some(command)=command {service.execute(command).await?;Cx::post_action(super::project_tree::ProjectTreeChanged {identity:identity.clone()});}
                load(&client,&service,&pin.room).await
            }).await.unwrap_or_else(|_|Err("room_discovery_unavailable".into()));
            Cx::post_action(Loaded {pin,request,identity,mutation,result});
        });self.view.redraw(cx);
    }
    fn set_room(&mut self,cx:&mut Cx,room:Option<&RoomNameId>)->bool {
        let client=get_client();
        let pin=room.zip(client.as_ref()).filter(|(_,c)|!crate::sliding_sync::account_changing() && c.user_id().is_some())
            .map(|(r,c)|Pin {room:r.room_id().clone(),owner:c.user_id().unwrap().to_owned(),origin:c.homeserver().to_string(),epoch:crate::account_session::epoch()});
        if pin!=self.pin {
            self.pin=pin;self.announced=false;self.data=None;self.identity.clear();self.busy=false;
            self.view.label(cx,ids!(heading)).set_text(cx,&room.map_or_else(||crate::i18n::tr("Group Chats").into(),|r|r.display().into_owned()));
            self.view.drop_down(cx,ids!(rooms)).set_labels(cx,vec![room.map_or_else(||crate::i18n::tr("Select a Room").into(),|r|r.display().into_owned())]);
            self.view.drop_down(cx,ids!(rooms)).set_selected_item(cx,0);
            self.view.label(cx,ids!(status)).set_visible(cx,false);self.request(cx,None);
        }
        self.pin.is_some()
    }
    fn sync(&self,cx:&mut Cx) {
        let data=self.data.as_ref();
        for (id,visible) in [(ids!(boards),data.is_some_and(|d|d.project.is_some())),(ids!(overview),data.is_some_and(|d|d.project.is_some() || d.parent.is_some())),(ids!(access),data.is_some_and(|d|d.can_manage))] {
            let button=self.view.button(cx,id);if button.visible()!=visible {button.set_visible(cx,visible);}
        }
        let is_default=data.is_some_and(|d|d.default_room.as_ref()==self.pin.as_ref().map(|p|&p.room) && d.default_room.is_some());
        let badge=self.view.view(cx,ids!(default_badge));if badge.visible()!=is_default {badge.set_visible(cx,is_default);}
        let set=self.view.button(cx,ids!(set_default));let show=data.is_some_and(|d|d.can_manage && !d.archived && d.project.is_some() && d.revision>=0) && !is_default;
        if set.visible()!=show {set.set_visible(cx,show);}let enabled=!self.busy && self.current();
        if set.borrow().is_some_and(|b|b.enabled()!=enabled) {set.set_enabled(cx,enabled);}
        let add=self.view.view(cx,ids!(add_holder));let visible=data.is_some_and(|d|d.can_add);
        if add.visible()!=visible {add.set_visible(cx,visible);}
        let selector=self.view.drop_down(cx,ids!(rooms));let disabled=self.busy || !self.current() || data.is_none();
        if selector.disabled(cx)!=disabled {selector.set_disabled(cx,disabled);}
        let refresh=self.view.button(cx,ids!(refresh));
        if refresh.borrow().is_some_and(|b|b.enabled()==self.busy) {refresh.set_enabled(cx,!self.busy);}
        let spinner=self.view.widget(cx,ids!(loading));if spinner.visible()!=self.busy {spinner.set_visible(cx,self.busy);}
    }
    fn matches_response(&self,loaded:&Loaded)->bool {self.pin.as_ref()==Some(&loaded.pin) && self.request==loaded.request && self.identity==loaded.identity}
    fn accepts(&self,loaded:&Loaded)->bool {self.matches_response(loaded) && self.current()}
}
impl Widget for RoomNavigation {
    fn handle_event(&mut self,cx:&mut Cx,event:&Event,scope:&mut Scope) {
        self.view.handle_event(cx,event,scope);
        if let Event::Actions(actions)=event {
            for action in actions {
                if let Some(loaded)=action.downcast_ref::<Loaded>() && self.accepts(loaded) {
                    self.busy=false;
                    match &loaded.result {
                        Ok(data)=>{
                            self.announced=false;self.data=Some(data.clone());
                            self.view.label(cx,ids!(heading)).set_text(cx,&data.title);
                            self.view.drop_down(cx,ids!(rooms)).set_labels(cx,data.rooms.iter().map(|r|if data.default_room.as_ref()==Some(r.room_id()) {format!("{} · {}",r.display(),crate::i18n::tr("default"))}else{r.display().into_owned()}).collect());
                            let index=data.rooms.iter().position(|r|Some(r.room_id())==self.pin.as_ref().map(|p|&p.room)).unwrap_or(0);
                            self.view.drop_down(cx,ids!(rooms)).set_selected_item(cx,index);
                            self.view.label(cx,ids!(status)).set_visible(cx,false);
                        }
                        Err(_)=>{self.data=None;self.view.label(cx,ids!(status)).set_text(cx,crate::i18n::tr(if loaded.mutation {"Could not set default. Refresh to retry."}else{"Could not load Rooms. Refresh to retry."}));self.view.label(cx,ids!(status)).set_visible(cx,true);}
                    }
                    self.sync(cx);self.view.redraw(cx);
                }
                if action.downcast_ref::<super::room_settings::RoomSettingsChanged>().is_some_and(|changed|self.pin.as_ref().is_some_and(|pin|pin.room==*changed.0.room_id())) {
                    self.request(cx,None);
                }
                if action.downcast_ref::<super::space_management::SpaceManagementChanged>().is_some() || action.downcast_ref::<super::project_tree::ProjectTreeChanged>().is_some() {
                    if !self.busy {self.request(cx,None);}
                }
            }
            if self.view.button(cx,ids!(refresh)).clicked(actions) && !self.busy {self.request(cx,None);}
            if !self.current() {return;}
            for (id,action) in [(ids!(search),RoomNavigationAction::Search),(ids!(media),RoomNavigationAction::Media),(ids!(info),RoomNavigationAction::Info)] {
                if self.view.navigation_bar_button(cx,id).clicked(actions) {cx.widget_action(self.widget_uid(),action);}
            }
            if self.view.button(cx,ids!(back)).clicked(actions) {
                if self.data.as_ref().is_some_and(|d|d.project.is_some()) {cx.action(HagencyAction::OpenProjects);}else{cx.action(super::main_desktop_ui::MainDesktopUiAction::ShowGroupChats);}
            }
            let Some(data)=&self.data else {return;};
            if let Some(index)=self.view.drop_down(cx,ids!(rooms)).selected(actions)
                && let Some(room)=data.rooms.get(index) && Some(room.room_id())!=self.pin.as_ref().map(|p|&p.room)
                && let Some(sdk)=get_client().and_then(|c|c.get_room(room.room_id())) {
                let selected=match sdk.state() {RoomState::Joined=>Some(crate::app::SelectedRoom::JoinedRoom {room_name_id:room.clone()}),RoomState::Invited=>Some(crate::app::SelectedRoom::InvitedRoom {room_name_id:room.clone()}),_=>None};
                if let Some(selected)=selected {
                    if let Some(project_id)=&data.project {cx.action(super::project_tree::ProjectSelectionChanged {identity:self.identity.clone(),project_id:project_id.clone(),target:super::project_tree::ProjectSelection::Room(room.room_id().clone())});}
                    cx.widget_action(self.widget_uid(),super::rooms_list::RoomsListAction::Selected(selected));
                }
            }
            if self.view.button(cx,ids!(set_default)).clicked(actions) && data.can_manage && !data.archived && data.revision>=0 && !self.busy
                && let (Some(project_id),Some(pin))=(data.project.clone(),self.pin.clone()) {
                let command=Command::ProjectBoard {project_id,action: ::hagency::project_board::BoardAction::SetNavigationDefault {kind:"room".into(),target_id:pin.room.to_string(),expected_revision:data.revision}};
                self.request(cx,Some(command));return;
            }
            if self.view.navigation_bar_button(cx,ids!(add)).clicked(actions) && data.can_add && !self.busy {
                if let Some(project_id)=&data.project {cx.action(HagencyAction::NewRoom {project_id:project_id.clone()});}
                else {cx.action(super::space_management::SpaceManagementAction::CreateRoom {parent:data.parent.clone()});}
            }
            for (id,section) in [(ids!(overview),ProjectDetailSection::Overview),(ids!(boards),ProjectDetailSection::Board),(ids!(members),ProjectDetailSection::Members),(ids!(access),ProjectDetailSection::Access)] {
                if !self.view.button(cx,id).clicked(actions) {continue;}
                if let Some(project_id)=&data.project {cx.action(HagencyAction::ProjectSection {project_id:project_id.clone(),section});}
                else if id==ids!(members) {cx.widget_action(self.widget_uid(),RoomNavigationAction::Members);}
                else if let Some(parent)=&data.parent {cx.action(crate::app::AppStateAction::NavigateToRoom {room_to_close:None,destination_room:crate::room::BasicRoomDetails::RoomId(parent.clone())});}
            }
        }
    }
    fn draw_walk(&mut self,cx:&mut Cx2d,scope:&mut Scope,walk:Walk)->DrawStep {
        let width=cx.peek_walk_turtle(walk).size.x;
        // Mutate layout directly: script_apply_eval has its own scope and does
        // not import the Flow/Size aliases from this widget's script_mod.
        if let Some(mut header)=self.view.view(cx,ids!(header)).borrow_mut() {header.layout.flow=if width<760.0 {Flow::Down}else{Flow::right()};}
        if let Some(mut tabs)=self.view.view(cx,ids!(tabs)).borrow_mut() {tabs.walk.width=if width<760.0 {Size::fill()}else{Size::fit()};}
        if let Some(mut toolbar)=self.view.view(cx,ids!(toolbar)).borrow_mut() {toolbar.layout.flow=if width<600.0 {Flow::Down}else{Flow::right()};}
        if !self.announced && self.current() && let (Some(data),Some(pin))=(&self.data,&self.pin) && let Some(project_id)=&data.project {
            // Only a drawn Room can own sidebar selection. A late response to a
            // cached/hidden Room must never steal it from the active Board.
            cx.action(super::project_tree::ProjectSelectionChanged {identity:self.identity.clone(),project_id:project_id.clone(),target:super::project_tree::ProjectSelection::Room(pin.room.clone())});
            self.announced=true;
        }
        self.sync(cx);self.view.draw_walk(cx,scope,walk)
    }
}
impl RoomNavigationRef {
    pub(crate) fn requested(&self,actions:&Actions,action:RoomNavigationAction)->bool {
        actions.iter().any(|a|a.as_widget_action().is_some_and(|a|a.widget_uid==self.widget_uid() && a.action.downcast_ref::<RoomNavigationAction>()==Some(&action)))
    }
    pub(crate) fn set_room(&self,cx:&mut Cx,room:Option<&RoomNameId>)->bool {self.borrow_mut().is_some_and(|mut w|w.set_room(cx,room))}
    pub(crate) fn set_info_visible(&self,cx:&mut Cx,show:bool) {if let Some(w)=self.borrow(){w.view.navigation_bar_button(cx,ids!(info)).set_selected(cx,show);}}
    pub(crate) fn info_rect(&self,cx:&Cx)->Rect {self.borrow().map_or(Rect::default(),|w|w.view.navigation_bar_button(cx,ids!(info)).area().rect(cx))}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn widget(cx:&mut Cx)->RoomNavigation {
        cx.with_vm(|vm| {makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);vm.bx.captured_errors=Some(Vec::new());
            let value=script_eval!(vm,{mod.widgets.RoomNavigation {}});let nav=RoomNavigation::script_from_value(vm,value);
            assert!(vm.take_errors().is_empty());nav})
    }
    fn draw(cx:&mut Cx,nav:&mut RoomNavigation,width:f64) {
        cx.redraw_all();let pass=DrawPass::new(cx);pass.set_size(cx,dvec2(width,400.0));let mut list=DrawList2d::new(cx);let overlay=Overlay{draw_list:DrawList::new(cx)};let event=DrawEvent::default();let mut draw=CxDraw::new(cx,&event);
        let mut cx2d=Cx2d::new(&mut draw);cx2d.begin_pass(&pass,None);list.begin_always(&mut cx2d);overlay.begin(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(width,400.0),Layout::flow_down());nav.draw_walk_all(&mut cx2d,&mut Scope::empty(),Walk{width:Size::Fixed(width),height:Size::Fit{min:None,max:None},..Walk::default()});
        overlay.end(&mut cx2d);cx2d.end_pass_sized_turtle();list.end(&mut cx2d);cx2d.end_pass(&pass);
    }
    #[test]
    fn project_selector_uses_current_authorized_metadata_before_sdk_sync() {
        let records=serde_json::json!([
            {"roomId":"!old:test","name":"A room","active":true,"membership":"join"},
            {"roomId":"!new:test","name":"Renamed Room","active":true,"membership":"invite","isDefault":true},
            {"roomId":"!left:test","name":"Hidden","active":true,"membership":"leave"},
            {"roomId":"!inactive:test","name":"Inactive","active":false,"membership":"join"}
        ]);
        let rooms=project_room_names(records.as_array().unwrap(),Some(&matrix_sdk::ruma::room_id!("!new:test").into()));
        assert_eq!(rooms.len(),2);assert_eq!(rooms[0].display(),"Renamed Room");
        assert_eq!(rooms[0].room_id().as_str(),"!new:test");
    }
    #[test]
    fn room_header_uses_compact_rows_and_wraps_without_clipping_the_selector_or_tools() {
        let mut cx=Cx::new(Box::new(|_,_|{}));let mut nav=widget(&mut cx);
        nav.data=Some(Navigation {title:"Project".into(),project:Some("p".into()),parent:None,rooms:vec![],can_add:true,can_manage:true,default_room:None,revision:0,archived:false});
        nav.view.label(&cx,ids!(heading)).set_text(&mut cx,"A long Project title that must leave space for its tabs");
        for width in [900.0,310.0] {
            draw(&mut cx,&mut nav,width);
            cx.with_vm(|vm|assert!(vm.take_errors().is_empty(),"responsive Room header must evaluate without script errors"));
            let selector=nav.view.drop_down(&cx,ids!(rooms)).area().rect(&cx);let add=nav.view.navigation_bar_button(&cx,ids!(add)).area().rect(&cx);
            let tools=nav.view.view(&cx,ids!(tools)).area().rect(&cx);let header=nav.view.view(&cx,ids!(header)).area().rect(&cx);
            let tabs=nav.view.view(&cx,ids!(tabs)).area().rect(&cx);assert!(tabs.pos.x+tabs.size.x<=width-19.0,"tabs must fit at {width}: {tabs:?}");
            assert_eq!(selector.size,dvec2(220.0,32.0));assert_eq!(add.size,dvec2(32.0,32.0));
            assert!(selector.pos.y>=header.pos.y+header.size.y);assert!(add.pos.x+add.size.x<=width-19.0);
            assert!(tools.pos.x>=0.0 && tools.pos.x+tools.size.x<=width && tools.size.y==32.0);
            if width<600.0 {assert!(tools.pos.y>=selector.pos.y+selector.size.y,"width={width} tools={tools:?} selector={selector:?}");}
            else {assert_eq!(selector.pos.y,tools.pos.y);}
        }
        nav.data.as_mut().unwrap().project=None;nav.data.as_mut().unwrap().can_manage=false;nav.sync(&mut cx);
        assert!(!nav.view.button(&cx,ids!(boards)).visible());assert!(!nav.view.button(&cx,ids!(access)).visible());
    }
    #[test]
    fn unchanged_header_controls_do_not_schedule_another_frame() {
        let mut cx=Cx::new(Box::new(|_,_|{}));let nav=widget(&mut cx);
        nav.sync(&mut cx);cx.new_draw_event=DrawEvent::default();nav.sync(&mut cx);
        assert!(!cx.new_draw_event.redraw_all && cx.new_draw_event.draw_lists.is_empty(),"unchanged controls must not dirty each draw");
    }
    #[test]
    fn results_and_toolbar_actions_are_pinned_to_the_original_room_account_request_and_widget() {
        let mut cx=Cx::new(Box::new(|_,_|{}));let mut nav=widget(&mut cx);
        let pin=Pin {room:matrix_sdk::ruma::room_id!("!room:s").into(),owner:matrix_sdk::ruma::user_id!("@owner:s").into(),origin:"https://s/".into(),epoch:3};
        nav.pin=Some(pin.clone());nav.identity="owner-service".into();nav.request=8;
        let mut loaded=Loaded {pin,request:8,identity:nav.identity.clone(),mutation:false,result:Err("unavailable".into())};
        assert!(nav.matches_response(&loaded));assert!(!nav.accepts(&loaded),"without the same live account, even a matching reply is refused");
        loaded.request=9;assert!(!nav.matches_response(&loaded));loaded.request=8;
        loaded.pin.room=matrix_sdk::ruma::room_id!("!other:s").into();assert!(!nav.matches_response(&loaded));loaded.pin.room=nav.pin.as_ref().unwrap().room.clone();
        loaded.pin.epoch+=1;assert!(!nav.matches_response(&loaded));loaded.pin.epoch-=1;
        loaded.pin.owner=matrix_sdk::ruma::user_id!("@other:s").into();assert!(!nav.matches_response(&loaded));
        let reference=WidgetRef::new_with_inner(Box::new(nav)).as_room_navigation();
        let other=WidgetUid(99999);
        let wrong=cx.capture_actions(|cx|cx.widget_action(other,RoomNavigationAction::Info));
        assert!(!reference.requested(&wrong,RoomNavigationAction::Info));
        let own=cx.capture_actions(|cx|cx.widget_action(reference.widget_uid(),RoomNavigationAction::Info));
        assert!(reference.requested(&own,RoomNavigationAction::Info));assert!(!reference.requested(&own,RoomNavigationAction::Search));
    }
}
