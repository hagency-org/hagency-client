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
                    name := RinxLabel {
                        width: Fill max_lines: 1 text_overflow: Ellipsis
                        draw_text +: {color: mod.widgets.RINX_INK text_style: theme.font_regular {font_size: mod.widgets.RINX_UI_SIZE}}
                    }
                    unread := UnreadBadge {}
                }
                more := RinxMoreButton {}
                }

            }
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectTreeMode {
    #[default]
    Projects,
    Chats,
}

/// A refresh error is a non-blocking notice alongside the retained list.
fn tree_notice(mode: ProjectTreeMode, error: Option<&str>, loading: bool, empty: bool, query: bool) -> (&'static str, bool) {
    if mode == ProjectTreeMode::Chats {
        return (if !empty {""} else if query {"No matching chats"} else {"No chats yet. Use the Chats menu to start a conversation."}, empty);
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
}
#[derive(Clone, Debug)]
pub struct ProjectTreeSnapshot {
    pub owner: OwnedUserId,
    pub homeserver: String,
    pub revision: u64,
    pub service_identity: String,
    pub projects: Vec<TreeProject>,
}
#[derive(Clone, Debug)]
pub(crate) struct TreeRoom {
    pub name: RoomNameId,
    pub unread: u64,
    pub marked: bool,
    pub invited: bool,
    pub is_direct: bool,
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
    Other,
    Direct,
    Invitations,
    Invite(OwnedRoomId),
    Room(OwnedRoomId),
}
fn tree_rows(
    projects: &[TreeProject],
    rooms: &[TreeRoom],
    expanded: &HashSet<String>,
    query: &str,
) -> Vec<Row> {
    let joined: HashMap<_, _> = rooms
        .iter()
        .filter(|room| !room.invited)
        .map(|room| (room.name.room_id(), room))
        .collect();
    let mut projects: Vec<_> = projects.iter().collect();
    projects.sort_by_key(|p| (p.name.to_lowercase(), p.id.clone()));
    let mut assigned = HashSet::new();
    let mut result = Vec::new();
    let mut invites: Vec<_> = rooms
        .iter()
        .filter(|room| room.invited && room.name.display().to_lowercase().contains(query))
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
            visible.sort_by_key(|id| (joined[*id].name.display().to_lowercase(), (*id).clone()));
            result.extend(visible.into_iter().map(|id| Row::Room(id.clone())));
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
    if mode == ProjectTreeMode::Chats {
        return tree_rows(&[], rooms, expanded, query);
    }
    let assigned: HashSet<_> = projects.iter().flat_map(|p| p.rooms.iter()).collect();
    tree_rows(projects, rooms, expanded, "")
        .into_iter()
        .filter(|row| matches!(row, Row::Project(_)) || matches!(row, Row::Room(id) if assigned.contains(id) && rooms.iter().any(|r| r.name.room_id() == id && !r.is_direct)))
        .collect()
}

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

type RowStyles = HashMap<WidgetUid, (Vec4f, bool)>;

#[derive(Script, Widget)]
pub struct ProjectTree {
    #[deref]
    view: View,
    #[rust]
    appearance: TreeAppearance,
    #[rust]
    row_styles: RowStyles,
    #[rust]
    snapshot: Option<ProjectTreeSnapshot>,
    #[rust]
    rooms: Vec<TreeRoom>,
    #[rust]
    rows: Vec<Row>,
    #[rust]
    expanded: HashSet<String>,
    #[rust]
    query: String,
    #[rust]
    mode: ProjectTreeMode,
    #[rust]
    chat_query: String,
    #[rust]
    active: bool,
    #[rust]
    timer: Timer,
    #[rust]
    pending: bool,
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
                if clicked.insert(index) && let Some(Row::Project(id)) = self.rows.get(index).cloned() {
                    if !self.expanded.remove(&id) {
                        self.expanded.insert(id);
                    }
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
        self.pending = false;
        self.refresh_again = false;
        self.identity = None;
        self.error = None;
        self.rooms.clear();
        self.rows.clear();
        self.expanded.clear();
        self.query.clear();
        self.chat_query.clear();
        self.redraw(cx);
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
                    projects.push(TreeProject {
                        id,
                        space,
                        name,
                        rooms,
                    });
                }
                Ok(ProjectTreeSnapshot {
                    owner,
                    homeserver,
                    revision: request,
                    service_identity: identity.clone(),
                    projects,
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
        let rows = workspace_rows(
            self.mode,
            self.snapshot
                .as_ref()
                .map(|s| s.projects.as_slice())
                .unwrap_or_default(),
            &rooms,
            &self.expanded,
            &self.query,
        );
        let changed = rows != self.rows || rooms.len() != self.rooms.len()
            || rooms.iter().zip(&self.rooms).any(|(a, b)| a.name.room_id() != b.name.room_id()
                || a.name.display() != b.name.display() || a.unread != b.unread || a.marked != b.marked
                || a.invited != b.invited || a.is_direct != b.is_direct || a.is_favorite != b.is_favorite);
        self.rooms = rooms;
        self.rows = rows;
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
        let captured = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        self.handle_row_actions(cx, &captured);
        if let Event::Actions(actions) = event {
            for action in actions {
                if matches!(action.downcast_ref::<crate::login::login_screen::LoginAction>(), Some(crate::login::login_screen::LoginAction::LoginSuccess)) {
                    self.refresh(cx);
                }
                if let Some(navigation) = action.downcast_ref::<crate::hagency::ui::ProjectRoomNavigation>()
                    && navigation.current() {
                    self.expand_navigation(cx, &navigation.identity, &navigation.project_id);
                }
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
                            self.snapshot = Some(snapshot.clone());
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
                    && self.mode == ProjectTreeMode::Chats {
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
                let row_selected = matches!(row,Row::Room(id)|Row::Invite(id) if selected.as_ref()==Some(id));
                item.navigation_bar_button(cx, ids!(row)).set_selected(cx, row_selected);
                let background = if row_selected {selection_color} else {Vec4f::default()};
                let project = if let Row::Project(id) = row {
                    self.snapshot
                        .as_ref()
                        .and_then(|s| s.projects.iter().find(|p| &p.id == id))
                } else {
                    None
                };
                let room = if let Row::Room(id) | Row::Invite(id) = row {
                    self.rooms.iter().find(|r| r.name.room_id() == id)
                } else {
                    None
                };
                let text = project
                    .map(|p| p.name.clone())
                    .or_else(|| room.map(|r| if r.is_favorite {
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
                let open = project
                    .is_some_and(|p| self.expanded.contains(&p.id) || !self.query.is_empty());
                let mut indent = item.view(cx, ids!(indent));
                indent.set_visible(cx, room.is_some());
                let style = (background, room.is_some());
                if self.row_styles.get(&item.widget_uid()) != Some(&style) {
                    self.row_styles.insert(item.widget_uid(), style);
                    let mut surface = item.view(cx, ids!(row_surface));
                    script_apply_eval!(cx, surface, {draw_bg.color: #(background)});
                    script_apply_eval!(cx, indent, {width: #(if room.is_some() {18.0} else {0.0})});
                }
                item.label(cx, ids!(name)).set_text(cx, &text);
                item.button(cx, ids!(more))
                    .set_visible(cx, project.is_some() || matches!(row, Row::Room(_)));
                item.view(cx, ids!(closed))
                    .set_visible(cx, project.is_some() && !open);
                item.view(cx, ids!(opened))
                    .set_visible(cx, project.is_some() && open);
                item.navigation_bar_button(cx, ids!(disclosure))
                    .set_visible(cx, project.is_some());
                let (unread, marked) = if let Some(p) = project {
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
            if inner.mode == ProjectTreeMode::Chats {inner.chat_query = inner.query.clone();}
            inner.mode = mode;
            inner.query = if mode == ProjectTreeMode::Chats {inner.chat_query.clone()} else {String::new()};
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
            let ids: HashSet<_> = snapshot.projects.iter().map(|p| p.id.clone()).collect();
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
            is_favorite: false,
        }
    }
    fn project(value: &str, rooms: &[&str]) -> TreeProject {
        TreeProject {
            id: value.into(),
            space: id(&format!("space-{value}")),
            name: value.into(),
            rooms: rooms.iter().map(|r| id(r)).collect(),
        }
    }
    #[test]
    fn refresh_error_stays_visible_beside_nonempty_cached_projects() {
        assert_eq!(tree_notice(ProjectTreeMode::Projects,Some("temporary"),false,false,false),("Hagency Projects could not be loaded. Retrying…",true));
        assert_eq!(tree_notice(ProjectTreeMode::Projects,Some("owner_authorization_required"),false,false,false),("Sign in again to restore Projects for this account",true));
        assert_eq!(tree_notice(ProjectTreeMode::Projects,None,false,false,false),("",false));
        assert_eq!(tree_notice(ProjectTreeMode::Chats,Some("temporary"),false,false,false),("",false),"Project notices do not leak into chat navigation");
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
        let chats = workspace_rows(ProjectTreeMode::Chats, &projects, &rooms, &expanded, "");
        assert!(!chats.iter().any(|row| matches!(row, Row::Project(_))));
        for name in ["Design", "General", "Alice"] {assert!(chats.contains(&Row::Room(id(name))));}
        assert!(chats.contains(&Row::Invite(id("Invite"))));
        assert!(!chats.contains(&Row::Room(id("Secret"))));
        assert_eq!(workspace_rows(ProjectTreeMode::Chats, &projects, &rooms, &expanded, "alice"), vec![Row::Direct, Row::Room(id("Alice"))]);
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
    fn invitations_remain_separate_until_matrix_acceptance() {
        let projects = vec![project("Work", &["Private"])];
        let mut invited = room("Private");
        invited.invited = true;
        let expanded = HashSet::from(["Work".into()]);
        assert_eq!(
            tree_rows(&projects, &[invited.clone()], &expanded, ""),
            vec![
                Row::Invitations,
                Row::Invite(id("Private")),
                Row::Project("Work".into())
            ]
        );
        invited.invited = false;
        assert_eq!(
            tree_rows(&projects, &[invited], &expanded, ""),
            vec![Row::Project("Work".into()), Row::Room(id("Private"))]
        );
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
}
