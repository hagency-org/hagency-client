//! Keep the pinned Windows renderer's GPU completion receipts moving.
//!
//! D3D11 charges replacement instance buffers until their submission completes.
//! Its render path submits frames but does not poll the completion query. Without
//! an explicit poll, ordinary redraws eventually exhaust the allocation budget
//! and subsequent draw calls are silently skipped, leaving partial/blank windows.
use makepad_widgets::{Cx, Event, Timer};

/// The backend's once-per-frame collector inspects only 64 allocation records.
/// A full chat can replace more than that in one redraw. Visit its inventory
/// with a bounded amount of extra work instead of accumulating old charges.
#[cfg(any(target_os = "windows", test))]
fn collect_completed(
    budget: &mut makepad_widgets::makepad_platform::retained_instances::RetainedAllocationBudget,
    completed: u64,
) {
    for _ in 0..budget.record_count().div_ceil(64).min(16) {
        budget.collect(completed);
    }
}

#[derive(Default)]
pub struct RenderMaintenance {
    timer: Timer,
    timer_ticks: u64,
    #[cfg(debug_assertions)]
    trace_counts: [u64; 4],
}

impl RenderMaintenance {
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.handle_event_in_context(cx, event, true);
    }

    pub fn handle_event_in_context(&mut self, cx: &mut Cx, event: &Event, owns_context: bool) {
        if owns_context && matches!(event, Event::Startup) {
            cx.draw_lists
                .1
                .allocations
                .set_device_limit(crate::memory_budget::GPU_INSTANCE_BYTES);
            cx.set_publication_envelope(16 * 1024 * 1024);
            cx.draw_lists.1.recordings.configure(32 * 1024 * 1024);
            cx.with_vm(|vm| vm.bx.heap.set_max_heap_bytes(Some(128 * 1024 * 1024)));
        }
        if matches!(event, Event::Shutdown) {
            if !self.timer.is_empty() {
                cx.stop_timer(self.timer);
                self.timer = Timer::empty();
            }
            return;
        }
        if self.timer.is_empty() {
            // Complete the last submitted frame even when the UI is idle.
            self.timer = cx.start_interval(0.25);
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("HAGENCY_UI_IDLE_TRACE").is_some() {
            match event {
                Event::Draw(_) => self.trace_counts[0] += 1,
                Event::NextFrame(_) => self.trace_counts[1] += 1,
                Event::Actions(actions) => {self.trace_counts[2] += 1; self.trace_counts[3] += actions.len() as u64;},
                _ => (),
            }
            if self.timer.is_event(event).is_some() && self.timer_ticks.is_multiple_of(4) {
                makepad_widgets::log!("UI idle trace: draws={} frames={} action_batches={} actions={} shader_clock={} post_draw={}",
                    self.trace_counts[0],self.trace_counts[1],self.trace_counts[2],self.trace_counts[3],cx.demo_time_repaint,cx.post_draw_hook.is_some());
                self.trace_counts = [0; 4];
            }
        }
        let timed = self.timer.is_event(event).is_some();
        if timed {
            self.timer_ticks += 1;
            if owns_context && self.timer_ticks.is_multiple_of(4) {
                // Host-side script_apply calls do not necessarily reach a
                // VM eval boundary. Collect between UI events, before the
                // retained heap reaches its admission cap.
                cx.with_vm(|vm| {
                    if vm.bx.heap.accounted_heap_bytes() >= 32 * 1024 * 1024 {
                        vm.gc();
                        vm.bx.heap.reconcile_heap_bytes();
                    }
                });
            }
        }
        if matches!(event, Event::Draw(_)) || timed {
            crate::memory_budget::collect_images(cx);
        }
        #[cfg(target_os = "windows")]
        {
            if matches!(event, Event::Draw(_)) || self.timer.is_event(event).is_some() {
                // Nonblocking; this observes an older query and queues the next
                // one. It neither redraws the UI nor waits for the GPU.
                let completed = cx.frame_completion_serial();
                collect_completed(&mut cx.draw_lists.1.allocations, completed);
                if self.timer.is_event(event).is_some() {
                    if (self.timer_ticks == 4 || self.timer_ticks.is_multiple_of(40))
                        && std::env::var_os("HAGENCY_RENDER_TRACE").is_some()
                    {
                        let budget = &cx.draw_lists.1.allocations;
                        makepad_widgets::log!(
                            "GPU allocation maintenance: submitted={} completed={} bytes={} records={} refusals={} textures={} image_working={}",
                            cx.frame_submission_serial(),
                            completed,
                            budget.bytes(),
                            budget.record_count(),
                            budget.refusals(),
                            cx.texture_pool_bytes(),
                            crate::memory_budget::image_budget_bytes(cx)
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_platform::retained_instances::RetainedAllocationBudget;

    #[test]
    fn hosted_client_preserves_host_memory_limits() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.draw_lists
            .1
            .allocations
            .set_device_limit(80 * 1024 * 1024);
        cx.draw_lists.1.recordings.configure(48 * 1024 * 1024);
        cx.with_vm(|vm| vm.bx.heap.set_max_heap_bytes(Some(192 * 1024 * 1024)));
        let mut maintenance = RenderMaintenance::default();
        maintenance.handle_event_in_context(&mut cx, &Event::Startup, false);
        assert_eq!(cx.draw_lists.1.allocations.limit(), 80 * 1024 * 1024);
        assert_eq!(cx.draw_lists.1.recordings.limit(), 48 * 1024 * 1024);
        assert_eq!(
            cx.with_vm(|vm| vm.bx.heap.max_heap_bytes()),
            Some(192 * 1024 * 1024)
        );
        maintenance.handle_event(&mut cx, &Event::Startup);
        assert_eq!(
            cx.draw_lists.1.allocations.limit(),
            crate::memory_budget::GPU_INSTANCE_BYTES
        );
        assert_eq!(cx.draw_lists.1.recordings.limit(), 32 * 1024 * 1024);
        maintenance.handle_event(&mut cx, &Event::Shutdown);
    }

    #[test]
    fn completed_allocations_keep_up_with_complex_redraws() {
        let mut budget = RetainedAllocationBudget::new(4 * 1024 * 1024);
        let mut live = Vec::new();
        // No backend disposal worker in this unit fixture; keep its metadata
        // queue below 4096. The native example also exercises worker draining.
        for frame in 1..=12 {
            // 256 independent draw calls, beyond the upstream 64-record pass.
            collect_completed(&mut budget, frame - 1);
            live.clear();
            for _ in 0..256 {
                let allocation = budget.reserve(1024).expect("no missing draw calls");
                allocation.submitted(frame);
                live.push(allocation);
            }
            assert!(
                budget.bytes() <= 768 * 1024,
                "retired records must not accumulate"
            );
        }
        assert_eq!(budget.refusals(), 0);
        live.clear();
        collect_completed(&mut budget, 12);
        assert_eq!(budget.bytes(), 0);
    }

    #[test]
    fn pending_gpu_work_and_live_owners_are_not_collected() {
        let mut budget = RetainedAllocationBudget::new(1024);
        let allocation = budget.reserve(512).unwrap();
        allocation.submitted(10);
        collect_completed(&mut budget, 10);
        assert_eq!(budget.bytes(), 512, "live owner retains its allocation");
        drop(allocation);
        collect_completed(&mut budget, 9);
        assert_eq!(budget.bytes(), 512, "GPU must finish before collection");
        collect_completed(&mut budget, 10);
        assert_eq!(budget.bytes(), 0);
    }
}
