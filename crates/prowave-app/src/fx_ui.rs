//! The window of an effect in a channel strip's Audio FX slot (Logic: the plug-in window),
//! built as a PW-1 hardware unit: a bone faceplate with the model and the strip's name plate,
//! an ON key, the unit's display (the curve of the EQs and the filter, the transfer curve and
//! gain-reduction ladder of the dynamics, the echoes of the delay, the sweep of the
//! modulation effects, the drive curve) and a knob for each setting with its LCD readout.
//! Every knob is automatable: turning one while the project plays writes it, and it turns by
//! itself when its lane is played.
use crate::automation;
use crate::gear::{self, Control};
use crate::i18n::{tr, trf};
use crate::mixer::Meters;
use crate::model::{AutoTarget, Insert, Project, Track};
use crate::UiState;
use egui::{pos2, vec2, Align2, Color32, Pos2, Rect, Shape, Stroke, Ui};
use prowave_engine::{FxKind, ParamInfo, AMP_CABS, AMP_MODELS};

/// Sample rate the curve of the EQs is drawn for.
const CURVE_RATE: f32 = 48_000.0;
/// Margin inside the faceplate.
const PAD: f32 = 26.0;
/// Height of the display row.
const DISPLAY_H: f32 = 168.0;

/// A slider the user moved: parameter number, new value, and whether the gesture ended.
pub(crate) struct Edit {
    pub(crate) param: usize,
    pub(crate) value: f32,
    pub(crate) ended: bool,
}

/// The model code on a unit's faceplate.
pub(crate) fn model_code(kind: FxKind) -> &'static str {
    match kind {
        FxKind::ChannelEq => "EQ-3",
        FxKind::MultiEq => "EQ-7",
        FxKind::Compressor => "CP-1",
        FxKind::Gate => "NG-1",
        FxKind::Limiter => "LM-1",
        FxKind::Delay => "DL-2",
        FxKind::Chorus => "CH-1",
        FxKind::Phaser => "PH-4",
        FxKind::Overdrive => "OD-1",
        FxKind::Filter => "FL-1",
        FxKind::Amp => "AM-5",
        FxKind::Reverb => "SD-1",
    }
}

/// The name plate of strip `ti`: its number and name.
pub(crate) fn strip_plate(p: &Project, ti: usize) -> (String, String) {
    if ti == crate::mixer::MASTER_STRIP {
        ("ST".into(), tr("Stereo Out").to_string())
    } else {
        (
            format!("{:02}", ti + 1),
            p.tracks.get(ti).map(|t| t.name.clone()).unwrap_or_default(),
        )
    }
}

/// Draws the effect window for the insert the Mixer opened, if any.
pub fn show(ctx: &egui::Context, p: &mut Project, s: &mut UiState, meters: &Meters) {
    let Some((ti, slot)) = s.mixer.fx_window else {
        return;
    };
    // The Stereo Out strip: a stand-in track with its effects and its automation.
    let master = ti == crate::mixer::MASTER_STRIP;
    let found = if master {
        p.master_inserts.get(slot)
    } else {
        p.tracks.get(ti).and_then(|t| t.inserts.get(slot))
    };
    let Some(insert) = found.copied() else {
        s.mixer.fx_window = None;
        return;
    };
    let stand_in;
    let track: &Track = if master {
        let mut t = Project::empty().tracks[0].clone();
        t.inserts = p.master_inserts.clone();
        t.auto = p.master_auto.clone();
        stand_in = t;
        &stand_in
    } else {
        &p.tracks[ti]
    };
    let (reduction, input_level) = if master {
        (meters.master_reduction, meters.master_fx_input)
    } else {
        (
            meters.reduction.get(ti).copied().unwrap_or(0.0),
            meters.fx_input.get(ti).copied().unwrap_or(0.0),
        )
    };
    let (badge, strip_name) = strip_plate(p, ti);
    if insert.kind == FxKind::Reverb {
        #[allow(deprecated)]
        let id = egui::Id::new(("space_designer", ti, slot));
        let (edited, edits, open) = crate::space_designer_ui::window(
            ctx,
            &insert,
            track,
            slot,
            p.position_beats,
            id,
            input_level,
            (&badge, &strip_name),
        );
        finish(p, s, ti, slot, insert, edited, edits, open);
        return;
    }
    let beat = p.position_beats;
    let mut edited = insert;
    let mut edits: Vec<Edit> = Vec::new();
    let width = unit_width(insert.kind);
    let plate = gear::Plate {
        model: model_code(insert.kind),
        name: insert.name(),
        strip: Some((&badge, &strip_name)),
        reset: true,
    };
    let id = egui::Id::unique("fx_unit").with((ti, slot));
    let (head, _) = gear::unit_window(ctx, id, width, &plate, Some(!edited.bypass), |ui| {
        if edited.kind == FxKind::Amp {
            amp_body(ui, width, id, &mut edited, track, slot, beat, &mut edits);
        } else {
            unit_body(
                ui,
                width,
                id,
                &mut edited,
                track,
                slot,
                beat,
                reduction,
                &mut edits,
            );
        }
    });
    if head.power {
        edited.bypass = !edited.bypass;
    }
    if head.reset {
        edited.params = edited.kind.defaults();
    }
    finish(p, s, ti, slot, insert, edited, edits, !head.close);
}

/// The faceplate's width for `kind`: room for its row of knobs.
fn unit_width(kind: FxKind) -> f32 {
    let per_row = per_row(kind);
    if kind == FxKind::MultiEq {
        return 6.0 * BAND_W + PAD * 2.0;
    }
    (per_row as f32 * gear::CELL_W + PAD * 2.0).max(560.0)
}

