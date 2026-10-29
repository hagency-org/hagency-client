use std::borrow::Cow;
use crate::theme::Snapshot as ThemeSnapshot;
use crossbeam_queue::SegQueue;
use makepad_widgets::*;
use crate::{LivePtr, view_from_live_ptr};

struct PendingPopup {
    epoch: Option<u64>,
    item: PopupItem,
}
static PENDING_POPUP_NOTIFICATIONS: SegQueue<PendingPopup> = SegQueue::new();

/// Displays a new popup notification with a popup item.
///
/// This function can be used when there is no Makepad widget context in its arguments.
/// Popup notifications will be shown in the order they were enqueued,
/// and can be removed when manually closed by the user or automatically.
/// Maximum auto dismissal duration is 3 minutes.
pub fn enqueue_popup_notification(
    message: impl Into<Cow<'static, str>>,
    kind: PopupKind,
    auto_dismissal_duration: Option<f64>,
) {
    enqueue_popup(None, message, kind, auto_dismissal_duration);
}

/// Diagnostics belonging to one original account operation are checked again
/// when the UI actually dequeues them, including messages posted before logout.
pub(crate) fn enqueue_scoped_popup_notification(
    epoch: u64,
    message: impl Into<Cow<'static, str>>,
    kind: PopupKind,
    duration: Option<f64>,
) {
    enqueue_popup(Some(epoch), message, kind, duration);
}
fn enqueue_popup(
    epoch: Option<u64>,
    message: impl Into<Cow<'static, str>>,
    kind: PopupKind,
    auto_dismissal_duration: Option<f64>,
) {
    let mut popup_item = PopupItem {
        message: message.into(),
        kind,
        auto_dismissal_duration,
    };
    // Limit auto dismiss duration to 180 seconds
    popup_item.auto_dismissal_duration = popup_item
        .auto_dismissal_duration
        .map(|duration| duration.min(3. * 60.));
    PENDING_POPUP_NOTIFICATIONS.push(PendingPopup {
        epoch,
        item: popup_item,
    });
    SignalToUI::set_ui_signal();
}

fn dequeue_current_popup(
    queue: &SegQueue<PendingPopup>,
    current: impl Fn() -> (u64, bool),
) -> Option<PendingPopup> {
    while let Some(pending) = queue.pop() {
        let (epoch, logout) = current();
        if pending
            .epoch
            .is_none_or(|original| original == epoch && !logout)
        {
            return Some(pending);
        }
    }
    None
}

/// Retrieves a mutable reference to the global `RobrixPopupNotificationRef`.
///
/// This function accesses the global context to obtain a reference to the
/// `RobrixPopupNotificationRef`, which is used for managing and displaying
/// popup notifications within the application. It enables interaction with
/// the popup notification system from various parts of the application.
pub fn get_global_popup_list(cx: &mut Cx) -> &mut RobrixPopupNotificationRef {
    cx.get_global::<RobrixPopupNotificationRef>()
}

/// Sets the global popup list notification widget reference.
///
/// This function sets the global context to point to the provided
/// `WidgetRef`, which is expected to be a `RobrixPopupNotificationRef`.
/// It is used to display popup notifications anywhere in the application.
pub fn set_global_popup_list(cx: &mut Cx, parent_ref: &WidgetRef) {
    Cx::set_global(
        cx,
        parent_ref.robrix_popup_notification(cx, ids!(popup_notification)),
    );
}

/// Kind of a popup notification.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PopupKind {
    /// Shows no icon at all.
    #[default]
    Blank,
    /// Shows a red background and a error icon.
    Error,
    /// Shows a white background and a blue info icon.
    Info,
    /// Shows a green background and a checkmark icon.
    Success,
    /// Shows a yellow background and a warning icon.
    Warning,
}

