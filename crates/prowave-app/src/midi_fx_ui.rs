//! The MIDI FX window (Track > MIDI FX…) as a PW-1 hardware rack: the chain of effects of the
//! selected track, each one its own rack unit (an ON key, keys to move it up or down or take it
//! out, its settings as knobs, keys and menu fields with LCD readouts, and a small LCD showing
//! what it does). Changes bump the song revision so the engine gets the new notes.
use crate::gear::{self, Cap, Control, LedAt};
use crate::i18n::{tr, trf};
use crate::midi_fx::{
    ArpPattern, ChordKind, FxSlot, LfoShape, MidiFx, ModTarget, MAX_FX, MOD_PERIODS,
};
use crate::model::Project;
use crate::pattern::StepRate;
use crate::step_editor::NOTE_NAMES;
use crate::UiState;
use egui::{pos2, vec2, Align2, Color32, Id, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui};

fn title(fx: &MidiFx) -> &'static str {
    match fx {
        MidiFx::Scripter { .. } => tr("Scripter"),
        MidiFx::Modulator { .. } => tr("Modulator"),
        MidiFx::Transposer { .. } => tr("Transposer"),
        MidiFx::Velocity { .. } => tr("Velocity Processor"),
        MidiFx::Scale { .. } => tr("Scale Quantizer"),
        MidiFx::Chord { .. } => tr("Chord Trigger"),
        MidiFx::Repeater { .. } => tr("Note Repeater"),
        MidiFx::Arpeggiator { .. } => tr("Arpeggiator"),
        MidiFx::Randomizer { .. } => tr("Randomizer"),
    }
}

fn shape_name(s: LfoShape) -> &'static str {
    match s {
        LfoShape::Sine => tr("Sine"),
        LfoShape::Triangle => tr("Triangle"),
        LfoShape::Saw => tr("Saw"),
        LfoShape::Square => tr("Square"),
        LfoShape::Random => tr("Random"),
    }
}

/// A Modulator's cycle length as text: quarter notes, or bars of four when longer.
fn period_text(i: usize) -> String {
    let beats = MOD_PERIODS[i.min(MOD_PERIODS.len() - 1)];
    if beats >= 4.0 {
        trf("{} bars", &[&(beats / 4.0)])
    } else {
        trf("{} beats", &[&beats])
    }
}

fn chord_name(k: ChordKind) -> &'static str {
    match k {
        ChordKind::Major => tr("Major"),
        ChordKind::Minor => tr("Minor"),
        ChordKind::Seventh => tr("Dominant 7th"),
        ChordKind::MajorSeventh => tr("Major 7th"),
        ChordKind::MinorSeventh => tr("Minor 7th"),
        ChordKind::Sus4 => tr("Sus4"),
        ChordKind::Fifth => tr("Power Chord"),
        ChordKind::Octave => tr("Octave"),
    }
}

fn pattern_name(p: ArpPattern) -> &'static str {
    match p {
        ArpPattern::Up => tr("Up"),
        ArpPattern::Down => tr("Down"),
        ArpPattern::UpDown => tr("Up and Down"),
        ArpPattern::Random => tr("Random"),
        ArpPattern::AsPlayed => tr("As Played"),
    }
}

/// The window's width and its inner margin.
const W: f32 = 780.0;
const PAD: f32 = 24.0;
/// A rack unit's header strip and its control row.
const UNIT_HEAD: f32 = 40.0;
const SCRIPT_H: f32 = 236.0;
/// The LCD at a unit's right end.
const LCD_W: f32 = 230.0;

/// A label as the silkscreen prints it: the existing text without its colon.
fn silk_label(t: &str) -> String {
    t.trim_end_matches(':').trim().to_string()
}

/// A whole-number knob control.
fn int_control(
    label: &str,
    v: i32,
    min: i32,
    max: i32,
    default: i32,
    text: String,
    on: bool,
) -> Control {
    let span = (max - min).max(1) as f32;
    Control {
        label: silk_label(label),
        norm: (v - min) as f32 / span,
        default: (default - min) as f32 / span,
        text,
        choices: Vec::new(),
        choice: 0,
        automated: None,
        enabled: on,
    }
}

/// A choice control (keys for up to three, else a menu field).
fn choice_control(label: &str, names: Vec<String>, current: usize, on: bool) -> Control {
    Control {
        label: silk_label(label),
        norm: 0.0,
        default: 0.0,
        text: String::new(),
        choices: names,
        choice: current,
        automated: None,
        enabled: on,
    }
}

