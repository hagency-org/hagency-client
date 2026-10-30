//! Functions for generating text previews of timeline events.
//!
//! These text previews are used for:
//! * inline replies within the timeline
//! * preview of a message being replied to above the message input box
//! * previews of each room's latest message in the rooms list

use std::borrow::Cow;

use matrix_sdk::{ruma::{OwnedUserId, events::{room::{guest_access::GuestAccess, history_visibility::HistoryVisibility, join_rules::JoinRule, member::MembershipState, message::{MessageFormat, MessageType}}, AnyRedactionEvent, AnySyncMessageLikeEvent, AnySyncTimelineEvent, StateEventContentChange, SyncMessageLikeEvent}, serde::Raw, UserId}};
use matrix_sdk_base::crypto::types::events::UtdCause;
use matrix_sdk_ui::timeline::{self, AnyOtherStateEventContentChange, EncryptedMessage, EventTimelineItem, MemberProfileChange, MembershipChange, MsgLikeKind, OtherMessageLike, RoomMembershipChange, TimelineItemContent};

use crate::utils;

/// What should be displayed before the text preview of an event.
pub enum BeforeText {
    /// Nothing should be displayed before the text preview.
    Nothing,
    /// The sender's username with a colon should be displayed before the text preview.
    UsernameWithColon,
    /// The sender's username (without a colon) should be displayed before the text preview.
    UsernameWithoutColon,
}

/// A text preview of a timeline event, plus how a username should be displayed before it.
///
/// Call [`TextPreview::format_with()`] to generate displayable text
/// with the appropriately-formatted preceding username.
pub struct TextPreview {
    text: String,
    before_text: BeforeText,
}
impl From<(String, BeforeText)> for TextPreview {
    fn from((text, before_text): (String, BeforeText)) -> Self {
        Self { text, before_text }
    }
}
impl TextPreview {
    /// For a direct-chat list, ordinary messages need only their body. State
    /// events still need the sender as part of the sentence explaining them.
    pub fn direct_message_body(&self) -> Option<&str> {
        matches!(self.before_text, BeforeText::UsernameWithColon).then_some(self.text.as_str())
    }

    /// Formats the text preview with the appropriate preceding username.
    pub fn format_with(
        self,
        username: &str,
        as_html: bool,
    ) -> String {
        let Self { text, before_text } = self;
        match before_text {
            BeforeText::Nothing => text,
            BeforeText::UsernameWithColon => if as_html {
                format!("<b>{}</b>: {}", htmlize::escape_text(username), text)
            } else {
                format!("{}: {}", username, text)
            },
            BeforeText::UsernameWithoutColon => format!(
                "{} {}",
                if as_html { htmlize::escape_text(username) } else { username.into() },
                text,
            ),
        }
    }
}

/// Returns a text preview of the given timeline event as an Html-formatted string.
pub fn text_preview_of_timeline_item(
    content: &TimelineItemContent,
    sender_user_id: &UserId,
    sender_username: &str,
) -> TextPreview {
    match content {
        TimelineItemContent::MsgLike(msg_like_content) => {
            match &msg_like_content.kind {
                MsgLikeKind::Message(msg) => text_preview_of_message(msg.msgtype(), sender_user_id, sender_username),
                MsgLikeKind::Sticker(sticker) => TextPreview::from((
                    format!("[Sticker]: <i>{}</i>", htmlize::escape_text(&sticker.content().body)),
                    BeforeText::UsernameWithColon,
                )),
                MsgLikeKind::Poll(poll_state) => TextPreview::from((
                    format!(
                        "[Poll]: {}",
                        htmlize::escape_text(
                            poll_state.fallback_text()
                                .unwrap_or_else(|| poll_state.results().question)
                        ),
                    ),
                    BeforeText::UsernameWithColon,
                )),
                MsgLikeKind::Redacted => {
                    let mut preview = text_preview_of_redacted_message(
                        None,
                        sender_user_id,
                        sender_username,
                    );
                    preview.text = htmlize::escape_text(&preview.text).into();
                    preview
                }
                MsgLikeKind::UnableToDecrypt(em) => text_preview_of_encrypted_message(em),
                MsgLikeKind::LiveLocation(_) => TextPreview::from((
                    String::from("[Live Location]"),
                    BeforeText::UsernameWithColon,
                )),
                MsgLikeKind::Other(oml) => text_preview_of_other_message_like(oml),
            }
        }
        TimelineItemContent::MembershipChange(membership_change) =>
            text_preview_of_room_membership_change(membership_change, sender_user_id, true),
        TimelineItemContent::ProfileChange(profile_change) =>
            text_preview_of_member_profile_change(profile_change, sender_username, true),
        TimelineItemContent::OtherState(other_state) =>
            text_preview_of_other_state(other_state, true),
        TimelineItemContent::FailedToParseMessageLike { event_type, .. } => TextPreview::from((
            format!("[Failed to parse <i>{}</i> message]", htmlize::escape_text(event_type.to_string())),
            BeforeText::UsernameWithColon,
        )),
        TimelineItemContent::FailedToParseState { event_type, .. } => TextPreview::from((
            format!("[Failed to parse <i>{}</i> state]", htmlize::escape_text(event_type.to_string())),
            BeforeText::UsernameWithColon,
        )),
        TimelineItemContent::CallInvite => TextPreview::from((
            String::from("[Call Invitation]"),
            BeforeText::UsernameWithColon,
        )),
        TimelineItemContent::RtcNotification { .. } => TextPreview::from((
            String::from("[RTC Call Notification]"),
            BeforeText::UsernameWithColon,
        )),
    }
}