/// Popup notification item.
#[derive(Default, Debug, Clone)]
pub struct PopupItem {
    /// Text to be displayed in the popup.
    pub message: Cow<'static, str>,
    /// Duration in seconds after which the popup will be automatically closed.
    /// Maximum duration is 3 minutes.
    /// If none, the popup will not automatically close.
    pub auto_dismissal_duration: Option<f64>,
    /// Kind of the popup defined by [`PopupKind`].
    pub kind: PopupKind,
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.ProgressBar = View {
        width: Fill,
        height: 10,
        show_bg: true,
        margin: Inset{ bottom: 0 },
        padding: 0,
        draw_bg +: {
            direction: uniform(0.0), // Direction of the progress bar: 0.0 is right to left, 1.0 is top to bottom.
            border_radius:uniform(4.),
            border_size:uniform(1.0),
            progress_bar_color: uniform(#00000080), //Black with 50% opacity.
            // Display progress bar when there is auto_dismissal_duration.
            display_progress_bar: instance(1.0)
            anim_time: instance(0.0),
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size);
                let rect_size = self.rect_size;
                // The animator owns this clock and stops when the popup ends.
                // A draw_pass.time reference marks even the disabled debug branch
                // as time-dependent and keeps the entire window repainting.
                let time = self.anim_time;
                if self.display_progress_bar > 0.5 {
                    if self.direction > 0.5 {
                        // Top to bottom
                        sdf.box(
                            self.border_size * 2.0,
                            self.border_size * 2.0,
                            rect_size.x - self.border_size * 2.0,
                            rect_size.y * min(1.0, time) - self.border_size * 2.0,
                            max(1.0, self.border_radius)
                        )
                    } else {
                        // Right to left
                        sdf.box(
                            self.border_size * 2.0,
                            self.border_size * 2.0,
                            rect_size.x * max(0.0, 1.0 - time) - self.border_size * 2.0,
                            rect_size.y - self.border_size * 2.0,
                            max(1.0, self.border_radius)
                        )
                    }
                    sdf.fill(self.progress_bar_color);
                }
                return sdf.result
            }
        }
        animator: Animator{
            progress: {
                default: @off
                off: AnimatorState{
                    redraw: true
                    from: {all: Snap}
                    apply: {
                        draw_bg: {anim_time: 0.0}
                    }
                }
                on: AnimatorState{
                    redraw: true
                    from: {all: Forward {duration: 1.0}}
                    apply: {
                        draw_bg: {anim_time: 1.0}
                    }
                }
            }
        }
    }
    mod.widgets.MainContent = View {
        width: Fill,
        height: Fit,
        align: Align{ x: 0.0, y: 0.5 }
        padding: Inset{left: 0, top: 0, bottom: 10, right: 5}
        popup_label := RinxLabel {
            width: Fill,
            height: Fit,
            draw_text +: {
                color: mod.widgets.RINX_INK
                text_style: mod.widgets.MESSAGE_TEXT_STYLE { font_size: mod.widgets.RINX_BODY_SIZE },
            }
        }
    }
    mod.widgets.LeftSideView = View {
        width: Fit,
        height: Fit,
        visible: false,
        popup_icon := Icon {
            width: 28,
            height: 28,
            draw_icon +: {
                svg: (ICON_CHECKMARK),
                color: (COLOR_PRIMARY),
            }
            icon_walk: Walk{ width: 22, height: 22 }
        }
    }

    mod.widgets.PopupCloseButton = RinxIconButton {
        draw_icon.svg: ICON_CLOSE
    }

    mod.widgets.CloseButtonView = View {
        width: Fill,
        height: Fit,
        flow: Down,
        padding: Inset{ top: 5, right: 5 }
        align: Align{ x: 1.0 }

        close_button := mod.widgets.PopupCloseButton {}
    }

    // Other possible color themes that is not too glaring.
    // COLOR_POPUP_GREEN = #43bb9e;
    // COLOR_POPUP_RED = #e74c3c;
    mod.widgets.PopupDialogRightToLeftProgress = RoundedView {
        width: 275
        height: Fit
        padding: 0,
        flow: Overlay
        show_bg: true,
        draw_bg +: {
            border_radius: uniform(4.0)
            border_color: instance(mod.widgets.RINX_INK)
            border_size: uniform(2.0)
            color: instance(mod.widgets.RINX_SURFACE)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size);
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )
                sdf.fill_keep(self.color)

                // Only draw black border for white background (blank popups)
                if length(self.color.rgb - vec3(1.0, 1.0, 1.0)) < 0.1 {
                    sdf.stroke(
                        self.border_color,
                        self.border_size
                    )
                }
                return sdf.result
            }
        }

        popup_content := View {
            width: Fill, height: Fit,
            flow: Down
            //Right side view with close button
            close_button_view := mod.widgets.CloseButtonView {}
            padding: Inset{ right: 2, top: 2}
            inner := View {
                width: Fill, height: Fit,
                padding: Inset{ top: 0, right: 5, bottom: 0, left: 10 }
                flow: Right,
                align: Align{
                    y: 0,
                }
                // Left side with icon for popup kind.
                left_side_view := mod.widgets.LeftSideView {}
                // Main content area
                main_content := mod.widgets.MainContent {}
            }
            progress_bar := mod.widgets.ProgressBar {}
            // Add a small gap between the progress bar and the end of the popup
            // to ensure the progress bar is within the popup.
            View {
                height: 0.2
            }
        }
    }
    mod.widgets.PopupDialogTopToBottomProgress = mod.widgets.PopupDialogRightToLeftProgress {
        popup_content := View {
            width: Fill,
            height: Fit,
            flow: Right,
            spacing: 0,
            align: Align{ x: 0.0, y: 0.5 }
            // Left side with for popup kind.
            left_side_view := mod.widgets.LeftSideView {
                height: Fit,
                margin: Inset{left: 10 }
                spacing: 0,
            }
            inner := View {
                width: 230,
                height: Fit,
                padding: 0,
                flow: Down,
                close_button_view := mod.widgets.CloseButtonView {}
                // Main content area
                main_content := mod.widgets.MainContent {
                    padding: Inset{left: 0}
                }
            }
            progress_bar := mod.widgets.ProgressBar {
                width: 10,
                height: Fill,
                draw_bg +: {
                    direction: 1.0,
                    border_radius: 2.0,
                }
            }
        }
    }

    mod.widgets.RobrixPopupNotificationRightToLeftProgress = set_type_default() do #(RobrixPopupNotification::register_widget(vm)) {
        ..mod.widgets.SolidView

        width: 275
        height: Fit
        flow: Down
        draw_bg +: {
            color: (COLOR_TRANSPARENT)
        }
        content: mod.widgets.PopupDialogRightToLeftProgress {}
    }

    mod.widgets.RobrixPopupNotificationTopToBottomProgress = mod.widgets.RobrixPopupNotificationRightToLeftProgress {
        content: mod.widgets.PopupDialogTopToBottomProgress {}
    }

    // A widget that displays a vertical list of popups at the top right corner of the screen.
    // The progress bar slides from right to left.
    // The margins are to ensure that popups don't get rendered in the device safe inset area.
    mod.widgets.PopupList = View {
        width: Fill,
        height: Fill,
        margin: Inset{
            top: 10,
            left: (mod.widgets.SAFE_INSET_PAD_LEFT),
            right: (mod.widgets.SAFE_INSET_PAD_RIGHT),
        }
        align: Align{x: 0.99, }
        popup_notification := mod.widgets.RobrixPopupNotificationRightToLeftProgress {}
    }

    // A widget that displays a vertical list of popups at the top right corner of the screen.
    // The progress bar slides from top to bottom.
    mod.widgets.PopupListTopToBottomProgress = mod.widgets.PopupList {
        popup_notification := mod.widgets.RobrixPopupNotificationTopToBottomProgress {}
    }
}

