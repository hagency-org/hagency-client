//! Room metadata and personal notifications, using fresh Matrix authority.
//! Single-field writes retain their original value on unknown outcomes. A retry
//! reconciles current state before resending, and never overwrites a concurrent edit.
use makepad_widgets::*;
use anyhow::{Result,anyhow,ensure};
use matrix_sdk::{Client,Room,RoomState,notification_settings::RoomNotificationMode,ruma::{OwnedRoomId,OwnedUserId,events::StateEventType}};
use crate::{matrix_context,sliding_sync::{get_client,current_user_id,spawn_async_task},utils::RoomNameId};
use super::space_management::fresh_state;

#[derive(Clone,Debug)]
pub enum RoomSettingsAction {Open(RoomNameId),Close}
#[derive(Clone,Debug)]
pub struct RoomSettingsChanged(pub RoomNameId);
#[derive(Clone,Debug,PartialEq,Eq)]
struct Pin {owner:OwnedUserId,origin:String,epoch:u64,room:OwnedRoomId,request:u64}
impl Pin {
    fn current(&self)->bool {
        self.epoch==crate::account_session::epoch() && !crate::sliding_sync::account_changing()
            && current_user_id().as_ref()==Some(&self.owner)
            && get_client().is_some_and(|c|c.homeserver().as_str()==self.origin)
    }
}
fn next_request()->u64 {static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(1);NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)}
#[derive(Clone,Debug)]
struct Snapshot {room:RoomNameId,topic:String,can_name:bool,can_topic:bool,can_invite:bool,encrypted:bool,members:usize,mode:Option<RoomNotificationMode>}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum Field {Name,Topic,Notifications}
#[derive(Clone,Debug)]
struct Intent {field:Field,value:String,base:String}
#[derive(Debug)]
struct Finished {pin:Pin,result:Result<Snapshot,String>,saved:Option<Intent>,definite:bool}

