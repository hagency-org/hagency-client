use makepad_widgets::*;
use crate::hagency::ui::{HagencyAction, HagencyPanelWidgetRefExt};
use ruma::OwnedRoomId;
use tokio::sync::Notify;
use std::{collections::{HashMap, HashSet}, sync::Arc};

use crate::settings::app_preferences::{AppPreferencesAction, AppPreferencesGlobal};
use crate::{app::{AppState, AppStateAction, SavedDockState, SelectedRoom}, home::{navigation_tab_bar::{NavigationBarAction, SelectedTab}, rooms_list::RoomsListRef, space_lobby::SpaceLobbyScreenWidgetRefExt}, utils::RoomNameId};
use super::{invite_screen::InviteScreenWidgetRefExt, room_screen::RoomScreenWidgetRefExt, rooms_list::{AcceptedInviteKind, RoomsListAction}, spaces_bar::SpacesBarAction};

#[derive(Default)]
struct ContactDock(Option<makepad_widgets::widget::WidgetWeakRef>);

/// Return the original timeline before Contacts takes it. Old saved layouts may
/// still contain DM tabs. The weak binding never keeps an obsolete dock alive.
pub(crate) fn release_legacy_direct_timeline(cx:&mut Cx,room:&RoomNameId) {
    let dock=cx.global::<ContactDock>().0.as_ref().and_then(makepad_widgets::widget::WidgetWeakRef::upgrade);
    if let Some(dock)=dock {
        let tab=SelectedRoom::JoinedRoom {room_name_id:room.clone()}.tab_id();
        dock.as_dock().item(tab).as_room_screen().hide_displayed_room(cx);
    }
}
struct RestoredRoomContext {
    room: OwnedRoomId,
    epoch: u64,
}
script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*


    mod.widgets.MainDesktopUI = #(MainDesktopUI::register_widget(vm)) {
        dock := mod.widgets.RobrixDock {
            width: Fill,
            height: Fill,
            padding: 0,
            spacing: 0,
            margin: 0

            tab_bar +: {
                CloseableTab := mod.widgets.RobrixTab { closeable: true }
                PermanentTab := mod.widgets.RobrixTab { closeable: false }
            }


            root := DockTabs {tabs: [@home_tab] selected: 0}

            home_tab := DockTab{
                name: "Projects"
                kind: @hagency_panel
                template: @PermanentTab
            }

            // Below are the templates of widgets that can be created within dock tabs.
            group_chats_welcome := mod.widgets.GroupChatsWelcome {}
            hagency_panel := mod.widgets.HagencyPanel {}
            room_screen := mod.widgets.RoomScreen {
                desktop_chat_header +: { visible: true }
            }
            invite_screen := mod.widgets.InviteScreen {}
            space_lobby_screen := mod.widgets.SpaceLobbyScreen {}
        }
    }
}

#[derive(Script, Widget)]
pub struct MainDesktopUI {
    #[deref]
    view: View,

    /// The default layout that should be loaded into the dock
    /// when there is no previously-saved content to restore.
    /// This is a Rust-level instance of the dock content defined in the above live DSL.
    #[rust] default_layout: SavedDockState,

    /// The rooms that are currently open, keyed by the LiveId of their tab.
    #[rust]
    open_rooms: HashMap<LiveId, SelectedRoom>,

    /// The order in which room/thread tabs were last viewed,
    /// from oldest at the front to most recent at the end.
    #[rust]
    room_order: Vec<SelectedRoom>,

    /// The most recently selected room, used to prevent re-selecting the same room in Dock
    /// which would trigger redraw of whole Widget.
    #[rust]
    most_recently_selected_room: Option<SelectedRoom>,

    /// The ID of the currently-selected space, if any.
    ///
    /// This determines which set of rooms this dock is currently showing.
    /// If `None`, we're displaying the main home view of all rooms from any space.
    #[rust] selected_space: Option<OwnedRoomId>,

    /// Boolean to indicate if we've drawn the MainDesktopUi previously in the desktop view.
    ///
    /// When switching mobile view to desktop, we need to restore the saved app state to the UI.
    /// * If false, this widget emits an action to load the dock from the saved dock state.
    /// * If true, this widget proceeds to draw the desktop UI as normal.
    #[rust]
    drawn_previously: bool,

    /// Mirrors the `tabbed_chats` preference.
    ///
    /// When `false`, the dock behaves like WeChat desktop: its tab bars are hidden
    /// and at most one room stays open, so selecting a room replaces the open chat.
    #[rust]
    tabbed_chats: bool,
    #[rust]
    restored_context: Option<RestoredRoomContext>,
    #[rust]
    matrix_link_root: Option<crate::matrix_links::Ticket>,
}

impl ScriptHook for MainDesktopUI {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.default_layout = self.save_dock_state(cx);
            cx.global::<ContactDock>().0=Some(self.view.dock(cx,ids!(dock)).downgrade());
        });
    }
}
impl Widget for MainDesktopUI {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.widget_match_event(cx, event, scope); // invokes `WidgetMatchEvent` impl
        self.view.handle_event(cx, event, scope);

        // For convenience, we support go-back gestures when viewing a thread's tab
        // to easily go back to the most recent room.
        if let Some(sr @ SelectedRoom::Thread { .. }) = self.most_recently_selected_room.as_ref()
            && (event.back_pressed() || matches!(event, Event::MouseUp(e) if e.button.is_back()))
        {
            self.close_tab(cx, sr.tab_id());
            self.redraw(cx);
            cx.action(MainDesktopUiAction::SaveDockIntoAppState);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.drawn_previously && cx.has_global::<RoomsListRef>() {
            self.tabbed_chats = cx.global::<AppPreferencesGlobal>().0.tabbed_chats;
            // When changing from Mobile to Desktop view mode, we need to restore the state
            // of this widget, which we get from the `AppState` passed down via `scope`.
            // This includes the currently selected space, which we get from the RoomsList widget.
            // We must set `selected_space` first before the load operation occurs, in order for
            // the proper space-specific instance of the saved dock UI layout/state to be selected.
            self.selected_space = cx.get_global::<RoomsListRef>().get_selected_space_id();
            cx.action(MainDesktopUiAction::LoadDockFromAppState);
            self.drawn_previously = true;
        }
        if let Some(pending) = self.restored_context.as_ref() && cx.has_global::<RoomsListRef>() {
            let direct = cx.get_global::<RoomsListRef>().is_direct_room(&pending.room);
            self.apply_restored_context(cx, direct, crate::account_session::epoch());
        }
        self.view.draw_walk(cx, scope, walk)
    }
}

impl MainDesktopUI {
    fn apply_restored_context(&mut self, cx: &mut Cx, direct: Option<bool>, epoch: u64) {
        let Some(pending) = self.restored_context.as_ref() else { return; };
        if pending.epoch != epoch || self.most_recently_selected_room.as_ref().is_none_or(|r| r.room_id() != &pending.room) {
            self.restored_context = None;
            return;
        }
        if let Some(direct) = direct {
            self.restored_context = None;
            if direct {
                // Only the sidebar context changes. ShowGroupChats would reload a
                // dock and could replace the exact Room we just restored.
                if let Some(room)=self.most_recently_selected_room.as_ref() {super::mobile::navigate_direct(cx,room.room_name().clone());}
                cx.redraw_all();
            }
        }
    }

    /// Moves or adds the given room the the end of the `room_order`, the most recent spot.
    fn mark_room_as_recent(&mut self, room: &SelectedRoom) {
        self.room_order.retain(|sr| sr != room);
        self.room_order.push(room.clone());
    }

    /// Selects the tab for the given `room`, or the home tab if `None`.
    ///
    /// Updates the room order, current selection, and app state.
    fn select_room(&mut self, cx: &mut Cx, room: Option<SelectedRoom>) {
        let dock = self.view.dock(cx, ids!(dock));
        if let Some(room) = room {
            dock.select_tab(cx, room.tab_id());
            self.mark_room_as_recent(&room);
            cx.action(AppStateAction::RoomFocused(room.clone()));
            self.most_recently_selected_room = Some(room);
        } else {
            dock.select_tab(cx, id!(home_tab));
            cx.action(AppStateAction::FocusNone);
            self.most_recently_selected_room = None;
        }
        // Dock::select_tab invalidates the tab bar's draw list. In single-pane
        // mode that bar is hidden, so invalidate the content parent as well;
        // otherwise its old welcome/Room pixels can remain after navigation.
        self.redraw(cx);
    }

