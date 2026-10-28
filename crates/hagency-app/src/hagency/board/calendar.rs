//! Optional, local-calendar due dates. Opening the picker never assigns a date.
use chrono::{Datelike, Local, Months, NaiveDate};

use super::*;

fn month_start(date: NaiveDate) -> NaiveDate {
    date.with_day(1).expect("every month has a first day")
}

fn calendar_days(month: NaiveDate) -> [Option<NaiveDate>; 42] {
    let first = month_start(month);
    let offset = first.weekday().num_days_from_monday() as i64;
    std::array::from_fn(|index| {
        first
            .checked_add_signed(chrono::Duration::days(index as i64 - offset))
            .filter(|date| date.month() == first.month() && date.year() == first.year())
    })
}

fn day_id(index: usize) -> [LiveId; 1] { [LiveId::from_str(&format!("calendar_day_{index}"))] }

impl ProjectBoard {
    pub(super) fn close_calendar(&mut self, cx: &mut Cx) {
        self.calendar_month = None;
        self.view.view(cx, ids!(calendar)).set_visible(cx, false);
    }

    pub(super) fn open_calendar(&mut self, cx: &mut Cx) {
        let selected = self.view.text_input(cx, ids!(due_date)).text();
        self.calendar_month = Some(month_start(
            NaiveDate::parse_from_str(&selected, "%Y-%m-%d")
                .unwrap_or_else(|_| Local::now().date_naive()),
        ));
        self.view.view(cx, ids!(calendar)).set_visible(cx, true);
        self.sync_calendar(cx);
    }

    fn sync_calendar(&mut self, cx: &mut Cx) {
        let Some(month) = self.calendar_month else {
            return;
        };
        let calendar = self.view.view(cx, ids!(calendar));
        calendar
            .label(cx, ids!(calendar_month))
            .set_text(cx, &month.format("%Y-%m").to_string());
        let today = Local::now().date_naive();
        let selected = self.view.text_input(cx, ids!(due_date)).text();
        let selected = NaiveDate::parse_from_str(&selected, "%Y-%m-%d").ok();
        // With no deadline, today is the initial choice, without changing the task.
        let highlighted = selected.unwrap_or(today);
        for (index, date) in calendar_days(month).into_iter().enumerate() {
            let mut button = calendar.button(cx, &day_id(index));
            button.set_text(cx, &date.map(|d| d.day().to_string()).unwrap_or_default());
            button.set_board_enabled(cx, date.is_some());
            if date == Some(highlighted) {
                script_apply_eval!(cx, button, {draw_bg +: {color:mod.widgets.RINX_ACCENT color_hover:mod.widgets.RINX_ACCENT_HOVER color_focus:mod.widgets.RINX_ACCENT} draw_text +: {color:mod.widgets.RINX_ON_ACCENT color_hover:mod.widgets.RINX_ON_ACCENT color_focus:mod.widgets.RINX_ON_ACCENT}});
            } else if date == Some(today) {
                script_apply_eval!(cx, button, {draw_bg +: {color:mod.widgets.RINX_SURFACE color_hover:mod.widgets.RINX_HOVER color_focus:mod.widgets.RINX_HOVER} draw_text +: {color:mod.widgets.RINX_ACCENT color_hover:mod.widgets.RINX_ACCENT color_focus:mod.widgets.RINX_ACCENT}});
            } else {
                script_apply_eval!(cx, button, {draw_bg +: {color:#0000 color_hover:mod.widgets.RINX_HOVER color_focus:mod.widgets.RINX_HOVER} draw_text +: {color:mod.widgets.RINX_INK color_hover:mod.widgets.RINX_INK color_focus:mod.widgets.RINX_INK}});
            }
        }
        self.view.redraw(cx);
    }

    fn choose_date(&mut self, cx: &mut Cx, date: Option<NaiveDate>) {
        let input = self.view.text_input(cx, ids!(due_date));
        let value = date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        if input.text() != value {
            input.set_text(cx, &value);
            self.dirty = true;
            self.discard_armed = false;
        }
        self.close_calendar(cx);
    }

    pub(super) fn handle_calendar(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.view.button(cx, ids!(open_calendar)).clicked(actions) {
            if self.calendar_month.is_some() {
                self.close_calendar(cx);
            } else {
                self.open_calendar(cx);
            }
        }
        let Some(month) = self.calendar_month else {
            return;
        };
        if self.pending.is_some()
            || self.conflict
            || self
                .selected
                .as_ref()
                .is_some_and(|t| t.archived_at.is_some())
        {
            self.close_calendar(cx);
            return;
        }
        if self.view.button(cx, ids!(calendar_close)).clicked(actions) {
            self.close_calendar(cx);
        } else if self.view.button(cx, ids!(calendar_today)).clicked(actions) {
            self.choose_date(cx, Some(Local::now().date_naive()));
        } else if self.view.button(cx, ids!(calendar_clear)).clicked(actions) {
            self.choose_date(cx, None);
        } else if self.view.button(cx, ids!(calendar_prev)).clicked(actions) {
            self.calendar_month = month.checked_sub_months(Months::new(1));
            self.sync_calendar(cx);
        } else if self.view.button(cx, ids!(calendar_next)).clicked(actions) {
            self.calendar_month = month.checked_add_months(Months::new(1));
            self.sync_calendar(cx);
        } else {
            for (index, date) in calendar_days(month).into_iter().enumerate() {
                if date.is_some() && self.view.button(cx, &day_id(index)).clicked(actions) {
                    self.choose_date(cx, date);
                    break;
                }
            }
            if self
                .view
                .text_input(cx, ids!(due_date))
                .changed(actions)
                .is_some()
            {
                self.sync_calendar(cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calendar_grid_handles_leap_year_six_weeks_and_year_boundary() {
        let leap = calendar_days(NaiveDate::from_ymd_opt(2024, 2, 1).unwrap());
        assert_eq!(leap.iter().flatten().count(), 29);
        assert_eq!(leap[3].unwrap().day(), 1);
        assert_eq!(leap[31].unwrap().day(), 29);
        let six = calendar_days(NaiveDate::from_ymd_opt(2026, 3, 1).unwrap());
        assert_eq!(six[6].unwrap().day(), 1);
        assert_eq!(six[36].unwrap().day(), 31);
        let jan = month_start(NaiveDate::from_ymd_opt(2026, 1, 31).unwrap());
        assert_eq!(
            jan.checked_sub_months(Months::new(1)).unwrap().to_string(),
            "2025-12-01"
        );
    }
    #[test]
    fn opening_calendar_preserves_optional_deadline_and_selecting_clear_marks_draft() {
        let (mut cx, mut board) = super::super::tests::panel();
        board.creating = true;
        board.open_calendar(&mut cx);
        assert_eq!(
            board.calendar_month,
            Some(month_start(Local::now().date_naive()))
        );
        assert_eq!(board.fields(&mut cx).due_date, None);
        assert!(!board.dirty);
        let date = NaiveDate::from_ymd_opt(2028, 2, 29).unwrap();
        board.choose_date(&mut cx, Some(date));
        assert_eq!(
            board.fields(&mut cx).due_date.as_deref(),
            Some("2028-02-29")
        );
        assert!(board.dirty && board.calendar_month.is_none());
        board.open_calendar(&mut cx);
        assert_eq!(board.calendar_month, Some(month_start(date)));
        board.choose_date(&mut cx, None);
        assert_eq!(board.fields(&mut cx).due_date, None);
    }
}
