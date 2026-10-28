//! Busy animation schedules work only while the spinner is actually drawn.
use makepad_widgets::*;
use std::time::Instant;

script_mod! {
    use mod.prelude.widgets.*
    mod.widgets.LoadingSpinner = #(VisibleSpinner::register_widget(vm)) {
        width: Fill height: Fill show_bg: true
        draw_bg +: {
            color: uniform(theme.color_makepad)
            stroke_width: uniform(3.0)
            rotation_speed: uniform(1.2)
            phase: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let radius = min(self.rect_size.x, self.rect_size.y) * 0.5 - self.stroke_width * 0.5
                let rotation = self.phase * self.rotation_speed * 2.0 * PI
                let gap = mix(0.12, 0.92, 0.5 + 0.5 * sin(rotation * 0.5)) * 2.0 * PI
                sdf.arc_round_caps(self.rect_size.x * 0.5, self.rect_size.y * 0.5,
                    radius, rotation, rotation + 2.0 * PI - gap, self.stroke_width)
                return sdf.fill(self.color)
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct VisibleSpinner {
    #[deref] view: View,
    #[rust] timer: Timer,
    #[rust] started: Option<Instant>,
    #[rust] drawn_since_tick: bool,
}
impl Widget for VisibleSpinner {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if matches!(event, Event::Shutdown) {
            cx.stop_timer(self.timer);
            self.timer = Timer::default();
        } else if self.timer.is_event(event).is_some() {
            self.timer = Timer::default();
            if std::mem::take(&mut self.drawn_since_tick) && self.view.visible {
                let phase = self.started.map_or(0.0, |t| t.elapsed().as_secs_f64() % 1024.0);
                script_apply_eval!(cx, self.view, {draw_bg.phase: #(phase)});
                self.view.redraw(cx);
            }
            // The next visible draw rearms the clock. A cached spinner behind
            // a hidden parent can receive this tick but cannot keep it alive.
        }
        self.view.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.view.visible {
            self.drawn_since_tick = true;
            self.started.get_or_insert_with(Instant::now);
            if self.timer.is_empty() { self.timer = cx.start_timeout(1.0 / 30.0); }
        }
        self.view.draw_walk(cx, scope, walk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_draw::cx_draw::CxDraw;
    fn draw(cx: &mut Cx, spinner: &mut VisibleSpinner) {
        let pass = DrawPass::new(cx);
        pass.set_size(cx, dvec2(20.0, 20.0));
        let mut list = DrawList2d::new(cx);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(20.0, 20.0), Layout::flow_overlay());
        spinner.draw_walk_all(&mut cx2d, &mut Scope::empty(), Walk::fixed(20.0, 20.0));
        cx2d.end_pass_sized_turtle();
        list.end(&mut cx2d);
        cx2d.end_pass(&pass);
    }
    #[test]
    fn busy_clock_requires_a_visible_draw_to_continue_and_can_restart() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut spinner = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm,{mod.widgets.LoadingSpinner {}});
            VisibleSpinner::script_from_value(vm, value)
        });
        draw(&mut cx, &mut spinner);
        assert!(!spinner.timer.is_empty());
        let event = Event::Timer(TimerEvent {timer_id: spinner.timer.0, time: Some(0.1)});
        spinner.handle_event(&mut cx, &event, &mut Scope::empty());
        assert!(spinner.timer.is_empty(), "a hidden ancestor cannot perpetuate the clock");
        draw(&mut cx, &mut spinner);
        assert!(!spinner.timer.is_empty(), "visible loading can animate again");
        spinner.view.set_visible(&mut cx, false);
        cx.new_draw_event = DrawEvent::default();
        let event = Event::Timer(TimerEvent {timer_id: spinner.timer.0, time: Some(0.1)});
        spinner.handle_event(&mut cx, &event, &mut Scope::empty());
        assert!(spinner.timer.is_empty());
        assert!(!cx.new_draw_event.redraw_all && cx.new_draw_event.draw_lists.is_empty());
    }
}