/// Knobs to a row.
fn per_row(kind: FxKind) -> usize {
    match kind {
        FxKind::MultiEq => 6,
        FxKind::Amp => 6,
        k => k.params().len().clamp(3, 6),
    }
}

/// The value of setting `k` as an LCD shows it.
pub(crate) fn value_text(info: &ParamInfo, v: f32) -> String {
    match info.unit {
        "dB" => format!("{v:.1} dB"),
        "Hz" if v >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
        "Hz" => format!("{v:.0} Hz"),
        "ms" if v >= 100.0 => format!("{v:.0} ms"),
        "ms" => format!("{v:.1} ms"),
        "s" => format!("{v:.2} s"),
        ":1" => format!("{v:.1}:1"),
        "%" => format!("{v:.0} %"),
        "" if info.max <= 1.0 => format!("{:.0} %", v * 100.0),
        "" => format!("{v:.2}"),
        u => format!("{v:.1} {u}"),
    }
}

/// Setting `k` of `insert` as a panel control (its lane's value while one is played).
fn control(insert: &Insert, track: &Track, slot: usize, k: usize, beat: f64) -> Control {
    let info = &insert.kind.params()[k];
    let target = AutoTarget::Fx {
        slot: slot as u8,
        param: k as u8,
    };
    let played = track.auto.played(target, beat);
    let v = shown(insert, track, slot, k, beat);
    let automated =
        (!track.auto.lane(target).is_empty()).then(|| !track.auto.is_overridden(target));
    let mut c = Control {
        label: info.name.to_string(),
        norm: info.to_norm(v),
        default: info.to_norm(info.default),
        text: value_text(info, v),
        choices: Vec::new(),
        choice: 0,
        automated,
        enabled: played.is_none() && automation::editable(track, target),
    };
    if insert.kind == FxKind::Filter && info.name == "Mode" {
        c.choices = ["Low Pass", "High Pass", "Band Pass"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        c.choice = (v.round().max(0.0) as usize).min(2);
    }
    c
}

/// Applies a panel change to setting `k`.
fn apply(edited: &mut Insert, edits: &mut Vec<Edit>, ch: gear::Change) {
    let info = &edited.kind.params()[ch.index];
    let value = match ch.choice {
        Some(i) => i as f32,
        None => info.from_norm(ch.norm),
    };
    edited.params[ch.index] = value;
    edits.push(Edit {
        param: ch.index,
        value,
        ended: ch.ended,
    });
}

/// The panel of every effect but the amplifier: its display, then its knobs.
#[allow(clippy::too_many_arguments)]
fn unit_body(
    ui: &mut Ui,
    width: f32,
    id: egui::Id,
    edited: &mut Insert,
    track: &Track,
    slot: usize,
    beat: f64,
    reduction: f32,
    edits: &mut Vec<Edit>,
) {
    let kind = edited.kind;
    let n = kind.params().len();
    let controls: Vec<Control> = (0..n)
        .map(|k| control(edited, track, slot, k, beat))
        .collect();
    let values: Vec<f32> = (0..n)
        .map(|k| shown(edited, track, slot, k, beat))
        .collect();
    let per = per_row(kind);
    let grid_h = if kind == FxKind::MultiEq {
        28.0 + 3.0 * BAND_H - 10.0
    } else {
        gear::grid_height(n, per)
    };
    let total = DISPLAY_H + 26.0 + grid_h + 6.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, total), egui::Sense::hover());
    let p = ui.painter().clone();
    let disp = Rect::from_min_size(
        pos2(rect.left() + PAD, rect.top() + 18.0),
        vec2(width - PAD * 2.0, DISPLAY_H - 18.0),
    );
    display(ui, &p, disp, kind, &values, reduction, edited.bypass);
    let grid_top = disp.bottom() + 22.0;
    gear::seam_h(&p, rect.left() + 1.0, rect.right() - 1.0, grid_top - 8.0);
    if kind == FxKind::MultiEq {
        for ch in band_columns(ui, &p, pos2(rect.left() + PAD, grid_top), id, &controls) {
            apply(edited, edits, ch);
        }
        return;
    }
    let grid_w = per as f32 * gear::CELL_W;
    let left = rect.left() + (width - grid_w) / 2.0;
    let indices: Vec<usize> = (0..n).collect();
    for ch in gear::control_grid(
        ui,
        pos2(left, grid_top + 6.0),
        per,
        id,
        &controls,
        &indices,
        48.0,
    ) {
        apply(edited, edits, ch);
    }
}

/// The Multi-Band EQ's settings of each band, one band to a column (numbered as on its
/// display), Freq, Gain and Q down the rows.
const BANDS: [[Option<usize>; 3]; 6] = [
    [Some(0), Some(1), None],
    [Some(2), Some(3), Some(4)],
    [Some(5), Some(6), Some(7)],
    [Some(8), Some(9), Some(10)],
    [Some(11), Some(12), Some(13)],
    [Some(14), Some(15), None],
];
const BAND_W: f32 = 100.0;
const BAND_H: f32 = 116.0;

