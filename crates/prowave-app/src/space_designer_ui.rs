//! The window of Space D, Logic Pro's Space Designer, built as a PW-1 hardware unit (SD-1): a
//! bone faceplate with the preset field, keys for previous / next, Compare, Copy, Paste, Undo,
//! Redo, View and Link; the Sampled IR and Synthesized IR keys; the envelope keys, the response's
//! field and Output EQ; the impulse response drawn on LCD glass with its Volume Envelope (or
//! Filter Envelope) and the handles to drag; an LCD strip of its values; and the controls below
//! (Input with its meter, Predelay, Length, Size, X-Over, Lo and Hi Spread, Dry and Wet) as
//! knurled knobs and slide faders with LCD readouts. Every control is an effect setting, so it
//! is automatable.
use crate::fx_ui::Edit;
use crate::gear::{self, Cap, Glyph, LedAt};
use crate::i18n::tr;
use crate::model::{AutoTarget, Insert, Track};
use egui::{pos2, vec2, Align2, Rect, Sense, Shape, Stroke, Ui};
use prowave_engine::{sd, FxKind, IR_NAMES, MAX_FX_PARAMS};

const BASE_W: f32 = 760.0;
const PAD: f32 = 26.0;
const TOP_H: f32 = 46.0;
const TAB_H: f32 = 40.0;
const SUB_H: f32 = 42.0;
const DISPLAY_H: f32 = 236.0;
const READOUT_H: f32 = 40.0;
const OPTIONS_H: f32 = 44.0;
const PANEL_H: f32 = 214.0;
const FOOT_H: f32 = 44.0;

/// The response's waveform and grid on the glass: LCD ink, faint.
fn ink(a: f32) -> egui::Color32 {
    gear::alpha(gear::LCD_INK, a)
}

/// The drawn response: what it was made for, its min/max bins and its length in seconds.
/// Moves a setting by hand: the insert, the moves so far, which setting, its value, and whether
/// the gesture ended.
type SetFn<'a> = dyn FnMut(&mut Insert, &mut Vec<Edit>, usize, f32, bool) + 'a;

type Preview = (Vec<u32>, Vec<(f32, f32)>, f32);

/// What the window remembers between frames.
#[derive(Clone, Default)]
struct Memory {
    /// Settings when the window opened (what Compare goes back to).
    original: Option<[f32; MAX_FX_PARAMS]>,
    /// Compare is on: the settings that were being edited wait here.
    stashed: Option<[f32; MAX_FX_PARAMS]>,
    undo: Vec<[f32; MAX_FX_PARAMS]>,
    redo: Vec<[f32; MAX_FX_PARAMS]>,
    /// View: window size in percent (100 when unset).
    view: u32,
    /// The display edits the Filter Envelope, not the Volume Envelope.
    filter_view: bool,
    /// The drawn response and what it was made for.
    preview: Option<Preview>,
}

/// The Copy and Paste clipboard, for every Space Designer window.
fn clipboard() -> &'static std::sync::Mutex<Option<[f32; MAX_FX_PARAMS]>> {
    static CLIP: std::sync::OnceLock<std::sync::Mutex<Option<[f32; MAX_FX_PARAMS]>>> =
        std::sync::OnceLock::new();
    CLIP.get_or_init(|| std::sync::Mutex::new(None))
}

/// The settings a preset of the Sampled IR tab sets: the response and its natural length. The
/// mix, the stereo and the output EQ stay as they are.
pub(crate) fn preset_params(base: &[f32; MAX_FX_PARAMS], index: usize) -> [f32; MAX_FX_PARAMS] {
    let mut p = *base;
    let defaults = FxKind::Reverb.defaults();
    let index = index.min(IR_NAMES.len() - 1);
    let secs = prowave_engine::ir_seconds(index);
    p[sd::IR_TYPE] = 0.0;
    p[sd::IR_INDEX] = index as f32;
    // Length only cuts: it is left above the response, so the whole room is heard.
    p[sd::LENGTH] = secs.max(defaults[sd::LENGTH]);
    p[sd::DECAY] = secs;
    p[sd::SIZE] = defaults[sd::SIZE];
    p[sd::INIT_LEVEL] = defaults[sd::INIT_LEVEL];
    p[sd::ATTACK] = defaults[sd::ATTACK].min(secs * 1000.0 * 0.05);
    p[sd::END_LEVEL] = defaults[sd::END_LEVEL];
    p[sd::IR_OFFSET] = 0.0;
    p[sd::REVERSE] = 0.0;
    p
}

/// The mastering presets: a little reverb on the master bus that glues and widens the mix.
pub(crate) const MASTERING_PRESETS: [&str; 3] = [
    "Mastering Glue - Small Room",
    "Mastering Plate - Vocal Sheen",
    "Mastering Wide Hall",
];

/// The settings of mastering preset `index`: a small wet level, the dry sound at 0 dB, a short
/// exponential decay that ends near silence, and the Predelay that keeps transients clear.
pub(crate) fn mastering_preset(index: usize) -> [f32; MAX_FX_PARAMS] {
    let mut p = FxKind::Reverb.defaults();
    p[sd::IR_TYPE] = 0.0;
    p[sd::QUALITY] = 1.0;
    p[sd::DRY] = 0.0;
    p[sd::VOLUME_ENV] = 1.0;
    p[sd::INIT_LEVEL] = 100.0;
    p[sd::ATTACK] = 10.0;
    p[sd::CURVE] = 1.0;
    p[sd::END_LEVEL] = 5.0;
    p[sd::OUTPUT_EQ] = 1.0;
    match index {
        0 => {
            // Small Room, 0.5 s: tight and clean, the bass nearly mono and the air widened.
            p[sd::IR_INDEX] = 2.0;
            p[sd::PREDELAY] = 15.0;
            p[sd::LENGTH] = 0.8;
            p[sd::SIZE] = 100.0;
            p[sd::XOVER] = 700.0;
            p[sd::LO_SPREAD] = 60.0;
            p[sd::HI_SPREAD] = 110.0;
            p[sd::WET] = -20.0;
            p[sd::DECAY] = 0.8;
            p[sd::FILTER_ENV] = 1.0;
            p[sd::FILTER_START] = 20_000.0;
            p[sd::FILTER_END] = 8_000.0;
            p[sd::OUT_LO] = -2.0;
            p[sd::OUT_HI] = -1.0;
        }
        1 => {
            // Plate: a sheen on the vocal, with the top of the reverb taken down 1 dB.
            p[sd::IR_INDEX] = 3.0;
            p[sd::PREDELAY] = 20.0;
            p[sd::LENGTH] = 1.2;
            p[sd::SIZE] = 110.0;
            p[sd::WET] = -18.0;
            p[sd::DECAY] = 1.2;
            p[sd::OUT_LO] = 0.0;
            p[sd::OUT_HI] = -1.0;
        }
        _ => {
            // Hall, 2.0 s cut to 1.5 s: wide and very quiet.
            p[sd::IR_INDEX] = 5.0;
            p[sd::PREDELAY] = 15.0;
            p[sd::LENGTH] = 1.5;
            p[sd::SIZE] = 100.0;
            p[sd::LO_SPREAD] = 70.0;
            p[sd::HI_SPREAD] = 120.0;
            p[sd::WET] = -25.0;
            p[sd::DECAY] = 1.5;
            p[sd::FILTER_ENV] = 1.0;
            p[sd::FILTER_START] = 20_000.0;
            p[sd::FILTER_END] = 8_000.0;
            p[sd::OUT_LO] = -2.0;
            p[sd::OUT_HI] = -1.0;
        }
    }
    FxKind::Reverb.clamp_params(p)
}