    /// Focuses on a room if it is already open, otherwise creates a new tab for the room.
    fn focus_or_create_tab(&mut self, cx: &mut Cx, room: SelectedRoom) {
        // Do nothing if the room to select is already created and focused.
        if self.most_recently_selected_room.as_ref().is_some_and(|sr| sr == &room) {
            return;
        }

        let dock = self.view.dock(cx, ids!(dock));

        // If the room is already open, select (jump to) its existing tab
        let room_tab_id = room.tab_id();
        if self.open_rooms.contains_key(&room_tab_id) {
            self.select_room(cx, Some(room));
            // Lazily initialize the tab's widget if it was deferred during dock restoration.
            self.init_tab_if_needed(cx, room_tab_id);
            cx.action(MainDesktopUiAction::SaveDockIntoAppState);
            return;
        }

        // Create a new tab for the room
        let kind = room.dock_kind();

        // Insert the tab after the currently-selected room's tab, if possible.
        // Otherwise, insert it after the home tab, which should always exist.
        let (tab_bar, insert_after) = self.most_recently_selected_room.as_ref()
            .and_then(|curr_room| dock.find_tab_bar_of_tab(curr_room.tab_id()))
            .unwrap_or_else(|| dock.find_tab_bar_of_tab(id!(home_tab)).unwrap());

        let new_tab_widget = dock.create_and_select_tab(
            cx,
            tab_bar,
            room_tab_id,
            kind,
            room.display_name(),
            id!(CloseableTab),
            Some(insert_after),
        );

        // if the tab was created, set the room screen
        if let Some(new_widget) = new_tab_widget {
            match &room {
                SelectedRoom::JoinedRoom { room_name_id }  => {
                    new_widget.as_room_screen().set_displayed_room(
                        cx,
                        room_name_id,
                        None,
                    );
                }
                SelectedRoom::Thread { room_name_id, thread_root_event_id } => {
                    new_widget.as_room_screen().set_displayed_room(
                        cx,
                        room_name_id,
                        Some(thread_root_event_id.clone()),
                    );
                }
                SelectedRoom::InvitedRoom { room_name_id } => {
                    new_widget.as_invite_screen().set_displayed_invite(
                        cx,
                        room_name_id,
                    );
                }
                SelectedRoom::Space { space_name_id } => {
                    new_widget.as_space_lobby_screen().set_displayed_space(
                        cx,
                        space_name_id,
                    );
                }
            }
            cx.action(MainDesktopUiAction::SaveDockIntoAppState);
            self.open_rooms.insert(room_tab_id, room.clone());
            self.select_room(cx, Some(room));
            if !self.tabbed_chats {
                self.close_background_tabs(cx);
            }
        } else {
            error!("BUG: failed to create tab for {room:?}");
        }
    }

    /// Closes the current room tab, if any, selecting the next most recent one.
    ///
    /// Returns whether a tab was actually closed. The permanent home tab is
    /// never closed: it is not in `open_rooms`, and selecting it clears
    /// `most_recently_selected_room`. Single-pane mode has no tab bar, so
    /// there is no "current tab" to close.
    fn close_current_tab(&mut self, cx: &mut Cx) -> bool {
        if !self.tabbed_chats {
            return false;
        }
        let Some(room) = self.most_recently_selected_room.clone() else { return false };
        let tab_id = room.tab_id();
        if !self.open_rooms.contains_key(&tab_id) {
            return false;
        }
        self.close_tab(cx, tab_id);
        true
    }

    /// Closes a tab in the dock and selects the next most recently viewed tab.
    fn close_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        let dock = self.view.dock(cx, ids!(dock));

        let Some(room_being_closed) = self.open_rooms.get(&tab_id).cloned() else {
            // This shouldn't happen (the tab should always be in the set of open rooms),
            // but we still need to handle it gracefully.
            dock.close_tab(cx, tab_id);
            self.init_all_visible_tabs(cx);
            return;
        };
        // If we're closing a thread timeline, free up its resources & bkgd async tasks.
        room_being_closed.close_thread_timeline(cx);
        self.room_order.retain(|sr| sr != &room_being_closed);

        let is_active_tab = self.most_recently_selected_room.as_ref() == Some(&room_being_closed);
        let room_to_select = if is_active_tab {
            self.room_order.last().cloned()
        } else {
            self.most_recently_selected_room.clone()
        };

        dock.close_tab(cx, tab_id);
        self.open_rooms.remove(&tab_id);

        // Makepad's dock chooses an adjacent tab by itself, so we have to override that.
        self.select_room(cx, room_to_select);

