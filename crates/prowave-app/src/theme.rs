//! The Hardware look (PW-1): a bone-white chassis with seams and screws, bone and black key caps
//! with LED dots, one signal colour (orange), a green LCD with seven-segment digits, dark
//! segmented meters, and silkscreened condensed caps for labels. Every window takes its colours
//! and type from here; the painters for the controls are in `hw.rs`.
//!
//! Tokens come in two layers: the primitives (the materials: bone, ink, orange, LCD glass) and
//! the semantic names the panels use (`BG`, `PANEL`, `TEXT`, `ACCENT`…). Text tokens are dark
//! and surface tokens light, so any text token reads on any surface token; the dark materials
//! (caps, regions, meter wells, LCD) have their own text colours (`ON_DARK`, `LCD_INK`).
// The whole token set, used or not yet: windows pick from it.
#![allow(dead_code)]
use egui::{epaint::Shadow, Color32, CornerRadius, FontFamily, FontId, Stroke};
use std::sync::Arc;

// ---------------------------------------------------------------------------------------------
// Primitives: the materials.

/// The chassis.
pub const BONE: Color32 = Color32::from_rgb(0xe9, 0xe5, 0xdb);
/// Key faces and highlights.
pub const BONE_HI: Color32 = Color32::from_rgb(0xf4, 0xf1, 0xea);
/// The brightest key face (top of a key's gradient).
pub const BONE_TOP: Color32 = Color32::from_rgb(0xfb, 0xf9, 0xf4);
/// Recessed panels and empty slots.
pub const BONE2: Color32 = Color32::from_rgb(0xdc, 0xd7, 0xca);
/// Deep wells: the arrange floor past the regions.
pub const BONE3: Color32 = Color32::from_rgb(0xcf, 0xc8, 0xb8);
/// Panel seams.
pub const SEAM: Color32 = Color32::from_rgb(0xaf, 0xa7, 0x96);
pub const SEAM_DK: Color32 = Color32::from_rgb(0x8f, 0x88, 0x7a);
/// A bone key's outline and lip.
pub const KEY_EDGE: Color32 = Color32::from_rgb(0xa4, 0x9d, 0x8e);
pub const KEY_LIP: Color32 = Color32::from_rgb(0x9f, 0x98, 0x89);
/// Black print and black caps.
pub const INK: Color32 = Color32::from_rgb(0x14, 0x14, 0x13);
/// Secondary print.
pub const INK2: Color32 = Color32::from_rgb(0x2e, 0x2c, 0x29);
/// Tertiary silkscreen (captions under keys, scales).
pub const PRINT: Color32 = Color32::from_rgb(0x6e, 0x69, 0x5f);
/// The one signal colour.
pub const ORANGE: Color32 = Color32::from_rgb(0xff, 0x5a, 0x14);
pub const ORANGE_HI: Color32 = Color32::from_rgb(0xff, 0x7a, 0x3d);
pub const ORANGE_DK: Color32 = Color32::from_rgb(0xb2, 0x3a, 0x06);
/// LCD glass and its segments.
pub const LCD: Color32 = Color32::from_rgb(0xb2, 0xbc, 0x9f);
pub const LCD_INK: Color32 = Color32::from_rgb(0x1a, 0x20, 0x13);
/// Unlit LCD segments.
pub const LCD_GHOST: Color32 = Color32::from_rgba_premultiplied(1, 2, 1, 13);
/// An unlit LED.
pub const LED_OFF: Color32 = Color32::from_rgb(0x5d, 0x58, 0x4f);
/// Meter windows.
pub const METER_WELL: Color32 = Color32::from_rgb(0x1b, 0x1a, 0x18);
/// Black cap faces (top and bottom of their gradient).
pub const CAP_TOP: Color32 = Color32::from_rgb(0x34, 0x32, 0x2f);
pub const CAP: Color32 = Color32::from_rgb(0x1d, 0x1c, 0x1a);
pub const CAP_EDGE: Color32 = Color32::from_rgb(0x0b, 0x0b, 0x0a);
/// Text on black caps, regions and fields.
pub const ON_DARK: Color32 = Color32::from_rgb(0xe9, 0xe5, 0xdb);
/// Muted text on dark surfaces.
pub const ON_DARK_DIM: Color32 = Color32::from_rgb(0x9a, 0x94, 0x88);
/// Region bodies (MIDI) and their print.
pub const REGION: Color32 = Color32::from_rgb(0x1f, 0x1e, 0x1c);
/// Audio region bodies: warm grey, the waveform printed dark on it.
pub const REGION_AUDIO: Color32 = Color32::from_rgb(0xa8, 0xa2, 0x95);