/// A widget that displays a vertical list of popups.
struct PopupEntry {
    epoch: Option<u64>,
    kind: PopupKind,
    view: View,
    close_timer: Timer,
}

/// A widget that displays a vertical list of popups.
#[derive(Script, Widget)]
pub struct RobrixPopupNotification {
    #[rust]
    appearance: ThemeSnapshot,
    #[rust]
    restyle: bool,
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub content: Option<LivePtr>,

    #[rust]
    draw_list: Option<DrawList2d>,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[layout]
    layout: Layout,
    #[walk]
    walk: Walk,
    // A list of tuples containing individual widgets, its content and the close timer in the order they were added.
    #[rust]
    popups: Vec<PopupEntry>,
}

impl ScriptHook for RobrixPopupNotification {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        _value: ScriptValue,
    ) {
        self.appearance = crate::theme::snapshot_for_vm(vm);
        if apply.is_script_reapply() {
            if let Some(template) = self.content {
                for popup in &mut self.popups {
                    let text = popup.view.label(vm.cx_mut(), ids!(popup_label)).text();
                    popup.view.script_apply(vm, apply, scope, template);
                    popup
                        .view
                        .label(vm.cx_mut(), ids!(popup_label))
                        .set_text(vm.cx_mut(), &text);
                }
            }
            self.restyle = true;
        }
        vm.with_cx_mut(|cx| {
            if let Some(draw_list) = &self.draw_list {
                draw_list.redraw(cx);
            }
        });
    }
}

