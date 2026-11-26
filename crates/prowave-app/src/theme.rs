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
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------------------------
// The looks.

/// The looks the app can wear: Graphite Amber (the default: a charcoal body, amber signal, amber
/// LCDs) and Bone (the bone-white chassis with signal orange and a green LCD). Every token below
/// is a function that answers for the look in use, so a window drawn next frame wears the new
/// look at once. View > Theme picks it; it is kept in the settings file `theme`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Graphite,
    Bone,
}

impl Theme {
    /// The View > Theme entry of this look.
    pub fn label(self) -> &'static str {
        match self {
            Theme::Graphite => "Graphite Amber",
            Theme::Bone => "Bone",
        }
    }

    fn from_u8(v: u8) -> Self {
        if v == 1 {
            Theme::Bone
        } else {
            Theme::Graphite
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Theme::Graphite => 0,
            Theme::Bone => 1,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);
/// The look `apply` last gave egui's visuals (`u8::MAX`: none yet).
static APPLIED: AtomicU8 = AtomicU8::new(u8::MAX);
/// Walnut end cheeks down both sides of the main window (View > Wood Cheeks).
static WOOD: AtomicBool = AtomicBool::new(true);

thread_local! {
    /// A look for this thread alone (pictures and tests of one look, run beside others).
    static OVERRIDE: std::cell::Cell<Option<Theme>> = const { std::cell::Cell::new(None) };
}

/// The look in use.
pub fn current() -> Theme {
    OVERRIDE
        .with(|o| o.get())
        .unwrap_or_else(|| Theme::from_u8(CURRENT.load(Ordering::Relaxed)))
}

/// Switches the app to `theme` (the visuals follow on the next [`sync`]).
pub fn set(theme: Theme) {
    CURRENT.store(theme.as_u8(), Ordering::Relaxed);
}

/// Draws this thread in `theme` until it is set to `None` (pictures of one look).
pub fn set_for_thread(theme: Option<Theme>) {
    OVERRIDE.with(|o| o.set(theme));
}

/// Whether the walnut end cheeks are drawn.
pub fn wood() -> bool {
    WOOD.load(Ordering::Relaxed)
}

pub fn set_wood(on: bool) {
    WOOD.store(on, Ordering::Relaxed);
}

/// `bone` in the Bone look, `graphite` in Graphite Amber: for the few shades painters mix
/// themselves (gradients of a key, the chassis).
#[inline]
pub fn pick(bone: Color32, graphite: Color32) -> Color32 {
    match current() {
        Theme::Bone => bone,
        Theme::Graphite => graphite,
    }
}

/// Whether the look is a dark one (Graphite Amber).
pub fn is_dark() -> bool {
    current() == Theme::Graphite
}

/// A light highlight line under an edge (an emboss): white at `alpha` on bone; a faint one on
/// the dark body, where a bright line would read as an outline.
pub fn emboss(alpha: u8) -> Color32 {
    match current() {
        Theme::Bone => Color32::from_white_alpha(alpha),
        Theme::Graphite => Color32::from_white_alpha(alpha / 9),
    }
}

/// Re-applies the visuals when the look changed since they were last set (every frame).
pub fn sync(ctx: &egui::Context) {
    if APPLIED.load(Ordering::Relaxed) != current().as_u8() {
        apply(ctx);
    }
}

fn setting_file(name: &str) -> Option<std::path::PathBuf> {
    crate::recent_files::config_dir().map(|d| d.join(name))
}

/// The look and the wood cheeks as they were left (Graphite Amber with wood when never set).
pub fn load_settings() {
    if cfg!(test) {
        return;
    }
    let read = |n: &str| {
        setting_file(n)
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|t| t.trim().to_string())
    };
    set(match read("theme").as_deref() {
        Some("bone") => Theme::Bone,
        _ => Theme::Graphite,
    });
    set_wood(read("wood-cheeks").as_deref() != Some("off"));
}

/// Remembers the look and the wood cheeks for the next start.
fn save_settings() {
    // The tests of the menus must not change the settings of whoever runs them.
    if cfg!(test) {
        return;
    }
    let write = |n: &str, v: &str| {
        if let Some(f) = setting_file(n) {
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(f, v));
            }
        }
    };
    write(
        "theme",
        match current() {
            Theme::Graphite => "graphite",
            Theme::Bone => "bone",
        },
    );
    write("wood-cheeks", if wood() { "on" } else { "off" });
}

