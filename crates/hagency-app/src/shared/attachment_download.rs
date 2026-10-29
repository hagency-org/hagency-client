//! Download a Matrix media attachment and save it to storage.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use makepad_widgets::SignalToUI;
use matrix_sdk::ruma::{OwnedMxcUri, events::room::MediaSource};
use robius_share::ShareSheet;
use crate::{
    home::room_screen::TimelineUpdate,
    shared::popup_list::{
        PopupKind, enqueue_popup_notification, enqueue_scoped_popup_notification,
    },
    sliding_sync::{MatrixRequest, submit_async_request},
    temp_storage::{ShareScope, ShareTicket},
};

pub type TimelineUpdateSenderOption = Option<crate::timeline_updates::Sender>;

/// The result of a download request.
pub enum MediaDownloadResult {
    Downloaded(crate::attachment_transport::DownloadedFile),
    Failed(String),
    Cancelled,
}

pub fn enqueue_already_downloading_notification() {
    const ALREADY_DOWNLOADING_MESSAGE: &str = "This media is already being downloaded.";
    enqueue_popup_notification(ALREADY_DOWNLOADING_MESSAGE, PopupKind::Warning, Some(4.0));
}

/// The mxc URI inside any media source, whether plain or encrypted.
pub fn media_source_mxc(source: &MediaSource) -> &OwnedMxcUri {
    match source {
        MediaSource::Plain(uri) => uri,
        MediaSource::Encrypted(file) => &file.url,
    }
}

/// Whether a pending transfer is a plain download or a share.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TransferKind {
    Download,
    Share,
    Preview,
}

/// Info about a download or share that has begun or recently completed.
pub struct PendingDownload {
    pub mxc: OwnedMxcUri,
    pub state: PendingDownloadState,
    pub kind: TransferKind,
}

pub enum PendingDownloadState {
    /// The request has been submitted to and is being handled by the backend worker task.
    InProgress,
    /// It succeeded, and will show a success indicator for a few seconds.
    JustSucceeded,
    /// It failed, and will show an error indicator for a few seconds.
    JustFailed,
}
impl PendingDownloadState {
    pub fn display(&self, kind: TransferKind) -> DownloadDisplayState {
        match self {
            Self::InProgress => DownloadDisplayState::InProgress,
            Self::JustSucceeded => DownloadDisplayState::Succeeded(kind),
            Self::JustFailed => DownloadDisplayState::Failed,
        }
    }
}

/// What the download section below a message should show.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum DownloadDisplayState {
    /// Default: show the download and share buttons.
    #[default]
    Idle,
    /// Show a loading spinner and cancel button.
    InProgress,
    /// Briefly show a green success button ("Downloaded" or "Shared").
    Succeeded(TransferKind),
    /// Briefly show a red failed button.
    Failed,
}

/// How long (in seconds) the success/failure state stays visible before resetting the button.
pub const DOWNLOAD_RESULT_DURATION_SECS: f64 = 5.0;

/// Metadata describing an attachment/media file to be downloaded.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DownloadableAttachment {
    pub media_source: MediaSource,
    pub filename: String,
    pub size: Option<u64>,
    pub kind: DownloadKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DownloadKind {
    File,
    Markdown,
    Theme,
    Audio,
    Video,
    Image,
}
impl DownloadKind {
    /// Returns the top-level MIME type for this kind of download.
    fn basic_mime_type(&self) -> Option<&'static str> {
        match self {
            Self::Image => Some("image/*"),
            Self::Audio => Some("audio/*"),
            Self::Video => Some("video/*"),
            Self::File => None,
            Self::Markdown => Some("text/markdown"),
            Self::Theme => Some(octosense_theme_contract::MIME),
        }
    }
}