impl Widget for RobrixPopupNotification {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.prune_for_scope(
            cx,
            crate::account_session::epoch(),
            crate::logout::logout_state_machine::is_logout_in_progress(),
        );
        if std::mem::take(&mut self.restyle) {
            for popup in &mut self.popups {
                style_popup(cx, &mut popup.view, popup.kind, &self.appearance);
            }
        }
        if matches!(event, Event::Signal) {
            while let Some(popup_item) = dequeue_current_popup(&PENDING_POPUP_NOTIFICATIONS, || {
                (
                    crate::account_session::epoch(),
                    crate::logout::logout_state_machine::is_logout_in_progress(),
                )
            }) {
                self.push_pending(cx, popup_item);
            }
        }
        if self.popups.is_empty() {
            return;
        }

        let mut remove_index = None;

        for (index, popup) in self.popups.iter_mut().enumerate() {
            popup.view.handle_event(cx, event, scope);

            if remove_index.is_none() && popup.close_timer.is_event(event).is_some() {
                remove_index = Some(index);
            }
        }

        if let Some(index) = remove_index {
            self.popups.remove(index);
            self.redraw_overlay(cx);
        }

        self.widget_match_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let draw_list = self.draw_list.as_mut().unwrap();
        draw_list.begin_overlay_reuse(cx);
        self.draw_bg.begin(cx, walk, self.layout);
        if !self.popups.is_empty() {
            cx.begin_turtle(walk, self.layout);
            for popup in self.popups.iter_mut() {
                let walk = walk.with_margin_bottom(5.0);
                let _ = popup.view.draw_walk(cx, scope, walk);
            }
            cx.end_turtle();
        }
        self.draw_bg.end(cx);
        draw_list.end(cx);
        DrawStep::done()
    }
}

impl RobrixPopupNotification {
    fn push_pending(&mut self, cx: &mut Cx, pending: PendingPopup) {
        self.push(cx, pending.item);
        if let Some(entry) = self.popups.last_mut() {
            entry.epoch = pending.epoch;
        }
    }
    fn prune_for_scope(&mut self, cx: &mut Cx, epoch: u64, logout: bool) {
        let before = self.popups.len();
        self.popups.retain(|popup| {
            let keep = popup
                .epoch
                .is_none_or(|original| original == epoch && !logout);
            if !keep {
                cx.stop_timer(popup.close_timer);
            }
            keep
        });
        if self.popups.len() != before {
            self.redraw_overlay(cx);
        }
    }
    fn redraw_overlay(&self, cx: &mut Cx) {
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
        self.draw_bg.redraw(cx);
    }

