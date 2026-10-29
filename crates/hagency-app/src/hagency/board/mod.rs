//! A Project-scoped shared board. Every async reply is pinned to account, Project
//! and view generation; writes retain their original intent until acknowledged.
use makepad_widgets::*;
use crate::shared::unread_badge::UnreadBadgeWidgetExt;
use ::hagency::native_owner::Command;
use ::hagency::project_board::*;
use super::backend::Service;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

mod connection;
use connection::BoardConnection;
mod view;
mod controller;
mod interaction;
mod drag;
mod markdown;
mod calendar;
mod quick_create;
use quick_create::QuickCreateDraft;
mod dialog;
pub(crate) use dialog::route_task_dialog_input;
mod collaboration;
mod media;
use media::MediaState;
mod extensions;
mod lists;
mod labels;
mod filters;
use extensions::column_title;
pub use view::script_mod;

#[derive(Clone, Debug, Default)]
enum CardAction {
    Open,
    Start(Vec2d),
    Drag(Vec2d),
    Drop(Vec2d),
    #[default]
    None,
}
#[derive(Script, ScriptHook, Widget)]
pub struct KanbanCard {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    dragging: bool,
    #[rust]
    focused: bool,
    #[rust]
    painted_focus: Option<bool>,
    #[rust]
    paint_key: String,
}
impl Widget for KanbanCard {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        // Mouse users can grab the whole card. Touch keeps ordinary list scrolling;
        // the explicit handle remains the touch drag target.
        let handle = self.view.view(cx, ids!(drag_handle)).area();
        let area = if matches!(event, Event::TouchUpdate(_)) {
            handle
        } else {
            self.view.area()
        };
        match event.hits(cx, area) {
            Hit::FingerDown(e) => {
                self.dragging = true;
                cx.set_key_focus(self.view.area());
                cx.widget_action(uid, CardAction::Start(e.abs));
            }
            Hit::FingerMove(e) if self.dragging => cx.widget_action(uid, CardAction::Drag(e.abs)),
            Hit::FingerUp(e) if self.dragging => {
                self.dragging = false;
                cx.widget_action(uid, CardAction::Drop(e.abs));
                if e.was_tap() {
                    cx.widget_action(uid, CardAction::Open);
                }
            }
            Hit::KeyDown(key)
                if !key.is_repeat
                    && matches!(key.key_code, KeyCode::ReturnKey | KeyCode::Space) =>
            {
                cx.widget_action(uid, CardAction::Open)
            }
            Hit::KeyFocus(_) => {
                self.focused = true;
                self.view.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                self.focused = false;
                self.view.redraw(cx);
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                script_apply_eval!(cx, self.view, {draw_bg.hover: 1.0});
                self.view.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                script_apply_eval!(cx, self.view, {draw_bg.hover: 0.0});
                self.view.redraw(cx);
            }
            _ => (),
        }
        self.view.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.painted_focus != Some(self.focused) {
            self.painted_focus = Some(self.focused);
            let border_size = if self.focused { 2.0 } else { 1.0 };
            script_apply_eval!(cx,self.view,{draw_bg.border_size:#(border_size)});
            if self.focused {
                script_apply_eval!(cx,self.view,{draw_bg.border_color:mod.widgets.RINX_ACCENT});
            }
        }
        let result = self.view.draw_walk(cx, scope, walk);
        if result.is_done() {
            cx.add_nav_stop(self.view.area(), NavRole::TextInput, Inset::default());
        }
        result
    }
}
#[derive(Clone, Debug)]
enum Read {
    Notifications(i64),
    NotificationAck,
    OpenTask(String),
    Attachment(String, bool),
    InlineImage(String, String, u64),
    Snapshot,
    Changes,
    Page(usize),
    Detail(String),
    Comments(String, bool),
}
#[derive(Debug)]
struct Completed {
    identity: String,
    project: String,
    epoch: u64,
    request: u64,
    kind: Read,
    detail_generation: u64,
    result: Result<Value, String>,
}
#[derive(Debug)]
struct Saved {
    identity: String,
    project: String,
    epoch: u64,
    request: u64,
    result: Result<Value, String>,
}
#[derive(Clone)]
struct Drag {
    task: BoardTask,
    start: Vec2d,
    at: Vec2d,
    moved: bool,
    source: Rect,
}
impl Drag {
    fn position(&self) -> Vec2d {
        self.source.pos + self.at - self.start
    }
}
#[derive(Clone)]
struct Landing {
    drag: Drag,
    target: Vec2d,
    started: Option<f64>,
    progress: f64,
    awaiting: bool,
    layout_ready: bool,
}
impl Landing {
    fn position(&self) -> Vec2d {
        let t = 1.0 - (1.0 - self.progress).powi(3);
        self.drag.position() * (1.0 - t) + self.target * t
    }
}
#[derive(Script, ScriptHook, Widget)]
pub struct ProjectBoard {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[live]
    draw_dialog_bg: DrawColor,
    #[rust]
    dialog_draw_list: Option<DrawList2d>,
    #[rust]
    dialog_cancel_scope: Option<CancelScope>,
    #[rust]
    dialog_focus_pending: bool,
    #[rust]
    dialog_return_focus: Area,
    #[rust]
    connection_override: Option<BoardConnection>,
    #[rust]
    identity: String,
    #[rust]
    parent_project: String,
    #[rust]
    read_only: bool,
    #[rust]
    project: String,
    #[rust]
    active: bool,
    #[rust]
    epoch: u64,
    #[rust]
    snapshot: Option<Snapshot>,
    #[rust]
    query: Query,
    #[rust]
    members: Vec<String>,
    #[rust]
    member_names: HashMap<String, String>,
    #[rust]
    reads: HashSet<u64>,
    #[rust]
    loading: bool,
    #[rust]
    reload_pending: bool,
    #[rust]
    detail_request: u64,
    #[rust]
    detail_loaded: bool,
    #[rust]
    pending: Option<BoardAction>,
    #[rust]
    saving: bool,
    #[rust]
    save_request: u64,
    #[rust]
    retained: HashMap<String, BoardAction>,
    #[rust]
    timer: Timer,
    #[rust]
    selected: Option<BoardTask>,
    #[rust]
    creating: bool,
    #[rust]
    calendar_month: Option<chrono::NaiveDate>,
    #[rust]
    quick_create_state: Option<String>,
    #[rust]
    quick_drafts: HashMap<String, QuickCreateDraft>,
    #[rust]
    quick_restore: Option<QuickCreateDraft>,
    #[rust]
    quick_save_key: Option<String>,
    #[rust]
    quick_focus: bool,
    #[rust]
    dirty: bool,
    #[rust]
    subtask_after_save: Option<(String, String)>,
    #[rust]
    conflict: bool,
    #[rust]
    comments: Vec<Value>,
    #[rust]
    comments_after: i64,
    #[rust]
    comments_request: u64,
    #[rust]
    card_areas: HashMap<String, Area>,
    #[rust]
    card_targets: HashMap<WidgetUid, BoardTask>,
    #[rust]
    more_targets: HashMap<WidgetUid, usize>,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    drag_frame: NextFrame,
    #[rust]
    landing: Option<Landing>,
    #[rust]
    scroll_x: f64,
    #[rust(true)]
    foreground: bool,
    #[rust]
    retry_delay: f64,
    #[rust]
    offline: bool,
    #[rust]
    failed: bool,
    #[rust]
    discard_armed: bool,
    #[rust]
    columns_draft: Option<Vec<ColumnDefinition>>,
    #[rust]
    columns_sequence: i64,
    #[rust]
    column_rows: HashMap<usize, WidgetRef>,
    #[rust]
    column_paint_keys: HashMap<WidgetUid, String>,
    #[rust]
    label_options: Vec<String>,
    #[rust]
    created_card: Option<BoardTask>,
    #[rust]
    created_filters_cleared: bool,
    #[rust]
    created_reveal_pending: bool,
    #[rust]
    column_edit: usize,
    #[rust]
    column_drag: Option<(usize, Vec2d)>,
    #[rust]
    draft_checklist: Vec<ChecklistItem>,
    #[rust]
    description_preview: bool,
    #[rust]
    comment_preview: bool,
    #[rust]
    comment_drafts: HashMap<(String, String), String>,
    #[rust]
    notifications: Vec<Value>,
    #[rust]
    discussion: bool,
    #[rust]
    notification_before: i64,
    #[rust]
    notification_latest: i64,
    #[rust]
    open_request: u64,
    #[rust]
    attachment_request: u64,
    #[rust]
    notification_request: u64,
    #[rust]
    notification_targets: HashMap<WidgetUid, Value>,
    #[rust]
    attachments: Vec<Value>,
    #[rust]
    attachment_targets: HashMap<WidgetUid, (String, bool)>,
    #[rust]
    media: MediaState,
    #[rust]
    layout_key: String,
    #[rust]
    drag_preview_active: bool,
    #[rust]
    draft_parent: Option<String>,
    #[rust]
    draft_state: String,
    #[rust]
    parent_task: Option<BoardTask>,
    #[rust]
    subtasks: Vec<BoardTask>,
    #[rust]
    checklist_rows: HashMap<usize, WidgetRef>,
    #[rust]
    subtask_rows: HashMap<usize, WidgetRef>,
}
fn nonce() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
fn tr(key: &str) -> &str {
    crate::i18n::tr(key)
}
fn priority_label(priority: &str) -> &str {
    tr(match priority {
        "low" => "Low",
        "normal" => "Normal",
        "high" => "High",
        "urgent" => "Urgent",
        _ => "Normal",
    })
}
fn member_label(mxid: &str) -> &str {
    mxid.strip_prefix('@')
        .and_then(|m| m.split_once(':').map(|(name, _)| name))
        .unwrap_or(mxid)
}
fn timestamp(value: &Value) -> String {
    value
        .as_i64()
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M %:z")
                .to_string()
        })
        .unwrap_or_default()
}
fn error_message(code: &str) -> &str {
    tr(match code {
        "project_archived" => "This Project is archived. Restore it before making changes.",
        "board_unsupported" => "This server does not support project boards yet.",
        "board_version_conflict" | "board_snapshot_changed" | "board_neighbours_changed" => {
            "The board changed on another device. Refreshed the board; your draft is preserved."
        }
        "project_membership_required"
        | "authorization_expired"
        | "owner_authorization_required"
        | "sign_in_required"
        | "matrix_account_changed"
        | "device_authorization_required"
        | "device_authorization_expired"
        | "invalid_device_authorization" => {
            "Project access has expired. Sign in again or check your membership."
        }
        "invalid_board_columns" => {
            "Keep 2–12 uniquely named columns, at least two active columns and one completed column. Task limits must be between 1 and 10000."
        }
        "board_wip_limit_reached" => {
            "This column is full. Move or archive a task, or increase its task limit."
        }
        "board_column_archived" => "Restore this column before editing its cards.",
        "board_column_not_empty" => {
            "Move all tasks, including archived tasks, before deleting this column."
        }
        "invalid_board_parent" => {
            "Choose an active parent in this Project. Subtasks support one level and up to 100 children."
        }
        "invalid_board_checklist" => {
            "Use up to 100 checklist items, with 1–160 characters per item."
        }
        "invalid_board_labels" => "Use up to 12 labels, each with 1–32 characters and no control characters.",
        "invalid_board_fields"
        | "invalid_board_date"
        | "invalid_board_payload" => "Check the title, labels and date before saving.",
        "board_assignee_not_member" => {
            "The assignee is no longer a Project member. Choose another member."
        }
        "invalid_board_attachment" => "Choose a non-empty file smaller than 2 MB.",
        "board_attachment_limit" => "This task has reached its attachment limit.",
        "board_attachment_not_found" => "This attachment is no longer available.",
        "board_human_assignee_required" => {
            "Choose a person. Agent execution will be available separately."
        }
        _ => "The server could not confirm this request. Check your connection and retry.",
    })
}
fn denied(code: &str) -> bool {
    matches!(
        code,
        "project_membership_required"
            | "authorization_expired"
            | "owner_authorization_required"
            | "sign_in_required"
            | "matrix_account_changed"
            | "matrix_authorization_required"
    )
}
fn uncertain(code: &str) -> bool {
    !matches!(
        code,
        "invalid_board_attachment"
            | "board_attachment_limit"
            | "board_attachment_not_found"
            | "invalid_board_columns"
            | "board_wip_limit_reached"
            | "board_column_archived"
            | "board_column_not_empty"
            | "invalid_board_parent"
            | "invalid_board_checklist"
            | "board_unsupported"
            | "invalid_board_fields"
            | "invalid_board_date"
            | "invalid_board_labels"
            | "invalid_board_payload"
            | "invalid_board_comment"
            | "invalid_board_neighbours"
            | "invalid_board_version"
            | "invalid_board_state"
            | "invalid_identifier"
            | "board_assignee_not_member"
            | "board_human_assignee_required"
            | "board_version_conflict"
            | "board_snapshot_changed"
            | "board_neighbours_changed"
            | "board_idempotency_conflict"
            | "board_archive_conflict"
            | "board_capacity_exceeded"
            | "board_task_not_found"
    )
}
fn column_path(index: usize) -> &'static [LiveId] {
    match index {
        0 => ids!(todo),
        1 => ids!(in_progress),
        2 => ids!(blocked),
        3 => ids!(in_review),
        4 => ids!(done),
        5 => ids!(custom_0),
        6 => ids!(custom_1),
        7 => ids!(custom_2),
        8 => ids!(custom_3),
        9 => ids!(custom_4),
        10 => ids!(custom_5),
        _ => ids!(custom_6),
    }
}
impl ProjectBoardRef {
    pub fn controls(&self,cx:&Cx)->WidgetRef {self.borrow().map(|b|b.view.widget(cx,ids!(toolbar))).unwrap_or_default()}
    pub fn is_loading(&self)->bool {self.borrow().is_some_and(|b|b.active && (b.loading || b.saving))}
    pub fn read_only_for(&self,project:&str)->bool {self.borrow().is_some_and(|b|b.parent_project==project && b.read_only)}
    pub fn bind_scope(&self,cx:&mut Cx,identity:&str,project:Option<&str>,board:Option<&str>,active:bool) {
        if active {dialog::bind_dialog_widget(cx,&self.0);}
        if let Some(mut inner)=self.borrow_mut() {
            inner.parent_project=project.unwrap_or_default().into();
            inner.bind(cx,identity,board.or(project),active);
        }
    }
    pub fn can_switch(&self,cx:&Cx)->bool {
        self.borrow().is_none_or(|b| !b.dirty && b.pending.is_none() && !b.modal_visible(cx))
    }
    pub fn bind(&self, cx: &mut Cx, identity: &str, project: Option<&str>, active: bool) {
        if active {
            dialog::bind_dialog_widget(cx, &self.0);
        }
        if let Some(mut inner) = self.borrow_mut() {
            inner.bind(cx, identity, project, active);
        }
    }
}
#[cfg(test)]
mod tests;

