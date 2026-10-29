//! The RoomsSideBar is the widget that contains the RoomsList and other items.
//!
//! It differs in what content it includes based on the adaptive view:
//! * On a narrow mobile view, it acts as the root_view of StackNavigation
//!   * It includes a title label, a search bar, and the RoomsList.
//! * On desktop, the permanent global sidebar contains workspace navigation
//!   and the current account menu. Contextual chat/project lists live beside it.
//!   A zero-height RoomsList keeps the existing SDK metadata consumer alive.

use makepad_widgets::*;

use crate::home::rooms_list::RoomsListWidgetExt;
use crate::home::project_tree::ProjectTreeWidgetExt;
use crate::home::rooms_list_header::RoomsListHeaderAction;
use crate::home::joined_spaces::JoinedSpacesWidgetExt;
use crate::home::navigation_tab_bar::{NavigationBarAction, SelectedTab};
use crate::shared::navigation_bar_button::NavigationBarButtonWidgetExt;
use crate::settings::app_preferences::{AppPreferencesGlobal, AppPreferencesAction, ViewModeOverride};
use crate::shared::room_filter_input_bar::{MainFilterAction, RoomFilterInputBarWidgetExt};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let ChatSwitchButton = NavigationBarButton {
        width: Fill height: 32 align: Align{x: 0.0 y: 0.5} padding: Inset{left: 12 right: 8}
        draw_bg +: {color_hover: mod.widgets.RINX_HOVER color_active: mod.widgets.RINX_SELECTED border_radius: 5}
        label := RinxLabel {
            draw_text +: {color: mod.widgets.RINX_INK text_style: theme.font_regular {font_size: mod.widgets.RINX_UI_SIZE}}
        }
    }
    let RowActions = RinxMoreButton {visible:false}
    let DesktopNavButton = NavigationBarButton {
        width: Fill height: 40 flow: Right spacing: 10
        align: Align{x: 0 y: 0.5} padding: Inset{left: 12 right: 8}
        draw_bg +: {color_hover: mod.widgets.RINX_HOVER color_active: mod.widgets.RINX_NAV_SELECTED border_radius: 5}
        nav_icon := Icon {icon_walk: Walk{width: 18 height: 18} draw_icon.color: mod.widgets.RINX_INK}
        label := RinxLabel {width: Fill max_lines: 1 text_overflow: Ellipsis
            draw_text +: {color: mod.widgets.RINX_INK text_style: theme.font_regular{font_size: mod.widgets.RINX_UI_SIZE}}
        }
    }
    let ChatViewSwitch = SolidView {
        width: Fill height: 46 flow: Right
        padding: Inset{left: 12 right: 12 bottom: 10 top: 2}
        draw_bg.color: MOBILE_BG
        RoundedView {
            width: Fill height: Fill flow: Right padding: 2
            draw_bg +: {color: mod.widgets.RINX_FIELD border_radius: 6}
            all_chats := ChatSwitchButton {label.text: #(crate::i18n::tr("All Chats")) label.i18n_text: "All Chats"}
            joined_spaces_tab := ChatSwitchButton {label.text: #(crate::i18n::tr("Spaces")) label.i18n_text: "Spaces"}
        }
    }

    mod.widgets.RoomsSideBar = #(RoomsSideBar::register_widget(vm)) {
        Desktop := SolidView {
            padding: Inset{top: 12, bottom: 12}
            flow: Down, spacing: 5
            width: Fill, height: Fill

            draw_bg.color: mod.widgets.RINX_RAIL_BG

            sidebar_heading := View {width: Fill height: 56 flow: Right align: Align{y: 0.5} margin: Inset{left: 10 right: 10}
                sidebar_brand := View {
                    width: Fill height: Fill flow: Right align: Align{x: 0.5 y: 0.5}
                    padding: Inset{left: 12}
                    sidebar_logo := RinxBrandLogo {}
                    sidebar_title := RinxLabel {text: "Hagency" width: Fill height: Fit margin: Inset{left: 8} max_lines: 1 text_overflow: Ellipsis draw_text +: {color: mod.widgets.RINX_INK text_style: theme.font_regular {font_size: mod.widgets.RINX_BRAND_SIZE}}}
                }
                toggle_sidebar := NavigationBarButton {width: 32 height: 32
                    tooltip_text: #(crate::i18n::tr("Collapse sidebar"))
                    fold_icon := Icon {icon_walk: Walk{width: 18 height: 18} draw_icon +: {svg: ICON_CHEVRON_LEFT color: mod.widgets.RINX_INK}}
                }
            }
            primary_navigation := ScrollYView {width: Fill height: 224 flow: Down spacing: 6 padding: Inset{left: 10 right: 10}
            projects_row := View {show_bg:true draw_bg.color:#0000 width: Fill height: 40 flow: Right align: Align{y: 0.5}
                open_projects := DesktopNavButton {nav_icon.draw_icon.svg: ICON_HIERARCHY tooltip_text: #(crate::i18n::tr("Projects")) label.text: #(crate::i18n::tr("Projects")) label.i18n_text: "Projects"}
                projects_action_slot := View {width:32 height:32 projects_actions := RowActions {}}
            }
            agents_row := View {show_bg:true draw_bg.color:#0000 width: Fill height: 40 flow: Right align: Align{y: 0.5}
                open_agents := DesktopNavButton {nav_icon.draw_icon.svg: ICON_ROBOT tooltip_text: #(crate::i18n::tr("Agents")) label.text: #(crate::i18n::tr("Agents")) label.i18n_text: "Agents"}
                agents_action_slot := View {width:32 height:32 agents_actions := RowActions {}}
            }
            group_chats_row := View {show_bg:true draw_bg.color:#0000 width: Fill height: 40 flow: Right align: Align{y: 0.5}
                open_group_chats := DesktopNavButton {nav_icon.draw_icon.svg: ICON_THREADS tooltip_text: #(crate::i18n::tr("Group Chats")) label.text: #(crate::i18n::tr("Group Chats")) label.i18n_text: "Group Chats"}
                group_chats_action_slot := View {width:32 height:32 group_chats_actions := RowActions {}}
            }
            open_contacts := DesktopNavButton {unread := UnreadBadge {} nav_icon.draw_icon.svg: ICON_PEOPLE tooltip_text: #(crate::i18n::tr("Contacts")) label.text: #(crate::i18n::tr("Contacts")) label.i18n_text: "Contacts"}
            open_miniapps := DesktopNavButton {nav_icon.draw_icon.svg: ICON_SQUARES tooltip_text: #(crate::i18n::tr("Mini apps")) label.text: #(crate::i18n::tr("Mini apps")) label.i18n_text: "Mini apps"}
            }
            more := DesktopNavButton {margin: Inset{left: 10 right: 10} nav_icon.draw_icon.svg: ICON_MORE_VERT tooltip_text: #(crate::i18n::tr("More")) label.text: #(crate::i18n::tr("More")) label.i18n_text: "More"}
            // CachedWidget eagerly instantiates at on_after_apply. Keep its
            // SDK consumer drawable at zero height before the tree, too.
            conversations := View {
                width: Fill height: 0 visible: true
                CachedWidget {rooms_list := RoomsList {}}
            }
            View {width: Fill height: Fill}
            open_settings := DesktopNavButton {margin: Inset{left: 10 right: 10} nav_icon.draw_icon.svg: ICON_SETTINGS tooltip_text: #(crate::i18n::tr("Settings")) label.text: #(crate::i18n::tr("Settings")) label.i18n_text: "Settings"}
            HagencyAccountBar {margin: Inset{left: 10 right: 10}}
        },

        Mobile := SolidView {
            width: Fill height: Fill flow: Down
            draw_bg.color: mod.widgets.RINX_SURFACE
            title_bar := MobileTitle {
                title.text: #(crate::i18n::tr("Group Chats")) title.i18n_text: "Group Chats"
                controls.right.visible: true
            }
            SolidView {
                width: Fill height: 48
                padding: Inset{left: 10 right: 10 bottom: 10}
                draw_bg.color: MOBILE_BG
                room_filter_input_bar := RoomFilterInputBar {
                    height: 36
                    draw_bg +: {color: mod.widgets.RINX_SURFACE border_size: 0 border_radius: 5}
                    input +: {
                        empty_text: #(crate::i18n::tr("Search")) i18n_empty_text: "Search"
                        draw_text.text_style.font_size: mod.widgets.RINX_UI_SIZE
                        draw_bg +: {color: mod.widgets.RINX_SURFACE color_hover: mod.widgets.RINX_SURFACE color_focus: mod.widgets.RINX_SURFACE color_empty: mod.widgets.RINX_SURFACE}
                    }
                }
            }
            ChatViewSwitch {}
            file_transfer_entry := MobileRow {height: 46 title.text: #(crate::i18n::tr("File Transfer")) title.i18n_text: "File Transfer" icon.draw_icon.svg: ICON_FILE}
            conversations := View {
                width: Fill height: Fill flow: Down
                View {width: Fill height: 0 CachedWidget {rooms_list := RoomsList {}}}
                mobile_groups := ProjectTree {}
            }
            spaces_tree := View {
                width: Fill height: Fill visible: false
                CachedWidget {joined_spaces := JoinedSpaces {}}
            }
        }
    }
}

/// Global navigation selection survives adaptive variants and room focus changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DesktopWorkspace { #[default] Projects, GroupChats, Other }

#[derive(Debug)]
pub struct DesktopRailAction;

/// Hosted content keeps its own toggle when the host has no Hagency titlebar.
#[derive(Default)]
pub struct SidebarToggleInTitlebar(pub bool);

/// Shared across adaptive sidebar replacements, but reset when signing out.
#[derive(Default)]
struct GroupChatsViewState {
    spaces: bool,
}

/// A simple wrapper around `AdaptiveView` that contains several global singleton widgets.
///
/// * In the mobile view, it serves as the root view of the StackNavigation,
///   showing the title label, the search bar, and the RoomsList.
/// * In the desktop view, it is the permanent Project workspace sidebar.
#[derive(Script, Widget)]
pub struct RoomsSideBar {
    #[deref]
    view: AdaptiveView,

    /// The most recently applied view-mode override.
    #[rust]
    applied_view_mode: ViewModeOverride,
    #[rust] selected_primary: Option<LiveId>,
    #[rust] painted_collapsed: Option<bool>,
    #[rust] direct_counts: (u64,bool),
    #[rust] pointer:Option<Vec2d>,
}

impl ScriptHook for RoomsSideBar {
    fn on_after_apply(&mut self, _vm: &mut ScriptVm, _: &Apply, _: &mut Scope, _: ScriptValue) {
        self.painted_collapsed = None;
    }
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            // Here we set the global singleton for the RoomsList widget,
            // which is used to access the list of rooms from anywhere in the app.
            cx.set_global(self.view.rooms_list(cx, ids!(rooms_list)));

            // The RoomsSideBar is re-instantiated every time the HomeScreen's
            // AdaptiveView switches between Desktop and Mobile view modes
            // (cuz it's not wrapped in a CachedWidget).
            // Thus we just re-read the current value here and apply it.
            let mode = cx.global::<AppPreferencesGlobal>().0.view_mode;
            self.apply_view_mode(mode);
        });
    }
}

impl RoomsSideBar {
    fn apply_view_mode(&mut self, mode: ViewModeOverride) {
        // The sidebar is only ~280 px wide even in a desktop window. Its
        // variant must follow the enclosing HomeScreen, not its own width.
        self.view.set_variant_selector(|cx, _| {
            if crate::home::home_screen::effective_is_desktop(cx) {
                id!(Desktop)
            } else {
                id!(Mobile)
            }
        });
        self.applied_view_mode = mode;
    }
}

impl Widget for RoomsSideBar {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event {Event::MouseMove(e)=>self.pointer=Some(e.abs),Event::MouseDown(e)=>self.pointer=Some(e.abs),Event::WindowLostFocus(_)|Event::MouseLeave(_)=>self.pointer=None,_=>()}
        for (row,action) in [(ids!(projects_row).as_slice(),ids!(projects_actions).as_slice()),(ids!(agents_row).as_slice(),ids!(agents_actions).as_slice()),(ids!(group_chats_row).as_slice(),ids!(group_chats_actions).as_slice())] {
            let show=self.painted_collapsed!=Some(true) && self.pointer.is_some_and(|p|self.view.view(cx,row).area().rect(cx).contains(p));
            let button=self.view.button(cx,action);if button.visible()!=show {button.set_visible(cx,show);self.view.redraw(cx);}
        }
        if matches!(event,Event::Signal) {let counts=super::mobile::direct_unread(cx);if counts!=self.direct_counts {self.direct_counts=counts;self.view.redraw(cx);}}

        // If the main room filter input bar changed keywords, re-emit that action
        // as a MainFilterAction so that other widgets can handle it.
        if let Event::Actions(actions) = event {
            if self.view.navigation_bar_button(cx, ids!(toggle_sidebar)).clicked(actions) {
                cx.action(DesktopRailAction);
            }
            for (button, kind) in [(id!(projects_actions), super::sidebar_action_menu::SidebarMenuKind::Projects), (id!(group_chats_actions), super::sidebar_action_menu::SidebarMenuKind::GroupChats), (id!(agents_actions), super::sidebar_action_menu::SidebarMenuKind::Agents)] {
                if self.view.button(cx, &[button]).clicked(actions) {
                    let rect = self.view.widget(cx, &[button]).area().rect(cx);
                    cx.action(super::sidebar_action_menu::SidebarMenuAction::Open {kind, pos: dvec2(rect.pos.x, rect.pos.y + rect.size.y)});
                }
            }
            if self.view.navigation_bar_button(cx, ids!(open_contacts)).clicked(actions) {
                cx.action(NavigationBarAction::GoToTab(SelectedTab::Contacts));
            }
            if self.view.navigation_bar_button(cx, ids!(open_settings)).clicked(actions) {
                cx.action(NavigationBarAction::OpenSettings);
            }
            if self.view.navigation_bar_button(cx, ids!(open_group_chats)).clicked(actions) {
                cx.action(NavigationBarAction::GoToHome);
                cx.action(crate::home::main_desktop_ui::MainDesktopUiAction::ShowGroupChats);
            }
            if self.view.navigation_bar_button(cx, ids!(more)).clicked(actions) {
                let rect = self.view.widget(cx, ids!(more)).area().rect(cx);
                cx.action(super::sidebar_action_menu::SidebarMenuAction::Open {kind: super::sidebar_action_menu::SidebarMenuKind::Tools, pos: dvec2(rect.pos.x, rect.pos.y + rect.size.y)});
            }
            if self.view.navigation_bar_button(cx, ids!(open_miniapps)).clicked(actions) {cx.action(crate::miniapps::MiniAppsAction::Open);}
            if self
                .view
                .navigation_bar_button(cx, ids!(open_projects))
                .clicked(actions)
            {
                cx.action(crate::hagency::ui::HagencyAction::OpenProjects);
            }
            if self
                .view
                .navigation_bar_button(cx, ids!(open_agents))
                .clicked(actions)
            {
                cx.action(crate::hagency::ui::HagencyAction::OpenAgents);
            }
            if self
                .view
                .navigation_bar_button(cx, ids!(file_transfer_entry))
                .clicked(actions)
            {
                cx.action(crate::moments::ui::MomentsAction::FileTransfer);
            }
            if self
                .view
                .navigation_bar_button(cx, ids!(all_chats))
                .clicked(actions)
            {
                cx.global::<GroupChatsViewState>().spaces = false;
                cx.action(NavigationBarAction::GoToHome);
                self.view.redraw(cx);
            }
            if self
                .view
                .navigation_bar_button(cx, ids!(joined_spaces_tab))
                .clicked(actions)
            {
                cx.global::<GroupChatsViewState>().spaces = true;
                cx.action(NavigationBarAction::GoToHome);
                self.view.redraw(cx);
            }
            if self
                .view
                .button(cx, ids!(title_bar.controls.right))
                .clicked(actions)
            {
                cx.action(
                    crate::home::navigation_tab_bar::NavigationBarAction::GoToTab(
                        crate::home::navigation_tab_bar::SelectedTab::Contacts,
                    ),
                );
            }
            if let Some(keywords) = self
                .view
                .room_filter_input_bar(cx, ids!(room_filter_input_bar))
                .changed(actions)
            {
                cx.action(MainFilterAction::Changed(keywords));
            }

            for action in actions {
                if let Some(action) = action.downcast_ref::<crate::hagency::ui::HagencyAction>() {
                    self.selected_primary = Some(match action {crate::hagency::ui::HagencyAction::OpenAgents | crate::hagency::ui::HagencyAction::NewAgent {..} | crate::hagency::ui::HagencyAction::AgentRun {..} => id!(open_agents), crate::hagency::ui::HagencyAction::Close => id!(open_group_chats), _ => id!(open_projects)});
                }
                if let Some(navigation) = action.downcast_ref::<crate::hagency::ui::ProjectRoomNavigation>()
                    && navigation.current() {
                    self.selected_primary = Some(id!(open_projects));
                }
                if matches!(action.downcast_ref::<crate::home::main_desktop_ui::MainDesktopUiAction>(), Some(crate::home::main_desktop_ui::MainDesktopUiAction::ShowGroupChats)) {
                    self.selected_primary = Some(id!(open_group_chats));
                }
                // Mini apps are hosted separately; opening or closing one
                // must preserve the underlying Project or Chat selection.
                if action.downcast_ref::<crate::home::home_screen::MainViewVariantChangedAction>().is_some() {
                    self.apply_view_mode(self.applied_view_mode);
                    self.view.redraw(cx);
                }
                if let Some(crate::logout::logout_confirm_modal::LogoutAction::ClearAppState {
                    ..
                }) = action.downcast_ref()
                {
                    cx.global::<GroupChatsViewState>().spaces = false;
                        self.selected_primary = None;
                }
                // A legacy desktop Space shortcut still opens its filtered list.
                if let Some(NavigationBarAction::TabSelected(SelectedTab::Space { .. })) =
                    action.downcast_ref()
                {
                    cx.global::<GroupChatsViewState>().spaces = false;
                }
                // Focus the filter hosted by the currently selected sidebar variant.
                if let Some(RoomsListHeaderAction::OpenRoomFilterModal) = action.downcast_ref() {
                    let input = self.view.text_input(cx, ids!(room_filter_input_bar.input));
                    if !input.is_empty() {
                        input.set_key_focus(cx);
                    }
                }
                if let Some(AppPreferencesAction::ViewModeChanged(new_mode)) = action.downcast_ref()
                {
                    if *new_mode != self.applied_view_mode {
                        self.apply_view_mode(*new_mode);
                        self.view.redraw(cx);
                    }
                }
            }
        }
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Keep primary navigation bounded on short windows; creation and More
        // menus use the app overlay and never consume tree/footer space.
        let height = cx.peek_walk_turtle(walk).size.y;
        let collapsed = crate::home::home_screen::effective_is_desktop(cx)
            && cx.global::<AppPreferencesGlobal>().0.desktop_sidebar_collapsed;
        self.view.label(cx, ids!(sidebar_title)).set_visible(cx, !collapsed);
        let fold = self.view.navigation_bar_button(cx, ids!(toggle_sidebar));
        let in_titlebar = cx.global::<SidebarToggleInTitlebar>().0;
        let brand = self.view.view(cx, ids!(sidebar_brand));
        brand.set_visible(cx, !collapsed || in_titlebar);
        if let Some(mut brand) = brand.borrow_mut() {
            brand.layout.padding.left = if collapsed {0.0} else {12.0};
        }
        if let Some(mut button) = fold.borrow_mut() {
            button.set_visible(cx, !in_titlebar);
            button.set_tooltip_text(crate::i18n::tr(if collapsed {"Expand sidebar"} else {"Collapse sidebar"}));
            button.walk.width = Size::Fixed(if collapsed {44.0} else {32.0});
        }
        if self.painted_collapsed != Some(collapsed) {
            self.painted_collapsed = Some(collapsed);
            let mut icon = self.view.icon(cx, ids!(fold_icon));
            if collapsed {script_apply_eval!(cx, icon, {draw_icon.svg: mod.widgets.ICON_CHEVRON_RIGHT});}
            else {script_apply_eval!(cx, icon, {draw_icon.svg: mod.widgets.ICON_CHEVRON_LEFT});}
        }
        for id in [id!(open_projects),id!(open_agents),id!(open_group_chats),id!(open_contacts),id!(open_miniapps),id!(more),id!(open_settings)] {
            self.view.navigation_bar_button(cx, &[id]).label(cx, ids!(label)).set_visible(cx, !collapsed);
        }
        // Hit testing belongs to events: an Area from the preceding draw can
        // become invalid while rebuilding its parent draw list.
        if collapsed {
            for action in [ids!(projects_actions).as_slice(),ids!(agents_actions).as_slice(),ids!(group_chats_actions).as_slice()] {
                self.view.button(cx,action).set_visible(cx,false);
            }
        }
        for slot in [ids!(projects_action_slot).as_slice(),ids!(agents_action_slot).as_slice(),ids!(group_chats_action_slot).as_slice()] {self.view.view(cx,slot).set_visible(cx,!collapsed);}
        // Show every primary entry on a normal desktop window, reserving
        // space for the tree and footer before scrolling on short windows.
        let navigation_height = (height - 260.0).clamp(100.0, 224.0);
        let navigation = self.view.view(cx, ids!(primary_navigation));
        if let Some(mut view) = navigation.borrow_mut() {view.walk.height = Size::Fixed(navigation_height); }
        if let Some(state) = scope.data.get::<crate::app::AppState>() {
            // Restored Room/context navigation may not emit a primary-button click.
            // Use the same workspace as HomeScreen's context column.
            if state.selected_tab == SelectedTab::Home {
                match *cx.global::<DesktopWorkspace>() {
                    DesktopWorkspace::Projects => self.selected_primary=Some(id!(open_projects)),
                    DesktopWorkspace::GroupChats => self.selected_primary=Some(id!(open_group_chats)),
                    DesktopWorkspace::Other => {},
                }
            }
            for button in [id!(open_projects), id!(open_agents), id!(open_group_chats), id!(open_miniapps)] {
                self.view.navigation_bar_button(cx, &[button]).set_selected(cx, state.selected_tab == SelectedTab::Home && self.selected_primary.unwrap_or(id!(open_projects)) == button);
            }
            for (button, selected) in [(ids!(open_contacts), state.selected_tab == SelectedTab::Contacts), (ids!(open_discover), state.selected_tab == SelectedTab::Discover), (ids!(open_settings), state.selected_tab == SelectedTab::Settings)] {
                self.view.navigation_bar_button(cx, button).set_selected(cx, selected);
            }
        }
        let (unread,marked)=super::mobile::direct_unread(cx);
        use crate::shared::unread_badge::UnreadBadgeWidgetExt;
        self.view.unread_badge(cx,ids!(open_contacts.unread)).update_counts(marked,0,unread);
        // The desktop Project tree replaces the mobile Space list.
        let spaces = cx.global::<GroupChatsViewState>().spaces
            && !crate::home::home_screen::effective_is_desktop(cx);
        let desktop = crate::home::home_screen::effective_is_desktop(cx);
        self.view
            .view(cx, ids!(conversations))
            .set_visible(cx, desktop || !spaces);
        self.view
            .view(cx, ids!(spaces_tree))
            .set_visible(cx, spaces);
        self.view
            .navigation_bar_button(cx, ids!(all_chats))
            .set_selected(cx, !spaces);
        self.view
            .navigation_bar_button(cx, ids!(joined_spaces_tab))
            .set_selected(cx, spaces);
        self.view.joined_spaces(cx, ids!(joined_spaces)).set_active(cx, spaces);
        let groups=self.view.project_tree(cx,ids!(mobile_groups));
        groups.set_mode(cx,crate::home::project_tree::ProjectTreeMode::GroupChats);
        groups.set_active(cx,!desktop && !spaces);
        self.view.draw_walk(cx, scope, walk)
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use crate::shared::avatar::AvatarWidgetExt;
    use crate::{app::AppState, home::home_screen::HomeScreen};
    use makepad_widgets::makepad_draw::cx_draw::CxDraw;

    fn draw(cx: &mut Cx, widget: &mut impl Widget, state: &mut AppState, width: f64) {
        draw_sized(cx, widget, state, width, 760.0);
    }
    fn draw_sized(cx: &mut Cx, widget: &mut impl Widget, state: &mut AppState, width: f64, height: f64) {
        cx.redraw_all();
        let pass = DrawPass::new(cx);
        pass.set_size(cx, dvec2(1280.0, 900.0));
        let mut list = DrawList2d::new(cx);
        let overlay = Overlay {draw_list: DrawList::new(cx)};
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        list.begin_always(&mut cx2d);
        overlay.begin(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(1280.0, 900.0), Layout::flow_overlay());
        widget.draw_walk_all(&mut cx2d, &mut Scope::with_data(state), Walk {
            width: Size::Fixed(width), height: Size::Fixed(height), ..Walk::default()
        });
        overlay.end(&mut cx2d);
        cx2d.end_pass_sized_turtle();
        list.end(&mut cx2d);
        cx2d.end_pass(&pass);
    }

    fn collect_sidebars(children: Vec<WidgetRef>, result: &mut Vec<WidgetRef>) {
        collect_widgets::<RoomsSideBar>(children, result);
    }
    fn collect_widgets<T: Widget + 'static>(children: Vec<WidgetRef>, result: &mut Vec<WidgetRef>) {
        for child in children {
            if child.borrow::<T>().is_some() {
                result.push(child);
            } else {
                let mut nested = Vec::new();
                child.children(&mut |_, child| nested.push(child));
                collect_widgets::<T>(nested, result);
            }
        }
    }

    #[test]
    fn unchanged_navigation_selection_does_not_apply_or_schedule_another_frame() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut button=cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);
            let value=script_eval!(vm,{mod.widgets.NavigationBarButton {width:80 height:32}});
            crate::shared::navigation_bar_button::NavigationBarButton::script_from_value(vm,value)
        });
        draw_sized(&mut cx,&mut button,&mut AppState::default(),80.0,32.0);
        for selected in [true,false] {
            button.set_selected(&mut cx,selected);
            cx.new_draw_event=DrawEvent::default();
            button.set_selected(&mut cx,selected);
            assert!(!cx.new_draw_event.redraw_all && cx.new_draw_event.draw_lists.is_empty());
            assert!(button.animator_in_state(&cx,if selected {ids!(active.on)} else {ids!(active.off)}));
        }
    }

    #[test]
    fn registered_navigation_focus_is_visible_and_does_not_change_selection() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut button=cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);vm.bx.captured_errors=Some(Vec::new());
            let value=script_eval!(vm,{mod.widgets.NavigationBarButton {width: 80 height: 32}});
            let button=crate::shared::navigation_bar_button::NavigationBarButton::script_from_value(vm,value);
            assert!(vm.take_errors().is_empty());button
        });
        draw_sized(&mut cx,&mut button,&mut AppState::default(),80.0,32.0);
        let area=button.area();
        button.handle_event(&mut cx,&Event::KeyFocus(KeyFocusEvent{prev:Area::Empty,focus:area}),&mut Scope::empty());
        assert!(button.animator_in_state(&cx,ids!(focus.on)));
        assert!(!button.animator_in_state(&cx,ids!(active.on)),"focus is distinct from selected navigation");
        button.handle_event(&mut cx,&Event::KeyFocus(KeyFocusEvent{prev:area,focus:Area::Empty}),&mut Scope::empty());
        assert!(button.animator_in_state(&cx,ids!(focus.off)));
    }
    #[test]
    fn narrow_sidebar_follows_main_layout_and_preserves_cached_rooms_consumer() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.display_context.screen_size = dvec2(1280.0, 900.0);
        let mut home = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.HomeScreen {}});
            let home = HomeScreen::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty());
            home
        });
        let mut state = AppState::default();
        let mut consumer_uid = None;
        for (mode, width, desktop) in [
            (ViewModeOverride::Automatic, 1280.0, true),
            (ViewModeOverride::Automatic, 375.0, false),
            (ViewModeOverride::ForceWide, 375.0, true),
            (ViewModeOverride::ForceNarrow, 1280.0, false),
            (ViewModeOverride::Automatic, 1280.0, true),
        ] {
            state.app_prefs.view_mode = mode;
            let actions = cx.capture_actions(|cx| state.app_prefs.on_view_mode_changed(cx));
            home.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::with_data(&mut state));
            let actions = cx.capture_actions(|cx| draw(cx, &mut home, &mut state, width));
            // A main layout change must reevaluate even if sidebar width stays 280.
            home.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::with_data(&mut state));
            draw(&mut cx, &mut home, &mut state, width);
            let mut found = Vec::new();
            let mut children = Vec::new();
            home.children(&mut |_, child| children.push(child));
            collect_sidebars(children, &mut found);
            assert_eq!(found.len(), 1, "Exactly one sidebar must be active");
            let sidebar = found[0].borrow::<RoomsSideBar>().unwrap();
            let consumer = cx.global::<crate::home::rooms_list::RoomsListRef>().clone();
            assert!(!consumer.is_empty());
            assert_eq!(*consumer_uid.get_or_insert(consumer.widget_uid()), consumer.widget_uid());
            assert_eq!(sidebar.view.active_variant(), Some(if desktop {id!(Desktop)} else {id!(Mobile)}));
            let list = sidebar.view.rooms_list(&cx, ids!(rooms_list));
            assert_eq!(list.widget_uid(), consumer.widget_uid());
            assert_eq!(cx.global::<crate::home::rooms_list::RoomsListRef>().widget_uid(), consumer.widget_uid());
            assert_eq!(!sidebar.view.navigation_bar_button(&cx, ids!(open_projects)).is_empty(), desktop);
            assert_eq!(!sidebar.view.navigation_bar_button(&cx, ids!(open_agents)).is_empty(), desktop);
        }
    }
    #[test]
    fn desktop_navigation_keeps_one_sidebar_and_routes_contacts_chats_and_more() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.display_context.screen_size = dvec2(1280.0, 900.0);
        let mut home = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.HomeScreen {}});
            let home = HomeScreen::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty()); home
        });
        let mut state = AppState::default();
        draw(&mut cx, &mut home, &mut state, 1280.0);
        let mut children = Vec::new(); home.children(&mut |_, child| children.push(child));
        let mut pages = Vec::new(); collect_widgets::<PageFlip>(children, &mut pages);
        assert_eq!(pages.len(), 1);
        let page_rect = pages[0].area().rect(&cx);
        assert!((page_rect.pos.x - 440.0).abs() < 1.0 && (page_rect.size.x - 840.0).abs() < 1.0, "Right content uses remaining width: {page_rect:?}");
        let mut children = Vec::new(); home.children(&mut |_, child| children.push(child));
        let mut sidebars = Vec::new(); collect_sidebars(children, &mut sidebars);
        let sidebar = sidebars[0].borrow::<RoomsSideBar>().unwrap();
        let mut active = Vec::new(); sidebar.view.children(&mut |_, child| active.push(child));
        assert_eq!(active.len(), 1);
        let sidebar_rect = active[0].area().rect(&cx);
        assert!((sidebar_rect.size.x - 200.0).abs() < 1.0, "Sidebar has fixed width: {sidebar_rect:?}");
        let nav = sidebar.view.view(&cx, ids!(primary_navigation)).area().rect(&cx);
        for button in [id!(open_projects),id!(open_agents),id!(open_group_chats),id!(open_contacts),id!(open_miniapps)] {
            let rect = sidebar.view.widget(&cx, &[button]).area().rect(&cx);
            assert!(rect.size.y > 0.0 && rect.pos.y >= nav.pos.y - 1.0 && rect.pos.y + rect.size.y <= nav.pos.y + nav.size.y + 1.0, "Primary entry visible initially: {button:?} {rect:?} within {nav:?}");
        }
        drop(sidebar);
        let consumer = cx.global::<crate::home::rooms_list::RoomsListRef>().widget_uid();
        for (button, tab) in [(id!(open_contacts), SelectedTab::Contacts), (id!(open_settings), SelectedTab::Settings), (id!(open_group_chats), SelectedTab::Home)] {
            let mut children = Vec::new(); home.children(&mut |_, child| children.push(child));
            let mut found = Vec::new(); collect_sidebars(children, &mut found);
            assert_eq!(found.len(), 1);
            let sidebar = found[0].borrow::<RoomsSideBar>().unwrap();
            let uid = sidebar.view.navigation_bar_button(&cx, &[button]).widget_uid();
            assert_ne!(uid, WidgetUid(0)); drop(sidebar);
            let clicks = cx.capture_actions(|cx| cx.widget_action(uid, crate::shared::navigation_bar_button::NavigationBarButtonAction::Clicked));
            let navigation = cx.capture_actions(|cx| home.handle_event(cx, &Event::Actions(clicks), &mut Scope::with_data(&mut state)));
            let _trailing = cx.capture_actions(|cx| home.handle_event(cx, &Event::Actions(navigation), &mut Scope::with_data(&mut state)));
            assert_eq!(state.selected_tab, tab);
            draw(&mut cx, &mut home, &mut state, 1280.0);
            assert_eq!(cx.global::<crate::home::rooms_list::RoomsListRef>().widget_uid(), consumer);
        }
        let mut children = Vec::new(); home.children(&mut |_, child| children.push(child));
        let mut found = Vec::new(); collect_sidebars(children, &mut found);
        assert_eq!(found.len(), 1);
        let sidebar = found[0].borrow::<RoomsSideBar>().unwrap();
        for button in [id!(open_projects),id!(open_agents),id!(open_contacts),id!(open_group_chats),id!(open_miniapps),id!(more)] {
            assert!(!sidebar.view.navigation_bar_button(&cx, &[button]).is_empty());
        }
        drop(sidebar);
        draw_sized(&mut cx, &mut home, &mut state, 1280.0, 500.0);
        let mut children = Vec::new(); home.children(&mut |_, child| children.push(child));
        let mut found = Vec::new(); collect_sidebars(children, &mut found);
        let sidebar = found[0].borrow::<RoomsSideBar>().unwrap();
        let chats = home.view(&cx, ids!(project_chats)).area().rect(&cx);
        assert!(chats.size.y >= 40.0, "Navigation preserves a usable chat tree: {chats:?}");
        let account = sidebar.view.widget(&cx, ids!(account)).area().rect(&cx);
        assert!(account.size.y > 0.0 && account.pos.y + account.size.y <= 500.0, "Account stays within sidebar: {account:?}");
    }

    #[test]
    fn account_identity_children_fit_short_and_long_names() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut bar = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.HagencyAccountBar {}});
            let bar = crate::home::navigation_tab_bar::HagencyAccountBar::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty()); bar
        });
        for (name, user) in [("Alice", "@alice:hagency.local"), ("A very long display name that must remain a single line", "@verylongmatrixaccountidentity:very-long-hagency-server.example.org")] {
            std::ops::DerefMut::deref_mut(&mut bar).label(&cx, ids!(name)).set_text(&mut cx, name);
            std::ops::DerefMut::deref_mut(&mut bar).label(&cx, ids!(user)).set_text(&mut cx, user);
            draw_sized(&mut cx, std::ops::DerefMut::deref_mut(&mut bar), &mut AppState::default(), 260.0, 86.0);
            let button = std::ops::Deref::deref(&bar).widget(&cx, ids!(account)).area().rect(&cx);
            for id in [id!(name), id!(user)] {
                let rect = std::ops::Deref::deref(&bar).widget(&cx, &[id]).area().rect(&cx);
                assert!(rect.size.y > 0.0 && rect.pos.y >= button.pos.y && rect.pos.y + rect.size.y <= button.pos.y + button.size.y && rect.pos.y + rect.size.y <= 86.0, "Identity line fits account button: {rect:?} {button:?}");
            }
        }
    }

    #[test]
    fn sidebar_creation_menus_route_and_dismiss_without_moving_tree() {
        use super::super::sidebar_action_menu::{SidebarActionMenu, SidebarMenuAction, SidebarMenuKind};
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (mut home, mut menu) = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.HomeScreen {}});
            let home = HomeScreen::script_from_value(vm, value);
            let value = script_eval!(vm, {mod.widgets.SidebarActionMenu {}});
            let menu = SidebarActionMenu::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty()); (home, menu)
        });
        let mut state = AppState::default();
        draw(&mut cx, &mut home, &mut state, 1280.0);
        for (dots, item, kind) in [(id!(projects_actions), id!(new_project), SidebarMenuKind::Projects), (id!(group_chats_actions), id!(new_group_chat), SidebarMenuKind::GroupChats), (id!(agents_actions), id!(new_agent), SidebarMenuKind::Agents)] {
            let mut children = Vec::new(); home.children(&mut |_, child| children.push(child));
            let mut found = Vec::new(); collect_sidebars(children, &mut found);
            let sidebar = found[0].borrow::<RoomsSideBar>().unwrap();
            let tree_before = home.widget(&cx, ids!(project_chats)).area().rect(&cx);
            assert!(sidebar.view.navigation_bar_button(&cx, ids!(new_project)).is_empty());
            assert!(sidebar.view.navigation_bar_button(&cx, ids!(new_chat)).is_empty());
            let uid = sidebar.view.button(&cx, &[dots]).widget_uid();
            drop(sidebar);
            let clicks = cx.capture_actions(|cx| cx.widget_action(uid, ButtonAction::Clicked(KeyModifiers::default())));
            let opened = cx.capture_actions(|cx| home.handle_event(cx, &Event::Actions(clicks), &mut Scope::with_data(&mut state)));
            assert!(!opened.iter().any(|a| a.downcast_ref::<crate::hagency::ui::HagencyAction>().is_some() || a.downcast_ref::<NavigationBarAction>().is_some()), "Dots do not trigger row navigation");
            let (actual, pos) = opened.iter().find_map(|a| match a.downcast_ref() {Some(SidebarMenuAction::Open {kind, pos}) => Some((*kind, *pos)), _ => None}).unwrap();
            assert_eq!(actual, kind);
            menu.show(&mut cx, kind, pos, Rect {pos: dvec2(0.0, 0.0), size: dvec2(1280.0, 760.0)});
            draw(&mut cx, &mut menu, &mut state, 1280.0);
            let item_uid = menu.button(&cx, &[item]).widget_uid();
            let clicks = cx.capture_actions(|cx| cx.widget_action(item_uid, ButtonAction::Clicked(KeyModifiers::default())));
            let emitted = cx.capture_actions(|cx| menu.handle_event(cx, &Event::Actions(clicks), &mut Scope::with_data(&mut state)));
            assert!(emitted.iter().any(|a| match kind {
                SidebarMenuKind::Projects => matches!(a.downcast_ref(), Some(crate::hagency::ui::HagencyAction::NewProject)),
                SidebarMenuKind::GroupChats => matches!(a.downcast_ref(), Some(super::super::space_management::SpaceManagementAction::Open {parent:None})),
                SidebarMenuKind::Agents => matches!(a.downcast_ref(), Some(crate::hagency::ui::HagencyAction::NewAgent {project_id: None})),
                SidebarMenuKind::Tools => false,
            }));
            assert!(!menu.visible);
            draw(&mut cx, &mut home, &mut state, 1280.0);
            assert_eq!(home.widget(&cx, ids!(project_chats)).area().rect(&cx), tree_before);
        }
        menu.show(&mut cx, SidebarMenuKind::Projects, dvec2(200.0,100.0), Rect {pos: dvec2(0.0,0.0), size: dvec2(1280.0,760.0)});
        draw(&mut cx, &mut menu, &mut state, 1280.0);
        menu.handle_event(&mut cx, &Event::KeyDown(KeyEvent {key_code: KeyCode::Escape, is_repeat: false, modifiers: KeyModifiers::default(), time: 0.0}), &mut Scope::empty());
        assert!(!menu.visible);
        menu.show(&mut cx, SidebarMenuKind::Projects, dvec2(200.0,100.0), Rect {pos: dvec2(0.0,0.0), size: dvec2(1280.0,760.0)});
        draw(&mut cx, &mut menu, &mut state, 1280.0);
        let down = Event::MouseDown(MouseDownEvent {abs: dvec2(900.0,600.0), button: MouseButton::PRIMARY, window_id: WindowId(0,0), modifiers: KeyModifiers::default(), handled: std::cell::Cell::new(Area::Empty), time: 1.0});
        menu.handle_event(&mut cx, &down, &mut Scope::empty());
        let up = Event::MouseUp(MouseUpEvent {abs: dvec2(900.0,600.0), button: MouseButton::PRIMARY, window_id: WindowId(0,0), modifiers: KeyModifiers::default(), time: 1.1});
        menu.handle_event(&mut cx, &up, &mut Scope::empty());
        assert!(!menu.visible, "Clicking outside dismisses the real overlay");
    }

    #[test]
    fn more_overlay_shows_all_tools_in_short_window_and_routes_existing_actions() {
        use super::super::sidebar_action_menu::{SidebarActionMenu, SidebarMenuKind};
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut menu = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.SidebarActionMenu {}});
            SidebarActionMenu::script_from_value(vm, value)
        });
        for button in [id!(open_discover), id!(explore_rooms), id!(open_moments), id!(desktop_files), id!(open_article), id!(open_browser)] {
            menu.show(&mut cx, SidebarMenuKind::Tools, dvec2(240.0, 400.0), Rect {pos: dvec2(0.0,0.0), size: dvec2(1280.0,500.0)});
            draw_sized(&mut cx, &mut menu, &mut AppState::default(), 1280.0, 500.0);
            for id in [id!(open_discover), id!(explore_rooms), id!(open_moments), id!(desktop_files), id!(open_article), id!(open_browser)] {
                let rect = menu.widget(&cx, &[id]).area().rect(&cx);
                assert!(rect.size.y > 0.0 && rect.pos.y >= 0.0 && rect.pos.y + rect.size.y <= 500.0, "All More tools fit in viewport: {id:?} {rect:?}");
            }
            let uid = menu.button(&cx, &[button]).widget_uid();
            let clicks = cx.capture_actions(|cx| cx.widget_action(uid, ButtonAction::Clicked(KeyModifiers::default())));
            let emitted = cx.capture_actions(|cx| menu.handle_event(cx, &Event::Actions(clicks), &mut Scope::empty()));
            assert!(emitted.iter().any(|a| {
                if button == id!(open_discover) {matches!(a.downcast_ref(), Some(NavigationBarAction::GoToTab(SelectedTab::Discover)))}
                else if button == id!(explore_rooms) {matches!(a.downcast_ref(), Some(NavigationBarAction::GoToAddRoom))}
                else if button == id!(open_moments) {matches!(a.downcast_ref(), Some(crate::moments::ui::MomentsAction::Open {author: None}))}
                else if button == id!(desktop_files) {matches!(a.downcast_ref(), Some(crate::moments::ui::MomentsAction::FileTransfer))}
                else if button == id!(open_browser) {a.downcast_ref::<crate::shared::web_browser::WebBrowserAction>().is_some()}
                else {a.downcast_ref::<crate::article_app::ArticleAction>().is_some()}
            }));
            assert!(!menu.visible);
        }
    }

    #[test]
    fn workspace_context_is_separate_keeps_project_room_selection_and_hides_for_other_roots() {
        use crate::hagency::ui::HagencyAction;
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut home = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.HomeScreen {}});
            HomeScreen::script_from_value(vm, value)
        });
        let mut state = AppState::default();
        draw(&mut cx, &mut home, &mut state, 1280.0);
        *cx.global::<DesktopWorkspace>()=DesktopWorkspace::GroupChats;
        draw(&mut cx,&mut home,&mut state,1280.0);
        let sidebar=home.widget(&cx,ids!(persistent_sidebar));
        assert_eq!(sidebar.borrow::<RoomsSideBar>().unwrap().selected_primary,Some(id!(open_group_chats)),"Restored Chats context selects Chats without a click");
        *cx.global::<DesktopWorkspace>()=DesktopWorkspace::Projects;
        draw(&mut cx,&mut home,&mut state,1280.0);
        assert_eq!(sidebar.borrow::<RoomsSideBar>().unwrap().selected_primary,Some(id!(open_projects)));
        let consumer = cx.global::<crate::home::rooms_list::RoomsListRef>().widget_uid();
        let mut trees = Vec::new(); let mut children = Vec::new(); home.children(&mut |_, child| children.push(child));
        collect_widgets::<crate::home::project_tree::ProjectTree>(children, &mut trees);
        assert_eq!(trees.len(), 1);
        let tree_uid = trees[0].widget_uid();
        let context = home.view(&cx, ids!(workspace_context)).area().rect(&cx);
        assert_eq!(context.pos.x, 200.0); assert_eq!(context.size.x, 240.0);
        assert_eq!(home.label(&cx, ids!(context_title)).text(), crate::i18n::tr("Projects"));
        assert!(!home.view(&cx, ids!(context_search)).borrow().unwrap().visible);
        let room = crate::app::SelectedRoom::JoinedRoom {room_name_id: crate::utils::RoomNameId::new(matrix_sdk::RoomDisplayName::Named("Project discussion".into()), "!discussion:hagency.local".try_into().unwrap())};
        let actions = cx.capture_actions(|cx| cx.action(crate::home::rooms_list::RoomsListAction::Selected(room)));
        home.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::with_data(&mut state));
        assert_eq!(*cx.global::<DesktopWorkspace>(), DesktopWorkspace::Projects, "Room focus preserves Projects context");
        for (workspace, visible, chats) in [(DesktopWorkspace::Other, false, false), (DesktopWorkspace::GroupChats, true, true), (DesktopWorkspace::Projects, true, false)] {
            let actions = cx.capture_actions(|cx| match workspace {
                DesktopWorkspace::Other => cx.action(HagencyAction::OpenAgents),
                DesktopWorkspace::GroupChats => cx.action(crate::home::main_desktop_ui::MainDesktopUiAction::ShowGroupChats),
                DesktopWorkspace::Projects => cx.action(HagencyAction::OpenProjects),
            });
            home.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::with_data(&mut state));
            draw(&mut cx, &mut home, &mut state, 1280.0);
            assert_eq!(home.view(&cx, ids!(workspace_context)).borrow().unwrap().visible, visible);
            let mut pages = Vec::new(); let mut children = Vec::new(); home.children(&mut |_, child| children.push(child)); collect_widgets::<PageFlip>(children, &mut pages);
            assert_eq!(pages[0].area().rect(&cx).pos.x, if visible {440.0} else {200.0});
            if visible {assert_eq!(home.view(&cx, ids!(context_search)).borrow().unwrap().visible, chats);}
            assert_eq!(cx.global::<crate::home::rooms_list::RoomsListRef>().widget_uid(), consumer);
            assert_eq!(home.widget(&cx, ids!(project_tree)).widget_uid(), tree_uid);
        }
        let actions = cx.capture_actions(|cx| cx.action(NavigationBarAction::GoToTab(SelectedTab::Contacts)));
        home.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::with_data(&mut state));
        draw(&mut cx, &mut home, &mut state, 1280.0);
        assert!(!home.view(&cx, ids!(workspace_context)).borrow().unwrap().visible);
    }

    #[test]
    fn overflow_follows_pointer_between_navigation_rows() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut home=cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);
            let value=script_eval!(vm,{mod.widgets.HomeScreen {}});
            HomeScreen::script_from_value(vm,value)
        });
        let mut state=AppState::default();
        draw(&mut cx,&mut home,&mut state,1280.0);
        let mut children=Vec::new();home.children(&mut |_,child|children.push(child));
        let mut sidebars=Vec::new();collect_sidebars(children,&mut sidebars);
        let sidebar=sidebars[0].clone();
        for target in [Some(id!(projects_row)),Some(id!(agents_row)),Some(id!(group_chats_row)),None] {
            let abs=target.map(|row| {
                let binding=sidebar.borrow::<RoomsSideBar>().unwrap();
                let rect=binding.view.view(&cx,&[row]).area().rect(&cx);
                assert!(rect.size.x>0.0 && rect.size.y>0.0,"{row:?}: {rect:?}");
                rect.pos+rect.size*0.5
            }).unwrap_or(dvec2(900.0,600.0));
            home.handle_event(&mut cx,&Event::MouseMove(MouseMoveEvent {abs,lock_delta:DVec2::default(),window_id:WindowId(0,0),modifiers:KeyModifiers::default(),handled:std::cell::Cell::new(Area::Empty),time:1.0}),&mut Scope::with_data(&mut state));
            draw(&mut cx,&mut home,&mut state,1280.0);
            let binding=sidebar.borrow::<RoomsSideBar>().unwrap();
            for (row,button) in [(id!(projects_row),id!(projects_actions)),(id!(agents_row),id!(agents_actions)),(id!(group_chats_row),id!(group_chats_actions))] {
                assert_eq!(binding.view.button(&cx,&[button]).visible(),target==Some(row),"{row:?}, pointer={:?}",binding.pointer);
            }
        }
    }

    #[test]
    fn collapsed_rail_keeps_icons_account_and_context_without_duplicate_consumers() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut home = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.HomeScreen {}});
            let home = HomeScreen::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty()); home
        });
        let mut state = AppState::default();
        draw(&mut cx, &mut home, &mut state, 1280.0);
        let consumer = cx.global::<crate::home::rooms_list::RoomsListRef>().widget_uid();
        let tree = home.widget(&cx, ids!(project_tree)).widget_uid();
        for collapsed in [true, false, true] {
            let mut children = Vec::new(); home.children(&mut |_, child| children.push(child));
            let mut sidebars = Vec::new(); collect_sidebars(children, &mut sidebars);
            let sidebar = sidebars[0].borrow::<RoomsSideBar>().unwrap();
            let uid = sidebar.view.navigation_bar_button(&cx, ids!(toggle_sidebar)).widget_uid();
            drop(sidebar);
            let clicks = cx.capture_actions(|cx| cx.widget_action(uid, crate::shared::navigation_bar_button::NavigationBarButtonAction::Clicked));
            let actions = cx.capture_actions(|cx| home.handle_event(cx, &Event::Actions(clicks), &mut Scope::with_data(&mut state)));
            home.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::with_data(&mut state));
            assert_eq!(state.app_prefs.desktop_sidebar_collapsed, collapsed);
            draw(&mut cx, &mut home, &mut state, 1280.0);
            let rail = home.view(&cx, ids!(persistent_sidebar_frame)).area().rect(&cx);
            assert_eq!(rail.size.x, if collapsed {64.0} else {200.0});
            assert_eq!(home.view(&cx, ids!(workspace_context)).area().rect(&cx).pos.x, rail.size.x);
            let mut children = Vec::new(); home.children(&mut |_, child| children.push(child));
            let mut sidebars = Vec::new(); collect_sidebars(children, &mut sidebars);
            let sidebar = sidebars[0].borrow::<RoomsSideBar>().unwrap();
            for id in [id!(open_projects),id!(open_agents),id!(open_group_chats),id!(open_contacts),id!(open_miniapps),id!(more),id!(open_settings)] {
                let button = sidebar.view.navigation_bar_button(&cx, &[id]);
                let rect = button.area().rect(&cx);
                let icon = button.icon(&cx, ids!(nav_icon)).area().rect(&cx);
                assert!(icon.size.x > 0.0 && icon.size.y > 0.0 && icon.pos.x >= rect.pos.x && icon.pos.x + icon.size.x <= rect.pos.x + rect.size.x + 0.1, "{id:?} {icon:?} {rect:?}");
                assert_eq!(button.label(&cx, ids!(label)).visible(), !collapsed);
            }
            for (action, navigation) in [(id!(projects_actions), id!(open_projects)), (id!(agents_actions), id!(open_agents)), (id!(group_chats_actions), id!(open_group_chats))] {
                let button = sidebar.view.button(&cx, &[action]);
                assert!(!button.visible(),"Overflow remains hidden before row hover");
                if !collapsed {
                    let slot=if action==id!(projects_actions){id!(projects_action_slot)}else if action==id!(agents_actions){id!(agents_action_slot)}else{id!(group_chats_action_slot)};
                    let rect = sidebar.view.view(&cx,&[slot]).area().rect(&cx);
                    let row = sidebar.view.navigation_bar_button(&cx, &[navigation]).area().rect(&cx);
                    assert_eq!(rect.size, dvec2(32.0, 32.0));
                    assert_eq!(rect.pos.y + rect.size.y * 0.5, row.pos.y + row.size.y * 0.5, "overflow must share the navigation row center");
                    assert!(button.text().is_empty(), "overflow uses an icon, not a font glyph");
                }
            }
            let avatar = sidebar.view.avatar(&cx, ids!(account.avatar)).area().rect(&cx);
            let account = sidebar.view.widget(&cx, ids!(account)).area().rect(&cx);
            assert!(avatar.size.x > 0.0 && avatar.pos.x >= account.pos.x && avatar.pos.x + avatar.size.x <= account.pos.x + account.size.x + 0.1, "{avatar:?} {account:?}");
            assert_eq!(cx.global::<crate::home::rooms_list::RoomsListRef>().widget_uid(), consumer);
            assert_eq!(home.widget(&cx, ids!(project_tree)).widget_uid(), tree);
            let restored: crate::settings::app_preferences::AppPreferences = serde_json::from_value(serde_json::to_value(&state.app_prefs).unwrap()).unwrap();
            assert_eq!(restored.desktop_sidebar_collapsed, collapsed);
        }
    }

    #[test]
    fn compact_and_forced_wide_layout_leave_nonnegative_chat_content() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut home = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.HomeScreen {}});
            let home = HomeScreen::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty()); home
        });
        let mut state = AppState::default(); state.app_prefs.view_mode = ViewModeOverride::ForceWide;
        let actions = cx.capture_actions(|cx| state.app_prefs.on_view_mode_changed(cx));
        home.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::with_data(&mut state));
        for (width, context, remaining) in [(800.0, true, 460.0), (375.0, false, 215.0)] {
            draw(&mut cx, &mut home, &mut state, width);
            draw(&mut cx, &mut home, &mut state, width);
            let rail = home.view(&cx, ids!(persistent_sidebar_frame)).area().rect(&cx);
            assert_eq!(rail.size.x, 160.0);
            assert_eq!(home.view(&cx, ids!(workspace_context)).borrow().unwrap().visible, context);
            if context {assert_eq!(home.view(&cx, ids!(workspace_context)).area().rect(&cx).size.x, 180.0);}
            let mut pages = Vec::new(); let mut children = Vec::new(); home.children(&mut |_, child| children.push(child)); collect_widgets::<PageFlip>(children, &mut pages);
            let page = pages[0].area().rect(&cx);
            assert_eq!(page.size.x, remaining);
            assert!(page.pos.x >= 0.0 && page.pos.x + page.size.x <= width);
        }
    }

}