fn band_columns(
    ui: &mut Ui,
    p: &egui::Painter,
    top_left: Pos2,
    id: egui::Id,
    controls: &[Control],
) -> Vec<gear::Change> {
    let mut out = Vec::new();
    for (b, rows) in BANDS.iter().enumerate() {
        let x = top_left.x + b as f32 * BAND_W;
        let badge =
            Rect::from_center_size(pos2(x + BAND_W / 2.0, top_left.y + 14.0), vec2(20.0, 16.0));
        p.rect_filled(badge, 2.0, gear::INK);
        gear::legend(
            p,
            badge.center(),
            Align2::CENTER_CENTER,
            &format!("{}", b + 1),
            10.0,
            gear::BONE,
        );
        if b > 0 {
            gear::seam_v(p, x, top_left.y + 8.0, top_left.y + 30.0 + 3.0 * BAND_H);
        }
        for (r, k) in rows.iter().enumerate() {
            let Some(k) = k else { continue };
            let mut c = controls[*k].clone();
            c.label = ["Freq", "Gain", "Q"][r].to_string();
            let cell = Rect::from_min_size(
                pos2(x, top_left.y + 28.0 + r as f32 * BAND_H),
                vec2(BAND_W, BAND_H),
            );
            if let Some(ch) = gear::control_cell(ui, cell, id, *k, &c, 40.0) {
                out.push(ch);
            }
        }
    }
    out
}

/// The unit's display for `kind`.
fn display(
    ui: &mut Ui,
    p: &egui::Painter,
    rect: Rect,
    kind: FxKind,
    v: &[f32],
    reduction: f32,
    bypassed: bool,
) {
    match kind {
        FxKind::ChannelEq | FxKind::MultiEq => {
            let glass = gear::lcd(p, rect);
            eq_curve(p, glass, kind, v);
        }
        FxKind::Compressor | FxKind::Gate | FxKind::Limiter => {
            let sq = Rect::from_min_size(rect.min, vec2(rect.height() * 1.25, rect.height()));
            let glass = gear::lcd(p, sq);
            transfer_curve(p, glass, kind, v);
            let right = Rect::from_min_max(pos2(sq.right() + 18.0, rect.top()), rect.max);
            reduction_meter(ui, p, right, reduction, bypassed);
        }
        FxKind::Delay => {
            let glass = gear::lcd(p, rect);
            echoes(p, glass, v);
        }
        FxKind::Chorus | FxKind::Phaser => {
            let glass = gear::lcd(p, rect);
            sweep(p, glass, kind, v);
        }
        FxKind::Overdrive => {
            let glass = gear::lcd(p, rect);
            drive_curve(p, glass, v);
        }
        FxKind::Filter => {
            let glass = gear::lcd(p, rect);
            filter_curve(p, glass, v);
        }
        FxKind::Amp | FxKind::Reverb => {}
    }
}

/// Faint grid lines on LCD glass.
fn lcd_grid(p: &egui::Painter, xs: &[f32], ys: &[f32], glass: Rect) {
    let st = Stroke::new(1.0, gear::alpha(gear::LCD_INK, 0.12));
    for x in xs {
        p.line_segment([pos2(*x, glass.top()), pos2(*x, glass.bottom())], st);
    }
    for y in ys {
        p.line_segment([pos2(glass.left(), *y), pos2(glass.right(), *y)], st);
    }
}

/// A curve drawn in LCD ink, filled down to `base_y` with a ghost of the ink.
fn ink_curve(p: &egui::Painter, glass: Rect, pts: Vec<Pos2>, base_y: f32) {
    let clip = p.with_clip_rect(glass.shrink(1.0));
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let poly = vec![a, b, pos2(b.x, base_y), pos2(a.x, base_y)];
        let mut mesh = egui::epaint::Mesh::default();
        let col = gear::alpha(gear::LCD_INK, 0.10);
        for q in &poly {
            mesh.colored_vertex(*q, col);
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        clip.add(Shape::mesh(mesh));
    }
    clip.add(Shape::line(pts, Stroke::new(2.4, gear::LCD_INK)));
}

/// The EQ's gain over 20 Hz to 20 kHz, ±18 dB, with each band's handle.
fn eq_curve(p: &egui::Painter, glass: Rect, kind: FxKind, v: &[f32]) {
    let plot = glass.shrink2(vec2(28.0, 14.0));
    let x_of = |hz: f32| plot.left() + (hz / 20.0).log10() / 3.0 * plot.width();
    let y_of = |db: f32| plot.center().y - db / 18.0 * plot.height() / 2.0;
    let xs: Vec<f32> = [
        50.0f32, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0,
    ]
    .iter()
    .map(|h| x_of(*h))
    .collect();
    let ys: Vec<f32> = [-12.0f32, -6.0, 0.0, 6.0, 12.0]
        .iter()
        .map(|d| y_of(*d))
        .collect();
    lcd_grid(p, &xs, &ys, glass);
    for (hz, t) in [(100.0, "100"), (1000.0, "1K"), (10000.0, "10K")] {
        gear::lcd_label(
            p,
            pos2(x_of(hz), glass.bottom() - 3.0),
            Align2::CENTER_BOTTOM,
            t,
        );
    }
    for (db, t) in [(12.0, "+12"), (0.0, "0"), (-12.0, "-12")] {
        gear::lcd_label(
            p,
            pos2(glass.left() + 5.0, y_of(db)),
            Align2::LEFT_CENTER,
            t,
        );
    }
    let mut params = [0.0f32; prowave_engine::MAX_FX_PARAMS];
    for (k, x) in v.iter().enumerate().take(params.len()) {
        params[k] = *x;
    }
    let n = 180;
    let freqs: Vec<f32> = (0..=n)
        .map(|i| 20.0 * 1000f32.powf(i as f32 / n as f32))
        .collect();
    let curve = prowave_engine::response_db(kind, params, &freqs, CURVE_RATE);
    let pts: Vec<_> = freqs
        .iter()
        .zip(&curve)
        .map(|(f, db)| pos2(x_of(*f), y_of(db.clamp(-18.0, 18.0))))
        .collect();
    ink_curve(p, glass, pts, y_of(0.0));
    // Band handles: (frequency, gain).
    let bands: Vec<(f32, f32)> = match kind {
        FxKind::ChannelEq => vec![(80.0, v[0]), (v[2], v[1]), (10000.0, v[4])],
        _ => vec![
            (v[0], v[1]),
            (v[2], v[3]),
            (v[5], v[6]),
            (v[8], v[9]),
            (v[11], v[12]),
            (v[14], v[15]),
        ],
    };
    for (i, (hz, db)) in bands.iter().enumerate() {
        let c = pos2(x_of(*hz), y_of(db.clamp(-18.0, 18.0)));
        let r = Rect::from_center_size(c, vec2(13.0, 13.0));
        p.rect_filled(r, 1.0, gear::LCD_INK);
        gear::text_at(
            p,
            r.center(),
            Align2::CENTER_CENTER,
            &format!("{}", i + 1),
            gear::mono_heavy(9.0),
            gear::LCD,
            0.0,
        );
    }
}

