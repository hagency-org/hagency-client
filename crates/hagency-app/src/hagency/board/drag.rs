//! A visual lift/drop lifecycle. The snapshot stays authoritative until the
//! server acknowledges the move; a refused move settles back into its old slot.
use super::*;

impl ProjectBoard {
    pub(super) fn cancel_drag(&mut self, cx: &mut Cx) {
        if let Some(drag) = self.drag.take().filter(|d| d.moved) {
            self.landing = Some(Landing {
                target: drag.source.pos,
                drag,
                started: None,
                progress: 0.0,
                awaiting: false,
                layout_ready: false,
            });
            self.drag_frame = cx.new_next_frame();
        }
        self.view
            .label(cx, ids!(drag_notice))
            .set_visible(cx, false);
        self.view.redraw(cx);
    }

    pub(super) fn advance_landing(&mut self, cx: &mut Cx, time: f64) {
        let Some(landing) = &mut self.landing else {
            return;
        };
        if !landing.awaiting && !self.loading && landing.layout_ready {
            let start = *landing.started.get_or_insert(time);
            landing.progress = ((time - start) / 0.22).clamp(0.0, 1.0);
            if landing.progress >= 1.0 {
                self.landing = None;
            }
        }
        if self.landing.is_some() {
            self.drag_frame = cx.new_next_frame();
        }
        self.view.redraw(cx);
    }

    pub(super) fn resolve_landing_layout(&mut self, cx: &Cx) {
        if let Some(landing) = &mut self.landing
            && !landing.awaiting
            && !self.loading
            && landing.started.is_none()
        {
            // Only read areas AFTER the new snapshot's PortalLists have drawn.
            // A NextFrame may arrive between the HTTP reply and that draw.
            landing.target = self
                .card_areas
                .get(&landing.drag.task.id)
                .map(|a| a.rect(cx))
                .filter(|r| r.size.y > 0.0)
                .map(|r| r.pos)
                .unwrap_or(landing.drag.source.pos);
            landing.layout_ready = true;
        }
    }

    pub(super) fn draw_drag_preview(&mut self, cx: &mut Cx2d) {
        let active = self.drag.as_ref().is_some_and(|d| d.moved) || self.landing.is_some();
        // Tooltip::hide schedules a redraw even when already closed. Only clear
        // the overlay on the active-to-idle transition, never on every frame.
        if !active && !self.drag_preview_active {
            return;
        }
        self.drag_preview_active = active;
        let marker = self.view.tooltip(cx, ids!(drop_marker));
        marker.hide(cx);
        if let Some(drag) = self.drag.as_ref().filter(|d| d.moved) {
            if let Some(index) = (0..self.column_count()).find(|i| {
                self.view
                    .view(cx, column_path(*i))
                    .area()
                    .rect(cx)
                    .contains(drag.at)
            }) {
                let full = self.snapshot.as_ref().is_some_and(|s| {
                    let lane = &s.lanes[index];
                    lane.state != drag.task.state
                        && lane.wip_limit.is_some_and(|limit| lane.task_count >= limit)
                });
                let mut line = self.view.view(cx, ids!(drop_marker.content));
                if full {
                    script_apply_eval!(cx,line,{draw_bg.color:mod.widgets.RINX_WARNING_FG});
                } else {
                    script_apply_eval!(cx,line,{draw_bg.color:mod.widgets.RINX_ACCENT});
                }
                let viewport = self
                    .view
                    .view(cx, column_path(index))
                    .portal_list(cx, ids!(cards))
                    .area()
                    .rect(cx);
                let neighbours = self.drop_neighbours(cx, index, drag.at, &drag.task.id);
                let mut y = viewport.pos.y;
                if let Some((before, after)) = neighbours {
                    if let Some(rect) = before
                        .as_ref()
                        .and_then(|id| self.card_areas.get(id))
                        .map(|a| a.rect(cx))
                    {
                        y = rect.pos.y - 5.0;
                    } else if let Some(rect) = after
                        .as_ref()
                        .and_then(|id| self.card_areas.get(id))
                        .map(|a| a.rect(cx))
                    {
                        y = rect.pos.y + rect.size.y + 3.0;
                    }
                }
                marker.show_with_options(
                    cx,
                    dvec2(
                        viewport.pos.x,
                        y.clamp(
                            viewport.pos.y,
                            (viewport.pos.y + viewport.size.y - 3.0).max(viewport.pos.y),
                        ),
                    ),
                    "",
                );
            }
        }
        let visual = self
            .drag
            .as_ref()
            .filter(|d| d.moved)
            .map(|d| (d, d.position()))
            .or_else(|| self.landing.as_ref().map(|l| (&l.drag, l.position())));
        let preview = self.view.tooltip(cx, ids!(drag_preview));
        if let Some((drag, pos)) = visual {
            let mut card = self.view.widget(cx, ids!(drag_preview.content.floating));
            let width = drag.source.size.x;
            script_apply_eval!(cx,card,{width:#(width)});
            let is_done = self.snapshot.as_ref().is_some_and(|s| {
                s.lanes
                    .iter()
                    .any(|l| l.state == drag.task.state && l.is_done)
            });
            super::interaction::fill_card(cx, &card, &drag.task, is_done, true, false,
                drag.task
                    .assignee_mxid
                    .as_deref()
                    .map(|id| self.member_name(id)),
            );
            // Keep the exact pointer-to-card offset; the card never jumps to a
            // tooltip offset or moves away from the mouse near the window edge.
            preview.show_with_options(cx, pos, "");
        } else {
            preview.hide(cx);
        }
    }
}
