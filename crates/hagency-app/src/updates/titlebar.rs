use super::{Phase, Snapshot, UpdateAction, UpdateState};
use crate::i18n::Language;
use makepad_widgets::*;
use makepad_widgets::makepad_platform::event::finger::TouchState;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.UpdateNotice = #(UpdateNotice::register_widget(vm)) {
        width: Fit height: Fill align: Align{y: 0.5}
        trigger := RinxButton {
            height: 24 padding: Inset{left: 10 right: 10}
            text: #(crate::i18n::tr("Update Hagency")) i18n_text: "Update Hagency"
            draw_bg +: {
                color: #0000 color_focus: #0000
                color_hover: RINX_WARNING_BG color_down: RINX_WARNING_BG
                border_radius: 12 border_size: 1
                border_color: RINX_WARNING_FG border_color_hover: RINX_WARNING_FG
                border_color_down: RINX_WARNING_FG border_color_focus: RINX_WARNING_FG
            }
            draw_text +: {
                color: RINX_WARNING_FG color_hover: RINX_WARNING_FG
                color_down: RINX_WARNING_FG color_focus: RINX_WARNING_FG
            }
        }
        panel: RoundedView {
            width: 304 height: Fit flow: Down spacing: 10 padding: 14
            show_bg: true
            draw_bg +: {color: RINX_SURFACE border_color: RINX_BORDER border_size: 1 border_radius: 8}
            current := RinxHint {width: Fill height: Fit flow: Flow.Right{wrap: true}}
            version := RinxLabel {width: Fill height: Fit flow: Flow.Right{wrap: true}}
            status := RinxHint {width: Fill height: Fit flow: Flow.Right{wrap: true}}
            primary := RinxButton {width: Fill}
            later := RinxGhostButton {
                width: Fill
                text: #(crate::i18n::tr("Not now")) i18n_text: "Not now"
            }
        }
    }
}

pub(crate) fn titlebar_notice_visible(state: &Snapshot) -> bool {
    state.enabled
        && matches!(
            state.phase,
            Phase::Available
                | Phase::Downloading
                | Phase::Ready
                | Phase::Verifying
                | Phase::Waiting
                | Phase::Preparing
                | Phase::Canceling
                | Phase::Failed
        )
}

fn primary_action(phase: Phase) -> Option<UpdateAction> {
    match phase {
        Phase::Available => Some(UpdateAction::Download),
        Phase::Ready => Some(UpdateAction::Install),
        Phase::Failed => Some(UpdateAction::Check),
        _ => None,
    }
}

#[derive(Script, Widget)]
pub struct UpdateNotice {
    #[deref]
    view: View,
    #[live]
    panel: WidgetRef,
    #[rust]
    overlay: Option<DrawList2d>,
    #[rust]
    open: bool,
    #[rust]
    observed: Option<(Snapshot, Language)>,
}

impl ScriptHook for UpdateNotice {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.overlay = Some(DrawList2d::script_new(vm));
    }
}

impl UpdateNotice {
    fn redraw_notice(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
        if let Some(overlay) = &self.overlay {
            overlay.redraw(cx);
        }
    }

    fn close(&mut self, cx: &mut Cx) {
        self.open = false;
        self.redraw_notice(cx);
    }
}

