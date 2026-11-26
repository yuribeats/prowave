//! The Arpeggiator's rack unit in the MIDI FX window: a bar of keys (Presets, Randomize,
//! Latch), the step pattern on an LCD (each step's transpose, its velocity bar drawn by
//! dragging, its tie into the next step and whether it plays the whole chord), a row of step
//! keys with LEDs that switch the steps on and off, and a row of knobs and fields for the
//! rest.
use crate::arp_patterns::{randomize, PATTERNS};
use crate::gear::{self, Cap, LedAt};
use crate::i18n::tr;
use crate::midi_fx_ui::{choice_control, int_control, int_of, row};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Painter, Pos2, Rect, Sense, Stroke, Ui};
use prowave_engine::{ArpMode, ArpRate, ArpSettings, ARP_STEPS};

/// The height of the unit's control area.
pub const HEIGHT: f32 = 434.0;
/// The lane names' column on the LCD.
const LABELS: f32 = 84.0;
/// Lane heights on the LCD.
const NUMBERS_H: f32 = 12.0;
const TRANSPOSE_H: f32 = 22.0;
const VELOCITY_H: f32 = 92.0;
const MARK_H: f32 = 20.0;
const LANE_GAP: f32 = 4.0;

pub fn mode_name(m: ArpMode) -> &'static str {
    match m {
        ArpMode::Up => tr("Up"),
        ArpMode::Down => tr("Down"),
        ArpMode::UpDown => tr("Up and Down"),
        ArpMode::DownUp => tr("Down and Up"),
        ArpMode::Random => tr("Random"),
        ArpMode::AsPlayed => tr("As Played"),
        ArpMode::Chord => tr("Chord"),
    }
}

/// LCD ink.
fn ink(a: f32) -> Color32 {
    gear::alpha(gear::LCD_INK(), a)
}

/// Where the pattern is drawn: the columns of the steps and the lanes, top to bottom.
struct Grid {
    left: f32,
    col: f32,
    numbers: Rect,
    transpose: Rect,
    velocity: Rect,
    tie: Rect,
    chord: Rect,
}

impl Grid {
    fn new(glass: Rect) -> Self {
        let left = glass.left() + LABELS;
        let right = glass.right() - 8.0;
        let lane = |top: f32, h: f32| Rect::from_min_max(pos2(left, top), pos2(right, top + h));
        let numbers = lane(glass.top() + 6.0, NUMBERS_H);
        let transpose = lane(numbers.bottom() + 2.0, TRANSPOSE_H);
        let velocity = lane(transpose.bottom() + LANE_GAP, VELOCITY_H);
        let tie = lane(velocity.bottom() + LANE_GAP, MARK_H);
        let chord = lane(tie.bottom() + LANE_GAP, MARK_H);
        Self {
            left,
            col: (right - left) / ARP_STEPS as f32,
            numbers,
            transpose,
            velocity,
            tie,
            chord,
        }
    }

    /// The height of the glass the lanes need.
    fn glass_height() -> f32 {
        6.0 + NUMBERS_H + 2.0 + TRANSPOSE_H + VELOCITY_H + 2.0 * MARK_H + 3.0 * LANE_GAP + 7.0
    }

    fn x(&self, step: usize) -> f32 {
        self.left + step as f32 * self.col
    }

    /// Step `i`'s cell in `lane`.
    fn cell(&self, lane: Rect, i: usize) -> Rect {
        Rect::from_min_max(
            pos2(self.x(i), lane.top()),
            pos2(self.x(i + 1), lane.bottom()),
        )
    }

    /// The step under `x`.
    fn step_at(&self, x: f32) -> usize {
        (((x - self.left) / self.col).floor().max(0.0) as usize).min(ARP_STEPS - 1)
    }
}

/// For each step, the step whose note it belongs to: itself when it starts a note (or
/// rests), the step it is tied from otherwise. As the generator plays them from step 0.
fn runs(arp: &ArpSettings) -> [usize; ARP_STEPS] {
    let len = usize::from(arp.length.clamp(1, ARP_STEPS as u8));
    let mut head: [usize; ARP_STEPS] = std::array::from_fn(|i| i);
    let mut i = 0;
    while i < len {
        let mut run = 1;
        if arp.steps[i].on {
            while run < len && arp.steps[(i + run - 1) % len].tie {
                run += 1;
            }
        }
        for k in 1..run {
            if i + k < len {
                head[i + k] = i;
            }
        }
        i += run;
    }
    head
}