/// Output level over input level, -60 to 0 dB, of a dynamics unit.
fn transfer_curve(p: &egui::Painter, glass: Rect, kind: FxKind, v: &[f32]) {
    let plot = glass.shrink(10.0);
    let x_of = |db: f32| plot.left() + (db + 60.0) / 60.0 * plot.width();
    let y_of = |db: f32| plot.bottom() - (db + 60.0) / 60.0 * plot.height();
    let grid: Vec<f32> = [-48.0f32, -36.0, -24.0, -12.0].to_vec();
    lcd_grid(
        p,
        &grid.iter().map(|d| x_of(*d)).collect::<Vec<_>>(),
        &grid.iter().map(|d| y_of(*d)).collect::<Vec<_>>(),
        glass,
    );
    // The unity diagonal, dashed.
    let clip = p.with_clip_rect(plot);
    clip.add(Shape::dashed_line(
        &[pos2(x_of(-60.0), y_of(-60.0)), pos2(x_of(0.0), y_of(0.0))],
        Stroke::new(1.0, gear::alpha(gear::LCD_INK, 0.35)),
        3.0,
        3.0,
    ));
    let out = |x: f32| -> f32 {
        match kind {
            FxKind::Compressor => {
                let (thr, ratio, makeup) = (v[0], v[1].max(1.0), v[4]);
                let y = if x > thr { thr + (x - thr) / ratio } else { x };
                y + makeup
            }
            FxKind::Gate => {
                let (thr, range) = (v[0], v[1]);
                if x < thr {
                    x + range
                } else {
                    x
                }
            }
            _ => {
                let (gain, ceiling) = (v[0], v[1]);
                (x + gain).min(ceiling)
            }
        }
    };
    let pts: Vec<Pos2> = (0..=120)
        .map(|i| {
            let x = -60.0 + 60.0 * i as f32 / 120.0;
            pos2(x_of(x), y_of(out(x).clamp(-60.0, 0.0)))
        })
        .collect();
    ink_curve(p, glass, pts, plot.bottom());
    // The threshold (or ceiling) marker.
    let thr = match kind {
        FxKind::Limiter => v[1] - v[0],
        _ => v[0],
    };
    let tx = x_of(thr.clamp(-60.0, 0.0));
    clip.add(Shape::dashed_line(
        &[pos2(tx, plot.top()), pos2(tx, plot.bottom())],
        Stroke::new(1.0, gear::LCD_INK),
        2.0,
        3.0,
    ));
    gear::lcd_label(
        p,
        pos2(glass.left() + 5.0, glass.top() + 4.0),
        Align2::LEFT_TOP,
        "OUT",
    );
    gear::lcd_label(
        p,
        pos2(glass.right() - 5.0, glass.bottom() - 3.0),
        Align2::RIGHT_BOTTOM,
        "IN",
    );
}

/// The gain-reduction ladder (0 to 24 dB, lit from the right) and its readout.
fn reduction_meter(ui: &mut Ui, p: &egui::Painter, rect: Rect, reduction: f32, bypassed: bool) {
    let shown = if bypassed {
        0.0
    } else {
        reduction.clamp(0.0, 24.0)
    };
    gear::bracket(p, pos2(rect.left(), rect.top() + 6.0), rect.width(), "GR");
    let ladder = Rect::from_min_size(
        pos2(rect.left(), rect.top() + 20.0),
        vec2(rect.width(), 40.0),
    );
    gear::ladder_h(p, ladder, shown / 24.0, 0.0, None, true);
    // The printed scale under it.
    for (db, t) in [
        (0.0, "0"),
        (3.0, "3"),
        (6.0, "6"),
        (10.0, "10"),
        (15.0, "15"),
        (20.0, "20"),
        (24.0, "24"),
    ] {
        let x = ladder.right() - 4.0 - (ladder.width() - 8.0) * db / 24.0;
        p.line_segment(
            [
                pos2(x, ladder.bottom() + 3.0),
                pos2(x, ladder.bottom() + 7.0),
            ],
            Stroke::new(1.0, gear::INK2),
        );
        gear::silk(
            p,
            pos2(x, ladder.bottom() + 15.0),
            Align2::CENTER_CENTER,
            t,
            10.0,
            gear::PRINT,
        );
    }
    let chip = Rect::from_min_max(
        pos2(rect.left() + 6.0, rect.bottom() - 58.0),
        rect.max - vec2(6.0, 6.0),
    );
    gear::bezel(p, chip.expand(6.0));
    gear::glass(p, chip);
    gear::lcd_label(p, chip.left_top() + vec2(8.0, 6.0), Align2::LEFT_TOP, "GR");
    let t = format!("{:.1}", -shown);
    let w = gear::seg7_width(&t, 32.0);
    gear::seg7(
        p,
        pos2(chip.right() - 40.0 - w, chip.top() + 7.0),
        &t,
        32.0,
        gear::LCD_INK,
        true,
    );
    gear::lcd_text(
        p,
        pos2(chip.right() - 10.0, chip.top() + 39.0),
        Align2::RIGHT_BOTTOM,
        "DB",
        11.0,
    );
    ui.interact(ladder, ui.scope_id().with("gr_tip"), egui::Sense::hover())
        .on_hover_text(trf("Gain Reduction: {} dB", &[&format!("{shown:.1}")]));
}