        self.init_all_visible_tabs(cx);
    }

    /// Closes every room tab except the currently-selected one
    /// (and its main room, when a thread is selected, so that going back works).
    ///
    /// Used in single-pane mode, where only the open chat is kept alive.
    fn close_background_tabs(&mut self, cx: &mut Cx) {
        let dock = self.view.dock(cx, ids!(dock));
        let keep = self.most_recently_selected_room.as_ref().map(SelectedRoom::tab_id);
        let parent_room_id = match self.most_recently_selected_room.as_ref() {
            Some(SelectedRoom::Thread { room_name_id, .. }) => Some(room_name_id.room_id().clone()),
            _ => None,
        };
        let to_close: Vec<LiveId> = self.open_rooms.iter()
            .filter(|(tab_id, room)| Some(**tab_id) != keep && !(
                matches!(room, SelectedRoom::JoinedRoom { .. })
                    && parent_room_id.as_ref().is_some_and(|id| id == room.room_id())
            ))
            .map(|(tab_id, _)| *tab_id)
            .collect();
        for tab_id in to_close {
            if let Some(room) = self.open_rooms.remove(&tab_id) {
                room.close_thread_timeline(cx);
                self.room_order.retain(|sr| sr != &room);
            }
            dock.close_tab(cx, tab_id);
        }
        // Closing tabs lets the dock pick an adjacent tab, so reassert our selection.
        if let Some(tab_id) = keep {
            dock.select_tab(cx, tab_id);
        }
        self.init_all_visible_tabs(cx);
    }

    /// Shows or hides the tab bars of all content panes (not the rooms sidebar),
    /// according to `self.tabbed_chats`.
    fn apply_tab_bar_visibility(&self, cx: &mut Cx) {
        let dock = self.view.dock(cx, ids!(dock));
        let Some(mut dock_items) = dock.clone_state() else { return };
        let hide = !self.tabbed_chats;
        let mut changed = false;
        for item in dock_items.values_mut() {
            if let DockItem::Tabs { tabs, hide_tab_bar, .. } = item
                && !tabs.contains(&id!(rooms_sidebar_tab))
                && *hide_tab_bar != hide
            {
                *hide_tab_bar = hide;
                changed = true;
            }
        }
        if changed {
            dock.load_state_preserving_items(cx, dock_items);
        }
    }

    /// Switches between single-pane and tabbed mode.
    fn set_tabbed_chats(&mut self, cx: &mut Cx, tabbed: bool) {
        self.tabbed_chats = tabbed;
        if !tabbed {
            self.close_background_tabs(cx);
        }
        self.apply_tab_bar_visibility(cx);
        self.init_all_visible_tabs(cx);
        self.redraw(cx);
        cx.action(MainDesktopUiAction::SaveDockIntoAppState);
    }

    /// Closes all tabs
    pub fn close_all_tabs(&mut self, cx: &mut Cx) {
        let dock = self.view.dock(cx, ids!(dock));
        for (tab_id, room) in self.open_rooms.iter() {
            room.close_thread_timeline(cx);
            dock.close_tab(cx, *tab_id);
        }

        // Clear tab-related dock UI state.
        self.open_rooms.clear();
        self.room_order.clear();
        self.select_room(cx, None);
        cx.action(MainDesktopUiAction::SaveDockIntoAppState);
    }

    /// Replaces an invite with a joined room or space in the dock.
    fn replace_invite_with_joined(
        &mut self,
        cx: &mut Cx,
        room_name_id: &RoomNameId,
        is_space: bool,
    ) {
        let joined = SelectedRoom::to_joined(room_name_id.clone(), is_space);
        let dock = self.view.dock(cx, ids!(dock));
        if let Some((new_widget, _)) = dock.replace_tab(
            cx,
            joined.tab_id(),
            joined.dock_kind(),
            Some(joined.display_name()),
            false,
        ) {
            match is_space {
                true => new_widget.as_space_lobby_screen()
                    .set_displayed_space(cx, room_name_id),
                false => new_widget.as_room_screen()
                    .set_displayed_room(cx, room_name_id, None),
            }

            // Note: don't return here. Whether or not this invited room is opened in a tab,
            // we still need to upgrade its SelectedRoom instance, so fall through below.
        }

        self.upgrade_open_invites(cx, room_name_id, is_space);
    }

    /// Replaces all invited `SelectedRoom` instances for this room/space with the
    /// equivalent joined variant for this room/space.
    ///
    /// Also emits an action to instruct AppState to update itself accordingly.
    fn upgrade_open_invites(&mut self, cx: &mut Cx, room_name_id: &RoomNameId, is_space: bool) {
        for selected_room in self.most_recently_selected_room.iter_mut()
            .chain(self.room_order.iter_mut())
            .chain(self.open_rooms.values_mut())
        {
            selected_room.upgrade_invite_to_joined(room_name_id.room_id(), is_space);
        }
        cx.action(AppStateAction::UpgradedInviteToJoinedRoom {
            room_id: room_name_id.room_id().clone(),
            is_space,
        });
    }

    /// Saves the current dock's state and then loads the given space's dock (or the main dock).
    fn switch_dock_to_space(
        &mut self,
        cx: &mut Cx,
        app_state: &mut AppState,
        new_space: Option<OwnedRoomId>,
    ) {
        self.save_dock_state_to(cx, app_state);
        self.selected_space = new_space;
        self.load_dock_state_from(cx, app_state);
    }

    /// Saves a copy of the current UI state of the dock into the given app state,
    /// properly accounting for which space is currently selected.
    fn save_dock_state_to(&mut self, cx: &mut Cx, app_state: &mut AppState) {
        let saved_dock_state = self.save_dock_state(cx);
        if let Some(space_id) = self.selected_space.as_ref() {
            app_state.saved_dock_state_per_space.insert(
                space_id.clone(),
                saved_dock_state,
            );
        } else {
            app_state.saved_dock_state_home = saved_dock_state;
        }
    }

    /// An inner function that creates a `SavedDockState` from the current contents of this widget. 
    fn save_dock_state(&self, cx: &mut Cx) -> SavedDockState {
        let dock = self.view.dock(cx, ids!(dock));
        SavedDockState {
            dock_items: dock.clone_state().unwrap_or_default(),
            open_rooms: self.open_rooms.clone(),
            room_order: self.room_order.clone(),
            selected_room: self.most_recently_selected_room.clone(),
        }
    }

    /// Loads and populates the dock from the saved dock state for the currently-selected space.
    ///
    /// If the saved state is empty (has no open rooms), we use the default dock layout
    /// defined in the DSL: one splitter with the RoomsList on the left and a Welcome tab on the right.
    fn load_dock_state_from(&mut self, cx: &mut Cx, app_state: &mut AppState) {
        let dock = self.view.dock(cx, ids!(dock));

        let saved_ref: Option<&SavedDockState> = if let Some(ss) = self.selected_space.as_ref() {
            app_state.saved_dock_state_per_space.get(ss)
        } else {
            Some(&app_state.saved_dock_state_home)
        };
        let space_label = self.selected_space.as_ref()
            .map(|s| format!("space {s}"))
            .unwrap_or_else(|| "home".to_string());
        let (to_restore, recreate_from_room_order): (SavedDockState, Option<Vec<SelectedRoom>>) =
            match saved_ref {
                None => (self.default_layout.clone(), None),
                Some(sds) if sds.open_rooms.is_empty()
                    && sds.room_order.is_empty()
                    && sds.selected_room.is_none() => (self.default_layout.clone(), None),
                Some(sds) => {
                    let mut candidate = sds.clone();
                    match dock_state_repair::validate_and_repair_dock_state(&mut candidate) {
                        Ok(false) => (candidate, None),
                        Ok(true) => {
                            log!("Repaired corrupt saved dock state for {space_label}.");
                            // Update the app state with the repaired dock state to ensure that
                            // the next save operation will persist valid state to storage.
                            if let Some(ss) = self.selected_space.as_ref() {
                                app_state.saved_dock_state_per_space.insert(ss.clone(), candidate.clone());
                            } else {
                                app_state.saved_dock_state_home = candidate.clone();
                            }
                            (candidate, None)
                        }
                        Err(reason) => {
                            error!(
                                "Saved dock state for {space_label} is unrepairable ({reason}); \
                                 falling back to default layout and re-adding tabs from saved room_order."
                            );
                            let original_selected = sds.selected_room.clone();
                            let original_order = sds.room_order.clone();
                            let mut fallback = self.default_layout.clone();
                            fallback.selected_room = original_selected;
                            (fallback, Some(original_order))
                        }
                    }
                }
            };

        let SavedDockState { dock_items, open_rooms, room_order, selected_room } = to_restore;

        self.room_order = room_order;
        self.open_rooms = open_rooms;

        dock.load_state(cx, dock_items);
        // Lazily populate the dock content to avoid initializing tabs that aren't visible.
        self.init_all_visible_tabs(cx);

        // If using the fallback, we re-add each tab from room_order so the user's opened rooms aren't lost.
        if let Some(orig_room_order) = recreate_from_room_order {
            for room in orig_room_order {
                self.focus_or_create_tab(cx, room);
            }
        }

        // Now that we've loaded the dock content, we can re-select the selected room.
        // If this dock has no selection, clear the previous dock's selection so that,
        // for ex, it doesn't mistakenly save a now-closed invite screen as the selected room.
        let selected_room = selected_room.clone();
        match selected_room.clone() {
            Some(selected_room) => self.focus_or_create_tab(cx, selected_room),
            None => self.most_recently_selected_room = None,
        }
        if !self.tabbed_chats {
            self.close_background_tabs(cx);
        }
        self.apply_tab_bar_visibility(cx);
        self.init_all_visible_tabs(cx);
        self.restored_context = selected_room.as_ref().map(|r| RestoredRoomContext {room:r.room_id().clone(),epoch:crate::account_session::epoch()});
        if let Some(pending) = self.restored_context.as_ref() && cx.has_global::<RoomsListRef>() {
            let direct = cx.get_global::<RoomsListRef>().is_direct_room(&pending.room);
            self.apply_restored_context(cx, direct, crate::account_session::epoch());
        }
        app_state.selected_room = selected_room;
        self.redraw(cx);
    }

    /// Initializes a single dock tab's widget content based on the room it represents.
    ///
    /// This is extracted as a helper so it can be called both during dock restoration
    /// (for the selected tab only) and lazily when the user clicks on an uninitialized tab.
    fn init_tab_widget(
        cx: &mut Cx,
        open_rooms: &HashMap<LiveId, SelectedRoom>,
        tab_live_id: &LiveId,
        widget: &WidgetRef,
    ) {
        match open_rooms.get(tab_live_id) {
            Some(SelectedRoom::JoinedRoom { room_name_id }) => {
                widget.as_room_screen().set_displayed_room(
                    cx,
                    room_name_id,
                    None,
                );
            }
            Some(SelectedRoom::InvitedRoom { room_name_id }) => {
                widget.as_invite_screen().set_displayed_invite(
                    cx,
                    room_name_id,
                );
            }
            Some(SelectedRoom::Space { space_name_id }) => {
                widget.as_space_lobby_screen().set_displayed_space(
                    cx,
                    space_name_id,
                );
            }
            Some(SelectedRoom::Thread { room_name_id, thread_root_event_id }) => {
                widget.as_room_screen().set_displayed_room(
                    cx,
                    room_name_id,
                    Some(thread_root_event_id.clone()),
                );
            }
            None => { }
        }
    }

    /// Lazily initializes a tab's widget if it hasn't been initialized yet.
    ///
    /// This is called when a tab becomes visible (e.g., via user click or sidebar selection)
    /// that was restored from saved state but whose widget content was deferred
    /// to avoid blocking the UI thread.
    ///
    /// It is safe to call this on an already-initialized tab, as the underlying
    /// `set_displayed_*` methods short-circuit when the content is already set.
    fn init_tab_if_needed(&self, cx: &mut Cx, tab_id: LiveId) {
        if !self.open_rooms.contains_key(&tab_id) {
            return;
        }
        let widget = self.view.dock(cx, ids!(dock)).item(tab_id);
        if !widget.is_empty() {
            Self::init_tab_widget(cx, &self.open_rooms, &tab_id, &widget);
        }
    }

    /// Initializes all currently-visible (selected-in-their-pane) tabs that
    /// haven't been initialized yet.
    ///
    /// This is useful after operations that may change which tabs are visible
    /// without going through explicit tab selection (e.g., closing a tab causes
    /// the dock to auto-select an adjacent tab).
    fn init_all_visible_tabs(&self, cx: &mut Cx) {
        let dock = self.view.dock(cx, ids!(dock));
        let Some(mut dock) = dock.borrow_mut() else { return };
        for (tab_id, widget) in dock.visible_items() {
            Self::init_tab_widget(cx, &self.open_rooms, &tab_id, &widget);
        }
    }
}