pub fn is_theme_attachment(filename: &str, mimetype: Option<&str>) -> bool {
    filename.to_ascii_lowercase().ends_with(".octotheme")
        || mimetype.is_some_and(|m| {
            m.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case(octosense_theme_contract::MIME)
        })
}
pub fn start_theme_preview(
    info: DownloadableAttachment,
    update_sender: TimelineUpdateSenderOption,
) {
    let owner = crate::sliding_sync::current_user_id();
    let epoch = crate::account_session::epoch();
    if info
        .size
        .is_some_and(|s| s > octosense_theme_contract::MAX_BYTES as u64)
    {
        enqueue_popup_notification("Theme exceeds 256 KB", PopupKind::Error, None);
        finish_download_indicator(
            &update_sender,
            Some(media_source_mxc(&info.media_source)),
            DownloadOutcome::Failed,
        );
        return;
    }
    download_preview(
        info,
        update_sender,
        crate::attachment_transport::THEME_LIMIT,
        move |_title, mxc, bytes, sender| {
            if epoch != crate::account_session::epoch()
                || owner != crate::sliding_sync::current_user_id()
                || crate::logout::logout_state_machine::is_logout_in_progress()
            {
                finish_download_indicator(&sender, Some(&mxc), DownloadOutcome::Cancelled);
                return;
            }
            let result = octosense_theme_contract::ThemePackage::parse(&bytes).map(Box::new);
            let outcome = if result.is_ok() {
                DownloadOutcome::Succeeded
            } else {
                DownloadOutcome::Failed
            };
            makepad_widgets::Cx::post_action(
                crate::settings::theme_studio::ThemeStudioAction::Imported {
                    owner,
                    epoch,
                    result,
                },
            );
            finish_download_indicator(&sender, Some(&mxc), outcome);
        },
    );
}