/// Draws the pattern on the LCD.
fn paint_pattern(
    p: &Painter,
    glass: Rect,
    g: &Grid,
    arp: &ArpSettings,
    on: bool,
    at: Option<usize>,
) {
    let a = if on { 1.0 } else { 0.35 };
    let len = usize::from(arp.length.clamp(1, ARP_STEPS as u8));
    // Lane names.
    for (lane, name) in [
        (g.transpose, tr("Transpose")),
        (g.velocity, tr("Velocity")),
        (g.tie, tr("Tie")),
        (g.chord, tr("Chord")),
    ] {
        let t = gear::fit(p, &name.to_uppercase(), 9.0, LABELS - 16.0);
        gear::lcd_label(
            p,
            pos2(glass.left() + 9.0, lane.center().y),
            Align2::LEFT_CENTER,
            &t,
        );
    }
    // Every other beat of four steps is shaded, and the lanes are ruled off.
    let all = Rect::from_min_max(g.transpose.left_top(), g.chord.right_bottom());
    for group in (0..ARP_STEPS / 4).filter(|k| k % 2 == 1) {
        p.rect_filled(
            Rect::from_min_max(
                pos2(g.x(group * 4), all.top()),
                pos2(g.x(group * 4 + 4), all.bottom()),
            ),
            0.0,
            ink(0.045),
        );
    }
    for y in [
        g.transpose.bottom() + LANE_GAP / 2.0,
        g.velocity.bottom() + LANE_GAP / 2.0,
        g.tie.bottom() + LANE_GAP / 2.0,
    ] {
        p.line_segment(
            [pos2(g.left, y), pos2(all.right(), y)],
            Stroke::new(1.0, ink(0.16)),
        );
    }
    let head = runs(arp);
    for (i, st) in arp.steps.iter().enumerate() {
        // The steps past the pattern's length are kept, and shown faint.
        let a = if i < len { a } else { a * 0.22 };
        // Step numbers on the beats; the step the song is on is shown inverted.
        let playing = at == Some(i);
        if playing {
            p.rect_filled(g.cell(g.numbers, i).shrink2(vec2(1.0, 0.0)), 1.0, ink(a));
        }
        if i % 4 == 0 || playing {
            gear::text_at(
                p,
                pos2(g.x(i) + g.col / 2.0, g.numbers.center().y),
                Align2::CENTER_CENTER,
                &format!("{}", i + 1),
                gear::mono_font(8.5),
                if playing { gear::LCD() } else { ink(0.75 * a) },
                0.0,
            );
        }
        // Transpose: the semitones, or a dot.
        let c = g.cell(g.transpose, i).center();
        if st.transpose == 0 {
            p.circle_filled(c, 1.2, ink(0.3 * a));
        } else {
            gear::text_at(
                p,
                c,
                Align2::CENTER_CENTER,
                &format!("{:+}", st.transpose),
                gear::mono_font(8.5),
                ink(a),
                0.0,
            );
        }
        // Velocity: a bar. A tied note is one bar over its steps; a rest is an outline.
        let lane = g.velocity;
        let owner = arp.steps[head[i]];
        let h = (f32::from(owner.vel) / 127.0 * lane.height()).max(2.0);
        let joins_next = i + 1 < len && head[i + 1] == head[i];
        let bar = Rect::from_min_max(
            pos2(
                g.x(i) + if head[i] != i { 0.0 } else { 2.0 },
                lane.bottom() - h,
            ),
            pos2(
                g.x(i + 1) - if joins_next { 0.0 } else { 2.0 },
                lane.bottom(),
            ),
        );
        if head[i] != i {
            p.rect_filled(bar, 0.0, ink(0.62 * a));
        } else if st.on {
            p.rect_filled(bar, 0.0, ink(a));
        } else {
            p.rect_stroke(
                bar,
                0.0,
                Stroke::new(1.0, ink(0.3 * a)),
                egui::StrokeKind::Inside,
            );
        }
        // Tie: a link from this step into the next.
        let c = g.cell(g.tie, i).center();
        if st.tie {
            let to = pos2((c.x + g.col).min(g.tie.right()), c.y);
            p.line_segment([c, to], Stroke::new(2.0, ink(a)));
            p.circle_filled(c, 3.0, ink(a));
            p.circle_filled(to, 3.0, ink(a));
        } else {
            p.circle_filled(c, 1.2, ink(0.3 * a));
        }
        // Chord: three notes stacked.
        let c = g.cell(g.chord, i).center();
        if st.chord {
            for k in -1..=1 {
                p.rect_filled(
                    Rect::from_center_size(
                        pos2(c.x, c.y + k as f32 * 5.0),
                        vec2((g.col - 8.0).max(6.0), 3.0),
                    ),
                    0.0,
                    ink(a),
                );
            }
        } else {
            p.circle_filled(c, 1.2, ink(0.3 * a));
        }
    }
    // Where the pattern starts again.
    if len < ARP_STEPS {
        let x = g.x(len);
        p.line_segment(
            [pos2(x, g.numbers.top()), pos2(x, all.bottom())],
            Stroke::new(1.5, ink(0.8 * if on { 1.0 } else { 0.35 })),
        );
    }
}

