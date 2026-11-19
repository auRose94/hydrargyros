//! The Studio's look — a deliberate theme, not egui's defaults. Dark is
//! the default (as is the way of game engines), built on hyg's identity:
//! quicksilver — deep blue-gray bases, silver-blue accents, and the warm
//! gold of the selection highlight. A light theme rides along for bright
//! rooms; the sense of style grows from here (themes are data, not
//! decisions).

use egui::{
    Align2, Color32, Context, CornerRadius, FontId, Mesh, Painter, Rect, Sense, Shape, Stroke,
    StrokeKind, ThemePreference, Ui, Visuals,
};

// chrome colors — VIVID (the review: "bright and colorful even for
// neutral/no-interaction"): every button fill is saturated even at rest,
// the glass layers add the depth. Single source for the transport's
// color coding and any future accent widgets. The *_DIM names stay
// (call sites read them as "the not-engaged variant"); the VALUES no
// longer are.
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x3f, 0x8f, 0xd2);
pub const GOLD_DIM: Color32 = Color32::from_rgb(0xc9, 0x9a, 0x1e);
pub const GREEN_DIM: Color32 = Color32::from_rgb(0x2f, 0xa1, 0x4a);
pub const DANGER_DIM: Color32 = Color32::from_rgb(0xd6, 0x45, 0x4d);
// the transport's color language (user-specified): play green, stop
// red, restart orange — vivid at rest, radioactive when the halo joins
pub const PLAY_GREEN: Color32 = Color32::from_rgb(0x2f, 0xb1, 0x4a);
pub const STOP_RED: Color32 = Color32::from_rgb(0xe0, 0x41, 0x48);
pub const RESTART_ORANGE: Color32 = Color32::from_rgb(0xe8, 0x86, 0x2a);
// the create button's green
pub const CREATE_GREEN: Color32 = Color32::from_rgb(0x2f, 0xb1, 0x4a);

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

// ---- glass chrome ----
//
// The color buttons and tabs paint themselves as layered translucent
// shapes (the mid-2000s glass look): a tinted translucent body, a
// vertical brightness fade toward the top, a thin light border, and a
// specular highlight in the upper portion. egui has no blur, so the
// hover glow is faked with concentric rounded-rect strokes that grow
// and fade; pressed/toggled states pinch the same glow tighter and
// brighter so the state reads at a glance. Everything is 2D vector
// paint — no shaders, no textures, no new dependencies.

/// Which paint the chrome's color buttons use. [`ButtonStyle::Glass`]
/// is the layered translucent look; [`ButtonStyle::Flat`] is the plain
/// opaque fill (the same body a filled [`egui::Button`] draws). A View
/// menu toggle switches between them at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonStyle {
    /// Layered translucent glass (the default look).
    #[default]
    Glass,
    /// Flat opaque fill.
    Flat,
}

/// A button's interaction state at paint time, explicit rather than
/// read from the pointer — paint helpers stay pure, and headless tests
/// paint each state directly without simulating input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Idle,
    Hovered,
    /// Pressed, or engaged by selection/toggle (the playing transport
    /// button, the active gizmo mode, the open tab).
    Active,
    Disabled,
}

/// Corner radius of the chrome's buttons.
const BUTTON_RADIUS: u8 = 5;
/// Corner radius of the chrome's tabs (top corners only).
const TAB_RADIUS: u8 = 6;
/// The chrome's button/tab label size.
const LABEL_SIZE: f32 = 12.0;

/// Linear mix of two colors, alpha included (the glass builds its tints
/// by blending an accent into white/dark).
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}

/// The same color with its alpha replaced.
fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// The halo color for a button's glow. On the dark theme the accent
/// lifts only slightly toward white — the HUE is the signal (the review:
/// "colors pop and glow radioactive"), a washed-out halo reads pastel;
/// on the light theme white would vanish, so the accent glows as itself.
fn glow_tint(accent: Color32, dark: bool) -> Color32 {
    if dark {
        mix(accent, Color32::from_rgb(255, 255, 255), 0.18)
    } else {
        accent
    }
}