pub fn is_markdown_attachment(filename: &str, mimetype: Option<&str>) -> bool {
    let extension = filename.rsplit('.').next().unwrap_or_default();
    (filename.contains('.')
        && (extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")))
        || mimetype.is_some_and(|mime| {
            matches!(
                mime.split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
                    .as_str(),
                "text/markdown" | "text/x-markdown"
            )
        })
}

pub(crate) fn markdown_source(bytes: Vec<u8>) -> Result<Arc<str>, String> {
    if bytes.len() > article_core::document::MAX_BODY {
        return Err("Article exceeds its size limits.".into());
    }
    let text =
        String::from_utf8(bytes).map_err(|_| "Markdown must contain UTF-8 text".to_owned())?;
    Ok(text.trim_start_matches('\u{feff}').into())
}

/// Uses Matrix's authenticated media API, including encrypted attachments.
pub fn start_markdown_preview(
    info: DownloadableAttachment,
    update_sender: TimelineUpdateSenderOption,
) {
    let account = crate::sliding_sync::current_user_id();
    let epoch = crate::account_session::epoch();
    let attachment = Some(info.clone());
    download_preview(
        info,
        update_sender,
        crate::attachment_transport::MARKDOWN_LIMIT,
        move |title, mxc, bytes, sender| {
            if epoch != crate::account_session::epoch()
                || account != crate::sliding_sync::current_user_id()
                || crate::logout::logout_state_machine::is_logout_in_progress()
            {
                finish_download_indicator(&sender, Some(&mxc), DownloadOutcome::Cancelled);
                return;
            }
            match markdown_source(bytes) {
                Ok(source) => {
                    makepad_widgets::Cx::post_action(
                        crate::shared::web_browser::WebBrowserAction::OpenMarkdown {
                            epoch,
                            title,
                            source,
                            account,
                            attachment,
                        },
                    );
                    finish_download_indicator(&sender, Some(&mxc), DownloadOutcome::Succeeded);
                }
                Err(error) => {
                    enqueue_scoped_popup_notification(epoch, error, PopupKind::Error, None);
                    finish_download_indicator(&sender, Some(&mxc), DownloadOutcome::Failed);
                }
            }
        },
    );
}

/// Fetches an authenticated attachment into a verified private file.
fn download_file(
    info: DownloadableAttachment,
    update_sender: TimelineUpdateSenderOption,
    max_bytes: u64,
    on_downloaded: impl FnOnce(
        String,
        OwnedMxcUri,
        crate::attachment_transport::DownloadedFile,
        TimelineUpdateSenderOption,
        u64,
    ) + Send
    + 'static,
) {
    let epoch = crate::account_session::epoch();
    let mxc = media_source_mxc(&info.media_source).clone();
    let filename = info.filename.clone();
    submit_async_request(MatrixRequest::DownloadMedia {
        media_source: info.media_source,
        filename: info.filename,
        max_bytes,
        on_download_result: Box::new(move |result| match result {
            MediaDownloadResult::Downloaded(bytes) => {
                on_downloaded(filename, mxc, bytes, update_sender, epoch)
            }
            MediaDownloadResult::Failed(e) => {
                enqueue_scoped_popup_notification(
                    epoch,
                    format!("Failed to download \"{filename}\": {e}"),
                    PopupKind::Error,
                    None,
                );
                finish_download_indicator(&update_sender, Some(&mxc), DownloadOutcome::Failed);
            }
            MediaDownloadResult::Cancelled => {
                finish_download_indicator(&update_sender, Some(&mxc), DownloadOutcome::Cancelled)
            }
        }),
    });
}

/// Preview conversion happens only after the actual byte-limited transfer and
/// on a finite owned filesystem worker, never on the UI or Tokio runtime.
fn download_preview(
    info: DownloadableAttachment,
    sender: TimelineUpdateSenderOption,
    limit: u64,
    ready: impl FnOnce(String, OwnedMxcUri, Vec<u8>, TimelineUpdateSenderOption) + Send + 'static,
) {
    download_file(
        info,
        sender,
        limit,
        move |title, mxc, file, sender, epoch| {
            let failed_mxc = mxc.clone();
            let failed_sender = sender.clone();
            if let Err(error) = crate::attachment_transport::io_worker::submit(move || {
                match file.read_small(limit) {
                    Ok(bytes) => ready(title, mxc, bytes, sender),
                    Err(error) => {
                        enqueue_scoped_popup_notification(epoch, error, PopupKind::Error, None);
                        finish_download_indicator(&sender, Some(&mxc), DownloadOutcome::Failed);
                    }
                }
            }) {
                enqueue_scoped_popup_notification(epoch, error, PopupKind::Error, None);
                finish_download_indicator(
                    &failed_sender,
                    Some(&failed_mxc),
                    DownloadOutcome::Failed,
                );
            }
        },
    );
}

/// Downloads the attachment and shows the native save dialog for it.
pub fn start_attachment_download(
    info: DownloadableAttachment,
    update_sender: TimelineUpdateSenderOption,
) {
    download_file(
        info,
        update_sender,
        crate::attachment_transport::MAX_FILE_BYTES,
        |filename, mxc, file, sender, epoch| {
            show_file_save_dialog(filename, file, Some(mxc), sender, epoch);
        },
    );
}

/// Saves an attachment already in memory directly to storage, without showing any dialog.
pub fn save_loaded_attachment(filename: String, bytes: Arc<[u8]>) {
    show_save_dialog(filename, bytes, None, None);
}

/// Download and prepare off-thread; actual native presentation uses a fresh
/// account fence on the UI thread. The API provides no share-completion verdict.
pub fn start_attachment_share(
    info: DownloadableAttachment,
    update_sender: TimelineUpdateSenderOption,
) {
    let Some(scope) = ShareScope::capture() else {
        finish_download_indicator(
            &update_sender,
            Some(media_source_mxc(&info.media_source)),
            DownloadOutcome::Cancelled,
        );
        return;
    };
    let mime = info.kind.basic_mime_type();
    download_file(
        info,
        update_sender,
        crate::attachment_transport::MAX_FILE_BYTES,
        move |filename, mxc, file, sender, _epoch| {
            prepare_attachment_share(scope, filename, mxc, ShareInput::File(file), mime, sender);
        },
    );
}

pub fn share_loaded_attachment(info: &DownloadableAttachment, bytes: Arc<[u8]>) {
    let Some(scope) = ShareScope::capture() else {
        return;
    };
    prepare_attachment_share(
        scope,
        info.filename.clone(),
        media_source_mxc(&info.media_source).clone(),
        ShareInput::Bytes(bytes),
        info.kind.basic_mime_type(),
        None,
    );
}

type PreparedSharePayload = Result<(Arc<ShareTicket>, robius_share::PreparedShare), String>;

struct PreparedAttachmentShare {
    scope: ShareScope,
    mxc: OwnedMxcUri,
    sender: TimelineUpdateSenderOption,
    payload: Mutex<Option<PreparedSharePayload>>,
}
impl std::fmt::Debug for PreparedAttachmentShare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreparedAttachmentShare { .. }")
    }
}