fn value_of(insert: &Insert, track: &Track, slot: usize, param: usize, beat: f64) -> f32 {
    let info = &insert.kind.params()[param];
    track
        .auto
        .played(
            AutoTarget::Fx {
                slot: slot as u8,
                param: param as u8,
            },
            beat,
        )
        .map(|n| info.from_norm(n))
        .unwrap_or(insert.params[param])
}

/// Draws the window. Returns the insert as edited, the setting moves (for automation), and
/// whether the window is still open. `strip` is the strip's name plate: (badge, name).
#[allow(clippy::too_many_arguments)]
pub(crate) fn window(
    ctx: &egui::Context,
    insert: &Insert,
    track: &Track,
    slot: usize,
    beat: f64,
    id: egui::Id,
    input_level: f32,
    strip: (&str, &str),
) -> (Insert, Vec<Edit>, bool) {
    let mut edited = *insert;
    let mut edits: Vec<Edit> = Vec::new();
    let mut mem: Memory = ctx.data_mut(|d| d.get_temp(id).unwrap_or_default());
    if mem.view == 0 {
        mem.view = 100;
    }
    if mem.original.is_none() {
        mem.original = Some(insert.params);
    }
    let k = mem.view as f32 / 100.0;
    let plate = gear::Plate {
        model: crate::fx_ui::model_code(FxKind::Reverb),
        name: insert.name(),
        strip: Some(strip),
        reset: false,
    };
    let (head, _) = gear::unit_window(
        ctx,
        id.with("window"),
        BASE_W * k,
        &plate,
        Some(!edited.bypass),
        |ui| {
            body(
                ui,
                &mut edited,
                &mut edits,
                &mut mem,
                (track, slot, beat, input_level),
                k,
            );
        },
    );
    if head.power {
        edited.bypass = !edited.bypass;
    }
    // Undo and Redo: a gesture is one step.
    if edited.params != insert.params {
        let (pressed, down) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.any_down()));
        if pressed || !down {
            if mem.undo.last() != Some(&insert.params) {
                mem.undo.push(insert.params);
                if mem.undo.len() > 50 {
                    mem.undo.remove(0);
                }
            }
            mem.redo.clear();
        }
    }
    ctx.data_mut(|d| d.insert_temp(id, mem));
    (edited, edits, !head.close)
}

/// Every setting reported as moved (after a preset, Compare, Paste, Undo or Redo).
fn all(e: &Insert, edits: &mut Vec<Edit>) {
    edits.extend((0..e.kind.params().len()).map(|p| Edit {
        param: p,
        value: e.params[p],
        ended: true,
    }));
}

/// A key that can be greyed out: painted dimmed and deaf while `enabled` is false.
fn glyph_key_enabled(
    ui: &mut Ui,
    rect: Rect,
    id: egui::Id,
    glyph: Glyph,
    enabled: bool,
    tip: &str,
) -> bool {
    let resp = gear::glyph_key(ui, rect, id, Cap::Bone, false, glyph);
    if !enabled {
        ui.painter()
            .rect_filled(rect.expand(1.0), 5.0, gear::alpha(gear::BONE, 0.55));
    }
    resp.on_hover_text(tip).clicked() && enabled
}