impl Widget for UpdateNotice {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let state = cx.global::<UpdateState>().0.clone();
        let observed = (state.clone(), crate::i18n::language());
        let changed = self.observed.as_ref() != Some(&observed);
        self.observed = Some(observed);
        if !titlebar_notice_visible(&state) {
            if self.open {
                self.close(cx);
            }
            if changed {
                self.redraw_notice(cx);
            }
            return;
        }
        if self.open {
            let outside = |pos| {
                !self.panel.area().rect(cx).contains(pos)
                    && !self.view.area().rect(cx).contains(pos)
            };
            let dismiss = match event {
                Event::MouseDown(e) => outside(e.abs),
                Event::TouchUpdate(e) => e
                    .touches
                    .iter()
                    .any(|touch| touch.state == TouchState::Start && outside(touch.abs)),
                Event::KeyDown(e) => e.key_code == KeyCode::Escape,
                _ => false,
            };
            if dismiss || event.back_pressed() {
                self.close(cx);
            }
        }
        self.view.handle_event(cx, event, scope);
        if self.open {
            self.panel.handle_event(cx, event, scope);
        }
        if let Event::Actions(actions) = event {
            if self.view.button(cx, ids!(trigger)).clicked(actions) {
                self.open = !self.open;
            } else if self.open && self.panel.button(cx, ids!(primary)).clicked(actions) {
                if !state.preview
                    && let Some(action) = primary_action(state.phase)
                {
                    cx.action(action);
                }
            } else if self.open && self.panel.button(cx, ids!(later)).clicked(actions) {
                if state.phase == Phase::Waiting || super::sparkle_active() {
                    cx.action(UpdateAction::Later);
                }
                self.close(cx);
            }
        }
        if changed || matches!(event, Event::Actions(_) | Event::WindowGeomChange(_)) {
            self.redraw_notice(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let state = cx.global::<UpdateState>().0.clone();
        // The worker remains disabled for hosted and unsupported builds.
        if !titlebar_notice_visible(&state) {
            self.open = false;
            let overlay = self.overlay.as_mut().unwrap();
            overlay.begin_overlay_reuse(cx);
            overlay.end(cx);
            return DrawStep::done();
        }
        self.view.draw_walk_all(cx, scope, walk);
        // The parent is the right-aligned caption slot. Its inner bounds are
        // final now; this child's area is only positioned when that slot ends.
        let anchor = cx.turtle().inner_rect();
        let overlay = self.overlay.as_mut().unwrap();
        overlay.begin_overlay_reuse(cx);
        if self.open {
            // A caption's turtle clips to its height. Popovers need a fresh
            // pass-sized root, just like Modal and Tooltip, to escape that clip.
            cx.begin_root_turtle_for_pass(Layout::flow_overlay());
            self.panel.label(cx, ids!(current)).set_text(
                cx,
                &format!(
                    "{} {}",
                    crate::i18n::tr("Current version:"),
                    env!("CARGO_PKG_VERSION")
                ),
            );
            self.panel.label(cx, ids!(version)).set_text(
                cx,
                &format!("{} {}", crate::i18n::tr("New version:"), state.version),
            );
            self.panel
                .label(cx, ids!(version))
                .set_visible(cx, !state.version.is_empty());
            let status = if state.phase == Phase::Downloading {
                format!("{} {}%", crate::i18n::tr(state.status()), state.progress)
            } else {
                crate::i18n::tr(state.status()).to_owned()
            };
            self.panel.label(cx, ids!(status)).set_text(cx, &status);
            let primary = self.panel.button(cx, ids!(primary));
            primary.set_visible(
                cx,
                primary_action(state.phase).is_some() && !state.restart_required,
            );
            primary.set_enabled(
                cx,
                !state.preview && (state.phase != Phase::Failed || state.can_check),
            );
            primary.set_text(
                cx,
                crate::i18n::tr(match state.phase {
                    Phase::Ready => "Restart and Install",
                    Phase::Failed => "Check for Updates",
                    _ => super::download_label(),
                }),
            );
            self.panel.button(cx, ids!(later)).set_text(
                cx,
                crate::i18n::tr(
                    if super::sparkle_active() && state.phase != Phase::Available {
                        "Cancel Update"
                    } else {
                        "Not now"
                    },
                ),
            );
            self.panel.button(cx, ids!(later)).set_visible(
                cx,
                matches!(
                    state.phase,
                    Phase::Available | Phase::Downloading | Phase::Ready | Phase::Waiting
                ) || (super::sparkle_active() && state.phase == Phase::Verifying),
            );
            let window = cx.current_pass_size();
            let width = (window.x - 16.0).clamp(0.0, 304.0);
            self.panel.draw_walk_all(
                cx,
                scope,
                Walk {
                    abs_pos: Some(dvec2(
                        (anchor.pos.x + anchor.size.x - width).max(8.0),
                        anchor.pos.y + anchor.size.y + 8.0,
                    )),
                    width: Size::Fixed(width),
                    height: Size::fit(),
                    ..Walk::default()
                },
            );
            cx.end_pass_sized_turtle();
        }
        overlay.end(cx);
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_draw::cx_draw::CxDraw;

    #[test]
    fn unchanged_background_signals_do_not_repaint_the_update_overlay() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut notice = notice(&mut cx);
        notice.handle_event(&mut cx, &Event::Signal, &mut Scope::empty());
        draw(&mut cx, &mut notice, 1280.0);
        cx.new_draw_event = Default::default();
        for _ in 0..32 { notice.handle_event(&mut cx, &Event::Signal, &mut Scope::empty()); }
        assert!(!cx.new_draw_event.will_redraw());
        cx.global::<UpdateState>().0.enabled = true;
        cx.global::<UpdateState>().0.phase = Phase::Available;
        notice.handle_event(&mut cx, &Event::Signal, &mut Scope::empty());
        assert!(cx.new_draw_event.will_redraw(), "actual update state changes must remain visible");
    }
    fn notice(cx: &mut Cx) -> UpdateNotice {
        cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value = script_eval!(vm, {mod.widgets.UpdateNotice {}});
            let notice = UpdateNotice::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty());
            notice
        })
    }

    fn draw(cx: &mut Cx, notice: &mut UpdateNotice, width: f64) {
        cx.redraw_all();
        let pass = DrawPass::new(cx);
        pass.set_size(cx, dvec2(width, 760.0));
        let mut list = DrawList2d::new(cx);
        let overlay = Overlay {
            draw_list: DrawList::new(cx),
        };
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        list.begin_always(&mut cx2d);
        overlay.begin(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(width, 760.0), Layout::flow_overlay());
        cx2d.begin_turtle(
            Walk {
                width: Size::Fixed(width),
                height: Size::Fixed(36.0),
                ..Walk::default()
            },
            Layout {
                clip_x: true,
                clip_y: true,
                align: Align { x: 1.0, y: 0.5 },
                padding: Inset {
                    right: 12.0,
                    ..Inset::default()
                },
                ..Layout::flow_overlay()
            },
        );
        notice.draw_walk_all(
            &mut cx2d,
            &mut Scope::empty(),
            Walk {
                width: Size::fit(),
                height: Size::fill(),
                ..Walk::default()
            },
        );
        cx2d.end_turtle();
        cx2d.end_pass_sized_turtle();
        overlay.end(&mut cx2d);
        list.end(&mut cx2d);
        cx2d.end_pass(&pass);
    }

    #[test]
    fn titlebar_update_panel_fits_and_tracks_download_and_install_states() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut notice = notice(&mut cx);
        cx.global::<UpdateState>().0 = Snapshot {
            enabled: true,
            phase: Phase::Available,
            version: "0.7.0".into(),
            ..Default::default()
        };
        for width in [375.0, 560.0, 1280.0] {
            notice.open = true;
            draw(&mut cx, &mut notice, width);
            let anchor = notice.view.area().rect(&cx);
            let panel = notice.panel.area().rect(&cx);
            let visible_panel = notice.panel.area().clipped_rect(&cx);
            assert!(anchor.size.x > 0.0 && anchor.pos.x + anchor.size.x <= width);
            assert!(panel.pos.x >= 8.0 && panel.pos.x + panel.size.x <= width - 8.0);
            assert!(panel.pos.y >= anchor.pos.y + anchor.size.y);
            assert!(
                (panel.pos.x + panel.size.x - anchor.pos.x - anchor.size.x).abs() < 1.0,
                "Popup must align with the trigger after caption alignment: {panel:?}, {anchor:?}"
            );
            assert!(
                visible_panel.size.y >= panel.size.y - 0.1,
                "The caption must not clip its popup: {visible_panel:?} vs {panel:?}"
            );
            assert!(
                notice
                    .panel
                    .label(&cx, ids!(current))
                    .text()
                    .contains(env!("CARGO_PKG_VERSION"))
            );
            assert!(
                notice
                    .panel
                    .label(&cx, ids!(version))
                    .text()
                    .contains("0.7.0")
            );
        }
        cx.global::<UpdateState>().0.phase = Phase::Downloading;
        cx.global::<UpdateState>().0.progress = 42;
        draw(&mut cx, &mut notice, 375.0);
        assert!(!notice.panel.button(&cx, ids!(primary)).visible());
        assert!(notice.panel.label(&cx, ids!(status)).text().contains("42%"));
        cx.global::<UpdateState>().0.phase = Phase::Ready;
        draw(&mut cx, &mut notice, 375.0);
        assert_eq!(
            notice.panel.button(&cx, ids!(primary)).text(),
            crate::i18n::tr("Restart and Install")
        );
        cx.global::<UpdateState>().0.phase = Phase::Waiting;
        draw(&mut cx, &mut notice, 375.0);
        assert!(!notice.panel.button(&cx, ids!(primary)).visible());
        assert!(notice.panel.button(&cx, ids!(later)).visible());
        cx.global::<UpdateState>().0.phase = Phase::Current;
        draw(&mut cx, &mut notice, 375.0);
        assert!(!notice.open);
    }

    #[test]
    fn titlebar_update_panel_dismisses_without_installing() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut notice = notice(&mut cx);
        cx.global::<UpdateState>().0 = Snapshot {
            enabled: true,
            phase: Phase::Ready,
            version: "0.7.0".into(),
            ..Default::default()
        };
        notice.open = true;
        draw(&mut cx, &mut notice, 375.0);
        let event = Event::MouseDown(MouseDownEvent {
            abs: dvec2(10.0, 500.0),
            button: MouseButton::PRIMARY,
            window_id: WindowId(0, 0),
            modifiers: KeyModifiers::default(),
            handled: std::cell::Cell::new(Area::Empty),
            time: 1.0,
        });
        let actions = cx.capture_actions(|cx| notice.handle_event(cx, &event, &mut Scope::empty()));
        assert!(!notice.open);
        assert!(
            !actions
                .iter()
                .any(|action| action.downcast_ref::<UpdateAction>().is_some())
        );
        assert!(event.pointer_claimed_area().is_empty());
        cx.global::<UpdateState>().0.enabled = false;
        assert!(!titlebar_notice_visible(&cx.global::<UpdateState>().0));
    }

    #[test]
    fn titlebar_update_install_requires_explicit_action_and_wait_can_be_canceled() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut notice = notice(&mut cx);
        cx.global::<UpdateState>().0 = Snapshot {
            enabled: true,
            phase: Phase::Ready,
            version: "0.7.0".into(),
            ..Default::default()
        };
        draw(&mut cx, &mut notice, 375.0);
        let trigger = notice.view.button(&cx, ids!(trigger));
        let click = cx.capture_actions(|cx| {
            cx.widget_action(
                trigger.widget_uid(),
                ButtonAction::Clicked(KeyModifiers::default()),
            )
        });
        let actions = cx.capture_actions(|cx| {
            notice.handle_event(cx, &Event::Actions(click), &mut Scope::empty())
        });
        assert!(notice.open);
        assert!(
            !actions
                .iter()
                .any(|action| action.downcast_ref::<UpdateAction>().is_some())
        );
        draw(&mut cx, &mut notice, 375.0);
        let primary = notice.panel.button(&cx, ids!(primary));
        let click = cx.capture_actions(|cx| {
            cx.widget_action(
                primary.widget_uid(),
                ButtonAction::Clicked(KeyModifiers::default()),
            )
        });
        let actions = cx.capture_actions(|cx| {
            notice.handle_event(cx, &Event::Actions(click), &mut Scope::empty())
        });
        assert!(actions.iter().any(|action| matches!(
            action.downcast_ref::<UpdateAction>(),
            Some(UpdateAction::Install)
        )));
        cx.global::<UpdateState>().0.phase = Phase::Waiting;
        draw(&mut cx, &mut notice, 375.0);
        let later = notice.panel.button(&cx, ids!(later));
        let click = cx.capture_actions(|cx| {
            cx.widget_action(
                later.widget_uid(),
                ButtonAction::Clicked(KeyModifiers::default()),
            )
        });
        let actions = cx.capture_actions(|cx| {
            notice.handle_event(cx, &Event::Actions(click), &mut Scope::empty())
        });
        assert!(!notice.open);
        assert!(actions.iter().any(|action| matches!(
            action.downcast_ref::<UpdateAction>(),
            Some(UpdateAction::Later)
        )));
    }

    #[test]
    fn update_preview_shows_the_real_panel_without_authorizing_download() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut notice = notice(&mut cx);
        cx.global::<UpdateState>().0 = Snapshot {
            enabled: true,
            preview: true,
            phase: Phase::Available,
            version: "Preview version".into(),
            ..Default::default()
        };
        notice.open = true;
        draw(&mut cx, &mut notice, 1280.0);
        assert!(
            notice
                .panel
                .label(&cx, ids!(status))
                .text()
                .contains(crate::i18n::tr(
                    "Update reminder preview. Downloads and installation are disabled."
                ))
        );
        let primary = notice.panel.button(&cx, ids!(primary));
        let click = cx.capture_actions(|cx| {
            cx.widget_action(
                primary.widget_uid(),
                ButtonAction::Clicked(KeyModifiers::default()),
            )
        });
        let actions = cx.capture_actions(|cx| {
            notice.handle_event(cx, &Event::Actions(click), &mut Scope::empty())
        });
        assert!(
            !actions
                .iter()
                .any(|action| action.downcast_ref::<UpdateAction>().is_some())
        );
    }
}