/// Returns the plaintext `body` of the given timeline event.
pub fn plaintext_body_of_timeline_item(
    event_tl_item: &EventTimelineItem,
) -> String {
    match event_tl_item.content() {
        TimelineItemContent::MsgLike(msg_likecontent) => {
            match &msg_likecontent.kind {
                MsgLikeKind::Message(msg) => {
                    msg.body().into()
                }
                MsgLikeKind::Sticker(sticker) => {
                    sticker.content().body.clone()
                }
                MsgLikeKind::Poll(poll_state) => {
                    format!("[Poll]: {}", 
                        poll_state.fallback_text().unwrap_or_else(|| poll_state.results().question)
                    )
                }
                MsgLikeKind::Redacted => {
                    let sender_username = utils::get_or_fetch_event_sender(event_tl_item, None);
                    text_preview_of_redacted_message(
                        event_tl_item.latest_json(),
                        event_tl_item.sender(),
                        &sender_username,
                    ).format_with(&sender_username, false)
                }
                MsgLikeKind::UnableToDecrypt(em) => {
                    text_preview_of_encrypted_message(em)
                        .format_with(&utils::get_or_fetch_event_sender(event_tl_item, None), false)
                }
                MsgLikeKind::LiveLocation(_) => {
                    String::from("[Live Location]")
                }
                MsgLikeKind::Other(other_msg_like) => {
                    text_preview_of_other_message_like(other_msg_like)
                        .format_with(&utils::get_or_fetch_event_sender(event_tl_item, None), false)}
            }
        }
        TimelineItemContent::MembershipChange(membership_change) => {
            text_preview_of_room_membership_change(membership_change, event_tl_item.sender(), false)
                .format_with(&utils::get_or_fetch_event_sender(event_tl_item, None), false)
        }
        TimelineItemContent::ProfileChange(profile_change) => {
            let sender = utils::get_or_fetch_event_sender(event_tl_item, None);
            text_preview_of_member_profile_change(profile_change, &sender, false)
                .format_with(&sender, false)
        }
        TimelineItemContent::OtherState(other_state) => {
            text_preview_of_other_state(other_state, false)
                .format_with(&utils::get_or_fetch_event_sender(event_tl_item, None), false)
        }
        TimelineItemContent::FailedToParseMessageLike { event_type, error } => {
            format!("Failed to parse {} message. Error: {}", event_type, error)
        }
        TimelineItemContent::FailedToParseState { event_type, error, state_key } => {
            format!("Failed to parse {} state; key: {}. Error: {}", event_type, state_key, error)
        }
        TimelineItemContent::CallInvite => String::from("[Call Invitation]"),
        TimelineItemContent::RtcNotification { .. } => String::from("[RTC Call Notification]"),
    }
}