fn mode(index:usize)->Option<RoomNotificationMode> {match index {0=>Some(RoomNotificationMode::AllMessages),1=>Some(RoomNotificationMode::MentionsAndKeywordsOnly),2=>Some(RoomNotificationMode::Mute),_=>None}}
fn mode_index(mode:Option<RoomNotificationMode>)->usize {match mode {Some(RoomNotificationMode::AllMessages)=>0,Some(RoomNotificationMode::MentionsAndKeywordsOnly)=>1,Some(RoomNotificationMode::Mute)=>2,None=>3}}
fn validate(intent:&Intent)->Result<()> {
    match intent.field {
        Field::Name=>ensure!(!intent.value.trim().is_empty() && intent.value.chars().count()<=255 && !intent.value.chars().any(char::is_control),"Enter a room name of 1–255 characters."),
        Field::Topic=>ensure!(intent.value.chars().count()<=4096,"Keep the room topic under 4096 characters."),
        Field::Notifications=>ensure!(intent.value.parse::<usize>().is_ok_and(|n|n<4),"Choose a notification setting."),
    }Ok(())
}
fn reconcile(current:&str,intent:&Intent)->Result<bool> {
    if current==intent.value {return Ok(false);}
    ensure!(current==intent.base,"This setting changed in another session. Refresh to review it before saving.");
    Ok(true)
}
async fn joined_room(client:&Client,id:&OwnedRoomId)->Result<Room> {
    matrix_context::ensure_current(client)?;
    let room=client.get_room(id).ok_or_else(||anyhow!("This room is unavailable."))?;
    ensure!(room.state()==RoomState::Joined,"Join this room first.");Ok(room)
}
pub(super) async fn can_invite(client:&Client,id:&OwnedRoomId)->Result<bool> {
    let room=joined_room(client,id).await?;let state=fresh_state(client,&room).await?;
    let user=client.user_id().ok_or_else(||anyhow!("Sign in first."))?;
    Ok(state.joined.contains(user) && state.levels.user_can_invite(user))
}
async fn snapshot(client:&Client,id:&OwnedRoomId)->Result<Snapshot> {
    let room=joined_room(client,id).await?;
    let state=fresh_state(client,&room).await?;
    let user=client.user_id().ok_or_else(||anyhow!("Sign in first."))?;
    ensure!(state.joined.contains(user),"You are no longer a member of this room.");
    let mode=room.user_defined_notification_mode().await;
    matrix_context::ensure_current(client)?;
    Ok(Snapshot {room:state.name,topic:state.topic,can_name:state.levels.user_can_send_state(user,StateEventType::RoomName),can_topic:state.levels.user_can_send_state(user,StateEventType::RoomTopic),can_invite:state.levels.user_can_invite(user),encrypted:state.encrypted,members:state.joined.len(),mode})
}
async fn save(client:&Client,id:&OwnedRoomId,intent:&Intent)->(Result<Snapshot,String>,bool) {
    // Permission/conflict refusals have a known outcome. Transport failures may
    // have committed, so the original intent remains available for reconciliation.
    let before=match snapshot(client,id).await {Ok(v)=>v,Err(e)=>return (Err(e.to_string()),false)};
    let (permitted,current)=match intent.field {Field::Name=>(before.can_name,before.room.display().into_owned()),Field::Topic=>(before.can_topic,before.topic.clone()),Field::Notifications=>(true,mode_index(before.mode).to_string())};
    if !permitted {return (Err("You do not have permission to change this setting.".into()),true);}
    let changed=match reconcile(&current,intent) {Ok(c)=>c,Err(e)=>return (Err(e.to_string()),true)};
    if !changed {return (Ok(before),true);}
    let operation=async {
        let room=joined_room(client,id).await?;
        matrix_context::ensure_current(client)?;
        match intent.field {
            Field::Name=>{room.set_name(intent.value.clone()).await?;},
            Field::Topic=>{room.set_room_topic(&intent.value).await?;},
            Field::Notifications=>{
                let settings=client.notification_settings().await;
                matrix_context::ensure_current(client)?;
                match mode(intent.value.parse().unwrap_or(3)) {Some(m)=>settings.set_room_notification_mode(id,m).await?,None=>settings.delete_user_defined_room_rules(id).await?}
            }
        }
        matrix_context::ensure_current(client)?;
        snapshot(client,id).await
    }.await.map_err(|e:anyhow::Error|e.to_string());
    (operation,false)
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    let Text=RinxLabel {width:Fill height:Fit flow:Flow.Right{wrap:true}}
    mod.widgets.RoomSettingsPanel = #(RoomSettingsPanel::register_widget(vm)) {
        ..mod.widgets.SolidView
        width:Fill height:Fill flow:Down spacing:12 padding:16 draw_bg.color:mod.widgets.RINX_PAGE
        View {width:Fill height:36 spacing:8 align:Align{y:0.5}
            close := RinxIconButton {width:32 height:32 draw_icon.svg:ICON_ARROW_BACK}
            Text {width:Fill text:#(crate::i18n::tr("Room settings")) i18n_text:"Room settings" draw_text.text_style:theme.font_bold{font_size:mod.widgets.RINX_TITLE_SIZE}}
            reload := RinxIconButton {width:32 height:32 draw_icon.svg:crate_resource("self://resources/icons/article_refresh.svg")}
        }
        status := RinxHint {visible:false width:Fill height:Fit flow:Flow.Right{wrap:true}}
        retry := RinxGhostButton {visible:false text:#(crate::i18n::tr("Retry original save")) i18n_text:"Retry original save"}
        ScrollYView {width:Fill height:Fill flow:Down
            form := View {width:Fill{max:760} height:Fit flow:Down spacing:10
                View {width:Fill height:32 spacing:8 align:Align{y:0.5}
                    Text {text:#(crate::i18n::tr("Name")) i18n_text:"Name"}
                    save_name := RinxPrimaryButton {height:32 text:#(crate::i18n::tr("Save name")) i18n_text:"Save name" visible:false}
                }
                name := RinxInput {width:Fill height:36 empty_text:""}
                View {width:Fill height:32 spacing:8 align:Align{y:0.5}
                    Text {text:#(crate::i18n::tr("Topic")) i18n_text:"Topic"}
                    save_topic := RinxPrimaryButton {height:32 text:#(crate::i18n::tr("Save topic")) i18n_text:"Save topic" visible:false}
                }
                topic := RinxInput {width:Fill height:80 is_multiline:true empty_text:#(crate::i18n::tr("No topic set.")) i18n_empty_text:"No topic set."}
                security := RinxHint {width:Fill height:Fit flow:Flow.Right{wrap:true}}
                Text {text:#(crate::i18n::tr("Notifications")) i18n_text:"Notifications"}
                View {width:Fill height:36 spacing:8
                    notification_mode := RinxDropDown {width:Fill height:36 labels:[#(crate::i18n::tr("All Messages")) #(crate::i18n::tr("Mentions and Keywords")) #(crate::i18n::tr("Mute Notifications")) #(crate::i18n::tr("Use Account Defaults"))]}
                    save_notifications := RinxGhostButton {height:36 text:#(crate::i18n::tr("Save")) i18n_text:"Save"}
                }
                View {width:Fill height:Fit flow:Flow.Right{wrap:true} spacing:8
                    invite := RinxGhostButton {visible:false text:#(crate::i18n::tr("Invite people")) i18n_text:"Invite people"}
                    copy_link := RinxGhostButton {text:#(crate::i18n::tr("Copy Link to Room")) i18n_text:"Copy Link to Room"}
                }
            }
        }
    }
}
#[derive(Script,ScriptHook,Widget)]
pub struct RoomSettingsPanel {
    #[source] source:ScriptObjectRef,
    #[deref] view:View,
    #[rust] pin:Option<Pin>,
    #[rust] data:Option<Snapshot>,
    #[rust] pending:Option<Intent>,
    #[rust] busy:bool,
    #[rust] close_armed:bool,
    #[rust] refresh_armed:bool,
}
impl RoomSettingsPanel {
    fn status(&self,cx:&mut Cx,text:&str) {self.view.label(cx,ids!(status)).set_text(cx,crate::i18n::tr(text));self.view.label(cx,ids!(status)).set_visible(cx,!text.is_empty());}
    fn dirty(&self,cx:&Cx)->bool {self.data.as_ref().is_some_and(|s|self.view.text_input(cx,ids!(name)).text()!=s.room.display() || self.view.text_input(cx,ids!(topic)).text()!=s.topic || self.view.drop_down(cx,ids!(notification_mode)).selected_item()!=mode_index(s.mode))}
    fn sync(&self,cx:&mut Cx) {
        let locked=self.busy || self.pending.is_some();
        for (id,allowed) in [(ids!(name).as_slice(),self.data.as_ref().is_some_and(|s|s.can_name)),(ids!(topic).as_slice(),self.data.as_ref().is_some_and(|s|s.can_topic))] {
            let input=self.view.text_input(cx,id);if input.is_read_only()==(allowed && !locked) {input.set_is_read_only(cx,!allowed || locked);}
        }
        for (id,allowed,changed) in [(ids!(save_name).as_slice(),self.data.as_ref().is_some_and(|s|s.can_name),self.data.as_ref().is_some_and(|s|self.view.text_input(cx,ids!(name)).text()!=s.room.display())),(ids!(save_topic).as_slice(),self.data.as_ref().is_some_and(|s|s.can_topic),self.data.as_ref().is_some_and(|s|self.view.text_input(cx,ids!(topic)).text()!=s.topic))] {
            self.view.button(cx,id).set_visible(cx,allowed && changed);self.view.button(cx,id).set_enabled(cx,allowed && changed && !locked);
        }
        let dropdown=self.view.widget(cx,ids!(notification_mode));if dropdown.disabled(cx)!=locked {dropdown.set_disabled(cx,locked);}
        self.view.button(cx,ids!(save_notifications)).set_visible(cx,self.data.as_ref().is_some_and(|s|mode_index(s.mode)!=self.view.drop_down(cx,ids!(notification_mode)).selected_item()));
        self.view.button(cx,ids!(save_notifications)).set_enabled(cx,!locked && self.data.as_ref().is_some_and(|s|mode_index(s.mode)!=self.view.drop_down(cx,ids!(notification_mode)).selected_item()));
        self.view.button(cx,ids!(invite)).set_visible(cx,self.data.as_ref().is_some_and(|s|s.can_invite));
        self.view.button(cx,ids!(reload)).set_enabled(cx,!locked);
        self.view.button(cx,ids!(retry)).set_visible(cx,self.pending.is_some() && !self.busy);
    }
    fn load(&mut self,cx:&mut Cx,intent:Option<Intent>) {
        let Some(client)=get_client() else{return;};let Some(pin)=self.pin.as_mut() else{return;};if !pin.current() {return;}
        pin.request=next_request();let pin=pin.clone();self.busy=true;self.status(cx,if intent.is_some(){"Saving…"}else{"Loading…"});self.sync(cx);
        spawn_async_task(async move {
            let work=async {if let Some(intent)=&intent {save(&client,&pin.room,intent).await}else{(snapshot(&client,&pin.room).await.map_err(|e|e.to_string()),true)}};
            let (result,definite)=tokio::time::timeout(std::time::Duration::from_secs(30),work).await.unwrap_or_else(|_|(Err("The request timed out. Its result is not confirmed.".into()),false));
            Cx::post_action(Finished {pin,result,saved:intent,definite});
        });
    }
    fn start_save(&mut self,cx:&mut Cx,field:Field) {
        if self.busy || self.pending.is_some() {return;}
        let Some(data)=&self.data else{return;};
        let (value,base)=match field {Field::Name=>(self.view.text_input(cx,ids!(name)).text().trim().to_string(),data.room.display().into_owned()),Field::Topic=>(self.view.text_input(cx,ids!(topic)).text(),data.topic.clone()),Field::Notifications=>(self.view.drop_down(cx,ids!(notification_mode)).selected_item().to_string(),mode_index(data.mode).to_string())};
        let intent=Intent {field,value,base};
        if let Err(e)=validate(&intent) {self.status(cx,&e.to_string());return;}
        self.pending=Some(intent.clone());self.load(cx,Some(intent));
    }
    fn apply(&mut self,cx:&mut Cx,done:&Finished) {
        if self.pin.as_ref()!=Some(&done.pin) || !done.pin.current() {return;}
        self.busy=false;
        match &done.result {
            Ok(data)=>{
                // Refresh unchanged fields. Keep each unrelated draft's original
                // comparison base, so a later save still detects a concurrent edit.
                let mut accepted=data.clone();
                let name_dirty=self.data.as_ref().is_some_and(|old|self.view.text_input(cx,ids!(name)).text()!=old.room.display());
                let topic_dirty=self.data.as_ref().is_some_and(|old|self.view.text_input(cx,ids!(topic)).text()!=old.topic);
                let mode_dirty=self.data.as_ref().is_some_and(|old|self.view.drop_down(cx,ids!(notification_mode)).selected_item()!=mode_index(old.mode));
                if done.saved.as_ref().is_none_or(|s|s.field==Field::Name) || !name_dirty {self.view.text_input(cx,ids!(name)).set_text(cx,&data.room.display());}
                else if let Some(old)=&self.data {accepted.room=old.room.clone();}
                if done.saved.as_ref().is_none_or(|s|s.field==Field::Topic) || !topic_dirty {self.view.text_input(cx,ids!(topic)).set_text(cx,&data.topic);}
                else if let Some(old)=&self.data {accepted.topic=old.topic.clone();}
                if done.saved.as_ref().is_none_or(|s|s.field==Field::Notifications) || !mode_dirty {self.view.drop_down(cx,ids!(notification_mode)).set_selected_item(cx,mode_index(data.mode));}
                else if let Some(old)=&self.data {accepted.mode=old.mode;}
                self.view.label(cx,ids!(security)).set_text(cx,&format!("{} · {} {}",crate::i18n::tr(if data.encrypted{"End-to-end encrypted"}else{"Not encrypted"}),data.members,crate::i18n::tr("members")));
                self.data=Some(accepted);self.pending=None;self.status(cx,if done.saved.is_some(){"Saved"}else{""});
                if done.saved.is_some() {
                    cx.action(RoomSettingsChanged(data.room.clone()));
                    cx.action(crate::app::AppStateAction::RoomNameUpdated(data.room.clone()));
                    if done.saved.as_ref().is_some_and(|s|s.field==Field::Notifications) {super::rooms_list::enqueue_rooms_list_update(super::rooms_list::RoomsListUpdate::UpdateNotificationMode {room_id:done.pin.room.clone(),mode:data.mode});}
                }
            }
            Err(e)=>{if done.definite {self.pending=None;}self.status(cx,e);}
        }
        self.sync(cx);self.view.redraw(cx);
    }
}
impl Widget for RoomSettingsPanel {
    fn handle_event(&mut self,cx:&mut Cx,event:&Event,scope:&mut Scope) {
        if self.pin.as_ref().is_none_or(|p|!p.current()) {return;}
        self.view.handle_event(cx,event,scope);
        if event.back_pressed() {cx.action(RoomSettingsAction::Close);return;}
        let Event::Actions(actions)=event else{return;};
        for action in actions {if let Some(done)=action.downcast_ref::<Finished>() {self.apply(cx,done);}}
        if self.view.button(cx,ids!(close)).clicked(actions) {cx.action(RoomSettingsAction::Close);}
        if self.view.button(cx,ids!(reload)).clicked(actions) && !self.busy && self.pending.is_none() {
            if self.dirty(cx) && !self.refresh_armed {self.refresh_armed=true;self.status(cx,"Unsaved changes. Refresh again to discard them.");}
            else {self.refresh_armed=false;self.load(cx,None);}
        }
        for (id,field) in [(ids!(save_name).as_slice(),Field::Name),(ids!(save_topic).as_slice(),Field::Topic),(ids!(save_notifications).as_slice(),Field::Notifications)] {if self.view.button(cx,id).clicked(actions) {self.start_save(cx,field);}}
        if self.view.button(cx,ids!(retry)).clicked(actions) && !self.busy && let Some(intent)=self.pending.clone() {self.load(cx,Some(intent));}
        if self.view.button(cx,ids!(copy_link)).clicked(actions) && let Some(pin)=&self.pin {crate::sliding_sync::submit_async_request(crate::sliding_sync::MatrixRequest::GenerateMatrixLink {room_id:pin.room.clone(),event_id:None,use_matrix_scheme:false,join_on_click:false});}
        if self.view.button(cx,ids!(invite)).clicked(actions) && let Some(data)=&self.data && data.can_invite {cx.action(super::invite_modal::InviteModalAction::Open(data.room.clone()));}
        if self.view.text_input(cx,ids!(name)).changed(actions).is_some() || self.view.text_input(cx,ids!(topic)).changed(actions).is_some() || self.view.drop_down(cx,ids!(notification_mode)).changed(actions).is_some() {self.close_armed=false;self.refresh_armed=false;self.sync(cx);}
    }
    fn draw_walk(&mut self,cx:&mut Cx2d,scope:&mut Scope,walk:Walk)->DrawStep {
        if self.pin.as_ref().is_some_and(|p|!p.current()) {self.pin=None;self.data=None;self.pending=None;return DrawStep::done();}
        self.view.draw_walk(cx,scope,walk)
    }
}
impl RoomSettingsPanelRef {
    pub fn action(&self,cx:&mut Cx,modal:ModalRef,action:&RoomSettingsAction) {
        let Some(mut inner)=self.borrow_mut() else{return;};
        match action {
            RoomSettingsAction::Open(room)=>{
                let Some(client)=get_client() else{return;};let Some(owner)=client.user_id().map(ToOwned::to_owned) else{return;};
                inner.pin=Some(Pin {owner,origin:client.homeserver().to_string(),epoch:crate::account_session::epoch(),room:room.room_id().clone(),request:next_request()});inner.data=None;inner.pending=None;inner.close_armed=false;inner.refresh_armed=false;
                inner.view.text_input(cx,ids!(name)).set_text(cx,"");inner.view.text_input(cx,ids!(topic)).set_text(cx,"");
                inner.load(cx,None);modal.open(cx);
            }
            RoomSettingsAction::Close=>{
                if inner.pin.as_ref().is_some_and(Pin::current) && (inner.dirty(cx) || inner.pending.is_some()) && !inner.close_armed {inner.close_armed=true;inner.status(cx,"Unsaved or unconfirmed changes. Close again to leave this page.");return;}
                inner.pin=None;inner.data=None;inner.pending=None;inner.busy=false;modal.close(cx);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_fields_and_invites_follow_independent_permissions_and_unknown_writes_stay_locked() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut panel=cx.with_vm(|vm| {makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);vm.bx.captured_errors=Some(Vec::new());let value=script_eval!(vm,{mod.widgets.RoomSettingsPanel {}});let panel=RoomSettingsPanel::script_from_value(vm,value);assert!(vm.take_errors().is_empty());panel});
        panel.data=Some(Snapshot {room:RoomNameId::new(matrix_sdk::RoomDisplayName::Named("Planning".into()),ruma::room_id!("!r:test").to_owned()),topic:"Original".into(),can_name:false,can_topic:true,can_invite:false,encrypted:true,members:2,mode:None});
        panel.view.text_input(&cx,ids!(name)).set_text(&mut cx,"Planning");panel.view.text_input(&cx,ids!(topic)).set_text(&mut cx,"New topic");panel.view.drop_down(&cx,ids!(notification_mode)).set_selected_item(&mut cx,3);panel.sync(&mut cx);
        assert!(panel.view.text_input(&cx,ids!(name)).is_read_only());assert!(!panel.view.text_input(&cx,ids!(topic)).is_read_only());
        assert!(!panel.view.button(&cx,ids!(save_name)).visible());assert!(panel.view.button(&cx,ids!(save_topic)).visible());assert!(!panel.view.button(&cx,ids!(invite)).visible());
        panel.pending=Some(Intent {field:Field::Topic,value:"New topic".into(),base:"Original".into()});panel.sync(&mut cx);
        assert!(panel.view.text_input(&cx,ids!(topic)).is_read_only());assert!(panel.view.button(&cx,ids!(retry)).visible());assert_eq!(panel.view.text_input(&cx,ids!(topic)).text(),"New topic");
        panel.data.as_mut().unwrap().can_invite=true;panel.sync(&mut cx);assert!(panel.view.button(&cx,ids!(invite)).visible());
        assert!(!panel.view.view(&cx,ids!(form)).is_empty());
    }
    #[test] fn retry_reconciles_original_value_and_refuses_concurrent_edits() {
        let intent=Intent {field:Field::Name,value:"New name".into(),base:"Original".into()};
        assert!(reconcile("Original",&intent).unwrap());assert!(!reconcile("New name",&intent).unwrap());assert!(reconcile("Another editor",&intent).is_err());
        assert!(validate(&intent).is_ok());assert!(validate(&Intent {value:"\n".into(),..intent.clone()}).is_err());
        assert!(validate(&Intent {field:Field::Topic,value:String::new(),base:String::new()}).is_ok());
        for i in 0..4 {assert_eq!(mode_index(mode(i)),i);}
    }
}