// ---------------------------------------------------------------------------------------------
// Semantic tokens (the names the panels use).

/// Deepest well: empty slots, text fields, recessed lanes.
pub const WELL: Color32 = BONE2;
/// Track lanes and grid backgrounds.
pub const GRID_BG: Color32 = Color32::from_rgb(0xd8, 0xd2, 0xc4);
/// Lane of the selected track.
pub const GRID_SEL: Color32 = Color32::from_rgb(0xe2, 0xdd, 0xd1);
/// Window background and side panels.
pub const BG: Color32 = BONE;
/// Toolbars, rulers, mixer floor.
pub const TOOLBAR: Color32 = Color32::from_rgb(0xe4, 0xe0, 0xd5);
/// Panels, strips and track headers.
pub const PANEL: Color32 = Color32::from_rgb(0xec, 0xe8, 0xdf);
/// Raised controls and selected headers/strips.
pub const RAISED: Color32 = BONE_HI;
/// Hovered controls.
pub const HOVER: Color32 = BONE_TOP;
/// Hairlines between surfaces.
pub const EDGE: Color32 = SEAM;

pub const TEXT: Color32 = INK;
pub const TEXT_MID: Color32 = INK2;
pub const TEXT_DIM: Color32 = PRINT;

/// Selection, active buttons, lit LEDs, the playhead.
pub const ACCENT: Color32 = ORANGE;
/// Errors (a deeper orange-red, distinct from the signal orange).
pub const DANGER: Color32 = Color32::from_rgb(0xd2, 0x2f, 0x0c);
/// Warnings.
pub const WARN: Color32 = Color32::from_rgb(0xb8, 0x7a, 0x00);

// Names kept from the earlier looks so code written against them still builds; each is one of
// the Hardware tokens now.
pub const ROSE: Color32 = ORANGE;
pub const CHAMPAGNE: Color32 = ORANGE;
pub const MINT: Color32 = ORANGE;
pub const VIOLET: Color32 = INK2;
pub const ORCHID: Color32 = INK2;
pub const PERIWINKLE: Color32 = ORANGE;
pub const BUTTER: Color32 = ORANGE;
pub const CORAL: Color32 = ORANGE;
pub const TANGERINE: Color32 = ORANGE;

/// Track and region colours: the default palette for new tracks. Hardware tones (signal orange,
/// black, graphite, warm grey, LCD olive, khaki, rust, slate) rather than a rainbow.
pub const TRACK_PALETTE: [Color32; 16] = [
    ORANGE,
    Color32::from_rgb(0x2b, 0x29, 0x26), // black cap
    Color32::from_rgb(0x8f, 0x88, 0x7a), // warm grey
    Color32::from_rgb(0x7f, 0x8c, 0x6a), // LCD olive
    Color32::from_rgb(0xc9, 0x8a, 0x2e), // amber
    Color32::from_rgb(0x5c, 0x6b, 0x78), // slate
    Color32::from_rgb(0xa2, 0x4b, 0x2a), // rust
    Color32::from_rgb(0xb7, 0xa8, 0x82), // khaki
    Color32::from_rgb(0xff, 0x8a, 0x52), // light orange
    Color32::from_rgb(0x4a, 0x47, 0x41), // graphite
    Color32::from_rgb(0xc2, 0xbb, 0xab), // stone
    Color32::from_rgb(0x56, 0x63, 0x45), // moss
    Color32::from_rgb(0xd9, 0xb3, 0x4c), // mustard
    Color32::from_rgb(0x3b, 0x4a, 0x5a), // navy grey
    Color32::from_rgb(0x8a, 0x3a, 0x22), // brick
    Color32::from_rgb(0x9e, 0x93, 0x78), // sand
];

/// How a region is drawn: a body in the region material and a header bar in the track's colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionLook {
    /// The header bar (the track colour; orange when selected).
    pub header: Color32,
    /// Text on the header bar.
    pub header_ink: Color32,
    /// The body.
    pub body: Color32,
    /// Notes, waveform and controller lines on the body.
    pub print: Color32,
    /// The outline (orange when selected).
    pub outline: Color32,
}