#[allow(clippy::too_many_arguments)]
fn body(
    ui: &mut Ui,
    edited: &mut Insert,
    edits: &mut Vec<Edit>,
    mem: &mut Memory,
    (track, slot, beat, input_level): (&Track, usize, f64, f32),
    k: f32,
) {
    let w = BASE_W * k;
    let params = edited.kind.params();
    let shown = |e: &Insert, p: usize| value_of(e, track, slot, p, beat);
    let editable = |p: usize| {
        crate::automation::editable(
            track,
            AutoTarget::Fx {
                slot: slot as u8,
                param: p as u8,
            },
        ) && track
            .auto
            .played(
                AutoTarget::Fx {
                    slot: slot as u8,
                    param: p as u8,
                },
                beat,
            )
            .is_none()
    };
    // A setting moved by hand: kept in range and reported for automation.
    let mut set = |e: &mut Insert, edits: &mut Vec<Edit>, p: usize, v: f32, ended: bool| {
        let info = &params[p];
        let v = v.clamp(info.min, info.max);
        e.params[p] = v;
        edits.push(Edit {
            param: p,
            value: v,
            ended,
        });
    };

    // The whole panel is one allocation; each section is a slice of it, so the controls
    // placed by hand inside cannot push the next section around.
    let total = TOP_H + TAB_H + SUB_H + DISPLAY_H + READOUT_H + OPTIONS_H + PANEL_H + FOOT_H;
    let (whole, _) = ui.allocate_exact_size(vec2(w, total * k), Sense::hover());
    let mut y_next = whole.top();
    let mut next = |h: f32| {
        let r = Rect::from_min_size(pos2(whole.left(), y_next), vec2(w, h * k));
        y_next += h * k;
        r
    };
    let x_at = |r: Rect, x: f32| r.left() + x * k;
    let sid = ui.scope_id();

    // ---- Preset, previous / next, Compare, Copy, Paste, Undo, Redo, View, Link -----------
    let top = next(TOP_H);
    let g = ui.painter().clone();
    let row = |r: Rect, x: f32, wd: f32| {
        Rect::from_min_size(
            pos2(r.left() + x * k, r.top() + 12.0 * k),
            vec2(wd * k, 26.0 * k),
        )
    };
    let index = (edited.params[sd::IR_INDEX].round().max(0.0) as usize).min(IR_NAMES.len() - 1);
    let mastering = (0..MASTERING_PRESETS.len()).find(|&i| mastering_preset(i) == edited.params);
    let name = if let Some(i) = mastering {
        MASTERING_PRESETS[i].to_string()
    } else if edited.params[sd::IR_TYPE] >= 0.5 {
        tr("Synthesized IR").to_string()
    } else {
        IR_NAMES[index].to_string()
    };
    let resp = gear::field(ui, row(top, PAD, 232.0), sid.with("sd_preset"), &name, true);
    egui::Popup::menu(&resp).show(|ui| {
        for (i, n) in IR_NAMES.iter().enumerate() {
            let on = mastering.is_none() && i == index && edited.params[sd::IR_TYPE] < 0.5;
            if ui.selectable_label(on, *n).clicked() {
                edited.params = preset_params(&edited.params, i);
                all(edited, edits);
            }
        }
        ui.separator();
        for (i, n) in MASTERING_PRESETS.iter().enumerate() {
            if ui.selectable_label(mastering == Some(i), *n).clicked() {
                edited.params = mastering_preset(i);
                all(edited, edits);
            }
        }
    });
    if gear::glyph_key(
        ui,
        row(top, 266.0, 28.0),
        sid.with("sd_prev"),
        Cap::Bone,
        false,
        Glyph::Left,
    )
    .clicked()
    {
        let i = (index + IR_NAMES.len() - 1) % IR_NAMES.len();
        edited.params = preset_params(&edited.params, i);
        all(edited, edits);
    }
    if gear::glyph_key(
        ui,
        row(top, 298.0, 28.0),
        sid.with("sd_next"),
        Cap::Bone,
        false,
        Glyph::Right,
    )
    .clicked()
    {
        let i = (index + 1) % IR_NAMES.len();
        edited.params = preset_params(&edited.params, i);
        all(edited, edits);
    }
    let comparing = mem.stashed.is_some();
    if gear::key(
        ui,
        row(top, 336.0, 100.0),
        sid.with("sd_compare"),
        tr("Compare"),
        Cap::Bone,
        LedAt::Left,
        comparing,
        comparing,
    )
    .clicked()
    {
        match mem.stashed.take() {
            Some(back) => edited.params = back,
            None => {
                mem.stashed = Some(edited.params);
                if let Some(o) = mem.original {
                    edited.params = o;
                }
            }
        }
        all(edited, edits);
    }
    if glyph_key_enabled(
        ui,
        row(top, 446.0, 30.0),
        sid.with("sd_copy"),
        Glyph::Copy,
        true,
        tr("Copy"),
    ) {
        if let Ok(mut c) = clipboard().lock() {
            *c = Some(edited.params);
        }
    }
    let have_clip = clipboard().lock().is_ok_and(|c| c.is_some());
    if glyph_key_enabled(
        ui,
        row(top, 480.0, 30.0),
        sid.with("sd_paste"),
        Glyph::Paste,
        have_clip,
        tr("Paste"),
    ) {
        if let Some(c) = clipboard().lock().ok().and_then(|c| *c) {
            edited.params = c;
            all(edited, edits);
        }
    }
    if glyph_key_enabled(
        ui,
        row(top, 520.0, 30.0),
        sid.with("sd_undo"),
        Glyph::Undo,
        !mem.undo.is_empty(),
        tr("Undo"),
    ) {
        if let Some(prev) = mem.undo.pop() {
            mem.redo.push(edited.params);
            edited.params = prev;
            all(edited, edits);
        }
    }
    if glyph_key_enabled(
        ui,
        row(top, 554.0, 30.0),
        sid.with("sd_redo"),
        Glyph::Redo,
        !mem.redo.is_empty(),
        tr("Redo"),
    ) {
        if let Some(next) = mem.redo.pop() {
            mem.undo.push(edited.params);
            edited.params = next;
            all(edited, edits);
        }
    }
    // View and Link, at the right.
    gear::silk(
        &g,
        pos2(x_at(top, 600.0), top.top() + 25.0 * k),
        Align2::LEFT_CENTER,
        tr("View:").trim_end_matches(':'),
        11.0,
        gear::INK2,
    );
    let views = ["75 %", "100 %", "125 %", "150 %"];
    let cur = [75u32, 100, 125, 150]
        .iter()
        .position(|v| *v == mem.view)
        .unwrap_or(1);
    if let Some(i) = gear::choice_field(
        ui,
        row(top, 636.0, 98.0),
        sid.with("sd_view"),
        &views,
        cur,
        true,
    ) {
        mem.view = [75u32, 100, 125, 150][i];
    }

    // ---- Sampled IR / Synthesized IR ----------------------------------------------------
    let tabs = next(TAB_H);
    let tab_w = (w - 2.0 * PAD * k - 10.0 * k) / 2.0;
    for (n, label) in [tr("SAMPLED IR"), tr("SYNTHESIZED IR")]
        .into_iter()
        .enumerate()
    {
        let r = Rect::from_min_size(
            pos2(
                tabs.left() + PAD * k + n as f32 * (tab_w + 10.0 * k),
                tabs.top() + 6.0 * k,
            ),
            vec2(tab_w, 28.0 * k),
        );
        let on = (edited.params[sd::IR_TYPE] >= 0.5) == (n == 1);
        if gear::key(
            ui,
            r,
            sid.with(("sd_tab", n)),
            label,
            Cap::Bone,
            LedAt::Left,
            on,
            on,
        )
        .clicked()
            && !on
        {
            set(edited, edits, sd::IR_TYPE, n as f32, true);
        }
    }

    // ---- Envelope keys, the response's field and Output EQ ------------------------------
    let sub = next(SUB_H);
    // A key whose LED switches it on or off; the rest of the key shows its envelope in the
    // display (it then sits latched). Returns (LED clicked, rest clicked).
    let env_key = |ui: &mut Ui, x: f32, wd: f32, label: &str, on: bool, shown: bool| {
        let r = Rect::from_min_size(
            pos2(sub.left() + x * k, sub.top() + 8.0 * k),
            vec2(wd * k, 28.0 * k),
        );
        let power = Rect::from_min_size(r.min, vec2(24.0 * k, r.height()));
        let rest = Rect::from_min_max(pos2(power.right(), r.top()), r.max);
        let pr = ui.interact(
            power,
            ui.scope_id().with(("sd_power_zone", label)),
            Sense::click(),
        );
        let rr = ui.interact(
            rest,
            ui.scope_id().with(("sd_select_zone", label)),
            Sense::click(),
        );
        let p = ui.painter();
        let face = gear::paint_key(p, r, Cap::Bone, shown);
        gear::led(p, pos2(face.left() + 12.0 * k, face.center().y), 3.0, on);
        let t = gear::fit(p, label, 11.0, face.width() - 30.0 * k);
        gear::legend(
            p,
            pos2(face.left() + (wd * k + 22.0 * k) / 2.0, face.center().y),
            Align2::CENTER_CENTER,
            &t,
            11.0,
            gear::INK,
        );
        (pr.clicked(), rr.clicked())
    };
    let (power, select) = env_key(
        ui,
        PAD,
        128.0,
        tr("Volume Env"),
        edited.params[sd::VOLUME_ENV] >= 0.5,
        !mem.filter_view,
    );
    if power {
        let v = 1.0 - edited.params[sd::VOLUME_ENV].round();
        set(edited, edits, sd::VOLUME_ENV, v, true);
    }
    if select || power {
        mem.filter_view = false;
    }
    let (power, select) = env_key(
        ui,
        PAD + 136.0,
        120.0,
        tr("Filter Env"),
        edited.params[sd::FILTER_ENV] >= 0.5,
        mem.filter_view,
    );
    if power {
        let v = 1.0 - edited.params[sd::FILTER_ENV].round();
        set(edited, edits, sd::FILTER_ENV, v, true);
    }
    if select || power {
        mem.filter_view = true;
    }
    {
        let synth = edited.params[sd::IR_TYPE] >= 0.5;
        let text = if synth {
            tr("Synthesized IR").to_string()
        } else {
            IR_NAMES[index].replace(' ', "_")
        };
        let r = Rect::from_min_size(
            pos2(sub.left() + 290.0 * k, sub.top() + 9.0 * k),
            vec2(226.0 * k, 26.0 * k),
        );
        let resp = gear::field(ui, r, sid.with("sd_ir_name"), &text, true);
        egui::Popup::menu(&resp).show(|ui| {
            for (i, n) in IR_NAMES.iter().enumerate() {
                if ui.selectable_label(!synth && i == index, *n).clicked() {
                    // Only the response changes; the envelope and the rest stay.
                    edited.params[sd::IR_TYPE] = 0.0;
                    edited.params[sd::IR_INDEX] = i as f32;
                    for p in [sd::IR_TYPE, sd::IR_INDEX] {
                        edits.push(Edit {
                            param: p,
                            value: edited.params[p],
                            ended: true,
                        });
                    }
                }
            }
        });
    }
    gear::silk(
        &g,
        pos2(sub.left() + 524.0 * k, sub.top() + 22.0 * k),
        Align2::LEFT_CENTER,
        tr("Stereo"),
        11.0,
        gear::PRINT,
    );
    let out_eq = edited.params[sd::OUTPUT_EQ] >= 0.5;
    let r = Rect::from_min_size(
        pos2(sub.right() - (PAD + 138.0) * k, sub.top() + 8.0 * k),
        vec2(138.0 * k, 28.0 * k),
    );
    if gear::key(
        ui,
        r,
        sid.with("sd_out_eq"),
        tr("Output EQ"),
        Cap::Bone,
        LedAt::Left,
        out_eq,
        false,
    )
    .clicked()
    {
        let v = 1.0 - edited.params[sd::OUTPUT_EQ].round();
        set(edited, edits, sd::OUTPUT_EQ, v, true);
    }

    // ---- The response and its envelope on the LCD ----------------------------------------
    let display = next(DISPLAY_H);
    let lcd = Rect::from_min_max(
        pos2(display.left() + PAD * k, display.top() + 4.0 * k),
        pos2(display.right() - PAD * k, display.bottom() - 6.0 * k),
    );
    let glass = gear::lcd(&g, lcd);
    draw_display(ui, glass, edited, edits, mem, k, &editable, &mut set);

    // ---- The LCD strip of values under the display ---------------------------------------
    let readout = next(READOUT_H);
    let strip = Rect::from_min_max(
        pos2(readout.left() + PAD * k, readout.top() + 4.0 * k),
        pos2(readout.right() - PAD * k, readout.bottom() - 6.0 * k),
    );
    gear::glass(&g, strip);
    if mem.filter_view {
        filter_readout(ui, strip, edited, edits, k, &editable, &mut set);
    } else {
        g_readout(ui, strip, edited, edits, k, &editable, &mut set);
    }

    // ---- Options row: Quality, IR Offset, Reverse (and the synthesized response's) -------
    let opt = next(OPTIONS_H);
    gear::seam_h(&g, opt.left() + 1.0, opt.right() - 1.0, opt.top() + 2.0);
    let oy = opt.top() + 24.0 * k;
    let field_at = |x: f32, wd: f32| {
        Rect::from_min_size(
            pos2(opt.left() + x * k, oy - 12.0 * k),
            vec2(wd * k, 24.0 * k),
        )
    };
    gear::silk(
        &g,
        pos2(x_at(opt, PAD), oy),
        Align2::LEFT_CENTER,
        tr("Quality:").trim_end_matches(':'),
        11.0,
        gear::INK2,
    );
    let high = edited.params[sd::QUALITY] >= 0.5;
    if let Some(i) = gear::choice_field(
        ui,
        field_at(PAD + 58.0, 82.0),
        sid.with("sd_quality"),
        &[tr("High"), tr("Low")],
        if high { 0 } else { 1 },
        editable(sd::QUALITY),
    ) {
        set(
            edited,
            edits,
            sd::QUALITY,
            if i == 0 { 1.0 } else { 0.0 },
            true,
        );
    }
    gear::silk(
        &g,
        pos2(x_at(opt, 186.0), oy),
        Align2::LEFT_CENTER,
        tr("IR Offset:").trim_end_matches(':'),
        11.0,
        gear::INK2,
    );
    let off = shown(edited, sd::IR_OFFSET);
    if let Some(v) = drag_value(
        ui,
        field_at(254.0, 66.0),
        "sd_offset",
        &format!("{off:.2} s"),
        off,
        0.0,
        2.0,
        0.005,
        editable(sd::IR_OFFSET),
        k,
    ) {
        set(edited, edits, sd::IR_OFFSET, v, false);
    }
    let reversed = edited.params[sd::REVERSE] >= 0.5;
    if gear::key(
        ui,
        field_at(330.0, 92.0),
        sid.with("sd_reverse"),
        tr("Reverse"),
        Cap::Bone,
        LedAt::Left,
        reversed,
        reversed,
    )
    .clicked()
        && editable(sd::REVERSE)
    {
        set(
            edited,
            edits,
            sd::REVERSE,
            if reversed { 0.0 } else { 1.0 },
            true,
        );
    }
    // Synthesized IR: its own three controls, after Reverse.
    if edited.params[sd::IR_TYPE] >= 0.5 {
        for (n, (p, label)) in [
            (sd::DENSITY, tr("Density")),
            (sd::DIFFUSE, tr("Diffuse")),
            (sd::EARLY, tr("Early Reflections")),
        ]
        .into_iter()
        .enumerate()
        {
            let x = 436.0 + n as f32 * 100.0;
            let v = shown(edited, p);
            let short = gear::fit(&g, &label.to_uppercase(), 9.5, 50.0 * k);
            gear::text_at(
                &g,
                pos2(x_at(opt, x), oy),
                Align2::LEFT_CENTER,
                &short,
                gear::silk_font(10.5),
                gear::INK2,
                0.1,
            );
            if let Some(nv) = drag_value(
                ui,
                field_at(x + 50.0, 46.0),
                &format!("sd_synth_{p}"),
                &format!("{:.0} %", v * 100.0),
                v,
                0.0,
                1.0,
                0.004,
                editable(p),
                k,
            ) {
                set(edited, edits, p, nv, false);
            }
        }
    }

    // ---- The controls --------------------------------------------------------------------
    let panel = next(PANEL_H);
    gear::seam_h(
        &g,
        panel.left() + 1.0,
        panel.right() - 1.0,
        panel.top() + 2.0,
    );
    let top_y = panel.top() + 8.0 * k;
    let label_y = top_y + 12.0 * k;
    let chip = |cx: f32, wd: f32| {
        Rect::from_center_size(pos2(cx, top_y + 182.0 * k), vec2(wd * k, 24.0 * k))
    };
    let label = |cx: f32, t: &str| {
        gear::silk(
            &g,
            pos2(cx, label_y),
            Align2::CENTER_CENTER,
            t,
            11.0,
            gear::INK2,
        );
    };

    // Input fader with its meter beside it.
    {
        let cx = x_at(panel, 54.0);
        let v = shown(edited, sd::INPUT);
        label(cx + 6.0 * k, tr("Input"));
        if let Some(n) = fader(
            ui,
            &g,
            pos2(cx, top_y + 34.0 * k),
            124.0 * k,
            "sd_input",
            &params[sd::INPUT],
            v,
            editable(sd::INPUT),
            k,
        ) {
            set(edited, edits, sd::INPUT, n, false);
        }
        // The level going into the convolver, -60 to +6 dB.
        let db = 20.0 * input_level.max(1e-6).log10();
        let frac = ((db + 60.0) / 66.0).clamp(0.0, 1.0);
        let bar = Rect::from_min_size(
            pos2(cx + 22.0 * k, top_y + 34.0 * k),
            vec2(8.0 * k, 124.0 * k),
        );
        crate::hw::paint_meter_v(&g, bar, frac, 0.0);
        gear::lcd_chip(
            &g,
            chip(cx + 6.0 * k, 76.0),
            "",
            &readout_text(sd::INPUT, v),
        );
    }

    // The knobs: Predelay, Length, Size; then X-Over; then Lo and Hi Spread.
    let knob_d = 46.0 * k;
    let knob_y = top_y + 82.0 * k;
    let knobs: [(usize, &str, f32); 4] = [
        (sd::PREDELAY, "Predelay", 150.0),
        (sd::LENGTH, "Length", 232.0),
        (sd::SIZE, "Size", 314.0),
        (sd::LO_SPREAD, "Lo Spread", 480.0),
    ];
    let scales = |p: usize| -> (&'static str, &'static str) {
        match p {
            sd::SIZE => ("10", "1000"),
            sd::LO_SPREAD | sd::HI_SPREAD => ("0", "100"),
            _ => ("", ""),
        }
    };
    let turn =
        |ui: &mut Ui, edited: &mut Insert, edits: &mut Vec<Edit>, p: usize, cx: f32, name: &str| {
            let v = shown(edited, p);
            label(cx, name);
            let info = &params[p];
            let out = gear::knob(
                ui,
                sid.with(("sd_knob", p)),
                pos2(cx, knob_y),
                knob_d,
                info.to_norm(v),
                info.to_norm(info.default),
                true,
                editable(p),
            );
            if let Some(n) = out.value {
                set(edited, edits, p, info.from_norm(n), out.ended);
            } else if out.ended {
                set(edited, edits, p, v, true);
            }
            let (lo, hi) = scales(p);
            for (t, dx) in [(lo, -1.0f32), (hi, 1.0)] {
                if !t.is_empty() {
                    gear::silk(
                        &g,
                        pos2(cx + dx * 26.0 * k, knob_y + 44.0 * k),
                        Align2::CENTER_CENTER,
                        t,
                        9.5,
                        gear::PRINT,
                    );
                }
            }
            gear::lcd_chip(&g, chip(cx, 74.0), "", &readout_text(p, v));
        };
    for (p, name, x) in knobs {
        turn(ui, edited, edits, p, x_at(panel, x), tr(name));
    }
    turn(
        ui,
        edited,
        edits,
        sd::HI_SPREAD,
        x_at(panel, 562.0),
        tr("Hi Spread"),
    );

    // X-Over: a slide fader with its scale.
    {
        let (p, cx) = (sd::XOVER, x_at(panel, 398.0));
        let v = shown(edited, p);
        label(cx, tr("X-Over"));
        gear::silk(
            &g,
            pos2(cx + 20.0 * k, top_y + 34.0 * k),
            Align2::LEFT_CENTER,
            "16k",
            9.5,
            gear::PRINT,
        );
        gear::silk(
            &g,
            pos2(cx + 20.0 * k, top_y + 158.0 * k),
            Align2::LEFT_CENTER,
            "200",
            9.5,
            gear::PRINT,
        );
        if let Some(n) = fader(
            ui,
            &g,
            pos2(cx, top_y + 34.0 * k),
            124.0 * k,
            "sd_xover",
            &params[p],
            v,
            editable(p),
            k,
        ) {
            set(edited, edits, p, n, false);
        }
        gear::lcd_chip(&g, chip(cx, 66.0), "", &readout_text(p, v));
    }
    // Dry and Wet faders (right).
    for (p, name, x) in [(sd::DRY, "Dry", 654.0), (sd::WET, "Wet", 716.0)] {
        let cx = x_at(panel, x - 6.0);
        let v = shown(edited, p);
        label(cx, tr(name));
        if let Some(n) = fader(
            ui,
            &g,
            pos2(cx, top_y + 34.0 * k),
            124.0 * k,
            "sd_fader",
            &params[p],
            v,
            editable(p),
            k,
        ) {
            set(edited, edits, p, n, false);
        }
        gear::lcd_chip(&g, chip(cx, 56.0), "", &readout_text(p, v));
    }

    // ---- Output EQ: the two shelves, while it is on --------------------------------------
    let foot = next(FOOT_H);
    gear::seam_h(&g, foot.left() + 1.0, foot.right() - 1.0, foot.top() + 2.0);
    if edited.params[sd::OUTPUT_EQ] >= 0.5 {
        let fy = foot.top() + 23.0 * k;
        gear::bracket(&g, pos2(x_at(foot, PAD), fy), 92.0 * k, tr("Output EQ"));
        for (n, (p, name)) in [(sd::OUT_LO, tr("Low")), (sd::OUT_HI, tr("High"))]
            .into_iter()
            .enumerate()
        {
            let x = PAD + 112.0 + n as f32 * 150.0;
            let v = shown(edited, p);
            gear::silk(
                &g,
                pos2(x_at(foot, x), fy),
                Align2::LEFT_CENTER,
                name,
                11.0,
                gear::INK2,
            );
            let r = Rect::from_min_size(
                pos2(x_at(foot, x + 42.0), fy - 12.0 * k),
                vec2(84.0 * k, 24.0 * k),
            );
            if let Some(nv) = drag_value(
                ui,
                r,
                &format!("sd_eq_{p}"),
                &format!("{v:+.1} dB"),
                v,
                -12.0,
                12.0,
                0.05,
                editable(p),
                k,
            ) {
                set(edited, edits, p, nv, false);
            }
        }
    }
}