/// Remove under the lock, then return ownership so callback code may re-enter
/// download/cancel/share without retaining the global transfer-map mutex.
pub(crate) fn take_download<T>(
    tasks: &Mutex<hashbrown::HashMap<OwnedMxcUri, T>>,
    mxc: &OwnedMxcUri,
) -> Option<T> {
    tasks.lock().unwrap().remove(mxc)
}

enum ShareInput {
    Bytes(Arc<[u8]>),
    File(crate::attachment_transport::DownloadedFile),
}
fn prepare_attachment_share(
    scope: ShareScope,
    filename: String,
    mxc: OwnedMxcUri,
    source: ShareInput,
    mime: Option<&'static str>,
    sender: TimelineUpdateSenderOption,
) {
    let callback_scope = scope.clone();
    let callback_mxc = mxc.clone();
    let callback_sender = sender.clone();
    let title = format!("Share {filename}");
    let ready = Box::new(move |result: Result<Arc<ShareTicket>, String>| {
        // Runs on the dedicated tracked disk worker. Windows StorageFile and
        // macOS path preparation must finish here, never under SESSION_IO/UI.
        let payload = result.and_then(|ticket| {
            let sheet = ShareSheet::new().set_title(title);
            let sheet = match mime {
                Some(mime) => sheet.add_file_with_mime_type(&ticket.path, mime),
                None => sheet.add_file(&ticket.path),
            };
            match sheet.prepare() {
                Ok(prepared) => Ok((ticket, prepared)),
                Err(error) => {
                    ticket.discard();
                    Err(format!("Could not prepare share sheet: {error}"))
                }
            }
        });
        makepad_widgets::Cx::post_action(PreparedAttachmentShare {
            scope: callback_scope,
            mxc: callback_mxc,
            sender: callback_sender,
            payload: Mutex::new(Some(payload)),
        });
    });
    let staged = match source {
        ShareInput::Bytes(bytes) => {
            crate::temp_storage::stage_share(scope.clone(), filename, bytes, ready)
        }
        ShareInput::File(file) => {
            crate::temp_storage::stage_share_file(scope.clone(), filename, file, ready)
        }
    };
    if let Err(error) = staged {
        if scope.is_current() {
            enqueue_popup_notification(error, PopupKind::Error, None);
        }
        finish_download_indicator(&sender, Some(&mxc), DownloadOutcome::Cancelled);
    }
}

fn fenced_presentation_guard(
    gate: &'static Mutex<()>,
    authorize: impl Fn() -> bool + Send + Sync + 'static,
) -> robius_share::PresentationGuard {
    Arc::new(move |present| {
        // Reentrant native payload callbacks fail closed instead of deadlocking
        // the UI. Only actual presentation/payload commits hold this short fence.
        let _session = gate.try_lock().map_err(|_| robius_share::Error::Retired)?;
        if !authorize() {
            return Err(robius_share::Error::Retired);
        }
        present()
    })
}

/// Global UI action hook; preparing is insufficient authority to present.
pub(crate) fn handle_share_actions(actions: &makepad_widgets::Actions) {
    for action in actions
        .iter()
        .filter_map(|action| action.downcast_ref::<PreparedAttachmentShare>())
    {
        let Some(payload) = action.payload.lock().unwrap().take() else {
            continue;
        };
        let result = match payload {
            Ok((ticket, prepared)) => {
                if !ticket.begin_presentation() {
                    ticket.discard();
                    if action.scope.is_current() {
                        enqueue_popup_notification(
                            "Private share staging is busy or exceeds its storage limit",
                            PopupKind::Error,
                            None,
                        );
                    }
                    finish_download_indicator(
                        &action.sender,
                        Some(&action.mxc),
                        DownloadOutcome::Cancelled,
                    );
                    continue;
                }
                let authority = ticket.clone();
                let guard = fenced_presentation_guard(&crate::persistence::SESSION_IO, move || {
                    authority.is_current()
                });
                match prepared.present_guarded(guard) {
                    Ok(pending) => {
                        ticket.observe(pending);
                        Ok(())
                    }
                    Err(error) => {
                        ticket.presentation_failed();
                        if !matches!(error, robius_share::Error::OutcomeUnknown) {
                            ticket.discard();
                        }
                        Err(format!("Could not open share sheet: {error}"))
                    }
                }
            }
            Err(error) => Err(error),
        };
        if action.scope.is_current() {
            if let Err(error) = result {
                enqueue_popup_notification(error, PopupKind::Error, None);
            }
        }
        // Presentation is neither successful delivery nor known cancellation.
        // Reset the spinner without displaying a misleading "Shared" verdict.
        finish_download_indicator(
            &action.sender,
            Some(&action.mxc),
            DownloadOutcome::Cancelled,
        );
    }
}

