//! Control bar (Logic Pro: "Control Bar") in the Hardware look: the VIEW keys, the TRANSPORT
//! keys, the LCD (SMPTE and bar position, cycle locators, tempo and signature, status and CPU),
//! the MONITOR group (Record/Mix latency chip, Low Latency, Count-in, Click), the MAIN LEVEL
//! ladder and slider, and the WINDOW keys (List Editors, Note Pad, Loop Browser, Browsers).
//!
//! Every control is painted with `hw` (key caps, LEDs, LCD glass, seven-segment digits), and
//! every icon is drawn here with vector shapes on a 24-unit grid, so it renders the same on
//! Windows, Linux and macOS.
use crate::file_browser::Tab;
use crate::hw::{self, Cap, Legend};

/// Parts of the bar the shared kit has no painter for (also used by the menus and chooser).
#[path = "hw_transport.rs"]
pub(crate) mod hw_transport;
use crate::i18n::{tr, trf};
use crate::model::{Project, SigPart};
use crate::theme::{self, Face};
use crate::{Bottom, UiState};
use egui::{pos2, vec2, Align2, Color32, Id, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui};

/// The control bar's height in the Hardware layout: bracket labels over 54-point keys, their
/// silkscreen legends under them, and the full LCD. A lower bar gets the compact layout (no
/// brackets or legends, a two-row LCD).
pub const HEIGHT: f32 = 120.0;

/// Below this height the bar uses the compact layout.
const FULL_MIN_HEIGHT: f32 = 96.0;

/// The LCD's widest bezel (the mockup's 776-point glass and its bezel).
const LCD_MAX: f32 = 794.0;
/// Narrower than this, the main level gives its room to the LCD (the Tracks area's master row
/// still has one).
const LCD_WANTS: f32 = 340.0;

// ---------------------------------------------------------------- icons

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Icon {
    Info,
    Help,
    Toolbar,
    Mixer,
    Scissors,
    Appearance,
    Rewind,
    Forward,
    ToStart,
    Play,
    Record,
    Capture,
    Cycle,
    Shield,
    Metronome,
    List,
    NotePad,
    Loops,
    Windows,
}

/// Points of an arc around (`cx`, `cy`) on the 24-unit grid, from `a0` to `a1` degrees
/// (0 = right, increasing clockwise as the y axis points down).
fn arc(cx: f32, cy: f32, r: f32, a0: f32, a1: f32, steps: usize) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|i| {
            let a = (a0 + (a1 - a0) * i as f32 / steps as f32).to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// Draws `ic` `size` points square around `c` in `color`: the mockup's 24-unit icons with
/// 2-unit strokes.
fn icon(g: &Painter, ic: Icon, c: Pos2, size: f32, color: Color32) {
    let u = size / 24.0;
    let p = |x: f32, y: f32| pos2(c.x + (x - 12.0) * u, c.y + (y - 12.0) * u);
    let pts = |v: &[(f32, f32)]| v.iter().map(|&(x, y)| p(x, y)).collect::<Vec<_>>();
    let st = Stroke::new((2.0 * u).max(1.3), color);
    let line = |v: &[(f32, f32)]| {
        g.add(Shape::line(pts(v), st));
    };
    let closed = |v: &[(f32, f32)]| {
        g.add(Shape::closed_line(pts(v), st));
    };
    let fill = |v: &[(f32, f32)]| {
        g.add(Shape::convex_polygon(pts(v), color, Stroke::NONE));
    };
    let rect = |x: f32, y: f32, w: f32, h: f32| {
        g.rect_filled(Rect::from_min_max(p(x, y), p(x + w, y + h)), 0.0, color);
    };
    let ring = |x: f32, y: f32, r: f32| {
        g.circle_stroke(p(x, y), r * u, st);
    };
    match ic {
        Icon::Info => {
            ring(12.0, 12.0, 8.5);
            line(&[(12.0, 10.5), (12.0, 17.0)]);
            rect(11.0, 6.6, 2.0, 2.0);
        }
        Icon::Help => {
            ring(12.0, 12.0, 8.5);
            let mut q = arc(12.1, 9.5, 2.7, 178.0, 420.0, 10);
            q.extend([(12.0, 13.2), (12.0, 14.6)]);
            line(&q);
            rect(11.0, 15.8, 2.0, 2.0);
        }
        Icon::Toolbar => {
            closed(&[(3.5, 5.0), (20.5, 5.0), (20.5, 19.0), (3.5, 19.0)]);
            line(&[(3.5, 9.5), (20.5, 9.5)]);
            line(&[(9.0, 9.5), (9.0, 19.0)]);
        }
        Icon::Mixer => {
            for x in [6.0, 12.0, 18.0] {
                line(&[(x, 4.0), (x, 20.0)]);
            }
            rect(3.5, 12.0, 5.0, 3.5);
            rect(9.5, 6.5, 5.0, 3.5);
            rect(15.5, 14.0, 5.0, 3.5);
        }
        Icon::Scissors => {
            ring(7.0, 17.0, 3.0);
            ring(17.0, 17.0, 3.0);
            line(&[(9.0, 14.6), (17.5, 3.5)]);
            line(&[(15.0, 14.6), (6.5, 3.5)]);
        }
        Icon::Appearance => {
            ring(12.0, 12.0, 8.5);
            fill(&arc(12.0, 12.0, 8.5, 90.0, 270.0, 16));
        }
        Icon::Rewind => {
            fill(&[(11.5, 6.0), (11.5, 18.0), (3.0, 12.0)]);
            fill(&[(21.0, 6.0), (21.0, 18.0), (12.5, 12.0)]);
        }
        Icon::Forward => {
            fill(&[(12.5, 6.0), (21.0, 12.0), (12.5, 18.0)]);
            fill(&[(3.0, 6.0), (11.5, 12.0), (3.0, 18.0)]);
        }
        Icon::ToStart => {
            rect(4.0, 6.0, 2.6, 12.0);
            fill(&[(20.0, 6.0), (20.0, 18.0), (8.0, 12.0)]);
        }
        Icon::Play => fill(&[(7.0, 4.5), (20.0, 12.0), (7.0, 19.5)]),
        Icon::Record => {
            g.circle_filled(p(12.0, 12.0), 6.5 * u, color);
        }
        Icon::Capture => {
            g.circle_stroke(p(12.0, 12.0), 8.0 * u, Stroke::new(2.4 * u, color));
            g.circle_filled(p(12.0, 12.0), 4.2 * u, color);
        }
        Icon::Cycle => {
            let mut top = vec![(4.0, 13.0)];
            top.extend(arc(6.5, 9.5, 2.5, 180.0, 270.0, 4));
            top.push((19.0, 7.0));
            line(&top);
            line(&[(16.0, 4.0), (19.0, 7.0), (16.0, 10.0)]);
            let mut bottom = vec![(20.0, 11.0)];
            bottom.extend(arc(17.5, 14.5, 2.5, 0.0, 90.0, 4));
            bottom.push((5.0, 17.0));
            line(&bottom);
            line(&[(8.0, 20.0), (5.0, 17.0), (8.0, 14.0)]);
        }
        Icon::Shield => {
            closed(&[
                (12.0, 3.2),
                (19.0, 6.0),
                (19.0, 11.2),
                (18.2, 14.6),
                (16.0, 17.8),
                (12.0, 20.8),
                (8.0, 17.8),
                (5.8, 14.6),
                (5.0, 11.2),
                (5.0, 6.0),
            ]);
            line(&[(8.8, 11.8), (11.1, 14.1), (15.3, 9.5)]);
        }
        Icon::Metronome => {
            closed(&[(8.5, 20.5), (10.4, 4.0), (13.6, 4.0), (15.5, 20.5)]);
            line(&[(12.0, 15.5), (18.0, 7.0)]);
            line(&[(5.5, 20.5), (18.5, 20.5)]);
        }
        Icon::List => {
            for y in [6.5, 12.0, 17.5] {
                line(&[(9.0, y), (20.0, y)]);
                rect(3.5, y - 1.1, 2.2, 2.2);
            }
        }
        Icon::NotePad => {
            closed(&[
                (5.0, 3.5),
                (15.0, 3.5),
                (19.0, 7.5),
                (19.0, 20.5),
                (5.0, 20.5),
            ]);
            line(&[(8.5, 11.0), (15.5, 11.0)]);
            line(&[(8.5, 14.5), (15.5, 14.5)]);
            line(&[(8.5, 18.0), (12.5, 18.0)]);
        }
        Icon::Loops => {
            // A figure eight (lemniscate) 19 units wide and 9 tall.
            let v: Vec<(f32, f32)> = (0..48)
                .map(|i| {
                    let t = std::f32::consts::TAU * i as f32 / 48.0;
                    let d = 1.0 + t.sin() * t.sin();
                    (
                        12.0 + 9.0 * t.cos() / d,
                        12.0 + 12.6 * t.sin() * t.cos() / d,
                    )
                })
                .collect();
            closed(&v);
        }
        Icon::Windows => {
            closed(&[(3.5, 8.0), (14.5, 8.0), (14.5, 19.0), (3.5, 19.0)]);
            line(&[
                (8.0, 8.0),
                (8.0, 4.5),
                (20.5, 4.5),
                (20.5, 16.0),
                (14.5, 16.0),
            ]);
        }
    }
}

// ---------------------------------------------------------------- layout

/// Where things go up and down the bar.
#[derive(Clone, Copy, Debug)]
struct Rows {
    /// Brackets over the groups (the full-height bar).
    full: bool,
    key_top: f32,
    key_h: f32,
    bracket_y: f32,
    legend_y: f32,
    /// Size of the silkscreen under the keys and of the bracket labels.
    legend_size: f32,
    bracket_size: f32,
    /// The LCD bezel's top and bottom.
    lcd_top: f32,
    lcd_bottom: f32,
}

impl Rows {
    fn of(bar: Rect) -> Self {
        let h = bar.height();
        if h >= FULL_MIN_HEIGHT {
            let key_h = (h - 66.0).clamp(40.0, 60.0);
            let key_top = bar.top() + 27.0 + ((h - HEIGHT) * 0.5).max(0.0);
            Self {
                full: true,
                key_top,
                key_h,
                bracket_y: key_top - 14.0,
                legend_y: key_top + key_h + 12.0,
                legend_size: (key_h * 0.19).clamp(9.0, 11.5),
                bracket_size: (key_h * 0.2).clamp(10.0, 12.5),
                lcd_top: bar.top() + 6.0,
                lcd_bottom: bar.bottom() - 9.0,
            }
        } else {
            // Compact: shorter keys with their one-line silkscreen still under them.
            let key_h = (h - 25.0).max(20.0);
            let key_top = bar.top() + 5.0;
            Self {
                full: false,
                key_top,
                key_h,
                bracket_y: bar.top(),
                legend_y: key_top + key_h + 9.0,
                legend_size: 9.0,
                bracket_size: 10.0,
                lcd_top: bar.top() + 3.0,
                lcd_bottom: bar.bottom() - 4.0,
            }
        }
    }
}

/// Where the groups go across the bar (x from the bar's left edge). Key sizes run from the
/// tight layout of a 1280-point window through a MacBook Air's 1470 to the mockup's at 2400
/// points; the LCD takes the room that is left, up to [`LCD_MAX`], and any room past that
/// widens the gaps between groups. The margins keep the groups clear of the corner screws.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    gap: f32,
    margin: f32,
    view_x: f32,
    view_n: usize,
    view_key: f32,
    view_pitch: f32,
    /// Width of the separator between the first three view keys and the rest.
    view_sep: f32,
    tr_x: f32,
    tr_key: f32,
    tr_gap: f32,
    play_w: f32,
    cycle_w: f32,
    lcd_x: f32,
    lcd_w: f32,
    mon_x: f32,
    chip_w: f32,
    mon_key: f32,
    mon_pitch: f32,
    /// The main level's x and width, when there is room for it.
    level: Option<(f32, f32)>,
    win_x: f32,
    win_key: f32,
    win_gap: f32,
}