/// The unit's controls in `rect`; `song_beat` is where the song is while it plays. Returns
/// whether a setting changed.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    arp: &mut ArpSettings,
    id: Id,
    on: bool,
    song_beat: Option<f64>,
) -> bool {
    let mut changed = false;
    let p = ui.painter().clone();
    let left = rect.left() + 4.0;
    let right = rect.right() - 4.0;

    // The bar of keys.
    let bar_y = rect.top() + 10.0;
    let presets = Rect::from_min_size(pos2(left, bar_y), vec2(176.0, 28.0));
    let resp = gear::field(ui, presets, id.with("presets"), tr("Presets"), on);
    if on {
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(presets.width());
            for pattern in &PATTERNS {
                if ui.button(pattern.name).clicked() {
                    pattern.apply(arp);
                    changed = true;
                    ui.close();
                }
            }
        });
    }
    let random = Rect::from_min_size(pos2(presets.right() + 10.0, bar_y), vec2(124.0, 28.0));
    if on
        && gear::key(
            ui,
            random,
            id.with("randomize"),
            tr("Randomize"),
            Cap::Bone,
            LedAt::None,
            false,
            false,
        )
        .clicked()
    {
        arp.seed = arp.seed.wrapping_add(1) & 0x00ff_ffff;
        randomize(arp, arp.seed);
        changed = true;
    }
    if !on {
        gear::paint_key(&p, random, Cap::Bone, false);
        gear::legend(
            &p,
            random.center(),
            Align2::CENTER_CENTER,
            tr("Randomize"),
            11.0,
            gear::alpha(gear::INK(), 0.35),
        );
    }
    let latch = Rect::from_min_size(pos2(right - 104.0, bar_y), vec2(104.0, 28.0));
    if on {
        if gear::key(
            ui,
            latch,
            id.with("latch"),
            tr("Latch"),
            Cap::Bone,
            LedAt::Left,
            arp.latch,
            arp.latch,
        )
        .clicked()
        {
            arp.latch = !arp.latch;
            changed = true;
        }
    } else {
        let face = gear::paint_key(&p, latch, Cap::Bone, arp.latch);
        gear::led(
            &p,
            pos2(face.left() + 10.0, face.center().y),
            3.0,
            arp.latch,
        );
        gear::legend(
            &p,
            pos2(face.center().x + 7.0, face.center().y),
            Align2::CENTER_CENTER,
            tr("Latch"),
            11.0,
            gear::alpha(gear::INK(), 0.35),
        );
    }

    // The pattern.
    let lcd = Rect::from_min_max(
        pos2(left, bar_y + 40.0),
        pos2(right, bar_y + 40.0 + Grid::glass_height() + 10.0),
    );
    let glass = gear::lcd(&p, lcd);
    let g = Grid::new(glass);
    let len = usize::from(arp.length.clamp(1, ARP_STEPS as u8));
    if on {
        changed |= edit_pattern(ui, &g, arp, id);
    }
    // Locked to the song, the step the song is on is known.
    let at = song_beat
        .filter(|_| arp.sync && on)
        .map(|beat| ((beat / arp.rate.beats()).floor().max(0.0) as u64 % len as u64) as usize);
    paint_pattern(&p, glass, &g, arp, on, at);

    // The step keys: on or off, an LED each.
    let keys_y = lcd.bottom() + 8.0;
    gear::silk(
        &p,
        pos2(g.left - 10.0, keys_y + 12.0),
        Align2::RIGHT_CENTER,
        "ON",
        10.5,
        gear::PRINT(),
    );
    for i in 0..ARP_STEPS {
        let r = Rect::from_min_max(
            pos2(g.x(i) + 1.5, keys_y),
            pos2(g.x(i + 1) - 1.5, keys_y + 24.0),
        );
        let resp = ui.interact(
            r,
            id.with(("step", i)),
            if on { Sense::click() } else { Sense::hover() },
        );
        let lit = arp.steps[i].on;
        let face = gear::paint_key(&p, r, Cap::Bone, lit || resp.is_pointer_button_down_on());
        gear::led(&p, face.center(), 3.0, lit && on);
        if i >= len {
            p.rect_filled(r.expand(1.0), 3.0, gear::alpha(gear::BONE(), 0.55));
        }
        if on && resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            arp.steps[i].on = !lit;
            changed = true;
        }
    }

    // The knobs.
    let rates: Vec<String> = ArpRate::ALL.iter().map(|r| r.name().to_string()).collect();
    let modes: Vec<String> = ArpMode::ALL
        .iter()
        .map(|m| mode_name(*m).to_string())
        .collect();
    let triggers = vec![tr("Restart").to_string(), tr("Song Position").to_string()];
    let knob = |label: &str, v: u8, min: i32, max: i32, default: i32, text: String| {
        (
            int_control(label, i32::from(v), min, max, default, text, on),
            88.0,
        )
    };
    let cells = [
        (
            choice_control(tr("Rate:"), rates, usize::from(arp.rate.index()), on),
            96.0,
        ),
        (
            choice_control(tr("Mode"), modes, usize::from(arp.mode.index()), on),
            120.0,
        ),
        knob(
            tr("Octaves:"),
            arp.octaves,
            1,
            4,
            1,
            format!("{}", arp.octaves),
        ),
        knob(tr("Gate:"), arp.gate, 5, 200, 80, format!("{} %", arp.gate)),
        knob(
            tr("Swing:"),
            arp.swing,
            0,
            100,
            0,
            format!("{} %", arp.swing),
        ),
        knob(
            tr("Velocity:"),
            arp.step_velocity,
            0,
            100,
            100,
            format!("{} %", arp.step_velocity),
        ),
        knob(
            tr("Steps:"),
            arp.length,
            1,
            32,
            16,
            format!("{}", arp.length),
        ),
        (
            choice_control(tr("Trigger"), triggers, usize::from(arp.sync), on),
            152.0,
        ),
    ];
    let at = pos2(left, keys_y + 24.0 + 16.0);
    for (k, ch) in row(ui, at, id, &cells) {
        match k {
            0 => arp.rate = ArpRate::from_index(ch.choice.unwrap_or(7) as u8),
            1 => arp.mode = ArpMode::from_index(ch.choice.unwrap_or(0) as u8),
            2 => arp.octaves = int_of(&ch, 1, 4) as u8,
            3 => arp.gate = int_of(&ch, 5, 200) as u8,
            4 => arp.swing = int_of(&ch, 0, 100) as u8,
            5 => arp.step_velocity = int_of(&ch, 0, 100) as u8,
            6 => arp.length = int_of(&ch, 1, 32) as u8,
            _ => arp.sync = ch.choice == Some(1),
        }
        changed = true;
    }
    changed
}

