use makepad_widgets::*;

pub const SCROLL_TO_BOTTOM_SPEED: f64 = 90.0;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.JumpToBottomButton = #(JumpToBottomButton::register_widget(vm)) {
        width: Fill height: Fill flow: Overlay
        align: Align{x: 0.5, y: 1.0}
        visible: false
        bar := RoundedView {
            width: Fit{max: FitBound.Rel{base: Base.Parent, factor: 0.95}} height: Fit
            margin: Inset{bottom: 12}
            padding: 8 spacing: 10 flow: Flow.Right{wrap: true} align: Align{y: 0.5}
            draw_bg +: {
                color: mod.widgets.RINX_SURFACE
                border_color: mod.widgets.RINX_BORDER border_size: 1.0
                border_radius: mod.widgets.RINX_CONTROL_RADIUS
            }
            history_hint_holder := View {
                width: Fit height: 32 flow: Down align: Align{y: 0.5}
                history_hint := RinxLabel {
                    width: Fit height: Fit padding: 0
                    draw_text +: {color: mod.widgets.RINX_MUTED text_style.font_size: mod.widgets.RINX_UI_SIZE}
                    text: #(crate::i18n::tr("Viewing older messages")) i18n_text: "Viewing older messages"
                }
            }
            unread_message_badge := RoundedView {
                visible: false width: Fit height: 32 padding: Inset{left: 6 right: 6}
                flow: Down align: Align{y: 0.5}
                draw_bg +: {color: mod.widgets.RINX_ACCENT border_radius: mod.widgets.RINX_CONTROL_RADIUS}
                unread_messages_count := RinxLabel {
                    width: Fit height: Fit padding: 0
                    draw_text +: {color: mod.widgets.RINX_ON_ACCENT text_style.font_size: mod.widgets.RINX_UI_SIZE}
                }
            }
            inner_button := RinxButton {
                width: Fit height: 32 padding: Inset{left: 12 right: 12 top: 0 bottom: 0}
                icon_walk: Walk{width: 0 height: 0}
                text: #(crate::i18n::tr("Jump to latest")) i18n_text: "Jump to latest"
            }
        }
    }
}

#[derive(ScriptHook, Script, Widget)]
pub struct JumpToBottomButton {
    #[source] source: ScriptObjectRef,
    #[deref] view: View,
}

impl Widget for JumpToBottomButton {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible { return; }
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let wide = cx.peek_walk_turtle(walk).size.x >= 420.0;
        self.view(cx, ids!(history_hint_holder)).set_visible(cx, wide);
        self.view.draw_walk(cx, scope, walk)
    }
}

impl JumpToBottomButton {
    /// Returns whether the parent timeline needs to redraw this visibility change.
    pub fn update_visibility(&mut self, cx: &mut Cx, is_at_bottom: bool) -> bool {
        let changed = self.visible == is_at_bottom;
        self.visible = !is_at_bottom;
        if is_at_bottom {
            self.view(cx, ids!(unread_message_badge)).set_visible(cx, false);
        }
        changed
    }

    /// Unread results update the badge, never the visibility of the history bar.
    /// A late response must not bring it back after the user reached the latest.
    pub fn show_unread_message_badge(&mut self, cx: &mut Cx, count: UnreadMessageCount) {
        let text = match count {
            UnreadMessageCount::Unknown => crate::i18n::tr("New messages").to_owned(),
            UnreadMessageCount::Known(0) => String::new(),
            UnreadMessageCount::Known(count) => {
                format!("{}{}", count.min(99), if count > 99 { "+" } else { "" })
            }
        };
        self.view(cx, ids!(unread_message_badge)).set_visible(cx, !text.is_empty());
        self.label(cx, ids!(unread_messages_count)).set_text(cx, &text);
    }

    pub fn update_from_actions(
        &mut self,
        cx: &mut Cx,
        portal_list: &PortalListRef,
        actions: &Actions,
    ) -> bool {
        let clicked = self.visible && self.button(cx, ids!(inner_button)).clicked(actions);
        if clicked {
            portal_list.smooth_scroll_to_end(cx, SCROLL_TO_BOTTOM_SPEED, None);
        }
        clicked
    }

}

impl JumpToBottomButtonRef {
    /// See [`JumpToBottomButton::update_visibility()`].
    pub fn update_visibility(&self, cx: &mut Cx, is_at_bottom: bool) -> bool {
        self.borrow_mut().is_some_and(|mut inner| inner.update_visibility(cx, is_at_bottom))
    }

    /// See [`JumpToBottomButton::show_unread_message_badge()`].
    pub fn show_unread_message_badge(&self, cx: &mut Cx, count: UnreadMessageCount) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_unread_message_badge(cx, count);
        }
    }

    /// See [`JumpToBottomButton::update_from_actions()`].
    pub fn update_from_actions(
        &self,
        cx: &mut Cx,
        portal_list: &PortalListRef,
        actions: &Actions,
    ) -> bool {
        self.borrow_mut().is_some_and(|mut inner| inner.update_from_actions(cx, portal_list, actions))
    }
}

