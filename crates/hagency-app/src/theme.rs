//! Rinx's adapter to the host-owned Makepad stylesheet (ADR 0009).
//! No renderer or copied OctoSense preset catalog lives here.
use makepad_widgets::{
    desktop_style::{self, DesktopStyle, StyleSheet},
    *,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
pub mod packages;
pub mod system;
pub mod host;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    #[default]
    Teal,
    Violet,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub appearance: Appearance,
    pub accent: Accent,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub revision: u64,
    pub page: Vec4f,
    pub surface: Vec4f,
    pub field: Vec4f,
    pub ink: Vec4f,
    pub muted: Vec4f,
    pub accent: Vec4f,
    pub on_accent: Vec4f,
    pub border: Vec4f,
    pub hover: Vec4f,
    pub pressed: Vec4f,
    pub selected: Vec4f,
    pub radius: f64,
    pub tokens: octosense_theme_contract::Tokens,
    pub text_scale: f64,
    pub spacing_scale: f64,
    pub control_height: f64,
    pub reading_width: f64,
    pub page_gutter: f64,
    pub motion_ms: f64,
}

#[derive(Default)]
struct Runtime {
    heaps: HashMap<usize, State>,
}
#[derive(Default)]
struct State {
    hosted: bool,
    selection: Selection,
    preferences: packages::Preferences,
    previous: Option<packages::Preferences>,
    preview: Option<packages::Preferences>,
    pending_apply: Option<u64>,
    pending_ack: bool,
    transaction: u64,
    known_good: packages::Preferences,
    host_revision: Option<String>,
}

fn rgb(value: u32) -> Vec4f {
    vec4(
        (value >> 16 & 255) as f32 / 255.,
        (value >> 8 & 255) as f32 / 255.,
        (value & 255) as f32 / 255.,
        1.,
    )
}
fn mix(a: Vec4f, b: Vec4f, amount: f32) -> Vec4f {
    a * (1. - amount) + b * amount
}
pub fn argb(c: Vec4f) -> u32 {
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    byte(c.w) << 24 | byte(c.x) << 16 | byte(c.y) << 8 | byte(c.z)
}

impl Selection {
    /// Keep the platform family. Only palette roles change between selections.
    pub fn stylesheet(self, family: DesktopStyle) -> StyleSheet {
        let dark = self.appearance == Appearance::Dark;
        let mut sheet = StyleSheet::load_with_appearance(family, dark);
        let (accent, on_accent) = match (self.accent, dark) {
            (Accent::Teal, false) => ("09616f", "ffffff"),
            (Accent::Teal, true) => ("72d3df", "103239"),
            (Accent::Violet, false) => ("7045b8", "ffffff"),
            (Accent::Violet, true) => ("cbb1ff", "302044"),
        };
        for role in [
            "focus",
            "ctrl_selected",
            "ctrl_active",
            "outset_active",
            "cursor",
            "text_cursor",
            "bevel_inset_1_focus",
            "bevel_inset_2_focus",
            "bevel_outset_1_focus",
            "bevel_outset_2_focus",
        ] {
            sheet
                .theme
                .push_str(&format!("\nmod.theme.color_{role} = #x{accent}\n"));
        }
        sheet.theme.push_str(&format!(
            "mod.theme.color_text_on_accent = #x{on_accent}\ntrue\n"
        ));
        sheet
    }
}

fn platform_family(vm: &mut ScriptVm) -> DesktopStyle {
    match vm.cx().os_type() {
        OsType::Android(_) | OsType::OpenHarmony(_) => DesktopStyle::Android,
        OsType::Ios(_) => DesktopStyle::Ios,
        OsType::Windows => DesktopStyle::Windows,
        _ => DesktopStyle::Macos,
    }
}

/// Called before standalone widget registration. On reapply the installed sheet
/// wins, including a full stylesheet received from an external Makepad host.
pub fn init_standalone(vm: &mut ScriptVm) {
    let key = vm.bx.heap.heap_key();
    if vm.cx_mut().global::<Runtime>().heaps.contains_key(&key) {
        return;
    }
    let family = platform_family(vm);
    system::start();
    let stored = vm.with_cx_mut(|cx| packages::load(cx, crate::app_data_dir(), family));
    let selection = stored.current.selection;
    if desktop_style::current(vm).is_none() {
        let sheet = vm.with_cx_mut(|cx| packages::stylesheet(cx, &stored.current, family))
            .unwrap_or_else(|_| selection.stylesheet(family));
        desktop_style::install(vm, sheet);
    }
    vm.cx_mut().global::<Runtime>().heaps.insert(
        key,
        State {
            hosted: false,
            selection,
            preferences: stored.current.clone(),
            known_good: stored.current,
            previous: stored.previous,
            preview: None,
            pending_apply: None,
            pending_ack: false,
            transaction: 0,
            host_revision: None,
        },
    );
}