impl WidgetMatchEvent for MainDesktopUI {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, scope: &mut Scope) {
        let mut should_save_dock_action: bool = false;
        for action in actions {
            // Explicit navigation wins over a delayed restored Room classification.
            if action.downcast_ref::<HagencyAction>().is_some()
                || matches!(action.downcast_ref::<MainDesktopUiAction>(), Some(MainDesktopUiAction::ShowGroupChats)) {
                self.restored_context = None;
            }
            if let Some(navigation) = action.downcast_ref::<crate::hagency::ui::ProjectRoomNavigation>()
                && navigation.current() {self.restored_context = None;}
            let widget_action = action.as_widget_action();
            if matches!(action.downcast_ref::<MainDesktopUiAction>(), Some(MainDesktopUiAction::ShowGroupChats))
                || matches!(action.downcast_ref::<HagencyAction>(), Some(HagencyAction::Close)) {
                if self.selected_space.is_some() {
                    let state = scope.data.get_mut::<AppState>().unwrap();
                    self.switch_dock_to_space(cx, state, None);
                }
                {
                    let dock = self.view.dock(cx, ids!(dock));
                    let tab = id!(group_chats_welcome_tab);
                    if dock.item(tab).is_empty() {
                        if let Some((bar, _)) = dock.find_tab_bar_of_tab(id!(home_tab)) {
                            dock.create_and_select_tab(cx, bar, tab, id!(group_chats_welcome), crate::i18n::tr("Group Chats").to_owned(), id!(CloseableTab), None);
                        }
                    } else {dock.select_tab(cx, tab);}
                    cx.action(AppStateAction::FocusNone);
                    self.most_recently_selected_room = None;
                }
                self.redraw(cx);
            }
            if let Some(action) = action.downcast_ref::<HagencyAction>() {
                // Management lives in the permanent home pane; the Project tree
                // and the active account stay visible beside it.
                if !matches!(action, HagencyAction::Close) {
                    let app_state = scope.data.get_mut::<AppState>().unwrap();
                    if self.selected_space.is_some() {
                        self.switch_dock_to_space(cx, app_state, None);
                    }
                    cx.action(NavigationBarAction::GoToHome);
                    self.select_room(cx, None);
                    let panel = self.view.dock(cx, ids!(dock)).item(id!(home_tab));
                    panel.as_hagency_panel().action(cx, action);
                }
                self.redraw(cx);
                continue;
            }
            if matches!(action.downcast_ref(),Some(NavigationBarAction::GoToHome | NavigationBarAction::GoToTab(SelectedTab::Home)))
                || matches!(action.downcast_ref(),Some(HagencyAction::OpenProjects)) {self.matrix_link_root=None;}
            if let Some(crate::matrix_links::LinkAction::EnterHome(navigation)) = action.downcast_ref()
                && navigation.is_current() && !self.drawn_previously {self.matrix_link_root=Some(navigation.ticket.clone());}
            if let Some(crate::matrix_links::LinkAction::Navigate(navigation)) = action.downcast_ref() {
                if !navigation.is_current() {continue;}
                if crate::sliding_sync::get_client().and_then(|client|client.get_room(navigation.selected.room_id()))
                    .is_some_and(|room|!room.direct_targets().is_empty()) {
                    super::mobile::navigate_matrix_direct(cx, navigation.clone());
                    continue;
                }
                if matches!(navigation.selected, SelectedRoom::Space {..}) {
                    self.focus_or_create_tab(cx, navigation.selected.clone());
                    continue;
                }
                let tab = navigation.selected.tab_id();
                self.focus_or_create_tab(cx, navigation.selected.clone());
                if let Some(event) = &navigation.event {
                    self.view.dock(cx, ids!(dock)).item(tab).as_room_screen()
                        .jump_to_matrix_event(cx, event.clone(), navigation.ticket.clone());
                }
            }
            if let Some(super::room_history::RoomHistoryAction::Jump {room, event}) = action.downcast_ref() {
                if crate::sliding_sync::get_client().and_then(|c|c.get_room(room.room_id())).is_some_and(|r|!r.direct_targets().is_empty()) {
                    super::mobile::navigate_direct_view(cx,room.clone(),None,Some(event.clone()));continue;
                }
                let selected = SelectedRoom::JoinedRoom {room_name_id: room.clone()};
                let tab = selected.tab_id();
                self.focus_or_create_tab(cx, selected);
                self.view.dock(cx, ids!(dock)).item(tab).as_room_screen().jump_to_history_event(cx, event.clone());
            }

            if let Some(AppPreferencesAction::TabbedChatsChanged(tabbed)) = action.downcast_ref() {
                // Before the first draw, the dock hasn't been loaded yet;
                // `draw_walk()` picks up the preference when it loads the dock.
                if self.drawn_previously && *tabbed != self.tabbed_chats {
                    self.set_tabbed_chats(cx, *tabbed);
                }
                continue;
            }

            if let Some(crate::logout::logout_confirm_modal::LogoutAction::ClearAppState { .. }) = action.downcast_ref() {
                let dock = self.view.dock(cx, ids!(dock));
                for (tab, room) in self.open_rooms.drain() {
                    room.close_thread_timeline(cx);
                    dock.close_tab(cx, tab);
                }
                self.room_order.clear();
                self.most_recently_selected_room = None;
                self.selected_space = None;
                self.drawn_previously = false;
                self.restored_context = None;
                dock.select_tab(cx, id!(home_tab));
                self.redraw(cx);
                continue;
            }
            if let Some(MainDesktopUiAction::CloseAllTabs { on_close_all }) = action.downcast_ref() {
                self.close_all_tabs(cx);
                on_close_all.notify_one();
                continue;
            }
            if matches!(action.downcast_ref(), Some(MainDesktopUiAction::CloseCurrentTab)) {
                if self.close_current_tab(cx) {
                    self.redraw(cx);
                    should_save_dock_action = true;
                }
                continue;
            }

            // An invited space's InviteScreen should be shown in the main home dock.
            // We switch to that dock directly, here, instead of handling it as a `GoToHome` action,
            // because it's instant, and then we can create the new InviteScreen tab immediately.
            if let SpacesBarAction::InvitedSpaceClicked { space_name_id } = widget_action.cast() {
                if self.selected_space.is_some() {
                    let app_state = scope.data.get_mut::<AppState>().unwrap();
                    self.switch_dock_to_space(cx, app_state, None);
                }
                cx.action(NavigationBarAction::GoToHome);
                self.focus_or_create_tab(cx, SelectedRoom::InvitedRoom { room_name_id: space_name_id });
                continue;
            }

            // If the currently-selected space has been changed, we must handle that
            // by switching the dock to show the layout for another space.
            if let Some(NavigationBarAction::TabSelected(tab)) = action.downcast_ref() {
                let new_space = match (tab, self.selected_space.as_ref()) {
                    (SelectedTab::Space { space_name_id }, space_id_opt)
                        if space_id_opt.is_none_or(|id| id != space_name_id.room_id()) => 
                    {
                        Some(space_name_id.room_id().clone())
                    }
                    (SelectedTab::Home, Some(_)) => None,
                    _ => continue,
                };
                let app_state = scope.data.get_mut::<AppState>().unwrap();
                self.switch_dock_to_space(cx, app_state, new_space);
                self.redraw(cx);
                continue;
            }

            // Handle actions emitted by the dock within the MainDesktopUI
            match widget_action.cast() {
                // Whenever a tab (except for the home_tab) is pressed, notify the app state.
                DockAction::TabWasPressed(tab_id) => {
                    if tab_id == id!(home_tab) {
                        self.select_room(cx, None);
                    }
                    else if let Some(selected_room) = self.open_rooms.get(&tab_id).cloned() {
                        self.select_room(cx, Some(selected_room));
                    }
                    // Lazily initialize this tab's widget if it was deferred during dock restoration.
                    self.init_tab_if_needed(cx, tab_id);
                    should_save_dock_action = true;
                }
                DockAction::TabCloseWasPressed(tab_id) => {
                    self.close_tab(cx, tab_id);
                    self.redraw(cx);
                    should_save_dock_action = true;
                }
                // When dragging a tab, allow it to be dragged
                DockAction::ShouldTabStartDrag(tab_id) => {
                    self.view.dock(cx, ids!(dock)).tab_start_drag(
                        cx,
                        tab_id,
                        DragItem::FilePath {
                            path: "".to_string(),
                            internal_id: Some(tab_id),
                        },
                    );
                }
                // When dragging a tab, allow it to be dragged
                DockAction::Drag(drag_event) if drag_event.items.len() == 1 => {
                    self.view.dock(cx, ids!(dock)).accept_drag(cx, drag_event, DragResponse::Move);
                }
                // When dropping a tab, move it to the new position
                DockAction::Drop(drop_event) => {
                    // from inside the dock, otherwise it's an external file
                    if let DragItem::FilePath {
                        internal_id: Some(internal_id),
                        ..
                    } = &drop_event.items[0] {
                        self.view.dock(cx, ids!(dock)).drop_move(cx, drop_event.abs, *internal_id);
                    }
                    // A drag-drop may create a new split pane, revealing an
                    // uninitialized tab that was deferred during dock restoration.
                    self.init_all_visible_tabs(cx);
                    should_save_dock_action = true;
                }
                _ => (),
            }

            // Handle RoomsList actions, which are updates from the rooms list.
            match widget_action.cast_ref() {
                RoomsListAction::Selected(selected_room) => {
                    if super::mobile::is_direct_selection(cx,selected_room) {
                        let thread=if let SelectedRoom::Thread {thread_root_event_id,..}=selected_room {Some(thread_root_event_id.clone())}else{None};
                        super::mobile::navigate_direct_view(cx,selected_room.room_name().clone(),thread,None);
                        continue;
                    }
                    // Contacts can open a DM or a group while the chat dock is
                    // covered. Restore the home dock before selecting that room.
                    let from_contacts = scope.data.get::<AppState>().is_some_and(|app| app.selected_tab == SelectedTab::Contacts);
                    if from_contacts {
                        if self.selected_space.is_some() {
                            let app_state = scope.data.get_mut::<AppState>().unwrap();
                            self.switch_dock_to_space(cx, app_state, None);
                        }
                        cx.action(NavigationBarAction::GoToHome);
                        cx.action(MainDesktopUiAction::ShowGroupChats);
                    }
                    // Note that this cannot be performed within draw_walk() as the draw flow prevents from
                    // performing actions that would trigger a redraw, and the Dock internally performs (and expects)
                    // a redraw to be happening in order to draw the tab content.
                    self.focus_or_create_tab(cx, selected_room.clone());
                }
                RoomsListAction::InviteAccepted { room_name_id, kind } => {
                    if !kind.is_space() && scope.data.get::<AppState>().is_some_and(|app|app.selected_tab==SelectedTab::Contacts && app.selected_room.as_ref().is_some_and(|r|r.room_id()==room_name_id.room_id())) {
                        super::mobile::navigate_direct(cx,room_name_id.clone());
                        continue;
                    }
                    // A space's SpaceLobbyScreen should be shown in its top-level ancestor space's dock
                    // (assuming the user is still on that space's InviteScreen).
                    let space = SelectedRoom::Space { space_name_id: room_name_id.clone() };
                    if let AcceptedInviteKind::Space { dock_space: Some(dock_space) } = kind
                        && self.open_rooms.contains_key(&space.tab_id())
                        && self.most_recently_selected_room.as_ref()
                            .is_some_and(|sr| sr.room_id() == room_name_id.room_id())
                    {
                        self.close_tab(cx, space.tab_id());
                        self.upgrade_open_invites(cx, room_name_id, true);
                        let app_state = scope.data.get_mut::<AppState>().unwrap();
                        self.switch_dock_to_space(cx, app_state, Some(dock_space.room_id().clone()));
                        cx.action(NavigationBarAction::GoToSpace { space_name_id: dock_space.clone() });
                        self.focus_or_create_tab(cx, space);
                    } else {
                        self.replace_invite_with_joined(cx, room_name_id, kind.is_space());
                    }
                }
                RoomsListAction::OpenRoomContextMenu { .. } => {}
                RoomsListAction::None => { }
            }

            // Handle potential changes to room names.
            if let Some(
                AppStateAction::RoomNameUpdated(new_room_name)
                | AppStateAction::RoomLoadedSuccessfully { room_name_id: new_room_name, .. }
            ) = action.downcast_ref() {
                let dock = self.view.dock(cx, ids!(dock));
                for (tab_id, room) in self.open_rooms.iter_mut() {
                    if room.update_room_name(new_room_name) {
                        dock.set_tab_title(cx, *tab_id, room.display_name());
                    }
                }
                for room in self.most_recently_selected_room
                    .iter_mut()
                    .chain(self.room_order.iter_mut())
                {
                    room.update_room_name(new_room_name);
                }
                continue;
            }

            // Handle our own actions related to dock updates that we have previously emitted.
            match action.downcast_ref() {
                Some(MainDesktopUiAction::LoadDockFromAppState) => {
                    let app_state = scope.data.get_mut::<AppState>().unwrap();
                    // This records who requested initialization, not whether
                    // navigation remains current. Retirement cannot authorize an
                    // old passive LoadDock to enter the management factory.
                    let permalink_initialization=self.matrix_link_root.take().is_some();
                    self.load_dock_state_from(cx, app_state);
                    // A permalink initializes the dock to open its exact Room,
                    // not to enter the management factory as an intermediate page.
                    if self.most_recently_selected_room.is_none()
                        && !permalink_initialization {
                        self.view.dock(cx, ids!(dock)).item(id!(home_tab))
                            .as_hagency_panel().action(cx, &HagencyAction::OpenProjects);
                    }
                }
                Some(MainDesktopUiAction::SaveDockIntoAppState) => {
                    let app_state = scope.data.get_mut::<AppState>().unwrap();
                    self.save_dock_state_to(cx, app_state);
                }
                _ => {}
            }
        }