/// The glow as concentric outline layers, outside-in skipped: each
/// entry is `(outset, stroke width, alpha)`. Hover = a loose fading
/// halo; active = fewer, tighter, brighter — the halo "pinches" onto
/// the button so pressed/toggled reads at a glance. Alphas run HIGH
/// (the review: radioactive — the glow should be the loudest thing in
/// the chrome).
fn glow_layers(state: ButtonState) -> &'static [(f32, f32, u8)] {
    match state {
        ButtonState::Idle | ButtonState::Disabled => &[],
        ButtonState::Hovered => &[(2.0, 2.5, 130), (4.5, 2.0, 80), (7.5, 1.5, 45), (10.5, 1.0, 22)],
        ButtonState::Active => &[(1.5, 3.5, 200), (3.5, 2.5, 130), (6.0, 1.5, 70)],
    }
}

fn label_color(state: ButtonState, dark: bool) -> Color32 {
    match (state, dark) {
        (ButtonState::Disabled, true) => with_alpha(dark::FAINT, 150),
        (ButtonState::Disabled, false) => with_alpha(light::FAINT, 150),
        (ButtonState::Idle, true) => dark::TEXT,
        (ButtonState::Idle, false) => light::TEXT,
        // engaged/hovered labels lift toward white (dark) or black
        // (light) so they stay readable over the brighter glass
        (_, true) => mix(dark::TEXT, Color32::from_rgb(255, 255, 255), 0.6),
        (_, false) => mix(light::TEXT, Color32::from_rgb(0, 0, 0), 0.4),
    }
}

/// Paint the glow outlines around `rect` (a rounded rect per layer,
/// corner radius growing with the outset to keep the halo smooth).
fn paint_glow(painter: &Painter, rect: Rect, accent: Color32, state: ButtonState, dark: bool) {
    let tint = glow_tint(accent, dark);
    for (outset, width, alpha) in glow_layers(state) {
        painter.rect_stroke(
            rect.expand(*outset),
            CornerRadius::same(BUTTON_RADIUS + *outset as u8),
            Stroke::new(*width, with_alpha(tint, *alpha)),
            StrokeKind::Middle,
        );
    }
}

/// A vertical fade as one tiny mesh: four vertices, two triangles,
/// vertex colors carrying the top/bottom tint (egui has no gradient
/// rect fill, and one 4-vertex mesh is cheaper than stacked bands
/// while keeping a smooth fade). Inset by the corner radius so the
/// rounded body silhouette stays visible at the edges.
fn paint_vertical_fade(
    painter: &Painter,
    rect: Rect,
    radius: u8,
    top: Color32,
    bottom: Color32,
) {
    let r = f32::from(radius);
    let inner = Rect::from_min_max(
        egui::pos2(rect.left() + r, rect.top() + r),
        egui::pos2(rect.right() - r, rect.bottom() - r),
    );
    let mut mesh = Mesh::default();
    mesh.colored_vertex(egui::pos2(inner.left(), inner.top()), top);
    mesh.colored_vertex(egui::pos2(inner.right(), inner.top()), top);
    mesh.colored_vertex(egui::pos2(inner.right(), inner.bottom()), bottom);
    mesh.colored_vertex(egui::pos2(inner.left(), inner.bottom()), bottom);
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(Shape::mesh(mesh));
}

/// The glass body's layers for a state: `(body alpha, fade top alpha,
/// fade bottom alpha, border alpha, specular alpha)`. Hover brightens
/// the glass, active more — the surface answers the pointer before the
/// glow even starts. Alphas run high and the body stays close to the
/// accent (the review: the colors should POP — a desaturated body reads
/// gray under a glowing halo).
fn glass_layers(state: ButtonState, dark: bool) -> (u8, u8, u8, u8, u8) {
    if dark {
        match state {
            ButtonState::Idle => (140, 90, 70, 80, 45),
            ButtonState::Hovered => (170, 120, 85, 115, 65),
            ButtonState::Active => (205, 150, 105, 150, 85),
            ButtonState::Disabled => (60, 30, 25, 28, 14),
        }
    } else {
        match state {
            ButtonState::Idle => (165, 185, 110, 90, 125),
            ButtonState::Hovered => (190, 210, 130, 115, 145),
            ButtonState::Active => (215, 230, 150, 140, 165),
            ButtonState::Disabled => (110, 110, 60, 45, 70),
        }
    }
}