/// A knob's new whole value from a change.
fn int_of(ch: &gear::Change, min: i32, max: i32) -> i32 {
    (min as f32 + ch.norm * (max - min) as f32).round() as i32
}

/// Lays `cells` (control, width) out in a row from `at`; returns (cell index, change).
fn row(ui: &mut Ui, at: Pos2, id: Id, cells: &[(Control, f32)]) -> Vec<(usize, gear::Change)> {
    let mut out = Vec::new();
    let mut x = at.x;
    for (k, (c, w)) in cells.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, at.y), vec2(*w, gear::CELL_H));
        if let Some(ch) = gear::control_cell(ui, r, id, k, c, 44.0) {
            out.push((k, ch));
        }
        x += w;
    }
    out
}

/// The height of a unit's control area.
fn controls_h(fx: &MidiFx) -> f32 {
    match fx {
        MidiFx::Scripter { .. } => SCRIPT_H,
        _ => gear::CELL_H + 8.0,
    }
}

/// A key with an up or down arrow; dimmed and inert when `enabled` is off.
fn arrow_key(ui: &mut Ui, rect: Rect, id: Id, up: bool, enabled: bool) -> bool {
    let resp = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let p = ui.painter();
    let face = gear::paint_key(
        p,
        rect,
        Cap::Bone,
        enabled && resp.is_pointer_button_down_on(),
    );
    let c = face.center();
    let col = if enabled {
        gear::INK
    } else {
        gear::alpha(gear::INK, 0.3)
    };
    let pts = if up {
        vec![c + vec2(-5.0, 3.0), c + vec2(0.0, -4.0), c + vec2(5.0, 3.0)]
    } else {
        vec![
            c + vec2(-5.0, -3.0),
            c + vec2(5.0, -3.0),
            c + vec2(0.0, 4.0),
        ]
    };
    p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
    enabled && resp.clicked()
}

/// LCD ink.
fn ink(a: f32) -> Color32 {
    gear::alpha(gear::LCD_INK, a)
}