/// The region material for a track of colour `track`: MIDI regions are black with bone notes,
/// audio regions warm grey with a dark waveform; both take a hint of the track colour. Selected
/// regions get the signal-orange header and outline; muted ones go flat bone with grey print.
pub fn region_look(track: Color32, audio: bool, selected: bool, muted: bool) -> RegionLook {
    let mix = |a: Color32, b: Color32, t: f32| crate::hw::lerp(a, b, t);
    if muted {
        return RegionLook {
            header: BONE3,
            header_ink: PRINT,
            body: BONE2,
            print: SEAM_DK,
            outline: if selected { ORANGE } else { SEAM },
        };
    }
    let header = if selected { ORANGE } else { track };
    let body = if audio {
        mix(REGION_AUDIO, track, 0.15)
    } else {
        mix(REGION, track, 0.05)
    };
    RegionLook {
        header,
        header_ink: text_on(header),
        body,
        print: if audio { INK2 } else { ON_DARK },
        outline: if selected { ORANGE } else { CAP_EDGE },
    }
}

/// Whether text on `c` should be light (dark colour) or dark.
pub fn text_on(c: Color32) -> Color32 {
    let [r, g, b, _] = c.to_array();
    let l = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    if l < 140.0 {
        ON_DARK
    } else {
        INK
    }
}

// ---------------------------------------------------------------------------------------------
// Type.

/// Barlow Condensed Bold: the silkscreened labels (caps, spaced out).
pub const SILK: &str = "pw-silk";
/// Barlow Condensed ExtraBold: big silkscreen (the wordmark, section heads).
pub const SILK_X: &str = "pw-silk-x";
/// JetBrains Mono Medium: body text.
pub const MONO_M: &str = "pw-mono-m";
/// JetBrains Mono Bold: key legends, menus, names.
pub const MONO_B: &str = "pw-mono-b";
/// JetBrains Mono ExtraBold: LCD text.
pub const MONO_X: &str = "pw-mono-x";

const FONT_FILES: [(&str, &[u8]); 5] = [
    (
        SILK,
        include_bytes!("../assets/fonts/BarlowCondensed-Bold.ttf"),
    ),
    (
        SILK_X,
        include_bytes!("../assets/fonts/BarlowCondensed-ExtraBold.ttf"),
    ),
    (
        MONO_M,
        include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf"),
    ),
    (
        MONO_B,
        include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    ),
    (
        MONO_X,
        include_bytes!("../assets/fonts/JetBrainsMono-ExtraBold.ttf"),
    ),
];

/// The roles text plays in the Hardware look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    /// Silkscreen caps (labels, captions).
    Silk,
    /// Big silkscreen (wordmark, headings).
    SilkX,
    /// Body text.
    Body,
    /// Key legends, menus, names.
    Bold,
    /// LCD text.
    Lcd,
}

/// The font of `face` at `size` points.
pub fn font(face: Face, size: f32) -> FontId {
    let name = match face {
        Face::Silk => SILK,
        Face::SilkX => SILK_X,
        Face::Body => MONO_M,
        Face::Bold => MONO_B,
        Face::Lcd => MONO_X,
    };
    FontId::new(size, FontFamily::Name(name.into()))
}

/// Letter spacing (in em) that goes with `face`, as the silkscreen is set.
pub fn tracking(face: Face) -> f32 {
    match face {
        Face::Silk => 0.15,
        Face::SilkX => 0.28,
        Face::Body => 0.0,
        Face::Bold => 0.05,
        Face::Lcd => 0.04,
    }
}

/// Puts the Hardware fonts into `ctx` (once): the named faces above, JetBrains Mono as the
/// proportional and monospace faces, with egui's own fonts behind them for missing glyphs.
pub fn install_fonts(ctx: &egui::Context) {
    // A flag in the context's memory, not `ctx.fonts`: the fonts can't be read before the
    // first frame, and the app installs them while it is being created.
    let flag = egui::Id::unique("prowave-hw-fonts-installed");
    if ctx.data(|d| d.get_temp::<bool>(flag)).unwrap_or(false) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(flag, true));
    let mut defs = egui::FontDefinitions::default();
    let fallback = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for (name, bytes) in FONT_FILES {
        defs.font_data.insert(
            name.to_string(),
            Arc::new(egui::FontData::from_static(bytes)),
        );
        let mut fam = vec![name.to_string()];
        fam.extend(fallback.iter().cloned());
        defs.families.insert(FontFamily::Name(name.into()), fam);
    }
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        let list = defs.families.entry(fam).or_default();
        list.insert(0, MONO_M.to_string());
    }
    ctx.set_fonts(defs);
}

