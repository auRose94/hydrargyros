//! The Studio's look — a deliberate theme, not egui's defaults. Dark is
//! the default (as is the way of game engines), built on hyg's identity:
//! quicksilver — deep blue-gray bases, silver-blue accents, and the warm
//! gold of the selection highlight. A light theme rides along for bright
//! rooms; the sense of style grows from here (themes are data, not
//! decisions).

use egui::{Color32, Context, ThemePreference, Visuals};

/// The dark palette: quicksilver on deep space.
pub mod dark {
    use egui::Color32;
    pub const BASE: Color32 = Color32::from_rgb(0x14, 0x16, 0x1b);
    pub const PANEL: Color32 = Color32::from_rgb(0x1a, 0x1d, 0x24);
    pub const WINDOW: Color32 = Color32::from_rgb(0x1e, 0x22, 0x2a);
    pub const TEXT: Color32 = Color32::from_rgb(0xd8, 0xdc, 0xe4);
    pub const FAINT: Color32 = Color32::from_rgb(0x8a, 0x92, 0xa3);
    pub const ACCENT: Color32 = Color32::from_rgb(0x7f, 0xb4, 0xe8);
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x2a, 0x3a, 0x52);
    pub const GOLD: Color32 = Color32::from_rgb(0xff, 0xd1, 0x66);
    pub const DANGER: Color32 = Color32::from_rgb(0xe8, 0x6a, 0x6a);
}

/// The light palette: paper for bright rooms.
pub mod light {
    use egui::Color32;
    pub const BASE: Color32 = Color32::from_rgb(0xf2, 0xf3, 0xf5);
    pub const PANEL: Color32 = Color32::from_rgb(0xe9, 0xec, 0xef);
    pub const WINDOW: Color32 = Color32::from_rgb(0xee, 0xf1, 0xf4);
    pub const TEXT: Color32 = Color32::from_rgb(0x26, 0x2a, 0x33);
    pub const FAINT: Color32 = Color32::from_rgb(0x6b, 0x74, 0x84);
    pub const ACCENT: Color32 = Color32::from_rgb(0x2b, 0x6c, 0xb0);
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(0xd4, 0xe3, 0xf5);
    pub const GOLD: Color32 = Color32::from_rgb(0xb8, 0x86, 0x0b);
    pub const DANGER: Color32 = Color32::from_rgb(0xb0, 0x30, 0x30);
}

/// Apply the Studio theme to the context. `dark` is the default; the
/// toggle re-calls this. 0.37's styles are theme-keyed — both palettes
/// are installed, the preference picks which shows.
pub fn apply(ctx: &Context, dark: bool) {
    ctx.set_theme(if dark {
        ThemePreference::Dark
    } else {
        ThemePreference::Light
    });
    // kill drag-to-select on ALL labels globally — the studio's rows and
    // readouts are controls, not documents (hovering the UI must never
    // paint selections)
    ctx.all_styles_mut(|style| {
        style.interaction.selectable_labels = false;
    });
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.visuals = palette(dark::BASE, dark::PANEL, dark::WINDOW, dark::TEXT, dark::FAINT, dark::ACCENT, dark::ACCENT_DIM, true);
    });
    ctx.style_mut_of(egui::Theme::Light, |style| {
        style.visuals = palette(light::BASE, light::PANEL, light::WINDOW, light::TEXT, light::FAINT, light::ACCENT, light::ACCENT_DIM, false);
    });
}

/// The Studio chrome over egui's own dark/light base: fills, strokes,
/// rounding, selection.
#[allow(clippy::too_many_arguments)]
fn palette(
    base: Color32,
    panel: Color32,
    window: Color32,
    text: Color32,
    faint: Color32,
    accent: Color32,
    accent_dim: Color32,
    dark: bool,
) -> Visuals {
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.override_text_color = Some(text);
    v.faint_bg_color = faint.gamma_multiply(0.15);
    v.extreme_bg_color = base;
    v.panel_fill = panel;
    v.window_fill = window;
    v.selection.bg_fill = accent.gamma_multiply(0.35);
    v.selection.stroke = egui::Stroke::new(1.2, accent);
    v.widgets.noninteractive.bg_fill = panel;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(0.8, faint.gamma_multiply(0.4));
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, faint);
    v.widgets.noninteractive.corner_radius = egui::CornerRadius::same(4);
    let weak = accent_dim.gamma_multiply(if dark { 0.35 } else { 0.6 });
    for state in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
    ] {
        state.weak_bg_fill = weak;
        state.bg_fill = weak;
        state.fg_stroke = egui::Stroke::new(1.0, text);
        state.corner_radius = egui::CornerRadius::same(4);
    }
    v
}