enum DownloadOutcome {
    Succeeded,
    Failed,
    /// The user dismissed the save dialog; return the indicator to idle.
    Cancelled,
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
static SAVE_DIALOGS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn submit_save_operation(operation: impl FnOnce() + Send + 'static) -> Result<(), String> {
    let permit = SAVE_DIALOGS
        .try_acquire()
        .map_err(|_| "Attachment save dialogs are busy".to_owned())?;
    crate::attachment_transport::io_worker::submit(move || {
        let _original = permit;
        operation();
    })
}

/// The permit belongs to the original native operation, not an abortable async
/// observer. Win/Linux use our owned consumer thread. macOS dispatches only the
/// native selection to main and returns its actual result to that same worker.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn show_file_save_dialog(
    filename: String,
    file: crate::attachment_transport::DownloadedFile,
    mxc: Option<OwnedMxcUri>,
    sender: TimelineUpdateSenderOption,
    epoch: u64,
) {
    let failure_sender = sender.clone();
    let failure_mxc = mxc.clone();
    if let Err(error) = submit_save_operation(move || {
        if !file.is_current() {
            finish_download_indicator(&sender, mxc.as_ref(), DownloadOutcome::Cancelled);
            return;
        }
        let select_file = file.clone();
        let select = move || {
            if !select_file.is_current() {
                return None;
            }
            let mut dialog = rfd::FileDialog::new().set_file_name(&filename);
            if let Some(downloads) = robius_directories::UserDirs::new()
                .and_then(|dirs| dirs.download_dir().map(std::path::Path::to_owned))
            {
                dialog = dialog.set_directory(downloads);
            }
            dialog.save_file()
        };
        #[cfg(target_os = "macos")]
        let selected = {
            let (send, receive) = std::sync::mpsc::sync_channel(1);
            dispatch2::DispatchQueue::main().exec_async(move || {
                let selected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(select))
                    .unwrap_or_default();
                let _ = send.send(selected);
            });
            // This is the original owned worker, never UI/Tokio. Retirement
            // cannot drop this receiver/permit while the OS request is alive.
            receive.recv().unwrap_or_default()
        };
        #[cfg(not(target_os = "macos"))]
        let selected = select();
        // Linux portal transport failure can also yield None (and rfd may
        // try zenity). This proves no target/file saved, not OS cancellation
        // or closure of every external portal window. No plaintext was sent.
        let Some(selected) = selected else {
            finish_download_indicator(&sender, mxc.as_ref(), DownloadOutcome::Cancelled);
            return;
        };
        if !file.is_current() {
            finish_download_indicator(&sender, mxc.as_ref(), DownloadOutcome::Cancelled);
            return;
        }
        match file.save_to(&selected) {
            Ok(()) => {
                enqueue_scoped_popup_notification(
                    epoch,
                    "Downloaded attachment.",
                    PopupKind::Success,
                    Some(DOWNLOAD_RESULT_DURATION_SECS),
                );
                finish_download_indicator(&sender, mxc.as_ref(), DownloadOutcome::Succeeded);
            }
            Err(error) => {
                enqueue_scoped_popup_notification(
                    epoch,
                    format!("Failed to save attachment: {error}"),
                    PopupKind::Error,
                    None,
                );
                finish_download_indicator(&sender, mxc.as_ref(), DownloadOutcome::Failed);
            }
        }
    }) {
        enqueue_scoped_popup_notification(epoch, error, PopupKind::Error, None);
        finish_download_indicator(
            &failure_sender,
            failure_mxc.as_ref(),
            DownloadOutcome::Failed,
        );
    }
}
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn show_file_save_dialog(
    _filename: String,
    _file: crate::attachment_transport::DownloadedFile,
    mxc: Option<OwnedMxcUri>,
    sender: TimelineUpdateSenderOption,
    epoch: u64,
) {
    enqueue_scoped_popup_notification(
        epoch,
        "File-source Save As is not available on this platform",
        PopupKind::Error,
        None,
    );
    finish_download_indicator(&sender, mxc.as_ref(), DownloadOutcome::Failed);
}