/// Returns a text preview of the given message as an Html-formatted string.
fn text_preview_of_message(
    msg: &MessageType,
    sender_user_id: &UserId,
    sender_username: &str,
) -> TextPreview {
    let text = match msg {
        MessageType::Audio(audio) => format!(
            "[Audio]: <i>{}</i>",
            if let Some(formatted_body) = audio.formatted.as_ref() {
                Cow::Borrowed(formatted_body.body.as_str())
            } else {
                htmlize::escape_text(audio.body.as_str())
            }
        ),
        MessageType::Emote(emote) => format!(
            "* {} {}",
            sender_username,
            if let Some(formatted_body) = emote.formatted.as_ref() {
                Cow::Borrowed(formatted_body.body.as_str())
            } else {
                htmlize::escape_text(emote.body.as_str())
            }
        ),
        MessageType::File(file) => format!(
            "[File]: <i>{}</i>",
            if let Some(formatted_body) = file.formatted.as_ref() {
                Cow::Borrowed(formatted_body.body.as_str())
            } else {
                htmlize::escape_text(file.body.as_str())
            }
        ),
        MessageType::Image(image) => format!(
            "[Image]: <i>{}</i>",
            if let Some(formatted_body) = image.formatted.as_ref() {
                Cow::Borrowed(formatted_body.body.as_str())
            } else {
                htmlize::escape_text(image.body.as_str())
            }
        ),
        MessageType::Location(location) => format!(
            "[Location]: <i>{}</i>",
            htmlize::escape_text(&location.body),
        ),
        MessageType::Notice(notice) => format!("<i>{}</i>",
            if let Some(localized)=crate::room::request_notice::presentation(&serde_json::json!({"msgtype":"m.notice","body":notice.body})) {
                htmlize::escape_text(&localized).into_owned().into()
            } else if notice.formatted.is_none() && notice.body.starts_with(['⏳','⚠','⏸']) {
                htmlize::escape_text(crate::i18n::tr(&notice.body))
            } else if let Some(formatted_body) = notice.formatted.as_ref() {
                utils::trim_start_html_whitespace(&formatted_body.body).into()
            } else {
                htmlize::escape_text(notice.body.as_str())
            }
        ),
        MessageType::ServerNotice(notice) => format!(
            "[Server Notice]: <i>{} -- {}</i>",
            notice.server_notice_type.as_str(),
            notice.body,
        ),
        MessageType::Text(text) => {
            let usage=crate::shared::agent_usage::parse_usage(sender_user_id.localpart(),&text.body,text.formatted.is_some());
            let visible_body=usage.as_ref().map_or(text.body.as_str(),|u|u.answer);
            text.formatted
                .as_ref()
                .and_then(|fb|
                    (fb.format == MessageFormat::Html).then(|| {
                        let filtered_and_trimmed = utils::trim_start_html_whitespace(
                            utils::remove_mx_reply(&fb.body)
                        );
                        utils::linkify(filtered_and_trimmed, true).to_string()
                    })
                )
                .unwrap_or_else(|| match utils::linkify(visible_body, false) {
                    Cow::Borrowed(plaintext) => htmlize::escape_text(plaintext).to_string(),
                    Cow::Owned(linkified) => linkified,
                })
        }
        MessageType::VerificationRequest(verification) => format!(
            "[Verification Request] <i>to user {}</i>",
            verification.to,
        ),
        MessageType::Video(video) => format!(
            "[Video]: <i>{}</i>",
            if let Some(formatted_body) = video.formatted.as_ref() {
               Cow::Borrowed(formatted_body.body.as_str())
            } else {
                htmlize::escape_text(&video.body)
            }
        ),
        message if matches!(message.msgtype(), crate::mini_app::MSGTYPE | crate::article_app::MSGTYPE) => {
            crate::mini_app::SharedMiniApp::from_message(message)
                .map(|app| format!("[Mini app] {}", htmlize::escape_text(app.title())))
                .unwrap_or_else(|_| "[Mini app] Unsupported card".into())
        }
        MessageType::_Custom(custom) => {
            // Agent-chat approval events are custom msgtypes, so without this they
            // would fall through to the Rust debug formatting below and leak
            // `CustomMessageContent { msgtype: ... }` into the rooms list. The
            // bridge already writes a human-readable `body`; show that instead.
            #[cfg(feature = "agent_chat")]
            let agent_chat_body = crate::agent_chat::approval::is_approval_msgtype(msg.msgtype())
                .then(|| htmlize::escape_text(msg.body()).to_string());
            #[cfg(not(feature = "agent_chat"))]
            let agent_chat_body: Option<String> = None;
            agent_chat_body.unwrap_or_else(|| format!("[Custom message]: {:?}", custom))
        }
        other => format!(
            "[Unknown message type]: {}",
            htmlize::escape_text(other.body()),
        ),
    };
    TextPreview::from((text, BeforeText::UsernameWithColon))
}