/// Per isolate: a hosted module never consumes or overwrites standalone settings.
pub fn init_hosted(vm: &mut ScriptVm) {
    let key = vm.bx.heap.heap_key();
    vm.cx_mut()
        .global::<Runtime>()
        .heaps
        .entry(key)
        .or_default()
        .hosted = true;
}

pub fn selection(cx: &mut Cx) -> Option<Selection> {
    cx.with_vm(selection_for_vm)
}
pub(crate) fn selection_for_vm(vm: &mut ScriptVm) -> Option<Selection> {
    let key = vm.bx.heap.heap_key();
    let state = vm.cx_mut().global::<Runtime>().heaps.get(&key)?;
    (!state.hosted).then_some(state.selection)
}

/// Use the framework event so the normal app re-registration and ScriptReapply
/// path runs. The caller handles an error without changing its active selection.
pub fn select(cx: &mut Cx, selection: Selection) -> Result<(), String> {
    let mut preferences = packages::current(cx)?;
    preferences.selection = selection;
    preferences.follow_system = false;
    packages::apply(cx, preferences)
}

pub fn snapshot(cx: &mut Cx) -> Snapshot {
    cx.with_vm(snapshot_for_vm)
}

pub fn snapshot_for_vm(vm: &mut ScriptVm) -> Snapshot {
    let theme = vm.module(id!(theme));
    let color = |vm: &mut ScriptVm, name: &str, fallback| {
        vm.bx
            .heap
            .value(theme, LiveId::from_str(name).into(), NoTrap)
            .as_color()
            .map(|rgba| {
                vec4(
                    (rgba >> 24) as f32 / 255.,
                    (rgba >> 16 & 255) as f32 / 255.,
                    (rgba >> 8 & 255) as f32 / 255.,
                    (rgba & 255) as f32 / 255.,
                )
            })
            .unwrap_or_else(|| rgb(fallback))
    };
    let page = color(vm, "color_bg_app", 0xf7f9fc);
    let surface = color(vm, "color_bg_container", 0xffffff);
    let field = color(vm, "color_inset", 0xf0f2f5);
    let ink = color(vm, "color_text", 0x191919);
    // A readable secondary role. Disabled control opacity is not reused here.
    let muted = mix(ink, page, 0.25);
    let accent = color(vm, "color_focus", 0x0d7988);
    let on_accent = color(vm, "color_text_on_accent", 0xffffff);
    let border = color(vm, "color_bevel_outset_2", 0xc0c6cc);
    let radius = vm
        .bx
        .heap
        .value(theme, id!(corner_radius).into(), NoTrap)
        .as_number()
        .unwrap_or(6.);
    // Installation precedes widget reapply. Include resolved values so a read
    // between those steps cannot make consumers cache the old palette as new.
    let material = format!(
        "{}{page:?}{surface:?}{field:?}{ink:?}{accent:?}{on_accent:?}{border:?}{radius}",
        desktop_style::current(vm)
            .map(|s| s.to_json())
            .unwrap_or_default()
    );
    let hash = blake3::hash(material.as_bytes());
    let revision = u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap());
    let mut result = Snapshot {
        revision,
        page,
        surface,
        field,
        ink,
        muted,
        accent,
        on_accent,
        border,
        hover: mix(surface, ink, 0.06),
        pressed: mix(surface, ink, 0.12),
        selected: mix(surface, accent, 0.10),
        radius,
        tokens: Default::default(),
        text_scale: 1.,
        spacing_scale: 1.,
        control_height: 44.,
        reading_width: 760.,
        page_gutter: 16.,
        motion_ms: 150.,
    };
    let mut tokens = packages::base_tokens(&result);
    for role in octosense_theme_contract::COLORS {
        let name = format!("octo_{}", role.replace('.', "_"));
        let fallback = octosense_theme_contract::hex(&tokens[*role]).unwrap();
        let value = color(vm, &name, u32::from_str_radix(&fallback, 16).unwrap());
        tokens.insert(
            (*role).into(),
            octosense_theme_contract::color(&format!("{:06x}", argb(value) & 0xffffff)).unwrap(),
        );
    }
    let number = |vm: &mut ScriptVm, name: &str, fallback: f64| {
        vm.bx
            .heap
            .value(theme, LiveId::from_str(name).into(), NoTrap)
            .as_number()
            .unwrap_or(fallback)
    };
    result.text_scale = number(vm, "octo_typography_scale", 1.);
    result.spacing_scale = number(vm, "octo_metrics_spacing", 1.);
    result.control_height = number(vm, "octo_control_height", 44.);
    result.reading_width = number(vm, "octo_reading_width", 760.);
    result.page_gutter = number(vm, "octo_page_gutter", 16.);
    result.motion_ms = number(vm, "octo_motion_ms", 150.);
    for (key, n) in [
        ("shape.surface.radius", result.radius),
        ("metrics.control.height", result.control_height),
        ("metrics.reading.width", result.reading_width),
        ("metrics.page.gutter", result.page_gutter),
    ] {
        tokens.insert(key.into(), octosense_theme_contract::dimension(n));
    }
    tokens.insert(
        "typography.scale".into(),
        octosense_theme_contract::number(result.text_scale),
    );
    tokens.insert(
        "metrics.spacing".into(),
        octosense_theme_contract::number(result.spacing_scale),
    );
    tokens.get_mut("motion.duration").unwrap().value["value"] = serde_json::json!(result.motion_ms);
    let font_value = vm
        .bx
        .heap
        .value(theme, id!(octo_typography_family).into(), NoTrap);
    let mut font_family = String::new();
    vm.bx.heap.cast_to_string(font_value, &mut font_family);
    if matches!(font_family.as_str(), "system" | "mono") {
        tokens.get_mut("typography.family").unwrap().value = serde_json::json!(font_family);
    }
    result.tokens = tokens;
    result.muted = result.role("color.content.secondary");
    result.hover = result.role("color.state.hover");
    result.pressed = result.role("color.state.pressed");
    result.selected = result.role("color.state.selected");
    result
}