impl Layout {
    fn of(width: f32, view_n: usize) -> Self {
        // 0 at 1470 points, 1 at 2400; below 1470 the keys keep shrinking (to -0.4 at 1230).
        let t = if width >= 1470.0 {
            ((width - 1470.0) / (2400.0 - 1470.0)).min(1.0)
        } else {
            ((width - 1470.0) / 600.0).max(-0.4)
        };
        let l = |a: f32, b: f32| a + (b - a) * t;
        let margin = l(20.0, 30.0).max(20.0);
        let mut gap = l(12.0, 26.0).max(10.0);
        let (view_key, vg, view_sep) = (l(36.0, 52.0), l(6.0, 8.0), l(12.0, 22.0));
        let view_w = view_n as f32 * view_key + (view_n.max(2) - 2) as f32 * vg + view_sep;
        let (tr_key, tr_gap, play_w, cycle_w) =
            (l(40.0, 62.0), l(5.0, 8.0), l(58.0, 96.0), l(64.0, 106.0));
        let tr_w = 5.0 * tr_key + play_w + cycle_w + 6.0 * tr_gap;
        let (chip_w, mon_key) = (l(70.0, 118.0).max(64.0), view_key);
        // The monitor keys' legends are the longest: their keys sit further apart.
        let mon_pitch = mon_key + l(14.0, 8.0);
        let mon_w = chip_w + 12.0 + 2.0 * mon_pitch + mon_key;
        let level_w = l(100.0, 190.0).max(88.0);
        let (win_key, win_gap) = (l(28.0, 36.0), l(5.0, 8.0));
        let win_w = 2.0 * win_key + win_gap;
        let fixed = 2.0 * margin + view_w + tr_w + mon_w + win_w;
        let mut lcd_w = width - fixed - level_w - 5.0 * gap;
        let mut level = Some(level_w);
        if lcd_w < LCD_WANTS {
            level = None;
            lcd_w = width - fixed - 4.0 * gap;
        }
        if lcd_w > LCD_MAX {
            let gaps = if level.is_some() { 5.0 } else { 4.0 };
            gap += (lcd_w - LCD_MAX) / gaps;
            lcd_w = LCD_MAX;
        }
        let lcd_w = lcd_w.max(220.0);
        let view_x = margin;
        let tr_x = view_x + view_w + gap;
        let lcd_x = tr_x + tr_w + gap;
        let mon_x = lcd_x + lcd_w + gap;
        let mut x = mon_x + mon_w + gap;
        let level = level.map(|w| {
            let at = x;
            x += w + gap;
            (at, w)
        });
        Self {
            gap,
            margin,
            view_x,
            view_n,
            view_key,
            view_pitch: view_key + vg,
            view_sep,
            tr_x,
            tr_key,
            tr_gap,
            play_w,
            cycle_w,
            lcd_x,
            lcd_w,
            mon_x,
            chip_w,
            mon_key,
            mon_pitch,
            level,
            win_x: x,
            win_key,
            win_gap,
        }
    }

    /// Left edge of view key `i`: three keys, the separator, then the rest.
    fn view_key_x(&self, i: usize) -> f32 {
        let gap = self.view_pitch - self.view_key;
        if i < 3 {
            self.view_x + i as f32 * self.view_pitch
        } else {
            self.view_x
                + 3.0 * self.view_key
                + 2.0 * gap
                + self.view_sep
                + (i - 3) as f32 * self.view_pitch
        }
    }

    fn view_right(&self) -> f32 {
        self.view_key_x(self.view_n - 1) + self.view_key
    }

    /// The transport keys' x and width, left to right: rewind, forward, to start, play,
    /// record, capture, cycle.
    fn transport_keys(&self) -> [(f32, f32); 7] {
        let mut x = self.tr_x;
        let mut out = [(0.0, 0.0); 7];
        let widths = [
            self.tr_key,
            self.tr_key,
            self.tr_key,
            self.play_w,
            self.tr_key,
            self.tr_key,
            self.cycle_w,
        ];
        for (o, w) in out.iter_mut().zip(widths) {
            *o = (x, w);
            x += w + self.tr_gap;
        }
        out
    }

    fn mon_keys_x(&self) -> f32 {
        self.mon_x + self.chip_w + 12.0
    }

    fn mon_right(&self) -> f32 {
        self.mon_keys_x() + 2.0 * self.mon_pitch + self.mon_key
    }

    fn win_right(&self) -> f32 {
        self.win_x + 2.0 * self.win_key + self.win_gap
    }
}

// ---------------------------------------------------------------- keys

/// A key of the bar with an icon (and `text` after it) on its face; `tint` colours the icon
/// instead of the cap's print.
#[allow(clippy::too_many_arguments)]
fn icon_key(
    ui: &Ui,
    g: &Painter,
    rect: Rect,
    key: &str,
    cap: Cap,
    down: bool,
    led: Option<bool>,
    ic: Icon,
    scale: f32,
    tint: Option<Color32>,
    text: Option<&str>,
) -> egui::Response {
    let (resp, body) = hw::key(
        ui,
        g,
        rect,
        Id::unique(("control_bar", key)),
        cap,
        down,
        led,
        Legend::Custom,
    );
    // A latched key sits in its well: a soft shade along the top of its face.
    if down {
        let face = rect.translate(vec2(0.0, 1.0));
        let r = (rect.height() * 0.12).clamp(3.0, 5.0);
        g.rect_filled(face.shrink(1.0), r, Color32::from_black_alpha(10));
        hw::inner_shadow(g, face.shrink(1.0), r, 0.14);
    }
    let ink = tint.unwrap_or(cap.ink());
    let size = ((body.height() * 0.46).min(body.width() * 0.55) * scale).clamp(11.0, 30.0);
    match text {
        None => icon(g, ic, body.center(), size, ink),
        Some(t) => {
            let fs = (body.height() * 0.3).clamp(9.0, 12.0);
            let gal = hw::galley(g, t, Face::Bold, fs, ink);
            let tw = gal.size().x - theme::tracking(Face::Bold) * fs;
            let gap = (size * 0.3).max(4.0);
            let total = size + gap + tw;
            let left = body.center().x - total / 2.0;
            icon(g, ic, pos2(left + size / 2.0, body.center().y), size, ink);
            hw::text(
                g,
                pos2(left + size + gap, body.center().y),
                Align2::LEFT_CENTER,
                t,
                Face::Bold,
                fs,
                ink,
            );
        }
    }
    resp
}

/// The silkscreen legend under a key centred at `cx`, narrowed to `room`.
fn legend(g: &Painter, rows: &Rows, cx: f32, room: f32, label: &str) {
    let mut size = rows.legend_size;
    while size > 8.0 && hw::silk_width(g, label, size) > room - 5.0 {
        size -= 0.5;
    }
    hw::silk(
        g,
        pos2(cx, rows.legend_y),
        Align2::CENTER_CENTER,
        label,
        size,
        theme::PRINT,
    );
}

// ---------------------------------------------------------------- LCD

/// Pixels of vertical drag per step when editing the time signature.
const SIG_PX_PER_STEP: f32 = 10.0;

/// Adds `delta_px` to the drag in progress and takes out the whole steps it now holds, keeping
/// the remainder for the next frame so slow drags still add up.
fn take_steps(leftover: &mut f32, delta_px: f32, px_per_step: f32) -> i32 {
    *leftover += delta_px;
    let steps = (*leftover / px_per_step).trunc();
    *leftover -= steps * px_per_step;
    steps as i32
}

/// The tempo typed into the LCD: a text field over `rect`, focused with its text selected
/// when it opens. `None` while typing; `Some(Some(bpm))` on Enter or clicking away with a
/// number, `Some(None)` on Escape or text that is not a number.
fn tempo_field(
    ctx: &egui::Context,
    rect: Rect,
    id: egui::Id,
    text: &mut String,
) -> Option<Option<f32>> {
    let opened = !ctx.data(|d| d.get_temp::<bool>(id.with("opened")).unwrap_or(false));
    let size = (rect.height() * 0.6).clamp(12.0, 20.0);
    let out = egui::Area::new(id.with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.left_top() + vec2(0.0, rect.height() * 0.5 - size * 0.75))
        .show(ctx, |ui| {
            let edit = egui::TextEdit::singleline(text)
                .id(id.with("field"))
                .desired_width(rect.width())
                .font(theme::font(Face::Lcd, size))
                .text_color(theme::LCD_INK)
                .background_color(theme::LCD)
                .align(egui::Align2::LEFT_CENTER);
            let mut out = edit.show(ui);
            if opened {
                out.response.request_focus();
                let all = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(text.chars().count()),
                );
                out.state.cursor.set_char_range(Some(all));
                out.state.store(ui.ctx(), out.response.id);
            }
            out.response
        })
        .inner;
    ctx.data_mut(|d| d.insert_temp(id.with("opened"), true));
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if escape || (out.lost_focus() && !opened) {
        ctx.data_mut(|d| d.remove::<bool>(id.with("opened")));
        if escape {
            return Some(None);
        }
        return Some(text.trim().parse::<f32>().ok().filter(|b| b.is_finite()));
    }
    None
}