/// Returns a preview of the given raw timeline event.
pub fn text_preview_of_raw_timeline_event(
    raw_event: &Raw<AnySyncTimelineEvent>,
    sender_username: &str,
) -> Option<TextPreview> {
    match raw_event.deserialize().ok()? {
        AnySyncTimelineEvent::MessageLike(
            AnySyncMessageLikeEvent::RoomMessage(
                SyncMessageLikeEvent::Original(ev)
            )
        ) => Some(text_preview_of_message(
            &ev.content.msgtype,
            &ev.sender,
            sender_username,
        )),
        AnySyncTimelineEvent::MessageLike(
            AnySyncMessageLikeEvent::RoomMessage(
                SyncMessageLikeEvent::Redacted(_)
            )
        ) => {
            let sender_user_id = raw_event.get_field::<OwnedUserId>("sender").ok().flatten()?;
            Some(text_preview_of_redacted_message(
                Some(raw_event),
                sender_user_id.as_ref(),
                sender_username,
            ))
        }
        _ => None,
    }
}


/// Returns a plaintext preview of the given redacted message.
///
/// Note: this function accepts the component parts of an [`EventTimelineItem`]
/// instead of an `EventTimelineItem` itself, in order to also accommodate
/// being invoked with the content/details of an [`EmbeddedEvent`].
///
/// [`EmbeddedEvent`]: matrix_sdk_ui::timeline::EmbeddedEvent
pub fn text_preview_of_redacted_message(
    latest_json: Option<&Raw<AnySyncTimelineEvent>>,
    sender_user_id: &UserId,
    original_sender_username: &str,
) -> TextPreview {
    let mut redactor_and_reason = None;
    if let Some(redacted_msg) = latest_json {
        if let Ok(AnySyncTimelineEvent::MessageLike(
            AnySyncMessageLikeEvent::RoomMessage(
                SyncMessageLikeEvent::Redacted(redaction)
            )
        )) = redacted_msg.deserialize() {
            if let Ok(redacted_because) = redaction.unsigned.redacted_because.deserialize() {
                let reason = match &redacted_because {
                    AnyRedactionEvent::RoomRedaction(e) => e.content.reason.clone(),
                    _ => None,
                };
                redactor_and_reason = Some((
                    redacted_because.sender().to_owned(),
                    reason,
                ));
            }
        }
    }
    let text = match redactor_and_reason {
        Some((redactor, Some(reason))) => {
            if redactor == sender_user_id {
                format!("{} deleted their own message: \"{}\".", original_sender_username, reason)
            } else {
                format!("{} deleted {}'s message: \"{}\".", redactor, original_sender_username, reason)
            }
        }
        Some((redactor, None)) => {
            if redactor == sender_user_id {
                format!("{} deleted their own message.", original_sender_username)
            } else {
                format!("{} deleted {}'s message.", redactor, original_sender_username)
            }
        }
        None => {
            format!("{}'s message was deleted.", original_sender_username)
        }
    };
    TextPreview::from((text, BeforeText::Nothing))
}