/// The unit's LCD: what the effect does, drawn small.
fn display(p: &Painter, rect: Rect, fx: &MidiFx, on: bool) {
    let glass = gear::lcd(p, rect);
    let plot = glass.shrink2(vec2(12.0, 12.0));
    let a = if on { 1.0 } else { 0.35 };
    let st = Stroke::new(2.0, ink(a));
    let faint = Stroke::new(1.0, ink(0.12));
    match fx {
        MidiFx::Scripter { .. } => {}
        MidiFx::Modulator {
            shape,
            depth,
            target,
            ..
        } => {
            // One cycle of the shape at its depth.
            let amp = f32::from(*depth) / 100.0;
            let mid = plot.center().y;
            p.line_segment([pos2(plot.left(), mid), pos2(plot.right(), mid)], faint);
            let n = 96;
            let y = |t: f32| -> f32 {
                match shape {
                    LfoShape::Sine => (std::f32::consts::TAU * t).sin(),
                    LfoShape::Triangle => 1.0 - 4.0 * ((t + 0.25).rem_euclid(1.0) - 0.5).abs(),
                    LfoShape::Saw => 1.0 - 2.0 * t,
                    LfoShape::Square => {
                        if t < 0.5 {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    LfoShape::Random => {
                        [0.6, -0.3, 0.9, -0.8, 0.1, -0.5, 0.4, -0.1][((t * 8.0) as usize).min(7)]
                    }
                }
            };
            let pts: Vec<Pos2> = (0..=n)
                .map(|i| {
                    let t = i as f32 / n as f32;
                    pos2(
                        plot.left() + t * plot.width(),
                        mid - y(t) * amp * plot.height() / 2.0,
                    )
                })
                .collect();
            p.add(Shape::line(pts, st));
            if matches!(target, ModTarget::Controller { .. }) {
                gear::lcd_label(p, glass.left_top() + vec2(6.0, 4.0), Align2::LEFT_TOP, "CC");
            }
        }
        MidiFx::Transposer { semitones } => {
            let t = format!("{semitones}");
            let h = 40.0;
            let w = gear::seg7_width(&t, h);
            gear::seg7(
                p,
                pos2(
                    glass.center().x - w / 2.0 - 12.0,
                    glass.center().y - h / 2.0,
                ),
                &t,
                h,
                ink(a),
                true,
            );
            gear::lcd_text(
                p,
                pos2(glass.right() - 10.0, glass.center().y + h / 2.0),
                Align2::RIGHT_BOTTOM,
                "ST",
                12.0,
            );
        }
        MidiFx::Velocity { add, scale } => {
            // Velocity out over velocity in.
            p.line_segment([plot.left_bottom(), plot.right_top()], faint);
            let pts: Vec<Pos2> = (1..=127)
                .map(|v| {
                    let out =
                        (v as f32 * f32::from(*scale) / 100.0 + f32::from(*add)).clamp(1.0, 127.0);
                    pos2(
                        plot.left() + (v - 1) as f32 / 126.0 * plot.width(),
                        plot.bottom() - (out - 1.0) / 126.0 * plot.height(),
                    )
                })
                .collect();
            p.add(Shape::line(pts, st));
        }
        MidiFx::Scale { root, minor } => {
            let steps: [i32; 7] = if *minor {
                [0, 2, 3, 5, 7, 8, 10]
            } else {
                [0, 2, 4, 5, 7, 9, 11]
            };
            note_strip(
                p,
                plot,
                |n| {
                    steps
                        .iter()
                        .any(|s| (s + i32::from(*root)).rem_euclid(12) == n)
                },
                Some(i32::from(*root) % 12),
                a,
            );
        }
        MidiFx::Chord { kind } => {
            let ivs = kind.intervals();
            note_strip(
                p,
                plot,
                |n| n == 0 || ivs.iter().any(|i| i.rem_euclid(12) == n),
                Some(0),
                a,
            );
        }
        MidiFx::Repeater { rate } => {
            let n = (4.0 / rate.beats()).round().clamp(1.0, 32.0) as usize;
            steps(p, plot, n, |_| 0.5, a);
        }
        MidiFx::Arpeggiator {
            rate,
            pattern,
            octaves,
            gate,
        } => {
            let n = ((4.0 / rate.beats()).round() as usize).clamp(4, 16);
            let o = usize::from((*octaves).max(1));
            let notes = 3 * o;
            let level = |k: usize| -> f32 {
                let up = (k % notes) as f32 / (notes - 1).max(1) as f32;
                match pattern {
                    ArpPattern::Up => up,
                    ArpPattern::Down => 1.0 - up,
                    ArpPattern::UpDown => {
                        let cyc = (2 * notes - 2).max(1);
                        let j = k % cyc;
                        let j = if j < notes { j } else { cyc - j };
                        j as f32 / (notes - 1).max(1) as f32
                    }
                    ArpPattern::Random => [0.4, 0.9, 0.1, 0.7, 0.3, 1.0, 0.0, 0.6][k % 8],
                    ArpPattern::AsPlayed => [0.0, 0.7, 0.35][k % 3],
                }
            };
            let g = f32::from(*gate) / 100.0;
            let w = plot.width() / n as f32;
            for k in 0..n {
                let y = plot.bottom() - level(k) * (plot.height() - 6.0);
                let r = Rect::from_min_size(
                    pos2(plot.left() + k as f32 * w + 1.0, y - 6.0),
                    vec2((w - 2.0) * g.max(0.15), 6.0),
                );
                p.rect_filled(r, 0.0, ink(a));
            }
        }
        MidiFx::Randomizer {
            pitch,
            velocity,
            timing,
            ..
        } => {
            // A scatter of notes, spread by the three amounts.
            let (sp, sv, st2) = (
                f32::from(*pitch) / 24.0,
                f32::from(*velocity) / 127.0,
                f32::from(*timing) / 100.0,
            );
            let jit = [
                0.3f32, -0.8, 0.6, -0.2, 0.9, -0.5, 0.1, -0.9, 0.7, -0.4, 0.5, -0.6,
            ];
            let n = 12;
            let w = plot.width() / n as f32;
            for k in 0..n {
                let x = plot.left() + (k as f32 + 0.5) * w + jit[(k + 3) % 12] * st2 * w * 0.45;
                let y = plot.center().y - jit[k] * sp * plot.height() * 0.42;
                let len = w * 0.6 * (1.0 - 0.6 * sv * (jit[(k + 5) % 12] + 1.0) / 2.0);
                p.rect_filled(
                    Rect::from_center_size(pos2(x, y), vec2(len.max(3.0), 5.0)),
                    0.0,
                    ink(a),
                );
            }
        }
    }
}

/// An octave of keys on the LCD, the notes where `lit(n)` (C = 0) marked: a dot on a white
/// key, a filled black key; the `root` underlined.
fn note_strip(p: &Painter, plot: Rect, lit: impl Fn(i32) -> bool, root: Option<i32>, a: f32) {
    const WHITE: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    const BLACK: [(i32, f32); 5] = [(1, 1.0), (3, 2.0), (6, 4.0), (8, 5.0), (10, 6.0)];
    let kb = plot.shrink2(vec2(10.0, 0.0));
    let w = kb.width() / 7.0;
    let line = Stroke::new(1.2, ink(0.6 * a));
    for (i, n) in WHITE.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(kb.left() + i as f32 * w, kb.top()),
            vec2(w, kb.height() - 6.0),
        );
        p.rect_stroke(r, 0.0, line, egui::StrokeKind::Inside);
        if lit(*n) {
            p.circle_filled(
                pos2(r.center().x, r.bottom() - 9.0),
                (w * 0.22).min(5.0),
                ink(a),
            );
        }
        if root == Some(*n) {
            p.line_segment(
                [
                    pos2(r.left() + 3.0, kb.bottom()),
                    pos2(r.right() - 3.0, kb.bottom()),
                ],
                Stroke::new(2.0, ink(a)),
            );
        }
    }
    for (n, at) in BLACK {
        let bw = w * 0.6;
        let r = Rect::from_min_size(
            pos2(kb.left() + at * w - bw / 2.0, kb.top()),
            vec2(bw, (kb.height() - 6.0) * 0.58),
        );
        p.rect_filled(r, 0.0, gear::LCD);
        p.rect_stroke(r, 0.0, line, egui::StrokeKind::Inside);
        if lit(n) {
            p.rect_filled(r.shrink(2.0), 0.0, ink(a));
        }
        if root == Some(n) {
            p.line_segment(
                [
                    pos2(r.left(), r.bottom() + 3.0),
                    pos2(r.right(), r.bottom() + 3.0),
                ],
                Stroke::new(2.0, ink(a)),
            );
        }
    }
}