/// The text under a control, in its units.
fn readout_text(param: usize, v: f32) -> String {
    match param {
        sd::PREDELAY => format!("{v:.0} ms"),
        sd::LENGTH => format!("{v:.2} s"),
        sd::SIZE | sd::LO_SPREAD | sd::HI_SPREAD => format!("{v:.0} %"),
        sd::XOVER => format!("{v:.0} Hz"),
        sd::DRY | sd::WET | sd::INPUT => {
            if v <= -95.9 {
                "-∞ dB".to_string()
            } else {
                format!("{v:.1} dB")
            }
        }
        _ => format!("{v:.1}"),
    }
}

/// A slide fader standing up: a black slot with a printed scale at its left and a ribbed
/// cap. dB faders put the quiet part in the lower travel. Returns the new value while it
/// moves.
#[allow(clippy::too_many_arguments)]
fn fader(
    ui: &mut Ui,
    g: &egui::Painter,
    top: egui::Pos2,
    height: f32,
    id: &str,
    info: &prowave_engine::ParamInfo,
    v: f32,
    enabled: bool,
    k: f32,
) -> Option<f32> {
    let n = if info.min < -90.0 {
        db_to_pos(v, info)
    } else {
        info.to_norm(v)
    };
    for i in 0..=10 {
        let y = top.y + height * i as f32 / 10.0;
        let major = i % 5 == 0;
        g.line_segment(
            [
                pos2(top.x - (if major { 15.0 } else { 12.0 }) * k, y),
                pos2(top.x - 8.0 * k, y),
            ],
            Stroke::new(if major { 1.3 } else { 0.9 }, gear::INK2),
        );
    }
    let slot = Rect::from_min_max(
        pos2(top.x - 3.0 * k, top.y - 3.0 * k),
        pos2(top.x + 3.0 * k, top.y + height + 3.0 * k),
    );
    g.rect_filled(slot, 3.0, gear::INK);
    gear::inner_shadow(g, slot, 3.0, 1.0);
    let y = top.y + height * (1.0 - n);
    gear::fader_cap(g, pos2(top.x, y), 28.0 * k, 14.0 * k, false);
    let area = Rect::from_min_max(
        pos2(top.x - 16.0 * k, top.y - 8.0 * k),
        pos2(top.x + 16.0 * k, top.y + height + 8.0 * k),
    );
    let resp = ui.interact(
        area,
        ui.scope_id().with(id).with((top.x as i32, top.y as i32)),
        Sense::click_and_drag(),
    );
    if !enabled {
        return None;
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if resp.double_clicked() {
        return Some(info.default);
    }
    if resp.dragged() || resp.clicked() {
        let p = resp.interact_pointer_pos()?;
        let pos = (1.0 - (p.y - top.y) / height).clamp(0.0, 1.0);
        return Some(if info.min < -90.0 {
            pos_to_db(pos, info)
        } else {
            info.from_norm(pos)
        });
    }
    None
}

/// Where a dB value sits on a fader (0..1): the top quarter is -12 dB to the maximum, and the
/// bottom reaches minus infinity.
fn db_to_pos(db: f32, info: &prowave_engine::ParamInfo) -> f32 {
    let db = db.clamp(info.min, info.max);
    if db <= -95.9 {
        return 0.0;
    }
    // -60 dB at the bottom (just above off), the maximum at the top, a gentle curve between.
    let t = ((db + 60.0) / (info.max + 60.0)).clamp(0.0, 1.0);
    0.04 + 0.96 * t.sqrt()
}

fn pos_to_db(pos: f32, info: &prowave_engine::ParamInfo) -> f32 {
    if pos <= 0.02 {
        return info.min;
    }
    let t = ((pos - 0.04) / 0.96).clamp(0.0, 1.0);
    (t * t * (info.max + 60.0) - 60.0).clamp(-60.0, info.max)
}

/// A small LCD readout whose value is dragged up and down.
#[allow(clippy::too_many_arguments)]
fn drag_value(
    ui: &mut Ui,
    rect: Rect,
    id: &str,
    text: &str,
    v: f32,
    min: f32,
    max: f32,
    step: f32,
    enabled: bool,
    k: f32,
) -> Option<f32> {
    let resp = ui.interact(rect, ui.scope_id().with(id), Sense::click_and_drag());
    let _ = k;
    gear::lcd_chip(ui.painter(), rect, "", text);
    if !enabled {
        return None;
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if resp.double_clicked() {
        return Some(min);
    }
    resp.dragged()
        .then(|| (v - resp.drag_delta().y * step).clamp(min, max))
}

/// LCD text on the strip: a dim label then the value, dragged up and down. Returns the value
/// while dragged (and whether the drag ended).
#[allow(clippy::too_many_arguments)]
fn strip_value(
    ui: &mut Ui,
    rect: Rect,
    x: f32,
    label: &str,
    value: &str,
    id: (&str, usize),
    k: f32,
) -> (egui::Response, f32) {
    let p = ui.painter().clone();
    let lr = gear::lcd_label(&p, pos2(x, rect.center().y), Align2::LEFT_CENTER, label);
    let vx = lr.right() + 6.0 * k;
    let vr = gear::lcd_text(
        &p,
        pos2(vx, rect.center().y),
        Align2::LEFT_CENTER,
        value,
        13.0,
    );
    let hit = Rect::from_min_max(
        pos2(vx - 2.0, rect.top()),
        pos2(vr.right().max(vx + 60.0 * k), rect.bottom()),
    );
    let resp = ui.interact(hit, ui.scope_id().with(id), Sense::click_and_drag());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    (resp, vr.right())
}

/// The LCD strip of the Volume Envelope: "INIT LEVEL 100%  ATTACK 122 MS  DECAY 4.52 S
/// LIN EXP  END LEVEL 18%".
#[allow(clippy::too_many_arguments)]
fn g_readout(
    ui: &mut Ui,
    rect: Rect,
    edited: &mut Insert,
    edits: &mut Vec<Edit>,
    k: f32,
    editable: &dyn Fn(usize) -> bool,
    set: &mut SetFn,
) {
    let items: [(usize, &str, f32, f32, f32); 3] = [
        (sd::INIT_LEVEL, "Init Level", 12.0, 0.0, 100.0),
        (sd::ATTACK, "Attack", 172.0, 0.0, 2000.0),
        (sd::DECAY, "Decay", 318.0, 0.01, 10.0),
    ];
    let text_of = |p: usize, v: f32| match p {
        sd::INIT_LEVEL | sd::END_LEVEL => format!("{v:.0}%"),
        sd::ATTACK => format!("{v:.0} ms"),
        _ => format!("{v:.2} s"),
    };
    let mut place = |ui: &mut Ui, p: usize, label: &str, x: f32, lo: f32, hi: f32| {
        let v = edited.params[p];
        let (resp, _) = strip_value(
            ui,
            rect,
            rect.left() + x * k,
            label,
            &text_of(p, v),
            ("sd_read", p),
            k,
        );
        if editable(p) && resp.dragged() {
            let scale = (hi - lo) / 300.0;
            set(
                edited,
                edits,
                p,
                (v - resp.drag_delta().y * scale).clamp(lo, hi),
                false,
            );
        } else if resp.drag_stopped() {
            set(edited, edits, p, v, true);
        }
    };
    for (p, label, x, lo, hi) in items {
        place(ui, p, label, x, lo, hi);
    }
    place(ui, sd::END_LEVEL, "End Level", 556.0, 0.0, 100.0);
    // LIN / EXP: the one in use shown inverted, as segments on the glass.
    let exp = edited.params[sd::CURVE] >= 0.5;
    for (n, label) in ["LIN", "EXP"].into_iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(
                rect.left() + (452.0 + n as f32 * 46.0) * k,
                rect.center().y - 10.0 * k,
            ),
            vec2(42.0 * k, 20.0 * k),
        );
        let on = exp == (n == 1);
        let resp = ui.interact(r, ui.scope_id().with(("sd_curve", n)), Sense::click());
        let p = ui.painter();
        if on {
            p.rect_filled(r, 2.0, gear::LCD_INK);
            gear::text_at(
                p,
                r.center(),
                Align2::CENTER_CENTER,
                label,
                gear::mono_heavy(12.0),
                gear::LCD,
                0.02,
            );
        } else {
            p.rect_stroke(r, 2.0, Stroke::new(1.0, ink(0.5)), egui::StrokeKind::Inside);
            gear::lcd_text(p, r.center(), Align2::CENTER_CENTER, label, 12.0);
        }
        if resp.clicked() && editable(sd::CURVE) {
            set(edited, edits, sd::CURVE, n as f32, true);
        }
    }
}

