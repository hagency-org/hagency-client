//! A pass-wide task dialog; background navigation never receives modal input.
use super::*;
use makepad_widgets::widget::WidgetWeakRef;

#[derive(Default)]
struct TaskDialogOwner(Option<WidgetUid>);

#[derive(Default)]
struct TaskDialogBinding(Option<WidgetWeakRef>);

// Dock pages are lazy widget trees. Keep the original wrapper instead of
// depending on a later UID query resolving a transparent View to its owner.
pub(super) fn bind_dialog_widget(cx: &mut Cx, widget: &WidgetRef) {
    cx.global::<TaskDialogBinding>().0 = Some(widget.downgrade());
}

fn is_dialog_input(event: &Event) -> bool {
    event.requires_visibility()
        || matches!(
            event,
            Event::MouseUp(_)
                | Event::LongPress(_)
                | Event::KeyDown(_)
                | Event::KeyUp(_)
                | Event::KeyFocus(_)
                | Event::KeyFocusLost(_)
                | Event::TextInput(_)
                | Event::TextRangeReplace(_)
                | Event::TextCopy(_)
                | Event::TextCut(_)
                | Event::ImeAction(_)
                | Event::SelectionHandleDrag(_)
                | Event::Drag(_)
                | Event::Drop(_)
                | Event::DragEnd
                | Event::BackPressed { .. }
        )
}

/// Route before application shortcuts and sibling navigation. Top-level dialogs
/// opened above this one (e.g. verification or browser) retain their own input.
pub(crate) fn route_task_dialog_input(
    cx: &mut Cx,
    event: &Event,
    root: &WidgetRef,
    scope: &mut Scope,
) -> bool {
    if !is_dialog_input(event) {
        return false;
    }
    let Some(uid) = cx.global::<TaskDialogOwner>().0 else {
        return false;
    };
    let mut outer_modal = false;
    root.widget(cx, ids!(overlay_container))
        .children(&mut |_, child| {
            outer_modal |= child.as_modal().is_open();
            child.children(&mut |_, nested| outer_modal |= nested.as_modal().is_open());
        });
    if outer_modal {
        return false;
    }
    let bound = cx
        .global::<TaskDialogBinding>()
        .0
        .as_ref()
        .and_then(WidgetWeakRef::upgrade)
        .filter(|widget| widget.widget_uid() == uid);
    let widget = bound.unwrap_or_else(|| cx.widget_tree().widget(uid));
    let Some(mut board) = widget.borrow_mut::<ProjectBoard>() else {
        cx.global::<TaskDialogOwner>().0 = None;
        return false;
    };
    if !board.view.view(cx, ids!(inspector)).visible() {
        cx.global::<TaskDialogOwner>().0 = None;
        return false;
    }
    board.handle_event(cx, event, scope);
    true
}

impl ProjectBoard {
    pub(super) fn open_inspector(&mut self, cx: &mut Cx) {
        self.view.view(cx, ids!(inspector)).set_visible(cx, true);
        if self.dialog_cancel_scope.is_none() {
            self.dialog_return_focus = cx.key_focus();
            self.dialog_cancel_scope = Some(cx.begin_cancel_scope());
            self.dialog_focus_pending = true;
            cx.set_key_focus(Area::Empty);
        }
        cx.global::<TaskDialogOwner>().0 = Some(self.widget_uid());
        self.redraw_dialog(cx);
    }

    pub(super) fn hide_inspector(&mut self, cx: &mut Cx) {
        self.view.view(cx, ids!(inspector)).set_visible(cx, false);
        if let Some(scope) = self.dialog_cancel_scope.take() {
            cx.end_cancel_scope(scope);
            cx.set_key_focus(self.dialog_return_focus);
            cx.unblock_scrolling();
        }
        self.dialog_focus_pending = false;
        if cx.global::<TaskDialogOwner>().0 == Some(self.widget_uid()) {
            cx.global::<TaskDialogOwner>().0 = None;
        }
        self.close_calendar(cx);
        self.redraw_dialog(cx);
    }

    fn redraw_dialog(&mut self, cx: &mut Cx) {
        if let Some(list) = &self.dialog_draw_list {
            list.redraw(cx);
        }
        self.view.redraw(cx);
    }

    pub(super) fn request_close_inspector(&mut self, cx: &mut Cx) {
        if self.pending.is_some() {
            self.notice(cx, tr("A previous save is unconfirmed. Retry its original request to avoid duplicates."));
        } else if self.dirty && !self.discard_armed {
            self.discard_armed = true;
            self.notice(
                cx,
                tr("You have unsaved changes. Save them, or close again to discard the draft."),
            );
        } else {
            self.remember_comment(cx);
            self.clear_media(cx);
            self.selected = None;
            self.creating = false;
            self.dirty = false;
            self.discard_armed = false;
            self.detail_request = nonce();
            self.hide_inspector(cx);
            self.notice(cx, "");
        }
        self.sync_controls(cx);
    }