/// The delay's echoes: each repeat a bar at its time, falling by the feedback; ping-pong
/// sends every other one down (the other side).
fn echoes(p: &egui::Painter, glass: Rect, v: &[f32]) {
    let (time, fb, ping, mix) = (v[0].max(1.0), v[1], v[3], v[4]);
    let plot = glass.shrink2(vec2(16.0, 14.0));
    let span = (time * 7.0).max(500.0);
    let x_of = |ms: f32| plot.left() + ms / span * plot.width();
    let mid = plot.center().y;
    lcd_grid(p, &[], &[mid], glass);
    let bar = |x: f32, h: f32, up: bool| {
        let r = if up {
            Rect::from_min_max(pos2(x - 3.0, mid - h), pos2(x + 3.0, mid))
        } else {
            Rect::from_min_max(pos2(x - 3.0, mid), pos2(x + 3.0, mid + h))
        };
        p.rect_filled(r, 0.0, gear::LCD_INK);
    };
    let half = plot.height() / 2.0 - 4.0;
    // The dry hit full height, then the repeats at the wet level, each one quieter by the
    // feedback.
    bar(x_of(0.0) + 3.0, half, true);
    let mut level = (0.25 + 0.75 * mix).min(1.0);
    let mut k = 1;
    while level > 0.03 && k < 64 {
        let x = x_of(time * k as f32);
        if x > plot.right() {
            break;
        }
        let up = ping < 0.5 || k % 2 == 1;
        bar(x, half * level, up);
        level *= fb;
        k += 1;
    }
    let w = gear::seg7_width(&format!("{time:.0}"), 26.0);
    let at = pos2(glass.right() - 40.0 - w, glass.top() + 10.0);
    gear::seg7(p, at, &format!("{time:.0}"), 26.0, gear::LCD_INK, true);
    gear::lcd_text(
        p,
        pos2(glass.right() - 10.0, glass.top() + 36.0),
        Align2::RIGHT_BOTTOM,
        "MS",
        11.0,
    );
}

/// The modulation's sweep over two seconds: the LFO at its rate and depth.
fn sweep(p: &egui::Painter, glass: Rect, kind: FxKind, v: &[f32]) {
    let rate = v[0];
    let depth = match kind {
        FxKind::Chorus => v[1] / 8.0,
        _ => v[1],
    };
    let plot = glass.shrink2(vec2(16.0, 18.0));
    lcd_grid(
        p,
        &(1..4)
            .map(|i| plot.left() + plot.width() * i as f32 / 4.0)
            .collect::<Vec<_>>(),
        &[plot.center().y],
        glass,
    );
    let pts: Vec<Pos2> = (0..=240)
        .map(|i| {
            let t = 2.0 * i as f32 / 240.0;
            let y = (std::f32::consts::TAU * rate * t).sin() * depth;
            pos2(
                plot.left() + plot.width() * i as f32 / 240.0,
                plot.center().y - y * plot.height() / 2.0,
            )
        })
        .collect();
    ink_curve(p, glass, pts, plot.center().y);
    let t = format!("{rate:.2}");
    let w = gear::seg7_width(&t, 26.0);
    gear::seg7(
        p,
        pos2(glass.right() - 40.0 - w, glass.top() + 10.0),
        &t,
        26.0,
        gear::LCD_INK,
        true,
    );
    gear::lcd_text(
        p,
        pos2(glass.right() - 10.0, glass.top() + 36.0),
        Align2::RIGHT_BOTTOM,
        "HZ",
        11.0,
    );
}

/// The overdrive's curve: output over input at its drive.
fn drive_curve(p: &egui::Painter, glass: Rect, v: &[f32]) {
    let drive = 10f32.powf(v[0] / 20.0);
    let level = 10f32.powf(v[2] / 20.0);
    let plot = glass.shrink2(vec2(glass.width() / 2.0 - glass.height() * 0.6, 12.0));
    lcd_grid(p, &[plot.center().x], &[plot.center().y], glass);
    let pts: Vec<Pos2> = (0..=160)
        .map(|i| {
            let x = -1.0 + 2.0 * i as f32 / 160.0;
            let y = (x * drive).tanh() / drive.tanh().max(1e-3) * level.min(1.0);
            pos2(
                plot.center().x + x * plot.width() / 2.0,
                plot.center().y - y * plot.height() / 2.0,
            )
        })
        .collect();
    ink_curve(p, glass, pts, plot.center().y);
    let t = format!("{:.1}", v[0]);
    let w = gear::seg7_width(&t, 26.0);
    gear::seg7(
        p,
        pos2(glass.right() - 40.0 - w, glass.top() + 10.0),
        &t,
        26.0,
        gear::LCD_INK,
        true,
    );
    gear::lcd_text(
        p,
        pos2(glass.right() - 10.0, glass.top() + 36.0),
        Align2::RIGHT_BOTTOM,
        "DB",
        11.0,
    );
}