/// The response drawn on the LCD glass, the Volume Envelope over it and its three handles.
#[allow(clippy::too_many_arguments)]
fn draw_display(
    ui: &mut Ui,
    rect: Rect,
    edited: &mut Insert,
    edits: &mut Vec<Edit>,
    mem: &mut Memory,
    k: f32,
    editable: &dyn Fn(usize) -> bool,
    set: &mut SetFn,
) {
    let g = ui.painter_at(rect);
    let plot = Rect::from_min_max(
        pos2(rect.left() + 34.0 * k, rect.top() + 16.0 * k),
        pos2(rect.right() - 24.0 * k, rect.bottom() - 22.0 * k),
    );
    // The response, made again when a setting that shapes it changes.
    let key: Vec<u32> = prowave_engine::IR_PARAMS
        .iter()
        .map(|&i| edited.params[i].to_bits())
        .collect();
    let bins = plot.width().max(8.0) as usize;
    if mem
        .preview
        .as_ref()
        .is_none_or(|(kk, b, _)| *kk != key || b.len() != bins)
    {
        let (peaks, secs) = prowave_engine::ir_preview(&edited.params, 48_000.0, bins);
        mem.preview = Some((key, peaks, secs));
    }
    let Some((_, peaks, secs)) = mem.preview.clone() else {
        return;
    };
    let secs = secs.max(0.05);
    let mid = plot.center().y;
    let half = plot.height() * 0.5;
    let x_of = |t: f32| plot.left() + (t / secs).clamp(0.0, 1.0) * plot.width();
    let y_of = |a: f32| plot.bottom() - a.clamp(0.0, 1.0) * plot.height();

    // The axes: seconds along the bottom, with faint grid lines.
    let mut t = 0.0;
    while t <= secs + 1e-3 {
        let x = x_of(t);
        gear::lcd_label(
            &g,
            pos2(x, plot.bottom() + 5.0 * k),
            if t == 0.0 {
                Align2::LEFT_TOP
            } else {
                Align2::CENTER_TOP
            },
            &format!("{t:.1}"),
        );
        if t > 0.0 {
            g.line_segment(
                [pos2(x, plot.top()), pos2(x, plot.bottom())],
                Stroke::new(1.0, ink(0.12)),
            );
        }
        t += if secs > 6.0 {
            2.0
        } else if secs > 2.5 {
            1.0
        } else if secs > 1.0 {
            0.5
        } else {
            0.1
        };
    }
    g.line_segment(
        [pos2(plot.left(), mid), pos2(plot.right(), mid)],
        Stroke::new(1.0, ink(0.12)),
    );
    // The filled waveform, as a ghost of the ink.
    for (i, (lo, hi)) in peaks.iter().enumerate() {
        let x = plot.left() + i as f32;
        g.line_segment(
            [
                pos2(x, mid - hi.min(1.0) * half * 0.9),
                pos2(x, mid - lo.max(-1.0) * half * 0.9),
            ],
            Stroke::new(1.0, ink(0.32)),
        );
    }
    // The envelope: a ghost fill under the line, then the line (the Filter Envelope has its own).
    let env_on = edited.params[sd::VOLUME_ENV] >= 0.5;
    let n = plot.width() as usize;
    let pts: Vec<egui::Pos2> = (0..=n)
        .map(|i| {
            let t = secs * i as f32 / n.max(1) as f32;
            let a = if env_on {
                prowave_engine::volume_env(&edited.params, t)
            } else {
                1.0
            };
            pos2(plot.left() + i as f32, y_of(a))
        })
        .collect();
    if !mem.filter_view {
        let mut mesh = egui::Mesh::default();
        for w in pts.windows(2) {
            let b = mesh.vertices.len() as u32;
            for q in [
                w[0],
                w[1],
                pos2(w[1].x, plot.bottom()),
                pos2(w[0].x, plot.bottom()),
            ] {
                mesh.colored_vertex(q, ink(0.09));
            }
            mesh.add_triangle(b, b + 1, b + 2);
            mesh.add_triangle(b, b + 2, b + 3);
        }
        g.add(Shape::mesh(mesh));
        g.add(Shape::line(pts, Stroke::new(2.2 * k, gear::LCD_INK)));
    }

    if mem.filter_view {
        filter_overlay(ui, plot, secs, edited, edits, k, editable, set);
        return;
    }
    if !env_on {
        return;
    }
    // The three handles: Init Level (left), Attack (on the baseline), End Level (where the
    // decay ends).
    let attack = edited.params[sd::ATTACK] / 1000.0;
    let decay = edited.params[sd::DECAY];
    let end_t = (attack + decay).min(secs);
    let init = edited.params[sd::INIT_LEVEL] / 100.0;
    let end = edited.params[sd::END_LEVEL] / 100.0;
    let dots = [
        (0, pos2(x_of(0.0), y_of(init))),
        (1, pos2(x_of(attack), y_of(0.0))),
        (2, pos2(x_of(end_t), y_of(end))),
    ];
    for (which, c) in dots {
        let hit = Rect::from_center_size(c, vec2(18.0 * k, 18.0 * k));
        let resp = ui.interact(hit, ui.scope_id().with(("sd_dot", which)), Sense::drag());
        handle(&g, c, k, true, resp.hovered() || resp.dragged());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if !resp.dragged() {
            if resp.drag_stopped() {
                let p = match which {
                    0 => sd::INIT_LEVEL,
                    1 => sd::ATTACK,
                    _ => sd::END_LEVEL,
                };
                set(edited, edits, p, edited.params[p], true);
            }
            continue;
        }
        let Some(pt) = resp.interact_pointer_pos() else {
            continue;
        };
        let time = ((pt.x - plot.left()) / plot.width()).clamp(0.0, 1.0) * secs;
        let level = ((plot.bottom() - pt.y) / plot.height()).clamp(0.0, 1.0);
        match which {
            0 if editable(sd::INIT_LEVEL) => {
                set(edited, edits, sd::INIT_LEVEL, level * 100.0, false)
            }
            1 if editable(sd::ATTACK) => set(
                edited,
                edits,
                sd::ATTACK,
                (time * 1000.0).min(secs * 1000.0 * 0.9),
                false,
            ),
            2 => {
                if editable(sd::END_LEVEL) {
                    set(edited, edits, sd::END_LEVEL, level * 100.0, false);
                }
                if editable(sd::DECAY) {
                    set(edited, edits, sd::DECAY, (time - attack).max(0.01), false);
                }
            }
            _ => {}
        }
    }
}

