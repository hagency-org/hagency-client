use makepad_widgets::*;
use crate::home::navigation_tab_bar::NavigationBarAction;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.GroupChatsWelcome = #(GroupChatsWelcome::register_widget(vm)) {
        width: Fill height: Fill flow: Down
        align: Align{x: 0.0 y: 0.5}
        padding: 40
        spacing: 20
        show_bg: true
        draw_bg.color: mod.widgets.RINX_CONTENT_BG
        title := RinxLabel {
            width: Fill height: Fit
            text: #(crate::i18n::tr("Group Chats")) i18n_text: "Group Chats"
            draw_text +: {color: mod.widgets.RINX_INK text_style: theme.font_bold {font_size: mod.widgets.RINX_DISPLAY_SIZE}}
        }
        description := RinxLabel {
            width: Fill height: Fit flow: Flow.Right{wrap: true}
            text: #(crate::i18n::tr("Choose a group chat in the sidebar, create a group, or explore rooms on your server."))
            i18n_text: "Choose a group chat in the sidebar, create a group, or explore rooms on your server."
            draw_text +: {color: mod.widgets.RINX_MUTED text_style: theme.font_regular {font_size: mod.widgets.RINX_BODY_SIZE}}
        }
        View {width: Fill height: Fit flow: Right spacing: 12
            new_group_chat := RinxButton {text: #(crate::i18n::tr("New group chat")) i18n_text: "New group chat"}
            explore := RinxButton {text: #(crate::i18n::tr("Explore rooms")) i18n_text: "Explore rooms"}
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct GroupChatsWelcome {
    #[source] source: ScriptObjectRef,
    #[deref] view: View,
}
impl Widget for GroupChatsWelcome {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if self.view.button(cx, ids!(new_group_chat)).clicked(actions) {
                cx.action(super::space_management::SpaceManagementAction::Open {parent:None});
            }
            if self.view.button(cx, ids!(explore)).clicked(actions) {
                cx.action(NavigationBarAction::GoToAddRoom);
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chat_empty_state_has_real_new_group_chat_and_explore_actions() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut screen = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm); crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.GroupChatsWelcome {}});
            let screen = GroupChatsWelcome::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty()); screen
        });
        assert_eq!(screen.view.label(&cx, ids!(title)).text(), crate::i18n::tr("Group Chats"));
        for (button, contacts) in [(id!(new_group_chat), true), (id!(explore), false)] {
            let uid = screen.view.button(&cx, &[button]).widget_uid();
            let actions = cx.capture_actions(|cx| cx.widget_action(uid, ButtonAction::Clicked(KeyModifiers::default())));
            let emitted = cx.capture_actions(|cx| screen.handle_event(cx, &Event::Actions(actions), &mut Scope::empty()));
            assert!(emitted.iter().any(|a| if contacts {matches!(a.downcast_ref(),Some(super::super::space_management::SpaceManagementAction::Open {parent:None}))}else{matches!(a.downcast_ref(),Some(NavigationBarAction::GoToAddRoom))}));
        }
    }
}