/// `n` steps across the LCD, each as long as `len(k)` of its slot.
fn steps(p: &Painter, plot: Rect, n: usize, len: impl Fn(usize) -> f32, a: f32) {
    let w = plot.width() / n as f32;
    for k in 0..n {
        let r = Rect::from_min_size(
            pos2(plot.left() + k as f32 * w + 1.0, plot.center().y - 8.0),
            vec2(((w - 2.0) * len(k)).max(2.0), 16.0),
        );
        p.rect_filled(r, 0.0, ink(a));
    }
}

/// The settings of one effect in the controls area `rect`; returns whether any changed.
fn settings(ui: &mut Ui, rect: Rect, fx: &mut MidiFx, id: Id, on: bool) -> bool {
    let mut changed = false;
    let at = rect.left_top() + vec2(0.0, 4.0);
    let rate_names = || {
        StepRate::ALL
            .iter()
            .map(|r| r.name().to_string())
            .collect::<Vec<_>>()
    };
    let rate_index = |r: StepRate| StepRate::ALL.iter().position(|x| *x == r).unwrap_or(0);
    match fx {
        MidiFx::Scripter { source } => {
            let editor = Rect::from_min_max(
                rect.min + vec2(4.0, 6.0),
                pos2(rect.right() - 4.0, rect.bottom() - 46.0),
            );
            gear::well(ui.painter(), editor, 3.0);
            let edit = egui::TextEdit::multiline(source)
                .code_editor()
                .frame(egui::Frame::NONE)
                .text_color(crate::theme::ON_DARK)
                .desired_rows(9)
                .desired_width(editor.width() - 16.0)
                .char_limit(crate::scripter::MAX_SCRIPT_CHARS);
            changed |= ui
                .add_enabled_ui(on, |ui| ui.put(editor.shrink(8.0), edit))
                .inner
                .changed();
            let status = Rect::from_min_max(
                pos2(rect.left() + 4.0, rect.bottom() - 36.0),
                pos2(rect.right() - 130.0, rect.bottom() - 6.0),
            );
            let p = ui.painter().clone();
            gear::glass(&p, status);
            let msg = match crate::scripter::check(source) {
                Ok(()) => tr("The script works").to_string(),
                Err(e) => e,
            };
            let shown = gear::fit(&p, &msg, 11.0, status.width() - 20.0);
            gear::lcd_text(
                &p,
                pos2(status.left() + 10.0, status.center().y),
                Align2::LEFT_CENTER,
                &shown,
                11.0,
            );
            let r = Rect::from_min_max(
                pos2(rect.right() - 118.0, rect.bottom() - 36.0),
                pos2(rect.right() - 4.0, rect.bottom() - 8.0),
            );
            if on
                && gear::key(
                    ui,
                    r,
                    id.with("start_over"),
                    tr("Start Over"),
                    Cap::Bone,
                    LedAt::None,
                    false,
                    false,
                )
                .clicked()
            {
                *source = crate::scripter::DEFAULT_SCRIPT.to_string();
                changed = true;
            }
        }
        MidiFx::Modulator {
            shape,
            period,
            depth,
            target,
            seed,
        } => {
            let shapes: Vec<String> = LfoShape::ALL
                .iter()
                .map(|s| shape_name(*s).to_string())
                .collect();
            let periods: Vec<String> = (0..MOD_PERIODS.len()).map(period_text).collect();
            let targets = vec![
                tr("Velocity").to_string(),
                tr("Pitch").to_string(),
                tr("Controller").to_string(),
            ];
            let ti = match target {
                ModTarget::Velocity => 0,
                ModTarget::Pitch => 1,
                ModTarget::Controller { .. } => 2,
            };
            let si = LfoShape::ALL.iter().position(|s| s == shape).unwrap_or(0);
            let mut cells = vec![
                (choice_control(tr("Shape:"), shapes, si, on), 128.0),
                (
                    choice_control(tr("Cycle:"), periods, *period as usize, on),
                    120.0,
                ),
                (
                    int_control(
                        tr("Depth:"),
                        i32::from(*depth),
                        0,
                        100,
                        50,
                        format!("{depth} %"),
                        on,
                    ),
                    gear::CELL_W,
                ),
                (choice_control(tr("Moves:"), targets, ti, on), 120.0),
            ];
            if let ModTarget::Controller { number } = target {
                cells.push((
                    int_control(
                        tr("Number:"),
                        i32::from(*number),
                        0,
                        127,
                        74,
                        format!("{number}"),
                        on,
                    ),
                    gear::CELL_W,
                ));
            }
            for (k, ch) in row(ui, at, id, &cells) {
                match k {
                    0 => *shape = LfoShape::ALL[ch.choice.unwrap_or(si).min(4)],
                    1 => *period = ch.choice.unwrap_or(*period as usize) as u8,
                    2 => *depth = int_of(&ch, 0, 100) as u8,
                    3 => {
                        *target = match ch.choice {
                            Some(0) => ModTarget::Velocity,
                            Some(1) => ModTarget::Pitch,
                            _ => ModTarget::Controller { number: 74 },
                        }
                    }
                    _ => {
                        if let ModTarget::Controller { number } = target {
                            *number = int_of(&ch, 0, 127) as u8;
                        }
                    }
                }
                changed = true;
            }
            let free = rect.right() - LCD_W - 16.0 - 132.0;
            let mut y = rect.top() + 26.0;
            if *shape == LfoShape::Random {
                let r = Rect::from_min_size(pos2(free, y), vec2(124.0, 28.0));
                if on
                    && gear::key(
                        ui,
                        r,
                        id.with("variation"),
                        tr("New Variation"),
                        Cap::Bone,
                        LedAt::None,
                        false,
                        false,
                    )
                    .clicked()
                {
                    *seed = seed.wrapping_add(1);
                    changed = true;
                }
                y += 40.0;
            }
            if matches!(target, ModTarget::Controller { .. }) {
                let p = ui.painter();
                let t = gear::fit(
                    p,
                    &tr("Sent to External MIDI tracks").to_uppercase(),
                    10.0,
                    124.0,
                );
                gear::silk(
                    p,
                    pos2(free, y + 8.0),
                    Align2::LEFT_CENTER,
                    &t,
                    10.0,
                    gear::PRINT,
                );
            }
        }
        MidiFx::Transposer { semitones } => {
            let cells = [(
                int_control(
                    tr("Semitones:"),
                    i32::from(*semitones),
                    -48,
                    48,
                    0,
                    format!("{semitones} st"),
                    on,
                ),
                gear::CELL_W + 20.0,
            )];
            for (_, ch) in row(ui, at, id, &cells) {
                *semitones = int_of(&ch, -48, 48) as i8;
                changed = true;
            }
        }
        MidiFx::Velocity { add, scale } => {
            let cells = [
                (
                    int_control(
                        tr("Add:"),
                        i32::from(*add),
                        -127,
                        127,
                        0,
                        format!("{add}"),
                        on,
                    ),
                    gear::CELL_W,
                ),
                (
                    int_control(
                        tr("Scale:"),
                        i32::from(*scale),
                        10,
                        200,
                        100,
                        format!("{scale} %"),
                        on,
                    ),
                    gear::CELL_W,
                ),
            ];
            for (k, ch) in row(ui, at, id, &cells) {
                if k == 0 {
                    *add = int_of(&ch, -127, 127) as i8;
                } else {
                    *scale = int_of(&ch, 10, 200) as u8;
                }
                changed = true;
            }
        }
        MidiFx::Scale { root, minor } => {
            let names: Vec<String> = NOTE_NAMES.iter().map(|n| n.to_string()).collect();
            let modes = vec![tr("Major").to_string(), tr("Minor").to_string()];
            let cells = [
                (
                    choice_control(tr("Root:"), names, *root as usize % 12, on),
                    110.0,
                ),
                (choice_control("", modes, usize::from(*minor), on), 130.0),
            ];
            for (k, ch) in row(ui, at, id, &cells) {
                if k == 0 {
                    *root = ch.choice.unwrap_or(0) as u8;
                } else {
                    *minor = ch.choice == Some(1);
                }
                changed = true;
            }
        }
        MidiFx::Chord { kind } => {
            let names: Vec<String> = ChordKind::ALL
                .iter()
                .map(|k| chord_name(*k).to_string())
                .collect();
            let cur = ChordKind::ALL.iter().position(|k| k == kind).unwrap_or(0);
            let cells = [(choice_control(tr("Chord:"), names, cur, on), 170.0)];
            for (_, ch) in row(ui, at, id, &cells) {
                *kind = ChordKind::ALL[ch.choice.unwrap_or(cur).min(ChordKind::ALL.len() - 1)];
                changed = true;
            }
        }
        MidiFx::Repeater { rate } => {
            let cells = [(
                choice_control(tr("Rate:"), rate_names(), rate_index(*rate), on),
                110.0,
            )];
            for (_, ch) in row(ui, at, id, &cells) {
                *rate = StepRate::ALL[ch.choice.unwrap_or(0).min(5)];
                changed = true;
            }
        }
        MidiFx::Arpeggiator {
            rate,
            pattern,
            octaves,
            gate,
        } => {
            let pats: Vec<String> = ArpPattern::ALL
                .iter()
                .map(|p| pattern_name(*p).to_string())
                .collect();
            let pi = ArpPattern::ALL
                .iter()
                .position(|p| p == pattern)
                .unwrap_or(0);
            let cells = [
                (
                    choice_control(tr("Rate:"), rate_names(), rate_index(*rate), on),
                    104.0,
                ),
                (choice_control(tr("Pattern:"), pats, pi, on), 140.0),
                (
                    int_control(
                        tr("Octaves:"),
                        i32::from(*octaves),
                        1,
                        4,
                        1,
                        format!("{octaves}"),
                        on,
                    ),
                    gear::CELL_W,
                ),
                (
                    int_control(
                        tr("Gate:"),
                        i32::from(*gate),
                        5,
                        100,
                        80,
                        format!("{gate} %"),
                        on,
                    ),
                    gear::CELL_W,
                ),
            ];
            for (k, ch) in row(ui, at, id, &cells) {
                match k {
                    0 => *rate = StepRate::ALL[ch.choice.unwrap_or(0).min(5)],
                    1 => *pattern = ArpPattern::ALL[ch.choice.unwrap_or(0).min(4)],
                    2 => *octaves = int_of(&ch, 1, 4) as u8,
                    _ => *gate = int_of(&ch, 5, 100) as u8,
                }
                changed = true;
            }
        }
        MidiFx::Randomizer {
            pitch,
            velocity,
            timing,
            seed,
        } => {
            let cells = [
                (
                    int_control(
                        tr("Pitch:"),
                        i32::from(*pitch),
                        0,
                        24,
                        0,
                        format!("{pitch} st"),
                        on,
                    ),
                    gear::CELL_W,
                ),
                (
                    int_control(
                        tr("Velocity:"),
                        i32::from(*velocity),
                        0,
                        127,
                        0,
                        format!("{velocity}"),
                        on,
                    ),
                    gear::CELL_W,
                ),
                (
                    int_control(
                        tr("Timing:"),
                        i32::from(*timing),
                        0,
                        100,
                        0,
                        format!("{timing} %"),
                        on,
                    ),
                    gear::CELL_W,
                ),
            ];
            for (k, ch) in row(ui, at, id, &cells) {
                match k {
                    0 => *pitch = int_of(&ch, 0, 24) as u8,
                    1 => *velocity = int_of(&ch, 0, 127) as u8,
                    _ => *timing = int_of(&ch, 0, 100) as u8,
                }
                changed = true;
            }
            let r = Rect::from_min_size(
                pos2(at.x + 3.0 * gear::CELL_W + 14.0, rect.top() + 26.0),
                vec2(124.0, 28.0),
            );
            if on
                && gear::key(
                    ui,
                    r,
                    id.with("variation"),
                    tr("New Variation"),
                    Cap::Bone,
                    LedAt::None,
                    false,
                    false,
                )
                .clicked()
            {
                *seed = seed.wrapping_add(1);
                changed = true;
            }
        }
    }
    changed
}