/// What the LCD's CPU column shows.
pub(crate) struct Meters<'a> {
    /// The I/O callback's load (live tracks and the mix) and the playback thread's, 1.0 =
    /// the whole budget.
    pub live: f32,
    pub playback: f32,
    /// System overloads so far.
    pub overloads: u32,
    /// Allocator calls from our own audio-thread code so far (release builds count them).
    pub rt_allocs: u64,
    /// The heaviest plug-in lately and its load.
    pub heaviest: Option<&'a (String, f32)>,
}

/// The widest each LCD readout gets (all segments of "8"), so the layout never jumps as the
/// numbers change.
const W_SMPTE: &str = "88:88:88:88.88";
const W_POS: &str = "888 8 8 888";
const W_TEMPO: &str = "888.8888";
const W_SIG: &str = "88/88";

/// Segment thickness (fraction of the digit height): the hero position heaviest.
const THICK_HERO: f32 = 0.15;
const THICK: f32 = 0.14;

/// How the LCD's readouts are arranged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arrangement {
    /// The mockup's: SMPTE over the bar position | CYCLE L/R | TEMPO over SIG | status.
    Columns,
    /// A narrower LCD: SMPTE, TEMPO and SIG along the top, the bar position and the cycle
    /// locators under them, the status column at the right.
    Stacked,
    /// A low LCD (the compact bar): two rows of small readouts.
    Compact,
}

