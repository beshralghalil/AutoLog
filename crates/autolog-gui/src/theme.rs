//! The look of the window, in one place.
//!
//! Colours are named for what they mean rather than what they are, and each
//! has a light and a dark value, so every widget asks for `palette.danger`
//! and gets something legible either way.

use autolog_core::EntryStatus;
use autolog_core::config::Theme;
use egui::{Color32, CornerRadius, Stroke, Visuals};

/// Semantic colours for one appearance.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Behind everything.
    pub bg: Color32,
    /// Panels and cards sitting on the background.
    pub surface: Color32,
    /// Table headers and striped rows.
    pub surface_alt: Color32,
    /// Hairlines between regions.
    pub border: Color32,
    pub text: Color32,
    /// Secondary text: hints, units, counts.
    pub text_muted: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    /// Text drawn on top of the accent.
    pub on_accent: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub info: Color32,
}

const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x0f, 0x11, 0x15),
    surface: Color32::from_rgb(0x17, 0x1a, 0x21),
    surface_alt: Color32::from_rgb(0x1d, 0x21, 0x2a),
    border: Color32::from_rgb(0x2a, 0x2f, 0x3a),
    text: Color32::from_rgb(0xe4, 0xe7, 0xec),
    text_muted: Color32::from_rgb(0x9a, 0xa3, 0xb2),
    accent: Color32::from_rgb(0x5b, 0x8d, 0xef),
    accent_hover: Color32::from_rgb(0x7a, 0xa3, 0xf2),
    on_accent: Color32::from_rgb(0x0b, 0x10, 0x20),
    success: Color32::from_rgb(0x4a, 0xc4, 0x6a),
    warning: Color32::from_rgb(0xe0, 0xa3, 0x2e),
    danger: Color32::from_rgb(0xf8, 0x61, 0x59),
    info: Color32::from_rgb(0x58, 0xa6, 0xff),
};

const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xf4, 0xf6, 0xf9),
    surface: Color32::from_rgb(0xff, 0xff, 0xff),
    surface_alt: Color32::from_rgb(0xef, 0xf2, 0xf6),
    border: Color32::from_rgb(0xd8, 0xde, 0xe6),
    text: Color32::from_rgb(0x1b, 0x1f, 0x27),
    text_muted: Color32::from_rgb(0x5b, 0x64, 0x72),
    accent: Color32::from_rgb(0x3b, 0x6f, 0xd4),
    accent_hover: Color32::from_rgb(0x2f, 0x5c, 0xb5),
    on_accent: Color32::from_rgb(0xff, 0xff, 0xff),
    success: Color32::from_rgb(0x1a, 0x7f, 0x37),
    warning: Color32::from_rgb(0x9a, 0x66, 0x00),
    danger: Color32::from_rgb(0xcf, 0x22, 0x2e),
    info: Color32::from_rgb(0x09, 0x69, 0xda),
};

impl Palette {
    /// The colour a status is drawn in.
    pub fn status(&self, status: EntryStatus) -> Color32 {
        match status {
            EntryStatus::Pending => self.text_muted,
            EntryStatus::Queued => self.info,
            EntryStatus::Submitting => self.accent,
            EntryStatus::Success => self.success,
            EntryStatus::Skipped | EntryStatus::Invalid => self.warning,
            EntryStatus::Failed => self.danger,
        }
    }

    /// The same colour, faded to sit behind text as a chip.
    pub fn tint(color: Color32) -> Color32 {
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 38)
    }
}

/// Which appearance to paint, resolving `System` against what the desktop says.
pub fn resolve(theme: Theme, ctx: &egui::Context) -> egui::Theme {
    match theme {
        Theme::Light => egui::Theme::Light,
        Theme::Dark => egui::Theme::Dark,
        Theme::System => ctx.system_theme().unwrap_or(egui::Theme::Dark),
    }
}

pub fn palette(theme: egui::Theme) -> Palette {
    match theme {
        egui::Theme::Light => LIGHT,
        egui::Theme::Dark => DARK,
    }
}

/// Apply the palette and the spacing the whole window is laid out on.
pub fn apply(ctx: &egui::Context, theme: egui::Theme) {
    let palette = palette(theme);
    let mut visuals = match theme {
        egui::Theme::Light => Visuals::light(),
        egui::Theme::Dark => Visuals::dark(),
    };

    let radius = CornerRadius::same(6);

    visuals.panel_fill = palette.bg;
    visuals.window_fill = palette.surface;
    visuals.extreme_bg_color = palette.bg;
    visuals.faint_bg_color = palette.surface_alt;
    visuals.text_edit_bg_color = Some(palette.surface);
    visuals.override_text_color = Some(palette.text);
    visuals.hyperlink_color = palette.accent;
    visuals.warn_fg_color = palette.warning;
    visuals.error_fg_color = palette.danger;
    visuals.window_stroke = Stroke::new(1.0, palette.border);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = radius;
    visuals.selection.bg_fill = Palette::tint(palette.accent);
    visuals.selection.stroke = Stroke::new(1.0, palette.accent);

    visuals.widgets.noninteractive.bg_fill = palette.surface;
    visuals.widgets.noninteractive.weak_bg_fill = palette.surface;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text_muted);
    visuals.widgets.noninteractive.corner_radius = radius;

    visuals.widgets.inactive.bg_fill = palette.surface_alt;
    visuals.widgets.inactive.weak_bg_fill = palette.surface_alt;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, palette.text);
    visuals.widgets.inactive.corner_radius = radius;

    visuals.widgets.hovered.bg_fill = Palette::tint(palette.accent);
    visuals.widgets.hovered.weak_bg_fill = Palette::tint(palette.accent);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.accent_hover);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, palette.text);
    visuals.widgets.hovered.corner_radius = radius;

    visuals.widgets.active.bg_fill = palette.accent;
    visuals.widgets.active.weak_bg_fill = palette.accent;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, palette.accent);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, palette.on_accent);
    visuals.widgets.active.corner_radius = radius;

    visuals.widgets.open.bg_fill = palette.surface_alt;
    visuals.widgets.open.weak_bg_fill = palette.surface_alt;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, palette.accent);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, palette.text);
    visuals.widgets.open.corner_radius = radius;

    ctx.set_visuals_of(theme, visuals);

    ctx.style_mut_of(theme, |style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.combo_height = 320.0;
        style.spacing.menu_width = 260.0;
        style.spacing.tooltip_width = 420.0;
        style.spacing.scroll.bar_width = 10.0;
        style.visuals.indent_has_left_vline = false;
        style.wrap_mode = Some(egui::TextWrapMode::Truncate);
    });
}

/// Height of one row in the entries table.
pub const ROW_HEIGHT: f32 = 30.0;
/// Height of the table's header row.
pub const HEADER_HEIGHT: f32 = 30.0;