/// The number of unread messages in a room.
#[derive(Clone, Debug)]
pub enum UnreadMessageCount {
    /// There are unread messages, but we do not know how many.
    Unknown,
    /// There are unread messages, and we know exactly how many.
    Known(u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_draw::cx_draw::CxDraw;

    fn widget() -> (Cx, JumpToBottomButton) {
        let mut cx = Cx::new(Box::new(|_,_|{}));
        let widget = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm,{mod.widgets.JumpToBottomButton {}});
            let widget = JumpToBottomButton::script_from_value(vm,value);
            assert!(vm.take_errors().is_empty());widget
        });
        (cx,widget)
    }

    fn draw(cx: &mut Cx, widget: &mut JumpToBottomButton, width: f64) {
        cx.redraw_all();
        let pass = DrawPass::new(cx);pass.set_size(cx,dvec2(width,400.0));
        let mut list = DrawList2d::new(cx);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx,&event);let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass,None);list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(width,400.0),Layout::flow_overlay());
        widget.draw_walk_all(&mut cx2d,&mut Scope::empty(),Walk::fill());
        cx2d.end_pass_sized_turtle();list.end(&mut cx2d);cx2d.end_pass(&pass);
    }

    #[test]
    fn late_unread_results_never_reopen_the_bar_and_hidden_buttons_cannot_jump() {
        let (mut cx,mut widget) = widget();
        assert!(!widget.visible);
        widget.update_visibility(&mut cx,false);
        draw(&mut cx,&mut widget,900.0);
        let button = widget.button(&cx,ids!(inner_button));
        let actions = cx.capture_actions(|cx|cx.widget_action(button.widget_uid(),ButtonAction::Clicked(KeyModifiers::default())));
        assert!(widget.update_from_actions(&mut cx,&PortalListRef::default(),&actions));
        assert!(widget.update_visibility(&mut cx,true));
        for count in [UnreadMessageCount::Known(2),UnreadMessageCount::Unknown] {
            widget.show_unread_message_badge(&mut cx,count);
            assert!(!widget.visible,"late unread counts must not reopen navigation at the latest message");
        }
        assert!(!widget.update_from_actions(&mut cx,&PortalListRef::default(),&actions));
        assert!(!widget.update_visibility(&mut cx,true),"a stable hidden state must not request another parent redraw");
        assert!(!widget.view(&cx,ids!(unread_message_badge)).visible());
        assert!(widget.update_visibility(&mut cx,false));
        widget.show_unread_message_badge(&mut cx,UnreadMessageCount::Known(105));
        assert_eq!(widget.label(&cx,ids!(unread_messages_count)).text(),"99+");
    }

    #[test]
    fn centered_history_bar_fits_wide_and_narrow_timelines() {
        let (mut cx,mut widget) = widget();
        widget.update_visibility(&mut cx,false);
        for width in [900.0,310.0] {
            for count in [UnreadMessageCount::Known(0),UnreadMessageCount::Unknown,UnreadMessageCount::Known(105)] {
                widget.show_unread_message_badge(&mut cx,count);
                draw(&mut cx,&mut widget,width);
                let bar = widget.view(&cx,ids!(bar)).area().rect(&cx);
                let button = widget.button(&cx,ids!(inner_button)).area().rect(&cx);
                assert!(bar.size.x > 0.0 && bar.size.y > 0.0);
                assert!((bar.pos.x + bar.size.x * 0.5 - width * 0.5).abs() < 0.5,"{bar:?}");
                assert!(bar.pos.x >= 0.0 && bar.pos.x + bar.size.x <= width + 0.5,"{bar:?}");
                assert!(bar.pos.y + bar.size.y <= 400.0 && button.size.x > 0.0);
                assert_eq!(widget.view(&cx,ids!(history_hint_holder)).visible(),width >= 420.0);
                if width >= 420.0 {
                    let hint = widget.label(&cx,ids!(history_hint));
                    let hint_rect = widget.view(&cx,ids!(history_hint_holder)).area().rect(&cx);
                    // A text area contains one instance per glyph; rect() only
                    // measures the first letter. Check all visible glyphs.
                    let text = hint.borrow().unwrap().draw_text.draw_vars.area.rect_union(&cx,false);
                    let center_y = button.pos.y + button.size.y * 0.5;
                    assert!((hint_rect.pos.y + hint_rect.size.y * 0.5 - center_y).abs() < 0.5,
                        "hint and button must share one vertical center: {hint_rect:?}, {button:?}");
                    assert!((text.pos.y + text.size.y * 0.5 - center_y).abs() < 1.0,
                        "hint text must be centered within the control row: {text:?}, {button:?}");
                }
            }
        }
        cx.with_vm(|vm|assert!(vm.take_errors().is_empty()));
    }
}