/// An envelope handle on the glass: a small ink square (hollow while its envelope is off),
/// with a ring while the pointer is on it.
fn handle(g: &egui::Painter, c: egui::Pos2, k: f32, on: bool, hot: bool) {
    let r = Rect::from_center_size(c, vec2(9.0 * k, 9.0 * k));
    if on {
        g.rect_filled(r, 1.0, gear::LCD_INK);
        g.rect_filled(r.shrink(2.5 * k), 0.0, gear::LCD);
    } else {
        g.rect_stroke(r, 1.0, Stroke::new(1.5, ink(0.5)), egui::StrokeKind::Inside);
    }
    if hot {
        g.rect_stroke(
            r.expand(3.0 * k),
            2.0,
            Stroke::new(1.0, gear::LCD_INK),
            egui::StrokeKind::Outside,
        );
    }
}

/// The Filter Envelope over the display: the cut-off along the response, from its start value to
/// its end value (drawn on a log scale from 100 Hz to 20 kHz), with a handle at each end.
#[allow(clippy::too_many_arguments)]
fn filter_overlay(
    ui: &mut Ui,
    plot: Rect,
    secs: f32,
    edited: &mut Insert,
    edits: &mut Vec<Edit>,
    k: f32,
    editable: &dyn Fn(usize) -> bool,
    set: &mut SetFn,
) {
    let g = ui.painter_at(plot.expand(30.0 * k));
    let on = edited.params[sd::FILTER_ENV] >= 0.5;
    let hz_to_y =
        |hz: f32| plot.bottom() - ((hz / 100.0).ln() / 200f32.ln()).clamp(0.0, 1.0) * plot.height();
    let y_to_hz =
        |y: f32| 100.0 * 200f32.powf(((plot.bottom() - y) / plot.height()).clamp(0.0, 1.0));
    for (hz, label) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        let y = hz_to_y(hz);
        g.line_segment(
            [pos2(plot.left(), y), pos2(plot.right(), y)],
            Stroke::new(1.0, ink(0.12)),
        );
        gear::lcd_label(
            &g,
            pos2(plot.left() - 5.0 * k, y),
            Align2::RIGHT_CENTER,
            label,
        );
    }
    let (start, end) = (
        edited.params[sd::FILTER_START],
        edited.params[sd::FILTER_END],
    );
    let colour = if on { gear::LCD_INK } else { ink(0.4) };
    let n = plot.width() as usize;
    let pts: Vec<egui::Pos2> = (0..=n)
        .map(|i| {
            let hz = start * (end / start).powf(i as f32 / n.max(1) as f32);
            pos2(plot.left() + i as f32, hz_to_y(hz))
        })
        .collect();
    g.add(Shape::line(pts, Stroke::new(2.2 * k, colour)));
    let _ = secs;
    for (which, c, p) in [
        (0, pos2(plot.left(), hz_to_y(start)), sd::FILTER_START),
        (1, pos2(plot.right(), hz_to_y(end)), sd::FILTER_END),
    ] {
        let resp = ui.interact(
            Rect::from_center_size(c, vec2(18.0 * k, 18.0 * k)),
            ui.scope_id().with(("sd_filter_dot", which)),
            Sense::drag(),
        );
        handle(&g, c, k, on, resp.hovered() || resp.dragged());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if resp.dragged() && editable(p) {
            if let Some(pt) = resp.interact_pointer_pos() {
                set(edited, edits, p, y_to_hz(pt.y), false);
            }
        } else if resp.drag_stopped() {
            set(edited, edits, p, edited.params[p], true);
        }
    }
}