/// A label on the glass: where, how it is anchored, its text and whether it is a locator
/// letter (L / R, set in the LCD face and a little larger).
type LcdLabel = (Pos2, Align2, &'static str, bool);

/// Where each readout of the LCD goes inside the glass.
#[derive(Clone, Debug)]
struct LcdPlan {
    #[cfg_attr(not(test), allow(dead_code))]
    arr: Arrangement,
    /// Label size.
    lab: f32,
    smpte: (Pos2, f32),
    pos: (Pos2, f32),
    /// The cycle locators (left, right) and their digit height, when they fit.
    cycle: Option<(Pos2, Pos2, f32)>,
    tempo: (Pos2, f32),
    sig: (Pos2, f32),
    labels: Vec<LcdLabel>,
    /// Divider lines: x and the y range.
    dividers: Vec<(f32, f32, f32)>,
    /// The SMPTE/position zone, the tempo's and the signature's areas, the status column.
    time_zone: Rect,
    tempo_zone: Rect,
    sig_zone: Rect,
    status: Rect,
    /// Top left of the PLAY, CYC, REC and CPU indicators, and their text size.
    st_rows: [Pos2; 4],
    st_fs: f32,
}

/// Width of a label on the glass.
fn lab_w(g: &Painter, s: &str, face: Face, size: f32) -> f32 {
    hw::galley(g, s, face, size, theme::LCD_INK).size().x - theme::tracking(face) * size
}

/// Width of the status column: the PLAY row (a triangle and the word) or the CPU row (the word
/// and five bars), whichever is longer.
fn status_width(g: &Painter, lab: f32) -> f32 {
    let fs = (lab * 1.08).min(11.0);
    let play = fs * 0.8 + 5.0 + lab_w(g, "PLAY", Face::Lcd, fs);
    let cpu = lab_w(g, "CPU", Face::Bold, lab) + 5.0 + 5.0 * 5.4 * (lab / 10.0).max(0.75);
    play.max(cpu) + 4.0
}

impl LcdPlan {
    fn new(g: &Painter, glass: Rect) -> Self {
        if glass.height() < 70.0 {
            return Self::compact(g, glass);
        }
        let cols = Self::columns(g, glass);
        if let Some(c) = cols {
            return c;
        }
        Self::stacked(g, glass)
    }

    /// The mockup's four columns, when the bar position can still be at least 0.36 of the
    /// glass's height.
    fn columns(g: &Painter, glass: Rect) -> Option<Self> {
        let (gw, gh, top) = (glass.width(), glass.height(), glass.top());
        let k = gh / 108.0;
        let lab = (10.0 * k).clamp(8.5, 10.0);
        let w = hw::seg_width;
        let lab_col = lab_w(g, "SMPTE", Face::Bold, lab) + 12.0;
        let lr_col = lab_w(g, "L", Face::Lcd, lab * 1.2) + 7.0;
        let sig_lab = lab_w(g, "SIG", Face::Bold, lab) + 7.0;
        let status_w = status_width(g, lab);
        let (pad, dg) = (14.0, 12.0);
        let (hs, hp, hc, ht, hg) = (17.0 * k, 44.0 * k, 17.0 * k, 24.0 * k, 17.0 * k);
        let time_d = |f: f32| w(W_POS, hp * f).max(w(W_SMPTE, hs * f));
        let cyc_d = |f: f32| w(W_POS, hc * f);
        let tempo_d = |f: f32| w(W_TEMPO, ht * f).max(sig_lab + w(W_SIG, hg * f));
        let fixed = 2.0 * pad + lab_col + lr_col + 6.0 * dg + status_w;
        let digits = time_d(1.0) + cyc_d(1.0) + tempo_d(1.0);
        let f = ((gw - fixed) / digits).min(1.0);
        if hp * f < 0.36 * gh {
            return None;
        }
        let (hs, hp, hc, ht, hg) = (hs * f, hp * f, hc * f, ht * f, hg * f);
        let zones = [lab_col + time_d(f), lr_col + cyc_d(f), tempo_d(f), status_w];
        let used: f32 = zones.iter().sum::<f32>() + 2.0 * pad + 6.0 * dg;
        let spare = ((gw - used) / 4.0).max(0.0);
        let mut xs = [0.0; 4];
        let mut dividers = Vec::new();
        let mut x = glass.left() + pad;
        let (d0, d1) = (top + 10.0 * k, glass.bottom() - 10.0 * k);
        for (i, z) in zones.iter().enumerate() {
            xs[i] = x;
            x += z + spare;
            if i < 3 {
                x += dg;
                dividers.push((x, d0, d1));
                x += dg;
            }
        }
        let y = |v: f32| top + v * k;
        // The top row (SMPTE digits and the column labels) keeps at least 7 points from the
        // bezel.
        let row1 = (top + 7.0).max(y(9.0));
        let row1_c = row1 + hs / 2.0;
        let x0 = xs[0];
        let digits_x = x0 + lab_col;
        let hero_c = y(61.0);
        let mut labels: Vec<LcdLabel> = vec![
            (pos2(x0, row1_c), Align2::LEFT_CENTER, "SMPTE", false),
            (pos2(x0, hero_c - 1.0), Align2::LEFT_BOTTOM, "BAR", false),
            (pos2(x0, hero_c + 1.0), Align2::LEFT_TOP, "POS", false),
        ];
        let cx = xs[1];
        let (l_c, r_c) = (y(44.0), y(76.0));
        labels.push((pos2(cx, row1_c), Align2::LEFT_CENTER, "CYCLE", false));
        labels.push((pos2(cx, l_c), Align2::LEFT_CENTER, "L", true));
        labels.push((pos2(cx, r_c), Align2::LEFT_CENTER, "R", true));
        let tx = xs[2];
        let tz_w = zones[2] + spare;
        let (tempo_c, sig_c) = (y(46.0), y(76.0));
        labels.push((pos2(tx, row1_c), Align2::LEFT_CENTER, "TEMPO", false));
        labels.push((
            pos2(tx, sig_c + hg / 2.0),
            Align2::LEFT_BOTTOM,
            "SIG",
            false,
        ));
        let sx = xs[3];
        let st_fs = (lab * 1.08).min(11.0);
        let st_rows = [
            pos2(sx, row1),
            pos2(sx - 3.0, y(33.0)),
            pos2(sx - 3.0, y(54.0)),
            pos2(sx, y(76.0)),
        ];
        Some(Self {
            arr: Arrangement::Columns,
            lab,
            smpte: (pos2(digits_x, row1), hs),
            pos: (pos2(digits_x, hero_c - hp / 2.0), hp),
            cycle: Some((
                pos2(cx + lr_col, l_c - hc / 2.0),
                pos2(cx + lr_col, r_c - hc / 2.0),
                hc,
            )),
            tempo: (pos2(tx, tempo_c - ht / 2.0), ht),
            sig: (pos2(tx + sig_lab, sig_c - hg / 2.0), hg),
            labels,
            dividers,
            time_zone: Rect::from_min_max(
                pos2(glass.left(), top),
                pos2(xs[0] + zones[0] + spare, glass.bottom()),
            ),
            tempo_zone: Rect::from_min_max(pos2(tx - 4.0, top), pos2(tx + tz_w, y(62.0))),
            sig_zone: Rect::from_min_max(pos2(tx - 4.0, y(62.0)), pos2(tx + tz_w, glass.bottom())),
            status: Rect::from_min_max(pos2(sx, top), glass.max),
            st_rows,
            st_fs,
        })
    }

    /// SMPTE, TEMPO and SIG along the top; the bar position as large as the rest of the width
    /// allows with the cycle locators beside it; the status column at the right.
    fn stacked(g: &Painter, glass: Rect) -> Self {
        let (gw, gh, top) = (glass.width(), glass.height(), glass.top());
        let lab = (gh * 0.1).clamp(8.5, 10.0);
        let w = hw::seg_width;
        let lab_col = lab_w(g, "SMPTE", Face::Bold, lab) + 9.0;
        let lr_col = lab_w(g, "L", Face::Lcd, lab * 1.2) + 6.0;
        let status_w = status_width(g, lab);
        let (pad, dg) = (10.0, 8.0);
        let block_l = glass.left() + pad;
        let block_w = gw - 2.0 * pad - 2.0 * dg - status_w;
        let block_r = block_l + block_w;
        let digits_x = block_l + lab_col;
        let hc = (gh * 0.137).clamp(11.0, 15.0);
        let hp_max = gh * 0.42;
        let unit = w(W_POS, 1.0);
        let mut hp = ((block_w - lab_col - 2.0 * dg - lr_col - w(W_POS, hc)) / unit).min(hp_max);
        let with_cycle = hp >= gh * 0.295;
        if !with_cycle {
            hp = ((block_w - lab_col) / unit).min(hp_max);
        }
        // Top row: SMPTE at the digits' column, SIG at the right end, TEMPO between them.
        let tl = lab_w(g, "TEMPO", Face::Bold, lab) + 6.0;
        let sl = lab_w(g, "SIG", Face::Bold, lab) + 6.0;
        let mut h1 = (gh * 0.155).clamp(11.0, 16.0);
        let row_w = |h: f32| w(W_SMPTE, h) + tl + w(W_TEMPO, h) + sl + w(W_SIG, h) + 24.0;
        while h1 > 9.5 && digits_x + row_w(h1) > block_r {
            h1 -= 0.5;
        }
        let row1 = top + (gh * 0.08).max(7.0);
        let row1_c = row1 + h1 / 2.0;
        let sig_x = block_r - w(W_SIG, h1);
        let sig_lab_x = sig_x - sl;
        let smpte_end = digits_x + w(W_SMPTE, h1);
        let tempo_w = tl + w(W_TEMPO, h1);
        let cycle_x = digits_x + w(W_POS, hp) + 2.0 * dg;
        let free = (smpte_end + 12.0, sig_lab_x - 12.0);
        let tempo_lab_x = if with_cycle && cycle_x >= free.0 && cycle_x + tempo_w <= free.1 {
            cycle_x
        } else {
            (free.0 + free.1 - tempo_w) / 2.0
        };
        let tempo_x = tempo_lab_x + tl;
        let r2_top = row1 + h1 + (gh * 0.1).max(8.0);
        let r2_bot = glass.bottom() - (gh * 0.08).max(7.0);
        let hero_c = (r2_top + r2_bot) / 2.0;
        let mut labels: Vec<LcdLabel> = vec![
            (pos2(block_l, row1_c), Align2::LEFT_CENTER, "SMPTE", false),
            (
                pos2(tempo_lab_x, row1_c),
                Align2::LEFT_CENTER,
                "TEMPO",
                false,
            ),
            (pos2(sig_lab_x, row1_c), Align2::LEFT_CENTER, "SIG", false),
            (
                pos2(block_l, hero_c - 1.0),
                Align2::LEFT_BOTTOM,
                "BAR",
                false,
            ),
            (pos2(block_l, hero_c + 1.0), Align2::LEFT_TOP, "POS", false),
        ];
        let status_l = block_r + 2.0 * dg;
        let mut dividers = vec![(block_r + dg, top + 8.0, glass.bottom() - 8.0)];
        let cycle = with_cycle.then(|| {
            dividers.push((cycle_x - dg, r2_top, r2_bot));
            let gap = ((r2_bot - r2_top - 2.0 * hc) / 3.0).max(4.0);
            let l_top = hero_c - gap / 2.0 - hc;
            let r_top = hero_c + gap / 2.0;
            labels.push((
                pos2(cycle_x, l_top + hc / 2.0),
                Align2::LEFT_CENTER,
                "L",
                true,
            ));
            labels.push((
                pos2(cycle_x, r_top + hc / 2.0),
                Align2::LEFT_CENTER,
                "R",
                true,
            ));
            (
                pos2(cycle_x + lr_col, l_top),
                pos2(cycle_x + lr_col, r_top),
                hc,
            )
        });
        let st_fs = (lab * 1.08).min(11.0);
        let step = (gh - 2.0 * (row1 - top) - st_fs) / 3.0;
        let st_rows = [
            pos2(status_l, row1),
            pos2(status_l - 3.0, row1 + step),
            pos2(status_l - 3.0, row1 + 2.0 * step),
            pos2(status_l, row1 + 3.0 * step),
        ];
        let tempo_zone = Rect::from_min_max(
            pos2(tempo_lab_x - 4.0, top),
            pos2(tempo_x + w(W_TEMPO, h1) + 4.0, r2_top - 2.0),
        );
        let sig_zone = Rect::from_min_max(
            pos2(sig_lab_x - 4.0, top),
            pos2(block_r + 4.0, r2_top - 2.0),
        );
        Self {
            arr: Arrangement::Stacked,
            lab,
            smpte: (pos2(digits_x, row1), h1),
            pos: (pos2(digits_x, hero_c - hp / 2.0), hp),
            cycle,
            tempo: (pos2(tempo_x, row1), h1),
            sig: (pos2(sig_x, row1), h1),
            labels,
            dividers,
            time_zone: Rect::from_min_max(
                pos2(glass.left(), top),
                pos2(
                    if with_cycle { cycle_x - dg } else { block_r },
                    glass.bottom(),
                ),
            ),
            tempo_zone,
            sig_zone,
            status: Rect::from_min_max(pos2(status_l, top), glass.max),
            st_rows,
            st_fs,
        }
    }

    /// Two rows of small readouts for a low LCD: SMPTE / L / TEMPO over BAR / R / SIG, then
    /// the status in two columns at a fixed width so the CPU bars stay on the glass.
    fn compact(g: &Painter, glass: Rect) -> Self {
        let (gw, gh, top) = (glass.width(), glass.height(), glass.top());
        let lab = 9.0;
        let w = hw::seg_width;
        let lab_col = lab_w(g, "SMPTE", Face::Bold, lab) + 6.0;
        let lr_col = lab_w(g, "L", Face::Lcd, lab * 1.2) + 5.0;
        let sig_lab = lab_w(g, "SIG", Face::Bold, lab) + 6.0;
        let st_fs = 9.0;
        let play_w = st_fs * 0.8 + 5.0 + lab_w(g, "PLAY", Face::Lcd, st_fs);
        let cpu_w = lab_w(g, "CPU", Face::Bold, lab) + 5.0 + 5.0 * 5.5;
        let col2_off = play_w + 10.0;
        let status_w = col2_off + cpu_w + 4.0;
        let (pad, dg) = (8.0, 8.0);
        let h0 = gh * 0.31;
        let time_d = |h: f32| w(W_POS, h).max(w(W_SMPTE, h));
        let digits = |h: f32| time_d(h) + w(W_POS, h) + w(W_TEMPO, h).max(sig_lab + w(W_SIG, h));
        let fixed = 2.0 * pad + lab_col + lr_col + 6.0 * dg + status_w;
        let f = ((gw - fixed) / digits(h0)).min(1.0);
        let with_cycle = f >= 0.8;
        let h = if with_cycle {
            h0 * f
        } else {
            h0 * ((gw - fixed + lr_col + 2.0 * dg) / (digits(h0) - w(W_POS, h0))).clamp(0.4, 1.0)
        };
        let mut zones = vec![lab_col + time_d(h)];
        if with_cycle {
            zones.push(lr_col + w(W_POS, h));
        }
        zones.push(w(W_TEMPO, h).max(sig_lab + w(W_SIG, h)));
        zones.push(status_w);
        let n = zones.len();
        let used: f32 = zones.iter().sum::<f32>() + 2.0 * pad + 2.0 * dg * (n - 1) as f32;
        let spare = ((gw - used) / n as f32).max(0.0);
        let mut xs = Vec::new();
        let mut dividers = Vec::new();
        let mut x = glass.left() + pad;
        for (i, z) in zones.iter().enumerate() {
            xs.push(x);
            x += z + spare;
            if i + 1 < n {
                x += dg;
                dividers.push((x, top + 5.0, glass.bottom() - 5.0));
                x += dg;
            }
        }
        let r1 = top + gh * 0.15;
        let r2 = top + gh * 0.55;
        let mut labels: Vec<LcdLabel> = vec![
            (
                pos2(xs[0], r1 + h / 2.0),
                Align2::LEFT_CENTER,
                "SMPTE",
                false,
            ),
            (pos2(xs[0], r2 + h / 2.0), Align2::LEFT_CENTER, "BAR", false),
        ];
        let mut zi = 1;
        let cycle = with_cycle.then(|| {
            let cx = xs[zi];
            zi += 1;
            labels.push((pos2(cx, r1 + h / 2.0), Align2::LEFT_CENTER, "L", true));
            labels.push((pos2(cx, r2 + h / 2.0), Align2::LEFT_CENTER, "R", true));
            (pos2(cx + lr_col, r1), pos2(cx + lr_col, r2), h)
        });
        let tx = xs[zi];
        let tz_w = zones[zi] + spare;
        zi += 1;
        labels.push((pos2(tx, r2 + h / 2.0), Align2::LEFT_CENTER, "SIG", false));
        let sx = xs[zi];
        let st_rows = [
            pos2(sx, r1),
            pos2(sx + col2_off - 3.0, r1 - 1.0),
            pos2(sx - 3.0, r2 - 1.0),
            pos2(sx + col2_off, r2),
        ];
        Self {
            arr: Arrangement::Compact,
            lab,
            smpte: (pos2(xs[0] + lab_col, r1), h),
            pos: (pos2(xs[0] + lab_col, r2), h),
            cycle,
            tempo: (pos2(tx, r1), h),
            sig: (pos2(tx + sig_lab, r2), h),
            labels,
            dividers,
            time_zone: Rect::from_min_max(
                pos2(glass.left(), top),
                pos2(xs[0] + zones[0] + spare, glass.bottom()),
            ),
            tempo_zone: Rect::from_min_max(pos2(tx - 4.0, top), pos2(tx + tz_w, top + gh * 0.5)),
            sig_zone: Rect::from_min_max(
                pos2(tx - 4.0, top + gh * 0.5),
                pos2(tx + tz_w, glass.bottom()),
            ),
            status: Rect::from_min_max(pos2(sx, top), glass.max),
            st_rows,
            st_fs,
        }
    }
}

/// The LCD: bezel and glass, then the readouts, the status column and their controls.
fn lcd(ui: &Ui, g: &Painter, p: &mut Project, bezel: Rect, meters: &Meters) {
    let glass = hw::lcd(g, bezel);
    let plan = LcdPlan::new(g, glass);
    let g = &g.with_clip_rect(glass);
    let ink = theme::LCD_INK;
    let ghost = ink.gamma_multiply(0.2);
    for (at, anchor, label, letter) in &plan.labels {
        let (size, face, strength) = if *letter {
            ((plan.lab * 1.2).min(12.0), Face::Lcd, 1.0)
        } else {
            (plan.lab, Face::Bold, 0.72)
        };
        hw::text(
            g,
            *at,
            *anchor,
            label,
            face,
            size,
            ink.gamma_multiply(strength),
        );
    }
    for (x, y0, y1) in &plan.dividers {
        g.vline(
            x.round() + 0.5,
            *y0..=*y1,
            Stroke::new(1.0, ink.gamma_multiply(0.22)),
        );
    }
    let bbdt = |(b, be, d, t): (u32, u32, u32, u32)| format!("{b:03} {be} {d} {t:03}");
    let seg =
        |at: Pos2, s: &str, h: f32, thick: f32| hw_transport::seg(g, at, s, h, ink, true, thick);

    // SMPTE over the bar position. Right-click for the frame rate and the MIDI Time Code
    // ports.
    let smpte = ui
        .interact(
            plan.time_zone,
            Id::unique(("control_bar", "smpte")),
            Sense::click(),
        )
        .on_hover_text(tr("Right-click for the SMPTE and synchronization settings"));
    crate::widgets::context_menu(&smpte, |ui| crate::sync_menu::items(ui, p));
    seg(plan.smpte.0, &p.timecode(), plan.smpte.1, THICK);
    seg(
        plan.pos.0,
        &bbdt(p.bbdt(p.position_beats)),
        plan.pos.1,
        THICK_HERO,
    );

    // The cycle locators.
    if let Some((l, r, h)) = plan.cycle {
        seg(l, &bbdt(p.bbdt(p.cycle_start)), h, THICK);
        seg(r, &bbdt(p.bbdt(p.cycle_end)), h, THICK);
    }

    // Tempo: drag vertically to change it, like Logic; click to type one. With a Tempo track
    // it is the tempo at the playhead, and dragging changes that one.
    let tempo_id = Id::unique(("control_bar", "tempo"));
    let resp = ui
        .interact(plan.tempo_zone, tempo_id, Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::ResizeVertical);
    let tempo_now = p.tempo_at(p.position_beats);
    if resp.dragged() {
        p.set_tempo_at(p.position_beats, tempo_now - resp.drag_delta().y * 0.5);
    }
    let edit_id = tempo_id.with("typing");
    let mut typing: Option<String> = ui.data(|d| d.get_temp(edit_id));
    if resp.clicked() && typing.is_none() {
        typing = Some(format!("{tempo_now:.2}"));
    }
    let (tp, th) = plan.tempo;
    if let Some(text) = typing.as_mut() {
        let field = Rect::from_min_size(tp, vec2(hw::seg_width(W_TEMPO, th), th));
        if let Some(bpm) = tempo_field(ui.ctx(), field, edit_id, text) {
            if let Some(bpm) = bpm {
                p.set_tempo_at(p.position_beats, bpm);
            }
            typing = None;
        }
        ui.data_mut(|d| match &typing {
            Some(t) => {
                d.insert_temp(edit_id, t.clone());
            }
            None => d.remove::<String>(edit_id),
        });
    } else {
        // Right-aligned in the widest tempo's room, with ghost digits in the unused places, as
        // on a fixed digit LCD.
        let tempo = format!("{:.4}", p.tempo_at(p.position_beats));
        let x = tp.x + hw::seg_width(W_TEMPO, th) - hw::seg_width(&tempo, th);
        seg(pos2(x, tp.y), &tempo, th, THICK);
    }

    // Time signature. Drag the numerator or the denominator up or down, like the tempo.
    let (sp, sh) = plan.sig;
    let (num, den) = p.signature_of_bar(p.beats_to_bar(p.position_beats) as u32);
    let num_s = num.to_string();
    let split = sp.x + hw::seg_width(&num_s, sh) + hw::seg_width("/", sh) * 0.5;
    for (part, range) in [
        (SigPart::Numerator, plan.sig_zone.left()..=split),
        (SigPart::Denominator, split..=plan.sig_zone.right()),
    ] {
        let id = Id::unique(("control_bar", "signature", part as u8));
        let zone = Rect::from_x_y_ranges(range, plan.sig_zone.y_range());
        let resp = ui
            .interact(zone, id, Sense::drag())
            .on_hover_cursor(egui::CursorIcon::ResizeVertical)
            .on_hover_text(tr("Drag to change the time signature"));
        let mut leftover = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
        if resp.drag_started() {
            leftover = 0.0;
        }
        let steps = if resp.dragged() {
            take_steps(&mut leftover, -resp.drag_delta().y, SIG_PX_PER_STEP)
        } else {
            0
        };
        ui.data_mut(|d| d.insert_temp(id, leftover));
        if steps != 0 {
            let bar0 = p.beats_to_bar(p.position_beats) as u32;
            p.step_signature_at(bar0, part, steps);
        }
    }
    seg(sp, &format!("{num}/{den}"), sh, THICK);

    // Status: PLAY, CYC (inverted while cycling), REC (a ghost until recording) and the CPU
    // histogram, as Logic's Audio and Disk meters: the louder of Live (the I/O callback: live
    // tracks and the mix) and Playback (the pre-rendered tracks' thread).
    let fs = plan.st_fs;
    let [play_at, cyc_at, rec_at, cpu_at] = plan.st_rows;
    let play_c = if p.playing { ink } else { ghost };
    let tri_h = fs * 0.95;
    let (x1, y1) = (play_at.x, play_at.y + 1.0);
    g.add(Shape::convex_polygon(
        vec![
            pos2(x1, y1),
            pos2(x1 + tri_h * 0.8, y1 + tri_h / 2.0),
            pos2(x1, y1 + tri_h),
        ],
        play_c,
        Stroke::NONE,
    ));
    hw::text(
        g,
        pos2(x1 + tri_h * 0.8 + 5.0, y1 + tri_h / 2.0),
        Align2::LEFT_CENTER,
        "PLAY",
        Face::Lcd,
        fs,
        play_c,
    );
    let chip = |at: Pos2, label: &str, on: bool| {
        let gal = hw::galley(g, label, Face::Lcd, fs * 0.92, ink);
        let r = Rect::from_min_size(at, gal.size() + vec2(6.0, 2.0));
        if on {
            g.rect_filled(r, 1.0, ink);
        }
        hw::text(
            g,
            r.center(),
            Align2::CENTER_CENTER,
            label,
            Face::Lcd,
            fs * 0.92,
            if on { theme::LCD } else { ghost },
        );
        r
    };
    chip(cyc_at, "CYC", p.cycle);
    let rec_c = if p.recording { ink } else { ghost };
    hw::text(
        g,
        rec_at + vec2(3.0, 1.0),
        Align2::LEFT_TOP,
        "REC",
        Face::Lcd,
        fs * 0.92,
        rec_c,
    );
    // CPU: the label (OVL, inverted, once the system has overloaded) and five rising bars,
    // kept on the glass.
    let lab_r = if meters.overloads > 0 {
        chip(cpu_at - vec2(3.0, 1.0), "OVL", true)
    } else {
        hw::text(
            g,
            cpu_at,
            Align2::LEFT_TOP,
            "CPU",
            Face::Bold,
            plan.lab,
            ink.gamma_multiply(0.72),
        )
    };
    let load = meters.live.max(meters.playback).clamp(0.0, 1.0);
    let lit = (load * 5.0).ceil() as usize;
    let bk = (plan.lab / 10.0).max(0.75);
    let start = lab_r.right() + 5.0;
    let pitch = (7.0 * bk).min((glass.right() - 4.0 - start) / 5.0).max(2.0);
    let bw = (pitch * 0.72).max(1.5);
    let base = lab_r.bottom() - 1.0;
    for i in 0..5 {
        let h = ((2.0 + 1.6 * i as f32) * bk * 1.2).min(lab_r.height() + 2.0);
        let x = start + i as f32 * pitch;
        let r = Rect::from_min_max(pos2(x, base - h), pos2(x + bw, base));
        g.rect_filled(r, 0.0, if i < lit { ink } else { theme::LCD_GHOST });
    }
    let mut text = format!(
        "{}\n{}",
        trf("Live: {}%", &[&format!("{:.0}", meters.live * 100.0)]),
        trf(
            "Playback: {}%",
            &[&format!("{:.0}", meters.playback * 100.0)]
        ),
    );
    if let Some((name, load)) = meters.heaviest {
        text.push('\n');
        text.push_str(&trf(
            "Heaviest plug-in: {} ({}%)",
            &[name, &format!("{:.0}", load * 100.0)],
        ));
    }
    if meters.overloads > 0 {
        text.push('\n');
        text.push_str(&trf("System overload: {}", &[&meters.overloads]));
    }
    if meters.rt_allocs > 0 {
        text.push('\n');
        text.push_str(&trf("Audio thread allocations: {}", &[&meters.rt_allocs]));
    }
    ui.interact(
        plan.status,
        Id::unique(("lcd", "cpu_meter")),
        Sense::hover(),
    )
    .on_hover_text(text);
}

/// The latency chip of the MONITOR group: a small LCD with the engine mode (RECORD / MIX, see
/// `engine_mode`) over the round trip of a monitored input. Returns whether it was clicked.
fn latency_chip(ui: &Ui, g: &Painter, bezel: Rect, s: &crate::UiState) -> bool {
    use crate::engine_mode::EngineMode;
    let resp = ui
        .interact(
            bezel,
            Id::unique(("control_bar", "engine_mode")),
            Sense::click(),
        )
        .on_hover_text(format!(
            "{}\n{}",
            match s.modes.mode {
                EngineMode::Record => {
                    tr("Record mode: lowest latency for playing and recording. Click for Mix mode.")
                }
                EngineMode::Mix => {
                    tr("Mix mode: most headroom for heavy plug-ins. Click for Record mode.")
                }
            },
            trf("{} ms", &[&format!("{:.1}", s.round_trip_ms)])
        ));
    let glass = hw::lcd(g, bezel);
    let g = &g.with_clip_rect(glass);
    let ink = theme::LCD_INK;
    let lab = (glass.height() * 0.2).clamp(8.0, 10.0);
    hw::text(
        g,
        glass.left_top() + vec2(6.0, glass.height() * 0.12),
        Align2::LEFT_TOP,
        s.modes.mode.label(),
        Face::Bold,
        lab,
        ink.gamma_multiply(0.72),
    );
    let ms = format!("{:.1}", s.round_trip_ms);
    let unit = |h: f32| (h * 0.5).clamp(7.0, 11.0);
    let unit_w = |h: f32| lab_w(g, "MS", Face::Lcd, unit(h));
    let mut h = (glass.height() * 0.44).clamp(9.0, 22.0);
    while h > 9.0 && 6.0 + hw::seg_width(&ms, h) + 5.0 + unit_w(h) + 5.0 > glass.width() {
        h -= 0.5;
    }
    let y = glass.bottom() - glass.height() * 0.12 - h;
    let w = hw_transport::seg(g, pos2(glass.left() + 6.0, y), &ms, h, ink, true, THICK);
    hw::text(
        g,
        pos2(glass.left() + 6.0 + w + 5.0, y + h),
        Align2::LEFT_BOTTOM,
        "MS",
        Face::Lcd,
        unit(h),
        ink,
    );
    if resp.hovered() {
        g.rect_stroke(
            glass,
            3.0,
            Stroke::new(1.0, ink.gamma_multiply(0.4)),
            egui::StrokeKind::Inside,
        );
    }
    resp.clicked()
}

// ---------------------------------------------------------------- main level

/// Where 0 dB sits along the main level (0..1): -∞ at the left end, -60..0 dB spread evenly
/// up to here, +6 dB at the right end.
const ZERO_AT: f32 = 0.726;

/// Position (0..1) of `db` along the main level.
fn level_pos(db: f32) -> f32 {
    if db <= -60.0 {
        0.0
    } else if db <= 0.0 {
        ZERO_AT * (db + 60.0) / 60.0
    } else {
        (ZERO_AT + (1.0 - ZERO_AT) * db / 6.0).min(1.0)
    }
}

/// The gain (linear) at position `pos` of the main level: 0 at the left end.
fn level_gain(pos: f32) -> f32 {
    let pos = pos.clamp(0.0, 1.0);
    if pos <= 0.0 {
        return 0.0;
    }
    let db = if pos <= ZERO_AT {
        pos / ZERO_AT * 60.0 - 60.0
    } else {
        (pos - ZERO_AT) / (1.0 - ZERO_AT) * 6.0
    };
    10f32.powf(db / 20.0)
}

fn amp_db(a: f32) -> f32 {
    if a <= 1e-6 {
        f32::NEG_INFINITY
    } else {
        20.0 * a.log10()
    }
}

/// A slider thumb: a black cap shaded left to right with a light index line down its middle.
fn thumb(g: &Painter, r: Rect) {
    g.rect_filled(
        r.translate(vec2(0.0, 2.0)),
        3.0,
        Color32::from_black_alpha(80),
    );
    let mut mesh = egui::epaint::Mesh::default();
    let (l, m, rr) = (
        Color32::from_rgb(0x3a, 0x38, 0x35),
        Color32::from_rgb(0x1a, 0x19, 0x17),
        Color32::from_rgb(0x0f, 0x0e, 0x0d),
    );
    let xs = [r.left(), r.center().x, r.right()];
    let cs = [l, m, rr];
    for i in 0..3 {
        mesh.colored_vertex(pos2(xs[i], r.top()), cs[i]);
        mesh.colored_vertex(pos2(xs[i], r.bottom()), cs[i]);
    }
    for i in 0..2u32 {
        let a = i * 2;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 2, a + 3);
    }
    g.add(Shape::mesh(mesh));
    g.rect_stroke(
        r,
        2.0,
        Stroke::new(1.0, theme::CAP_EDGE),
        egui::StrokeKind::Inside,
    );
    g.vline(
        r.left() + 1.5,
        r.top() + 2.0..=r.bottom() - 2.0,
        Stroke::new(1.0, Color32::from_white_alpha(40)),
    );
    let inset = (r.height() * 0.1).clamp(2.0, 3.0);
    g.rect_filled(
        Rect::from_center_size(r.center(), vec2(3.0, r.height() - 2.0 * inset)),
        0.0,
        theme::BONE_HI,
    );
}