/// Paint a glass button inside `rect` and return the label's rect (the
/// "text sits inside the glass" assertion at geometry level). `accent`
/// drives the body tint, the border hue and the glow — the semantic
/// color of the button (green play, red stop, …).
pub fn paint_glass_button(
    painter: &Painter,
    rect: Rect,
    label: &str,
    accent: Color32,
    state: ButtonState,
    dark: bool,
) -> Rect {
    paint_glow(painter, rect, accent, state, dark);
    paint_drop_shadow(painter, rect);
    let (body_a, top_a, bottom_a, border_a, spec_a) = glass_layers(state, dark);
    let body = mix(accent, if dark { dark::WINDOW } else { light::WINDOW }, 0.12);
    painter.rect_filled(rect, CornerRadius::same(BUTTON_RADIUS), with_alpha(body, body_a));
    let fade_top = mix(
        Color32::from_rgb(255, 255, 255),
        accent,
        if dark { 0.25 } else { 0.1 },
    );
    let fade_bottom = mix(accent, if dark { dark::BASE } else { light::BASE }, 0.25);
    paint_vertical_fade(
        painter,
        rect,
        BUTTON_RADIUS,
        with_alpha(fade_top, top_a),
        with_alpha(fade_bottom, bottom_a),
    );
    let border = if dark {
        Color32::from_rgb(255, 255, 255)
    } else {
        mix(accent, Color32::from_rgb(70, 80, 100), 0.5)
    };
    painter.rect_stroke(
        rect,
        CornerRadius::same(BUTTON_RADIUS),
        Stroke::new(1.0, with_alpha(border, border_a)),
        StrokeKind::Inside,
    );
    // the specular: a soft white window in the top portion — the light
    // "catching" the glass
    let spec = Rect::from_min_max(
        egui::pos2(rect.left() + 6.0, rect.top() + 2.0),
        egui::pos2(rect.right() - 6.0, rect.top() + rect.height() * 0.42),
    );
    painter.rect_filled(spec, CornerRadius::same(3), with_alpha(Color32::WHITE, spec_a));
    paint_label(painter, rect, label, state, dark)
}

// ---- the rainbow beam ----
//
// A ribbon of rainbow color along the very top of the window, beaming
// light down into the chrome. While a session is processing (sim
// playing, or the AI seam busy — see `ShellShared::processing`) the
// hues ROTATE and random "hot spots" brighten along the length at
// random intensities; at rest the ribbon holds a calm, still rainbow.

/// A segment's hue, degrees. The rainbow spreads across the window and
/// the whole wheel rotates with `phase` (which the shell advances only
/// while processing) — slowly: the review wants the colors to walk,
/// not spin.
#[must_use]
pub fn beam_hue(segment: f32, segments: f32, phase: f32) -> f32 {
    (segment / segments * 360.0 + phase * 8.0) % 360.0
}

/// The per-segment drift wave: continuous in segment AND phase — the
/// old per-segment hash wrapped its fractional term and popped between
/// frames (the review: "shaky, doesn't linearly interpolate"); two slow
/// sine waves glide instead.
#[must_use]
fn beam_wobble(segment: f32, phase: f32) -> f32 {
    0.5 + 0.5 * (segment * 0.55 + phase * 0.9).sin()
}

/// A segment's sparkle: a deterministic 0..=1 hot-spot intensity — the
/// product of two drift waves, so most segments stay calm while a few
/// glide hot, all interpolated smoothly over time. Idle (not
/// processing) collapses to zero.
#[must_use]
pub fn beam_sparkle(segment: f32, _segments: f32, phase: f32, processing: bool) -> f32 {
    if !processing {
        return 0.0;
    }
    let w = beam_wobble(segment, phase);
    let v = beam_wobble(segment * 1.7 + 11.3, phase * 0.83);
    let s = w * v; // the product: sparse hot spots
    s * s // sharpen further: a few blaze, the rest rest
}

/// One segment's color: fully saturated HSV. Processing lifts the
/// value with the sparkle; at rest the ribbon sits at a calm mid value.
#[must_use]
pub fn beam_color(segment: f32, segments: f32, phase: f32, processing: bool) -> Color32 {
    let hue = beam_hue(segment, segments, phase);
    let value = if processing {
        0.75 + 0.25 * beam_sparkle(segment, segments, phase, processing)
    } else {
        0.5
    };
    hsv_to_color(hue, 1.0, value)
}