/// The LCD strip while the display shows the Filter Envelope: "START 20.0 KHZ  END 5.0 KHZ
/// RESONANCE 10 %  LOW PASS", each value dragged up and down.
fn filter_readout(
    ui: &mut Ui,
    rect: Rect,
    edited: &mut Insert,
    edits: &mut Vec<Edit>,
    k: f32,
    editable: &dyn Fn(usize) -> bool,
    set: &mut SetFn,
) {
    let hz_text = |hz: f32| {
        if hz >= 1000.0 {
            format!("{:.1} kHz", hz / 1000.0)
        } else {
            format!("{hz:.0} Hz")
        }
    };
    let info = FxKind::Reverb.params();
    for (p, label, x) in [
        (sd::FILTER_START, tr("Start:"), 12.0),
        (sd::FILTER_END, tr("End:"), 196.0),
        (sd::RESONANCE, tr("Resonance:"), 366.0),
    ] {
        let v = edited.params[p];
        let text = if p == sd::RESONANCE {
            format!("{:.0} %", v * 100.0)
        } else {
            hz_text(v)
        };
        let (resp, _) = strip_value(
            ui,
            rect,
            rect.left() + x * k,
            label.trim_end_matches(':'),
            &text,
            ("sd_filter_read", p),
            k,
        );
        if editable(p) && resp.dragged() {
            let n = info[p].to_norm(v) - resp.drag_delta().y * 0.004;
            set(edited, edits, p, info[p].from_norm(n), false);
        } else if resp.drag_stopped() {
            set(edited, edits, p, v, true);
        }
    }
    gear::lcd_text(
        ui.painter(),
        pos2(rect.right() - 12.0 * k, rect.center().y),
        Align2::RIGHT_CENTER,
        tr("Low Pass"),
        13.0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preset_sets_its_response_and_length_and_keeps_the_mix() {
        let mut base = FxKind::Reverb.defaults();
        base[sd::DRY] = -3.0;
        base[sd::WET] = -9.0;
        base[sd::REVERSE] = 1.0;
        let p = preset_params(&base, 4);
        assert_eq!(p[sd::IR_INDEX], 4.0);
        assert_eq!(p[sd::IR_TYPE], 0.0);
        assert_eq!(p[sd::LENGTH], 6.0);
        assert_eq!(p[sd::REVERSE], 0.0);
        assert_eq!((p[sd::DRY], p[sd::WET]), (-3.0, -9.0));
        assert_eq!(
            FxKind::Reverb.clamp_params(p),
            p,
            "every setting is in range"
        );
        for i in 0..IR_NAMES.len() {
            let q = preset_params(&base, i);
            assert_eq!(FxKind::Reverb.clamp_params(q), q);
        }
    }

    #[test]
    fn a_db_fader_runs_from_minus_infinity_to_the_maximum_and_back() {
        let info = FxKind::Reverb.params()[sd::DRY];
        assert_eq!(db_to_pos(-96.0, &info), 0.0);
        assert!((db_to_pos(6.0, &info) - 1.0).abs() < 1e-5);
        for db in [-50.0, -20.0, -12.5, -7.0, 0.0, 6.0] {
            let back = pos_to_db(db_to_pos(db, &info), &info);
            assert!((back - db).abs() < 0.05, "{db} -> {back}");
        }
        assert_eq!(pos_to_db(0.0, &info), -96.0);
    }

    #[test]
    fn the_readouts_use_the_units_of_the_reference() {
        assert_eq!(readout_text(sd::PREDELAY, 15.0), "15 ms");
        assert_eq!(readout_text(sd::LENGTH, 4.52), "4.52 s");
        assert_eq!(readout_text(sd::SIZE, 100.0), "100 %");
        assert_eq!(readout_text(sd::XOVER, 710.0), "710 Hz");
        assert_eq!(readout_text(sd::DRY, -12.5), "-12.5 dB");
        assert_eq!(readout_text(sd::WET, -96.0), "-∞ dB");
    }
}
