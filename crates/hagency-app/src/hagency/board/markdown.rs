//! Markdown remains the stored source. Rendering never enables executable blocks
//! or fetches remote images, and toolbar edits use the native undo/IME pipeline.
use super::*;
use pulldown_cmark::{Event as MdEvent, Options, Parser, Tag, TagEnd, html};
use std::ops::Range;

#[derive(Script, ScriptHook, Widget)]
pub struct BoardMarkdown {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    text: String,
}
impl Widget for BoardMarkdown {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        for action in actions {
            if let Some(action) = action.as_widget_action()
                && let HtmlLinkAction::Clicked { url, .. } = action.cast()
                && let Ok(url) = url::Url::parse(&url)
                && matches!(url.scheme(), "http" | "https")
            {
                cx.action(crate::shared::web_browser::WebBrowserAction::Open(url));
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
    fn text(&self) -> String {
        self.text.clone()
    }
    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        if self.text == text {
            return;
        }
        self.text = text.to_owned();
        self.view.html(cx, ids!(body)).set_text(cx, text)
    }
}

pub(super) fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
pub(super) fn render_markdown(source: &str) -> String {
    render_markdown_with_media(source, 0)
}

pub(super) fn render_markdown_with_media(source: &str, scope: u64) -> String {
    let mut image_end = false;
    let events = Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH,
    )
    .map(|event| match event {
        MdEvent::Html(text) | MdEvent::InlineHtml(text) => MdEvent::Text(text),
        MdEvent::Start(Tag::Image { dest_url, .. }) => {
            if let Some(id) = dest_url
                .strip_prefix("attachment:")
                .filter(|id| super::media::valid_id(id))
            {
                image_end = true;
                MdEvent::Html(format!("<boardimage src=\"{scope}:{id}\">").into())
            } else {
                image_end = false;
                MdEvent::Text("[".into())
            }
        }
        MdEvent::End(TagEnd::Image) => {
            if image_end {
                MdEvent::Html("</boardimage>".into())
            } else {
                MdEvent::Text("]".into())
            }
        }
        MdEvent::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let safe =
                url::Url::parse(&dest_url).is_ok_and(|u| matches!(u.scheme(), "https" | "http"));
            MdEvent::Start(Tag::Link {
                link_type,
                dest_url: if safe { dest_url } else { "".into() },
                title,
                id,
            })
        }
        other => other,
    });
    let mut output = String::new();
    html::push_html(&mut output, events);
    output
}