impl Snapshot {
    pub fn role(&self, name: &str) -> Vec4f {
        self.tokens
            .get(name)
            .and_then(|t| octosense_theme_contract::rgb(t).ok())
            .map(|[r, g, b]| vec4(r as f32, g as f32, b as f32, 1.))
            .unwrap_or(self.ink)
    }
}

/// Registered in the parent and trusted Splash preludes. Resolves once per
/// registration/reapply; controls and app aliases all read the same values.
pub fn script_mod(vm: &mut ScriptVm) {
    let s = snapshot_for_vm(vm);
    // Splash reruns presentation source on reapply. Apps can guard startup
    // service calls and initial models without suppressing event handlers.
    let theme_api = vm.new_module(id!(rinx_theme));
    vm.add_method(
        theme_api,
        id_lut!(reloading),
        script_args_def!(),
        |vm, _| vm.is_reload().into(),
    );
    // Move away from the foreground so hover/pressed text keeps its contrast.
    let away = if luminance(s.on_accent) > 0.5 {
        rgb(0)
    } else {
        rgb(0xffffff)
    };
    let accent_hover = mix(s.accent, away, 0.08);
    let accent_down = mix(s.accent, away, 0.16);
    let dark = luminance(s.page) < 0.5;
    // TextFlow paints its selection rectangles over glyphs. Keep the highlight
    // translucent so selected text remains legible on both light and dark UI.
    let text_selection_bg = vec4(
        s.accent.x,
        s.accent.y,
        s.accent.z,
        if dark { 0.34 } else { 0.26 },
    );
    script_eval!(vm, {
        mod.widgets.RINX_PAGE = #(s.page)
        mod.widgets.RINX_SURFACE = #(s.surface)
        mod.widgets.RINX_FIELD = #(s.field)
        mod.widgets.RINX_INK = #(s.ink)
        mod.widgets.RINX_MUTED = #(s.muted)
        mod.widgets.RINX_ACCENT = #(s.accent)
        mod.widgets.RINX_ON_ACCENT = #(s.on_accent)
        mod.widgets.RINX_BORDER = #(s.border)
        mod.widgets.RINX_HOVER = #(s.hover)
        mod.widgets.RINX_PRESSED = #(s.pressed)
        mod.widgets.RINX_CAPTION_HOVER = #(mix(s.page, s.ink, 0.12))
        mod.widgets.RINX_CAPTION_PRESSED = #(mix(s.page, s.ink, 0.20))
        mod.widgets.RINX_SELECTED = #(s.selected)
        // Primary navigation needs a stronger fill than contextual list rows.
        mod.widgets.RINX_NAV_SELECTED = #(mix(s.selected, s.accent, 0.30))
        mod.widgets.RINX_SCROLLBAR = #(mix(s.page, s.ink, 0.35))
        mod.widgets.RINX_SCROLLBAR_HOVER = #(mix(s.page, s.ink, 0.50))
        mod.widgets.RINX_SCROLLBAR_DRAG = #(mix(s.page, s.ink, 0.65))
        // Surface elevation ramp: recessed rail → panel → lifted content.
        // Dark mode lifts with ink, light mode recesses with ink / lifts to white.
        mod.widgets.RINX_RAIL_BG = #(if dark { mix(s.page, vec4(0., 0., 0., 1.), 0.17) } else { mix(s.page, s.ink, 0.045) })
        mod.widgets.RINX_PANEL_BG = #(if dark { mix(s.page, s.ink, 0.02) } else { s.page })
        mod.widgets.RINX_CONTENT_BG = #(if dark { mix(s.page, s.ink, 0.055) } else { mix(s.page, vec4(1., 1., 1., 1.), 0.7) })
        mod.widgets.RINX_DIVIDER = #(mix(s.page, s.ink, if dark { 0.10 } else { 0.09 }))
        mod.widgets.RINX_TEXT_SELECTION_BG = #(text_selection_bg)
        mod.widgets.RINX_ACCENT_HOVER = #(accent_hover)
        mod.widgets.RINX_ACCENT_DOWN = #(accent_down)
    });
    for (name, role) in [
        ("RINX_DISABLED", "color.content.disabled"),
        ("RINX_SUCCESS_FG", "color.status.success.foreground"),
        ("RINX_SUCCESS_BG", "color.status.success.background"),
        ("RINX_WARNING_FG", "color.status.warning.foreground"),
        ("RINX_WARNING_BG", "color.status.warning.background"),
        ("RINX_DANGER_FG", "color.status.danger.foreground"),
        ("RINX_DANGER_BG", "color.status.danger.background"),
        ("RINX_INFO_FG", "color.status.info.foreground"),
        ("RINX_INFO_BG", "color.status.info.background"),
        ("RINX_INCOMING", "color.chat.incoming"),
        ("RINX_OUTGOING", "color.chat.outgoing"),
        ("RINX_MENTION", "color.chat.mention"),
        ("RINX_CODE_BG", "color.code.background"),
        ("RINX_CODE_FG", "color.code.foreground"),
    ] {
        let widgets = vm.module(id!(widgets));
        let value = script_eval!(vm, { #(s.role(role)) });
        vm.bx
            .heap
            .set_value(widgets, LiveId::from_str(name).into(), value, NoTrap);
    }
    script_eval!(vm, {
        mod.widgets.RINX_TEXT_SCALE = #(s.text_scale)
        // TextStyle sizes are points: at 100%, 0.75 pt is one logical pixel.
        mod.widgets.RINX_UI_SIZE = #(9.75 * s.text_scale) // 13 px navigation and controls
        mod.widgets.RINX_BODY_SIZE = #(10.5 * s.text_scale) // 14 px reading text
        mod.widgets.RINX_META_SIZE = #(9. * s.text_scale) // 12 px secondary text
        mod.widgets.RINX_HEADING_SIZE = #(11.25 * s.text_scale) // 15 px pane headings
        mod.widgets.RINX_SECTION_SIZE = #(12. * s.text_scale) // 16 px sections
        mod.widgets.RINX_TITLE_SIZE = #(13.5 * s.text_scale) // 18 px page titles
        mod.widgets.RINX_BRAND_SIZE = #(13.5 * s.text_scale)
        mod.widgets.RINX_DISPLAY_SIZE = #(21. * s.text_scale) // 28 px welcome / result headings
        mod.widgets.RINX_DOCUMENT_SIZE = #(13.5 * s.text_scale) // 18 px article reading / editing
        mod.widgets.RINX_GLYPH_SIZE = #(15. * s.text_scale) // 20 px symbolic toolbar glyphs
        mod.widgets.RINX_BUTTON_HEIGHT = #(if cfg!(any(target_os = "android", target_os = "ios", target_env = "ohos")) { s.control_height.max(44. * s.text_scale) } else { (36. * s.text_scale).max(s.control_height * 36. / 44.) })
        mod.widgets.RINX_ICON_BUTTON_SIZE = #(if cfg!(any(target_os = "android", target_os = "ios", target_env = "ohos")) {44. * s.text_scale} else {32. * s.text_scale})
        mod.widgets.RINX_CONTROL_RADIUS = #(s.radius * 0.75)
        mod.widgets.RINX_CARD_RADIUS = #(s.radius * 1.25)
        mod.widgets.RINX_CHIP_HEIGHT = #(if cfg!(any(target_os = "android", target_os = "ios", target_env = "ohos")) {44. * s.text_scale} else {24. * s.text_scale})
        mod.widgets.RINX_SPACING = #(s.spacing_scale)
        mod.widgets.RINX_CONTROL_HEIGHT = #(if cfg!(any(target_os = "android", target_os = "ios", target_env = "ohos")) {s.control_height.max(44. * s.text_scale)} else {(36. * s.text_scale).max(s.control_height * 36. / 44.)})
        mod.widgets.RINX_READING_WIDTH = #(s.reading_width)
        mod.widgets.RINX_GUTTER = #(s.page_gutter)
        mod.widgets.RINX_MOTION = #(s.motion_ms / 1000.)
    });
    controls::script_mod(vm);
}

fn luminance(c: Vec4f) -> f32 {
    let linear = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c.x) + 0.7152 * linear(c.y) + 0.0722 * linear(c.z)
}

mod controls {
    use makepad_widgets::*;
    script_mod! {
        use mod.prelude.widgets.*
        use mod.widgets.*
        // Update the existing prototypes in place: ScrollBars, PortalList and
        // TextInput already reference them before application recipes register.
        // An 8px track with the upstream 1px inset and 5px thumb leaves 2px
        // between the visible handle and the edge on either scroll axis.
        mod.widgets.ScrollBar.bar_size = 8
        mod.widgets.ScrollBar.bar_side_margin = 2
        mod.widgets.ScrollBar.draw_bg.size = uniform(5.0)
        mod.widgets.ScrollBar.draw_bg.border_size = uniform(0.0)
        mod.widgets.ScrollBar.draw_bg.border_radius = uniform(2.5)
        mod.widgets.ScrollBar.draw_bg.color = uniform(RINX_SCROLLBAR)
        mod.widgets.ScrollBar.draw_bg.color_hover = uniform(RINX_SCROLLBAR_HOVER)
        mod.widgets.ScrollBar.draw_bg.color_drag = uniform(RINX_SCROLLBAR_DRAG)
        mod.widgets.ScrollBarTabs.bar_size = 8
        mod.widgets.ScrollBarTabs.bar_side_margin = 2
        mod.widgets.ScrollBarTabs.draw_bg.size = uniform(5.0)
        mod.widgets.ScrollBarTabs.draw_bg.border_size = uniform(0.0)
        mod.widgets.ScrollBarTabs.draw_bg.border_radius = uniform(2.5)
        mod.widgets.ScrollBarTabs.draw_bg.color_hover = uniform(RINX_SCROLLBAR_HOVER)
        mod.widgets.ScrollBarTabs.draw_bg.color_drag = uniform(RINX_SCROLLBAR_DRAG)
        // All native windows share these prototypes, including windows opened later.
        mod.widgets.Window.caption_bar.windows_buttons.min.draw_bg.color = uniform(RINX_INK)
        mod.widgets.Window.caption_bar.windows_buttons.min.draw_bg.color_hover = uniform(RINX_INK)
        mod.widgets.Window.caption_bar.windows_buttons.min.draw_bg.color_down = uniform(RINX_INK)
        mod.widgets.Window.caption_bar.windows_buttons.min.draw_bg.bg_color_hover = uniform(RINX_CAPTION_HOVER)
        mod.widgets.Window.caption_bar.windows_buttons.min.draw_bg.bg_color_down = uniform(RINX_CAPTION_PRESSED)
        mod.widgets.Window.caption_bar.windows_buttons.max.draw_bg.color = uniform(RINX_INK)
        mod.widgets.Window.caption_bar.windows_buttons.max.draw_bg.color_hover = uniform(RINX_INK)
        mod.widgets.Window.caption_bar.windows_buttons.max.draw_bg.color_down = uniform(RINX_INK)
        mod.widgets.Window.caption_bar.windows_buttons.max.draw_bg.bg_color_hover = uniform(RINX_CAPTION_HOVER)
        mod.widgets.Window.caption_bar.windows_buttons.max.draw_bg.bg_color_down = uniform(RINX_CAPTION_PRESSED)
        mod.widgets.Window.caption_bar.windows_buttons.close.draw_bg.color = uniform(RINX_INK)
        mod.widgets.RinxLabel = Label {draw_text +: {color: RINX_INK text_style: theme.font_regular{font_size: RINX_BODY_SIZE}}}
        mod.widgets.RinxPageTitle = mod.widgets.RinxLabel {draw_text.text_style: theme.font_bold{font_size: RINX_TITLE_SIZE}}
        mod.widgets.RinxHint = mod.widgets.RinxLabel {draw_text +: {color: RINX_MUTED text_style.font_size: RINX_META_SIZE}}
        mod.widgets.RinxHtml = Html {
            font_size: RINX_BODY_SIZE font_color: RINX_INK
            text_style_normal: theme.font_regular{font_size: RINX_BODY_SIZE}
            text_style_italic: theme.font_italic{font_size: RINX_BODY_SIZE}
            text_style_bold: theme.font_bold{font_size: RINX_BODY_SIZE}
            text_style_bold_italic: theme.font_bold_italic{font_size: RINX_BODY_SIZE}
        }
        mod.widgets.RinxRadioButton = RadioButton {
            draw_text +: {color: RINX_INK color_hover: RINX_INK color_active: RINX_INK color_focus: RINX_INK color_down: RINX_INK text_style: theme.font_regular{font_size: RINX_UI_SIZE}}
        }
        mod.widgets.RinxButton = ButtonFlat {
            height: RINX_BUTTON_HEIGHT
            padding: Inset{left: 12 * RINX_SPACING right: 12 * RINX_SPACING top: 0 bottom: 0}
            spacing: 6 margin: 0 align: Center
            icon_walk: Walk{width: 16 height: 16}
            grab_key_focus: false
            animator.hover.off.from.all.duration: RINX_MOTION
            animator.hover.on.from.all.duration: RINX_MOTION
            animator +: {disabled +: {
                off +: {apply +: {draw_icon: {opacity: 1.0}}}
                on +: {apply +: {draw_icon: {opacity: 0.4}}}
            }}
            draw_bg +: {
                color: RINX_SURFACE color_focus: RINX_SURFACE color_hover: RINX_HOVER color_down: RINX_PRESSED
                color_disabled: RINX_FIELD
                border_color: RINX_BORDER border_color_hover: RINX_BORDER border_color_down: RINX_BORDER
                border_color_focus: RINX_ACCENT border_color_disabled: RINX_BORDER
                border_size: 1 border_radius: RINX_CONTROL_RADIUS
                // One flat surface and one stroke; no inherited bevel/gradient states.
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    let line = max(self.border_size, self.focus * 1.5)
                    let inset = max(0.5, line * 0.5)
                    sdf.box(inset, inset, self.rect_size.x - inset * 2.0, self.rect_size.y - inset * 2.0, self.border_radius)
                    let fill = self.color.mix(self.color_focus, self.focus)
                        .mix(self.color_hover, self.hover).mix(self.color_down, self.down)
                        .mix(self.color_disabled, self.disabled)
                    let stroke = self.border_color.mix(self.border_color_hover, self.hover)
                        .mix(self.border_color_down, self.down).mix(self.border_color_focus, self.focus)
                        .mix(self.border_color_disabled, self.disabled)
                    sdf.fill_keep(fill)
                    if line > 0.0 {sdf.stroke(stroke, line)}
                    return sdf.result
                }
            }
            draw_text +: {color: RINX_INK color_hover: RINX_INK color_down: RINX_INK color_focus: RINX_INK color_disabled: RINX_DISABLED text_style: theme.font_regular{font_size: RINX_UI_SIZE}}
            draw_icon +: {color: RINX_INK}
        }
        mod.widgets.RinxPrimaryButton = mod.widgets.RinxButton {
            draw_bg +: {color: RINX_ACCENT color_focus: RINX_ACCENT color_hover: RINX_ACCENT_HOVER color_down: RINX_ACCENT_DOWN
                border_size: 0 border_color: RINX_ACCENT border_color_hover: RINX_ACCENT_HOVER border_color_down: RINX_ACCENT_DOWN border_color_focus: RINX_ON_ACCENT}
            draw_text +: {color: RINX_ON_ACCENT color_hover: RINX_ON_ACCENT color_down: RINX_ON_ACCENT color_focus: RINX_ON_ACCENT}
            draw_icon +: {color: RINX_ON_ACCENT}
        }
        mod.widgets.RinxGhostButton = mod.widgets.RinxButton {
            draw_bg +: {color: #0000 color_focus: #0000 color_hover: RINX_HOVER color_down: RINX_PRESSED border_size: 0}
        }
        mod.widgets.RinxIconButton = mod.widgets.RinxGhostButton {
            width: RINX_ICON_BUTTON_SIZE height: RINX_ICON_BUTTON_SIZE
            padding: 0 spacing: 0 text: ""
        }
        mod.widgets.RinxCompactButton = mod.widgets.RinxButton {
            height: RINX_ICON_BUTTON_SIZE
            padding: Inset{left: 10 right: 10 top: 0 bottom: 0}
        }
        mod.widgets.RinxCard = RoundedView {
            draw_bg +: {color: RINX_SURFACE border_color: RINX_BORDER border_size: 1 border_radius: RINX_CARD_RADIUS}
        }
        mod.widgets.RinxInput = TextInput {
            height: RINX_CONTROL_HEIGHT padding: Inset{left: 10 * RINX_SPACING right: 10 * RINX_SPACING top: 7 bottom: 7}
            draw_bg +: {color: RINX_FIELD color_hover: RINX_FIELD color_focus: RINX_FIELD color_empty: RINX_FIELD
                color_down: RINX_FIELD color_disabled: RINX_FIELD
                border_color: RINX_BORDER border_color_hover: RINX_BORDER border_color_down: RINX_ACCENT border_color_focus: RINX_ACCENT border_color_empty: RINX_BORDER border_color_disabled: RINX_BORDER border_size: 1 border_radius: RINX_CONTROL_RADIUS
                color_2: vec4(-1, -1, -1, -1) border_color_2: vec4(-1, -1, -1, -1)}
            draw_text +: {color: RINX_INK color_hover: RINX_INK color_focus: RINX_INK color_down: RINX_INK color_empty: RINX_MUTED color_disabled: RINX_DISABLED text_style: theme.font_regular{font_size: RINX_UI_SIZE}}
            draw_cursor.color: RINX_ACCENT
        }
        mod.widgets.RinxCheckBox = CheckBox {
            padding: Inset{left: 0 right: 0 top: 6 bottom: 6} align: Align{y: 0.5}
            label_walk +: {margin: Inset{left: 24}}
            draw_bg +: {size: 16 border_size: 1 border_radius: RINX_CONTROL_RADIUS
                color: RINX_FIELD color_hover: RINX_HOVER color_down: RINX_PRESSED color_focus: RINX_FIELD color_active: RINX_ACCENT color_disabled: RINX_FIELD
                border_color: RINX_BORDER border_color_hover: RINX_ACCENT border_color_focus: RINX_ACCENT border_color_down: RINX_ACCENT border_color_active: RINX_ACCENT border_color_disabled: RINX_BORDER
                mark_color: #0000 mark_color_hover: #0000 mark_color_down: #0000 mark_color_focus: #0000
                mark_color_active: RINX_ON_ACCENT mark_color_active_hover: RINX_ON_ACCENT
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    let center = vec2(self.size * 0.5, self.rect_size.y * 0.5)
                    let top = center.y - self.size * 0.5
                    sdf.box(self.border_size, top + self.border_size, self.size - self.border_size * 2.0, self.size - self.border_size * 2.0, self.border_radius * 0.5)
                    let fill = self.color.mix(self.color_hover, self.hover).mix(self.color_down, self.down)
                        .mix(self.color_active, self.active).mix(self.color_disabled, self.disabled)
                    let border = self.border_color.mix(self.border_color_hover, self.hover)
                        .mix(self.border_color_focus, self.focus).mix(self.border_color_active, self.active)
                        .mix(self.border_color_disabled, self.disabled)
                    sdf.fill_keep(fill)
                    sdf.stroke(border, self.border_size)
                    let inset = self.size * 0.275
                    sdf.move_to(inset, center.y)
                    sdf.line_to(center.x, center.y + self.size * 0.5 - inset)
                    sdf.line_to(self.size - inset, top + inset)
                    sdf.stroke(self.mark_color_active.mix(self.mark_color_disabled, self.disabled) * self.active, self.size * 0.09)
                    return sdf.result
                }}
            draw_text +: {color: RINX_INK color_hover: RINX_INK color_down: RINX_INK color_focus: RINX_INK color_active: RINX_INK color_disabled: RINX_DISABLED text_style: theme.font_regular{font_size: RINX_UI_SIZE}}
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn install(vm: &mut ScriptVm, selection: Selection) {
        desktop_style::install(vm, selection.stylesheet(DesktopStyle::Macos));
        vm.bx.captured_errors = Some(Vec::new());
        makepad_widgets::script_mod(vm);
        script_mod(vm);
        let errors = vm.take_errors();
        assert!(errors.is_empty(), "Theme registration failed: {errors:?}");
    }

    #[test]
    fn scrollbar_palette_reaches_preexisting_list_input_and_view_templates() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        for (index, appearance) in [Appearance::Light, Appearance::Dark, Appearance::Light].into_iter().enumerate() {
            cx.with_vm(|vm| {
                if index == 0 {install(vm, Selection {appearance, accent: Accent::Teal});}
                else {vm.with_reload(|vm| install(vm, Selection {appearance, accent: Accent::Teal}));}
                let snapshot = snapshot_for_vm(vm);
                let expected = mix(snapshot.page, snapshot.ink, 0.35);
                let values = [
                    script_eval!(vm, {mod.widgets.ScrollYView.scroll_bars.scroll_bar_y.draw_bg.color}),
                    script_eval!(vm, {mod.widgets.ScrollXView.scroll_bars.scroll_bar_x.draw_bg.color}),
                    script_eval!(vm, {mod.widgets.ScrollXYView.scroll_bars.scroll_bar_y.draw_bg.color}),
                    script_eval!(vm, {mod.widgets.PortalList.scroll_bar.draw_bg.color}),
                    script_eval!(vm, {mod.widgets.TextInput.scroll_bar.draw_bg.color}),
                ];
                for value in values {
                    let object = value.as_object().expect("scrollbar colors must retain shader uniform metadata");
                    assert!(vm.bx.heap.as_shader_io(object).is_some());
                    let value = vm.bx.heap.proto(object);
                    let color = Vec4f::script_from_value(vm, value);
                    assert!((color.x - expected.x).abs() < 0.005 && (color.y - expected.y).abs() < 0.005 && (color.z - expected.z).abs() < 0.005,
                        "the shared palette must reach widgets registered before the theme: {color:?} != {expected:?}");
                    assert_eq!(color.w, 1.0);
                }
                assert_eq!(script_eval!(vm, {mod.widgets.PortalList.scroll_bar.bar_size}).as_number(), Some(8.0));
                let size = script_eval!(vm, {mod.widgets.TextInput.scroll_bar.draw_bg.size});
                let object = size.as_object().expect("scrollbar geometry must retain shader uniform metadata");
                assert!(vm.bx.heap.as_shader_io(object).is_some());
                assert_eq!(vm.bx.heap.proto(object).as_number(), Some(5.0));
                let hover = script_eval!(vm, {mod.widgets.ScrollBarTabs.draw_bg.color_hover});
                let hover = vm.bx.heap.proto(hover.as_object().unwrap());
                let hover = Vec4f::script_from_value(vm, hover);
                let expected = mix(snapshot.page, snapshot.ink, 0.50);
                assert!((hover.x - expected.x).abs() < 0.005 && (hover.y - expected.y).abs() < 0.005 && (hover.z - expected.z).abs() < 0.005);
                assert!(vm.take_errors().is_empty());
            });
        }
    }

    #[test]
    fn shared_roles_resolve_with_readable_text_and_distinct_revisions() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut revisions = Vec::new();
        for appearance in [Appearance::Light, Appearance::Dark] {
            for accent in [Accent::Teal, Accent::Violet] {
                cx.with_vm(|vm| {
                    if revisions.is_empty() {
                        install(vm, Selection { appearance, accent });
                    } else {
                        vm.with_reload(|vm| install(vm, Selection { appearance, accent }));
                    }
                    let s = snapshot_for_vm(vm);
                    let contrast = |a, b| {
                        let (a, b) = (luminance(a), luminance(b));
                        (a.max(b) + 0.05) / (a.min(b) + 0.05)
                    };
                    for (ink, bg) in [
                        (s.ink, s.page),
                        (s.ink, s.surface),
                        (s.ink, s.field),
                        (s.muted, s.page),
                        (s.muted, s.surface),
                        (s.on_accent, s.accent),
                    ] {
                        assert!(
                            contrast(ink, bg) >= 4.5,
                            "{appearance:?}/{accent:?}: {ink:?} on {bg:?}: {}",
                            contrast(ink, bg)
                        );
                    }
                    let away = if luminance(s.on_accent) > 0.5 {
                        rgb(0)
                    } else {
                        rgb(0xffffff)
                    };
                    for amount in [0.08, 0.16] {
                        assert!(contrast(s.on_accent, mix(s.accent, away, amount)) >= 4.5);
                    }
                    assert!(!revisions.contains(&s.revision));
                    revisions.push(s.revision);
                });
            }
        }
    }

    #[test]
    fn hosted_ownership_never_uses_the_standalone_selection() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            install(
                vm,
                Selection {
                    appearance: Appearance::Dark,
                    accent: Accent::Violet,
                },
            );
            let before = desktop_style::current(vm);
            init_hosted(vm);
            init_standalone(vm);
            assert_eq!(desktop_style::current(vm), before);
            assert_eq!(selection_for_vm(vm), None);
        });
        assert!(select(&mut cx, Selection::default()).is_err());
    }

    #[test]
    fn stored_selection_rejects_unknown_and_malformed_values() {
        for json in [
            r#"{"appearance":"neon","accent":"teal"}"#,
            r#"{"appearance":"light","accent":"teal","source":"file"}"#,
            "invalid",
        ] {
            assert!(serde_json::from_str::<Selection>(json).is_err());
        }
        assert_eq!(
            serde_json::from_slice::<Selection>(
                &serde_json::to_vec(&Selection::default()).unwrap()
            )
            .unwrap(),
            Selection::default()
        );
    }
}