/// The spec's shadow under floating windows and menus: (0, 3), blur 5, #00000038.
fn shadow(spread: u8, blur: u8, alpha: u8) -> Shadow {
    Shadow {
        offset: [0, 3],
        blur,
        spread,
        color: Color32::from_rgba_premultiplied(0, 0, 0, alpha),
    }
}

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    let mut v = egui::Visuals::light();
    v.dark_mode = false;
    v.panel_fill = BG;
    v.window_fill = BONE;
    v.faint_bg_color = TOOLBAR;
    v.extreme_bg_color = Color32::from_rgb(0xf8, 0xf6, 0xf0);
    v.code_bg_color = BONE2;
    v.override_text_color = Some(TEXT);
    v.weak_text_color = Some(TEXT_DIM);
    v.hyperlink_color = ORANGE_DK;
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;

    let r = CornerRadius::same(4);
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.weak_bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, SEAM);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, INK2);
    v.widgets.noninteractive.corner_radius = r;

    v.widgets.inactive.bg_fill = BONE2;
    v.widgets.inactive.weak_bg_fill = BONE_HI;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, KEY_EDGE);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, INK);
    v.widgets.inactive.corner_radius = r;

    v.widgets.hovered.bg_fill = BONE3;
    v.widgets.hovered.weak_bg_fill = BONE_TOP;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, INK2);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, INK);
    v.widgets.hovered.corner_radius = r;
    v.widgets.hovered.expansion = 0.0;

    v.widgets.active.bg_fill = ORANGE;
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0xe3, 0xde, 0xd3);
    v.widgets.active.bg_stroke = Stroke::new(1.0, INK);
    v.widgets.active.fg_stroke = Stroke::new(2.0, INK);
    v.widgets.active.corner_radius = r;
    v.widgets.active.expansion = 0.0;

    v.widgets.open.bg_fill = BONE2;
    v.widgets.open.weak_bg_fill = BONE2;
    v.widgets.open.bg_stroke = Stroke::new(1.0, INK2);
    v.widgets.open.fg_stroke = Stroke::new(1.0, INK);
    v.widgets.open.corner_radius = r;

    v.selection.bg_fill = ORANGE;
    v.selection.stroke = Stroke::new(1.0, INK);
    v.text_cursor.stroke = Stroke::new(2.0, ORANGE);
    v.slider_trailing_fill = true;

    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(5);
    v.window_stroke = Stroke::new(1.0, SEAM_DK);
    v.window_shadow = shadow(0, 5, 0x38);
    v.popup_shadow = shadow(0, 5, 0x38);
    v.window_highlight_topmost = false;

    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(7.0, 3.0);
        s.spacing.interact_size.y = 20.0;
        s.animation_time = 0.12;
        let body = |size| FontId::new(size, FontFamily::Name(MONO_M.into()));
        s.text_styles.insert(egui::TextStyle::Body, body(12.0));
        s.text_styles.insert(
            egui::TextStyle::Button,
            FontId::new(11.5, FontFamily::Name(MONO_B.into())),
        );
        s.text_styles.insert(egui::TextStyle::Small, body(10.0));
        s.text_styles.insert(egui::TextStyle::Monospace, body(12.0));
        s.text_styles.insert(
            egui::TextStyle::Heading,
            FontId::new(17.0, FontFamily::Name(SILK_X.into())),
        );
    });
}

/// Kept from the earlier look (a lacquer sheen); the Hardware look has a matte finish, so it
/// draws only the bright hairline under a key's top edge.
pub fn sheen(g: &egui::Painter, rect: egui::Rect, radius: f32) {
    let y = rect.top() + 1.0;
    g.line_segment(
        [
            egui::pos2(rect.left() + radius, y),
            egui::pos2(rect.right() - radius, y),
        ],
        Stroke::new(1.0, Color32::from_white_alpha(70)),
    );
}

/// A soft glow around `rect` in `color` (lit LEDs, the recording LCD).
pub fn glow(g: &egui::Painter, rect: egui::Rect, radius: f32, color: Color32) {
    for (grow, alpha) in [(5.0, 0.07), (3.0, 0.12), (1.5, 0.18)] {
        g.rect_filled(
            rect.expand(grow),
            radius + grow,
            color.gamma_multiply(alpha),
        );
    }
}