        if should_save_dock_action {
            cx.action(MainDesktopUiAction::SaveDockIntoAppState);
        }
    }
}

/// Actions sent to the MainDesktopUI widget for saving/restoring its dock state.
#[derive(Debug)]
pub enum MainDesktopUiAction {
    /// Save the state of the dock into the AppState.
    SaveDockIntoAppState,
    /// Select the most recent chat, or the chat welcome pane when none exist.
    ShowGroupChats,
    /// Load the room panel state from the AppState to the dock.
    LoadDockFromAppState,
    /// Close the current room tab (Ctrl/Cmd+W); see [`MainDesktopUI::close_current_tab()`].
    CloseCurrentTab,
    /// Close all tabs; see [`MainDesktopUI::close_all_tabs()`]
    CloseAllTabs {
        on_close_all: Arc<Notify>,
    },
}

/// Saved dock state validation, repair, and helpers for translating
/// `SelectedRoom`s into dock items.
mod dock_state_repair {
    use super::*;

    /// Checks the given saved dock state and makes repairs in place.
    ///
    /// Removes orphaned, duplicated, and dangling dock item references while preserving
    /// all open rooms that can still be reconstructed.
    ///
    /// * Returns `Ok` if we successfully repaired the dock state (or it was already good),
    /// * Returns `Err` is the state is unrepairable, meaning we should fall back to the default.
    pub(super) fn validate_and_repair_dock_state(saved: &mut SavedDockState) -> Result<bool, &'static str> {
        let root = id!(root);
        let home_tab = id!(home_tab);
        let main_tabs_dsl = id!(main_tabs);
        let rooms_sidebar_tab = id!(rooms_sidebar_tab);
        let rooms_sidebar_tabs = id!(rooms_sidebar_tabs);

        let mut repaired = false;

        repaired |= ensure_fixed_tab_item(
            saved,
            home_tab,
            DockItem::Tab {
                name: "Projects".to_string(),
                template: id!(PermanentTab),
                kind: id!(hagency_panel),
            },
        )?;
        // The navigation sidebar now belongs to HomeScreen, outside the Dock.
        // Remove only its old fixed tab, retaining all chat and user tab content.
        for item in saved.dock_items.values_mut() {
            if let DockItem::Tabs {tabs, selected, ..} = item {
                let before = tabs.len();
                tabs.retain(|id| *id != rooms_sidebar_tab);
                repaired |= tabs.len() != before;
                *selected = (*selected).min(tabs.len().saturating_sub(1));
            }
        }
        repaired |= saved.dock_items.remove(&rooms_sidebar_tab).is_some();
        if matches!(saved.dock_items.get(&rooms_sidebar_tabs), Some(DockItem::Tabs {tabs, ..}) if tabs.is_empty()) {
            collapse_tabs_container(saved, rooms_sidebar_tabs, main_tabs_dsl, rooms_sidebar_tabs)?;
            repaired = true;
        }
        if !any_tabs_contains(&saved.dock_items, home_tab) {
            let target = if matches!(saved.dock_items.get(&root), Some(DockItem::Tabs {..})) {root} else {main_tabs_dsl};
            repaired |= ensure_tab_ref_in_tabs_container(saved, target, home_tab)?;
        }

        // Rebuild missing bookkeeping from the more redundant fields. Older corrupt
        // snapshots often have a valid room_order but an incomplete open_rooms map.
        let known_rooms: Vec<SelectedRoom> = saved.room_order.iter()
            .chain(saved.selected_room.iter())
            .cloned()
            .collect();
        for room in known_rooms {
            let tab_id = room.tab_id();
            if saved.open_rooms.get(&tab_id) != Some(&room) {
                saved.open_rooms.insert(tab_id, room);
                repaired = true;
            }
        }
        if let Some(sr) = saved.selected_room.clone()
            && !saved.room_order.iter().any(|r| r == &sr)
        {
            saved.room_order.push(sr);
            repaired = true;
        }

        // If an open room is still known semantically but its DockItem::Tab was lost,
        // recreate that tab item so it can be rescued into the main tab bar.
        for (&tab_id, room) in saved.open_rooms.iter() {
            match saved.dock_items.get(&tab_id) {
                Some(DockItem::Tab { .. }) => {}
                None => {
                    saved.dock_items.insert(tab_id, dock_tab_for_selected_room(room));
                    repaired = true;
                }
                Some(_) => return Err("open room tab id collides with a dock container"),
            }
        }