/// HSV → RGB (hue degrees, s/v 0..=1) — the beam's tiny color math,
/// kept local so the ribbon needs no dependency.
#[must_use]
fn hsv_to_color(hue: f32, s: f32, v: f32) -> Color32 {
    let c = v * s;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match hue as u32 % 360 / 60 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Color32::from_rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

/// The beam's height: a 2px rainbow line plus the light it beams down.
/// Short — the review cut the length by 65%: the light hugs the edge
/// instead of washing the menu bar.
pub const BEAM_HEIGHT: f32 = 8.0;

/// Paint the beam across `rect` (the window's top strip). One mesh: the
/// top vertex row carries the segment colors (the ribbon line), the
/// bottom row the same hues fading to transparent — the light shining
/// down. SUBTLE at rest (low alpha, mid value — a quiet rainbow),
/// OBVIOUS while processing (full alpha, brightened hot spots).
pub fn paint_beam(painter: &Painter, rect: Rect, phase: f32, processing: bool) {
    let segments = 64.0f32;
    let (top_alpha, fade_alpha) = if processing { (255, 42) } else { (95, 12) };
    let mut mesh = Mesh::default();
    let step = rect.width() / segments;
    let x0 = rect.left();
    for i in 0..=(segments as u32) {
        let seg = i as f32;
        let color = beam_color(seg, segments, phase, processing);
        let x = x0 + seg * step;
        mesh.colored_vertex(
            egui::pos2(x, rect.top()),
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), top_alpha),
        );
        mesh.colored_vertex(
            egui::pos2(x, rect.top() + BEAM_HEIGHT),
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), fade_alpha),
        );
    }
    let top = (0..=(segments as u32)).map(|i| i * 2).collect::<Vec<_>>();
    for w in top.windows(2) {
        mesh.indices.extend_from_slice(&[w[0], w[0] + 1, w[1] + 1, w[0], w[1] + 1, w[1]]);
    }
    painter.add(Shape::mesh(mesh));
}

/// A soft dark drop shadow under a button body (two layered offsets —
/// egui has no blur; layered alpha fakes the softness). The review: the
/// menu bar needs shadows to fight the beam's light for contrast.
fn paint_drop_shadow(painter: &Painter, rect: Rect) {
    let wide = Rect::from_min_size(
        egui::pos2(rect.left(), rect.top() + 2.5),
        rect.size(),
    );
    painter.rect_filled(
        wide,
        CornerRadius::same(BUTTON_RADIUS + 1),
        Color32::from_black_alpha(40),
    );
    let tight = Rect::from_min_size(
        egui::pos2(rect.left(), rect.top() + 1.5),
        rect.size(),
    );
    painter.rect_filled(
        tight,
        CornerRadius::same(BUTTON_RADIUS),
        Color32::from_black_alpha(75),
    );
}

/// Paint the flat alternative: an opaque semantic fill with a thin
/// border and the same glow treatment (the halo is what both styles
/// share). The body a filled [`egui::Button`] draws, kept.
pub fn paint_flat_button(
    painter: &Painter,
    rect: Rect,
    label: &str,
    accent: Color32,
    state: ButtonState,
    dark: bool,
) -> Rect {
    paint_glow(painter, rect, accent, state, dark);
    paint_drop_shadow(painter, rect);
    let fill = if state == ButtonState::Disabled {
        accent.gamma_multiply(0.5)
    } else {
        accent
    };
    painter.rect_filled(rect, CornerRadius::same(BUTTON_RADIUS), fill);
    let border_a = match state {
        ButtonState::Idle => if dark { 40 } else { 50 },
        ButtonState::Hovered => 70,
        ButtonState::Active => 100,
        ButtonState::Disabled => 25,
    };
    let border = if dark {
        Color32::from_rgb(255, 255, 255)
    } else {
        Color32::from_rgb(30, 35, 45)
    };
    painter.rect_stroke(
        rect,
        CornerRadius::same(BUTTON_RADIUS),
        Stroke::new(1.0, with_alpha(border, border_a)),
        StrokeKind::Inside,
    );
    // flat keeps one label color on both themes: the fill is a
    // mid-tone semantic color, so light-on-color reads everywhere
    let text = match state {
        ButtonState::Disabled => mix(dark::TEXT, dark::WINDOW, 0.55),
        _ => dark::TEXT,
    };
    paint_label_text(painter, rect, label, text)
}