/// The filter's response: a resonant low-, high- or band-pass at its cutoff.
fn filter_curve(p: &egui::Painter, glass: Rect, v: &[f32]) {
    let (fc, q, mode) = (v[0].max(20.0), v[1].max(0.1), v[2].round() as i32);
    let plot = glass.shrink2(vec2(28.0, 14.0));
    let x_of = |hz: f32| plot.left() + (hz / 20.0).log10() / 3.0 * plot.width();
    let y_of = |db: f32| plot.top() + (12.0 - db) / 48.0 * plot.height();
    lcd_grid(
        p,
        &[100.0f32, 1000.0, 10000.0]
            .iter()
            .map(|h| x_of(*h))
            .collect::<Vec<_>>(),
        &[0.0f32, -12.0, -24.0]
            .iter()
            .map(|d| y_of(*d))
            .collect::<Vec<_>>(),
        glass,
    );
    for (hz, t) in [(100.0, "100"), (1000.0, "1K"), (10000.0, "10K")] {
        gear::lcd_label(
            p,
            pos2(x_of(hz), glass.bottom() - 3.0),
            Align2::CENTER_BOTTOM,
            t,
        );
    }
    let w0 = std::f32::consts::TAU * fc / CURVE_RATE;
    let alpha = w0.sin() / (2.0 * q);
    let cw = w0.cos();
    let (b0, b1, b2) = match mode {
        1 => ((1.0 + cw) / 2.0, -(1.0 + cw), (1.0 + cw) / 2.0),
        2 => (alpha, 0.0, -alpha),
        _ => ((1.0 - cw) / 2.0, 1.0 - cw, (1.0 - cw) / 2.0),
    };
    let (a0, a1, a2) = (1.0 + alpha, -2.0 * cw, 1.0 - alpha);
    let mag = |f: f32| {
        let w = std::f32::consts::TAU * f / CURVE_RATE;
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let nr = b0 + b1 * c1 + b2 * c2;
        let ni = -(b1 * s1 + b2 * s2);
        let dr = a0 + a1 * c1 + a2 * c2;
        let di = -(a1 * s1 + a2 * s2);
        let m = ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt();
        20.0 * m.max(1e-6).log10()
    };
    let pts: Vec<Pos2> = (0..=180)
        .map(|i| {
            let f = 20.0 * 1000f32.powf(i as f32 / 180.0);
            pos2(x_of(f), y_of(mag(f).clamp(-36.0, 12.0)))
        })
        .collect();
    ink_curve(p, glass, pts, plot.bottom());
    let t = if fc >= 1000.0 {
        format!("{:.2}", fc / 1000.0)
    } else {
        format!("{fc:.0}")
    };
    let unit = if fc >= 1000.0 { "KHZ" } else { "HZ" };
    let w = gear::seg7_width(&t, 26.0);
    gear::seg7(
        p,
        pos2(glass.right() - 46.0 - w, glass.top() + 10.0),
        &t,
        26.0,
        gear::LCD_INK,
        true,
    );
    gear::lcd_text(
        p,
        pos2(glass.right() - 10.0, glass.top() + 36.0),
        Align2::RIGHT_BOTTOM,
        unit,
        11.0,
    );
}