/// Returns a plaintext preview of the given encrypted message that could not be decrypted.
///
/// This is used for "Unable to decrypt" messages, which may have a known cause
/// for why they could not be decrypted.
pub fn text_preview_of_encrypted_message(
    encrypted_message: &EncryptedMessage,
) -> TextPreview {
    let cause_str = match encrypted_message {
        EncryptedMessage::MegolmV1AesSha2 { cause, .. } => match cause {
            UtdCause::Unknown => None,
            UtdCause::SentBeforeWeJoined => Some(
                crate::i18n::tr("this message was sent before you joined the room.")
            ),
            UtdCause::VerificationViolation => Some(
                crate::i18n::tr("this message was sent by an unverified user.")
            ),
            UtdCause::UnsignedDevice => Some(
                crate::i18n::tr("the sending device wasn't signed by its owner.")
            ),
            UtdCause::UnknownDevice => Some(
                crate::i18n::tr("the sending device's signature was not found.")
            ),
            UtdCause::HistoricalMessageAndBackupIsDisabled => Some(
                crate::i18n::tr("historical messages are not available on this device because server-side key backup was disabled.")
            ),
            UtdCause::WithheldForUnverifiedOrInsecureDevice => Some(
                crate::i18n::tr("your device doesn't meet the sender's security requirements.")
            ),
            UtdCause::WithheldBySender => Some(
                crate::i18n::tr("the sender withheld this message from you.")
            ),
            UtdCause::HistoricalMessageAndDeviceIsUnverified => Some(
                crate::i18n::tr("This device needs historical room keys. Verify it with another trusted device or restore your recovery key.")
            ),
        }
        _ => None,
    };
    let text = if let Some(cause) = cause_str {
        crate::i18n::format("Unable to decrypt: {reason}", &[("reason", cause.into())])
    } else {
        crate::i18n::tr("Unable to decrypt this message.").to_owned()
    };
    TextPreview::from((text, BeforeText::UsernameWithColon))
}

/// Returns a plaintext preview of the given other message-like event.
pub fn text_preview_of_other_message_like(
    other_msg_like: &OtherMessageLike,
) -> TextPreview {
    TextPreview::from((
        format!("[Other message type: {}]", other_msg_like.event_type()),
        BeforeText::UsernameWithColon,
    ))
}

