//! Skeleton placeholder rows shown while content is loading.
//!
//! A skeleton block pulses between the theme's inset field color and its
//! hover color (driven by a `BounceLoop` animator, ~1.2s per cycle), so
//! skeletons adapt to light and dark themes automatically. When the theme
//! disables motion (`RINX_MOTION` = 0), the pulse never starts and blocks
//! render statically in the base color.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // A single shimmering block; its color pulses between RINX_FIELD and RINX_HOVER.
    mod.widgets.SkeletonBlock = #(SkeletonBlock::register_widget(vm)) {
        width: Fill, height: Fit
        show_bg: true
        draw_bg +: {
            pulse: instance(0.0)
            color: uniform(mod.widgets.RINX_FIELD)
            color_pulse: uniform(mod.widgets.RINX_HOVER)
            border_radius: uniform(mod.widgets.RINX_CONTROL_RADIUS)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0., 0., self.rect_size.x, self.rect_size.y, self.border_radius)
                sdf.fill(self.color.mix(self.color_pulse, self.pulse))
                return sdf.result
            }
        }
        animator: Animator {
            pulse: {
                default: @off
                off: AnimatorState {
                    redraw: true
                    from: {all: Snap}
                    apply: {draw_bg: {pulse: 0.0}}
                }
                on: AnimatorState {
                    redraw: true
                    from: {all: BounceLoop {duration: 0.6, end: 1.0}}
                    apply: {draw_bg: {pulse: 1.0}}
                }
            }
        }
    }

    // A list-entry skeleton: a round avatar placeholder on the left plus
    // rounded text bars (one long, the rest shorter). `show_avatar` and
    // `num_lines` (1..=3) are applied in `SkeletonListEntry::draw_walk`.
    mod.widgets.SkeletonListEntry = #(SkeletonListEntry::register_widget(vm)) {
        width: Fill, height: Fit
        flow: Right
        spacing: 12
        padding: Inset{top: 9, bottom: 9, left: 12, right: 16}

        skeleton_avatar := mod.widgets.SkeletonBlock {
            width: 36, height: 36
            draw_bg +: {border_radius: uniform(18.0)}
        }
        skeleton_lines := View {
            width: Fill, height: Fit
            flow: Down, spacing: 8
            padding: Inset{top: 3}
            skeleton_line_1 := mod.widgets.SkeletonBlock {width: Fill, height: 13}
            skeleton_line_2_wrap := View {
                width: Fill, height: Fit, flow: Right
                skeleton_line_2 := mod.widgets.SkeletonBlock {width: Fill, height: 11}
                View {width: 110, height: 1}
            }
            skeleton_line_3_wrap := View {
                width: Fill, height: Fit, flow: Right
                skeleton_line_3 := mod.widgets.SkeletonBlock {width: Fill, height: 11}
                View {width: 60, height: 1}
            }
        }
    }
}

/// Whether theme motion is enabled (`RINX_MOTION` > 0 seconds).
/// Falls back to the default 150ms when the theme hasn't been installed
/// (e.g., in widget tests).
fn motion_enabled(cx: &mut Cx) -> bool {
    cx.with_vm(|vm| {
        let widgets = vm.module(id!(widgets));
        vm.bx.heap
            .value(widgets, id!(RINX_MOTION).into(), NoTrap)
            .as_number()
            .unwrap_or(0.15) > 0.0
    })
}

/// A single shimmering skeleton block.
#[derive(Script, ScriptHook, Widget, Animator)]
pub struct SkeletonBlock {
    #[rust] drawn_since_frame: bool,
    #[source] source: ScriptObjectRef,
    #[deref] view: View,
    #[apply_default] animator: Animator,
}

impl Widget for SkeletonBlock {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // PortalList retains skeleton widgets after real rows replace them.
        // Stop their looping pulse when a frame no longer draws this block;
        // checking only this View's visible flag misses a hidden ancestor.
        if self.animator.next_frame.is_event(event).is_some()
            && !std::mem::take(&mut self.drawn_since_frame)
        {
            self.animator_cut(cx, ids!(pulse.off));
        }
        if self.animator_handle_event(cx, event).must_redraw() {
            self.redraw(cx);
        }
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.drawn_since_frame = true;
        // Restart only when this block is actually presented again.
        if motion_enabled(cx)
            && !self.animator_in_state(cx, ids!(pulse.on))
            && !self.animator.is_track_animating(id!(pulse))
        {
            self.animator_play(cx, ids!(pulse.on));
        }
        self.view.draw_walk(cx, scope, walk)
    }
}

/// A list-entry skeleton: a round avatar placeholder plus text bars.
#[derive(Script, ScriptHook, Widget)]
pub struct SkeletonListEntry {
    #[source] source: ScriptObjectRef,
    #[deref] view: View,
    /// Whether to show the round avatar placeholder on the left.
    #[live(true)] show_avatar: bool,
    /// Number of text-line placeholders (clamped to 1..=3).
    #[live(2.0)] num_lines: f64,
}

impl Widget for SkeletonListEntry {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Idempotent: `set_visible` no-ops when the flag already matches.
        self.view.view(cx, ids!(skeleton_avatar)).set_visible(cx, self.show_avatar);
        self.view.view(cx, ids!(skeleton_line_2_wrap)).set_visible(cx, self.num_lines >= 2.0);
        self.view.view(cx, ids!(skeleton_line_3_wrap)).set_visible(cx, self.num_lines >= 3.0);
        self.view.draw_walk(cx, scope, walk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_draw::cx_draw::CxDraw;
    fn draw(cx: &mut Cx, block: &mut SkeletonBlock) {
        let pass = DrawPass::new(cx);
        pass.set_size(cx, dvec2(120.0, 30.0));
        let mut list = DrawList2d::new(cx);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(120.0, 30.0), Layout::flow_overlay());
        block.draw_walk_all(&mut cx2d, &mut Scope::empty(), Walk::fixed(120.0, 30.0));
        cx2d.end_pass_sized_turtle();
        list.end(&mut cx2d);
        cx2d.end_pass(&pass);
    }
    fn frame(cx: &mut Cx, block: &mut SkeletonBlock, time: f64) {
        let event = Event::NextFrame(NextFrameEvent {
            frame: (time * 60.0) as u64, time,
            set: [block.animator.next_frame].into_iter().collect(),
        });
        block.handle_event(cx, &event, &mut Scope::empty());
    }
    #[test]
    fn retained_placeholder_stops_after_ancestor_hides_and_restarts_when_drawn() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut block = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm,{mod.widgets.SkeletonBlock {}});
            SkeletonBlock::script_from_value(vm, value)
        });
        draw(&mut cx, &mut block);
        frame(&mut cx, &mut block, 0.0);
        assert!(block.animator.is_track_animating(id!(pulse)));
        draw(&mut cx, &mut block);
        frame(&mut cx, &mut block, 0.2);
        assert!(block.animator.is_track_animating(id!(pulse)));
        // The parent list has replaced the placeholder. The retained block is
        // still individually visible, but no longer participates in drawing.
        assert!(block.view.visible);
        frame(&mut cx, &mut block, 0.4);
        assert!(!block.animator.is_track_animating(id!(pulse)));
        assert!(block.animator_in_state(&cx, ids!(pulse.off)));
        draw(&mut cx, &mut block);
        assert!(block.animator.is_track_animating(id!(pulse)));
    }
}