/// Shows the native save-file dialog and writes `data` to the user-chosen location.
fn show_save_dialog<D: AsRef<[u8]> + Send + 'static>(
    filename: String,
    data: D,
    mxc: Option<OwnedMxcUri>,
    update_sender: TimelineUpdateSenderOption,
) {
    let epoch = crate::account_session::epoch();
    use robius_file_picker::{PickedFile, FileDialog, StartLocation};
    let filename2 = filename.clone();
    let mxc2 = mxc.clone();
    let sender2 = update_sender.clone();
    let on_done = move |result: robius_file_picker::Result<Option<PickedFile>>| {
        match result {
            Ok(Some(_)) => {
                enqueue_scoped_popup_notification(
                    epoch,
                    format!("Downloaded \"{filename2}\"."),
                    PopupKind::Success,
                    Some(DOWNLOAD_RESULT_DURATION_SECS),
                );
                finish_download_indicator(&update_sender, mxc.as_ref(), DownloadOutcome::Succeeded);
            }
            // User dismissed the save dialog. The bytes are discarded, but the
            // matrix media cache makes a re-download effectively instant, so we
            // just return the indicator to idle without a popup.
            Ok(None) => {
                finish_download_indicator(&update_sender, mxc.as_ref(), DownloadOutcome::Cancelled);
            }
            Err(e) => {
                enqueue_scoped_popup_notification(
                    epoch,
                    format!("Failed to save \"{filename2}\": {e}"),
                    PopupKind::Error,
                    None,
                );
                finish_download_indicator(&update_sender, mxc.as_ref(), DownloadOutcome::Failed);
            }
        }
    };

    let res = FileDialog::new()
        .set_file_name(&filename)
        .set_start_location(StartLocation::Downloads)
        .save_data(data, on_done);
    if let Err(e) = res {
        enqueue_scoped_popup_notification(
            epoch,
            format!("Failed to open save dialog: {e}"),
            PopupKind::Error,
            None,
        );
        finish_download_indicator(&sender2, mxc2.as_ref(), DownloadOutcome::Failed);
    }
}

/// Handles the completion of a download, whether success, failure, or cancelled.
fn finish_download_indicator(
    update_sender: &TimelineUpdateSenderOption,
    mxc: Option<&OwnedMxcUri>,
    outcome: DownloadOutcome,
) {
    let Some(mxc) = mxc else { return };
    let Some(sender) = update_sender.as_ref() else {
        return;
    };
    match outcome {
        DownloadOutcome::Cancelled => {
            let _ = sender.send(TimelineUpdate::AttachmentDownloadReset(mxc.clone()));
            SignalToUI::set_ui_signal();
        }
        DownloadOutcome::Succeeded | DownloadOutcome::Failed => {
            let result = match outcome {
                DownloadOutcome::Succeeded => Ok(()),
                _ => Err(String::new()),
            };
            let _ = sender.send(TimelineUpdate::AttachmentDownloadFinished(
                mxc.clone(),
                result,
            ));
            SignalToUI::set_ui_signal();
            // Clear the success/failure display after a short delay.
            let sender = sender.clone();
            let mxc = mxc.clone();
            crate::sliding_sync::spawn_async_task(async move {
                tokio::time::sleep(Duration::from_secs_f64(DOWNLOAD_RESULT_DURATION_SECS)).await;
                let _ = sender.send(TimelineUpdate::AttachmentDownloadReset(mxc));
                SignalToUI::set_ui_signal();
            });
        }
    }
}

#[cfg(test)]
mod markdown_tests {
    use super::*;

