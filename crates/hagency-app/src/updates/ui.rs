use super::{Phase, Snapshot, UpdateAction, UpdateState};
use crate::i18n::Language;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.UpdateSettings = #(UpdateSettings::register_widget(vm)) {
        width: Fill height: Fit flow: Down spacing: 8
        SubsectionLabel { text: #(crate::i18n::tr("Software Updates")) i18n_text: "Software Updates" }
        status := RinxLabel { width: Fill height: Fit flow: Flow.Right{wrap: true} }
        version := RinxLabel { width: Fill height: Fit flow: Flow.Right{wrap: true} }
        View {
            width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
            download := RobrixIconButton { text: #(crate::i18n::tr(super::download_label())) i18n_text: #(super::download_label()) }
            install := RobrixIconButton { text: #(crate::i18n::tr("Restart and Install")) i18n_text: "Restart and Install" }
            later := RobrixIconButton { text: #(crate::i18n::tr("Install Later")) i18n_text: "Install Later" }
            check := RinxGhostButton { text: #(crate::i18n::tr("Check for Updates")) i18n_text: "Check for Updates" }
        }
        auto_check := ToggleFlat { width: Fill height: Fit label_walk +: {width: Fill height: Fit} text: #(crate::i18n::tr("Automatically check for updates")) i18n_text: "Automatically check for updates" }
        auto_download := ToggleFlat { width: Fill height: Fit label_walk +: {width: Fill height: Fit} text: #(crate::i18n::tr("Download updates in the background")) i18n_text: "Download updates in the background" }
        release_notes := RinxGhostButton { text: #(crate::i18n::tr("Release Notes")) i18n_text: "Release Notes" }
        notes_panel := ScrollYView {
            visible: false width: Fill height: Fit{max: FitBound.Abs(160.0)}
            notes := RinxLabel { width: Fill height: Fit flow: Flow.Right{wrap: true} }
        }
    }
}

#[derive(Script, Widget)]
pub struct UpdateSettings {
    #[deref]
    view: View,
    #[rust]
    rendered: Option<(Snapshot, Language)>,
    #[rust]
    notes_open: bool,
}
impl ScriptHook for UpdateSettings {
    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        self.rendered = None;
    }
}

impl Widget for UpdateSettings {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Actions(actions) = event {
            if self.view.button(cx, ids!(release_notes)).clicked(actions) {
                self.notes_open = !self.notes_open;
                self.rendered = None;
            }
            if cx.global::<UpdateState>().0.preview {
                self.view.handle_event(cx, event, scope);
                return;
            }
            for (id, action) in [
                (id!(check), UpdateAction::Check),
                (id!(download), UpdateAction::Download),
                (id!(install), UpdateAction::Install),
                (id!(later), UpdateAction::Later),
            ] {
                if self.view.button(cx, &[id]).clicked(actions) {
                    cx.action(action);
                }
            }
            if let Some(value) = self.view.check_box(cx, ids!(auto_check)).changed(actions) {
                cx.action(UpdateAction::AutoCheck(value));
            }
            if let Some(value) = self
                .view
                .check_box(cx, ids!(auto_download))
                .changed(actions)
            {
                cx.action(UpdateAction::AutoDownload(value));
            }
        }
        if matches!(event, Event::Signal | Event::Actions(_))
            && self.rendered.as_ref() != Some(&(cx.global::<UpdateState>().0.clone(), crate::i18n::language())) {
            self.view.redraw(cx);
        }
        self.view.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let state = cx.global::<UpdateState>().0.clone();
        let language = crate::i18n::language();
        if self.rendered.as_ref() == Some(&(state.clone(), language)) {
            return self.view.draw_walk(cx, scope, walk);
        }
        self.rendered = Some((state.clone(), language));
        self.view.set_visible(
            cx,
            cfg!(any(
                target_os = "windows",
                target_os = "macos",
                all(target_os = "linux", not(target_env = "ohos"))
            )),
        );
        let status = if state.phase == Phase::Downloading {
            format!("{} {}%", crate::i18n::tr(state.status()), state.progress)
        } else {
            crate::i18n::tr(state.status()).to_owned()
        };
        self.view.label(cx, ids!(status)).set_text(cx, &status);
        self.view
            .label(cx, ids!(version))
            .set_text(cx, &state.version);
        self.view.label(cx, ids!(notes)).set_text(cx, &state.notes);
        self.view
            .button(cx, ids!(release_notes))
            .set_visible(cx, !state.notes.is_empty());
        self.view
            .view(cx, ids!(notes_panel))
            .set_visible(cx, self.notes_open && !state.notes.is_empty());
        for (id, active) in [
            (id!(auto_check), state.auto_check),
            (id!(auto_download), state.auto_download),
        ] {
            let control = self.view.check_box(cx, &[id]);
            control.set_active(cx, active, Animate::No);
            control.set_visible(
                cx,
                state.enabled
                    && !state.preview
                    && (id != id!(auto_download) || cfg!(target_os = "windows")),
            );
        }
        self.view.button(cx, ids!(check)).set_visible(
            cx,
            state.enabled
                && matches!(
                    state.phase,
                    Phase::Idle | Phase::Current | Phase::Failed | Phase::Ready | Phase::Available
                ),
        );
        self.view
            .button(cx, ids!(download))
            .set_visible(cx, state.enabled && state.phase == Phase::Available);
        for id in [id!(check), id!(download), id!(install), id!(later)] {
            self.view.button(cx, &[id]).set_enabled(
                cx,
                !state.preview && !state.restart_required && (id != id!(check) || state.can_check),
            );
        }
        self.view
            .button(cx, ids!(install))
            .set_visible(cx, state.enabled && state.phase == Phase::Ready);
        self.view.button(cx, ids!(later)).set_text(
            cx,
            crate::i18n::tr(if super::sparkle_active() {
                "Cancel Update"
            } else {
                "Install Later"
            }),
        );
        self.view.button(cx, ids!(later)).set_visible(
            cx,
            state.enabled
                && (state.phase == Phase::Waiting
                    || (super::sparkle_active()
                        && matches!(
                            state.phase,
                            Phase::Checking | Phase::Downloading | Phase::Verifying | Phase::Ready
                        ))),
        );
        self.view.draw_walk(cx, scope, walk)
    }
}