/// Development-only read-only rendering. It cannot authenticate or submit work.
#[cfg(debug_assertions)]
impl ProjectBoardRef {
    pub fn show_design_preview(&self, cx: &mut Cx, snapshot: Snapshot, selected: Option<&str>) {
        if let Some(mut board) = self.borrow_mut() {
            board.active = false;
            board.project = snapshot.project_id.clone();
            let members = snapshot.members.clone();
            let task = selected
                .and_then(|id| {
                    snapshot
                        .lanes
                        .iter()
                        .flat_map(|l| &l.tasks)
                        .find(|t| t.id == id)
                })
                .cloned();
            board.snapshot = Some(snapshot);
            board.sync_members(cx, members);
            if let Some(task) = task {
                board.selected = Some(task.clone());
                board.detail_loaded = true;
                board.fill_editor(cx, task.fields());
                board.open_inspector(cx);
                board
                    .view
                    .label(cx, ids!(detail_heading))
                    .set_text(cx, &format!("#{}", task.number));
                board
                    .view
                    .label(cx, ids!(detail_notice))
                    .set_text(cx, tr("Visible to current Project members."));
                board
                    .view
                    .widget(cx, ids!(comments))
                    .set_text(cx, tr("No comments yet"));
            }
            board.sync_controls(cx);
        }
    }
}

// Makepad ButtonRef setters redraw even when the value is unchanged. Board
// PortalList rows are configured during drawing, so only write real changes.
trait BoardButtonState {
    fn set_board_enabled(&self, cx: &mut Cx, enabled: bool);
    fn set_board_visible(&self, cx: &mut Cx, visible: bool);
}
impl BoardButtonState for ButtonRef {
    fn set_board_enabled(&self, cx: &mut Cx, enabled: bool) {
        let changed = self
            .borrow()
            .is_some_and(|button| button.enabled() != enabled);
        if changed {
            self.set_enabled(cx, enabled);
        }
    }
    fn set_board_visible(&self, cx: &mut Cx, visible: bool) {
        if self.visible() != visible {
            self.set_visible(cx, visible);
        }
    }
}