/// The main level (master volume): a segmented stereo ladder over a slider, sharing one
/// scale (-∞ … 0 … +6 dB). Drag or click anywhere along it; double-click for 0 dB. In a rect
/// under 40 points tall (the Tracks area's master row) it packs the same parts tighter.
pub(crate) fn master_volume(
    ui: &Ui,
    g: &Painter,
    p: &mut Project,
    rect: Rect,
    peaks: Option<(f32, f32)>,
    id: egui::Id,
) {
    let full = rect.height() >= 40.0;
    let resp = ui.interact(rect.expand2(vec2(4.0, 4.0)), id, Sense::click_and_drag());
    if let Some(pt) = resp.interact_pointer_pos() {
        if resp.is_pointer_button_down_on() || resp.clicked() {
            p.master_vol = level_gain((pt.x - rect.left()) / rect.width()).min(2.0);
        }
    }
    if resp.double_clicked() {
        p.master_vol = 1.0;
    }
    let x_of = |f: f32| rect.left() + f * rect.width();

    // Ladder: L over R, segments lit against the same scale as the slider.
    let ladder = if full {
        Rect::from_min_size(rect.min + vec2(0.0, 2.0), vec2(rect.width(), 14.0))
    } else {
        Rect::from_min_size(rect.min, vec2(rect.width(), (rect.height() * 0.4).max(6.0)))
    };
    g.hline(
        ladder.x_range(),
        ladder.bottom() + 0.5,
        Stroke::new(1.0, theme::BONE_HI),
    );
    g.rect_filled(ladder, 2.0, theme::METER_WELL);
    hw::inner_shadow(g, ladder, 2.0, 0.5);
    // Fine segments with hairline gaps, about 40 of them along a full-size ladder.
    let pitch = if full {
        (rect.width() / 42.0).clamp(2.8, 4.6)
    } else {
        3.0
    };
    let sw = pitch - if full { 1.2 } else { 1.0 };
    let inner = ladder.shrink2(vec2(4.0, if full { 3.0 } else { 1.5 }));
    let n = ((inner.width() + pitch - sw) / pitch).floor().max(1.0) as usize;
    let (l, r) = peaks.unwrap_or((0.0, 0.0));
    let row_h = (inner.height() - 1.0) / 2.0;
    let (warm_at, hot_at) = (level_pos(-6.0), ZERO_AT);
    for (row, amp) in [l, r].into_iter().enumerate() {
        let lvl = level_pos(amp_db(amp));
        let y0 = inner.top() + row as f32 * (row_h + 1.0);
        for i in 0..n {
            let x = inner.left() + i as f32 * pitch;
            let seg = Rect::from_min_size(pos2(x, y0), vec2(sw, row_h));
            let f = (seg.center().x - rect.left()) / rect.width();
            let c = if f <= lvl && lvl > 0.0 {
                if f > hot_at {
                    g.rect_filled(seg.expand(1.0), 1.0, theme::ORANGE.gamma_multiply(0.35));
                    theme::ORANGE
                } else if f > warm_at {
                    Color32::from_rgb(0xff, 0xe6, 0xcf)
                } else {
                    Color32::from_rgb(0xee, 0xea, 0xdf)
                }
            } else {
                Color32::from_rgb(0x2b, 0x29, 0x26)
            };
            g.rect_filled(seg, 0.0, c);
        }
    }

    // Slider: a groove, the printed ticks under it, the thumb.
    let (track_y, track_h, thumb_size) = if full {
        (
            rect.top() + rect.height() * 0.63,
            6.0,
            vec2(24.0, (rect.height() * 0.5).clamp(20.0, 30.0)),
        )
    } else {
        let below = rect.bottom() - ladder.bottom();
        (
            ladder.bottom() + below / 2.0 + 0.5,
            4.0,
            vec2(12.0, (below - 1.0).max(8.0)),
        )
    };
    let track = Rect::from_min_size(
        pos2(rect.left(), track_y - track_h / 2.0),
        vec2(rect.width(), track_h),
    );
    g.hline(
        track.x_range(),
        track.bottom() + 0.5,
        Stroke::new(1.0, theme::BONE_HI),
    );
    g.rect_filled(track, track_h / 2.0, Color32::from_rgb(0x0f, 0x0f, 0x0e));
    if full {
        let tick = Stroke::new(1.0, theme::INK2.gamma_multiply(0.6));
        for db in [
            -60.0f32, -50.0, -40.0, -30.0, -20.0, -10.0, -6.0, -3.0, 0.0, 3.0, 6.0,
        ] {
            let major = db == -60.0 || db == 0.0 || db == 6.0;
            let x = x_of(if db <= -60.0 { 0.0 } else { level_pos(db) }).round() + 0.5;
            let y0 = track.bottom() + thumb_size.y * 0.5 - 3.0;
            g.vline(x, y0..=y0 + if major { 8.0 } else { 5.0 }, tick);
        }
    }
    let pos = level_pos(amp_db(p.master_vol));
    let tr = Rect::from_center_size(pos2(x_of(pos), track_y), thumb_size);
    thumb(g, tr);
}