        let (mut tree_order, mut reachable) = walk_dock_tree(&saved.dock_items)?;

        // The main-tabs container is the first reachable `Tabs` (in DFS order) that
        // holds `home_tab`.
        let Some(main_tabs_id) = find_tabs_containing(&saved.dock_items, &tree_order, home_tab) else {
            return Err("no reachable main-tabs container (holding home_tab) was found");
        };
        let mut main_tabs_id = main_tabs_id;
        if main_tabs_id != root && main_tabs_id != main_tabs_dsl {
            normalize_main_tabs_id(saved, main_tabs_id, main_tabs_dsl, &reachable)?;
            main_tabs_id = main_tabs_dsl;
            repaired = true;
            (tree_order, _) = walk_dock_tree(&saved.dock_items)?;
        }

        // Dedupe in tree (DFS) order so the root-most reference wins. Otherwise
        // find_tab_bar_of_tab picks an arbitrary HashMap entry and new tabs can
        // land in the wrong container.
        let mut referenced_tabs = HashSet::new();
        repaired |= remove_bad_and_duplicate_tab_refs(saved, &tree_order, &mut referenced_tabs);

        // Rescue open room tabs that are no longer referenced by any reachable Tabs
        // container. Recreated missing DockItem::Tab entries from above are handled here.
        repaired |= append_unreferenced_open_rooms_to_main_tabs(saved, main_tabs_id, &mut referenced_tabs);
        (tree_order, reachable) = walk_dock_tree(&saved.dock_items)?;

        // Collapse reachable empty Tabs out of the tree. Dedupe can strip the only
        // tab from a container; if we leave it in place, Makepad renders it as an
        // empty pane. Keep the home-holding container; its ID may become root.
        loop {
            let empty_id = tree_order.iter()
                .find(|id| **id != root
                    && **id != main_tabs_id
                    && matches!(
                        saved.dock_items.get(*id),
                        Some(DockItem::Tabs { tabs, .. }) if tabs.is_empty()
                    )
                )
                .copied();
            let Some(empty_id) = empty_id else { break };
            collapse_tabs_container(saved, empty_id, main_tabs_id, rooms_sidebar_tabs)?;
            repaired = true;
            (tree_order, reachable) = walk_dock_tree(&saved.dock_items)?;
            main_tabs_id = find_tabs_containing(&saved.dock_items, &tree_order, home_tab)
                .ok_or("main tabs missing after empty pane collapse")?;
        }

        // Drop anything still unreachable, such as orphaned Tabs/Splitter items.
        let len_before = saved.dock_items.len();
        saved.dock_items.retain(|id, _| reachable.contains(id));
        if saved.dock_items.len() != len_before {
            repaired = true;
            (tree_order, _) = walk_dock_tree(&saved.dock_items)?;
        }

        if find_tabs_containing(&saved.dock_items, &tree_order, home_tab) != Some(main_tabs_id) {
            return Err("home_tab is not in the reachable main-tabs container");
        }

        // Collect the post-cleanup set of tab IDs reachable from the tree, used to
        // prune open_rooms below. No new dedupe needed; collapse/drop only remove
        // containers, they can't introduce duplicate tab refs.
        referenced_tabs.clear();
        for &id in &tree_order {
            if let Some(DockItem::Tabs { tabs, .. }) = saved.dock_items.get(&id) {
                referenced_tabs.extend(tabs.iter().copied());
            }
        }

        // Adjust the indices for the selected room.
        if let Some(sel_id) = saved.selected_room.as_ref().map(|sr| sr.tab_id())
            && let Some(DockItem::Tabs { tabs, selected, .. }) =
                saved.dock_items.get_mut(&main_tabs_id)
            && let Some(idx) = tabs.iter().position(|t| *t == sel_id)
            && *selected != idx
        {
            *selected = idx;
            repaired = true;
        }

        let len_before = saved.open_rooms.len();
        saved.open_rooms.retain(|id, _| referenced_tabs.contains(id));
        repaired |= saved.open_rooms.len() != len_before;

        // Drop room_order entries whose tab is gone or duplicated, then append any
        // open_rooms entries that aren't in the order yet (sorted by tab id for stability).
        let mut seen_room_tabs = HashSet::with_capacity(saved.room_order.len());
        let len_before = saved.room_order.len();
        saved.room_order.retain(|sr| {
            saved.open_rooms.contains_key(&sr.tab_id()) && seen_room_tabs.insert(sr.tab_id())
        });
        let mut missing: Vec<SelectedRoom> = saved.open_rooms.values()
            .filter(|r| !seen_room_tabs.contains(&r.tab_id()))
            .cloned()
            .collect();
        missing.sort_by_key(|r| r.tab_id().0);
        repaired |= saved.room_order.len() != len_before || !missing.is_empty();
        saved.room_order.extend(missing);

        repaired |= saved.selected_room
            .take_if(|sr| !saved.open_rooms.contains_key(&sr.tab_id()))
            .is_some();