    #[test]
    fn recognizes_markdown_filenames_and_mime_without_matching_other_attachments() {
        for name in ["notes.md", "NOTES.MD", "notes.Markdown"] {
            assert!(is_markdown_attachment(name, None));
        }
        assert!(is_markdown_attachment(
            "notes",
            Some("text/markdown; charset=utf-8")
        ));
        assert!(is_markdown_attachment("notes.txt", Some("text/x-markdown")));
        for name in ["md", "notes.md.exe", "image.png", "notes.txt"] {
            assert!(!is_markdown_attachment(name, None));
        }
    }

    #[test]
    fn accepts_utf8_bom_and_rejects_binary_or_oversize_documents() {
        assert_eq!(
            &*markdown_source("\u{feff}# 文档".as_bytes().to_vec()).unwrap(),
            "# 文档"
        );
        assert!(markdown_source(vec![0xff, 0xfe]).is_err());
        assert!(markdown_source(vec![b'a'; article_core::document::MAX_BODY + 1]).is_err());
    }
}

#[cfg(test)]
mod download_callback_tests {
    use super::*;
    #[test]
    fn actual_presentation_and_delayed_payload_recheck_authority_without_reentrant_deadlock() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        static GATE: Mutex<()> = Mutex::new(());
        let live = Arc::new(AtomicBool::new(true));
        let checked = live.clone();
        let guard = fenced_presentation_guard(&GATE, move || checked.load(Ordering::Acquire));
        let committed = AtomicUsize::new(0);
        guard(&mut || {
            committed.fetch_add(1, Ordering::AcqRel);
            // Windows can deliver callbacks while native presentation runs.
            assert!(matches!(
                guard(&mut || panic!("nested payload must not run")),
                Err(robius_share::Error::Retired)
            ));
            Ok(())
        })
        .unwrap();
        live.store(false, Ordering::Release);
        assert!(matches!(
            guard(&mut || {
                committed.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }),
            Err(robius_share::Error::Retired)
        ));
        assert_eq!(committed.load(Ordering::Acquire), 1);
    }

    #[test]
    fn completion_and_cancellation_callbacks_can_reenter_the_download_map() {
        for _cancelled in [false, true] {
            type Callback = Box<dyn FnOnce() + Send>;
            let map = Arc::new(Mutex::new(
                hashbrown::HashMap::<OwnedMxcUri, Callback>::new(),
            ));
            let uri: OwnedMxcUri = "mxc://example.org/fixture".into();
            let callback_map = map.clone();
            map.lock().unwrap().insert(
                uri.clone(),
                Box::new(move || {
                    assert!(
                        callback_map.try_lock().is_ok(),
                        "callback must not inherit the map mutex"
                    );
                }),
            );
            take_download(&map, &uri).unwrap()();
            assert!(
                take_download(&map, &uri).is_none(),
                "callback remains single-use"
            );
        }
    }
}

#[cfg(all(
    test,
    any(target_os = "macos", target_os = "windows", target_os = "linux")
))]
mod original_save_request_tests {
    use super::*;
    #[test]
    fn original_native_request_keeps_slot_when_observer_and_account_retire() {
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let entered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let account = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let mut observers = Vec::new();
        for _ in 0..4 {
            let gate = gate.clone();
            let entered = entered.clone();
            let account = account.clone();
            let (done, observer) = std::sync::mpsc::sync_channel(1);
            observers.push(observer);
            submit_save_operation(move || {
                if account.load(std::sync::atomic::Ordering::Acquire) {
                    entered.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    let mut release = gate.0.lock().unwrap();
                    while !*release {
                        release = gate.1.wait(release).unwrap();
                    }
                }
                let _ = done.send(());
            })
            .unwrap();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while entered.load(std::sync::atomic::Ordering::Acquire) < 2 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        drop(observers);
        account.store(false, std::sync::atomic::Ordering::Release);
        assert!(
            submit_save_operation(|| {}).is_err(),
            "dropping observers/retiring accounts cannot release actual native slots"
        );
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        while SAVE_DIALOGS.available_permits() != 4 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(
            entered.load(std::sync::atomic::Ordering::Acquire),
            2,
            "retired queued requests must not invoke native UI"
        );
    }
}