    /// Adds a new popup with a close button to the right side of the screen.
    ///
    /// The popup's content is a string given by the `PopupItem` parameter.
    /// New popup will be displayed below the previous ones.
    pub fn push(&mut self, cx: &mut Cx, popup_item: PopupItem) {
        let mut view = view_from_live_ptr(cx, self.content);
        view.label(cx, ids!(popup_label))
            .set_text(cx, &popup_item.message);
        style_popup(cx, &mut view, popup_item.kind, &self.appearance);

        let close_timer = if let Some(duration) = popup_item.auto_dismissal_duration {
            let mut progress_bar = view.view(cx, ids!(popup_content.progress_bar));
            script_apply_eval!(cx, progress_bar, {
                draw_bg +: { display_progress_bar: 1.0 }
            });
            progress_bar.animator_cut(cx, ids!(progress.off));
            progress_bar.animator_play_with(cx, ids!(progress.on), Play::Forward { duration });
            cx.start_timeout(duration)
        } else {
            let mut progress_bar = view.view(cx, ids!(popup_content.progress_bar));
            script_apply_eval!(cx, progress_bar, {
                draw_bg +: { display_progress_bar: 0.0 }
            });
            progress_bar.animator_cut(cx, ids!(progress.off));
            Timer::empty()
        };
        self.popups.push(PopupEntry {
            epoch: None,
            kind: popup_item.kind,
            view,
            close_timer,
        });
        self.redraw_overlay(cx);
    }

    /// Adds a new popup with a custom view to the right side of the screen.
    ///
    /// This allows arbitrary content to be displayed via RobrixPopupNotification's content live dsl.
    ///
    /// The `view` parameter should be constructed using `RobrixPopupNotification::content()`.
    /// The view should have a view with the id `popup_content` which should contain
    /// a `progress_bar` view with the id `progress_bar` and a `popup_label` view with the id `popup_label`.
    /// The `popup_label` view should have a `draw_text` field that will be used to display the popup's message.
    /// The custom view should also have a close button with the id `close_button` which should have a `draw_icon` field that will be used to display the close button's icon.
    ///
    /// The `auto_dismissal_duration` field of the `PopupItem` parameter will be used to automatically dismiss the popup after the given duration.
    /// If `auto_dismissal_duration` is `None`, the popup will not be automatically dismissed and the user will have to manually close it.
    /// The maximum auto dismissal duration is 3 minutes.
    pub fn push_with_custom_view(
        &mut self,
        mut popup_item: PopupItem,
        view: View,
        close_timer: Timer,
    ) {
        popup_item.auto_dismissal_duration = popup_item
            .auto_dismissal_duration
            .map(|duration| duration.min(3. * 60.));
        self.popups.push(PopupEntry {
            epoch: None,
            kind: popup_item.kind,
            view,
            close_timer,
        });
    }

    /// Returns a clone of the template for each  popup in the list.
    ///
    /// This is used to construct the View for the popup notification.
    pub fn content(&self) -> Option<LivePtr> {
        self.content
    }
}

impl WidgetMatchEvent for RobrixPopupNotification {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        if actions.iter().any(|a| {
            matches!(
                a.downcast_ref(),
                Some(crate::logout::logout_confirm_modal::LogoutAction::ClearAppState { .. })
            )
        }) {
            while PENDING_POPUP_NOTIFICATIONS.pop().is_some() {}
            for popup in self.popups.drain(..) {
                cx.stop_timer(popup.close_timer);
            }
            self.redraw_overlay(cx);
            return;
        }
        for (i, popup) in self.popups.iter_mut().enumerate() {
            if popup.view.button(cx, ids!(close_button)).clicked(actions) {
                cx.stop_timer(popup.close_timer);
                self.popups.remove(i);
                self.redraw_overlay(cx);
                break;
            }
        }
    }
}
impl RobrixPopupNotificationRef {
    /// See [`RobrixPopupNotification::push()`].
    pub fn push(&self, cx: &mut Cx, popup_item: PopupItem) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.push(cx, popup_item);
        } else {
            log!("RobrixPopupNotificationRef is not initialized.");
        }
    }

    /// See [`RobrixPopupNotification::content()`].
    pub fn content(&self) -> Option<LivePtr> {
        if let Some(inner) = self.borrow() {
            inner.content()
        } else {
            None
        }
    }

    /// See [`RobrixPopupNotification::push_with_custom_view()`].
    pub fn push_with_custom_view(&self, popup_item: PopupItem, view: View, close_timer: Timer) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.push_with_custom_view(popup_item, view, close_timer);
        } else {
            log!("RobrixPopupNotificationRef is not initialized.");
        }
    }
}