        Ok(repaired)
    }

    fn dock_tab_for_selected_room(room: &SelectedRoom) -> DockItem {
        DockItem::Tab {
            name: room.display_name(),
            template: id!(CloseableTab),
            kind: room.dock_kind(),
        }
    }

    fn ensure_fixed_tab_item(
        saved: &mut SavedDockState,
        tab_id: LiveId,
        tab_item: DockItem,
    ) -> Result<bool, &'static str> {
        match saved.dock_items.get(&tab_id) {
            Some(DockItem::Tab { .. }) => Ok(false),
            None => {
                saved.dock_items.insert(tab_id, tab_item);
                Ok(true)
            }
            Some(_) => Err("fixed tab id collides with a dock container"),
        }
    }

    fn any_tabs_contains(dock_items: &HashMap<LiveId, DockItem>, tab_id: LiveId) -> bool {
        dock_items.values().any(|item| {
            matches!(item, DockItem::Tabs { tabs, .. } if tabs.contains(&tab_id))
        })
    }

    fn ensure_tab_ref_in_tabs_container(
        saved: &mut SavedDockState,
        tabs_id: LiveId,
        tab_id: LiveId,
    ) -> Result<bool, &'static str> {
        match saved.dock_items.get_mut(&tabs_id) {
            Some(DockItem::Tabs { tabs, selected, .. }) => {
                if tabs.contains(&tab_id) {
                    return Ok(false);
                }
                if !tabs.is_empty() {
                    *selected = selected.saturating_add(1);
                }
                tabs.insert(0, tab_id);
                Ok(true)
            }
            Some(DockItem::Tab { .. }) | Some(DockItem::Splitter { .. }) => {
                Err("fixed tabs container id collides with another dock item")
            }
            None => Ok(false),
        }
    }

    fn walk_dock_tree(
        dock_items: &HashMap<LiveId, DockItem>,
    ) -> Result<(Vec<LiveId>, HashSet<LiveId>), &'static str> {
        fn visit(
            id: LiveId,
            dock_items: &HashMap<LiveId, DockItem>,
            order: &mut Vec<LiveId>,
            reachable: &mut HashSet<LiveId>,
            visiting: &mut HashSet<LiveId>,
        ) -> Result<(), &'static str> {
            let Some(item) = dock_items.get(&id) else {
                return Err("dock item reference is missing");
            };
            if visiting.contains(&id) {
                return Err("dock item graph contains a cycle");
            }
            if reachable.contains(&id) {
                return if matches!(item, DockItem::Tab { .. }) {
                    Ok(())
                } else {
                    Err("dock container is referenced more than once")
                };
            }

            reachable.insert(id);
            visiting.insert(id);
            order.push(id);
            match item {
                DockItem::Splitter { a, b, .. } => {
                    if a == b {
                        return Err("splitter references the same child twice");
                    }
                    visit(*a, dock_items, order, reachable, visiting)?;
                    visit(*b, dock_items, order, reachable, visiting)?;
                }
                DockItem::Tabs { tabs, .. } => {
                    for tab_id in tabs {
                        if matches!(dock_items.get(tab_id), Some(DockItem::Tab { .. })) {
                            visit(*tab_id, dock_items, order, reachable, visiting)?;
                        }
                    }
                }
                DockItem::Tab { .. } => {}
            }
            visiting.remove(&id);
            Ok(())
        }

        if !dock_items.contains_key(&id!(root)) {
            return Err("root dock item is missing");
        }

        let mut order = Vec::with_capacity(dock_items.len());
        let mut reachable = HashSet::with_capacity(dock_items.len());
        let mut visiting = HashSet::with_capacity(dock_items.len());
        visit(id!(root), dock_items, &mut order, &mut reachable, &mut visiting)?;
        Ok((order, reachable))
    }

    fn find_tabs_containing(
        dock_items: &HashMap<LiveId, DockItem>,
        tree_order: &[LiveId],
        tab_id: LiveId,
    ) -> Option<LiveId> {
        tree_order.iter().find_map(|id| {
            if let Some(DockItem::Tabs { tabs, .. }) = dock_items.get(id) {
                if tabs.contains(&tab_id) {
                    return Some(*id);
                }
            }
            None
        })
    }

    fn normalize_main_tabs_id(
        saved: &mut SavedDockState,
        current_main_tabs_id: LiveId,
        main_tabs_dsl: LiveId,
        reachable: &HashSet<LiveId>,
    ) -> Result<(), &'static str> {
        if current_main_tabs_id == id!(root) {
            return Err("main-tabs container is root");
        }
        let Some(current_main_tabs) = saved.dock_items.remove(&current_main_tabs_id) else {
            return Err("main-tabs container is missing");
        };
        let existing_dsl_item = saved.dock_items.remove(&main_tabs_dsl);
        let existing_dsl_was_reachable = existing_dsl_item.as_ref().is_some_and(|item| {
            reachable.contains(&main_tabs_dsl) && !matches!(item, DockItem::Tab { .. })
        });

        saved.dock_items.insert(main_tabs_dsl, current_main_tabs);
        if existing_dsl_was_reachable
            && let Some(existing_dsl_item) = existing_dsl_item
        {
            saved.dock_items.insert(current_main_tabs_id, existing_dsl_item);
        }

        // Rewrite splitter refs: when the existing dsl entry was reachable we swap
        // the two IDs everywhere; otherwise we just point current -> dsl.
        let remap = |id: &mut LiveId| {
            if *id == current_main_tabs_id {
                *id = main_tabs_dsl;
            } else if existing_dsl_was_reachable && *id == main_tabs_dsl {
                *id = current_main_tabs_id;
            }
        };
        for item in saved.dock_items.values_mut() {
            if let DockItem::Splitter { a, b, .. } = item {
                remap(a);
                remap(b);
            }
        }

        Ok(())
    }

    fn remove_bad_and_duplicate_tab_refs(
        saved: &mut SavedDockState,
        tree_order: &[LiveId],
        referenced_tabs: &mut HashSet<LiveId>,
    ) -> bool {
        let valid_tabs: HashSet<LiveId> = saved.dock_items.iter()
            .filter_map(|(id, item)| matches!(item, DockItem::Tab { .. }).then_some(*id))
            .collect();
        let mut seen = HashSet::with_capacity(valid_tabs.len());
        let mut repaired = false;
        for &id in tree_order {
            if let Some(DockItem::Tabs { tabs, selected, .. }) = saved.dock_items.get_mut(&id) {
                let original_len = tabs.len();
                tabs.retain(|tab_id| {
                    valid_tabs.contains(tab_id)
                        && seen.insert(*tab_id)
                        && {
                            referenced_tabs.insert(*tab_id);
                            true
                        }
                });
                if tabs.len() != original_len {
                    repaired = true;
                }
                let new_selected = if tabs.is_empty() {
                    0
                } else if *selected >= tabs.len() {
                    tabs.len() - 1
                } else {
                    *selected
                };
                if *selected != new_selected {
                    *selected = new_selected;
                    repaired = true;
                }
            }
        }
        repaired
    }

    fn append_unreferenced_open_rooms_to_main_tabs(
        saved: &mut SavedDockState,
        main_tabs_id: LiveId,
        referenced_tabs: &mut HashSet<LiveId>,
    ) -> bool {
        let mut repaired = false;
        let mut candidates = Vec::with_capacity(saved.room_order.len() + saved.open_rooms.len());
        let mut candidate_ids = HashSet::with_capacity(saved.room_order.len() + saved.open_rooms.len());
        for room in saved.room_order.iter() {
            let tab_id = room.tab_id();
            if candidate_ids.insert(tab_id) {
                candidates.push(tab_id);
            }
        }
        let mut unordered: Vec<LiveId> = saved.open_rooms.keys()
            .filter(|tab_id| candidate_ids.insert(**tab_id))
            .copied()
            .collect();
        unordered.sort_by_key(|tab_id| tab_id.0);
        candidates.extend(unordered);

        let candidates: Vec<LiveId> = candidates.into_iter()
            .filter(|tab_id| matches!(saved.dock_items.get(tab_id), Some(DockItem::Tab { .. })))
            .collect();
        if let Some(DockItem::Tabs { tabs, selected, .. }) = saved.dock_items.get_mut(&main_tabs_id) {
            for tab_id in candidates {
                if referenced_tabs.contains(&tab_id) {
                    continue;
                }
                tabs.push(tab_id);
                referenced_tabs.insert(tab_id);
                repaired = true;
            }
            if *selected >= tabs.len() && !tabs.is_empty() {
                *selected = tabs.len() - 1;
                repaired = true;
            }
        }
        repaired
    }

    fn collapse_tabs_container(
        saved: &mut SavedDockState,
        tabs_id: LiveId,
        _main_tabs_id: LiveId,
        _rooms_sidebar_tabs_id: LiveId,
    ) -> Result<(), &'static str> {
        let root = id!(root);
        let Some((parent_split, sibling)) = saved.dock_items.iter().find_map(|(sid, item)| {
            if let DockItem::Splitter { a, b, .. } = item {
                if *a == tabs_id {
                    return Some((*sid, *b));
                }
                if *b == tabs_id {
                    return Some((*sid, *a));
                }
            }
            None
        }) else {
            saved.dock_items.remove(&tabs_id);
            return Ok(());
        };

        if parent_split == root {
            let Some(sibling_content) = saved.dock_items.remove(&sibling) else {
                return Err("empty pane sibling is missing");
            };
            saved.dock_items.insert(root, sibling_content);
        } else {
            if !saved.dock_items.contains_key(&sibling) {
                return Err("empty pane sibling is missing");
            }
            if !replace_splitter_refs(&mut saved.dock_items, parent_split, sibling) {
                return Err("empty pane parent splitter is detached");
            }
            saved.dock_items.remove(&parent_split);
        }
        saved.dock_items.remove(&tabs_id);
        Ok(())
    }

    fn replace_splitter_refs(
        dock_items: &mut HashMap<LiveId, DockItem>,
        from: LiveId,
        to: LiveId,
    ) -> bool {
        let mut replaced = false;
        for item in dock_items.values_mut() {
            if let DockItem::Splitter { a, b, .. } = item {
                if *a == from { *a = to; replaced = true; }
                if *b == from { *b = to; replaced = true; }
            }
        }
        replaced
    }

}