// ---------------------------------------------------------------- the bar

pub fn control_bar(
    ui: &mut Ui,
    p: &mut Project,
    s: &mut UiState,
    master_peaks: Option<(f32, f32)>,
) {
    let bar = ui.max_rect();
    let g = ui.painter_at(bar);
    hw::chassis(&g, bar);
    let rows = Rows::of(bar);
    let view_n = if crate::menus::IN_WINDOW { 6 } else { 5 };
    let lay = Layout::of(bar.width(), view_n);
    let x = |dx: f32| bar.left() + dx;
    let key_rect =
        |left: f32, w: f32| Rect::from_min_size(pos2(x(left), rows.key_top), vec2(w, rows.key_h));
    // Screws in the corners (one at each end of the compact bar), clear of the groups: the
    // layout's margin is wider than a screw plus 8 points.
    let r = 4.5 + (lay.margin - 20.0) / 10.0;
    let sx = 3.0 + r;
    if rows.full {
        hw::seam_h(&g, bar.x_range(), bar.bottom() - 3.0);
        for (cx, cy, a) in [
            (bar.left() + sx, bar.top() + 10.0 + r, 20.0),
            (bar.left() + sx, bar.bottom() - 13.0 - r, -35.0),
            (bar.right() - sx, bar.top() + 10.0 + r, 50.0),
            (bar.right() - sx, bar.bottom() - 13.0 - r, 10.0),
        ] {
            hw::screw(&g, pos2(cx, cy), r, a);
        }
    } else {
        hw::seam_h(&g, bar.x_range(), bar.bottom() - 2.0);
        let cy = bar.center().y - 1.0;
        hw::screw(&g, pos2(bar.left() + sx, cy), r.min(4.0), 30.0);
        hw::screw(&g, pos2(bar.right() - sx, cy), r.min(4.0), -20.0);
    }

    // VIEW: Inspector / Tips / Toolbar, then Mixer / Editors and, where the menu bar is in the
    // window, its light or dark look.
    let mixer_on = s.show_mixer && s.bottom == Bottom::Mixer;
    let editors_on = s.bottom == Bottom::PianoRoll;
    let view = [
        (Icon::Info, s.show_inspector, "INSP", "Show Inspector"),
        (Icon::Help, s.show_tips, "HELP", "Show Tips"),
        (Icon::Toolbar, s.show_toolbar, "TOOLBAR", "Show Toolbar"),
        (Icon::Mixer, mixer_on, "MIXER", "Show Mixer"),
        (Icon::Scissors, editors_on, "EDIT", "Show Piano Roll"),
        (Icon::Appearance, s.light_menus, "MENUS", ""),
    ];
    if rows.full {
        hw_transport::bracket(
            &g,
            x(lay.view_x),
            x(lay.view_right()),
            rows.bracket_y,
            "VIEW",
            rows.bracket_size,
        );
    }
    let sep_x = x(lay.view_key_x(2)
        + lay.view_key
        + (lay.view_key_x(3) - lay.view_key_x(2) - lay.view_key) / 2.0);
    g.vline(
        sep_x.round() + 0.5,
        rows.key_top + 8.0..=rows.key_top + rows.key_h - 8.0,
        Stroke::new(1.0, theme::SEAM),
    );
    let mut hit = None;
    for (i, (ic, on, label, tip)) in view.iter().take(view_n).enumerate() {
        let r = key_rect(lay.view_key_x(i), lay.view_key);
        let resp = icon_key(
            ui,
            &g,
            r,
            label,
            Cap::Bone,
            *on,
            Some(*on),
            *ic,
            1.0,
            None,
            None,
        );
        let resp = if tip.is_empty() {
            resp
        } else {
            resp.on_hover_text(tr(tip))
        };
        if resp.clicked() {
            hit = Some(i);
        }
        legend(&g, &rows, r.center().x, lay.view_pitch, label);
    }
    match hit {
        Some(0) => s.show_inspector = !s.show_inspector,
        Some(1) => s.show_tips = !s.show_tips,
        Some(2) => s.show_toolbar = !s.show_toolbar,
        Some(3) => {
            s.show_mixer = !mixer_on;
            s.bottom = Bottom::Mixer;
        }
        Some(4) => {
            s.bottom = if editors_on {
                Bottom::Mixer
            } else {
                Bottom::PianoRoll
            };
        }
        // Switches the in-window menu bar and its menus between dark and light.
        Some(5) => s.light_menus = !s.light_menus,
        _ => {}
    }

    // TRANSPORT.
    let keys = lay.transport_keys();
    if rows.full {
        let (last_x, last_w) = keys[6];
        hw_transport::bracket(
            &g,
            x(lay.tr_x),
            x(last_x + last_w),
            rows.bracket_y,
            "TRANSPORT",
            rows.bracket_size,
        );
    }
    let tk = |i: usize| key_rect(keys[i].0, keys[i].1);
    let legends = ["REW", "FWD", "START", "PLAY", "REC", "CAPTURE", "CYCLE"];
    for (i, label) in legends.iter().enumerate() {
        legend(&g, &rows, tk(i).center().x, keys[i].1 + lay.tr_gap, label);
    }
    if icon_key(
        ui,
        &g,
        tk(0),
        "rew",
        Cap::Dark,
        false,
        None,
        Icon::Rewind,
        1.0,
        None,
        None,
    )
    .clicked()
    {
        // Back to the start of this bar, or of the one before when already there.
        let bar = p.beats_to_bar(p.position_beats);
        let back = if bar.fract() < 1e-6 {
            bar - 1.0
        } else {
            bar.floor()
        };
        p.position_beats = p.bar_to_beats(back.max(0.0));
    }
    if icon_key(
        ui,
        &g,
        tk(1),
        "ffw",
        Cap::Dark,
        false,
        None,
        Icon::Forward,
        1.0,
        None,
        None,
    )
    .on_hover_text(tr("Forward"))
    .clicked()
    {
        p.position_beats = p.next_bar_start(p.position_beats);
    }
    if icon_key(
        ui,
        &g,
        tk(2),
        "start",
        Cap::Dark,
        false,
        None,
        Icon::ToStart,
        1.0,
        None,
        None,
    )
    .on_hover_text(tr("Go to Beginning"))
    .clicked()
    {
        p.position_beats = 0.0;
    }
    if icon_key(
        ui,
        &g,
        tk(3),
        "play",
        Cap::Bone,
        p.playing,
        Some(p.playing),
        Icon::Play,
        1.15,
        p.playing.then_some(theme::ORANGE),
        None,
    )
    .clicked()
    {
        p.playing = !p.playing;
    }
    if p.recording {
        theme::glow(&g, tk(4).translate(vec2(0.0, 1.0)), 5.0, theme::ORANGE);
    }
    if icon_key(
        ui,
        &g,
        tk(4),
        "rec",
        Cap::Orange,
        p.recording,
        None,
        Icon::Record,
        1.1,
        None,
        None,
    )
    .on_hover_text(tr("Record"))
    .clicked()
    {
        p.recording = !p.recording;
    }
    if icon_key(
        ui,
        &g,
        tk(5),
        "capture",
        Cap::Dark,
        false,
        None,
        Icon::Capture,
        1.1,
        Some(theme::ORANGE),
        None,
    )
    .on_hover_text(tr("Capture Recording"))
    .clicked()
    {
        s.capture_cmd = true;
    }
    if icon_key(
        ui,
        &g,
        tk(6),
        "cycle",
        Cap::Bone,
        p.cycle,
        Some(p.cycle),
        Icon::Cycle,
        0.85,
        None,
        Some("CYC"),
    )
    .clicked()
    {
        p.cycle = !p.cycle;
    }

    // LCD.
    let bezel = Rect::from_min_max(
        pos2(x(lay.lcd_x), rows.lcd_top),
        pos2(x(lay.lcd_x + lay.lcd_w), rows.lcd_bottom),
    );
    let meters = Meters {
        live: s.cpu_load,
        playback: s.playback_load,
        overloads: s.overloads,
        rt_allocs: s.rt_allocs.ours,
        heaviest: s.heaviest.as_ref(),
    };
    lcd(ui, &g, p, bezel, &meters);

    // MONITOR: the latency chip (Record / Mix mode), Low Latency, Count-in, Click.
    if rows.full {
        hw_transport::bracket(
            &g,
            x(lay.mon_x),
            x(lay.mon_right()),
            rows.bracket_y,
            "MONITOR",
            rows.bracket_size,
        );
    }
    let chip = Rect::from_min_size(
        pos2(x(lay.mon_x), rows.key_top - 2.0),
        vec2(lay.chip_w, rows.key_h + 4.0),
    );
    if latency_chip(ui, &g, chip, s) {
        crate::menus::apply("Record / Mix Mode", p, s);
    }
    legend(&g, &rows, chip.center().x, lay.chip_w, "LATENCY");
    let mk = |i: usize| key_rect(lay.mon_keys_x() + i as f32 * lay.mon_pitch, lay.mon_key);
    if icon_key(
        ui,
        &g,
        mk(0),
        "lowlat",
        Cap::Bone,
        s.low_latency,
        Some(s.low_latency),
        Icon::Shield,
        1.0,
        None,
        None,
    )
    .on_hover_text(tr("Low Latency Mode"))
    .clicked()
    {
        crate::menus::apply("Low Latency Mode", p, s);
    }
    let (count_in, body) = hw::key(
        ui,
        &g,
        mk(1),
        Id::unique(("control_bar", "countin")),
        Cap::Bone,
        p.count_in,
        Some(p.count_in),
        Legend::Custom,
    );
    let fs = (body.width() * 0.26).clamp(9.0, 12.0);
    hw::text(
        &g,
        body.center(),
        Align2::CENTER_CENTER,
        "1234",
        Face::Bold,
        fs,
        theme::INK,
    );
    let count_in = count_in.on_hover_text(tr("Count-in"));
    if count_in.clicked() {
        p.count_in = !p.count_in;
    }
    if icon_key(
        ui,
        &g,
        mk(2),
        "metronome",
        Cap::Bone,
        p.metronome,
        Some(p.metronome),
        Icon::Metronome,
        1.0,
        None,
        None,
    )
    .on_hover_text(tr("Metronome"))
    .clicked()
    {
        p.metronome = !p.metronome;
    }
    for (i, label) in ["LOW LAT", "COUNT IN", "CLICK"].iter().enumerate() {
        legend(&g, &rows, mk(i).center().x, lay.mon_pitch, label);
    }

    // MAIN LEVEL.
    if let Some((lx, lw)) = lay.level {
        if rows.full {
            hw_transport::bracket(
                &g,
                x(lx),
                x(lx + lw),
                rows.bracket_y,
                "MAIN LEVEL",
                rows.bracket_size,
            );
        }
        let r = if rows.full {
            key_rect(lx, lw)
        } else {
            Rect::from_min_size(
                pos2(x(lx), rows.key_top + rows.key_h / 2.0 - 11.0),
                vec2(lw, 22.0),
            )
        };
        master_volume(
            ui,
            &g,
            p,
            r,
            master_peaks,
            Id::unique(("control_bar", "master")),
        );
        // The scale's legends sit under their ticks: -∞ at the left end, 0 dB, +6 at the
        // right end.
        let y = rows.legend_y;
        let c = theme::PRINT;
        let size = rows.legend_size;
        let tick = |f: f32| (r.left() + f * lw).round() + 0.5;
        if !rows.full {
            let st = Stroke::new(1.0, theme::INK2.gamma_multiply(0.6));
            for f in [0.0, ZERO_AT, 1.0] {
                g.vline(tick(f), r.bottom() + 1.0..=r.bottom() + 4.0, st);
            }
        }
        hw::silk(&g, pos2(r.left(), y), Align2::LEFT_CENTER, "-∞", size, c);
        hw::silk(
            &g,
            pos2(tick(ZERO_AT), y),
            Align2::CENTER_CENTER,
            "0",
            size,
            c,
        );
        hw::silk(&g, pos2(r.right(), y), Align2::RIGHT_CENTER, "+6", size, c);
    }

    // WINDOW: List Editors, Note Pad, Loop Browser, Browsers in a 2 × 2 block.
    if rows.full {
        hw_transport::bracket(
            &g,
            x(lay.win_x),
            x(lay.win_right()),
            rows.bracket_y,
            "WINDOW",
            rows.bracket_size,
        );
    }
    let wgap = if rows.full { 6.0 } else { 4.0 };
    let wh = (rows.key_h - wgap) / 2.0;
    let win = [
        (Icon::List, s.show_lists, tr("Show List Editors")),
        (Icon::NotePad, s.show_notes, tr("Show Project Notes")),
        (Icon::Loops, s.browser.open == Some(Tab::Loops), tr("Loops")),
        (
            Icon::Windows,
            s.browser.open == Some(Tab::Files),
            tr("Show Media Browser"),
        ),
    ];
    for (i, (ic, on, tip)) in win.into_iter().enumerate() {
        let (col, row) = (i % 2, i / 2);
        let r = Rect::from_min_size(
            pos2(
                x(lay.win_x + col as f32 * (lay.win_key + lay.win_gap)),
                rows.key_top + row as f32 * (wh + wgap),
            ),
            vec2(lay.win_key, wh),
        );
        let tint = on.then_some(theme::ORANGE_DK);
        if icon_key(
            ui,
            &g,
            r,
            &format!("window{i}"),
            Cap::Bone,
            on,
            None,
            ic,
            1.15,
            tint,
            None,
        )
        .on_hover_text(tip)
        .clicked()
        {
            match i {
                0 => s.show_lists = !s.show_lists,
                1 => s.show_notes = !s.show_notes,
                2 => s.browser.toggle(Tab::Loops),
                _ => s.browser.toggle(Tab::Files),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_groups_fit_side_by_side_from_a_laptop_window_up() {
        for width in [1280.0, 1470.0, 1728.0, 2000.0, 2400.0, 3000.0] {
            for n in [5, 6] {
                let l = Layout::of(width, n);
                let keys = l.transport_keys();
                let tr_right = keys[6].0 + keys[6].1;
                let mut edges = vec![
                    (l.view_x, l.view_right()),
                    (l.tr_x, tr_right),
                    (l.lcd_x, l.lcd_x + l.lcd_w),
                    (l.mon_x, l.mon_right()),
                ];
                if let Some((x, w)) = l.level {
                    edges.push((x, x + w));
                }
                edges.push((l.win_x, l.win_right()));
                for pair in edges.windows(2) {
                    assert!(pair[0].1 + 8.0 <= pair[1].0, "{width} {n}: {pair:?}");
                }
                // Clear of the corner screws (about 12 points from each edge) by 8 points.
                assert!(edges[0].0 >= 19.0, "{width}");
                assert!(
                    edges.last().unwrap().1 <= width - 19.0,
                    "{width} {n}: {edges:?}"
                );
            }
        }
        // A MacBook Air window keeps the main level and a readable LCD.
        let air = Layout::of(1470.0, 5);
        assert!(air.level.is_some());
        assert!(air.lcd_w >= LCD_WANTS, "{}", air.lcd_w);
        // The mockup's width gets (nearly) the mockup's LCD, and wider windows all of it.
        assert!(Layout::of(2400.0, 5).lcd_w >= 760.0);
        assert_eq!(Layout::of(2600.0, 5).lcd_w, LCD_MAX);
        // A small window keeps the main level too: the keys get smaller first.
        let small = Layout::of(1280.0, 6);
        assert!(small.level.is_some());
        assert!(small.lcd_w >= LCD_WANTS, "{}", small.lcd_w);
    }

    /// The LCD plan for a glass `w` × `h` points.
    fn plan(w: f32, h: f32) -> LcdPlan {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut out = None;
        for _ in 0..2 {
            let mut o = ctx.run_ui(Default::default(), |ui| {
                let glass = Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h));
                out = Some(LcdPlan::new(ui.painter(), glass));
            });
            o.textures_delta.clear();
        }
        out.unwrap()
    }

    #[test]
    fn the_lcd_keeps_every_readout_and_a_large_bar_position() {
        // The mockup's glass: four columns at the mockup's sizes.
        let wide = plan(776.0, 108.0);
        assert_eq!(wide.arr, Arrangement::Columns);
        assert!((wide.pos.1 - 44.0).abs() < 0.5, "{}", wide.pos.1);
        // A MacBook Air's and a 1280-point window's glass: the readouts stacked, the cycle
        // locators kept, the bar position at least a third of the glass and the small
        // readouts at least 11 points.
        for w in [427.0, 560.0] {
            let p = plan(w, 95.0);
            assert_eq!(p.arr, Arrangement::Stacked, "{w}");
            assert!(p.cycle.is_some(), "{w}");
            assert!(p.pos.1 >= 28.0, "{w}: {}", p.pos.1);
            assert!(p.smpte.1 >= 11.0 && p.tempo.1 >= 11.0, "{w}");
            // Everything stays on the glass.
            let (l, _, _) = p.cycle.unwrap();
            assert!(
                l.x + hw::seg_width(W_POS, p.cycle.unwrap().2) <= p.status.left(),
                "{w}"
            );
            assert!(
                p.sig.0.x + hw::seg_width(W_SIG, p.sig.1) <= p.status.left(),
                "{w}"
            );
            assert!(p.smpte.0.y >= 6.0, "{w}");
        }
        // Narrower still (a 1280-point window): the cycle locators go, the bar position stays
        // large.
        let narrow = plan(380.0, 95.0);
        assert!(narrow.pos.1 >= 28.0, "{}", narrow.pos.1);
        // The compact bar's low glass.
        let low = plan(560.0, 41.0);
        assert_eq!(low.arr, Arrangement::Compact);
        assert!(low.lab >= 9.0);
    }

    #[test]
    fn the_main_level_scale_runs_from_silence_to_plus_six() {
        assert_eq!(level_gain(0.0), 0.0);
        assert!((level_gain(ZERO_AT) - 1.0).abs() < 1e-4);
        assert!((amp_db(level_gain(1.0)) - 6.0).abs() < 1e-3);
        for db in [-48.0f32, -12.0, -3.0, 0.0, 2.5, 6.0] {
            let back = amp_db(level_gain(level_pos(db)));
            assert!((back - db).abs() < 1e-3, "{db} -> {back}");
        }
        assert_eq!(level_pos(f32::NEG_INFINITY), 0.0);
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Runs the tempo field for one frame with `events`; returns what it reported.
    fn frame(
        ctx: &egui::Context,
        text: &mut String,
        events: Vec<egui::Event>,
    ) -> Option<Option<f32>> {
        let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(84.0, 40.0));
        let id = egui::Id::unique("tempo test");
        let mut got = None;
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| got = tempo_field(ui.ctx(), rect, id, text));
        out.textures_delta.clear();
        got
    }

    #[test]
    fn typing_a_tempo_replaces_it_and_enter_applies_it() {
        let ctx = egui::Context::default();
        let mut text = String::from("120.00");
        assert_eq!(frame(&ctx, &mut text, vec![]), None, "opens, still typing");
        // The old value is selected, so typing replaces it.
        let typed = vec![egui::Event::Text("93.5".into())];
        assert_eq!(frame(&ctx, &mut text, typed), None);
        assert_eq!(text, "93.5");
        assert_eq!(
            frame(&ctx, &mut text, vec![key(egui::Key::Enter)]),
            Some(Some(93.5))
        );
    }

    #[test]
    fn escape_or_nonsense_leaves_the_tempo_alone() {
        let ctx = egui::Context::default();
        let mut text = String::from("120.00");
        frame(&ctx, &mut text, vec![]);
        frame(&ctx, &mut text, vec![egui::Event::Text("77".into())]);
        assert_eq!(
            frame(&ctx, &mut text, vec![key(egui::Key::Escape)]),
            Some(None)
        );
        let ctx = egui::Context::default();
        let mut text = String::from("120.00");
        frame(&ctx, &mut text, vec![]);
        frame(&ctx, &mut text, vec![egui::Event::Text("fast".into())]);
        assert_eq!(
            frame(&ctx, &mut text, vec![key(egui::Key::Enter)]),
            Some(None)
        );
    }

    #[test]
    fn slow_drags_add_up_to_whole_steps() {
        let mut left = 0.0;
        let total: i32 = (0..25).map(|_| take_steps(&mut left, 1.0, 10.0)).sum();
        assert_eq!(total, 2);
        assert!((left - 5.0).abs() < 1e-4);

        // Dragging down gives negative steps, and a reversal cancels what was built up.
        let mut left = 0.0;
        assert_eq!(take_steps(&mut left, -25.0, 10.0), -2);
        assert_eq!(take_steps(&mut left, 25.0, 10.0), 2);
        assert!(left.abs() < 1e-4);
    }

    /// Pictures of the control bar (`PROWAVE_UI_PNG_DIR`, else the temp folder): the full
    /// layout at a MacBook Air's width and the mockup's, stopped and playing, and the compact
    /// layout the bar gets when it is lower.
    #[test]
    #[ignore = "pictures"]
    fn pictures_control_bar() {
        let dir = std::env::var_os("PROWAVE_UI_PNG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("prowave-transport"));
        let _ = std::fs::create_dir_all(&dir);
        for (w, h, playing, name) in [
            (1470.0, HEIGHT, false, "control-bar-1470.png"),
            (1470.0, HEIGHT, true, "control-bar-1470-playing.png"),
            (2400.0, 134.0, true, "control-bar-2400.png"),
            (1470.0, 58.0, false, "control-bar-compact-1470.png"),
            (1280.0, HEIGHT, false, "control-bar-1280.png"),
            (1728.0, HEIGHT, false, "control-bar-1728.png"),
        ] {
            let mut p = Project::demo();
            let mut s = crate::UiState::new();
            p.playing = playing;
            p.cycle = playing;
            p.position_beats = 16.0;
            s.show_inspector = true;
            s.show_mixer = true;
            s.low_latency = playing;
            s.round_trip_ms = 2.7;
            s.cpu_load = 0.3;
            crate::ui_test::render_png(
                w,
                h,
                2.0,
                theme::BONE,
                |ui| {
                    theme::apply(ui.ctx());
                    control_bar(ui, &mut p, &mut s, Some((0.55, 0.4)))
                },
                &dir.join(name),
            )
            .unwrap();
        }
    }
}
