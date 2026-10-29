//! Two-level Hagency Project → joined chat navigation. Matrix Spaces alone
//! never identify a Hagency Project; only authenticated backend snapshots do.
use std::collections::{HashMap, HashSet};
use makepad_widgets::*;
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId};
use crate::{
    app::SelectedRoom,
    logout::logout_confirm_modal::LogoutAction,
    shared::{
        navigation_bar_button::{NavigationBarButtonAction, NavigationBarButtonWidgetRefExt},
        room_filter_input_bar::MainFilterAction,
        unread_badge::UnreadBadgeWidgetRefExt,
    },
    sliding_sync::{current_user_id, get_client, spawn_async_task},
    utils::RoomNameId,
};
use super::rooms_list::{RoomsListAction, RoomsListRef};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.ICON_PROJECT_FOLDER_CLOSED = crate_resource("self://resources/icons/project_folder_closed.svg")
    mod.widgets.ICON_PROJECT_FOLDER_OPEN = crate_resource("self://resources/icons/project_folder_open.svg")

    mod.widgets.ProjectTree = #(ProjectTree::register_widget(vm)) {
        width: Fill height: Fill flow: Down
        archived_projects := RinxCheckBox {text: #(crate::i18n::tr("Archived projects")) i18n_text: "Archived projects" margin: 8}
        status := RinxLabel {
            width: Fill height: Fit padding: 18
            flow: Flow.Right{wrap: true}
            draw_text +: {color: mod.widgets.RINX_MUTED text_style: theme.font_regular {font_size: mod.widgets.RINX_BODY_SIZE}}
            text: #(crate::i18n::tr("Loading projects…")) i18n_text: "Loading projects…"
        }
        list := PortalList {
            width: Fill height: Fill flow: Down auto_tail: false
            Row := View {
                width: Fill height: 37 flow: Down
                row_surface := RoundedView {width: Fill height: 36 flow: Right align: Align{y: 0.5}
                draw_bg +: {color: #0000 border_radius: 5 border_size: 0}
                disclosure := NavigationBarButton {
                    width: 28 height: 36 flow: Overlay align: Align{x: 0.5 y: 0.5}
                    draw_bg +: {color_hover: mod.widgets.RINX_HOVER color_active: #0000 border_radius: 5}
                    closed := View {
                        width: 20 height: 20
                        Icon {icon_walk: Walk{width: 20 height: 20} draw_icon +: {svg: mod.widgets.ICON_PROJECT_FOLDER_CLOSED color: mod.widgets.RINX_ACCENT}}
                    }
                    opened := View {
                        width: 20 height: 20
                        Icon {icon_walk: Walk{width: 20 height: 20} draw_icon +: {svg: mod.widgets.ICON_PROJECT_FOLDER_OPEN color: mod.widgets.RINX_ACCENT}}
                    }
                }
                row := NavigationBarButton {
                    width: Fill height: 36 flow: Right spacing: 8
                    align: Align{y: 0.5} padding: Inset{left: 4 right: 6}
                    draw_bg +: {color_hover: mod.widgets.RINX_HOVER color_active: #0000 border_radius: 5}
                    indent := View {width: 0 height: 1}
                    board_icon := View {visible: false width: 16 height: 16 Icon {icon_walk: Walk{width: 16 height: 16} draw_icon +: {svg: ICON_SQUARES color: mod.widgets.RINX_ACCENT}}}
                    room_icon := View {visible: false width: 16 height: 16 Icon {icon_walk: Walk{width: 16 height: 16} draw_icon +: {svg: ICON_THREADS color: mod.widgets.RINX_MUTED}}}
                    name := RinxLabel {
                        width: Fill max_lines: 1 text_overflow: Ellipsis
                        draw_text +: {color: mod.widgets.RINX_INK text_style: theme.font_regular {font_size: mod.widgets.RINX_UI_SIZE}}
                    }
                    default_badge := mod.widgets.RinxDefaultBadge {visible:false label +: {i18n_text:"default"}}
                    unread := UnreadBadge {}
                }
                View {width:32 height:32 more := RinxMoreButton {visible:false}}
                }

            }
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectTreeMode {
    #[default]
    Projects,
    GroupChats,
}

/// A refresh error is a non-blocking notice alongside the retained list.
fn tree_notice(mode: ProjectTreeMode, error: Option<&str>, loading: bool, empty: bool, query: bool) -> (&'static str, bool) {
    if mode == ProjectTreeMode::GroupChats {
        if error.is_some() || loading {return ("Loading group chat classification…",true);}
        return (if !empty {""} else if query {"No matching group chats"} else {"No group chats yet. Use the Group Chats menu to create a group."}, empty);
    }
    if let Some(error) = error {
        return (if error == "owner_authorization_required" {"Sign in again to restore Projects for this account"} else {"Hagency Projects could not be loaded. Retrying…"}, true);
    }
    (if loading {"Loading Hagency Projects…"} else if !empty {""} else if query {"No matching Projects"} else {"No Projects yet. Use the Projects menu to create one."}, loading || empty)
}

/// Produced only after Hagency backend owner authentication and room discovery.
#[derive(Clone, Debug)]
pub struct TreeProject {
    pub id: String,
    pub space: OwnedRoomId,
    pub name: String,
    pub rooms: Vec<OwnedRoomId>,
    pub boards: Vec<(String,String)>,
    pub archived: bool,
    pub default_board:Option<String>,
    pub default_room:Option<OwnedRoomId>,
}
#[derive(Clone, Debug)]
pub struct ProjectTreeSnapshot {
    pub owner: OwnedUserId,
    pub homeserver: String,
    pub revision: u64,
    pub service_identity: String,
    pub projects: Vec<TreeProject>,
    pub spaces:HashMap<OwnedRoomId,Vec<OwnedRoomId>>,
}
#[derive(Clone, Debug)]
pub(crate) struct TreeRoom {
    pub name: RoomNameId,
    pub unread: u64,
    pub marked: bool,
    pub invited: bool,
    pub is_direct: bool,
    pub is_space:bool,
    pub is_favorite: bool,
}
fn active_records<'a>(
    value: &'a serde_json::Value,
    key: &str,
) -> Result<Vec<&'a serde_json::Value>, String> {
    Ok(value[key]
        .as_array()
        .ok_or("invalid_project_response")?
        .iter()
        .filter(|item| item["active"].as_bool() == Some(true))
        .collect())
}
#[derive(Clone, Debug, PartialEq)]
enum Row {
    Project(String),
    Board {project: String, board: String},
    Other,
    Direct,
    Invitations,
    Invite(OwnedRoomId),
    Room(OwnedRoomId),
    Space(OwnedRoomId),
}
fn tree_rows(
    projects: &[TreeProject],
    rooms: &[TreeRoom],
    expanded: &HashSet<String>,
    query: &str,
) -> Vec<Row> {
    let joined: HashMap<_, _> = rooms
        .iter()
        .filter(|room| !room.is_space)
        .map(|room| (room.name.room_id(), room))
        .collect();
    let mut projects: Vec<_> = projects.iter().collect();
    projects.sort_by_key(|p| (p.name.to_lowercase(), p.id.clone()));
    let mut assigned = HashSet::new();
    let mut result = Vec::new();
    let project_room_ids:HashSet<_>=projects.iter().flat_map(|p|p.rooms.iter()).collect();
    let mut invites: Vec<_> = rooms
        .iter()
        .filter(|room| room.invited && !project_room_ids.contains(room.name.room_id()) && room.name.display().to_lowercase().contains(query))
        .collect();
    invites.sort_by_key(|room| {
        (
            room.name.display().to_lowercase(),
            room.name.room_id().clone(),
        )
    });
    if !invites.is_empty() {
        result.push(Row::Invitations);
        result.extend(
            invites
                .into_iter()
                .map(|room| Row::Invite(room.name.room_id().clone())),
        );
    }
    for project in projects {
        let mut seen = HashSet::new();
        let members: Vec<_> = project
            .rooms
            .iter()
            .filter(|id| {
                joined.get(*id).is_some_and(|room| !room.is_direct) && seen.insert((*id).clone())
            })
            .collect();
        assigned.extend(members.iter().map(|id| (*id).clone()));
        let project_match = project.name.to_lowercase().contains(query);
        let visible: Vec<_> = members
            .into_iter()
            .filter(|id| project_match || joined[*id].name.display().to_lowercase().contains(query))
            .collect();
        if !query.is_empty() && !project_match && visible.is_empty() {
            continue;
        }
        result.push(Row::Project(project.id.clone()));
        if expanded.contains(&project.id) || !query.is_empty() {
            let mut visible = visible;
            visible.sort_by_key(|id| (project.default_room.as_ref()!=Some(*id),joined[*id].name.display().to_lowercase(), (*id).clone()));
            let mut boards:Vec<_>=project.boards.iter().collect();boards.sort_by_key(|(id,name)|(project.default_board.as_ref()!=Some(id),name.to_lowercase(),id.clone()));
            result.extend(boards.into_iter().map(|(board,_)|Row::Board {project:project.id.clone(),board:board.clone()}));
            result.extend(visible.into_iter().map(|id| if joined[id].invited {Row::Invite(id.clone())}else{Row::Room(id.clone())}));
        }
    }
    let direct: Vec<_> = rooms
        .iter()
        .filter(|room| {
            !room.invited && room.is_direct && room.name.display().to_lowercase().contains(query)
        })
        .collect();
    if !direct.is_empty() {
        result.push(Row::Direct);
        result.extend(
            direct
                .into_iter()
                .map(|room| Row::Room(room.name.room_id().clone())),
        );
    }
    let other: Vec<_> = rooms
        .iter()
        .filter(|room| {
            !room.invited
                && !room.is_direct
                && !assigned.contains(room.name.room_id())
                && room.name.display().to_lowercase().contains(query)
        })
        .collect();
    if !other.is_empty() {
        result.push(Row::Other);
        // Preserve the actual RoomsList recency/favourite ordering outside Projects.
        result.extend(
            other
                .into_iter()
                .map(|room| Row::Room(room.name.room_id().clone())),
        );
    }
    result
}
fn workspace_rows(
    mode: ProjectTreeMode,
    projects: &[TreeProject],
    rooms: &[TreeRoom],
    expanded: &HashSet<String>,
    query: &str,
) -> Vec<Row> {
    if mode == ProjectTreeMode::GroupChats {
        let assigned:HashSet<_>=projects.iter().flat_map(|p|p.rooms.iter()).collect();
        return rooms.iter().filter(|r|!r.is_direct && !r.is_space && !assigned.contains(r.name.room_id()) && r.name.display().to_lowercase().contains(query))
            .map(|r|if r.invited {Row::Invite(r.name.room_id().clone())} else {Row::Room(r.name.room_id().clone())}).collect();
    }
    let assigned: HashSet<_> = projects.iter().flat_map(|p| p.rooms.iter()).collect();
    tree_rows(projects, rooms, expanded, "")
        .into_iter()
        .filter(|row| matches!(row,Row::Project(_)|Row::Board {..})
            || matches!(row,Row::Invitations) && rooms.iter().any(|r|r.invited && r.is_space)
            || matches!(row,Row::Invite(id) if rooms.iter().any(|r|r.name.room_id()==id && !r.is_direct && (r.is_space || assigned.contains(id))))
            || matches!(row,Row::Room(id) if assigned.contains(id) && rooms.iter().any(|r|r.name.room_id()==id && !r.is_direct)))
        .collect()
}

#[derive(Clone,Debug,PartialEq,Eq)]
pub(crate) enum ProjectSelection {Project,Board(String),Room(OwnedRoomId)}
#[derive(Clone,Debug,PartialEq,Eq)]
pub(crate) struct ProjectSelectionChanged {pub identity:String,pub project_id:String,pub target:ProjectSelection}
impl ProjectSelectionChanged {
    fn current(&self)->bool {crate::hagency::backend::Service::for_current().is_ok_and(|s|s.identity()==self.identity)}
    fn matches(&self,row:&Row)->bool {match (&self.target,row) {
        (ProjectSelection::Project,Row::Project(p))=>p==&self.project_id,
        (ProjectSelection::Board(id),Row::Board {project,board})=>project==&self.project_id && id==board,
        (ProjectSelection::Room(id),Row::Room(room)|Row::Invite(room))=>id==room,
        _=>false,
    }}
}

/// The Group Chats graph contains only non-project, non-direct accessible rooms.
/// Space links organize navigation and never grant access to an unjoined room.
fn group_rows(projects:&[TreeProject],rooms:&[TreeRoom],spaces:&HashMap<OwnedRoomId,Vec<OwnedRoomId>>,expanded:&HashSet<String>,query:&str)->Vec<(Row,usize)> {
    let assigned:HashSet<_>=projects.iter().flat_map(|p|p.rooms.iter().chain(std::iter::once(&p.space))).collect();
    let visible:HashMap<_,_>=rooms.iter().filter(|r|!r.is_direct && (!r.is_space || !r.invited) && !assigned.contains(r.name.room_id())).map(|r|(r.name.room_id().clone(),r)).collect();
    let graph:HashMap<_,Vec<_>>=spaces.iter().filter(|(id,_)|visible.get(*id).is_some_and(|r|r.is_space && !r.invited)).map(|(id,children)|(id.clone(),children.iter().filter(|c|visible.contains_key(*c)).cloned().collect())).collect();
    let children:HashSet<_>=graph.values().flatten().cloned().collect();
    fn descendants(graph:&HashMap<OwnedRoomId,Vec<OwnedRoomId>>,id:&OwnedRoomId)->HashSet<OwnedRoomId> {let mut seen=HashSet::new();let mut todo=vec![id.clone()];while let Some(id)=todo.pop(){if seen.insert(id.clone()) && let Some(children)=graph.get(&id){todo.extend(children.iter().cloned());}}seen}
    let sort=|ids:&mut Vec<OwnedRoomId>|ids.sort_by_key(|id|(!visible[id].is_space,visible[id].name.display().to_lowercase(),id.clone()));
    let mut roots:Vec<_>=graph.keys().filter(|id|!children.contains(*id)).cloned().collect();sort(&mut roots);
    let mut covered=HashSet::new();for root in &roots {covered.extend(descendants(&graph,root));}
    let mut all:Vec<_>=graph.keys().cloned().collect();sort(&mut all);for id in all {if !covered.contains(&id){covered.extend(descendants(&graph,&id));roots.push(id);}}
    roots.extend(visible.keys().filter(|id|!graph.contains_key(*id) && !children.contains(*id)).cloned());sort(&mut roots);
    let mut result=Vec::new();
    fn append(position:(&OwnedRoomId,usize),visible:&HashMap<OwnedRoomId,&TreeRoom>,graph:&HashMap<OwnedRoomId,Vec<OwnedRoomId>>,expanded:&HashSet<String>,filter:(&str,bool),path:&mut HashSet<OwnedRoomId>,result:&mut Vec<(Row,usize)>) {
        let (id,depth)=position;let (query,inherited)=filter;
        if depth>16 || !path.insert(id.clone()){return;}
        let room=visible[id];let matched=inherited || query.is_empty() || descendants(graph,id).iter().any(|id|visible[id].name.display().to_lowercase().contains(query));
        if matched {
            result.push((if room.invited {Row::Invite(id.clone())}else if room.is_space {Row::Space(id.clone())}else{Row::Room(id.clone())},depth));
            if room.is_space && (!query.is_empty() || expanded.contains(&format!("space:{id}"))) && let Some(children)=graph.get(id) {
                let mut children=children.clone();children.sort_by_key(|id|(!visible[id].is_space,visible[id].name.display().to_lowercase(),id.clone()));
                for child in children {append((&child,depth+1),visible,graph,expanded,(query,inherited || room.name.display().to_lowercase().contains(query)),path,result);}
            }
        }
        path.remove(id);
    }
    for root in roots {append((&root,0),&visible,&graph,expanded,(query,false),&mut HashSet::new(),&mut result);}
    result
}

fn group_unread(projects:&[TreeProject],rooms:&[TreeRoom],spaces:&HashMap<OwnedRoomId,Vec<OwnedRoomId>>,root:&OwnedRoomId)->(u64,bool) {
    let blocked:HashSet<_>=projects.iter().flat_map(|p|p.rooms.iter().chain(std::iter::once(&p.space))).collect();
    let mut seen=HashSet::new();let mut todo=vec![root.clone()];let mut result=(0u64,false);
    while let Some(id)=todo.pop(){
        if blocked.contains(&id) || !seen.insert(id.clone()){continue;}
        if let Some(room)=rooms.iter().find(|r|r.name.room_id()==&id && !r.is_direct){
            if !room.is_space {result.0=result.0.saturating_add(room.unread);result.1|=room.marked || room.invited;}
            else if let Some(children)=spaces.get(&id){todo.extend(children.iter().cloned());}
        }
    }
    result
}

#[derive(Clone, Debug)]
pub struct ArchivedProjectsVisibility(pub bool);

#[derive(Clone, Debug)]
pub struct ProjectTreeChanged {
    pub identity: String,
}

#[derive(Clone, Debug)]
struct LoadedProjects {
    request: u64,
    identity: String,
    result: Result<ProjectTreeSnapshot, String>,
}

use crate::theme::Snapshot as TreeAppearance;

type RowStyles = HashMap<WidgetUid, (Vec4f, f64)>;

#[derive(Script, Widget)]
pub struct ProjectTree {
    #[deref]
    view: View,
    #[rust]
    appearance: TreeAppearance,
    #[rust]
    row_styles: RowStyles,
    #[rust] pointer:Option<Vec2d>,
    #[rust] hovered_row:Option<Row>,
    #[rust] selection:Option<ProjectSelectionChanged>,
    #[rust]
    snapshot: Option<ProjectTreeSnapshot>,
    #[rust]
    rooms: Vec<TreeRoom>,
    #[rust] unclassified_rooms:HashSet<OwnedRoomId>,
    #[rust]
    rows: Vec<Row>,
    #[rust] depths:Vec<usize>,
    #[rust]
    expanded: HashSet<String>,
    #[rust]
    query: String,
    #[rust]
    mode: ProjectTreeMode,
    #[rust]
    chat_query: String,
    #[rust] show_archived: bool,
    #[rust]
    active: bool,
    #[rust]
    timer: Timer,
    #[rust]
    pending: bool,
    #[rust] legacy_selection_checked:bool,
    #[rust]
    refresh_again: bool,
    #[rust]
    request: u64,
    #[rust]
    identity: Option<String>,
    #[rust]
    error: Option<String>,
}
impl ScriptHook for ProjectTree {
    fn on_after_apply(&mut self, vm: &mut ScriptVm, _: &Apply, _: &mut Scope, _: ScriptValue) {
        self.appearance = crate::theme::snapshot_for_vm(vm);
        self.row_styles.clear();
    }
}
impl ProjectTree {
    fn handle_row_actions(&mut self, cx: &mut Cx, captured: &Actions) {
        let mut clicked = HashSet::new();
        for (index, item) in self
            .view
            .portal_list(cx, ids!(list))
            .items_with_actions(captured)
        {
            let uid = item.navigation_bar_button(cx, ids!(row)).widget_uid();
            let secondary = item.button(cx, ids!(more)).clicked(captured)
                || item
                    .navigation_bar_button(cx, ids!(row))
                    .secondary_clicked(captured);
            if secondary {
                let row = self.rows.get(index).cloned();
                match row {
                    Some(Row::Project(project_id)) => {
                        if let Some(snapshot) = self.snapshot.as_ref().filter(|snapshot| snapshot_current(snapshot))
                            && let Some(project) = snapshot.projects.iter().find(|project| project.id == project_id) {
                            let rect = item.button(cx, ids!(more)).area().rect(cx);
                            cx.action(super::sidebar_action_menu::SidebarMenuAction::OpenProject {
                                target: super::sidebar_action_menu::ProjectMenuTarget {project_id,
                                    space_id: project.space.clone(), service_identity: snapshot.service_identity.clone(),
                                    owner_mxid: snapshot.owner.to_string(), homeserver: snapshot.homeserver.clone(), epoch: crate::account_session::epoch()},
                                pos: dvec2(rect.pos.x, rect.pos.y + rect.size.y),
                            });
                        }
                    }
                    Some(Row::Room(id)) => {
                        if get_client()
                            .and_then(|client| client.get_room(&id))
                            .is_some_and(|room| room.state() == matrix_sdk::RoomState::Joined)
                            && let Some(details) = cx
                                .global::<RoomsListRef>()
                                .clone()
                                .room_context_details(&id)
                        {
                            let rect = item.area().rect(cx);
                            cx.widget_action(
                                self.widget_uid(),
                                RoomsListAction::OpenRoomContextMenu {
                                    details,
                                    pos: dvec2(rect.pos.x + rect.size.x, rect.pos.y + rect.size.y),
                                },
                            );
                        }
                    }
                    _ => {}
                }
                continue;
            }
            if item.navigation_bar_button(cx, ids!(disclosure)).clicked(captured) {
                if clicked.insert(index) && let Some(id)=self.rows.get(index).and_then(|row|match row {Row::Project(id)=>Some(id.clone()),Row::Space(id)=>Some(format!("space:{id}")),_=>None}) {
                    if !self.expanded.remove(&id) {self.expanded.insert(id);}
                    self.rebuild(cx);
                }
                continue;
            }
            if !clicked.insert(index)
                || !captured.iter().any(|action| {
                    action.as_widget_action().is_some_and(|a| {
                        a.widget_uid == uid
                            && matches!(a.cast(), NavigationBarButtonAction::Clicked)
                    })
                })
            {
                continue;
            }
            match self.rows.get(index).cloned() {
                Some(Row::Space(id))=>{
                    if self.mode==ProjectTreeMode::GroupChats && let Some(room)=self.rooms.iter().find(|r|r.name.room_id()==&id && r.is_space && !r.invited) {
                        cx.widget_action(self.widget_uid(),RoomsListAction::Selected(SelectedRoom::Space {space_name_id:room.name.clone()}));
                    }
                }
                Some(Row::Board {project,board}) => {
                    cx.action(crate::hagency::ui::HagencyAction::ProjectBoard {project_id:project,board_id:board});
                }
                Some(Row::Project(id)) => {
                    cx.action(crate::hagency::ui::HagencyAction::ProjectDetails {project_id: id});
                }
                Some(Row::Invite(id)) => {
                    if let Some(room) = self
                        .rooms
                        .iter()
                        .find(|room| room.invited && room.name.room_id() == &id)
                        && get_client()
                            .and_then(|c| c.get_room(&id))
                            .is_some_and(|r| r.state() == matrix_sdk::RoomState::Invited)
                    {
                        cx.widget_action(
                            self.widget_uid(),
                            RoomsListAction::Selected(SelectedRoom::InvitedRoom {
                                room_name_id: room.name.clone(),
                            }),
                        );
                    }
                }
                Some(Row::Room(id)) => {
                    if let Some(room) = self.rooms.iter().find(|room| room.name.room_id() == &id)
                        && get_client().and_then(|c| c.get_room(&id)).is_some_and(|r| {
                            r.state() == matrix_sdk::RoomState::Joined && !r.is_space()
                        })
                    {
                        cx.widget_action(
                            self.widget_uid(),
                            RoomsListAction::Selected(SelectedRoom::JoinedRoom {
                                room_name_id: room.name.clone(),
                            }),
                        );
                    }
                }
                _ => {}
            }
        }
    }
    fn clear(&mut self, cx: &mut Cx) {
        self.snapshot = None;
        self.selection=None;
        self.legacy_selection_checked=false;
        self.pending = false;
        self.refresh_again = false;
        self.identity = None;
        self.error = None;
        self.rooms.clear();self.unclassified_rooms.clear();
        self.rows.clear();
        self.expanded.clear();
        self.query.clear();
        self.chat_query.clear();
        self.show_archived=false;
        self.view.check_box(cx,ids!(archived_projects)).set_active(cx,false,Animate::No);
        self.redraw(cx);
    }
    fn follow_selection(&mut self,cx:&mut Cx,selection:&ProjectSelectionChanged) {
        if self.identity.as_ref()!=Some(&selection.identity) {self.clear(cx);self.identity=Some(selection.identity.clone());}
        let expand=selection.target!=ProjectSelection::Project;
        if self.selection.as_ref()==Some(selection) && (!expand || self.expanded.contains(&selection.project_id)) {return;}
        if expand {self.expanded.insert(selection.project_id.clone());self.query.clear();}
        self.selection=Some(selection.clone());self.rebuild(cx);
    }
    // Called only after the navigation's full current-account identity is verified.
    // Initialize before the first Projects activation, so refresh won't clear
    // the selected expansion. No late load callback re-applies this intent.
    fn expand_navigation(&mut self, cx: &mut Cx, identity: &str, project_id: &str) {
        if self.identity.as_deref() != Some(identity) {
            self.clear(cx);
            self.identity = Some(identity.to_owned());
        }
        self.expanded.insert(project_id.to_owned());
        self.query.clear();
        self.rebuild(cx);
    }
    // An already-visible tree can have activated before the SDK established
    // the account. Observe readiness again without waiting for the periodic poll.
    fn refresh_ready_account(&mut self, cx: &mut Cx) {
        if !self.active { return; }
        if let Ok(service) = crate::hagency::backend::Service::for_current()
            && self.identity.as_deref() != Some(service.identity().as_str()) {
            self.refresh(cx);
        }
    }

    fn refresh(&mut self, cx: &mut Cx) {
        if !self.active {
            return;
        }
        let service = match crate::hagency::backend::Service::for_current() {
            Ok(service) => service,
            Err(_) => {
                self.snapshot = None;
                self.identity = None;
                self.pending = false;
                self.error = Some("owner_authorization_required".into());
                self.rebuild(cx);
                return;
            }
        };
        let identity = service.identity();
        if self.identity.as_ref() != Some(&identity) {
            self.clear(cx);
            self.identity = Some(identity.clone());
        }
        if self.pending {
            self.refresh_again = true;
            return;
        }
        let Some(client) = get_client() else { return };
        let Some(owner) = current_user_id() else {
            return;
        };
        let homeserver = client.homeserver().to_string();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let request = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.request = request;
        self.pending = true;
        spawn_async_task(async move {
            use crate::hagency::model::Command;
            let result = async {
                let value = service.execute(Command::Projects).await?;
                let values = active_records(&value, "projects")?;
                let mut projects = Vec::new();
                let mut seen = HashSet::new();
                for value in values {
                    let id = value["id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .ok_or("invalid_project_response")?
                        .to_owned();
                    if !seen.insert(id.clone()) {
                        return Err("invalid_project_response".into());
                    }
                    let space: OwnedRoomId = value["spaceId"]
                        .as_str()
                        .ok_or("invalid_project_response")?
                        .parse()
                        .map_err(|_| "invalid_project_response")?;
                    let name = if let Some(name) = value["name"].as_str().filter(|name| !name.trim().is_empty()) {
                        name.to_owned()
                    } else if let Some(room) = client.get_room(&space) {
                        RoomNameId::from_room(&room).await.display().into_owned()
                    } else {
                        space.as_str().to_owned()
                    };
                    let rooms = service
                        .execute(Command::ProjectRooms {
                            project_id: id.clone(),
                        })
                        .await?;
                    let default_room=active_records(&rooms,"rooms")?.into_iter().find(|r|r["isDefault"]==true).and_then(|r|r["roomId"].as_str()).and_then(|s|s.parse().ok());
                    let rooms = active_records(&rooms, "rooms")?
                        .into_iter()
                        .map(|room| {
                            room["roomId"]
                                .as_str()
                                .ok_or("invalid_project_response")?
                                .parse()
                                .map_err(|_| "invalid_project_response")
                        })
                        .collect::<Result<Vec<OwnedRoomId>, _>>()?;
                    let boards=service.execute(Command::ProjectBoard {project_id:id.clone(),action: ::hagency::project_board::BoardAction::Boards}).await?;
                    let default_board=boards["boards"].as_array().and_then(|b|b.iter().find(|b|b["isDefault"]==true)).and_then(|b|b["id"].as_str()).map(str::to_owned);
                    let boards=boards["boards"].as_array().ok_or("invalid_board_response")?.iter().map(|b|Ok((b["id"].as_str().ok_or("invalid_board_response")?.to_owned(),b["name"].as_str().ok_or("invalid_board_response")?.to_owned()))).collect::<Result<Vec<_>,String>>()?;
                    projects.push(TreeProject {
                        id,
                        space,
                        name,
                        rooms,
                        boards,
                        archived: value["archived"]==true,
                        default_board,default_room,
                    });
                }
                let mut spaces=HashMap::new();
                for room in client.joined_rooms().into_iter().filter(|r|r.is_space()) {
                    use matrix_sdk::{deserialized_responses::SyncOrStrippedState,ruma::events::{space::child::SpaceChildEventContent,SyncStateEvent}};
                    let events=room.get_state_events_static::<SpaceChildEventContent>().await.map_err(|_|"space_navigation_unavailable")?;
                    let children=events.into_iter().filter_map(|raw|match raw.deserialize(){Ok(SyncOrStrippedState::Sync(SyncStateEvent::Original(event))) if !event.content.via.is_empty()=>Some(event.state_key),_=>None}).collect();
                    spaces.insert(room.room_id().to_owned(),children);
                }
                if !crate::matrix_context::is_current(&client) {return Err("account_changed".into());}
                Ok(ProjectTreeSnapshot {
                    owner,
                    homeserver,
                    revision: request,
                    service_identity: identity.clone(),
                    projects,
                    spaces,
                })
            }
            .await;
            Cx::post_action(LoadedProjects {
                request,
                identity,
                result,
            });
        });
    }
    fn rebuild(&mut self, cx: &mut Cx) {
        self.rebuild_if_changed(cx);
        self.redraw(cx);
    }
    fn rebuild_if_changed(&mut self, cx: &mut Cx) {
        if self.snapshot.as_ref().is_some_and(|s| !snapshot_current(s)) {
            self.clear(cx);
        }
        let rooms = if cx.has_global::<RoomsListRef>() {
            {
                let list = cx.get_global::<RoomsListRef>().clone();
                list.project_tree_rooms(cx)
            }
        } else {
            Vec::new()
        };
        let topology_changed=rooms.len()!=self.rooms.len() || rooms.iter().any(|r|self.rooms.iter().find(|old|old.name.room_id()==r.name.room_id()).is_none_or(|old|old.invited!=r.invited || old.is_space!=r.is_space));
        if topology_changed {
            self.unclassified_rooms.extend(rooms.iter().filter(|r|!self.rooms.iter().any(|old|old.name.room_id()==r.name.room_id())).map(|r|r.name.room_id().clone()));
        }
        let projects:Vec<_>=self.snapshot.as_ref().map(|s|s.projects.as_slice()).unwrap_or_default().iter().filter(|p|self.mode==ProjectTreeMode::GroupChats || self.show_archived || !p.archived).cloned().collect();
        let mut rows = workspace_rows(
            self.mode,
            &projects,
            &rooms,
            &self.expanded,
            &self.query,
        );
        self.depths=vec![0;rows.len()];
        if self.mode==ProjectTreeMode::GroupChats {
            rows.clear();self.depths.clear();
            if let Some(snapshot)=&self.snapshot {
                let classified:Vec<_>=rooms.iter().filter(|r|!self.unclassified_rooms.contains(r.name.room_id())).cloned().collect();
                for (row,depth) in group_rows(&projects,&classified,&snapshot.spaces,&self.expanded,&self.query) {rows.push(row);self.depths.push(depth);}
            }
        }
        let changed = rows != self.rows || rooms.len() != self.rooms.len()
            || rooms.iter().zip(&self.rooms).any(|(a, b)| a.name.room_id() != b.name.room_id()
                || a.name.display() != b.name.display() || a.unread != b.unread || a.marked != b.marked
                || a.invited != b.invited || a.is_space!=b.is_space || a.is_direct != b.is_direct || a.is_favorite != b.is_favorite);
        self.rooms = rooms;
        self.rows = rows;
        if topology_changed {self.refresh(cx);}
        if changed { self.redraw(cx); }
    }
}
fn snapshot_current(snapshot: &ProjectTreeSnapshot) -> bool {
    Some(&snapshot.owner) == current_user_id().as_ref()
        && get_client().is_some_and(|client| client.homeserver().as_str() == snapshot.homeserver)
        && crate::hagency::backend::Service::for_current()
            .is_ok_and(|service| service.identity() == snapshot.service_identity)
}
impl Widget for ProjectTree {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event {Event::MouseMove(e)=>self.pointer=Some(e.abs),Event::MouseDown(e)=>self.pointer=Some(e.abs),Event::WindowLostFocus(_)|Event::MouseLeave(_)|Event::Scroll(_)=>self.pointer=None,_=>()}
        let items=self.view.portal_list(cx,ids!(list)).borrow().map(|list|list.items().iter().map(|(i,item)|(*i,item.widget.clone())).collect::<Vec<_>>()).unwrap_or_default();
        self.hovered_row=None;
        for (index,item) in items {
            let show=matches!(self.rows.get(index),Some(Row::Project(_)|Row::Room(_))) && self.pointer.is_some_and(|p|item.view(cx,ids!(row_surface)).area().rect(cx).contains(p));
            if show {self.hovered_row=self.rows.get(index).cloned();}
            let button=item.button(cx,ids!(more));if button.visible()!=show {button.set_visible(cx,show);self.view.redraw(cx);}
        }
        let captured = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        self.handle_row_actions(cx, &captured);
        if let Some(show)=self.view.check_box(cx,ids!(archived_projects)).changed(&captured) {
            self.show_archived=show;
            cx.action(ArchivedProjectsVisibility(show));
            self.rebuild(cx);
        }
        if let Event::Actions(actions) = event {
            for action in actions {
                if action.downcast_ref::<crate::hagency::ui::HagencyAction>().is_some() {self.legacy_selection_checked=true;}
                if let Some(ArchivedProjectsVisibility(show))=action.downcast_ref() {
                    self.show_archived = *show;
                    self.view.check_box(cx,ids!(archived_projects)).set_active(cx,*show,Animate::No);
                    self.rebuild(cx);
                }
                if matches!(action.downcast_ref::<crate::login::login_screen::LoginAction>(), Some(crate::login::login_screen::LoginAction::LoginSuccess)) {
                    self.refresh(cx);
                }
                if let Some(selection)=action.downcast_ref::<ProjectSelectionChanged>() && selection.current() {
                    self.follow_selection(cx,selection);
                }
                if let RoomsListAction::Selected(selected)=action.as_widget_action().cast() {
                    let id=selected.room_id();
                    if let Some(snapshot)=self.snapshot.as_ref().filter(|s|snapshot_current(s))
                        && let Some(project)=snapshot.projects.iter().find(|p|p.rooms.contains(id)) {
                        let selection=ProjectSelectionChanged {identity:snapshot.service_identity.clone(),project_id:project.id.clone(),target:ProjectSelection::Room(id.clone())};
                        self.follow_selection(cx,&selection);
                    }
                }
                if let Some(navigation) = action.downcast_ref::<crate::hagency::ui::ProjectRoomNavigation>()
                    && navigation.current() {
                    self.expand_navigation(cx, &navigation.identity, &navigation.project_id);
                }
                if action.downcast_ref::<super::space_management::SpaceManagementChanged>().is_some_and(|changed|Some(&changed.owner)==current_user_id().as_ref()) {self.refresh(cx);}
                if let Some(changed) = action.downcast_ref::<ProjectTreeChanged>()
                    && self.identity.as_ref() == Some(&changed.identity)
                {
                    self.refresh(cx);
                }
                if let Some(loaded) = action.downcast_ref::<LoadedProjects>()
                    && loaded.request == self.request
                    && self.identity.as_ref() == Some(&loaded.identity)
                    && crate::hagency::backend::Service::for_current()
                        .is_ok_and(|service| service.identity() == loaded.identity)
                {
                    self.pending = false;
                    match &loaded.result {
                        Ok(snapshot) => {
                            if !self.legacy_selection_checked {
                                self.legacy_selection_checked=true;
                                if self.mode==ProjectTreeMode::Projects && let Some(crate::app::AppState {selected_room:Some(SelectedRoom::Space {space_name_id}),..})=scope.data.get::<crate::app::AppState>() && let Some(project)=snapshot.projects.iter().find(|p|&p.space==space_name_id.room_id()) {
                                    cx.action(crate::hagency::ui::HagencyAction::ProjectDetails {project_id:project.id.clone()});
                                }
                            }
                            if !self.refresh_again {self.unclassified_rooms.clear();}
                            self.snapshot = Some(snapshot.clone());
                            if self.selection.is_none() && self.mode==ProjectTreeMode::Projects
                                && let Some(room)=scope.data.get::<crate::app::AppState>().and_then(|s|s.selected_room.as_ref())
                                && let Some(project)=snapshot.projects.iter().find(|p|p.rooms.contains(room.room_id())) {
                                self.expanded.insert(project.id.clone());
                                self.selection=Some(ProjectSelectionChanged {identity:snapshot.service_identity.clone(),project_id:project.id.clone(),target:ProjectSelection::Room(room.room_id().clone())});
                            }
                            self.error = None;
                        }
                        Err(code) => {
                            self.snapshot = None;
                            self.error = Some(
                                if matches!(
                                    code.as_str(),
                                    "owner_authorization_required"
                                        | "matrix_oauth_sign_in_required"
                                        | "matrix_authorization_required"
                                        | "sign_in_required"
                                ) {
                                    "owner_authorization_required".into()
                                } else {
                                    "project_service_unavailable".into()
                                },
                            );
                        }
                    }
                    self.rebuild(cx);
                    if self.refresh_again {
                        self.refresh_again = false;
                        self.refresh(cx);
                    }
                }
                if let Some(LogoutAction::ClearAppState { .. }) = action.downcast_ref() {
                    self.clear(cx);
                }
                if let Some(MainFilterAction::Changed(query)) = action.downcast_ref()
                    && self.mode == ProjectTreeMode::GroupChats {
                    self.query = query.trim().to_lowercase();
                    self.rebuild(cx);
                }
                if action
                    .downcast_ref::<super::chat_actions::ChatVisibilityChanged>()
                    .is_some()
                {
                    self.rebuild(cx);
                }
            }
        }
        if self.active && (self.timer.is_event(event).is_some() || matches!(event, Event::Signal)) {
            self.rebuild_if_changed(cx);
            self.refresh_ready_account(cx);
            if self.timer.is_event(event).is_some() {
                self.refresh(cx);
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.snapshot.as_ref().is_some_and(|s| !snapshot_current(s)) {
            self.clear(cx);
        }
        self.view.check_box(cx,ids!(archived_projects)).set_visible(cx,self.mode==ProjectTreeMode::Projects);
        let (notice, visible) = tree_notice(self.mode, self.error.as_deref(), self.snapshot.is_none(), self.rows.is_empty(), !self.query.is_empty());
        self.view.label(cx, ids!(status)).set_text(cx, crate::i18n::tr(notice));
        self.view.label(cx, ids!(status)).set_visible(cx, visible);
        let selected = scope
            .data
            .get_mut::<crate::app::AppState>()
            .and_then(|state| {
                state
                    .selected_room
                    .as_ref()
                    .map(|room| room.room_id().clone())
            });
        let selection_color = self.appearance.selected;
        while let Some(widget) = self.view.draw_walk(cx, scope, walk).step() {
            let Some(mut list) = widget.borrow_mut::<PortalList>() else {
                continue;
            };
            if list.first_id() >= self.rows.len() {
                list.set_first_id_and_scroll(0, 0.0);
            }
            list.set_item_range(cx, 0, self.rows.len());
            while let Some(index) = list.next_visible_item(cx) {
                let Some(row) = self.rows.get(index) else {
                    continue;
                };
                let item = list.item(cx, index, id!(Row));
                if let Some(mut view)=item.borrow_mut::<View>() {
                    let project_row=self.mode==ProjectTreeMode::Projects && matches!(row,Row::Project(_));
                    view.walk.height=Size::Fixed(if project_row {43.0}else{37.0});
                    view.layout.padding.top=if project_row {6.0}else{0.0};
                }
                let row_selected=if self.mode==ProjectTreeMode::Projects && let Some(selection)=self.selection.as_ref().filter(|s|self.identity.as_ref()==Some(&s.identity)) {
                    selection.matches(row)
                }else {matches!(row,Row::Room(id)|Row::Invite(id)|Row::Space(id) if selected.as_ref()==Some(id))};
                item.navigation_bar_button(cx, ids!(row)).set_selected(cx, row_selected);
                let background = if row_selected {selection_color} else {Vec4f::default()};
                let project = if let Row::Project(id) = row {
                    self.snapshot
                        .as_ref()
                        .and_then(|s| s.projects.iter().find(|p| &p.id == id))
                } else {
                    None
                };
                let room = if let Row::Room(id) | Row::Invite(id) | Row::Space(id) = row {
                    self.rooms.iter().find(|r| r.name.room_id() == id)
                } else {
                    None
                };
                let board=if let Row::Board {project,board}=row {self.snapshot.as_ref().and_then(|s|s.projects.iter().find(|p|p.id==*project)).and_then(|p|p.boards.iter().find(|(id,_)|id==board))} else {None};
                let is_default=match row {
                    Row::Board {project,board}=>self.snapshot.as_ref().and_then(|s|s.projects.iter().find(|p|p.id==*project)).is_some_and(|p|p.default_board.as_ref()==Some(board)),
                    Row::Room(id)|Row::Invite(id)=>self.snapshot.as_ref().is_some_and(|s|s.projects.iter().any(|p|p.default_room.as_ref()==Some(id))),
                    _=>false,
                };
                item.view(cx,ids!(default_badge)).set_visible(cx,is_default);
                item.view(cx,ids!(board_icon)).set_visible(cx,board.is_some());
                item.view(cx,ids!(room_icon)).set_visible(cx,room.is_some_and(|r|!r.is_space));
                let text = project
                    .map(|p| if p.archived {format!("{} · {}",p.name,crate::i18n::tr("Archived"))} else {p.name.clone()})
                    .or_else(||board.map(|(_,name)|if name=="Main Board" {crate::i18n::tr(name).into()} else {name.clone()}))
                    .or_else(|| room.map(|r| if r.invited {format!("{} · {}",r.name.display(),crate::i18n::tr("Invited"))} else if r.is_favorite {
                        format!("★ {}", r.name.display())
                    } else { r.name.display().into_owned() }))
                    .unwrap_or_else(|| {
                        if matches!(row, Row::Invitations) {
                            crate::i18n::tr("Invitations").to_owned()
                        } else if matches!(row, Row::Direct) {
                            crate::i18n::tr("Direct messages").to_owned()
                        } else {
                            crate::i18n::tr("Other rooms").to_owned()
                        }
                    });
                let space=if let Row::Space(id)=row {Some(id)}else{None};
                let open = project.is_some_and(|p|self.expanded.contains(&p.id) || !self.query.is_empty()) || space.is_some_and(|id|self.expanded.contains(&format!("space:{id}")) || !self.query.is_empty());
                let mut indent = item.view(cx, ids!(indent));
                let indentation=if self.mode==ProjectTreeMode::GroupChats {self.depths.get(index).copied().unwrap_or(0) as f64*18.0}else if room.is_some() || board.is_some(){18.0}else{0.0};
                indent.set_visible(cx,indentation>0.0);
                let style = (background,indentation);
                if self.row_styles.get(&item.widget_uid()) != Some(&style) {
                    self.row_styles.insert(item.widget_uid(), style);
                    let mut surface = item.view(cx, ids!(row_surface));
                    script_apply_eval!(cx, surface, {draw_bg.color: #(background)});
                    script_apply_eval!(cx, indent, {width: #(indentation)});
                }
                item.label(cx, ids!(name)).set_text(cx, &text);
                let show_more=(project.is_some() || matches!(row,Row::Room(_))) && self.hovered_row.as_ref()==Some(row);
                item.button(cx,ids!(more)).set_visible(cx,show_more);
                item.view(cx, ids!(closed))
                    .set_visible(cx, ((project.is_some() || space.is_some()) && !open) || room.is_some_and(|r|r.is_space && r.invited));
                item.view(cx, ids!(opened))
                    .set_visible(cx, (project.is_some() || space.is_some()) && open);
                item.navigation_bar_button(cx, ids!(disclosure))
                    .set_visible(cx, project.is_some() || space.is_some());
                let (unread, marked) = if let Some(space)=space {
                    self.snapshot.as_ref().map(|snapshot|group_unread(&snapshot.projects,&self.rooms,&snapshot.spaces,space)).unwrap_or_default()
                }else if let Some(p) = project {
                    self.rooms
                        .iter()
                        .filter(|r| {
                            !r.invited && !r.is_direct && p.rooms.contains(r.name.room_id())
                        })
                        .fold((0u64, false), |(n, m), r| {
                            (n.saturating_add(r.unread), m || r.marked)
                        })
                } else {
                    room.map(|r| (r.unread, r.marked)).unwrap_or_default()
                };
                item.unread_badge(cx, ids!(unread))
                    .update_counts(marked, 0, unread);
                item.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }
}
impl ProjectTreeRef {
    pub fn set_mode(&self, cx: &mut Cx, mode: ProjectTreeMode) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.mode == mode {return;}
            if inner.mode == ProjectTreeMode::GroupChats {inner.chat_query = inner.query.clone();}
            inner.mode = mode;
            inner.query = if mode == ProjectTreeMode::GroupChats {inner.chat_query.clone()} else {String::new()};
            inner.rebuild(cx);
        }
    }
    pub fn set_active(&self, cx: &mut Cx, active: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.active == active {
                inner.refresh_ready_account(cx);
                return;
            }
            inner.active = active;
            cx.stop_timer(inner.timer);
            if active {
                inner.timer = cx.start_interval(30.0);
                inner.rebuild(cx);
                inner.refresh(cx);
            }
        }
    }
    pub fn set_projects(&self, cx: &mut Cx, snapshot: ProjectTreeSnapshot) {
        if !snapshot_current(&snapshot) {
            return;
        }
        if let Some(mut inner) = self.borrow_mut() {
            if inner
                .snapshot
                .as_ref()
                .is_some_and(|old| snapshot_current(old) && old.revision > snapshot.revision)
            {
                return;
            }
            let ids: HashSet<_> = snapshot.projects.iter().map(|p| p.id.clone()).chain(snapshot.spaces.keys().map(|id|format!("space:{id}"))).collect();
            inner.expanded.retain(|id| ids.contains(id));
            inner.snapshot = Some(snapshot);
            inner.rebuild(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(value: &str) -> OwnedRoomId {
        format!("!{value}:example.test").parse().unwrap()
    }
    fn room(value: &str) -> TreeRoom {
        TreeRoom {
            name: RoomNameId::new(matrix_sdk::RoomDisplayName::Named(value.into()), id(value)),
            unread: 0,
            marked: false,
            invited: false,
            is_direct: false,
            is_space:false,
            is_favorite: false,
        }
    }
    fn project(value: &str, rooms: &[&str]) -> TreeProject {
        TreeProject {
            boards: Vec::new(), archived: false, default_board:None,default_room:None,
            id: value.into(),
            space: id(&format!("space-{value}")),
            name: value.into(),
            rooms: rooms.iter().map(|r| id(r)).collect(),
        }
    }
    #[test]
    fn default_children_are_first_and_navigation_selects_exactly_one_matching_row() {
        let mut p=project("Work",&["Aroom","Zroom"]);p.boards=vec![("a".into(),"A board".into()),("z".into(),"Z board".into())];p.default_board=Some("z".into());p.default_room=Some(id("Zroom"));
        let rows=tree_rows(&[p],&[room("Aroom"),room("Zroom")],&HashSet::from(["Work".into()]),"");
        assert_eq!(rows[1],Row::Board {project:"Work".into(),board:"z".into()});assert_eq!(rows[3],Row::Room(id("Zroom")));
        for target in [ProjectSelection::Project,ProjectSelection::Board("z".into()),ProjectSelection::Room(id("Zroom"))] {
            let selection=ProjectSelectionChanged {identity:"account".into(),project_id:"Work".into(),target};assert_eq!(rows.iter().filter(|r|selection.matches(r)).count(),1);
            assert!(!selection.matches(&Row::Board {project:"Other".into(),board:"z".into()}));
        }
    }
    #[test]
    fn following_actual_navigation_expands_its_project_and_clears_other_account_selection() {
        let mut cx=Cx::new(Box::new(|_,_|{}));let mut tree=cx.with_vm(|vm|{makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);let value=script_eval!(vm,{mod.widgets.ProjectTree {}});ProjectTree::script_from_value(vm,value)});
        tree.query="old filter".into();let mut selection=ProjectSelectionChanged {identity:"owner-a".into(),project_id:"Work".into(),target:ProjectSelection::Board("default".into())};
        tree.follow_selection(&mut cx,&selection);assert!(tree.expanded.contains("Work"));assert!(tree.query.is_empty());assert!(tree.selection.as_ref().unwrap().matches(&Row::Board {project:"Work".into(),board:"default".into()}));
        selection.target=ProjectSelection::Project;selection.project_id="Another".into();tree.follow_selection(&mut cx,&selection);assert!(tree.selection.as_ref().unwrap().matches(&Row::Project("Another".into())));
        selection.identity="owner-b".into();selection.project_id="New".into();selection.target=ProjectSelection::Room(id("Newroom"));tree.follow_selection(&mut cx,&selection);assert!(!tree.expanded.contains("Work"));assert_eq!(tree.identity.as_deref(),Some("owner-b"));
    }
    #[test]
    fn refresh_error_stays_visible_beside_nonempty_cached_projects() {
        assert_eq!(tree_notice(ProjectTreeMode::Projects,Some("temporary"),false,false,false),("Hagency Projects could not be loaded. Retrying…",true));
        assert_eq!(tree_notice(ProjectTreeMode::Projects,Some("owner_authorization_required"),false,false,false),("Sign in again to restore Projects for this account",true));
        assert_eq!(tree_notice(ProjectTreeMode::Projects,None,false,false,false),("",false));
        assert_eq!(tree_notice(ProjectTreeMode::GroupChats,Some("temporary"),false,false,false),("Loading group chat classification…",true),"Group classification fails closed until Project discovery recovers");
    }
    #[test]
    fn first_project_navigation_initializes_identity_without_late_expansion_override() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut tree = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.ProjectTree {}});
            ProjectTree::script_from_value(vm, value)
        });
        assert!(tree.identity.is_none());
        tree.expand_navigation(&mut cx, "account-a", "project-a");
        assert_eq!(tree.identity.as_deref(), Some("account-a"));
        assert!(tree.expanded.contains("project-a"));
        tree.expanded.remove("project-a");
        tree.rebuild(&mut cx);
        assert!(!tree.expanded.contains("project-a"), "user collapse isn't re-applied by a later rebuild");
        tree.expanded.insert("previous-account-project".into());
        tree.expand_navigation(&mut cx, "account-b", "project-b");
        assert_eq!(tree.identity.as_deref(), Some("account-b"));
        assert_eq!(tree.expanded, HashSet::from(["project-b".to_owned()]));
        tree.clear(&mut cx);
        assert!(tree.identity.is_none());
        assert!(tree.expanded.is_empty());
    }
    #[test]
    fn login_success_retries_an_already_active_tree_before_periodic_poll() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut tree = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.ProjectTree {}});
            ProjectTree::script_from_value(vm, value)
        });
        // The home tree already activated while account setup was incomplete.
        tree.active = true;
        tree.pending = true;
        tree.identity = Some("previous-account".into());
        tree.error = None;
        let actions = vec![Box::new(crate::login::login_screen::LoginAction::LoginSuccess) as Action];
        tree.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::empty());
        assert!(!tree.pending, "the previous request cannot block login refresh");
        assert!(tree.identity.is_none());
        assert_eq!(tree.error.as_deref(), Some("owner_authorization_required"), "without a ready client, refresh must expose the real readiness failure");
    }

    #[test]
    fn initial_projects_display_without_joined_rooms_or_creation_event() {
        let projects = vec![project("Existing A", &[]), project("Existing B", &["NotSyncedYet"])];
        assert_eq!(workspace_rows(ProjectTreeMode::Projects, &projects, &[], &HashSet::new(), ""),
            vec![Row::Project("Existing A".into()), Row::Project("Existing B".into())]);
    }

    #[test]
    fn registered_tree_row_keeps_menu_and_primary_navigation_as_distinct_actions() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let tree = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.ProjectTree {}});
            let tree = ProjectTree::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty());
            tree
        });
        let list = tree.view.portal_list(&cx, ids!(list));
        let row = list.item(&mut cx, 0, id!(Row));
        let more = row.button(&cx, ids!(more));
        let navigation = row.navigation_bar_button(&cx, ids!(row));
        assert!(more.borrow().is_some());
        assert!(!more.visible(),"Overflow starts hidden before hovering the row");
        more.set_visible(&mut cx,true);
        assert_ne!(more.widget_uid(), navigation.widget_uid());
        let actions = cx.capture_actions(|cx| {
            cx.group_widget_actions(list.widget_uid(), row.widget_uid(), |cx| {
                cx.widget_action(more.widget_uid(), ButtonAction::Clicked(KeyModifiers::default()));
            });
        });
        assert_eq!(list.items_with_actions(&actions).len(), 1);
        assert!(more.clicked(&actions));
        assert!(!navigation.clicked(&actions));

        // Draw the actual PortalList template: overflow must fit the compact
        // project/chat row, independently of font baseline or label padding.
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, dvec2(240.0, 100.0));
        let mut draw_list = DrawList2d::new(&mut cx);
        let event = DrawEvent::default();
        let mut draw = makepad_widgets::makepad_draw::cx_draw::CxDraw::new(&mut cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        draw_list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(240.0, 100.0), Layout::flow_overlay());
        row.draw_walk_all(&mut cx2d, &mut Scope::empty(), Walk {width: Size::Fixed(240.0), height: Size::Fixed(37.0), ..Walk::default()});
        cx2d.end_pass_sized_turtle();
        draw_list.end(&mut cx2d);
        cx2d.end_pass(&pass);
        drop(cx2d);
        drop(draw);
        let surface = row.view(&cx, ids!(row_surface)).area().rect(&cx);
        let rect = more.area().rect(&cx);
        assert_eq!(rect.size, dvec2(32.0, 32.0));
        assert_eq!(rect.pos.y + rect.size.y * 0.5, surface.pos.y + surface.size.y * 0.5);
        assert!(rect.pos.x + rect.size.x <= surface.pos.x + surface.size.x);
        assert!(more.text().is_empty());
        assert!(more.borrow().unwrap().draw_icon.svg.is_some());
    }
    #[test]
    fn project_disclosure_and_details_clicks_have_independent_effects() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut tree = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.ProjectTree {}});
            ProjectTree::script_from_value(vm, value)
        });
        let list = tree.view.portal_list(&cx, ids!(list));
        let row = list.item(&mut cx, 0, id!(Row));
        let disclosure = row.navigation_bar_button(&cx, ids!(disclosure));
        let navigation = row.navigation_bar_button(&cx, ids!(row));
        assert!(disclosure.borrow().is_some());
        assert_ne!(disclosure.widget_uid(), navigation.widget_uid());
        assert!(navigation.view(&cx, ids!(disclosure)).is_empty(), "disclosure must be a sibling, outside the primary button hit area");
        for (uid, expected_open, expected_details) in [
            (disclosure.widget_uid(), true, false),
            (navigation.widget_uid(), true, true),
            (disclosure.widget_uid(), false, false),
            (navigation.widget_uid(), false, true),
        ] {
            tree.rows = vec![Row::Project("Work".into())];
            let actions = cx.capture_actions(|cx| {
                cx.group_widget_actions(list.widget_uid(), row.widget_uid(), |cx| {
                    cx.widget_action(uid, NavigationBarButtonAction::Clicked);
                });
            });
            assert_eq!(list.items_with_actions(&actions).len(), 1);
            let output = cx.capture_actions(|cx| tree.handle_row_actions(cx, &actions));
            assert_eq!(tree.expanded.contains("Work"), expected_open);
            assert_eq!(output.iter().any(|action| matches!(
                action.downcast_ref::<crate::hagency::ui::HagencyAction>(),
                Some(crate::hagency::ui::HagencyAction::ProjectDetails {project_id}) if project_id == "Work"
            )), expected_details);
        }
    }
    #[test]
    fn workspace_modes_keep_project_navigation_separate_from_all_chats() {
        let mut dm = room("Alice");
        dm.is_direct = true;
        let mut invite = room("Invite");
        invite.invited = true;
        let rooms = vec![room("Design"), room("General"), dm, invite];
        let projects = vec![project("Work", &["Design", "Alice", "Secret"])];
        let expanded = HashSet::from(["Work".into()]);
        assert_eq!(workspace_rows(ProjectTreeMode::Projects, &projects, &rooms, &expanded, "alice"), vec![Row::Project("Work".into()), Row::Room(id("Design"))]);
        let chats = workspace_rows(ProjectTreeMode::GroupChats, &projects, &rooms, &expanded, "");
        assert!(!chats.iter().any(|row| matches!(row, Row::Project(_))));
        assert_eq!(chats,vec![Row::Room(id("General")),Row::Invite(id("Invite"))]);
        assert!(chats.contains(&Row::Invite(id("Invite"))));
        assert!(!chats.contains(&Row::Room(id("Secret"))));
        assert_eq!(workspace_rows(ProjectTreeMode::GroupChats, &projects, &rooms, &expanded, "alice"), Vec::<Row>::new());
    }
    #[test]
    fn only_registered_projects_form_two_level_tree_and_unjoined_rooms_never_appear() {
        let projects = vec![project("Work", &["Design", "Secret", "Design"])];
        let rooms = vec![room("General"), room("Design")];
        let closed = tree_rows(&projects, &rooms, &HashSet::new(), "");
        assert_eq!(
            closed,
            vec![
                Row::Project("Work".into()),
                Row::Other,
                Row::Room(id("General"))
            ]
        );
        let open = tree_rows(&projects, &rooms, &HashSet::from(["Work".into()]), "");
        assert_eq!(
            open,
            vec![
                Row::Project("Work".into()),
                Row::Room(id("Design")),
                Row::Other,
                Row::Room(id("General"))
            ]
        );
        assert!(!open.contains(&Row::Room(id("Secret"))));
        // Matrix membership without a registered Project never creates a header.
        assert_eq!(
            tree_rows(&[], &rooms, &HashSet::new(), ""),
            vec![
                Row::Other,
                Row::Room(id("General")),
                Row::Room(id("Design"))
            ]
        );
    }
    #[test]
    fn search_expands_matching_chats_without_mutating_user_collapse_state() {
        let projects = vec![project("Work", &["Design", "General"])];
        let rooms = vec![room("General"), room("Design"), room("Other")];
        let expanded = HashSet::new();
        assert_eq!(
            tree_rows(&projects, &rooms, &expanded, "design"),
            vec![Row::Project("Work".into()), Row::Room(id("Design"))]
        );
        assert_eq!(
            tree_rows(&projects, &rooms, &expanded, "work"),
            vec![
                Row::Project("Work".into()),
                Row::Room(id("Design")),
                Row::Room(id("General"))
            ]
        );
        assert!(expanded.is_empty());
        assert_eq!(
            tree_rows(&projects, &rooms, &expanded, "other"),
            vec![Row::Other, Row::Room(id("Other"))]
        );
    }
    #[test]
    fn direct_messages_remain_accessible_outside_project_mappings_and_search() {
        let mut dm = room("Alice");
        dm.is_direct = true;
        let mut invited = room("Bob");
        invited.is_direct = true;
        invited.invited = true;
        let rooms = vec![dm.clone(), room("General"), invited];
        let projects = vec![project("Work", &["Alice", "General"])];
        let expanded = HashSet::from(["Work".into()]);
        assert_eq!(
            tree_rows(&projects, &rooms, &expanded, ""),
            vec![
                Row::Invitations,
                Row::Invite(id("Bob")),
                Row::Project("Work".into()),
                Row::Room(id("General")),
                Row::Direct,
                Row::Room(id("Alice")),
            ]
        );
        assert_eq!(
            tree_rows(&projects, &rooms, &expanded, "alice"),
            vec![Row::Direct, Row::Room(id("Alice"))]
        );
        // A change in Matrix's direct-room classification moves the row back
        // into its registered Project without losing access or duplicating it.
        dm.is_direct = false;
        assert_eq!(
            tree_rows(&projects, &[dm], &expanded, ""),
            vec![Row::Project("Work".into()), Row::Room(id("Alice"))]
        );
    }
    #[test]
    fn inactive_or_missing_active_backend_records_are_never_projects_or_rooms() {
        let value = serde_json::json!({"projects":[{"id":"active","active":true},{"id":"paused","active":false},{"id":"unspecified"}],"rooms":[{"roomId":"joined","active":true},{"roomId":"revoked","active":false}]});
        assert_eq!(
            active_records(&value, "projects")
                .unwrap()
                .iter()
                .map(|p| p["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["active"]
        );
        assert_eq!(active_records(&value, "rooms").unwrap().len(), 1);
        assert!(active_records(&serde_json::json!({"rooms":null}), "rooms").is_err());
    }
    #[test]
    fn project_room_invitation_remains_under_its_project_until_matrix_acceptance() {
        let projects = vec![project("Work", &["Private"])];
        let mut invited = room("Private");
        invited.invited = true;
        let expanded = HashSet::from(["Work".into()]);
        assert_eq!(
            tree_rows(&projects, &[invited.clone()], &expanded, ""),
            vec![Row::Project("Work".into()),Row::Invite(id("Private"))]
        );
        invited.invited = false;
        assert_eq!(
            tree_rows(&projects, &[invited], &expanded, ""),
            vec![Row::Project("Work".into()), Row::Room(id("Private"))]
        );
    }
    #[test]
    fn space_and_project_room_invites_never_leak_into_group_chats_or_disappear() {
        let projects=vec![project("Work",&["Private"])];let expanded=HashSet::from(["Work".into()]);
        let mut space=room("New Project");space.invited=true;space.is_space=true;
        let mut private=room("Private");private.invited=true;
        let mut other=room("Social");other.invited=true;
        let rooms=vec![space,private,other];
        assert_eq!(workspace_rows(ProjectTreeMode::Projects,&projects,&rooms,&expanded,""),vec![Row::Invitations,Row::Invite(id("New Project")),Row::Project("Work".into()),Row::Invite(id("Private"))]);
        assert_eq!(workspace_rows(ProjectTreeMode::GroupChats,&projects,&rooms,&expanded,""),vec![Row::Invite(id("Social"))]);
    }
    #[test]
    fn removing_room_mapping_returns_chat_to_other_and_project_identity_not_space_name() {
        let rooms = vec![room("General")];
        let expanded = HashSet::from(["p1".into()]);
        let mut p = project("p1", &["General"]);
        p.name = "Work".into();
        assert_eq!(
            tree_rows(&[p.clone()], &rooms, &expanded, ""),
            vec![Row::Project("p1".into()), Row::Room(id("General"))]
        );
        p.rooms.clear();
        assert_eq!(
            tree_rows(&[p], &rooms, &expanded, ""),
            vec![
                Row::Project("p1".into()),
                Row::Other,
                Row::Room(id("General"))
            ]
        );
    }
    #[test]
    fn group_spaces_expand_search_and_exclude_projects_directs_and_unknown_rooms() {
        let mut social=room("Club");social.is_space=true;
        let mut nested=room("Side");nested.is_space=true;
        let mut dm=room("Private");dm.is_direct=true;
        let mut project_space=room("space-Work");project_space.is_space=true;
        let rooms=vec![social,nested,dm,project_space,room("General"),room("ProjectRoom"),room("Standalone")];
        let projects=vec![project("Work",&["ProjectRoom"])];
        let spaces=HashMap::from([(id("Club"),vec![id("Side"),id("General"),id("Private"),id("ProjectRoom"),id("Unknown")]),(id("Side"),vec![]),(id("space-Work"),vec![id("ProjectRoom")])]);
        assert_eq!(group_rows(&projects,&rooms,&spaces,&HashSet::new(),""),vec![(Row::Space(id("Club")),0),(Row::Room(id("Standalone")),0)]);
        let expanded=HashSet::from([format!("space:{}",id("Club"))]);
        assert_eq!(group_rows(&projects,&rooms,&spaces,&expanded,""),vec![(Row::Space(id("Club")),0),(Row::Space(id("Side")),1),(Row::Room(id("General")),1),(Row::Room(id("Standalone")),0)]);
        assert_eq!(group_rows(&projects,&rooms,&spaces,&HashSet::new(),"general"),vec![(Row::Space(id("Club")),0),(Row::Room(id("General")),1)]);
        assert_eq!(group_rows(&projects,&rooms,&spaces,&HashSet::new(),"club"),vec![(Row::Space(id("Club")),0),(Row::Space(id("Side")),1),(Row::Room(id("General")),1)]);
    }
    #[test]
    fn group_space_cycles_have_a_reachable_bounded_tree() {
        let mut a=room("A");a.is_space=true;let mut b=room("B");b.is_space=true;
        let spaces=HashMap::from([(id("A"),vec![id("B")]),(id("B"),vec![id("A")])]);
        let expanded=HashSet::from([format!("space:{}",id("A")),format!("space:{}",id("B"))]);
        assert_eq!(group_rows(&[],&[a,b],&spaces,&expanded,""),vec![(Row::Space(id("A")),0),(Row::Space(id("B")),1)]);
    }

}