/// Draws the window when it is open.
pub fn show(ctx: &egui::Context, p: &mut Project, s: &mut UiState) {
    if !s.show_midi_fx {
        return;
    }
    let ti = p.selected_track;
    let Some(name) = p.tracks.get(ti).map(|t| t.name.clone()) else {
        return;
    };
    let mut changed = false;
    let title_text = trf("MIDI FX: {}", &[&name]);
    let badge = format!("{:02}", ti + 1);
    let plate = gear::Plate {
        model: "MX-9",
        name: &title_text,
        strip: Some((&badge, &name)),
        reset: false,
    };
    let id = egui::Id::unique("midi_fx_window");
    let chain = &mut p.tracks[ti].midi_fx;
    let (head, _) = gear::unit_window(ctx, id, W, &plate, None, |ui| {
        let units: f32 = chain
            .iter()
            .map(|s| UNIT_HEAD + controls_h(&s.fx) + 14.0)
            .sum();
        let empty_h = if chain.is_empty() { 70.0 } else { 0.0 };
        let total = 18.0 + units + empty_h + 54.0;
        let (rect, _) = ui.allocate_exact_size(vec2(W, total), Sense::hover());
        let painter = ui.painter().clone();
        let left = rect.left() + PAD;
        let right = rect.right() - PAD;
        let mut y = rect.top() + 16.0;
        if chain.is_empty() {
            let r = Rect::from_min_max(pos2(left, y), pos2(right, y + 58.0));
            painter.rect_filled(r, 4.0, gear::BONE2);
            gear::inner_shadow(&painter, r, 4.0, 0.45);
            gear::silk(
                &painter,
                r.center(),
                Align2::CENTER_CENTER,
                tr("No MIDI FX on this track"),
                12.0,
                gear::PRINT,
            );
            y += empty_h;
        }
        let (mut remove, mut up) = (None, None);
        let count = chain.len();
        for (i, slot) in chain.iter_mut().enumerate() {
            let uid = id.with(("unit", i));
            let unit = Rect::from_min_max(
                pos2(left, y),
                pos2(right, y + UNIT_HEAD + controls_h(&slot.fx)),
            );
            crate::hw::faceplate(&painter, unit, 6.0, false);
            // The header strip: number and name plate, ON, up, down, out.
            let hy = unit.top() + 8.0;
            let plate_r =
                Rect::from_min_size(pos2(unit.left() + 12.0, hy + 2.0), vec2(300.0, 22.0));
            gear::name_plate(
                &painter,
                plate_r,
                &format!("{}", i + 1),
                title(&slot.fx),
                false,
            );
            let mut x = unit.right() - 12.0 - 28.0;
            let close = Rect::from_min_size(pos2(x, hy), vec2(28.0, 26.0));
            if gear::glyph_key(
                ui,
                close,
                uid.with("out"),
                Cap::Dark,
                false,
                gear::Glyph::Close,
            )
            .clicked()
            {
                remove = Some(i);
            }
            x -= 8.0 + 30.0;
            if arrow_key(
                ui,
                Rect::from_min_size(pos2(x, hy), vec2(30.0, 26.0)),
                uid.with("down"),
                false,
                i + 1 < count,
            ) {
                up = Some(i + 1);
            }
            x -= 6.0 + 30.0;
            if arrow_key(
                ui,
                Rect::from_min_size(pos2(x, hy), vec2(30.0, 26.0)),
                uid.with("up"),
                true,
                i > 0,
            ) {
                up = Some(i);
            }
            x -= 10.0 + 64.0;
            let on_r = Rect::from_min_size(pos2(x, hy), vec2(64.0, 26.0));
            if gear::key(
                ui,
                on_r,
                uid.with("on"),
                "ON",
                Cap::Bone,
                LedAt::Left,
                slot.on,
                slot.on,
            )
            .clicked()
            {
                slot.on = !slot.on;
                changed = true;
            }
            gear::seam_h(
                &painter,
                unit.left() + 1.0,
                unit.right() - 1.0,
                unit.top() + UNIT_HEAD - 2.0,
            );
            // The settings, and the LCD at the right end.
            let ctrl = Rect::from_min_max(
                pos2(unit.left() + 12.0, unit.top() + UNIT_HEAD),
                pos2(unit.right() - 12.0, unit.bottom()),
            );
            let scripter = matches!(slot.fx, MidiFx::Scripter { .. });
            if !scripter {
                let lcd = Rect::from_min_size(
                    pos2(ctrl.right() - LCD_W, ctrl.top() + 16.0),
                    vec2(LCD_W, ctrl.height() - 32.0),
                );
                display(&painter, lcd, &slot.fx, slot.on);
            }
            changed |= settings(ui, ctrl, &mut slot.fx, uid, slot.on);
            y = unit.bottom() + 14.0;
        }
        if let Some(i) = remove {
            chain.remove(i);
            changed = true;
        }
        if let Some(i) = up {
            if i > 0 && i < chain.len() {
                chain.swap(i - 1, i);
                changed = true;
            }
        }
        // Add MIDI FX: a key with the menu of effects (inert while the rack is full).
        let room = chain.len() < MAX_FX;
        let r = Rect::from_min_size(pos2(left, y + 6.0), vec2(170.0, 30.0));
        let resp = if room {
            gear::key(
                ui,
                r,
                id.with("add"),
                tr("Add MIDI FX"),
                Cap::Bone,
                LedAt::None,
                false,
                false,
            )
        } else {
            gear::paint_key(&painter, r, Cap::Bone, true);
            gear::legend(
                &painter,
                r.center() + vec2(0.0, 1.0),
                Align2::CENTER_CENTER,
                tr("Add MIDI FX"),
                11.0,
                gear::alpha(gear::INK, 0.35),
            );
            ui.interact(r, id.with("add"), Sense::hover())
        };
        if room {
            egui::Popup::menu(&resp).show(|ui| {
                for fx in MidiFx::all_defaults() {
                    if ui.button(title(&fx)).clicked() {
                        chain.push(FxSlot { fx, on: true });
                        changed = true;
                        ui.close();
                    }
                }
            });
        }
    });
    if changed {
        p.song_rev += 1;
    }
    if head.close {
        s.show_midi_fx = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_test::Harness;

    #[test]
    fn the_window_draws_every_effect_and_closes() {
        let mut p = Project::demo();
        let mut s = UiState::new();
        s.show_midi_fx = true;
        p.selected_track = 0;
        p.tracks[0].midi_fx = MidiFx::all_defaults()
            .into_iter()
            .map(|fx| FxSlot { fx, on: true })
            .chain([FxSlot {
                fx: MidiFx::Transposer { semitones: 3 },
                on: false,
            }])
            .collect();
        let mut h = Harness::new(900.0, 1400.0);
        let rev = p.song_rev;
        for _ in 0..4 {
            h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s));
        }
        assert!(s.show_midi_fx, "still open");
        assert_eq!(p.song_rev, rev, "looking changes nothing");
        // A track that is not there, and a window that is shut, draw nothing.
        p.selected_track = 999;
        h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s));
        s.show_midi_fx = false;
        h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s));
    }

    #[test]
    fn every_title_and_name_exists_in_english() {
        for fx in MidiFx::all_defaults() {
            assert!(!title(&fx).is_empty());
        }
        for k in ChordKind::ALL {
            assert!(!chord_name(k).is_empty());
        }
        for a in ArpPattern::ALL {
            assert!(!pattern_name(a).is_empty());
        }
    }
}