/// The amplifier head: the model and cabinet on the LCD with their menus, the six knobs,
/// and the microphone's slide from the edge of the cone to its centre.
#[allow(clippy::too_many_arguments)]
fn amp_body(
    ui: &mut Ui,
    width: f32,
    id: egui::Id,
    edited: &mut Insert,
    track: &Track,
    slot: usize,
    beat: f64,
    edits: &mut Vec<Edit>,
) {
    let model = shown(edited, track, slot, 0, beat).round().clamp(0.0, 4.0) as usize;
    let cab = shown(edited, track, slot, 7, beat).round().clamp(0.0, 3.0) as usize;
    let total = 132.0 + 22.0 + gear::CELL_H + 86.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, total), egui::Sense::hover());
    let p = ui.painter().clone();
    let editable = |param: usize| {
        let target = AutoTarget::Fx {
            slot: slot as u8,
            param: param as u8,
        };
        track.auto.played(target, beat).is_none() && automation::editable(track, target)
    };
    // The LCD: the amplifier's model large, the cabinet under it, and a speaker grille.
    let lcd = Rect::from_min_size(
        pos2(rect.left() + PAD, rect.top() + 18.0),
        vec2(width - PAD * 2.0 - 220.0, 100.0),
    );
    let glass = gear::lcd(&p, lcd);
    gear::lcd_label(
        &p,
        glass.left_top() + vec2(8.0, 7.0),
        Align2::LEFT_TOP,
        tr("Amp"),
    );
    gear::lcd_text(
        &p,
        glass.left_top() + vec2(8.0, 22.0),
        Align2::LEFT_TOP,
        AMP_MODELS[model],
        21.0,
    );
    gear::lcd_label(
        &p,
        glass.left_top() + vec2(8.0, 52.0),
        Align2::LEFT_TOP,
        tr("Cabinet"),
    );
    gear::lcd_text(
        &p,
        glass.left_top() + vec2(8.0, 66.0),
        Align2::LEFT_TOP,
        AMP_CABS[cab],
        13.0,
    );
    // The two menus beside it.
    let mx = lcd.right() + 16.0;
    let mw = rect.right() - PAD - mx;
    gear::silk(
        &p,
        pos2(mx, lcd.top() + 6.0),
        Align2::LEFT_CENTER,
        tr("Amp"),
        11.0,
        gear::INK2,
    );
    let r = Rect::from_min_size(pos2(mx, lcd.top() + 16.0), vec2(mw, 26.0));
    if let Some(i) =
        gear::choice_field(ui, r, id.with("amp_model"), &AMP_MODELS, model, editable(0))
    {
        edited.params[0] = i as f32;
        edits.push(Edit {
            param: 0,
            value: i as f32,
            ended: true,
        });
    }
    gear::silk(
        &p,
        pos2(mx, lcd.top() + 58.0),
        Align2::LEFT_CENTER,
        tr("Cabinet"),
        11.0,
        gear::INK2,
    );
    let r = Rect::from_min_size(pos2(mx, lcd.top() + 68.0), vec2(mw, 26.0));
    if let Some(i) = gear::choice_field(ui, r, id.with("amp_cab"), &AMP_CABS, cab, editable(7)) {
        edited.params[7] = i as f32;
        edits.push(Edit {
            param: 7,
            value: i as f32,
            ended: true,
        });
    }
    // The knobs: Gain, Bass, Mid, Treble, Presence, Master.
    let grid_top = lcd.bottom() + 26.0;
    gear::seam_h(&p, rect.left() + 1.0, rect.right() - 1.0, grid_top - 8.0);
    let controls: Vec<Control> = (0..edited.kind.params().len())
        .map(|k| control(edited, track, slot, k, beat))
        .collect();
    let grid_w = 6.0 * gear::CELL_W;
    let left = rect.left() + (width - grid_w) / 2.0;
    for ch in gear::control_grid(
        ui,
        pos2(left, grid_top + 6.0),
        6,
        id,
        &controls,
        &[1, 2, 3, 4, 5, 6],
        50.0,
    ) {
        apply(edited, edits, ch);
    }
    // The microphone.
    let my = grid_top + gear::CELL_H + 14.0;
    gear::seam_h(&p, rect.left() + 1.0, rect.right() - 1.0, my - 6.0);
    let mic = &controls[8];
    let lr = gear::silk(
        &p,
        pos2(rect.left() + PAD, my + 22.0),
        Align2::LEFT_CENTER,
        tr("Mic"),
        12.0,
        gear::INK2,
    );
    if let Some(on) = mic.automated {
        gear::led(&p, pos2(lr.right() + 8.0, lr.center().y), 2.5, on);
    }
    let fr = Rect::from_min_max(
        pos2(rect.left() + PAD + 70.0, my + 6.0),
        pos2(rect.right() - PAD - 130.0, my + 40.0),
    );
    let out = gear::slide_h(ui, fr, id.with("mic"), mic.norm, mic.default, mic.enabled);
    if let Some(n) = out.value {
        apply(
            edited,
            edits,
            gear::Change {
                index: 8,
                norm: n,
                choice: None,
                ended: out.ended,
            },
        );
    }
    out.resp
        .on_hover_text(tr("From the edge of the cone to its centre"));
    let chip = Rect::from_min_size(
        pos2(rect.right() - PAD - 110.0, my + 8.0),
        vec2(110.0, 30.0),
    );
    gear::lcd_chip(&p, chip, "", &controls[8].text);
    // The speaker cone on the LCD, with the microphone's spot on it.
    let c = pos2(glass.right() - 50.0, glass.center().y);
    for (r, a) in [(40.0f32, 0.55f32), (29.0, 0.4), (16.0, 0.6)] {
        p.circle_stroke(c, r, Stroke::new(1.6, gear::alpha(gear::LCD_INK, a)));
    }
    p.circle_filled(c, 7.0, gear::LCD_INK);
    let mic_r = 40.0 * (1.0 - mic.norm);
    p.circle_filled(
        c + vec2(mic_r * 0.7, -mic_r * 0.7),
        4.0,
        Color32::from_rgb(0xFF, 0x5A, 0x14),
    );
}

/// Puts an edited effect and the setting moves of its window into the project (and the
/// automation), and closes the window when it was closed.
#[allow(clippy::too_many_arguments)]
fn finish(
    p: &mut Project,
    s: &mut UiState,
    ti: usize,
    slot: usize,
    insert: Insert,
    mut edited: Insert,
    edits: Vec<Edit>,
    open: bool,
) {
    let beat = p.position_beats;
    if edited != insert {
        edited.params = edited.kind.clamp_params(edited.params);
        if ti == crate::mixer::MASTER_STRIP {
            if let Some(i) = p.master_inserts.get_mut(slot) {
                *i = edited;
            }
        } else {
            p.tracks[ti].inserts[slot] = edited;
        }
    }
    if ti == crate::mixer::MASTER_STRIP {
        // The Stereo Out's envelopes: a slider held plays its own value while it moves.
        for e in edits {
            let target = AutoTarget::Fx {
                slot: slot as u8,
                param: e.param as u8,
            };
            p.master_auto.held = (!e.ended).then_some(target);
        }
        if !open {
            s.mixer.fx_window = None;
        }
        return;
    }
    let playing = p.playing;
    for e in edits {
        let target = AutoTarget::Fx {
            slot: slot as u8,
            param: e.param as u8,
        };
        automation::moved(&mut p.tracks[ti], target, e.value);
        if e.ended {
            automation::released(&mut p.tracks[ti], target, beat, playing);
        }
    }
    if !open {
        s.mixer.fx_window = None;
    }
}

