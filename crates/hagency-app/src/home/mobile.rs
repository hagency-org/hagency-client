//! Shared desktop and mobile navigation surfaces. Profiles and contacts come from
//! the active Matrix account; the presentation never substitutes fixture data.

use makepad_widgets::*;
use std::collections::HashSet;
use matrix_sdk::ruma::{api::client::profile::{AvatarUrl, DisplayName}, OwnedEventId, OwnedRoomId, OwnedUserId, UserId};

use crate::{
    home::navigation_tab_bar::{get_own_profile, NavigationBarAction, SelectedTab},
    app::SelectedRoom,
    home::rooms_list::RoomsListAction,
    logout::logout_confirm_modal::LogoutAction,
    profile::user_profile::UserProfile,
    profile::agent_details::AgentProfileDetailsWidgetExt,
    shared::{avatar::{AvatarState, AvatarWidgetRefExt}, navigation_bar_button::{NavigationBarButtonWidgetExt, NavigationBarButtonWidgetRefExt}},
    sliding_sync::{current_user_id, get_client, spawn_async_task, submit_async_request, MatrixRequest},
    utils,
    home::room_screen::RoomScreenWidgetExt,
    home::invite_screen::InviteScreenWidgetExt,
    shared::unread_badge::UnreadBadgeWidgetRefExt,
};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.MOBILE_BG = mod.widgets.RINX_PAGE
    mod.widgets.MOBILE_GREEN = mod.widgets.RINX_ACCENT
    mod.widgets.MOBILE_INK = mod.widgets.RINX_INK
    mod.widgets.MOBILE_MUTED = mod.widgets.RINX_MUTED

    mod.widgets.MobileAvatar = Avatar {
        width: 48 height: 48
        text_view +: {draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 5.0)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        }
        img_view +: {img +: {draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 5.0)
                sdf.fill(self.get_color())
                return sdf.result
            }
        }}}
    }

    mod.widgets.MobileTitle = SolidView {
        width: Fill height: 48
        flow: Overlay
        align: Align{x: 0.5 y: 0.5}
        draw_bg.color: mod.widgets.MOBILE_BG
        title := RinxLabel {
            width: Fit height: Fit padding: 0
            draw_text +: {color: mod.widgets.MOBILE_INK text_style: theme.font_bold {font_size: mod.widgets.RINX_HEADING_SIZE}}
        }
        controls := View {
            width: Fill height: Fill
            flow: Right align: Align{y: 0.5}
            left := RinxIconButton {
                width: RINX_ICON_BUTTON_SIZE height: RINX_ICON_BUTTON_SIZE padding: 0 spacing: 0 icon_walk: Walk{width: 16 height: 16}
                visible: false align: Align{x: 0.5 y: 0.5} draw_bg +: {color: #x00000000 color_hover: mod.widgets.RINX_HOVER color_down: mod.widgets.RINX_PRESSED}
                draw_icon +: {svg: ICON_CHEVRON_LEFT color: mod.widgets.MOBILE_INK}
            }
            View {width: Fill height: 1}
            right := RinxIconButton {
                width: RINX_ICON_BUTTON_SIZE height: RINX_ICON_BUTTON_SIZE padding: 0 spacing: 0 icon_walk: Walk{width: 16 height: 16}
                visible: false align: Align{x: 0.5 y: 0.5} draw_bg +: {color: #x00000000 color_hover: mod.widgets.RINX_HOVER color_down: mod.widgets.RINX_PRESSED}
                draw_icon +: {svg: ICON_ADD color: mod.widgets.MOBILE_INK}
            }
        }
    }

    mod.widgets.MobileRow = NavigationBarButton {
        width: Fill height: 56
        flow: Right spacing: 16 padding: Inset{left: 20 right: 16}
        align: Align{y: 0.5}
        draw_bg +: {color_hover: mod.widgets.RINX_HOVER color_active: mod.widgets.RINX_HOVER border_radius: 0}
        icon := Icon {
            icon_walk: Walk{width: 24 height: 24}
            draw_icon +: {color: mod.widgets.RINX_ACCENT svg: ICON_PEOPLE}
        }
        title := RinxLabel {
            width: Fill height: Fit max_lines: 1 text_overflow: Ellipsis
            draw_text +: {color: mod.widgets.MOBILE_INK text_style: theme.font_regular {font_size: mod.widgets.RINX_BODY_SIZE}}
        }
        chevron := Icon {
            icon_walk: Walk{width: 8 height: 13}
            draw_icon +: {color: mod.widgets.RINX_DISABLED svg: ICON_CHEVRON_RIGHT}
        }
    }
    mod.widgets.MobileSection = SolidView {
        width: Fill height: Fit flow: Down
        draw_bg.color: mod.widgets.RINX_SURFACE
    }
    mod.widgets.MobileDivider = SolidView {
        width: Fill height: 0.5 margin: Inset{left: 60}
        draw_bg.color: mod.widgets.RINX_BORDER
    }

}
pub fn hub_script_mod(vm:&mut ScriptVm) {hub_design::script_mod(vm);}
mod hub_design {
use super::*;
script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    mod.widgets.MobileHub = #(MobileHub::register_widget(vm)) {
        ..mod.widgets.SolidView
        width: Fill height: Fill flow: Right show_bg: true draw_bg.color: mod.widgets.MOBILE_BG
        master_pane := View {
            width: Fill height: Fill flow: Down
            title_bar := mod.widgets.MobileTitle {}
            pages := PageFlip {
                width: Fill height: Fill lazy_init: true active_page: @contacts
                contacts := View {
                    width: Fill height: Fill flow: Down
                    search_area := SolidView {
                        width: Fill height: 48 padding: Inset{left: 10 right: 10 bottom: 10}
                        draw_bg.color: mod.widgets.MOBILE_BG
                        search := RobrixTextInput {
                            width: Fill height: mod.widgets.RINX_CONTROL_HEIGHT padding: Inset{left: 12 right: 12 top: 8 bottom: 8}
                            empty_text: #(crate::i18n::tr("Search")) i18n_empty_text: "Search" autocapitalize: None
                            draw_text +: {color: mod.widgets.MOBILE_INK text_style: theme.font_regular {font_size: mod.widgets.RINX_UI_SIZE}}
                            draw_bg +: {
                                color: mod.widgets.RINX_FIELD color_hover: mod.widgets.RINX_FIELD color_focus: mod.widgets.RINX_FIELD
                                color_empty: mod.widgets.RINX_FIELD border_color: mod.widgets.RINX_BORDER border_color_focus: mod.widgets.RINX_ACCENT border_size: 1 border_radius: mod.widgets.RINX_CONTROL_RADIUS
                            }
                        }
                    }
                    unread_only := RinxCheckBox {text: #(crate::i18n::tr("Unread contacts")) i18n_text: "Unread contacts" margin: Inset{left: 16 bottom: 8}}
                    shortcuts := mod.widgets.MobileSection {
                        new_friends := mod.widgets.MobileRow {
                            title.text: #(crate::i18n::tr("New Friends")) title.i18n_text: "New Friends"
                            icon +: {draw_icon +: {svg: ICON_ADD_USER color: mod.widgets.RINX_WARNING_FG}}
                        }
                    }
                    status := RinxLabel {
                        width: Fill height: Fit padding: 16
                        draw_text +: {color: mod.widgets.MOBILE_MUTED text_style: theme.font_regular {font_size: mod.widgets.RINX_BODY_SIZE}}
                        text: #(crate::i18n::tr("Loading contacts…")) i18n_text: "Loading contacts…"
                    }
                    list := PortalList {
                        width: Fill height: Fill
                        Filler := SolidView {width: Fill height: 100 draw_bg.color: mod.widgets.RINX_SURFACE}
                        Contact := View {
                            width: Fill height: Fit flow: Down
                            section := SolidView {
                                width: Fill height: 28 padding: Inset{left: 16} align: Align{y: 0.5}
                                draw_bg.color: mod.widgets.MOBILE_BG
                                letter := RinxLabel {draw_text +: {color: mod.widgets.MOBILE_MUTED text_style: theme.font_regular {font_size: mod.widgets.RINX_BODY_SIZE}}}
                            }
                            row := NavigationBarButton {
                                width: Fill height: 60 flow: Right spacing: 14 padding: Inset{left: 16 right: 16}
                                align: Align{y: 0.5}
                                draw_bg +: {
                                    color_hover: mod.widgets.RINX_HOVER color_active: mod.widgets.RINX_SELECTED border_radius: 0
                                }
                                avatar := mod.widgets.MobileAvatar {width: 40 height: 40}
                                name := RinxLabel {
                                    width: Fill max_lines: 1 text_overflow: Ellipsis
                                    draw_text +: {color: mod.widgets.MOBILE_INK text_style: theme.font_regular {font_size: mod.widgets.RINX_BODY_SIZE}}
                                }
                                unread := UnreadBadge {}
                                agent_badge := RoundedView {
                                    visible: false width: Fit height: 20 padding: Inset{left: 6 right: 6}
                                    align: Align{x: 0.5 y: 0.5}
                                    draw_bg +: {color: mod.widgets.RBX_ACCENT_SOFT border_radius: 4}
                                    RinxLabel {
                                        padding: 0 text: #(crate::i18n::tr("Agent")) i18n_text: "Agent"
                                        draw_text +: {color: mod.widgets.RBX_ACCENT text_style: theme.font_bold{font_size: mod.widgets.RINX_META_SIZE}}
                                    }
                                }
                            }
                            mod.widgets.MobileDivider {margin: Inset{left: 70}}
                        }
                    }
                }
                discover := ScrollYView {
                    width: Fill height: Fill flow: Down spacing: 8
                    mod.widgets.MobileSection {
                        discover_mini_apps := mod.widgets.MobileRow {title.text: "Mini apps" icon.draw_icon.svg: ICON_SQUARES}
                        mod.widgets.MobileDivider {}
                        discover_article := mod.widgets.MobileRow {title.text: #(crate::i18n::tr("Article editor")) title.i18n_text: "Article editor" icon.draw_icon.svg: ICON_FILE}
                        mod.widgets.MobileDivider {}
                        discover_moments := mod.widgets.MobileRow {title.text: #(crate::i18n::tr("Moments")) title.i18n_text: "Moments" icon.draw_icon.svg: ICON_GLOBE}
                        mod.widgets.MobileDivider {}
                        explore := mod.widgets.MobileRow {title.text: #(crate::i18n::tr("Explore Groups & Spaces")) title.i18n_text: "Explore Groups & Spaces" icon.draw_icon.svg: ICON_GLOBE}
                    }
                }
                joined_groups := View {
                    width: Fill height: Fill flow: Down
                    group_status := RinxLabel {
                        width: Fill height: Fit padding: 16
                        draw_text +: {color: mod.widgets.MOBILE_MUTED text_style: theme.font_regular {font_size: mod.widgets.RINX_BODY_SIZE}}
                    }
                    group_list := PortalList {
                        width: Fill height: Fill
                        Filler := View {width: Fill height: 100}
                        Group := mod.widgets.MobileSection {
                            group_row := NavigationBarButton {
                                width: Fill height: 72 flow: Right spacing: 14 padding: Inset{left: 16 right: 16}
                                align: Align{y: 0.5}
                                draw_bg +: {color_hover: mod.widgets.RINX_BORDER border_radius: 0}
                                avatar := mod.widgets.MobileAvatar {width: 48 height: 48}
                                name := RinxLabel {
                                    width: Fill max_lines: 1 text_overflow: Ellipsis
                                    draw_text +: {color: mod.widgets.MOBILE_INK text_style: theme.font_regular {font_size: mod.widgets.RINX_BODY_SIZE}}
                                }
                            }
                            mod.widgets.MobileDivider {margin: Inset{left: 78}}
                        }
                    }
                }
                account := ScrollYView {
                    width: Fill height: Fill flow: Down spacing: 8
                    mod.widgets.MobileSection {
                        own_profile := NavigationBarButton {
                            width: Fill height: 128 flow: Right spacing: 20 padding: Inset{left: 24 right: 20}
                            align: Align{y: 0.5}
                            avatar := mod.widgets.MobileAvatar {width: 64 height: 64}
                            View {
                                width: Fill height: Fit flow: Down spacing: 12
                                name := RinxLabel {
                                    width: Fill max_lines: 1 text_overflow: Ellipsis
                                    draw_text +: {color: mod.widgets.MOBILE_INK text_style: theme.font_bold {font_size: mod.widgets.RINX_TITLE_SIZE}}
                                }
                                user_id := RinxLabel {
                                    width: Fill max_lines: 1 text_overflow: Ellipsis
                                    draw_text +: {color: mod.widgets.MOBILE_MUTED text_style: theme.font_regular {font_size: mod.widgets.RINX_BODY_SIZE}}
                                }
                            }
                            Icon {icon_walk: Walk{width: 8 height: 13} draw_icon +: {color: mod.widgets.RINX_DISABLED svg: ICON_CHEVRON_RIGHT}}
                        }
                    }
                    mod.widgets.MobileSection {
                        my_posts := mod.widgets.MobileRow {title.text: #(crate::i18n::tr("My Posts")) title.i18n_text: "My Posts" icon.draw_icon.svg: ICON_GLOBE}
                        mod.widgets.MobileDivider {}
                        settings := mod.widgets.MobileRow {title.text: #(crate::i18n::tr("Settings")) title.i18n_text: "Settings" icon.draw_icon.svg: ICON_SETTINGS}
                    }
                }
            }
        }
        detail_divider := SolidView {
            width: 1 height: Fill visible: false draw_bg.color: mod.widgets.RINX_DIVIDER
        }
        detail_pane := View {
            width: Fill height: Fill flow: Down visible: false
            detail_title_bar := mod.widgets.MobileTitle {
                visible: false
                title.text: #(crate::i18n::tr("Contact Info")) title.i18n_text: "Contact Info"
            }
            conversations := View {visible: false width: Fill height: Fill flow: Right
                conversation_list_holder := View {visible: false width: 180 height: Fill
                conversation_list := PortalList {width: Fill height: Fill
                    Conversation := NavigationBarButton {width: Fill height: 64 flow: Down padding: 10 spacing: 4
                        name := RinxLabel {width: Fill max_lines: 1 text_overflow: Ellipsis}
                        identity := RinxLabel {width: Fill max_lines: 1 text_overflow: Ellipsis draw_text.color: mod.widgets.RINX_MUTED}
                        unread := UnreadBadge {}
                    }
                    Filler := View {width: Fill height: 64}
                }}
                contact_chat_holder := View {width: Fill height: Fill
                    contact_chat := RoomScreen {width: Fill height: Fill desktop_chat_header +: {visible: true}}
                }
                contact_invite_holder := View {visible: false width: Fill height: Fill
                    contact_invite := InviteScreen {width: Fill height: Fill}
                }
            }
            contact_profile := ScrollYView {
                visible: false
                width: Fill height: Fill flow: Down
                contact_card := DetailContactCard {}
                DetailGap {}
                agent_details := AgentProfileDetails {}
                DetailSection {
                    contact_moments := DetailRow {title.text: #(crate::i18n::tr("Moments")) title.i18n_text: "Moments"}
                    contact_copy := DetailRow {title.text: #(crate::i18n::tr("Copy Profile Link")) title.i18n_text: "Copy Profile Link"}
                }
                DetailGap {}
                DetailSection {message := DetailAction {title.text: #(crate::i18n::tr("Messages")) title.i18n_text: "Messages"}}
                DetailGap {}
                DetailSection {contact_block := DetailAction {title +: {text: #(crate::i18n::tr("Block")) i18n_text: "Block" draw_text.color: mod.widgets.RINX_DANGER_FG}}}
            }
        }
    }
}

}
#[derive(Debug)]
struct ContactsLoaded {
    owner: OwnedUserId,
    request: u64,
    epoch: u64,
    origin: String,
    result: Result<(Vec<UserProfile>, Vec<DirectConversation>), String>,
}

#[derive(Clone, Debug)]
struct DirectConversation {target: OwnedUserId, room: utils::RoomNameId}

/// The SDK unread counters apply to each original room; multiple rooms are never collapsed.
fn contact_unread(conversations: &[DirectConversation], target: &UserId) -> (u64, bool) {
    let Some(client) = get_client() else {return (0, false);};
    conversations.iter().filter(|c| c.target == target).filter_map(|c| client.get_room(c.room.room_id()))
        .fold((0u64, false), |(n,m),r| (n.saturating_add(r.num_unread_messages().max(r.num_unread_mentions())), m || r.is_marked_unread() || r.state()==matrix_sdk::RoomState::Invited))
}
fn contact_order(contacts:&[UserProfile],query:&str,unread_only:bool,counts:impl Fn(&UserId)->(u64,bool))->Vec<usize> {
    let query=query.to_lowercase();
    let mut rows:Vec<_>=contacts.iter().enumerate().filter_map(|(i,p)| {
        let (n,m)=counts(&p.user_id);
        ((p.displayable_name().to_lowercase().contains(&query) || p.user_id.as_str().to_lowercase().contains(&query)) && (!unread_only || n>0 || m)).then_some((i,n>0 || m))
    }).collect();
    rows.sort_by_key(|(i,unread)|(!unread,contacts[*i].displayable_name().to_lowercase(),contacts[*i].user_id.clone()));
    rows.into_iter().map(|(i,_)|i).collect()
}
pub(crate) fn is_direct_selection(cx:&mut Cx,room:&SelectedRoom)->bool {
    matches!(room,SelectedRoom::JoinedRoom {..}|SelectedRoom::Thread {..}|SelectedRoom::InvitedRoom {..})
        && (get_client().and_then(|c|c.get_room(room.room_id())).is_some_and(|r|!r.direct_targets().is_empty())
            || super::rooms_list::get_invited_rooms(cx).borrow().get(room.room_id()).is_some_and(|r|r.is_direct && !r.is_space))
}
pub(crate) fn direct_unread(cx: &mut Cx) -> (u64, bool) {
    let invited=super::rooms_list::get_invited_rooms(cx).borrow().values().any(|r|r.is_direct && !r.is_space);
    let Some(client) = get_client() else {return (0,false);};
    client.joined_rooms().iter().filter(|r| !r.is_space() && !r.direct_targets().is_empty())
        .fold((0u64,invited), |(n,m),r| (n.saturating_add(r.num_unread_messages().max(r.num_unread_mentions())),m || r.is_marked_unread()))
}

#[derive(Debug)]
struct ContactAgentsLoaded {
    owner: OwnedUserId,
    origin: String,
    epoch: u64,
    request: u64,
    agents: std::collections::HashSet<OwnedUserId>,
}

#[derive(Clone, Debug)]
struct MobileGroup {
    name: utils::RoomNameId,
    avatar: AvatarState,
}

#[derive(Debug)]
struct GroupsLoaded {
    owner: OwnedUserId,
    request: u64,
    groups: Vec<MobileGroup>,
}

#[derive(Clone)]
struct DirectNavigation {epoch:u64,room:utils::RoomNameId,started:std::time::Instant,thread:Option<OwnedEventId>,jump:Option<OwnedEventId>}
#[derive(Default)]
struct PendingDirectChat(Option<DirectNavigation>);
pub(crate) fn navigate_direct(cx:&mut Cx,room:utils::RoomNameId) {navigate_direct_view(cx,room,None,None);}
pub(crate) fn navigate_direct_view(cx:&mut Cx,room:utils::RoomNameId,thread:Option<OwnedEventId>,jump:Option<OwnedEventId>) {
    cx.global::<PendingDirectChat>().0=Some(DirectNavigation {epoch:crate::account_session::epoch(),room,started:std::time::Instant::now(),thread,jump});
    cx.action(NavigationBarAction::GoToTab(SelectedTab::Contacts));
    cx.redraw_all();
}

#[derive(Debug)]
pub enum MobileNavigationAction {
    DetailVisibility(bool),
    NewChatRequested,
    OpenDirect(utils::RoomNameId),
}

#[derive(Script, ScriptHook, Widget)]
pub struct MobileHub {
    #[source] source: ScriptObjectRef,
    #[deref] view: View,
    /// 0: Contacts, 1: Discover, 2: Me.
    #[live] kind: u32,
    #[rust] owner: Option<OwnedUserId>,
    #[rust] owner_epoch: u64,
    #[rust] contacts: Vec<UserProfile>,
    #[rust] conversations: Vec<DirectConversation>,
    #[rust] visible_conversations: Vec<utils::RoomNameId>,
    #[rust] selected_conversation: Option<OwnedRoomId>,
    #[rust] selected_thread: Option<OwnedEventId>,
    #[rust] suspended_conversation: Option<utils::RoomNameId>,
    #[rust] unread_only: bool,
    #[rust] contacts_pending: bool,
    #[rust] unread_counts: (u64,bool),
    #[rust] contact_agents: HashSet<OwnedUserId>,
    #[rust] visible_contacts: Vec<usize>,
    #[rust] selected_profile: Option<UserProfile>,
    #[rust] adding_friend: bool,
    #[rust] groups_open: bool,
    #[rust] groups: Vec<MobileGroup>,
    #[rust] group_request: u64,
    #[rust] query: String,
    #[rust] request: u64,
    #[rust] status: String,
}

impl MobileHub {
    fn begin_new_chat(&mut self, cx: &mut Cx) {
        if self.kind != 0 { return; }
        self.groups_open = false;
        self.suspend_chat(cx);
        self.suspended_conversation=None;
        self.selected_profile = None;
        if !self.adding_friend {
            self.request = Self::next_request();
            self.adding_friend = true;
            self.query.clear();
            self.contacts.clear();
            self.contact_agents.clear();
            self.status = crate::i18n::tr("Search by Matrix ID or name, then press Enter.").into();
            self.view.text_input(cx, ids!(search)).set_text(cx, "");
        }
        self.view.text_input(cx, ids!(search)).set_key_focus(cx);
        cx.action(MobileNavigationAction::DetailVisibility(true));
        self.view.redraw(cx);
    }

    fn clear_account_state(&mut self, cx: &mut Cx) {
        self.owner = None;
        self.owner_epoch=crate::account_session::epoch();
        self.contacts.clear();
        self.conversations.clear();
        self.visible_conversations.clear();
        self.selected_conversation = None;
        self.selected_thread=None;
        self.suspended_conversation=None;
        self.contacts_pending = false;
        self.unread_only = false;
        self.view.room_screen(cx, ids!(contact_chat)).hide_displayed_room(cx);
        self.view.invite_screen(cx,ids!(contact_invite)).hide_displayed_invite(cx);
        self.contact_agents.clear();
        self.visible_contacts.clear();
        self.selected_profile = None;
        self.adding_friend = false;
        self.groups_open = false;
        self.groups.clear();
        self.group_request = 0;
        self.query.clear();
        self.request = 0;
        self.status.clear();
        self.view.text_input(cx, ids!(search)).set_text(cx, "");
        cx.action(MobileNavigationAction::DetailVisibility(false));
    }

    fn next_request() -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    fn refresh_groups(&mut self, cx: &mut Cx) {
        let Some(client) = get_client() else { return };
        let Some(owner) = client.user_id().map(ToOwned::to_owned) else { return };
        self.request = 0;
        self.group_request = Self::next_request();
        let request = self.group_request;
        self.status = crate::i18n::tr("Loading group chats…").into();
        self.view.redraw(cx);
        spawn_async_task(async move {
            let mut groups = Vec::new();
            for room in client.joined_rooms() {
                if crate::moments::is_moments(&room) {continue;}
                if !room.is_space() && !room.is_direct().await.unwrap_or(false) {
                    groups.push(MobileGroup {
                        name: utils::RoomNameId::from_room(&room).await,
                        avatar: AvatarState::Known(room.avatar_url()),
                    });
                }
            }
            groups.sort_by_key(|group| (group.name.display().to_lowercase(), group.name.room_id().clone()));
            Cx::post_action(GroupsLoaded {owner, request, groups});
        });
    }

    fn refresh_contacts(&mut self, cx: &mut Cx, search_directory: bool) {
        let Some(client) = get_client() else { return };
        let Some(owner) = client.user_id().map(ToOwned::to_owned) else { return };
        self.owner = Some(owner.clone());
        self.owner_epoch=crate::account_session::epoch();
        self.contacts_pending = true;
        // A globally unique generation also distinguishes separate widget instances
        // recreated by AdaptiveView and prevents late requests overwriting new ones.
        self.request = Self::next_request();
        let request = self.request;
        let query = self.query.clone();
        self.contact_agents.clear();
        let badge_owner = owner.clone();
        let origin = client.homeserver().to_string();
        let epoch = crate::account_session::epoch();
        spawn_async_task(async move {
            let viewer = badge_owner.clone();
            let read = tokio::spawn(async move {crate::profile::agent_details::visible_agent_ids(&viewer).await});
            let agents = match tokio::time::timeout(std::time::Duration::from_secs(25), read).await {
                Ok(Ok(Ok(ids))) => ids,
                _ => std::collections::HashSet::new(),
            };
            Cx::post_action(ContactAgentsLoaded {owner: badge_owner, origin, epoch, request, agents});
        });
        self.status = crate::i18n::tr("Loading contacts…").into();
        self.view.redraw(cx);
        let origin=client.homeserver().to_string();
        spawn_async_task(async move {
            let result = async {
                let mut profiles = Vec::new();
                let mut conversations = Vec::new();
                if search_directory && !query.trim().is_empty() {
                    // Full Matrix IDs also work when a user is absent from the directory.
                    if let Ok(id) = UserId::parse(query.trim()) {
                        let p = client.account().fetch_user_profile_of(&id).await.map_err(|e| e.to_string())?;
                        profiles.push(UserProfile {
                            user_id: id,
                            username: p.get_static::<DisplayName>().ok().flatten(),
                            avatar_state: AvatarState::Known(p.get_static::<AvatarUrl>().ok().flatten()),
                        });
                    } else {
                        let results = client.search_users(&query, 100).await.map_err(|e| e.to_string())?;
                        profiles.extend(results.results.into_iter().filter(|u| u.user_id != owner).map(|u| UserProfile {
                            user_id: u.user_id, username: u.display_name, avatar_state: AvatarState::Known(u.avatar_url),
                        }));
                    }
                } else {
                    let mut ids = std::collections::BTreeSet::new();
                    for room in client.joined_rooms() {
                if crate::moments::is_moments(&room) {continue;}
                        for target in room.direct_targets() {
                            if let Ok(id) = UserId::parse(target.as_str()) {
                                if id != owner {
                                    ids.insert(id.clone());
                                    conversations.push(DirectConversation {target: id, room: utils::RoomNameId::from_room(&room).await});
                                }
                            }
                        }
                    }
                    for room in client.invited_rooms() {
                        if room.is_space() || !room.is_direct().await.unwrap_or(false) {continue;}
                        if let Ok(invite)=room.invite_details().await {
                            let id=invite.inviter_id;
                            if id!=owner {
                                ids.insert(id.clone());
                                conversations.push(DirectConversation {target:id,room:utils::RoomNameId::from_room(&room).await});
                            }
                        }
                    }
                    for id in ids {
                        let p = client.account().fetch_user_profile_of(&id).await;
                        profiles.push(UserProfile {
                            username: p.as_ref().ok().and_then(|p| p.get_static::<DisplayName>().ok().flatten()),
                            avatar_state: AvatarState::Known(p.ok().and_then(|p| p.get_static::<AvatarUrl>().ok().flatten())),
                            user_id: id,
                        });
                    }
                }
                profiles.sort_by_key(|p| (p.displayable_name().to_lowercase(), p.user_id.clone()));
                Ok((profiles, conversations))
            }.await;
            Cx::post_action(ContactsLoaded {owner, request, result, origin, epoch});
        });
    }

    fn conversation_menu(&self,cx:&mut Cx,room:&utils::RoomNameId,rect:Rect) {
        if self.owner_epoch!=crate::account_session::epoch() || crate::sliding_sync::account_changing() {return;}
        let list=cx.global::<super::rooms_list::RoomsListRef>().clone();
        if let Some(details)=list.room_context_details(room.room_id()) {
            cx.widget_action(self.widget_uid(),RoomsListAction::OpenRoomContextMenu {details,pos:dvec2(rect.pos.x+rect.size.x,rect.pos.y+rect.size.y)});
        }
    }
    fn open_direct(&mut self,cx:&mut Cx,room:utils::RoomNameId)->bool {self.open_direct_view(cx,room,None,None)}
    fn suspend_chat(&mut self,cx:&mut Cx) {
        if let Some(id)=self.selected_conversation.take() {
            self.suspended_conversation=self.conversations.iter().find(|c|c.room.room_id()==&id).map(|c|c.room.clone());
            self.selected_thread=None;
            self.view.room_screen(cx,ids!(contact_chat)).hide_displayed_room(cx);
            self.view.invite_screen(cx,ids!(contact_invite)).hide_displayed_invite(cx);
        }
    }
    fn open_direct_view(&mut self, cx: &mut Cx, room: utils::RoomNameId,thread:Option<OwnedEventId>,jump:Option<OwnedEventId>) -> bool {
        self.adding_friend=false;
        self.query.clear();
        self.view.text_input(cx,ids!(search)).set_text(cx,"");
        let Some(sdk_room) = get_client().and_then(|c| c.get_room(room.room_id())) else {return false;};
        let invited=sdk_room.state()==matrix_sdk::RoomState::Invited && super::rooms_list::get_invited_rooms(cx).borrow().get(room.room_id()).is_some_and(|r|r.is_direct);
        if !invited && (sdk_room.state() != matrix_sdk::RoomState::Joined || sdk_room.direct_targets().is_empty()) {return false;}
        if invited {
            self.view.room_screen(cx,ids!(contact_chat)).hide_displayed_room(cx);
            if self.selected_conversation.as_ref()!=Some(room.room_id()) {
                self.view.invite_screen(cx,ids!(contact_invite)).hide_displayed_invite(cx);
                self.view.invite_screen(cx,ids!(contact_invite)).set_displayed_invite(cx,&room);
            }
            self.selected_conversation=Some(room.room_id().clone());
            self.selected_thread=None;
            if let Some(target)=self.conversations.iter().find(|c|c.room.room_id()==room.room_id()).map(|c|&c.target) {
                self.selected_profile=self.contacts.iter().find(|p|&p.user_id==target).cloned();
            }
            cx.action(crate::app::AppStateAction::RoomFocused(SelectedRoom::InvitedRoom {room_name_id:room}));
            self.view.redraw(cx);
            return true;
        }
        self.view.invite_screen(cx,ids!(contact_invite)).hide_displayed_invite(cx);
        if self.selected_conversation.as_ref() != Some(room.room_id()) || self.selected_thread!=thread {
            super::main_desktop_ui::release_legacy_direct_timeline(cx,&room);
            self.view.room_screen(cx, ids!(contact_chat)).set_displayed_room(cx,&room,thread.clone());
            self.selected_conversation = Some(room.room_id().clone());
            self.selected_thread=thread.clone();
        }
        if let Some(event)=jump {self.view.room_screen(cx,ids!(contact_chat)).jump_to_history_event(cx,event);}
        if let Some(target) = sdk_room.direct_targets().iter().find_map(|t| UserId::parse(t.as_str()).ok()) {
            self.selected_profile = self.contacts.iter().find(|p|p.user_id==target).cloned().or_else(||Some(UserProfile {user_id:target,username:Some(room.display().into_owned()),avatar_state:AvatarState::Unknown}));
        }
        cx.action(crate::app::AppStateAction::RoomFocused(if let Some(thread_root_event_id)=thread {SelectedRoom::Thread {room_name_id:room,thread_root_event_id}}else{SelectedRoom::JoinedRoom {room_name_id:room}}));
        self.view.redraw(cx);
        true
    }
    fn back(&mut self, cx: &mut Cx) {
        self.view.room_screen(cx,ids!(contact_chat)).hide_displayed_room(cx);
        self.view.invite_screen(cx,ids!(contact_invite)).hide_displayed_invite(cx);
        self.selected_conversation=None;
        self.suspended_conversation=None;
        if self.selected_profile.take().is_none() {
            self.adding_friend = false;
            self.groups_open = false;
            self.group_request = 0;
            self.query.clear();
            self.view.text_input(cx, ids!(search)).set_text(cx, "");
            self.refresh_contacts(cx, false);
        }
        cx.action(MobileNavigationAction::DetailVisibility(self.adding_friend));
        self.view.redraw(cx);
    }

    fn populate_profile(cx: &mut Cx, widget: &WidgetRef, profile: &UserProfile) {
        widget.label(cx, ids!(name)).set_text(cx, profile.displayable_name());
        widget.label(cx, ids!(user_id)).set_text(cx, profile.user_id.as_str());
        let avatar = widget.avatar(cx, ids!(avatar));
        let mut avatar_state = profile.avatar_state.clone();
        if let Some(image) = avatar_state.update_from_cache(cx) {
            if avatar.show_image(cx, None, |cx, img| utils::load_avatar_image(&img, cx, image)).is_ok() { return; }
        }
        avatar.show_user_text(cx, &profile.user_id, profile.displayable_name());
    }
}

impl Widget for MobileHub {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if matches!(event, Event::Signal) {
            crate::profile::user_profile_cache::process_user_profile_updates(cx);
            crate::avatar_cache::process_avatar_updates(cx);
            if self.kind==0 {
                let counts=direct_unread(cx);
                if counts!=self.unread_counts || cx.global::<PendingDirectChat>().0.is_some() {self.unread_counts=counts;self.view.redraw(cx);}
                let known:HashSet<_>=self.conversations.iter().map(|c|c.room.room_id().clone()).collect();
                let mut current:HashSet<_>=get_client().map(|c|c.joined_rooms().iter().filter(|r|!r.direct_targets().is_empty() && !crate::moments::is_moments(r)).map(|r|r.room_id().to_owned()).collect()).unwrap_or_default();
                current.extend(super::rooms_list::get_invited_rooms(cx).borrow().values().filter(|r|r.is_direct && !r.is_space).map(|r|r.room_name_id.room_id().clone()));
                if self.selected_conversation.as_ref().is_some_and(|id|!current.contains(id)) {self.suspend_chat(cx);self.suspended_conversation=None;}
                let changed=current!=known;
                if changed && !self.contacts_pending && !self.adding_friend {self.refresh_contacts(cx,false);}
            }
        }
        if self.kind == 0 && (self.selected_profile.is_some() || self.adding_friend || self.groups_open)
            && scope.data.get::<crate::app::AppState>().is_some_and(|app| app.selected_tab == SelectedTab::Contacts)
            && event.back_pressed() {
            self.back(cx);
            return;
        }
        // Hidden contact timelines must not consume room input or send read receipts.
        if self.kind == 0 && scope.data.get::<crate::app::AppState>().is_some_and(|a| a.selected_tab != SelectedTab::Contacts) {
            self.suspend_chat(cx);
        }
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            for action in actions {
                if let RoomsListAction::InviteAccepted {room_name_id,..}=action.as_widget_action().cast()
                    && self.kind==0 && self.selected_conversation.as_ref()==Some(room_name_id.room_id()) {
                    self.selected_conversation=None;
                    self.view.invite_screen(cx,ids!(contact_invite)).hide_displayed_invite(cx);
                    self.refresh_contacts(cx,false);
                    navigate_direct(cx,room_name_id);
                }
                if let Some(MobileNavigationAction::OpenDirect(room)) = action.downcast_ref() {
                    if self.kind == 0 {self.open_direct(cx, room.clone());}
                }
                if let Some(loaded) = action.downcast_ref::<ContactAgentsLoaded>() {
                    if loaded.request == self.request && loaded.epoch == crate::account_session::epoch()
                        && Some(&loaded.owner) == current_user_id().as_ref()
                        && get_client().is_some_and(|client| client.homeserver().as_str() == loaded.origin)
                        && !crate::sliding_sync::account_changing() {
                        self.contact_agents = loaded.agents.clone();
                        self.view.redraw(cx);
                    }
                }
                if let Some(loaded) = action.downcast_ref::<GroupsLoaded>() {
                    if Some(&loaded.owner) == current_user_id().as_ref() && loaded.request == self.group_request {
                        self.groups = loaded.groups.clone();
                        self.status = if self.groups.is_empty() { crate::i18n::tr("No group chats yet.") } else { "" }.into();
                        self.view.redraw(cx);
                    }
                }
                if let Some(loaded) = action.downcast_ref::<ContactsLoaded>() {
                    if Some(&loaded.owner) == current_user_id().as_ref() && loaded.request == self.request
                        && loaded.epoch==crate::account_session::epoch() && !crate::sliding_sync::account_changing()
                        && get_client().is_some_and(|c|c.homeserver().as_str()==loaded.origin) {
                        match &loaded.result {
                            Ok((contacts, conversations)) => {
                                self.contacts_pending = false;
                                self.contacts = contacts.clone();
                                self.conversations = conversations.clone();
                                self.status = if contacts.is_empty() {
                                    if self.query.is_empty() { crate::i18n::tr("No contacts yet. Tap + to find a friend.") } else { crate::i18n::tr("No people found.") }
                                } else { "" }.into();
                            }
                            Err(error) => {self.contacts_pending = false; self.status = crate::i18n::format("Could not load contacts: {error}", &[("error", (error).to_string())]);},
                        }
                        self.view.redraw(cx);
                    }
                }
                if let Some(NavigationBarAction::TabSelected(SelectedTab::Contacts)) = action.downcast_ref() {
                    if self.kind == 0 {
                        if let Some(room)=self.suspended_conversation.take() {self.open_direct(cx,room);}
                        if self.groups_open { self.refresh_groups(cx); }
                        else if !self.adding_friend { self.refresh_contacts(cx, false); }
                    }
                }
                if matches!(action.downcast_ref(), Some(MobileNavigationAction::NewChatRequested)) {
                    self.begin_new_chat(cx);
                }
                if let Some(LogoutAction::ClearAppState {..}) = action.downcast_ref() {
                    self.clear_account_state(cx);
                }
            }
            if let Some(active) = self.view.check_box(cx, ids!(unread_only)).changed(actions) {self.unread_only = active; self.view.redraw(cx);}
            let conversations = self.view.portal_list(cx, ids!(conversation_list));
            for (index, widget) in conversations.items_with_actions(actions) {
                if widget.as_navigation_bar_button().secondary_clicked(actions)
                    && let Some(room)=self.visible_conversations.get(index) {self.conversation_menu(cx,room,widget.area().rect(cx));continue;}
                if !conversations.was_scrolling() && widget.as_navigation_bar_button().clicked(actions)
                    && let Some(room) = self.visible_conversations.get(index).cloned() {self.open_direct(cx, room);}
            }
            let search = self.view.text_input(cx, ids!(search));
            if let Some(query) = search.changed(actions) {
                self.query = query;
                self.view.redraw(cx);
            }
            if search.returned(actions).is_some() { self.refresh_contacts(cx, true); }
            if self.view.button(cx, ids!(title_bar.controls.right)).clicked(actions)
                || self.view.navigation_bar_button(cx, ids!(new_friends)).clicked(actions)
            {
                self.begin_new_chat(cx);
                if !self.query.trim().is_empty() { self.refresh_contacts(cx, true); }
            }
            let list = self.view.portal_list(cx, ids!(list));
            for (index, widget) in list.items_with_actions(actions) {
                if widget.navigation_bar_button(cx,ids!(row)).secondary_clicked(actions)
                    && let Some(profile)=self.visible_contacts.get(index).and_then(|i|self.contacts.get(*i)) {
                    let rooms:Vec<_>=self.conversations.iter().filter(|c|c.target==profile.user_id).collect();
                    let room=rooms.iter().find(|c|Some(c.room.room_id())==self.selected_conversation.as_ref()).or_else(||if rooms.len()==1 {rooms.first()}else{None});
                    if let Some(room)=room {self.conversation_menu(cx,&room.room,widget.area().rect(cx));}
                    continue;
                }
                if !list.was_scrolling() && widget.navigation_bar_button(cx, ids!(row)).clicked(actions) {
                    self.selected_profile = self.visible_contacts.get(index).and_then(|i| self.contacts.get(*i)).cloned();
                    if !self.adding_friend && let Some(profile) = self.selected_profile.clone() {
                        let mut rooms: Vec<_> = self.conversations.iter().filter(|c| c.target == profile.user_id).map(|c| c.room.clone()).collect();
                        rooms.sort_by_key(|r| std::cmp::Reverse(get_client().and_then(|c| c.get_room(r.room_id())).map_or(0,|r| r.num_unread_messages().max(r.num_unread_mentions()))));
                        if let Some(room) = rooms.first() {self.open_direct(cx, room.clone());}
                    }
                    let mobile_detail = self.selected_profile.is_some() && !super::home_screen::effective_is_desktop(cx);
                    cx.action(MobileNavigationAction::DetailVisibility(mobile_detail));
                    self.view.redraw(cx);
                }
            }
            if self.view.button(cx, ids!(title_bar.controls.left)).clicked(actions)
                || self.view.button(cx, ids!(detail_title_bar.controls.left)).clicked(actions) {
                self.back(cx);
            }
            if self.view.navigation_bar_button(cx,ids!(discover_mini_apps)).clicked(actions){cx.action(crate::octoscript_apps::MiniAppsAction::Open);}
            if self.view.navigation_bar_button(cx, ids!(discover_article)).clicked(actions) { crate::system_apps::open_article(cx); }
            if self.view.navigation_bar_button(cx, ids!(discover_moments)).clicked(actions) {
                cx.action(crate::moments::ui::MomentsAction::Open {author: None});
            }
            if self.view.navigation_bar_button(cx, ids!(my_posts)).clicked(actions) {
                cx.action(crate::moments::ui::MomentsAction::Open {author: current_user_id()});
            }
            if self.view.agent_profile_details(cx, ids!(agent_details)).allows_moments()
                && self.view.navigation_bar_button(cx, ids!(contact_moments)).clicked(actions) {
                if let Some(profile) = &self.selected_profile {
                    cx.action(crate::moments::ui::MomentsAction::Open {author: Some(profile.user_id.clone())});
                }
            }
            if self.view.navigation_bar_button(cx, ids!(message)).clicked(actions) {
                if let Some(profile) = self.selected_profile.clone() {
                    submit_async_request(MatrixRequest::OpenOrCreateDirectMessage {user_profile: profile, allow_create: false});
                }
            }
            let group_list = self.view.portal_list(cx, ids!(group_list));
            for (index, widget) in group_list.items_with_actions(actions) {
                if !group_list.was_scrolling() && widget.navigation_bar_button(cx, ids!(group_row)).clicked(actions) {
                    if let Some(group) = self.groups.get(index) {
                        cx.widget_action(self.widget_uid(), RoomsListAction::Selected(SelectedRoom::JoinedRoom {room_name_id: group.name.clone()}));
                    }
                }
            }
            if self.view.navigation_bar_button(cx, ids!(explore)).clicked(actions) {
                cx.action(NavigationBarAction::GoToAddRoom);
            }
            if let Some(profile) = &self.selected_profile {
                if self.view.navigation_bar_button(cx, ids!(contact_copy)).clicked(actions) {
                    cx.copy_to_clipboard(&profile.user_id.matrix_to_uri().to_string());
                    crate::shared::popup_list::enqueue_popup_notification(crate::i18n::tr("Profile link copied."), crate::shared::popup_list::PopupKind::Success, Some(2.0));
                }
                if self.view.navigation_bar_button(cx, ids!(contact_block)).clicked(actions) {
                    cx.action(crate::block_user_modal::BlockUserModalAction::Open(crate::block_user_modal::BlockUserRequest {
                        user_id: profile.user_id.clone(), display_name: profile.username.clone(),
                        block: !crate::sliding_sync::is_user_blocked(&profile.user_id), reject_invite_to: None,
                    }));
                }
            }
            if self.view.navigation_bar_button(cx, ids!(settings)).clicked(actions) {
                cx.action(NavigationBarAction::OpenSettings);
            }
            if self.view.navigation_bar_button(cx, ids!(own_profile)).clicked(actions) {
                cx.action(NavigationBarAction::OpenOwnProfile);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.kind == 0 && (self.owner.as_ref() != current_user_id().as_ref() || self.owner_epoch!=crate::account_session::epoch()) {
            // Lazy pages may miss the logout action while another tab is
            // active. Clear profile, search and group state before drawing any
            // content for a different signed-in account.
            self.clear_account_state(cx);
            self.refresh_contacts(cx, false);
        }
        if self.kind == 1 {
            let invites = get_client().map(|c|c.invited_rooms().into_iter().filter(crate::moments::is_moments).count()).unwrap_or(0);
            self.view.label(cx, ids!(discover_moments.title)).set_text(cx, &if invites == 0 {crate::i18n::tr("Moments").into()} else {crate::i18n::format("Moments · {invites} invitation{0}", &[("invites", (invites).to_string()), ("0", (crate::i18n::plural_suffix(invites)).to_string())])});
        }
        if self.kind==0 && scope.data.get::<crate::app::AppState>().is_some_and(|a|a.selected_tab==SelectedTab::Contacts)
            && let Some(pending)=cx.global::<PendingDirectChat>().0.clone() {
            if pending.epoch!=crate::account_session::epoch() || self.open_direct_view(cx,pending.room,pending.thread,pending.jump) {cx.global::<PendingDirectChat>().0=None;}
            else if pending.started.elapsed()>std::time::Duration::from_secs(30) {
                cx.global::<PendingDirectChat>().0=None;
                self.status=crate::i18n::tr("Could not open this private chat. Reopen Contacts and retry.").into();
            }
        }
        let invite_open=self.selected_conversation.as_ref().is_some_and(|id|get_client().and_then(|c|c.get_room(id)).is_some_and(|r|r.state()==matrix_sdk::RoomState::Invited));
        let chat_open = self.selected_conversation.is_some();
        self.view.view(cx,ids!(contact_chat_holder)).set_visible(cx,!invite_open);
        self.view.view(cx,ids!(contact_invite_holder)).set_visible(cx,invite_open);
        let profile_open = self.selected_profile.is_some() && !chat_open;
        let agent_details = self.view.agent_profile_details(cx, ids!(agent_details));
        agent_details.set_user(cx,
            self.selected_profile.as_ref().map(|profile| profile.user_id.as_ref()));
        self.view.navigation_bar_button(cx, ids!(contact_moments)).set_visible(cx, agent_details.allows_moments());
        let split = self.kind == 0 && super::home_screen::effective_is_desktop(cx);
        let mut master = self.view.view(cx, ids!(master_pane));
        master.set_visible(cx, split || !(profile_open || chat_open));
        script_apply_eval!(cx, master, {width: #(if split {Size::Fixed(300.0)} else {Size::fill()})});
        self.view.view(cx, ids!(detail_divider)).set_visible(cx, split);
        self.view.view(cx, ids!(detail_pane)).set_visible(cx, split || profile_open || chat_open);
        self.view.view(cx, ids!(detail_title_bar)).set_visible(cx, profile_open);
        self.view.button(cx, ids!(detail_title_bar.controls.left)).set_visible(cx, !split);
        self.view.view(cx, ids!(detail_pane.contact_profile)).set_visible(cx, profile_open);
        self.view.view(cx, ids!(conversations)).set_visible(cx, chat_open);
        self.visible_conversations = self.selected_profile.as_ref().map(|p| self.conversations.iter().filter(|c| c.target == p.user_id).map(|c| c.room.clone()).collect()).unwrap_or_default();
        self.view.view(cx, ids!(conversation_list_holder)).set_visible(cx, self.visible_conversations.len() > 1);
        self.view.check_box(cx, ids!(unread_only)).set_visible(cx, !self.adding_friend);
        let (page, title) = if self.groups_open {
            (id!(joined_groups), crate::i18n::tr("Group Chats"))
        } else if self.adding_friend {
            (id!(contacts), crate::i18n::tr("New Friends"))
        } else {
            match self.kind { 1 => (id!(discover), crate::i18n::tr("Discover")), 2 => (id!(account), crate::i18n::tr("Me")), _ => (id!(contacts), crate::i18n::tr("Contacts")) }
        };
        self.view.page_flip(cx, ids!(pages)).set_active_page(cx, page);
        self.view.label(cx, ids!(title_bar.title)).set_text(cx, title);
        self.view.button(cx, ids!(title_bar.controls.left)).set_visible(cx, self.adding_friend || self.groups_open);
        self.view.view(cx, ids!(shortcuts)).set_visible(cx, !self.adding_friend);
        self.view.button(cx, ids!(title_bar.controls.right)).set_visible(cx, self.kind == 0 && !self.groups_open && !self.adding_friend);
        if let Some(profile) = self.selected_profile.as_ref() {
            let widget = self.view.widget(cx, ids!(detail_pane.contact_profile));
            Self::populate_profile(cx, &widget, profile);
            widget.label(cx, ids!(contact_card.user_id)).set_text(cx, &crate::i18n::format("Matrix ID: {0}", &[("0", (profile.user_id).to_string())]));
            widget.label(cx, ids!(contact_block.title)).set_text(cx, if crate::sliding_sync::is_user_blocked(&profile.user_id) { crate::i18n::tr("Unblock") } else { crate::i18n::tr("Block") });
        } else if self.kind == 2 {
            if let Some(profile) = get_own_profile(cx) {
                let widget = self.view.widget(cx, ids!(own_profile));
                Self::populate_profile(cx, &widget, &profile);
            }
        }
        self.view.label(cx, ids!(status)).set_text(cx, &self.status);
        self.view.label(cx, ids!(status)).set_visible(cx, !self.status.is_empty());
        self.view.label(cx, ids!(group_status)).set_text(cx, &self.status);
        self.view.label(cx, ids!(group_status)).set_visible(cx, !self.status.is_empty());
        self.visible_contacts=contact_order(&self.contacts,&self.query,self.unread_only,|id|contact_unread(&self.conversations,id));
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if item.widget_uid() == self.view.portal_list(cx, ids!(conversation_list)).widget_uid() {
                if let Some(mut list) = item.borrow_mut::<PortalList>() {
                    list.set_item_range(cx,0,self.visible_conversations.len());
                    while let Some(index) = list.next_visible_item(cx) {
                        let Some(room) = self.visible_conversations.get(index) else {list.item(cx,index,id!(Filler)).draw_all(cx,scope);continue;};
                        let row = list.item(cx,index,id!(Conversation));
                        row.label(cx,ids!(name)).set_text(cx,&room.display());
                        row.label(cx,ids!(identity)).set_text(cx,room.room_id().as_str());
                        row.as_navigation_bar_button().set_selected(cx,self.selected_conversation.as_ref()==Some(room.room_id()));
                        let counts = get_client().and_then(|c|c.get_room(room.room_id())).map(|r|(r.num_unread_messages().max(r.num_unread_mentions()),r.is_marked_unread() || r.state()==matrix_sdk::RoomState::Invited)).unwrap_or_default();
                        row.unread_badge(cx,ids!(unread)).update_counts(counts.1,0,counts.0);
                        row.draw_all(cx,scope);
                    }
                }
                continue;
            }
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                if self.groups_open {
                    list.set_item_range(cx, 0, self.groups.len());
                    while let Some(index) = list.next_visible_item(cx) {
                        let Some(group) = self.groups.get_mut(index) else {
                            list.item(cx, index, id!(Filler)).draw_all(cx, scope);
                            continue;
                        };
                        let widget = list.item(cx, index, id!(Group));
                        widget.label(cx, ids!(name)).set_text(cx, &group.name.display());
                        let avatar = widget.avatar(cx, ids!(avatar));
                        let loaded = group.avatar.update_from_cache(cx).is_some_and(|image| {
                            avatar.show_image(cx, None, |cx, img| utils::load_avatar_image(&img, cx, image)).is_ok()
                        });
                        if !loaded { avatar.show_text(cx, None, None, group.name.display()); }
                        widget.draw_all(cx, scope);
                    }
                    continue;
                }
                list.set_item_range(cx, 0, self.visible_contacts.len());
                while let Some(index) = list.next_visible_item(cx) {
                    let Some(profile) = self.visible_contacts.get(index).and_then(|i| self.contacts.get(*i)) else {
                        list.item(cx, index, id!(Filler)).draw_all(cx, scope);
                        continue;
                    };
                    let widget = list.item(cx, index, id!(Contact));
                    let letter = profile.first_letter().to_uppercase();
                    let previous = index.checked_sub(1).and_then(|i| self.visible_contacts.get(i)).map(|i| self.contacts[*i].first_letter().to_uppercase());
                    widget.view(cx, ids!(section)).set_visible(cx, previous.as_ref() != Some(&letter));
                    widget.label(cx, ids!(letter)).set_text(cx, &letter);
                    let (unread,marked) = contact_unread(&self.conversations,&profile.user_id);
                    widget.unread_badge(cx,ids!(unread)).update_counts(marked,0,unread);
                    widget.view(cx, ids!(agent_badge)).set_visible(cx, self.contact_agents.contains(&profile.user_id));
                    widget.navigation_bar_button(cx, ids!(row)).set_selected(cx,
                        self.selected_profile.as_ref().is_some_and(|selected| selected.user_id == profile.user_id));
                    Self::populate_profile(cx, &widget, profile);
                    widget.draw_all(cx, scope);
                }
            }
        }
        DrawStep::done()
    }
}

impl MobileHubRef {
    pub(crate) fn suspend_chat(&self,cx:&mut Cx) {if let Some(mut hub)=self.borrow_mut() {hub.suspend_chat(cx);}}
}

#[cfg(test)]
mod new_chat_tests {
    use super::*;
    use makepad_widgets::makepad_draw::cx_draw::CxDraw;

    fn hub(cx: &mut Cx) -> MobileHub {
        cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.MobileHub {kind: 0}});
            let mut hub = MobileHub::script_from_value(vm, value);
            hub.owner_epoch=crate::account_session::epoch();
            assert!(vm.take_errors().is_empty());
            hub
        })
    }

    fn draw(cx: &mut Cx, widget: &mut impl Widget, width: f64) {
        cx.redraw_all();
        let pass = DrawPass::new(cx);
        pass.set_size(cx, dvec2(width, 760.0));
        let mut list = DrawList2d::new(cx);
        let overlay = Overlay {draw_list: DrawList::new(cx)};
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        list.begin_always(&mut cx2d);
        overlay.begin(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(width, 760.0), Layout::flow_overlay());
        widget.draw_walk_all(&mut cx2d, &mut Scope::with_data(&mut crate::app::AppState::default()),
            Walk {width: Size::Fixed(width), height: Size::Fixed(760.0), ..Walk::default()});
        cx2d.end_pass_sized_turtle();
        overlay.end(&mut cx2d);
        list.end(&mut cx2d);
        cx2d.end_pass(&pass);
    }

    fn profiles() -> Vec<UserProfile> {
        ["alice", "bob"].into_iter().map(|name| UserProfile {
            user_id: UserId::parse(format!("@{name}:example.org")).unwrap(),
            username: Some(name.into()), avatar_state: AvatarState::Unknown,
        }).collect()
    }

    #[test]
    fn unread_contacts_sort_first_and_unread_only_combines_with_search() {
        let contacts=profiles();
        let counts=|id:&UserId|if id.localpart()=="bob" {(4,false)}else{(0,false)};
        assert_eq!(contact_order(&contacts,"",false,counts),vec![1,0]);
        assert_eq!(contact_order(&contacts,"",true,counts),vec![1]);
        assert!(contact_order(&contacts,"alice",true,counts).is_empty());
        assert_eq!(contact_order(&contacts,"@bob",true,counts),vec![1]);
        assert_eq!(contact_order(&contacts,"",true,|_|(0,true)),vec![0,1]);
        // A direct invitation has no messages yet; its reminder must still surface the inviter.
        let invitation=|id:&UserId| (0,id.localpart()=="bob");
        assert_eq!(contact_order(&contacts,"",false,invitation),vec![1,0]);
        assert_eq!(contact_order(&contacts,"",true,invitation),vec![1]);
    }
    fn click_contact(cx: &mut Cx, hub: &mut MobileHub, index: usize) {
        let list = hub.view.portal_list(cx, ids!(list));
        let item = list.item(cx, index, id!(Contact));
        let row = item.navigation_bar_button(cx, ids!(row));
        let actions = cx.capture_actions(|cx| cx.group_widget_actions(list.widget_uid(), item.widget_uid(), |cx| {
            cx.widget_action(row.widget_uid(), crate::shared::navigation_bar_button::NavigationBarButtonAction::Clicked);
        }));
        hub.handle_event(cx, &Event::Actions(actions), &mut Scope::empty());
    }

    #[test]
    fn desktop_contact_click_keeps_list_and_updates_right_detail() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut hub = hub(&mut cx);
        hub.contacts = profiles();
        hub.contact_agents.insert(hub.contacts[0].user_id.clone());
        draw(&mut cx, &mut hub, 900.0);
        let title = hub.view.label(&cx, ids!(title_bar.title)).area().rect(&cx);
        let add = hub.view.button(&cx, ids!(title_bar.controls.right)).area().rect(&cx);
        assert!(title.size.y > 0.0 && title.size.y < 25.0, "Title uses its text height: {title:?}");
        assert!((title.pos.y + title.size.y * 0.5 - add.pos.y - add.size.y * 0.5).abs() < 1.0,
            "Title and add button share a center: {title:?} {add:?}");
        for index in [0, 1] {
            let row = hub.view.portal_list(&cx, ids!(list)).item(&mut cx, index, id!(Contact));
            assert_eq!(row.view(&cx, ids!(agent_badge)).visible(), index == 0);
        }
        assert!(hub.view.view(&cx, ids!(master_pane)).visible());
        assert!(hub.view.view(&cx, ids!(detail_pane)).visible());
        assert!(!hub.view.view(&cx, ids!(detail_pane.contact_profile)).visible());
        for index in [0, 1, 0] {
            click_contact(&mut cx, &mut hub, index);
            draw(&mut cx, &mut hub, 900.0);
            let master = hub.view.view(&cx, ids!(master_pane)).area().rect(&cx);
            let detail = hub.view.view(&cx, ids!(detail_pane.contact_profile)).area().rect(&cx);
            assert!((master.size.x - 300.0).abs() < 1.0, "{master:?}");
            assert!(detail.pos.x >= master.pos.x + master.size.x && detail.size.x > 590.0, "{detail:?}");
            assert!(hub.view.view(&cx, ids!(master_pane)).visible());
            assert_eq!(hub.selected_profile.as_ref().unwrap().user_id, hub.contacts[index].user_id);
            assert_eq!(hub.view.label(&cx, ids!(detail_pane.contact_profile.contact_card.name)).text(), hub.contacts[index].displayable_name());
            assert_eq!(hub.visible_contacts, vec![0, 1]);
        }
        hub.query = "bob".into();
        draw(&mut cx, &mut hub, 900.0);
        let recycled = hub.view.portal_list(&cx, ids!(list)).item(&mut cx, 0, id!(Contact));
        assert!(!recycled.view(&cx, ids!(agent_badge)).visible(), "Recycled ordinary rows lose the Agent badge");
        click_contact(&mut cx, &mut hub, 0);
        draw(&mut cx, &mut hub, 900.0);
        assert_eq!(hub.selected_profile.as_ref().unwrap().user_id, hub.contacts[1].user_id);
        assert_eq!(hub.query, "bob");
        hub.clear_account_state(&mut cx);
        draw(&mut cx, &mut hub, 900.0);
        assert!(!hub.view.view(&cx, ids!(detail_pane.contact_profile)).visible());
    }

    #[test]
    fn mobile_contact_detail_uses_full_width_and_back_restores_list() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut hub = hub(&mut cx);
        let mut home = cx.with_vm(|vm| {
            let value = script_eval!(vm, {mod.widgets.HomeScreen {}});
            super::super::home_screen::HomeScreen::script_from_value(vm, value)
        });
        draw(&mut cx, &mut home, 375.0);
        assert!(!super::super::home_screen::effective_is_desktop(&mut cx));
        hub.contacts = profiles();
        draw(&mut cx, &mut hub, 375.0);
        click_contact(&mut cx, &mut hub, 0);
        draw(&mut cx, &mut hub, 375.0);
        assert!(!hub.view.view(&cx, ids!(master_pane)).visible());
        let detail = hub.view.view(&cx, ids!(detail_pane.contact_profile)).area().rect(&cx);
        assert!((detail.size.x - 375.0).abs() < 1.0, "{detail:?}");
        let back = hub.view.button(&cx, ids!(detail_title_bar.controls.left));
        assert!(back.visible());
        let actions = cx.capture_actions(|cx| cx.widget_action(back.widget_uid(), ButtonAction::Clicked(KeyModifiers::default())));
        hub.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::empty());
        draw(&mut cx, &mut hub, 375.0);
        assert!(hub.selected_profile.is_none());
        assert!(hub.view.view(&cx, ids!(master_pane)).visible());
        assert!(!hub.view.view(&cx, ids!(detail_pane)).visible());
    }

    #[test]
    fn new_chat_opens_existing_contact_search_without_sending() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut hub = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.MobileHub {kind: 0}});
            MobileHub::script_from_value(vm, value)
        });
        hub.groups_open = true;
        hub.query = "old query".into();
        let request = hub.request;
        let actions = cx.capture_actions(|cx| cx.action(MobileNavigationAction::NewChatRequested));
        hub.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::empty());
        assert!(hub.adding_friend);
        assert!(!hub.groups_open);
        assert!(hub.query.is_empty());
        assert_eq!(hub.view.text_input(&cx, ids!(search)).text(), "");
        assert_ne!(hub.request, request, "Opening search invalidates old requests without sending a search or message");
    }
}