/// The pointer on the pattern: draws velocity bars, turns transposes, and switches ties and
/// chords. Returns whether a step changed.
fn edit_pattern(ui: &mut Ui, g: &Grid, arp: &mut ArpSettings, id: Id) -> bool {
    let before = arp.steps;
    // Velocity: press or drag across the bars to draw them. A rest drawn on plays.
    let vel_id = id.with("velocity");
    let resp = ui
        .interact(g.velocity, vel_id, Sense::click_and_drag())
        .on_hover_cursor(CursorIcon::Crosshair);
    let last_id = vel_id.with("last");
    if resp.is_pointer_button_down_on() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let from: Pos2 = ui.data(|d| d.get_temp(last_id)).unwrap_or(pos);
            let (a, b) = (g.step_at(from.x), g.step_at(pos.x));
            for i in a.min(b)..=a.max(b) {
                // The height along the line from where the pointer was to where it is.
                let t = if a == b {
                    1.0
                } else {
                    (i as f32 - a as f32) / (b as f32 - a as f32)
                };
                let y = from.y + (pos.y - from.y) * t;
                let level = ((g.velocity.bottom() - y) / g.velocity.height()).clamp(0.0, 1.0);
                arp.steps[i].vel = (1.0 + level * 126.0).round() as u8;
                arp.steps[i].on = true;
            }
            ui.data_mut(|d| d.insert_temp(last_id, pos));
        }
    } else {
        ui.data_mut(|d| d.remove::<Pos2>(last_id));
    }
    for i in 0..ARP_STEPS {
        // Transpose: drag up or down, a semitone every six points; a double-click is zero.
        let tid = id.with(("transpose", i));
        let resp = ui
            .interact(g.cell(g.transpose, i), tid, Sense::click_and_drag())
            .on_hover_cursor(CursorIcon::ResizeVertical);
        if resp.drag_started() {
            ui.data_mut(|d| d.insert_temp(tid, (f32::from(arp.steps[i].transpose), 0.0f32)));
        }
        if resp.dragged() {
            let (start, moved): (f32, f32) = ui
                .data(|d| d.get_temp(tid))
                .unwrap_or((f32::from(arp.steps[i].transpose), 0.0));
            let moved = moved - resp.drag_delta().y;
            ui.data_mut(|d| d.insert_temp(tid, (start, moved)));
            arp.steps[i].transpose = (start + moved / 6.0).round().clamp(-24.0, 24.0) as i8;
        }
        if resp.double_clicked() {
            arp.steps[i].transpose = 0;
        }
        if ui
            .interact(g.cell(g.tie, i), id.with(("tie", i)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            arp.steps[i].tie = !arp.steps[i].tie;
        }
        if ui
            .interact(g.cell(g.chord, i), id.with(("chord", i)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            arp.steps[i].chord = !arp.steps[i].chord;
        }
    }
    arp.steps != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_test::Harness;
    use egui::{Event, Modifiers, PointerButton};

    /// A harness drawing the unit at the top left, the settings it edits, and the grid it
    /// lays out.
    fn unit() -> (Harness, ArpSettings, Grid, Rect) {
        let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(808.0, HEIGHT));
        let lcd = Rect::from_min_max(
            pos2(rect.left() + 4.0, rect.top() + 50.0),
            pos2(
                rect.right() - 4.0,
                rect.top() + 50.0 + Grid::glass_height() + 10.0,
            ),
        );
        let glass = Rect::from_min_max(lcd.min + vec2(8.0, 5.0), lcd.max - vec2(8.0, 5.0));
        (
            Harness::new(840.0, 480.0),
            ArpSettings::default(),
            Grid::new(glass),
            rect,
        )
    }

    fn frame(h: &mut Harness, events: Vec<Event>, arp: &mut ArpSettings, rect: Rect) -> bool {
        let mut changed = false;
        h.frame(events, |ui| {
            crate::theme::apply(ui.ctx());
            changed = show(ui, rect, arp, Id::unique("arp"), true, None);
        });
        changed
    }

    fn press(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn click(h: &mut Harness, arp: &mut ArpSettings, rect: Rect, pos: Pos2) -> bool {
        frame(h, vec![Event::PointerMoved(pos)], arp, rect);
        let a = frame(h, vec![press(pos, true)], arp, rect);
        let b = frame(h, vec![press(pos, false)], arp, rect);
        let c = frame(h, vec![], arp, rect);
        a || b || c
    }

    #[test]
    fn looking_changes_nothing_and_the_keys_switch_steps_ties_and_chords() {
        let (mut h, mut arp, g, rect) = unit();
        for _ in 0..3 {
            assert!(!frame(&mut h, vec![], &mut arp, rect));
        }
        assert_eq!(arp, ArpSettings::default());
        // A tie mark, a chord mark, and a step key under the LCD.
        assert!(click(&mut h, &mut arp, rect, g.cell(g.tie, 2).center()));
        assert!(arp.steps[2].tie && !arp.steps[1].tie && !arp.steps[3].tie);
        assert!(click(&mut h, &mut arp, rect, g.cell(g.chord, 5).center()));
        assert!(arp.steps[5].chord);
        let key = pos2(g.x(7) + g.col / 2.0, g.chord.bottom() + 12.0 + 8.0 + 12.0);
        assert!(click(&mut h, &mut arp, rect, key));
        assert!(!arp.steps[7].on && arp.steps[6].on && arp.steps[8].on);
        assert!(click(&mut h, &mut arp, rect, key));
        assert!(arp.steps[7].on);
        assert!(click(&mut h, &mut arp, rect, g.cell(g.tie, 2).center()));
        assert!(!arp.steps[2].tie);
    }

    #[test]
    fn dragging_draws_velocity_bars_and_turns_a_transpose() {
        let (mut h, mut arp, g, rect) = unit();
        arp.steps[4].on = false;
        frame(&mut h, vec![], &mut arp, rect);
        // From the bottom of step 2 to the top of step 6: a ramp, and the rest plays.
        let from = pos2(g.x(2) + g.col / 2.0, g.velocity.bottom() - 1.0);
        let to = pos2(g.x(6) + g.col / 2.0, g.velocity.top() + 1.0);
        frame(&mut h, vec![Event::PointerMoved(from)], &mut arp, rect);
        frame(&mut h, vec![press(from, true)], &mut arp, rect);
        frame(&mut h, vec![Event::PointerMoved(to)], &mut arp, rect);
        frame(&mut h, vec![press(to, false)], &mut arp, rect);
        frame(&mut h, vec![], &mut arp, rect);
        let v: Vec<u8> = arp.steps[1..8].iter().map(|s| s.vel).collect();
        assert_eq!((v[0], v[6]), (100, 100), "the steps outside are untouched");
        assert!(v[1] < 10 && v[5] > 120, "{v:?}");
        assert!(v[1..6].windows(2).all(|w| w[0] < w[1]), "{v:?}");
        assert!(arp.steps[4].on);
        // A transpose turned up by 30 points is five semitones; a double-click is zero.
        let cell = g.cell(g.transpose, 9).center();
        let up = cell - vec2(0.0, 30.0);
        frame(&mut h, vec![Event::PointerMoved(cell)], &mut arp, rect);
        frame(&mut h, vec![press(cell, true)], &mut arp, rect);
        frame(&mut h, vec![Event::PointerMoved(up)], &mut arp, rect);
        frame(&mut h, vec![press(up, false)], &mut arp, rect);
        frame(&mut h, vec![], &mut arp, rect);
        assert_eq!(arp.steps[9].transpose, 5);
        assert_eq!(arp.steps[8].transpose, 0);
    }

    #[test]
    fn tied_steps_belong_to_the_note_they_are_tied_from() {
        let mut arp = ArpSettings {
            length: 8,
            ..ArpSettings::default()
        };
        arp.steps[1].tie = true;
        arp.steps[2].tie = true;
        arp.steps[5].on = false;
        arp.steps[5].tie = true;
        arp.steps[7].tie = true;
        let head = runs(&arp);
        assert_eq!(head[..8], [0, 1, 1, 1, 4, 5, 6, 7]);
        // Steps past the length are their own.
        assert_eq!(head[8..12], [8, 9, 10, 11]);
        for m in ArpMode::ALL {
            assert!(!mode_name(m).is_empty());
        }
    }
}
