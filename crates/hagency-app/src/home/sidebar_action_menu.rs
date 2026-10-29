//! Creation shortcuts anchored beside permanent navigation rows.
use makepad_widgets::*;
use crate::shared::context_menu::{ContextMenuClosed, expected_menu_size, menu_position_margin};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarMenuKind {
    Projects,
    GroupChats,
    Agents,
    Tools,
}
#[derive(Debug)]
pub enum SidebarMenuAction {
    Open {
        kind: SidebarMenuKind,
        pos: DVec2,
    },
    OpenProject {
        target: ProjectMenuTarget,
        pos: DVec2,
    },
    InviteProject {
        target: ProjectMenuTarget,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectMenuTarget {
    pub project_id: String,
    pub space_id: matrix_sdk::ruma::OwnedRoomId,
    pub service_identity: String,
    pub owner_mxid: String,
    pub homeserver: String,
    pub epoch: u64,
}
impl ProjectMenuTarget {
    pub(crate) fn is_current(&self) -> bool {
        if self.epoch != crate::account_session::epoch() || crate::sliding_sync::account_changing()
        {
            return false;
        }
        let Ok(service) = crate::hagency::backend::Service::for_current() else {
            return false;
        };
        if service.identity() != self.service_identity {
            return false;
        }
        let Some(client) = crate::sliding_sync::get_client() else {
            return false;
        };
        let valid = client.homeserver().as_str() == self.homeserver
            && client
                .user_id()
                .is_some_and(|id| id.as_str() == self.owner_mxid)
            && client.get_room(&self.space_id).is_some_and(|room| {
                room.state() == matrix_sdk::RoomState::Joined && room.is_space()
            });
        valid
            && self.epoch == crate::account_session::epoch()
            && !crate::sliding_sync::account_changing()
    }
}
#[derive(Debug)]
struct ProjectPermission {
    target: ProjectMenuTarget,
    request: u64,
    can_invite: bool,
    can_manage: bool,
    invite: bool,
}
fn permission_matches(
    target: &ProjectMenuTarget,
    request: u64,
    permission: &ProjectPermission,
) -> bool {
    permission.request == request && &permission.target == target
}
fn next_request() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
fn project_navigation(
    target: &ProjectMenuTarget,
    button: LiveId,
) -> Option<crate::hagency::ui::HagencyAction> {
    use crate::hagency::ui::{HagencyAction, ProjectDetailSection};
    let section = match button {
        id!(project_overview) => ProjectDetailSection::Overview,
        id!(project_access) => ProjectDetailSection::Access,
        id!(project_members) => ProjectDetailSection::Members,
        _ => return None,
    };
    Some(HagencyAction::ProjectSection {
        project_id: target.project_id.clone(),
        section,
    })
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    mod.widgets.SidebarActionMenu = set_type_default() do #(SidebarActionMenu::register_widget(vm)) {
        ..mod.widgets.View
        visible: false width: Fill height: Fill flow: Overlay
        // A hit-testing overlay, not a full-window paint surface. Do not use
        // SolidView/transparent color: only the anchored card submits pixels.
        show_bg: false padding: 0 margin: 0 spacing: 0
        cursor: MouseCursor.Default align: Align{x: 0 y: 0}
        main_content := mod.widgets.ContextMenuContent {
            width: 176 height: Fit flow: Down align: Align{x: 0 y: 0}
            show_bg: true
            draw_bg +: {color: mod.widgets.RINX_SURFACE border_size: 1 border_color: mod.widgets.RINX_BORDER}
            project_overview := mod.widgets.ContextMenuButton {visible: false icon_walk: Walk{width: 0 height: 0} spacing: 0 text: #(crate::i18n::tr("Overview")) i18n_text: "Overview"}
            project_members := mod.widgets.ContextMenuButton {visible: false icon_walk: Walk{width: 0 height: 0} spacing: 0 text: #(crate::i18n::tr("Project members")) i18n_text: "Project members"}
            project_invite := mod.widgets.ContextMenuButton {visible: false enabled: false icon_walk: Walk{width: 0 height: 0} spacing: 0 text: #(crate::i18n::tr("Invite members")) i18n_text: "Invite members"}
            project_access := mod.widgets.ContextMenuButton {visible: false icon_walk: Walk{width: 0 height: 0} spacing: 0 text: #(crate::i18n::tr("Access")) i18n_text: "Access"}
            new_project := mod.widgets.ContextMenuButton {icon_walk: Walk{width: 0 height: 0} spacing: 0 text: #(crate::i18n::tr("New project")) i18n_text: "New project"}
            new_group_chat := mod.widgets.ContextMenuButton {icon_walk: Walk{width: 0 height: 0} spacing: 0 text: #(crate::i18n::tr("New group chat")) i18n_text: "New group chat"}
            new_agent := mod.widgets.ContextMenuButton {icon_walk: Walk{width: 0 height: 0} spacing: 0 text: #(crate::i18n::tr("New agent")) i18n_text: "New agent"}
            open_discover := mod.widgets.ContextMenuButton {icon_walk: Walk{width: 0 height: 0} spacing: 0 text: #(crate::i18n::tr("Discover")) i18n_text: "Discover"}
            explore_rooms := mod.widgets.ContextMenuButton {icon_walk: Walk{width: 0 height: 0} spacing: 0 text: #(crate::i18n::tr("Explore rooms")) i18n_text: "Explore rooms"}
            open_moments := mod.widgets.ContextMenuButton {icon_walk: Walk{width: 0 height: 0} spacing: 0 text: #(crate::i18n::tr("Moments")) i18n_text: "Moments"}
            desktop_files := mod.widgets.ContextMenuButton {icon_walk: Walk{width: 0 height: 0} spacing: 0 text: #(crate::i18n::tr("File Transfer")) i18n_text: "File Transfer"}
            open_article := mod.widgets.ContextMenuButton {icon_walk: Walk{width: 0 height: 0} spacing: 0 text: #(crate::i18n::tr("Article editor")) i18n_text: "Article editor"}
            open_browser := mod.widgets.ContextMenuButton {icon_walk: Walk{width: 0 height: 0} spacing: 0 text: #(crate::i18n::tr("Browser")) i18n_text: "Browser"}
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct SidebarActionMenu {
    #[deref]
    view: View,
    #[rust]
    kind: Option<SidebarMenuKind>,
    #[rust]
    focus_after_draw: bool,
    #[rust]
    project: Option<ProjectMenuTarget>,
    #[rust]
    request: u64,
}
impl SidebarActionMenu {
    pub fn show(&mut self, cx: &mut Cx, kind: SidebarMenuKind, pos: DVec2, container: Rect) {
        self.project = None;
        self.request = next_request();
        self.project_buttons(cx, false);
        self.kind = Some(kind);
        for (button, target) in [
            (id!(new_project), SidebarMenuKind::Projects),
            (id!(new_group_chat), SidebarMenuKind::GroupChats),
            (id!(new_agent), SidebarMenuKind::Agents),
        ] {
            let button = self.view.button(cx, &[button]);
            button.set_visible(cx, kind == target);
            button.reset_hover(cx);
        }
        for id in [
            id!(open_discover),
            id!(explore_rooms),
            id!(open_moments),
            id!(desktop_files),
            id!(open_article),
            id!(open_browser),
        ] {
            let button = self.view.button(cx, &[id]);
            button.set_visible(cx, kind == SidebarMenuKind::Tools);
            button.reset_hover(cx);
        }
        let mut menu_size = expected_menu_size(if kind == SidebarMenuKind::Tools { 6 } else { 1 }, 0);
        menu_size.x = if kind == SidebarMenuKind::Tools {216.0} else {176.0};
        let margin = menu_position_margin(container, pos, menu_size);
        let mut content = self.view.view(cx, ids!(main_content));
        script_apply_eval!(cx, content, {width: #(menu_size.x) margin: #(margin)});
        self.visible = true;
        self.focus_after_draw = true;
        self.redraw(cx);
    }
    fn project_buttons(&self, cx: &mut Cx, visible: bool) {
        for id in [
            id!(project_overview),
            id!(project_members),
            id!(project_invite),
        ] {
            let button = self.view.button(cx, &[id]);
            button.set_visible(cx, visible);
            button.reset_hover(cx);
        }
        self.view.button(cx,ids!(project_access)).set_visible(cx,false);
        self.view
            .button(cx, ids!(project_invite))
            .set_enabled(cx, false);
        self.view.button(cx,ids!(project_invite)).set_visible(cx,false);
        self.view
            .button(cx, ids!(project_invite))
            .set_disabled(cx, true);
    }
    fn check_permission(&mut self, target: ProjectMenuTarget, invite: bool) {
        let Ok(service) = crate::hagency::backend::Service::for_current() else {
            return;
        };
        if service.identity() != target.service_identity || !target.is_current() {
            return;
        }
        self.request = next_request();
        let request = self.request;
        crate::sliding_sync::spawn_async_task(async move {
            let can_invite = service
                .room_members(target.space_id.to_string(), true)
                .await
                .is_ok_and(|value| value["canInvite"] == true);
            let can_manage=service.execute(crate::hagency::model::Command::ScopeServiceState {project_id:target.project_id.clone(),room_id:None}).await.is_ok_and(|v|v["canManagePolicy"]==true);
            Cx::post_action(ProjectPermission {
                can_manage,
                target,
                request,
                can_invite,
                invite,
            });
        });
    }
    pub fn show_project(
        &mut self,
        cx: &mut Cx,
        target: ProjectMenuTarget,
        pos: DVec2,
        container: Rect,
    ) {
        if !target.is_current() {
            self.close(cx);
            return;
        }
        self.show(cx, SidebarMenuKind::Projects, pos, container);
        self.view
            .button(cx, ids!(new_project))
            .set_visible(cx, false);
        self.project_buttons(cx, true);
        let menu_size = expected_menu_size(4, 0);
        let margin = menu_position_margin(container, pos, menu_size);
        let mut content = self.view.view(cx, ids!(main_content));
        script_apply_eval!(cx, content, {width: #(menu_size.x) margin: #(margin)});
        self.project = Some(target.clone());
        self.check_permission(target, false);
    }
    fn close(&mut self, cx: &mut Cx) {
        self.visible = false;
        self.kind = None;
        self.project = None;
        self.request = next_request();
        self.focus_after_draw = false;
        cx.revert_key_focus();
        cx.unblock_scrolling();
        cx.clear_all_hovers();
        cx.action(ContextMenuClosed);
        self.redraw(cx);
    }
}
impl SidebarActionMenuRef {
    pub fn show_project(
        &self,
        cx: &mut Cx,
        target: ProjectMenuTarget,
        pos: DVec2,
        container: Rect,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_project(cx, target, pos, container);
        }
    }

    pub fn show(&self, cx: &mut Cx, kind: SidebarMenuKind, pos: DVec2, container: Rect) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show(cx, kind, pos, container);
        }
    }
}
impl Widget for SidebarActionMenu {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.view.draw_walk(cx, scope, walk);
        if self.visible {
            let area = self.view.view(cx, ids!(main_content)).area();
            cx.block_scrolling_except_within(area);
            if step.is_done() && self.focus_after_draw {
                cx.set_key_focus(self.view.area());
                self.focus_after_draw = false;
            }
        }
        step
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible {
            return;
        }
        if self
            .project
            .as_ref()
            .is_some_and(|target| !target.is_current())
        {
            self.close(cx);
            return;
        }
        self.view.handle_event(cx, event, scope);
        let dismiss = event.back_pressed()
            || matches!(
                event,
                Event::KeyDown(KeyEvent {
                    key_code: KeyCode::Escape,
                    ..
                })
            )
            || matches!(event.hits_with_capture_overload(cx, self.view.area(), true), Hit::FingerUp(up) if up.is_over && !self.view.view(cx, ids!(main_content)).area().rect(cx).contains(up.abs));
        if dismiss {
            self.close(cx);
            return;
        }
        if let Event::Actions(actions) = event {
            if actions.iter().any(|action| {
                matches!(
                    action.downcast_ref(),
                    Some(crate::logout::logout_confirm_modal::LogoutAction::ClearAppState { .. })
                )
            }) {
                self.close(cx);
                return;
            }
            if let Some(target) = self.project.clone() {
                for action in actions {
                    if let Some(permission) = action.downcast_ref::<ProjectPermission>()
                        && permission_matches(&target, self.request, permission)
                        && target.is_current()
                    {
                        if permission.invite {
                            if permission.can_invite {
                                cx.action(SidebarMenuAction::InviteProject {
                                    target: target.clone(),
                                });
                            }
                            self.close(cx);
                            return;
                        }
                        self.view.button(cx,ids!(project_access)).set_visible(cx,permission.can_manage);
                        self.view.button(cx,ids!(project_invite)).set_visible(cx,permission.can_invite);
                        self.view
                            .button(cx, ids!(project_invite))
                            .set_enabled(cx, permission.can_invite);
                        self.view
                            .button(cx, ids!(project_invite))
                            .set_disabled(cx, !permission.can_invite);
                    }
                }
                let selected = [
                    id!(project_access),
                    id!(project_overview),
                    id!(project_members),
                    id!(project_invite),
                ]
                .into_iter()
                .find(|id| self.view.button(cx, &[*id]).clicked(actions));
                if let Some(button) = selected {
                    if !target.is_current() {
                        self.close(cx);
                        return;
                    }
                    if button == id!(project_invite) {
                        if self
                            .view
                            .button(cx, ids!(project_invite))
                            .borrow()
                            .is_some_and(|button| button.enabled())
                        {
                            self.view
                                .button(cx, ids!(project_invite))
                                .set_enabled(cx, false);
                            self.view
                                .button(cx, ids!(project_invite))
                                .set_disabled(cx, true);
                            self.check_permission(target, true);
                        }
                        return;
                    }
                    if let Some(action) = project_navigation(&target, button) {
                        cx.action(action);
                    }
                    self.close(cx);
                    return;
                }
                return;
            }

            if self.kind == Some(SidebarMenuKind::Tools) {
                let selected = [
                    id!(open_discover),
                    id!(explore_rooms),
                    id!(open_moments),
                    id!(desktop_files),
                    id!(open_article),
                    id!(open_browser),
                ]
                .into_iter()
                .find(|id| self.view.button(cx, &[*id]).clicked(actions));
                if let Some(id) = selected {
                    match id {
                        id!(open_discover) => {
                            cx.action(super::navigation_tab_bar::NavigationBarAction::GoToTab(
                                super::navigation_tab_bar::SelectedTab::Discover,
                            ))
                        }
                        id!(explore_rooms) => {
                            cx.action(super::navigation_tab_bar::NavigationBarAction::GoToAddRoom)
                        }
                        id!(open_moments) => {
                            cx.action(crate::moments::ui::MomentsAction::Open { author: None })
                        }
                        id!(desktop_files) => {
                            cx.action(crate::moments::ui::MomentsAction::FileTransfer)
                        }
                        id!(open_article) => crate::system_apps::open_article(cx),
                        id!(open_browser) => {
                            cx.action(crate::shared::web_browser::WebBrowserAction::Open(
                                "about:blank".parse().unwrap(),
                            ))
                        }
                        _ => {}
                    }
                    self.close(cx);
                    return;
                }
            }
            if actions.iter().any(|a| {
                matches!(
                    a.downcast_ref(),
                    Some(crate::logout::logout_confirm_modal::LogoutAction::ClearAppState { .. })
                )
            }) {
                self.close(cx);
                return;
            }
            let selected = match self.kind {
                Some(SidebarMenuKind::Projects) => {
                    self.view.button(cx, ids!(new_project)).clicked(actions)
                }
                Some(SidebarMenuKind::GroupChats) => {
                    self.view.button(cx, ids!(new_group_chat)).clicked(actions)
                }
                Some(SidebarMenuKind::Agents) => {
                    self.view.button(cx, ids!(new_agent)).clicked(actions)
                }
                Some(SidebarMenuKind::Tools) | None => false,
            };
            if selected {
                match self.kind {
                    Some(SidebarMenuKind::Projects) => {
                        cx.action(crate::hagency::ui::HagencyAction::NewProject)
                    }
                    Some(SidebarMenuKind::GroupChats) => {
                        cx.action(super::space_management::SpaceManagementAction::Open {parent:None})
                    }
                    Some(SidebarMenuKind::Agents) => {
                        cx.action(crate::hagency::ui::HagencyAction::NewAgent { project_id: None })
                    }
                    Some(SidebarMenuKind::Tools) | None => {}
                }
                self.close(cx);
            }
        }
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;

    fn target() -> ProjectMenuTarget {
        ProjectMenuTarget {
            project_id: "project-a".into(),
            space_id: matrix_sdk::ruma::room_id!("!space:server").to_owned(),
            service_identity: "old-owner/server/epoch".into(),
            owner_mxid: "@old:server".into(),
            homeserver: "https://server/".into(),
            epoch: 0,
        }
    }
    #[test]
    fn project_menu_permission_is_bound_to_identity_space_project_and_request() {
        let target = target();
        let permission = ProjectPermission {
            target: target.clone(),
            request: 5,
            can_invite: true, can_manage: false,
            invite: true,
        };
        assert!(permission_matches(&target, 5, &permission));
        assert!(!permission_matches(&target, 6, &permission));
        for changed in [
            ProjectMenuTarget {
                service_identity: "new-owner/server/epoch".into(),
                ..target.clone()
            },
            ProjectMenuTarget {
                space_id: matrix_sdk::ruma::room_id!("!other:server").to_owned(),
                ..target.clone()
            },
            ProjectMenuTarget {
                project_id: "project-b".into(),
                ..target.clone()
            },
        ] {
            assert!(!permission_matches(&changed, 5, &permission));
        }
        use crate::hagency::ui::{HagencyAction, ProjectDetailSection};
        for (button, section) in [
            (id!(project_overview), ProjectDetailSection::Overview),
            (id!(project_members), ProjectDetailSection::Members),
        ] {
            assert!(
                matches!(project_navigation(&target, button), Some(HagencyAction::ProjectSection {project_id, section: actual}) if project_id == target.project_id && actual == section)
            );
        }
        assert!(project_navigation(&target, id!(project_invite)).is_none());
        assert!(project_navigation(&target, id!(project_rooms)).is_none(), "removed Discussion Rooms cannot route even a stale click");
        assert!(project_navigation(&target, id!(project_space)).is_none());
    }
    #[test]
    fn registered_project_menu_hides_matrix_space_and_stale_targets_and_stale_targets_cannot_navigate_or_invite() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut menu = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.SidebarActionMenu {}});
            let menu = SidebarActionMenu::script_from_value(vm, value);
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "Project menu registration: {errors:?}");
            menu
        });
        let target = target();
        assert!(!target.is_current());
        let container = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(1280.0, 760.0),
        };
        assert!(menu.view.widget(&cx,ids!(project_rooms)).is_empty());
        for id in [
            id!(project_overview),
            id!(project_members),
            id!(project_invite),
        ] {
            menu.show(
                &mut cx,
                SidebarMenuKind::Projects,
                dvec2(200.0, 100.0),
                container,
            );
            menu.project_buttons(&mut cx, true);
            menu.project = Some(target.clone());
            assert_eq!(menu.view.button(&cx, &[id]).visible(),id!=id!(project_invite));
            assert!(
                !menu
                    .view
                    .button(&cx, ids!(project_invite))
                    .borrow()
                    .unwrap()
                    .enabled()
            );
            let uid = menu.view.button(&cx, &[id]).widget_uid();
            let actions = cx.capture_actions(|cx| {
                cx.widget_action(uid, ButtonAction::Clicked(KeyModifiers::default()));
                cx.action(ProjectPermission {
                    target: target.clone(),
                    request: menu.request,
                    can_invite: true, can_manage: false,
                    invite: true,
                });
            });
            let emitted = cx.capture_actions(|cx| {
                menu.handle_event(cx, &Event::Actions(actions), &mut Scope::empty())
            });
            assert!(!emitted.iter().any(|action| {
                action
                    .downcast_ref::<crate::hagency::ui::HagencyAction>()
                    .is_some()
                    || action
                        .downcast_ref::<super::super::invite_modal::InviteModalAction>()
                        .is_some()
                    || action
                        .downcast_ref::<crate::app::AppStateAction>()
                        .is_some()
            }));
            assert!(!menu.visible && menu.project.is_none());
        }
        menu.show_project(&mut cx, target, dvec2(200.0, 100.0), container);
        assert!(!menu.visible, "An expired target cannot even reopen a menu");
        menu.show(
            &mut cx,
            SidebarMenuKind::Projects,
            dvec2(200.0, 100.0),
            container,
        );
        for id in [
            id!(project_overview),
            id!(project_members),
            id!(project_invite),
        ] {
            assert!(
                !menu.view.button(&cx, &[id]).visible(),
                "Normal create menu cannot leak old Project actions"
            );
        }
    }

    #[test]
    fn popup_records_only_anchored_card_pixels_and_preserves_underlay() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (mut background, mut menu) = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let background = script_eval!(vm, {mod.widgets.SolidView {
                draw_bg.color: #1378b5
            }});
            let background = View::script_from_value(vm, background);
            let menu = script_eval!(vm, {mod.widgets.SidebarActionMenu {}});
            let menu = SidebarActionMenu::script_from_value(vm, menu);
            let errors = vm.take_errors();
            assert!(
                errors.is_empty(),
                "Render fixture script errors: {errors:?}"
            );
            (background, menu)
        });
        let anchor = dvec2(210.0, 100.0);
        let viewport = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(1280.0, 760.0),
        };
        menu.show(&mut cx, SidebarMenuKind::Projects, anchor, viewport);
        cx.redraw_all();
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, viewport.size);
        let mut scene = DrawList2d::new(&mut cx);
        let mut popup = DrawList2d::new(&mut cx);
        let event = DrawEvent::default();
        let popup_list;
        {
            let mut draw = CxDraw::new(&mut cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, None);
            scene.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(viewport.size, Layout::flow_overlay());
            let walk = Walk {
                width: Size::Fixed(1280.0),
                height: Size::Fixed(760.0),
                ..Walk::default()
            };
            background.draw_walk_all(&mut cx2d, &mut Scope::empty(), walk);
            popup.begin_always(&mut cx2d);
            popup_list = cx2d.get_current_draw_list_id().unwrap();
            menu.draw_walk_all(&mut cx2d, &mut Scope::empty(), walk);
            popup.end(&mut cx2d);
            cx2d.end_pass_sized_turtle();
            scene.end(&mut cx2d);
            cx2d.end_pass(&pass);
        }
        let card = menu.view.view(&cx, ids!(main_content)).area().rect(&cx);
        assert!(
            (card.pos.x - anchor.x).abs() < 1.0 && (card.pos.y - anchor.y).abs() < 1.0,
            "Card is anchored below its row: {card:?}"
        );
        assert!(
            (card.size.x - 176.0).abs() < 1.0 && card.size.y >= 40.0 && card.size.y <= 52.0,
            "Single-action card stays compact: {card:?}"
        );
        // Inspect the actual submitted draw-command instance buffers, rather
        // than just asserting show_bg/color properties or widget rectangles.
        let list = &cx.draw_lists[popup_list];
        let outside = dvec2(900.0, 600.0);
        let mut primitives = 0;
        for index in 0..list.draw_items.len() {
            let item = &list.draw_items[index];
            let Some(call) = item.draw_call() else {
                continue;
            };
            let Some(instances) = item.instances.as_ref() else {
                continue;
            };
            let mapping = &cx.draw_shaders.shaders[call.draw_shader_id.index].mapping;
            let (Some(pos), Some(size)) = (mapping.rect_pos, mapping.rect_size) else {
                continue;
            };
            for instance in instances.chunks_exact(mapping.instances.total_slots) {
                let rect = Rect {
                    pos: dvec2(instance[pos] as f64, instance[pos + 1] as f64),
                    size: dvec2(instance[size] as f64, instance[size + 1] as f64),
                };
                assert!(
                    !rect.contains(outside),
                    "Popup submits no full-screen scrim over the underlying page: {rect:?}"
                );
                assert!(
                    rect.pos.x >= card.pos.x - 1.0
                        && rect.pos.y >= card.pos.y - 1.0
                        && rect.pos.x + rect.size.x <= card.pos.x + card.size.x + 1.0
                        && rect.pos.y + rect.size.y <= card.pos.y + card.size.y + 1.0,
                    "All popup drawing stays inside its card: {rect:?}, {card:?}"
                );
                primitives += 1;
            }
        }
        assert!(primitives > 0, "The card actually records visible drawing");
        let Area::Instance(underlay) = background.area() else {
            panic!("Underlying page must record a painted instance");
        };
        let item = &cx.draw_lists[underlay.draw_list_id].draw_items[underlay.draw_item_id];
        assert!(
            item.instances.as_ref().is_some_and(|data| !data.is_empty()),
            "Underlying page draw data remains recorded"
        );
        assert!(background.area().rect(&cx).contains(outside));
    }
}