    fn tab_in_dialog(&self, cx: &mut Cx, backwards: bool) {
        let Some(list) = &self.dialog_draw_list else {
            return;
        };
        let mut areas = Vec::new();
        makepad_widgets::makepad_draw::cx_draw::CxDraw::iterate_nav_stops(
            cx,
            list.id(),
            |_, stop| {
                areas.push(stop.area);
                None::<Area>
            },
        );
        if areas.is_empty() {
            return;
        }
        let current = areas.iter().position(|area| cx.has_key_focus(*area));
        let target = match (current, backwards) {
            (Some(index), true) => (index + areas.len() - 1) % areas.len(),
            (Some(index), false) => (index + 1) % areas.len(),
            (None, true) => areas.len() - 1,
            (None, false) => 0,
        };
        if let Some((area, stack)) =
            makepad_widgets::makepad_draw::cx_draw::CxDraw::iterate_nav_stops(
                cx,
                list.id(),
                |_, stop| (stop.area == areas[target]).then_some(stop.area),
            )
        {
            makepad_widgets::nav_control::NavControl::send_trigger_to_scroll_stack(cx, stack);
            cx.set_key_focus(area);
        }
    }

    pub(super) fn handle_board_view_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope,
    ) {
        let inspector = self.view.view(cx, ids!(inspector));
        if !inspector.visible() {
            self.view.handle_event(cx, event, scope);
            return;
        }
        if let Event::KeyDown(key) = event {
            if key.key_code == KeyCode::Tab {
                self.tab_in_dialog(cx, key.modifiers.shift);
                return;
            }
            if key.key_code == KeyCode::Escape
                && !key.is_repeat
                && self
                    .dialog_cancel_scope
                    .as_ref()
                    .is_some_and(|s| cx.owns_cancel(s))
            {
                if self.calendar_month.is_some() {
                    self.close_calendar(cx);
                } else {
                    self.request_close_inspector(cx);
                }
                return;
            }
        }
        if matches!(event, Event::BackPressed { .. })
            && self
                .dialog_cancel_scope
                .as_ref()
                .is_some_and(|s| cx.owns_cancel(s))
            && event.back_pressed()
        {
            self.request_close_inspector(cx);
            return;
        }
        inspector.handle_event(cx, event, scope);
        // Claim the full scrim after children, without dismissing or forwarding
        // a background click. Close/Escape use the same draft-protection path.
        let _ = event.hits(cx, self.draw_dialog_bg.area());
    }

    pub(super) fn draw_task_dialog(&mut self, cx: &mut Cx2d, scope: &mut Scope) {
        let mut list = self
            .dialog_draw_list
            .take()
            .unwrap_or_else(|| DrawList2d::new(cx));
        list.begin_overlay_reuse(cx);
        let size = cx.current_pass_size();
        let bounds = cx.global::<ModalBounds>().0.unwrap_or(Rect {
            pos: dvec2(0.0, 0.0),
            size,
        });
        cx.begin_root_turtle(size, Layout::flow_overlay());
        let inspector = self.view.view(cx, ids!(inspector));
        if inspector.visible() {
            if self.dialog_cancel_scope.is_none() {
                self.open_inspector(cx);
            }
            self.draw_dialog_bg.draw_abs(cx, bounds);
            let margin = if bounds.size.x < 600.0 { 12.0 } else { 32.0 };
            let width = (bounds.size.x - margin * 2.0).clamp(1.0, 760.0);
            let height = (bounds.size.y - margin * 2.0).clamp(1.0, 900.0);
            let walk = Walk::abs_rect(Rect {
                pos: bounds.pos + (bounds.size - dvec2(width, height)) * 0.5,
                size: dvec2(width, height),
            });
            while let Some(step) = inspector.draw_walk(cx, scope, walk).step() {
                if !self.draw_collaboration(cx, scope, &step) {
                    self.draw_extension_list(cx, scope, &step);
                }
            }
            if self.dialog_focus_pending {
                self.dialog_focus_pending = false;
                cx.set_key_focus(inspector.area());
                if self.creating {
                    self.view.text_input(cx, ids!(title)).take_key_focus(cx);
                }
            }
            cx.block_scrolling_except_within(inspector.area());
        }
        cx.end_pass_sized_turtle();
        list.end(cx);
        self.dialog_draw_list = Some(list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_keeps_unsaved_draft_and_unconfirmed_save_inside_dialog() {
        let (mut cx, mut board) = super::super::tests::panel();
        board.new_task(&mut cx);
        board.dirty = true;
        board.request_close_inspector(&mut cx);
        assert!(board.view.view(&cx, ids!(inspector)).visible());
        assert!(board.discard_armed);
        assert!(board.view.label(&cx, ids!(dialog_notice)).visible());
        board.request_close_inspector(&mut cx);
        assert!(!board.view.view(&cx, ids!(inspector)).visible());
        assert!(board.dialog_cancel_scope.is_none());
        assert!(cx.global::<TaskDialogOwner>().0.is_none());
        board.new_task(&mut cx);
        board.pending = Some(BoardAction::Create {
            input: Create {
                idempotency_key: "original-dialog-save".into(),
                fields: Fields {
                    title: "Pending".into(),
                    ..Default::default()
                },
                state: "todo".into(),
            },
        });
        board.request_close_inspector(&mut cx);
        assert!(board.view.view(&cx, ids!(inspector)).visible());
        assert!(board.view.button(&cx, ids!(dialog_retry)).visible());
        assert!(
            board
                .view
                .label(&cx, ids!(dialog_notice))
                .text()
                .contains("unconfirmed")
        );
        assert!(board.pending.is_some());
        board.clear_content(&mut cx);
        assert!(board.dialog_cancel_scope.is_none());
    }

    #[test]
    fn application_input_routes_only_to_the_open_task_dialog_and_yields_to_outer_modal() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let root = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm, {mod.widgets.View {
                overlay_container := mod.widgets.View {
                    board := mod.widgets.ProjectBoard {}
                    confirmation := mod.widgets.Modal {}
                }
            }});
            WidgetRef::script_from_value(vm, value)
        });
        let board = root.project_board(&cx, ids!(board));
        bind_dialog_widget(&mut cx, &board);
        board.borrow_mut().unwrap().new_task(&mut cx);
        let key = Event::KeyDown(KeyEvent {
            key_code: KeyCode::KeyK,
            modifiers: KeyModifiers {
                logo: true,
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(
            route_task_dialog_input(&mut cx, &key, &root, &mut Scope::empty()),
            "background shortcuts must be intercepted before App navigation"
        );
        assert!(
            !route_task_dialog_input(&mut cx, &Event::Actions(vec![]), &root, &mut Scope::empty()),
            "async/model actions must continue normally"
        );
        root.modal(&cx, ids!(confirmation)).open(&mut cx);
        assert!(
            !route_task_dialog_input(&mut cx, &key, &root, &mut Scope::empty()),
            "a foreground confirmation must retain its input"
        );
        root.modal(&cx, ids!(confirmation)).close(&mut cx);
        board.borrow_mut().unwrap().hide_inspector(&mut cx);
        assert!(!route_task_dialog_input(
            &mut cx,
            &key,
            &root,
            &mut Scope::empty()
        ));
    }

    #[test]
    fn tab_and_shift_tab_wrap_within_dialog_navigation_stops() {
        let (mut cx, mut board) = super::super::tests::panel();
        board.new_task(&mut cx);
        super::super::tests::draw(&mut cx, &mut board, 1200.0, 900.0);
        let list = board.dialog_draw_list.as_ref().unwrap().id();
        let mut areas = Vec::new();
        makepad_widgets::makepad_draw::cx_draw::CxDraw::iterate_nav_stops(
            &mut cx,
            list,
            |_, stop| {
                areas.push(stop.area);
                None::<Area>
            },
        );
        assert!(areas.len() > 5);
        cx.set_key_focus(*areas.last().unwrap());
        cx.action(0usize);
        cx.handle_actions();
        board.tab_in_dialog(&mut cx, false);
        cx.action(0usize);
        cx.handle_actions();
        assert!(cx.has_key_focus(areas[0]));
        board.tab_in_dialog(&mut cx, true);
        cx.action(0usize);
        cx.handle_actions();
        assert!(cx.has_key_focus(*areas.last().unwrap()));
    }

    #[test]
    fn closing_dialog_restores_the_original_background_focus() {
        let (mut cx, mut board) = super::super::tests::panel();
        super::super::tests::draw(&mut cx, &mut board, 1200.0, 900.0);
        let original = board.view.text_input(&cx, ids!(search)).area();
        cx.set_key_focus(original);
        cx.action(0usize);
        cx.handle_actions();
        board.new_task(&mut cx);
        super::super::tests::draw(&mut cx, &mut board, 1200.0, 900.0);
        cx.action(0usize);
        cx.handle_actions();
        cx.set_key_focus(board.view.text_input(&cx, ids!(labels)).area());
        cx.action(0usize);
        cx.handle_actions();
        board.hide_inspector(&mut cx);
        cx.action(0usize);
        cx.handle_actions();
        assert!(cx.has_key_focus(original));
    }
}