/// What a control of the amplifier shows at `beat`: its lane's value while the strip plays
/// one, else the setting the user made.
fn shown(insert: &Insert, track: &Track, slot: usize, param: usize, beat: f64) -> f32 {
    let info = &insert.kind.params()[param];
    let target = AutoTarget::Fx {
        slot: slot as u8,
        param: param as u8,
    };
    track
        .auto
        .played(target, beat)
        .map(|n| info.from_norm(n))
        .unwrap_or(insert.params[param])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Insert;

    #[test]
    fn the_window_edits_the_insert_and_closes_with_it() {
        let mut p = Project::empty();
        p.tracks[0].inserts = vec![
            Insert::new(FxKind::Compressor),
            Insert::new(FxKind::ChannelEq),
            Insert::new(FxKind::MultiEq),
            Insert::new(FxKind::Limiter),
            Insert::new(FxKind::Gate),
        ];
        let mut s = UiState::new();
        let meters = Meters::default();
        let mut h = crate::ui_test::Harness::new(800.0, 700.0);
        for slot in 0..5 {
            s.mixer.fx_window = Some((0, slot));
            for _ in 0..3 {
                h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s, &meters));
            }
            assert_eq!(s.mixer.fx_window, Some((0, slot)));
        }
        // Nothing was changed by looking.
        assert_eq!(p.tracks[0].inserts[0], Insert::new(FxKind::Compressor));
        assert_eq!(p.tracks[0].inserts[2], Insert::new(FxKind::MultiEq));
        // The insert goes away: the window closes.
        p.tracks[0].inserts.clear();
        h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s, &meters));
        assert_eq!(s.mixer.fx_window, None);
    }

    #[test]
    fn the_space_designer_window_draws_and_changes_nothing_by_looking() {
        let mut p = Project::empty();
        p.tracks[0].inserts = vec![Insert::new(FxKind::Reverb)];
        let mut s = UiState::new();
        s.mixer.fx_window = Some((0, 0));
        let meters = Meters::default();
        let mut h = crate::ui_test::Harness::new(1000.0, 800.0);
        for _ in 0..4 {
            h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s, &meters));
        }
        assert_eq!(s.mixer.fx_window, Some((0, 0)));
        assert_eq!(p.tracks[0].inserts[0], Insert::new(FxKind::Reverb));
    }

    #[test]
    fn the_stereo_out_effect_window_opens_edits_and_closes() {
        let mut p = Project::empty();
        p.master_inserts = vec![Insert::new(FxKind::Reverb), Insert::new(FxKind::Compressor)];
        let mut s = UiState::new();
        let meters = Meters::default();
        let mut h = crate::ui_test::Harness::new(1000.0, 800.0);
        for slot in 0..2 {
            s.mixer.fx_window = Some((crate::mixer::MASTER_STRIP, slot));
            for _ in 0..3 {
                h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s, &meters));
            }
            assert_eq!(s.mixer.fx_window, Some((crate::mixer::MASTER_STRIP, slot)));
        }
        assert_eq!(
            p.master_inserts[0],
            Insert::new(FxKind::Reverb),
            "looking changes nothing"
        );
        // The effect goes away: the window closes.
        p.master_inserts.clear();
        h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s, &meters));
        assert_eq!(s.mixer.fx_window, None);
    }

    #[test]
    fn the_amp_designer_window_draws_its_automated_knobs() {
        let mut p = Project::empty();
        p.tracks[0].inserts = vec![Insert::new(FxKind::Amp)];
        let mut s = UiState::new();
        s.mixer.fx_window = Some((0, 0));
        let meters = Meters::default();
        let mut h = crate::ui_test::Harness::new(900.0, 760.0);
        for _ in 0..4 {
            h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s, &meters));
        }
        // Looking at the faceplate changes nothing.
        assert_eq!(s.mixer.fx_window, Some((0, 0)));
        assert_eq!(p.tracks[0].inserts[0], Insert::new(FxKind::Amp));
        // A played Gain lane moves the knob; the setting stays (moving it by hand would
        // override the lane, as in Live).
        let gain = AutoTarget::Fx { slot: 0, param: 1 };
        p.tracks[0].auto.set_point(gain, 0.0, 0.25);
        assert!(automation::editable(&p.tracks[0], gain));
        for _ in 0..4 {
            h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s, &meters));
        }
        assert_eq!(p.tracks[0].inserts[0].params[1], 12.0);
        assert_eq!(p.tracks[0].auto.lane(gain).len(), 1);
    }

    #[test]
    fn a_played_lane_moves_the_slider_without_touching_the_setting() {
        let mut p = Project::empty();
        p.tracks[0].inserts = vec![Insert::new(FxKind::Compressor)];
        let thr = AutoTarget::Fx { slot: 0, param: 0 };
        p.tracks[0].auto.set_point(thr, 0.0, 0.25);
        let mut s = UiState::new();
        s.mixer.fx_window = Some((0, 0));
        let meters = Meters::default();
        let mut h = crate::ui_test::Harness::new(800.0, 600.0);
        for _ in 0..4 {
            h.frame(vec![], |ui| show(ui.ctx(), &mut p, &mut s, &meters));
        }
        // Looking at a played lane changes neither the setting nor the lane.
        assert_eq!(p.tracks[0].inserts[0].params[0], -18.0);
        assert_eq!(p.tracks[0].auto.lane(thr).len(), 1);
        assert_eq!(p.tracks[0].auto.held, None);
        assert!(!p.tracks[0].auto.is_overridden(thr));
    }
}