/// Returns a text preview of the given other state event as an Html-formatted string.
pub fn text_preview_of_other_state(
    other_state: &timeline::OtherState,
    format_as_html: bool,
) -> TextPreview {
    let text = match other_state.content() {
        AnyOtherStateEventContentChange::RoomAvatar(_) => {
            crate::i18n::tr("set this room's avatar picture.").to_string()
        }
        AnyOtherStateEventContentChange::RoomCanonicalAlias(StateEventContentChange::Original { content, .. }) => {
            crate::i18n::format(
                "set the main address of this room to {alias}.",
                &[("alias", content.alias.as_ref().map(|a| a.as_str()).unwrap_or_else(|| crate::i18n::tr("none")).to_string())],
            )
        }
        AnyOtherStateEventContentChange::RoomCreate(StateEventContentChange::Original { content, .. }) => {
            crate::i18n::format(
                "created this room (v{version}).",
                &[("version", content.room_version.as_str().to_string())],
            )
        }
        AnyOtherStateEventContentChange::RoomEncryption(_) => {
            crate::i18n::tr("enabled encryption in this room.").to_string()
        }
        AnyOtherStateEventContentChange::RoomGuestAccess(StateEventContentChange::Original { content, .. }) => {
            match &content.guest_access {
                GuestAccess::CanJoin => crate::i18n::tr("has allowed guests to join this room.").to_string(),
                GuestAccess::Forbidden => crate::i18n::tr("has forbidden guests from joining this room.").to_string(),
                custom => crate::i18n::format(
                    "has set custom guest access rules for this room: {rules}",
                    &[("rules", custom.as_str().to_string())],
                ),
            }
        }
        AnyOtherStateEventContentChange::RoomHistoryVisibility(StateEventContentChange::Original { content, .. }) => {
            match &content.history_visibility {
                HistoryVisibility::Invited => crate::i18n::tr("set this room's history to be visible by invited users, since they were invited.").to_string(),
                HistoryVisibility::Joined => crate::i18n::tr("set this room's history to be visible by joined users, since they joined.").to_string(),
                HistoryVisibility::Shared => crate::i18n::tr("set this room's history to be visible by joined users, for all of time.").to_string(),
                HistoryVisibility::WorldReadable => crate::i18n::tr("set this room's history to be visible by anyone for all time.").to_string(),
                custom => crate::i18n::format(
                    "set this room's history to be visible by {visibility}",
                    &[("visibility", custom.as_str().to_string())],
                ),
            }
        }
        AnyOtherStateEventContentChange::RoomJoinRules(StateEventContentChange::Original { content, .. }) => {
            match &content.join_rule {
                JoinRule::Public => crate::i18n::tr("set this room to be joinable by anyone.").to_string(),
                JoinRule::Knock => crate::i18n::tr("set this room to be joinable by invite only or by request.").to_string(),
                JoinRule::Private => crate::i18n::tr("set this room to be private.").to_string(),
                JoinRule::Restricted(_) => crate::i18n::tr("set this room to be joinable by invite only or with restrictions.").to_string(),
                JoinRule::KnockRestricted(_) => crate::i18n::tr("set this room to be joinable by invite only or requestable with restrictions.").to_string(),
                JoinRule::Invite  => crate::i18n::tr("set this room to be joinable by invite only.").to_string(),
                custom => crate::i18n::format(
                    "set custom join rules for this room: {rules}",
                    &[("rules", custom.as_str().to_string())],
                ),
            }
        }
        AnyOtherStateEventContentChange::RoomPinnedEvents(StateEventContentChange::Original { content, .. }) => {
            crate::i18n::format(
                "pinned {count} events in this room.",
                &[("count", content.pinned.len().to_string())],
            )
        }
        AnyOtherStateEventContentChange::RoomName(StateEventContentChange::Original { content, .. }) => {
            let name = if format_as_html {
                htmlize::escape_text(&content.name)
            } else {
                Cow::Borrowed(content.name.as_str())
            };
            crate::i18n::format(
                "changed this room's name to \"{name}\".",
                &[("name", name.to_string())],
            )
        }
        AnyOtherStateEventContentChange::RoomPowerLevels(_) => {
            crate::i18n::tr("set the power levels for this room.").to_string()
        }
        AnyOtherStateEventContentChange::RoomServerAcl(_) => {
            crate::i18n::tr("set the server access control list for this room.").to_string()
        }
        AnyOtherStateEventContentChange::RoomThirdPartyInvite(StateEventContentChange::Original { content, .. }) => {
            let invitee = if format_as_html {
                htmlize::escape_text(&content.display_name)
            } else {
                Cow::Borrowed(content.display_name.as_str())
            };
            crate::i18n::format(
                "invited {user} to this room.",
                &[("user", invitee.to_string())],
            )
        }
        AnyOtherStateEventContentChange::RoomTombstone(StateEventContentChange::Original { content, .. }) => {
            crate::i18n::format(
                "closed this room and upgraded it to {room}.",
                &[("room", content.replacement_room.matrix_to_uri().to_string())],
            )
        }
        AnyOtherStateEventContentChange::RoomTopic(StateEventContentChange::Original { content, .. }) => {
            let topic = if format_as_html {
                htmlize::escape_text(&content.topic)
            } else {
                Cow::Borrowed(content.topic.as_str())
            };
            crate::i18n::format(
                "changed this room's topic to \"{topic}\".",
                &[("topic", topic.to_string())],
            )
        }
        AnyOtherStateEventContentChange::SpaceParent(_) => {
            let state_key  = if format_as_html {
                htmlize::escape_text(other_state.state_key())
            } else {
                Cow::Borrowed(other_state.state_key())
            };
            crate::i18n::format(
                "set this room's parent space to \"{space}\".",
                &[("space", state_key.to_string())],
            )
        }
        AnyOtherStateEventContentChange::SpaceChild(_) => {
            let state_key  = if format_as_html {
                htmlize::escape_text(other_state.state_key())
            } else {
                Cow::Borrowed(other_state.state_key())
            };
            crate::i18n::format(
                "added a new child to this space: \"{child}\".",
                &[("child", state_key.to_string())],
            )
        }
        other => {
            let event_type = other.event_type().to_string();
            crate::i18n::format(
                "changed this room's {event_type} state.",
                &[("event_type", if format_as_html { htmlize::escape_text(event_type).to_string() } else { event_type })],
            )
        }
    };
    TextPreview::from((text, BeforeText::UsernameWithoutColon))
}