#[cfg(test)]
pub(crate) mod persistent_navigation_tests {
    use super::*;
    pub(crate) fn has_matrix_link_initialization(widget:&WidgetRef)->bool {
        widget.as_main_desktop_ui().borrow().expect("original desktop UI").matrix_link_root.is_some()
    }
    pub(crate) fn matrix_link_navigation(navigation: crate::matrix_links::Navigation) {
        let ticket=navigation.ticket.clone();
        matrix_link_navigation_then(navigation,move |_|ticket.cancel());
    }
    pub(crate) fn matrix_link_navigation_then(navigation: crate::matrix_links::Navigation, replace: impl FnOnce(&mut Cx)) {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        let mut ui=cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);
            let value=script_eval!(vm,{mod.widgets.MainDesktopUI {}});
            MainDesktopUI::script_from_value(vm,value)
        });
        let mut state=AppState::default();
        super::super::mobile::new_chat_tests::draw_with_state(&mut cx,&mut ui,1280.0,&mut state);
        let actions=cx.capture_actions(|cx|cx.action(crate::matrix_links::LinkAction::Navigate(navigation.clone())));
        ui.handle_event(&mut cx,&Event::Actions(actions),&mut Scope::with_data(&mut state));
        super::super::mobile::new_chat_tests::draw_with_state(&mut cx,&mut ui,1280.0,&mut state);
        let screen=ui.view.dock(&cx,ids!(dock)).item(navigation.selected.tab_id()).as_room_screen();
        {
            let mut target=screen.borrow_mut().expect("selected native RoomScreen");
            super::super::mobile::new_chat_tests::draw_with_state(&mut cx,&mut *target,1280.0,&mut state);
        }
        screen.assert_matrix_link_target(&cx,&navigation);
        replace(&mut cx);
        screen.handle_event(&mut cx,&Event::Signal,&mut Scope::with_data(&mut state));
        screen.assert_matrix_link_cancelled();
    }
    fn room(name: &str) -> SelectedRoom {
        SelectedRoom::JoinedRoom {room_name_id: RoomNameId::new(matrix_sdk::RoomDisplayName::Named(name.into()), format!("!{name}:example.org").try_into().unwrap())}
    }
    fn tabs(ids: Vec<LiveId>) -> DockItem {DockItem::Tabs {tabs: ids, selected: 0, closable: true, hide_tab_bar: false}}
    fn tab(kind: LiveId) -> DockItem {DockItem::Tab {name: "Fixture".into(), template: id!(CloseableTab), kind}}
    #[test]
    fn restored_direct_room_selects_chat_context_without_replacing_room_or_project_dock() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut ui = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.MainDesktopUI {}});
            MainDesktopUI::script_from_value(vm, value)
        });
        let selected = room("restored"); let epoch = crate::account_session::epoch();
        ui.most_recently_selected_room = Some(selected.clone());
        ui.selected_space = Some("!project:example.org".try_into().unwrap());
        *cx.global::<super::super::rooms_sidebar::DesktopWorkspace>() = super::super::rooms_sidebar::DesktopWorkspace::Projects;
        ui.restored_context = Some(RestoredRoomContext {room:selected.room_id().clone(),epoch});
        ui.apply_restored_context(&mut cx, None, epoch);
        assert!(ui.restored_context.is_some(), "wait for exact Room metadata instead of guessing from its name");
        let actions=cx.capture_actions(|cx|ui.apply_restored_context(cx,Some(true),epoch));
        assert!(actions.iter().any(|a|matches!(a.downcast_ref(),Some(NavigationBarAction::GoToTab(SelectedTab::Contacts)))));
        assert_eq!(*cx.global::<super::super::rooms_sidebar::DesktopWorkspace>(), super::super::rooms_sidebar::DesktopWorkspace::Projects);
        assert_eq!(ui.most_recently_selected_room, Some(selected.clone()));
        assert!(ui.selected_space.is_some(), "classification must not reload or replace a saved dock");
        *cx.global::<super::super::rooms_sidebar::DesktopWorkspace>() = super::super::rooms_sidebar::DesktopWorkspace::Projects;
        ui.restored_context = Some(RestoredRoomContext {room:selected.room_id().clone(),epoch});
        ui.apply_restored_context(&mut cx, Some(false), epoch);
        assert_eq!(*cx.global::<super::super::rooms_sidebar::DesktopWorkspace>(), super::super::rooms_sidebar::DesktopWorkspace::Projects);
        ui.restored_context = Some(RestoredRoomContext {room:selected.room_id().clone(),epoch});
        ui.apply_restored_context(&mut cx, Some(true), epoch.wrapping_add(1));
        assert!(ui.restored_context.is_none());
        assert_eq!(*cx.global::<super::super::rooms_sidebar::DesktopWorkspace>(), super::super::rooms_sidebar::DesktopWorkspace::Projects, "same Room ID from another account epoch cannot steal navigation");
        ui.restored_context = Some(RestoredRoomContext {room:selected.room_id().clone(),epoch});
        let actions = cx.capture_actions(|cx| cx.action(HagencyAction::OpenProjects));
        ui.handle_actions(&mut cx, &actions, &mut Scope::with_data(&mut AppState::default()));
        assert!(ui.restored_context.is_none(), "explicit Projects navigation cancels delayed restoration");
        ui.apply_restored_context(&mut cx, Some(true), epoch);
        assert_eq!(*cx.global::<super::super::rooms_sidebar::DesktopWorkspace>(), super::super::rooms_sidebar::DesktopWorkspace::Projects);
    }
    #[test]
    fn removing_embedded_sidebar_preserves_chat_tabs_and_split_panes() {
        let a = room("first"); let b = room("second");
        let mut saved = SavedDockState::default();
        saved.dock_items.extend([
            (id!(root), DockItem::Splitter {axis: SplitterAxis::Horizontal, align: SplitterAlign::FromA(300.0), a: id!(rooms_sidebar_tabs), b: id!(chat_split)}),
            (id!(rooms_sidebar_tabs), tabs(vec![id!(rooms_sidebar_tab)])),
            (id!(rooms_sidebar_tab), tab(id!(rooms_sidebar))),
            (id!(chat_split), DockItem::Splitter {axis: SplitterAxis::Horizontal, align: SplitterAlign::Weighted(0.5), a: id!(main_tabs), b: id!(other_tabs)}),
            (id!(main_tabs), tabs(vec![id!(home_tab), a.tab_id()])),
            (id!(other_tabs), tabs(vec![b.tab_id()])),
            (id!(home_tab), tab(id!(hagency_panel))),
            (a.tab_id(), tab(a.dock_kind())), (b.tab_id(), tab(b.dock_kind())),
        ]);
        saved.open_rooms.extend([(a.tab_id(), a.clone()), (b.tab_id(), b.clone())]);
        saved.room_order = vec![a.clone(), b.clone()]; saved.selected_room = Some(b.clone());
        assert!(dock_state_repair::validate_and_repair_dock_state(&mut saved).unwrap());
        assert!(!saved.dock_items.contains_key(&id!(rooms_sidebar_tab)));
        assert!(!saved.dock_items.contains_key(&id!(rooms_sidebar_tabs)));
        assert!(matches!(saved.dock_items[&id!(root)], DockItem::Splitter {..}));
        assert_eq!(saved.open_rooms.len(), 2); assert_eq!(saved.room_order, vec![a, b.clone()]);
        assert_eq!(saved.selected_room, Some(b));
        assert!(!dock_state_repair::validate_and_repair_dock_state(&mut saved).unwrap());
    }
    #[test]
    fn new_root_tabs_need_no_sidebar_promotion_and_recover_missing_room_refs() {
        let a = room("first"); let mut saved = SavedDockState::default();
        saved.dock_items.extend([(id!(root), tabs(vec![id!(home_tab)])), (id!(home_tab), tab(id!(hagency_panel)))]);
        saved.open_rooms.insert(a.tab_id(), a.clone()); saved.room_order.push(a.clone());
        saved.selected_room = Some(a.clone());
        assert!(dock_state_repair::validate_and_repair_dock_state(&mut saved).unwrap());
        assert!(matches!(&saved.dock_items[&id!(root)], DockItem::Tabs {tabs, ..} if tabs.contains(&a.tab_id())));
        assert_eq!(saved.selected_room, Some(a));
        assert!(!saved.dock_items.contains_key(&id!(rooms_sidebar_tab)));
    }
    #[test]
    fn chats_action_opens_welcome_instead_of_projects_when_no_chat_exists() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut ui = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.MainDesktopUI {}});
            let ui = MainDesktopUI::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty()); ui
        });
        let mut state = AppState::default();
        for close in [true, false] {
            let actions = cx.capture_actions(|cx| if close {cx.action(HagencyAction::Close)} else {cx.action(MainDesktopUiAction::ShowGroupChats)});
            ui.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::with_data(&mut state));
            let dock = ui.view.dock(&cx, ids!(dock));
            assert!(!dock.item(id!(group_chats_welcome_tab)).is_empty());
            assert!(!dock.item(id!(home_tab)).is_empty());
            assert!(ui.most_recently_selected_room.is_none());
        }
    }

    #[test]
    fn close_current_tab_selects_the_next_recent_room_and_spares_the_home_tab() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut ui = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.MainDesktopUI {}});
            let ui = MainDesktopUI::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty()); ui
        });
        let mut state = AppState::default();
        let press_close = |ui: &mut MainDesktopUI, cx: &mut Cx, state: &mut AppState| {
            let actions = cx.capture_actions(|cx| cx.action(MainDesktopUiAction::CloseCurrentTab));
            ui.handle_event(cx, &Event::Actions(actions), &mut Scope::with_data(state));
        };

        // Nothing open: a harmless no-op.
        press_close(&mut ui, &mut cx, &mut state);
        assert!(ui.open_rooms.is_empty() && ui.most_recently_selected_room.is_none());

        let a = room("alpha"); let b = room("beta");
        ui.open_rooms.extend([(a.tab_id(), a.clone()), (b.tab_id(), b.clone())]);
        ui.room_order = vec![a.clone(), b.clone()];
        ui.most_recently_selected_room = Some(b.clone());

        // Single-pane mode has no tab bar, so there is no current tab to close.
        press_close(&mut ui, &mut cx, &mut state);
        assert_eq!(ui.open_rooms.len(), 2, "single-pane mode keeps every chat");
        assert_eq!(ui.most_recently_selected_room, Some(b.clone()));

        // Closing the current tab falls back to the next most recent room.
        ui.tabbed_chats = true;
        press_close(&mut ui, &mut cx, &mut state);
        assert!(!ui.open_rooms.contains_key(&b.tab_id()));
        assert_eq!(ui.room_order, vec![a.clone()]);
        assert_eq!(ui.most_recently_selected_room, Some(a.clone()));

        // Closing the last room tab falls back to the permanent home tab.
        press_close(&mut ui, &mut cx, &mut state);
        assert!(ui.open_rooms.is_empty());
        assert!(ui.most_recently_selected_room.is_none());
        let dock = ui.view.dock(&cx, ids!(dock));
        assert!(!dock.item(id!(home_tab)).is_empty(), "the permanent home tab is never closed");

        // With the home tab current, another close is a harmless no-op.
        press_close(&mut ui, &mut cx, &mut state);
        assert!(!dock.item(id!(home_tab)).is_empty());
    }

}