#[derive(Clone, Copy)]
enum Format {
    Bold,
    Italic,
    Heading,
    Bullet,
    Ordered,
    Quote,
    Code,
    Block,
    Link,
}
fn formats() -> [(&'static [LiveId], Format); 9] {
    [
        (ids!(bold), Format::Bold),
        (ids!(italic), Format::Italic),
        (ids!(heading), Format::Heading),
        (ids!(bullet), Format::Bullet),
        (ids!(ordered), Format::Ordered),
        (ids!(quote), Format::Quote),
        (ids!(code), Format::Code),
        (ids!(block), Format::Block),
        (ids!(link), Format::Link),
    ]
}

/// Returns the replacement and the selection within it, using UTF-8 byte offsets.
fn format_edit(
    source: &str,
    mut range: Range<usize>,
    format: Format,
) -> (Range<usize>, String, Range<usize>) {
    if matches!(
        format,
        Format::Heading | Format::Bullet | Format::Ordered | Format::Quote
    ) {
        range.start = source[..range.start].rfind('\n').map_or(0, |i| i + 1);
        let end = if range.end > range.start && source[..range.end].ends_with('\n') {
            range.end - 1
        } else {
            range.end
        };
        range.end = source[end..].find('\n').map_or(source.len(), |i| end + i);
        let prefix = match format {
            Format::Heading => "## ",
            Format::Bullet => "- ",
            Format::Ordered => "1. ",
            _ => "> ",
        };
        let selected = &source[range.clone()];
        let remove = !selected.is_empty() && selected.lines().all(|line| line.starts_with(prefix));
        let value = selected
            .split('\n')
            .map(|line| {
                if remove {
                    line.strip_prefix(prefix).unwrap_or(line).to_owned()
                } else {
                    format!("{prefix}{line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let end = value.len();
        return (range, value, 0..end);
    }
    let selected = &source[range.clone()];
    let (open, close) = match format {
        Format::Bold => ("**".into(), "**".into()),
        Format::Italic => ("*".into(), "*".into()),
        Format::Code => {
            let fence = "`".repeat(
                selected
                    .split(|c| c != '`')
                    .map(str::len)
                    .max()
                    .unwrap_or(0)
                    + 1,
            );
            // A space keeps a selected edge backtick separate from its delimiter.
            if selected.starts_with('`') || selected.ends_with('`') {
                (format!("{fence} "), format!(" {fence}"))
            } else {
                (fence.clone(), fence)
            }
        }
        Format::Block => {
            let fence = "`".repeat(
                3.max(
                    selected
                        .split(|c| c != '`')
                        .map(str::len)
                        .max()
                        .unwrap_or(0)
                        + 1,
                ),
            );
            (format!("\n{fence}\n"), format!("\n{fence}\n"))
        }
        Format::Link => ("[".into(), "](https://)".into()),
        _ => unreachable!(),
    };
    // Repeating a wrapping tool removes its own surrounding delimiters.
    if matches!(format, Format::Bold | Format::Italic)
        && source[..range.start].ends_with(&open)
        && source[range.end..].starts_with(&close)
    {
        return (
            range.start - open.len()..range.end + close.len(),
            selected.into(),
            0..selected.len(),
        );
    }
    let start = open.len();
    let value = format!("{open}{selected}{close}");
    let selection = if matches!(format, Format::Link) {
        start + selected.len() + 2..value.len() - 1
    } else {
        start..start + selected.len()
    };
    (range, value, selection)
}

impl ProjectBoard {
    pub(super) fn sync_markdown_views(&self, cx: &mut Cx) {
        for (path, preview) in [
            (ids!(description_editor), self.description_preview),
            (ids!(comment_editor), self.comment_preview),
        ] {
            let editor = self.view.view(cx, path);
            editor.view(cx, ids!(source_wrap)).set_visible(cx, !preview);
            editor.view(cx, ids!(tools)).set_visible(cx, !preview);
            editor.view(cx, ids!(media_tools)).set_visible(cx, !preview);
            let rendered = editor.widget(cx, ids!(rendered));
            rendered.set_visible(cx, preview);
            if preview {
                let source = editor.text_input(cx, ids!(source)).text();
                rendered.set_text(
                    cx,
                    &if source.trim().is_empty() {
                        format!("<em>{}</em>", escape_html(tr("Nothing to preview yet.")))
                    } else {
                        self.media_html(&source)
                    },
                );
            }
            for (id, selected) in [(ids!(edit), !preview), (ids!(preview), preview)] {
                let mut button = editor.button(cx, id);
                if selected {
                    script_apply_eval!(cx,button,{draw_bg.color:mod.widgets.RINX_HOVER draw_text.color:mod.widgets.RINX_ACCENT});
                } else {
                    script_apply_eval!(cx,button,{draw_bg.color:mod.widgets.RINX_SURFACE draw_text.color:mod.widgets.RINX_MUTED});
                }
            }
        }
    }

    pub(super) fn sync_markdown_controls(&self, cx: &mut Cx, description: bool, comment: bool) {
        for (path, enabled) in [
            (ids!(description_editor), description),
            (ids!(comment_editor), comment),
        ] {
            let editor = self.view.view(cx, path);
            let input = editor.text_input(cx, ids!(source));
            // The native setter invalidates text layout even when the value is
            // unchanged. Running it after every hover/action breaks the next
            // pointer hit and drag selection before another layout is ready.
            if input.is_read_only() == enabled {
                input.set_is_read_only(cx, !enabled);
            }
            for id in [ids!(insert_image), ids!(paste_image)] {
                editor.button(cx, id).set_board_enabled(
                    cx,
                    enabled && self.detail_loaded && self.selected.is_some(),
                );
            }
            for (id, _) in formats() {
                editor.button(cx, id).set_board_enabled(cx, enabled);
            }
        }
    }

    pub(super) fn handle_markdown(&mut self, cx: &mut Cx, actions: &Actions) {
        for (path, description) in [
            (ids!(description_editor), true),
            (ids!(comment_editor), false),
        ] {
            let editor = self.view.view(cx, path);
            if editor.button(cx, ids!(preview)).clicked(actions)
                || editor.button(cx, ids!(edit)).clicked(actions)
            {
                let preview = editor.button(cx, ids!(preview)).clicked(actions);
                if description {
                    self.description_preview = preview;
                } else {
                    self.comment_preview = preview;
                }
                self.sync_inline_images(cx);
                self.view.redraw(cx);
            }
            let input = editor.text_input(cx, ids!(source));
            if input.is_read_only() || input.is_composing() {
                continue;
            }
            for (id, format) in formats() {
                if !editor.button(cx, id).clicked(actions) {
                    continue;
                }
                let text = input.text();
                let mut selection = input.selection();
                let start = selection.anchor.index.min(selection.cursor.index);
                let end = selection.anchor.index.max(selection.cursor.index);
                let (range, replacement, selected) = format_edit(&text, start..end, format);
                let offset = range.start;
                if input
                    .replace_range(
                        cx,
                        range,
                        &replacement,
                        makepad_widgets::text_input::UndoGroup::New,
                    )
                    .is_ok()
                {
                    selection.anchor.index = offset + selected.start;
                    selection.cursor.index = offset + selected.end;
                    input.set_selection(cx, selection);
                    cx.set_key_focus(input.area());
                    if description {
                        self.dirty = true;
                        self.discard_armed = false;
                    }
                    self.sync_controls(cx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markdown_renders_content_without_html_execution_or_remote_image_fetches() {
        let html = render_markdown(
            "## 验收\n\n**重点** 和 `代码`\n\n- 完成\n\n```runsplash\nmalicious()\n```\n\n<script>bad()</script>\n\n![图片](https://example.com/track)\n\n[bad](javascript:bad)\n\n[docs](https://example.com)",
        );
        assert!(
            html.contains("<h2>验收</h2>")
                && html.contains("<strong>重点</strong>")
                && html.contains("<li>完成</li>")
        );
        assert!(html.contains("&lt;script&gt;") && html.contains("<pre><code"));
        assert!(
            !html.contains("<script>") && !html.contains("<img") && !html.contains("javascript:")
        );
        assert!(html.contains("href=\"https://example.com\""));
    }
    #[test]
    fn formatting_keeps_chinese_selection_and_toggles_undoable_delimiters() {
        let (range, text, selected) = format_edit("开始重点结束", 6..12, Format::Bold);
        assert_eq!(range, 6..12);
        assert_eq!(text, "**重点**");
        assert_eq!(selected, 2..8);
        let (range, text, _) = format_edit("开始**重点**结束", 8..14, Format::Bold);
        assert_eq!(range, 6..16);
        assert_eq!(text, "重点");
        let (_, text, _) = format_edit("甲\n乙\n丙", 0..8, Format::Bullet);
        assert_eq!(text, "- 甲\n- 乙");
        let (_, empty, _) = format_edit("", 0..0, Format::Heading);
        assert_eq!(empty, "## ");
        let (_, inline, _) = format_edit("`x`", 0..3, Format::Code);
        assert!(render_markdown(&inline).contains("<code>`x`</code>"));
        let (_, text, _) = format_edit("```", 0..3, Format::Block);
        assert_eq!(text, "\n````\n```\n````\n");
    }
}