/// Returns a text preview of the given member profile change
/// as a plaintext or HTML-formatted string.
pub fn text_preview_of_member_profile_change(
    change: &MemberProfileChange,
    username: &str,
    format_as_html: bool,
) -> TextPreview {
    let name_text = if let Some(name_change) = change.displayname_change() {
        let old = name_change.old.as_deref().unwrap_or(username);
        let old_un = if format_as_html { htmlize::escape_text(old) } else { old.into() };
        if let Some(new) = name_change.new.as_ref() {
            let new_un = if format_as_html { htmlize::escape_text(new) } else { new.into() };
            crate::i18n::format(
                "{old} changed their display name to \"{new}\"",
                &[("old", old_un.to_string()), ("new", new_un.to_string())],
            )
        } else {
            crate::i18n::format(
                "{old} removed their display name",
                &[("old", old_un.to_string())],
            )
        }
    } else {
        String::new()
    };
    let avatar_text = if let Some(_avatar_change) = change.avatar_url_change() {
        if name_text.is_empty() {
            let un = if format_as_html {
                htmlize::escape_text(username)
            } else {
                username.into()
            };
            crate::i18n::format(
                "{user} changed their profile picture",
                &[("user", un.to_string())],
            )
        } else {
            crate::i18n::tr(" and changed their profile picture").to_string()
        }
    } else {
        String::new()
    };

    if name_text.is_empty() && avatar_text.is_empty() {
        // When a profile change is redacted, both these fields are cleared,
        // so just fall back to a generic message.
        return TextPreview::from((
            crate::i18n::tr("changed their profile.").to_string(),
            BeforeText::UsernameWithoutColon,
        ));
    }

    TextPreview::from((
        crate::i18n::format(
            "{name}{avatar}.",
            &[("name", name_text), ("avatar", avatar_text)],
        ),
        BeforeText::Nothing,
    ))
}