/// A chrome tab: rounded-top glass when engaged, a fainter strip when
/// idle. The same paint language as the buttons, at lower contrast —
/// tabs stay background furniture.
pub fn paint_tab(
    painter: &Painter,
    rect: Rect,
    label: &str,
    accent: Color32,
    state: ButtonState,
    dark: bool,
) -> Rect {
    let radius = CornerRadius {
        nw: TAB_RADIUS,
        ne: TAB_RADIUS,
        sw: 0,
        se: 0,
    };
    let (body_a, fade_a, border_a) = match state {
        ButtonState::Active => (if dark { 85 } else { 160 }, if dark { 60 } else { 110 }, 80),
        ButtonState::Hovered => (if dark { 45 } else { 120 }, if dark { 30 } else { 75 }, 50),
        _ => (if dark { 22 } else { 90 }, 0, 30),
    };
    let body = mix(accent, if dark { dark::WINDOW } else { light::WINDOW }, 0.12);
    painter.rect_filled(rect, radius, with_alpha(body, body_a));
    if fade_a > 0 {
        let top = if dark { Color32::WHITE } else { mix(Color32::WHITE, accent, 0.15) };
        paint_vertical_fade(painter, rect, TAB_RADIUS, with_alpha(top, fade_a), Color32::TRANSPARENT);
    }
    let border = if dark {
        Color32::from_rgb(255, 255, 255)
    } else {
        mix(accent, Color32::from_rgb(70, 80, 100), 0.5)
    };
    painter.rect_stroke(rect, radius, Stroke::new(1.0, with_alpha(border, border_a)), StrokeKind::Inside);
    paint_label(painter, rect, label, state, dark)
}

/// Center the label inside the glass (state-aware color).
fn paint_label(painter: &Painter, rect: Rect, label: &str, state: ButtonState, dark: bool) -> Rect {
    paint_label_text(painter, rect, label, label_color(state, dark))
}

fn paint_label_text(painter: &Painter, rect: Rect, label: &str, color: Color32) -> Rect {
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(LABEL_SIZE),
        color,
    )
}

/// Resolve a live [`egui::Response`] into a paint state.
fn response_state(response: &egui::Response, selected: bool, enabled: bool) -> ButtonState {
    if !enabled {
        ButtonState::Disabled
    } else if selected || response.is_pointer_button_down_on() {
        ButtonState::Active
    } else if response.hovered() {
        ButtonState::Hovered
    } else {
        ButtonState::Idle
    }
}

/// One chrome color button: allocate the space, paint the style's
/// body under the pointer/selection state, and hand back the response
/// (call sites keep `.on_hover_text(...).clicked()`). `accent` is the
/// semantic color — the fill in [`ButtonStyle::Flat`], the glass tint
/// and glow hue in [`ButtonStyle::Glass`]. `selected` engages the
/// active glow (toggled-on buttons).
pub fn color_button(
    ui: &mut Ui,
    label: &str,
    accent: Color32,
    selected: bool,
    enabled: bool,
    dark: bool,
    style: ButtonStyle,
) -> egui::Response {
    let font = FontId::proportional(LABEL_SIZE);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, Color32::WHITE);
    let size = egui::vec2(galley.size().x + 16.0, (galley.size().y + 7.0).max(20.0));
    // the disabled gate scopes the allocation: the response inherits
    // enabled=false (clicked() stays false) and the painter fades
    ui.add_enabled_ui(enabled, |ui| {
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let state = response_state(&response, selected, enabled);
        // the glow strokes live outside the rect — paint with a clip
        // wide enough to hold them
        let painter = ui.painter_at(rect.expand(8.0));
        match style {
            ButtonStyle::Glass => {
                paint_glass_button(&painter, rect, label, accent, state, dark);
            }
            ButtonStyle::Flat => {
                paint_flat_button(&painter, rect, label, accent, state, dark);
            }
        }
        response
    })
    .inner
}

/// One chrome tab (the scene tabs, the bottom dock's row). Glass when
/// [`ButtonStyle::Glass`] (active = the open tab), the plain
/// selectable label when [`ButtonStyle::Flat`].
pub fn tab_button(
    ui: &mut Ui,
    label: &str,
    selected: bool,
    dark: bool,
    style: ButtonStyle,
) -> egui::Response {
    if style == ButtonStyle::Flat {
        return ui.selectable_label(selected, label);
    }
    let accent = if dark { dark::ACCENT } else { light::ACCENT };
    let font = FontId::proportional(LABEL_SIZE);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, Color32::WHITE);
    let size = egui::vec2(galley.size().x + 18.0, (galley.size().y + 6.0).max(22.0));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let state = if selected {
        ButtonState::Active
    } else if response.hovered() {
        ButtonState::Hovered
    } else {
        ButtonState::Idle
    };
    let painter = ui.painter_at(rect.expand(3.0));
    paint_tab(&painter, rect, label, accent, state, dark);
    response
}