/// Recolors an existing popup without restarting its dismissal timer.
fn style_popup(cx: &mut Cx, view: &mut View, kind: PopupKind, theme: &ThemeSnapshot) {
    let (fg, bg) = super::feedback::colors(theme, kind);
    view.view(cx, ids!(popup_content.inner.left_side_view))
        .set_visible(cx, kind != PopupKind::Blank);
    let mut icon = view.widget(cx, ids!(popup_content.inner.left_side_view.popup_icon));
    match kind {
        PopupKind::Error => {
            script_apply_eval!(cx,icon,{draw_icon.svg:mod.widgets.ICON_FORBIDDEN});
        }
        PopupKind::Info => {
            script_apply_eval!(cx,icon,{draw_icon.svg:mod.widgets.ICON_INFO});
        }
        PopupKind::Success => {
            script_apply_eval!(cx,icon,{draw_icon.svg:mod.widgets.ICON_CHECKMARK});
        }
        PopupKind::Warning => {
            script_apply_eval!(cx,icon,{draw_icon.svg:mod.widgets.ICON_WARNING});
        }
        PopupKind::Blank => {}
    }
    script_apply_eval!(cx,icon,{draw_icon.color:#(fg)});
    view.label(cx, ids!(popup_label)).set_text_color(cx, fg);
    let mut close = view.button(cx, ids!(close_button));
    script_apply_eval!(cx,close,{draw_bg +: {color: #(bg) color_hover: #(bg) color_down: #(bg) border_color: #(fg) border_color_hover: #(fg) border_color_down: #(fg)} draw_icon.color: #(fg)});
    script_apply_eval!(cx,view,{draw_bg.color:#(bg)});
}

#[cfg(test)]
mod scoped_popup_tests {
    use super::*;
    fn pending(epoch: Option<u64>, message: &'static str) -> PendingPopup {
        PendingPopup {
            epoch,
            item: PopupItem {
                message: message.into(),
                ..Default::default()
            },
        }
    }
    #[test]
    fn ui_dequeue_drops_old_account_filename_and_preserves_current_and_global_messages() {
        let queue = SegQueue::new();
        queue.push(pending(Some(7), "old-account-private-name.pdf"));
        queue.push(pending(Some(8), "current-account-result"));
        queue.push(pending(None, "global-status"));
        assert_eq!(
            dequeue_current_popup(&queue, || (8, false))
                .unwrap()
                .item
                .message,
            "current-account-result"
        );
        assert_eq!(
            dequeue_current_popup(&queue, || (9, false))
                .unwrap()
                .item
                .message,
            "global-status"
        );
        assert!(dequeue_current_popup(&queue, || (9, false)).is_none());
    }
    #[test]
    fn ui_dequeue_rejects_scoped_message_during_logout_but_keeps_global_api_behavior() {
        let queue = SegQueue::new();
        queue.push(pending(Some(8), "account-result"));
        queue.push(pending(None, "global-status"));
        assert_eq!(
            dequeue_current_popup(&queue, || (8, true))
                .unwrap()
                .item
                .message,
            "global-status"
        );
        assert!(dequeue_current_popup(&queue, || (8, false)).is_none());
    }
    #[test]
    fn already_visible_scoped_attachment_diagnostic_is_removed_when_account_changes() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut widget = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            vm.bx.captured_errors = Some(Vec::new());
            let value =
                script_eval!(vm,{mod.widgets.RobrixPopupNotificationRightToLeftProgress {}});
            let widget = RobrixPopupNotification::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty());
            widget
        });
        widget.push_pending(&mut cx, pending(Some(7), "old-account-private-name.pdf"));
        widget.push_pending(&mut cx, pending(Some(8), "current-account-result"));
        widget.push(
            &mut cx,
            PopupItem {
                message: "global-status".into(),
                ..Default::default()
            },
        );
        widget.prune_for_scope(&mut cx, 8, false);
        assert_eq!(widget.popups.len(), 2);
        assert!(widget.popups.iter().all(|popup| popup.epoch != Some(7)));
        widget.prune_for_scope(&mut cx, 8, true);
        assert_eq!(widget.popups.len(), 1);
        assert_eq!(widget.popups[0].epoch, None);
    }
}