/// Returns a text preview of the given room membership change
/// as a plaintext or HTML-formatted string.
pub fn text_preview_of_room_membership_change(
    change: &RoomMembershipChange,
    sender: &UserId,
    format_as_html: bool,
) -> TextPreview {
    let dn = change.display_name();
    let change_user_id = dn.as_deref()
        .unwrap_or_else(|| change.user_id().as_str());
    let change_user_id = if format_as_html {
        htmlize::escape_text(change_user_id)
    } else {
        change_user_id.into()
    };
    let (membership, prev_membership, reason) = match change.content() {
        StateEventContentChange::Original { content, prev_content } => (
            &content.membership,
            prev_content.as_ref().map(|prev| &prev.membership),
            content.reason.as_deref(),
        ),
        StateEventContentChange::Redacted(content) => (&content.membership, None, None),
    };
    let end = match reason.map(|r| r.trim().trim_end_matches('.')).filter(|r| !r.is_empty()) {
        Some(r) if format_as_html => crate::i18n::format(
            ": {reason}.",
            &[("reason", htmlize::escape_text(r).to_string())],
        ),
        Some(r) => crate::i18n::format(": {reason}.", &[("reason", r.to_string())]),
        None => crate::i18n::tr(".").to_string(),
    };
    let text = match change.change() {
        Some(MembershipChange::Joined) =>
            crate::i18n::tr("joined this room.").to_string(),
        Some(MembershipChange::Left) =>
            crate::i18n::format("left this room{end}", &[("end", end.clone())]),
        Some(MembershipChange::Banned) =>
            crate::i18n::format("banned {user} from this room{end}", &[("user", change_user_id.to_string()), ("end", end.clone())]),
        Some(MembershipChange::Unbanned) =>
            crate::i18n::format("unbanned {user} from this room.", &[("user", change_user_id.to_string())]),
        Some(MembershipChange::Kicked) =>
            crate::i18n::format("kicked {user} from this room{end}", &[("user", change_user_id.to_string()), ("end", end.clone())]),
        Some(MembershipChange::Invited) =>
            crate::i18n::format("invited {user} to this room.", &[("user", change_user_id.to_string())]),
        Some(MembershipChange::KickedAndBanned) =>
            crate::i18n::format("kicked and banned {user} from this room{end}", &[("user", change_user_id.to_string()), ("end", end.clone())]),
        Some(MembershipChange::InvitationAccepted) =>
            crate::i18n::tr("accepted an invitation to this room.").to_string(),
        Some(MembershipChange::InvitationRejected) =>
            crate::i18n::format("rejected an invitation to this room{end}", &[("end", end.clone())]),
        Some(MembershipChange::InvitationRevoked) =>
            crate::i18n::format("revoked {user}'s invitation to this room{end}", &[("user", change_user_id.to_string()), ("end", end.clone())]),
        Some(MembershipChange::Knocked) =>
            crate::i18n::format("requested to join this room{end}", &[("end", end.clone())]),
        Some(MembershipChange::KnockAccepted) =>
            crate::i18n::format("accepted {user}'s request to join this room.", &[("user", change_user_id.to_string())]),
        Some(MembershipChange::KnockRetracted) =>
            crate::i18n::tr("retracted their request to join this room.").to_string(),
        Some(MembershipChange::KnockDenied) =>
            crate::i18n::format("denied {user}'s request to join this room{end}", &[("user", change_user_id.to_string()), ("end", end.clone())]),
        // Anything else will be reported by ruma as one of the variants below,
        // so we treat them all the same and just check the new membership state.
        None
        | Some(MembershipChange::NotImplemented)
        | Some(MembershipChange::None)
        | Some(MembershipChange::Error) => match membership {
            MembershipState::Invite =>
                crate::i18n::format("invited {user} to this room.", &[("user", change_user_id.to_string())]),
            MembershipState::Knock =>
                crate::i18n::format("requested to join this room{end}", &[("end", end.clone())]),
            MembershipState::Ban =>
                crate::i18n::format("banned {user} from this room{end}", &[("user", change_user_id.to_string()), ("end", end.clone())]),
            MembershipState::Leave if sender == change.user_id() =>
                crate::i18n::format("left this room{end}", &[("end", end.clone())]),
            MembershipState::Leave =>
                crate::i18n::format("removed {user} from this room{end}", &[("user", change_user_id.to_string()), ("end", end.clone())]),
            // A join re-send is a no-op; the timeline hides it, so this string
            // only shows up in text-preview contexts.
            MembershipState::Join if prev_membership == Some(&MembershipState::Join) =>
                crate::i18n::tr("made no changes to their membership.").to_string(),
            MembershipState::Join =>
                crate::i18n::tr("joined this room.").to_string(),
            custom => {
                let custom = if format_as_html {
                    htmlize::escape_text(custom.as_str())
                } else {
                    custom.as_str().into()
                };
                crate::i18n::format(
                    "set {user}'s membership to \"{membership}\".",
                    &[("user", change_user_id.to_string()), ("membership", custom.to_string())],
                )
            }
        }
    };
    TextPreview::from((text, BeforeText::UsernameWithoutColon))
}

#[cfg(test)]
mod usage_preview_tests {
    use super::*;
    #[test]
    fn thread_and_room_previews_hide_only_verified_success_suffix() {
        let body="<Answer>\n\n✅ 本轮处理已结束 · 已运行 3 秒\n本次用量 50 Token（输入 30 / 输出 20；缓存输入 10，推理输出 5）· 工具请求 2 次，已返回 1 次";
        let message=MessageType::text_plain(body);
        let sender=matrix_sdk::ruma::user_id!("@_hagency_agt_01abc:test");
        let preview=text_preview_of_message(&message,sender,"Agent").format_with("Agent",true);
        assert!(preview.contains("&lt;Answer&gt;"));assert!(!preview.contains("Token"));
        let human=text_preview_of_message(&message,matrix_sdk::ruma::user_id!("@alice:test"),"Alice").format_with("Alice",true);
        assert!(human.contains("Token"));
        let raw=Raw::<AnySyncTimelineEvent>::from_json(serde_json::value::to_raw_value(&serde_json::json!({"event_id":"$reply:test","sender":sender,"type":"m.room.message","origin_server_ts":123,"content":{"msgtype":"m.text","body":body}})).unwrap());
        assert!(!text_preview_of_raw_timeline_event(&raw,"Agent").unwrap().format_with("Agent",true).contains("Token"));
        // Presentation does not change the SDK message used for copy/forward.
        assert_eq!(message.body(),body);
    }
}