/// View > Theme's entries and View > Wood Cheeks.
pub const MENU_THEMES: &[&str] = &["Graphite Amber", "Bone"];
pub const MENU_WOOD: &str = "Wood Cheeks";

/// Whether `label` is one of the look's menu entries.
pub fn menu_handles(label: &str) -> bool {
    label == MENU_WOOD || MENU_THEMES.contains(&label)
}

/// The check mark of a look's menu entry (`None` for other entries).
pub fn menu_checked(label: &str) -> Option<bool> {
    if label == MENU_WOOD {
        return Some(wood());
    }
    MENU_THEMES
        .contains(&label)
        .then(|| current().label() == label)
}

/// Runs a look's menu entry; false for other entries.
pub fn menu_apply(label: &str) -> bool {
    match label {
        MENU_WOOD => set_wood(!wood()),
        "Graphite Amber" => set(Theme::Graphite),
        "Bone" => set(Theme::Bone),
        _ => return false,
    }
    save_settings();
    true
}

/// One look's colours.
struct Palette {
    bone: Color32,
    bone_hi: Color32,
    bone_top: Color32,
    bone2: Color32,
    bone3: Color32,
    seam: Color32,
    seam_dk: Color32,
    key_edge: Color32,
    key_lip: Color32,
    ink: Color32,
    ink2: Color32,
    print: Color32,
    orange: Color32,
    orange_hi: Color32,
    orange_dk: Color32,
    on_accent: Color32,
    lcd: Color32,
    lcd_ink: Color32,
    lcd_ghost: Color32,
    led_off: Color32,
    meter_well: Color32,
    cap_top: Color32,
    cap: Color32,
    cap_edge: Color32,
    on_dark: Color32,
    on_dark_dim: Color32,
    region: Color32,
    region_audio: Color32,
    region_print: Color32,
    region_audio_print: Color32,
    grid_bg: Color32,
    grid_sel: Color32,
    toolbar: Color32,
    panel: Color32,
    danger: Color32,
    warn: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

const BONE_LOOK: Palette = Palette {
    bone: rgb(0xe9, 0xe5, 0xdb),
    bone_hi: rgb(0xf4, 0xf1, 0xea),
    bone_top: rgb(0xfb, 0xf9, 0xf4),
    bone2: rgb(0xdc, 0xd7, 0xca),
    bone3: rgb(0xcf, 0xc8, 0xb8),
    seam: rgb(0xaf, 0xa7, 0x96),
    seam_dk: rgb(0x8f, 0x88, 0x7a),
    key_edge: rgb(0xa4, 0x9d, 0x8e),
    key_lip: rgb(0x9f, 0x98, 0x89),
    ink: rgb(0x14, 0x14, 0x13),
    ink2: rgb(0x2e, 0x2c, 0x29),
    print: rgb(0x6e, 0x69, 0x5f),
    orange: rgb(0xff, 0x5a, 0x14),
    orange_hi: rgb(0xff, 0x7a, 0x3d),
    orange_dk: rgb(0xb2, 0x3a, 0x06),
    on_accent: rgb(0x14, 0x14, 0x13),
    lcd: rgb(0xb2, 0xbc, 0x9f),
    lcd_ink: rgb(0x1a, 0x20, 0x13),
    lcd_ghost: Color32::from_rgba_premultiplied(1, 2, 1, 13),
    led_off: rgb(0x5d, 0x58, 0x4f),
    meter_well: rgb(0x1b, 0x1a, 0x18),
    cap_top: rgb(0x34, 0x32, 0x2f),
    cap: rgb(0x1d, 0x1c, 0x1a),
    cap_edge: rgb(0x0b, 0x0b, 0x0a),
    on_dark: rgb(0xe9, 0xe5, 0xdb),
    on_dark_dim: rgb(0x9a, 0x94, 0x88),
    region: rgb(0x1f, 0x1e, 0x1c),
    region_audio: rgb(0xa8, 0xa2, 0x95),
    region_print: rgb(0xe9, 0xe5, 0xdb),
    region_audio_print: rgb(0x2e, 0x2c, 0x29),
    grid_bg: rgb(0xd8, 0xd2, 0xc4),
    grid_sel: rgb(0xe2, 0xdd, 0xd1),
    toolbar: rgb(0xe4, 0xe0, 0xd5),
    panel: rgb(0xec, 0xe8, 0xdf),
    danger: rgb(0xd2, 0x2f, 0x0c),
    warn: rgb(0xb8, 0x7a, 0x00),
};

/// Graphite Amber: a charcoal body, amber signal, amber digits on dark LCD glass. Text tokens are
/// light here and surface tokens dark, so a text token still reads on any surface token.
const GRAPHITE_LOOK: Palette = Palette {
    bone: rgb(0x1c, 0x1c, 0x1e),
    bone_hi: rgb(0x32, 0x32, 0x37),
    bone_top: rgb(0x3c, 0x3c, 0x42),
    bone2: rgb(0x17, 0x17, 0x1a),
    bone3: rgb(0x18, 0x18, 0x1b),
    seam: rgb(0x3a, 0x3a, 0x40),
    seam_dk: rgb(0x58, 0x58, 0x60),
    key_edge: rgb(0x4a, 0x4a, 0x51),
    key_lip: rgb(0x0e, 0x0e, 0x10),
    ink: rgb(0xec, 0xeb, 0xe6),
    ink2: rgb(0xc9, 0xc7, 0xc0),
    print: rgb(0x8d, 0x8b, 0x85),
    orange: rgb(0xff, 0xb0, 0x00),
    orange_hi: rgb(0xff, 0xc4, 0x3d),
    orange_dk: rgb(0xc4, 0x87, 0x00),
    on_accent: rgb(0x17, 0x15, 0x0f),
    lcd: rgb(0x2b, 0x1f, 0x05),
    lcd_ink: rgb(0xff, 0xb0, 0x00),
    lcd_ghost: Color32::from_rgba_premultiplied(18, 12, 0, 18),
    led_off: rgb(0x45, 0x45, 0x4c),
    meter_well: rgb(0x0d, 0x0d, 0x0f),
    cap_top: rgb(0x3d, 0x3d, 0x43),
    cap: rgb(0x0f, 0x0f, 0x11),
    cap_edge: rgb(0x00, 0x00, 0x00),
    on_dark: rgb(0xec, 0xeb, 0xe6),
    on_dark_dim: rgb(0x8d, 0x8b, 0x85),
    region: rgb(0x3a, 0x3a, 0x40),
    region_audio: rgb(0x5b, 0x64, 0x70),
    region_print: rgb(0xff, 0xb0, 0x00),
    region_audio_print: rgb(0xec, 0xeb, 0xe6),
    grid_bg: rgb(0x18, 0x18, 0x1b),
    grid_sel: rgb(0x23, 0x23, 0x27),
    toolbar: rgb(0x23, 0x23, 0x26),
    panel: rgb(0x20, 0x20, 0x23),
    danger: rgb(0xff, 0x5a, 0x3c),
    warn: rgb(0xff, 0xb0, 0x00),
};

#[inline]
fn pal() -> &'static Palette {
    match current() {
        Theme::Bone => &BONE_LOOK,
        Theme::Graphite => &GRAPHITE_LOOK,
    }
}

macro_rules! tokens {
    ($($(#[$m:meta])* $name:ident => $field:ident;)*) => {
        $(
            $(#[$m])*
            #[allow(non_snake_case)]
            #[inline]
            pub fn $name() -> Color32 {
                pal().$field
            }
        )*
    };
}

// ---------------------------------------------------------------------------------------------
// Primitives: the materials (named for the Bone look; each answers for the look in use).

tokens! {
    /// The chassis.
    BONE => bone;
    /// Key faces and highlights.
    BONE_HI => bone_hi;
    /// The brightest key face (top of a key's gradient).
    BONE_TOP => bone_top;
    /// Recessed panels and empty slots.
    BONE2 => bone2;
    /// Deep wells: the arrange floor past the regions.
    BONE3 => bone3;
    /// Panel seams.
    SEAM => seam;
    SEAM_DK => seam_dk;
    /// A bone key's outline and lip.
    KEY_EDGE => key_edge;
    KEY_LIP => key_lip;
    /// Print on the chassis (black on bone, light on graphite).
    INK => ink;
    /// Secondary print.
    INK2 => ink2;
    /// Tertiary silkscreen (captions under keys, scales).
    PRINT => print;
    /// The one signal colour (orange on bone, amber on graphite).
    ORANGE => orange;
    ORANGE_HI => orange_hi;
    ORANGE_DK => orange_dk;
    /// Print on the signal colour (always dark).
    ON_ACCENT => on_accent;
    /// LCD glass and its segments.
    LCD => lcd;
    LCD_INK => lcd_ink;
    /// Unlit LCD segments.
    LCD_GHOST => lcd_ghost;
    /// An unlit LED.
    LED_OFF => led_off;
    /// Meter windows.
    METER_WELL => meter_well;
    /// Black cap faces (top and bottom of their gradient).
    CAP_TOP => cap_top;
    CAP => cap;
    CAP_EDGE => cap_edge;
    /// Text on black caps, regions and fields.
    ON_DARK => on_dark;
    /// Muted text on dark surfaces.
    ON_DARK_DIM => on_dark_dim;
    /// Region bodies (MIDI).
    REGION => region;
    /// Audio region bodies.
    REGION_AUDIO => region_audio;
    /// Notes and controller lines on a MIDI region.
    REGION_PRINT => region_print;
    /// The waveform on an audio region.
    REGION_AUDIO_PRINT => region_audio_print;

    // Semantic tokens (the names the panels use).

    /// Deepest well: empty slots, text fields, recessed lanes.
    WELL => bone2;
    /// Track lanes and grid backgrounds.
    GRID_BG => grid_bg;
    /// Lane of the selected track.
    GRID_SEL => grid_sel;
    /// Window background and side panels.
    BG => bone;
    /// Toolbars, rulers, mixer floor.
    TOOLBAR => toolbar;
    /// Panels, strips and track headers.
    PANEL => panel;
    /// Raised controls and selected headers/strips.
    RAISED => bone_hi;
    /// Hovered controls.
    HOVER => bone_top;
    /// Hairlines between surfaces.
    EDGE => seam;
    TEXT => ink;
    TEXT_MID => ink2;
    TEXT_DIM => print;
    /// Selection, active buttons, lit LEDs, the playhead.
    ACCENT => orange;
    /// Errors (a deeper red, distinct from the signal colour).
    DANGER => danger;
    /// Warnings.
    WARN => warn;

    // Names kept from the earlier looks so code written against them still builds.
    ROSE => orange;
    CHAMPAGNE => orange;
    MINT => orange;
    VIOLET => ink2;
    ORCHID => ink2;
    PERIWINKLE => orange;
    BUTTER => orange;
    CORAL => orange;
    TANGERINE => orange;
}

/// The level meters' segments, the same in every look: olive for most of the scale, amber
/// near the top (above -6 dB), signal orange at the top (over 0 dB); unlit ones dark.
pub const METER_LO: Color32 = Color32::from_rgb(0x7f, 0x8c, 0x6a);
pub const METER_MID: Color32 = Color32::from_rgb(0xc9, 0x8a, 0x2e);
pub const METER_HOT: Color32 = Color32::from_rgb(0xff, 0x5a, 0x14);
pub const METER_OFF: Color32 = Color32::from_rgb(0x2b, 0x29, 0x26);

/// Track and region colours: the default palette for new tracks. Hardware tones (signal orange,
/// black, graphite, warm grey, LCD olive, khaki, rust, slate) rather than a rainbow.
pub const TRACK_PALETTE: [Color32; 16] = [
    Color32::from_rgb(0xff, 0x5a, 0x14), // signal orange
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

/// The region material for a track of colour `track`: MIDI regions are black with bone notes on
/// Bone (graphite with amber notes on Graphite Amber), audio regions warm grey with a dark
/// waveform (slate with a light one); both take a hint of the track colour. Selected regions get
/// the signal header and outline; muted ones go flat with dim print.
pub fn region_look(track: Color32, audio: bool, selected: bool, muted: bool) -> RegionLook {
    let mix = |a: Color32, b: Color32, t: f32| crate::hw::lerp(a, b, t);
    if muted {
        return RegionLook {
            header: BONE3(),
            header_ink: PRINT(),
            body: BONE2(),
            print: SEAM_DK(),
            outline: if selected { ORANGE() } else { SEAM() },
        };
    }
    let header = if selected { ORANGE() } else { track };
    let body = if audio {
        mix(REGION_AUDIO(), track, 0.15)
    } else {
        mix(REGION(), track, 0.05)
    };
    RegionLook {
        header,
        header_ink: text_on(header),
        body,
        print: if audio {
            REGION_AUDIO_PRINT()
        } else {
            REGION_PRINT()
        },
        outline: if selected { ORANGE() } else { CAP_EDGE() },
    }
}

/// Whether text on `c` should be light (dark colour) or dark: the light print of dark materials
/// or the dark print of light ones, whatever the look.
pub fn text_on(c: Color32) -> Color32 {
    let [r, g, b, _] = c.to_array();
    let l = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    if l < 140.0 {
        ON_DARK()
    } else {
        ON_ACCENT()
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
    APPLIED.store(current().as_u8(), Ordering::Relaxed);
    let dark = is_dark();
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.dark_mode = dark;
    v.panel_fill = BG();
    v.window_fill = BONE();
    v.faint_bg_color = TOOLBAR();
    v.extreme_bg_color = pick(Color32::from_rgb(0xf8, 0xf6, 0xf0), BONE2());
    v.code_bg_color = BONE2();
    v.override_text_color = Some(TEXT());
    v.weak_text_color = Some(TEXT_DIM());
    v.hyperlink_color = pick(ORANGE_DK(), ORANGE());
    v.warn_fg_color = WARN();
    v.error_fg_color = DANGER();

    let r = CornerRadius::same(4);
    v.widgets.noninteractive.bg_fill = PANEL();
    v.widgets.noninteractive.weak_bg_fill = PANEL();
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, SEAM());
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, INK2());
    v.widgets.noninteractive.corner_radius = r;

    v.widgets.inactive.bg_fill = BONE2();
    v.widgets.inactive.weak_bg_fill = BONE_HI();
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, KEY_EDGE());
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, INK());
    v.widgets.inactive.corner_radius = r;

    v.widgets.hovered.bg_fill = pick(BONE3(), BONE_TOP());
    v.widgets.hovered.weak_bg_fill = BONE_TOP();
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, INK2());
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, INK());
    v.widgets.hovered.corner_radius = r;
    v.widgets.hovered.expansion = 0.0;

    v.widgets.active.bg_fill = ORANGE();
    v.widgets.active.weak_bg_fill = pick(Color32::from_rgb(0xe3, 0xde, 0xd3), CAP_TOP());
    v.widgets.active.bg_stroke = Stroke::new(1.0, INK());
    v.widgets.active.fg_stroke = Stroke::new(2.0, pick(INK(), ON_ACCENT()));
    v.widgets.active.corner_radius = r;
    v.widgets.active.expansion = 0.0;

    v.widgets.open.bg_fill = BONE2();
    v.widgets.open.weak_bg_fill = BONE2();
    v.widgets.open.bg_stroke = Stroke::new(1.0, INK2());
    v.widgets.open.fg_stroke = Stroke::new(1.0, INK());
    v.widgets.open.corner_radius = r;

    // Selected text and list rows: the signal colour behind dark print on Bone; on the dark
    // body a deep amber wash behind the light print, so selected text stays readable.
    v.selection.bg_fill = pick(ORANGE(), Color32::from_rgb(0x6b, 0x4a, 0x00));
    v.selection.stroke = Stroke::new(1.0, INK());
    v.text_cursor.stroke = Stroke::new(2.0, ORANGE());
    v.slider_trailing_fill = true;

    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(5);
    v.window_stroke = Stroke::new(1.0, SEAM_DK());
    v.window_shadow = shadow(0, 5, if dark { 0x70 } else { 0x38 });
    v.popup_shadow = shadow(0, 5, if dark { 0x70 } else { 0x38 });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_look_has_its_own_tokens_and_the_menu_follows_it() {
        set_for_thread(Some(Theme::Bone));
        assert_eq!(BG(), Color32::from_rgb(0xe9, 0xe5, 0xdb));
        assert_eq!(ACCENT(), Color32::from_rgb(0xff, 0x5a, 0x14));
        assert_eq!(menu_checked("Bone"), Some(true));
        set_for_thread(Some(Theme::Graphite));
        assert_eq!(BG(), Color32::from_rgb(0x1c, 0x1c, 0x1e));
        assert_eq!(ACCENT(), Color32::from_rgb(0xff, 0xb0, 0x00));
        assert_eq!(LCD(), Color32::from_rgb(0x2b, 0x1f, 0x05));
        assert_eq!(menu_checked("Graphite Amber"), Some(true));
        assert_eq!(menu_checked("Bone"), Some(false));
        assert_eq!(menu_checked("Show Mixer"), None);
        // Print reads on its surfaces in both looks: light on the dark body, dark on amber.
        assert_eq!(text_on(ACCENT()), ON_ACCENT());
        assert_eq!(text_on(BG()), ON_DARK());
        set_for_thread(None);
        // The meters' top segments do not follow the look.
        assert_eq!(METER_HOT, Color32::from_rgb(0xff, 0x5a, 0x14));
    }
}
