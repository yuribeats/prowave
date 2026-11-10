//! Logic's List Editors (View > Show List Editors, D): the Event list on the right. It lists
//! the MIDI events of the selected region (or of the selected track) with their position,
//! note, velocity and length, and a line marks where the playhead is. While the song plays the
//! line moves down the list at the speed of the notes (fast passages go quickly, slow ones
//! slowly) and the list pages along with it.

use crate::hw::{self, Cap, Legend};
use crate::i18n::tr;
use crate::model::{Project, TrackKind, CHANNEL_PRESSURE, PITCH_BEND, SYSEX_BYTE};
use crate::piano_roll::caption;
use crate::theme::{self, Face};
use crate::UiState;
use egui::{pos2, vec2, Align2, Rect, Sense, Stroke, Ui};

pub const WIDTH: f32 = 400.0;
const ROW_H: f32 = 18.0;
const TICKS_PER_BEAT: i64 = 960;
const TICKS_PER_DIVISION: i64 = 240;
/// The title row (a menu row with keys) and the column titles.
const BAR_H: f32 = 46.0;
const COLS_H: f32 = 24.0;
/// The columns: right edges of the position, channel, number and value (right-aligned
/// numbers), and the left edges of the status and the length or info, from the list's left.
const COL_POS: f32 = 104.0;
const COL_STATUS: f32 = 114.0;
const COL_CH: f32 = 196.0;
const COL_NUM: f32 = 240.0;
const COL_VAL: f32 = 276.0;
const COL_LEN: f32 = 290.0;

/// The list's own state.
pub struct ListView {
    /// Pixels the list is scrolled down.
    pub scroll: f32,
    /// Logic's Catch Playhead: the list follows the playhead while the song plays.
    pub follow: bool,
}

impl Default for ListView {
    fn default() -> Self {
        Self {
            scroll: 0.0,
            follow: true,
        }
    }
}

/// One note of the list.
struct Event {
    /// Position in beats from the start of the song.
    beats: f64,
    /// Belongs to another region of the track: shown grey, as context.
    dim: bool,
    /// A controller event (the sustain pedal) rather than a note.
    control: bool,
    /// The note's pitch, or the controller's number.
    pitch: u8,
    /// The note's velocity.
    vel: u8,
    /// A controller's value in full (pitch bend is 14 bits); for a System Exclusive message its
    /// number of bytes.
    value: u16,
    /// A System Exclusive message as hex words; empty for the other events.
    text: String,
    len: f64,
    /// The note's index in its region (notes of the region edited only).
    note: Option<usize>,
}

/// The events to show: those of the selected MIDI region, with the other regions of its track
/// in grey around them (Logic shows them as context, and while a take is recorded the notes
/// of the earlier ones stay in view). With no region selected: all of the selected track's.
fn events(p: &Project, s: &UiState) -> Vec<Event> {
    let (track, selected) = match s.tracks.selected {
        Some((t, r))
            if p.tracks
                .get(t)
                .is_some_and(|tr| tr.kind != TrackKind::Audio && r < tr.regions.len()) =>
        {
            (t, Some(r))
        }
        _ => (p.selected_track, None),
    };
    let Some(track) = p.tracks.get(track).filter(|t| t.kind != TrackKind::Audio) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (ri, r) in track.regions.iter().enumerate() {
        let offset = p.region_start_beats(r);
        let dim = selected.is_some_and(|sr| sr != ri);
        // The bytes of a System Exclusive message are consecutive controls at one start: they are
        // one row.
        let mut i = 0;
        while i < r.controls.len() {
            let c = r.controls[i];
            if c.num == SYSEX_BYTE {
                let mut bytes = Vec::new();
                while i < r.controls.len()
                    && r.controls[i].num == SYSEX_BYTE
                    && r.controls[i].start == c.start
                {
                    bytes.push(r.controls[i].val.min(255) as u8);
                    i += 1;
                }
                out.push(Event {
                    beats: offset + c.start as f64,
                    dim,
                    control: true,
                    pitch: SYSEX_BYTE,
                    vel: 0,
                    value: bytes.len() as u16,
                    text: crate::sysex::text(&bytes),
                    len: 0.0,
                    note: None,
                });
                continue;
            }
            out.push(Event {
                beats: offset + c.start as f64,
                dim,
                control: true,
                pitch: c.num,
                vel: 0,
                value: c.val,
                text: String::new(),
                len: 0.0,
                note: None,
            });
            i += 1;
        }
        for (ni, n) in r.notes.iter().enumerate() {
            out.push(Event {
                beats: offset + n.start as f64,
                dim,
                control: false,
                pitch: n.pitch,
                vel: n.vel,
                value: 0,
                text: String::new(),
                len: n.len as f64,
                note: (!dim).then_some(ni),
            });
        }
    }
    // A controller at the same place as a note comes first, as in Logic.
    out.sort_by(|a, b| {
        a.beats
            .total_cmp(&b.beats)
            .then(b.control.cmp(&a.control))
            .then(a.pitch.cmp(&b.pitch))
    });
    out
}

/// Logic's position "bar beat division" and the ticks, from a position in beats.
fn position_parts(beats: f64, bpb: f64) -> ([i64; 3], i64) {
    let total = (beats * TICKS_PER_BEAT as f64).round().max(0.0) as i64;
    let per_bar = ((bpb * TICKS_PER_BEAT as f64).round() as i64).max(1);
    let rem = total % per_bar;
    let in_beat = rem % TICKS_PER_BEAT;
    (
        [
            total / per_bar + 1,
            rem / TICKS_PER_BEAT + 1,
            in_beat / TICKS_PER_DIVISION + 1,
        ],
        in_beat % TICKS_PER_DIVISION + 1,
    )
}

/// [`position_parts`] for a position in the song: bars have the length of their signature.
fn position_in(p: &Project, beats: f64) -> ([i64; 3], i64) {
    let bar0 = p.beats_to_bar(beats).floor();
    let (mut parts, tick) = position_parts(beats - p.bar_to_beats(bar0), p.bar_len(bar0 as u32));
    parts[0] += bar0 as i64;
    (parts, tick)
}

/// A length as Logic shows it: bars, beats, divisions and ticks, all counted from zero.
fn length_parts(len: f64, bpb: f64) -> [i64; 4] {
    let total = (len * TICKS_PER_BEAT as f64).round().max(0.0) as i64;
    let per_bar = ((bpb * TICKS_PER_BEAT as f64).round() as i64).max(1);
    let rem = total % per_bar;
    let in_beat = rem % TICKS_PER_BEAT;
    [
        total / per_bar,
        rem / TICKS_PER_BEAT,
        in_beat / TICKS_PER_DIVISION,
        in_beat % TICKS_PER_DIVISION,
    ]
}

/// Logic names the octaves so that pitch 60 is C3.
fn note_name(pitch: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[(pitch % 12) as usize],
        (pitch / 12) as i32 - 2
    )
}

/// What the Length/Info column says for a controller.
fn controller_name(num: u8) -> String {
    match num {
        crate::recorder::SUSTAIN => tr("Sustain").to_string(),
        n => format!("{} {n}", tr("Controller")),
    }
}

/// The type of a controller-kind event: a controller, pitch bend, channel pressure or SysEx.
fn control_type(num: u8) -> String {
    match num {
        PITCH_BEND => tr("Pitch Bend").to_string(),
        CHANNEL_PRESSURE => tr("Aftertouch").to_string(),
        SYSEX_BYTE => tr("Syst. Exclusive").to_string(),
        _ => tr("Control").to_string(),
    }
}

/// Index of the first event the playhead has not reached yet: the line goes above it.
fn playhead_row(events: &[Event], position: f64) -> usize {
    events.partition_point(|e| e.beats <= position + 1e-9)
}

/// The scroll that follows the playhead: it stays where it is while the line is in view and
/// pages along (line near the top) when the line reaches the bottom or leaves at the top.
fn followed_scroll(scroll: f32, line_y: f32, view_h: f32, max_scroll: f32) -> f32 {
    let rel = line_y - scroll;
    let new = if rel > view_h * 0.85 || rel < 0.0 {
        line_y - view_h * 0.15
    } else {
        scroll
    };
    new.clamp(0.0, max_scroll.max(0.0))
}

/// The window that adds a System Exclusive message (typed in hex) to the region at the playhead.
fn sysex_window(ctx: &egui::Context, p: &mut Project, s: &mut UiState) {
    let Some(mut text) = s.sysex_entry.take() else {
        return;
    };
    let target = crate::sysex::target(p, s.tracks.selected);
    let mut open = true;
    let mut done = false;
    egui::Window::new(tr("Add SysEx"))
        .id(egui::Id::unique("sysex_entry"))
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.label(tr("Bytes in hex, from F0 to F7"));
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .desired_rows(3)
                    .desired_width(320.0),
            );
            let parsed = crate::sysex::parse(&text);
            if parsed.is_err() && !text.trim().is_empty() {
                ui.colored_label(
                    theme::DANGER,
                    tr("The message must start with F0 and end with F7, in hex"),
                );
            }
            if target.is_none() {
                ui.label(tr("Select a MIDI region to add the message to"));
            }
            ui.horizontal(|ui| {
                let can = parsed.is_ok() && target.is_some();
                if ui
                    .add_enabled(can, egui::Button::new(tr("Insert")))
                    .clicked()
                {
                    if let (Ok(bytes), Some((t, r))) = (&parsed, target) {
                        if crate::sysex::insert(p, t, r, bytes) {
                            s.status = Some(crate::i18n::trf(
                                "Added a SysEx message of {} bytes",
                                &[&bytes.len()],
                            ));
                        }
                    }
                    done = true;
                }
                if ui.button(tr("Cancel")).clicked() {
                    done = true;
                }
            });
        });
    if open && !done {
        s.sysex_entry = Some(text);
    }
}

pub fn show(ui: &mut Ui, p: &mut Project, s: &mut UiState) {
    sysex_window(ui.ctx(), p, s);
    let area = ui.max_rect();
    let g = ui.painter_at(area);
    hw::chassis(&g, area);
    let events = events(p, s);

    // Title bar: "EVENT", the playhead's position on a small LCD, SysEx and Catch Playhead
    // (the Marker, Tempo and Signature lists, the local menus, the event-type filters and the
    // list's Quantize are not built).
    let mut y = area.top();
    let bar = Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), BAR_H));
    crate::piano_roll::menu_row(&g, bar);
    let cy = bar.center().y - 1.0;
    let title = hw::text(
        &g,
        pos2(bar.left() + 14.0, cy),
        Align2::LEFT_CENTER,
        &tr("Event").to_uppercase(),
        Face::SilkX,
        14.0,
        theme::INK,
    );
    // Catch: a toggle key with the playhead-chasing icon.
    let catch = Rect::from_min_size(pos2(bar.right() - 54.0, cy - 14.0), vec2(44.0, 28.0));
    let resp = crate::piano_roll::icon_key(
        ui,
        &g,
        catch,
        egui::Id::unique(("event_list", "catch")),
        s.lists.follow,
        false,
        crate::piano_roll::draw_catch,
    );
    if resp.on_hover_text(tr("Catch Playhead")).clicked() {
        s.lists.follow = !s.lists.follow;
    }
    // Add a System Exclusive message at the playhead (see `sysex.rs`).
    // The key's legend without the dots the menu entry has.
    let label = tr("SysEx…").trim_end_matches(['…', '.']);
    let w = hw::galley(&g, &label.to_uppercase(), Face::Bold, 12.0, theme::INK)
        .size()
        .x
        + 26.0;
    let sysex_btn = Rect::from_min_size(pos2(catch.left() - 8.0 - w, cy - 14.0), vec2(w, 28.0));
    let (sysex_resp, _) = hw::key(
        ui,
        &g,
        sysex_btn,
        egui::Id::unique(("event_list", "sysex")),
        Cap::Bone,
        false,
        None,
        Legend::Text(label),
    );
    if sysex_resp.clicked() {
        s.sysex_entry.get_or_insert_with(String::new);
    }
    // Where the playhead is, in bars and beats, on LCD glass.
    let chip = Rect::from_min_max(
        pos2(title.right() + 14.0, cy - 15.0),
        pos2(sysex_btn.left() - 10.0, cy + 15.0),
    );
    if chip.width() >= 90.0 {
        let ([bar_n, beat, div], tick) = position_in(p, p.position_beats);
        hw::lcd_readout(
            &g,
            chip,
            caption(tr("Position")),
            &format!("{bar_n} {beat} {div} {tick:>3}"),
        );
    }
    y += BAR_H;

    // Column titles, silkscreened on a strip with a seam under it; the numbers' titles are
    // right-aligned over their numbers.
    let cols = Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), COLS_H));
    hw::seam_h(&g, cols.x_range(), cols.bottom() - 2.0);
    let lx = area.left() + 8.0;
    let head = |x: f32, align: Align2, text: &str, room: f32| {
        let shown = hw::fit(&g, &text.to_uppercase(), Face::Silk, 10.0, room);
        hw::text(
            &g,
            pos2(lx + x, cols.center().y - 1.0),
            align,
            &shown,
            Face::Silk,
            10.0,
            theme::INK2,
        );
    };
    head(COL_POS, Align2::RIGHT_CENTER, tr("Position"), 90.0);
    head(COL_STATUS, Align2::LEFT_CENTER, tr("Status"), 70.0);
    head(COL_CH, Align2::RIGHT_CENTER, tr("Ch"), 30.0);
    head(COL_NUM, Align2::RIGHT_CENTER, tr("Num"), 40.0);
    head(COL_VAL, Align2::RIGHT_CENTER, tr("Val"), 34.0);
    head(
        COL_LEN,
        Align2::LEFT_CENTER,
        tr("Length/Info"),
        area.right() - (lx + COL_LEN) - 8.0,
    );
    y += COLS_H;

    // The rows, bone print on the dark field material, in whole rows.
    let well = Rect::from_min_max(pos2(area.left() + 4.0, y + 2.0), area.max - vec2(4.0, 4.0));
    let rows_fit = ((well.height() - 6.0) / ROW_H).floor().max(0.0);
    let well = Rect::from_min_size(well.min, vec2(well.width(), rows_fit * ROW_H + 6.0));
    g.rect_filled(well.translate(vec2(0.0, 1.0)), 3.0, theme::BONE_HI);
    hw::gradient(
        &g,
        well,
        3.0,
        egui::Color32::from_rgb(0x25, 0x23, 0x21),
        egui::Color32::from_rgb(0x15, 0x14, 0x13),
    );
    hw::inner_shadow(&g, well, 3.0, 0.5);
    g.rect_stroke(
        well,
        3.0,
        Stroke::new(1.0, theme::CAP_EDGE),
        egui::StrokeKind::Inside,
    );
    let list = well.shrink2(vec2(0.0, 3.0));
    let content_h = events.len() as f32 * ROW_H;
    let max_scroll = (content_h - list.height() + ROW_H).max(0.0);
    let resp = ui.interact(
        list,
        egui::Id::unique(("event_list", "rows")),
        Sense::hover(),
    );
    if resp.hovered() {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        s.lists.scroll = (s.lists.scroll - dy).clamp(0.0, max_scroll);
    }
    let line_row = playhead_row(&events, p.position_beats);
    let line_y = line_row as f32 * ROW_H;
    if s.lists.follow && p.playing {
        s.lists.scroll = followed_scroll(s.lists.scroll, line_y, list.height(), max_scroll);
    }
    s.lists.scroll = s.lists.scroll.clamp(0.0, max_scroll);
    // Scrolled by whole rows, so no row is cut.
    let scroll = (s.lists.scroll / ROW_H).round() * ROW_H;

    let gl = ui.painter_at(list);
    let first = (scroll / ROW_H).floor() as usize;
    let last = (((scroll + list.height()) / ROW_H).floor() as usize).min(events.len());
    let cx = |v: f32| lx + v;
    let size = 10.5;
    for (i, e) in events.iter().enumerate().take(last).skip(first) {
        let row = Rect::from_min_size(
            pos2(list.left(), list.top() + i as f32 * ROW_H - scroll),
            vec2(list.width(), ROW_H),
        );
        let cy = row.center().y;
        let selected = e.note.is_some_and(|n| s.piano.selected.contains(&n));
        if selected {
            gl.rect_filled(row, 0.0, theme::ORANGE.gamma_multiply(0.25));
            gl.rect_filled(
                Rect::from_min_size(row.min, vec2(4.0, ROW_H)),
                0.0,
                theme::ORANGE,
            );
        } else if i % 2 == 1 {
            gl.rect_filled(row, 0.0, theme::ON_DARK.gamma_multiply(0.035));
        }
        let ([bar, beat, div], tick) = position_in(p, e.beats);
        let l = length_parts(e.len, p.bar_len_at(e.beats));
        // The other regions' events print faintly.
        let color = if e.dim {
            theme::ON_DARK_DIM.gamma_multiply(0.7)
        } else {
            theme::ON_DARK
        };
        let at = |x: f32, align: Align2, text: &str, room: f32| {
            let shown = hw::fit(&gl, &text.to_uppercase(), Face::Bold, size, room);
            hw::text(&gl, pos2(cx(x), cy), align, &shown, Face::Bold, size, color);
        };
        at(
            COL_POS,
            Align2::RIGHT_CENTER,
            &format!("{bar:>3} {beat} {div} {tick:>3}"),
            100.0,
        );
        at(COL_CH, Align2::RIGHT_CENTER, "1", 24.0);
        let value = if e.control { e.value } else { u16::from(e.vel) };
        at(COL_VAL, Align2::RIGHT_CENTER, &format!("{value}"), 34.0);
        let info_room = list.right() - cx(COL_LEN) - 6.0;
        if e.control {
            at(
                COL_STATUS,
                Align2::LEFT_CENTER,
                &control_type(e.pitch),
                COL_CH - COL_STATUS - 26.0,
            );
            match e.pitch {
                SYSEX_BYTE => {
                    // The message's bytes, as many as the Info column shows.
                    at(COL_LEN, Align2::LEFT_CENTER, &e.text, info_room);
                }
                PITCH_BEND | CHANNEL_PRESSURE => {}
                num => {
                    at(COL_NUM, Align2::RIGHT_CENTER, &format!("{num}"), 40.0);
                    at(
                        COL_LEN,
                        Align2::LEFT_CENTER,
                        &controller_name(num),
                        info_room,
                    );
                }
            }
        } else {
            at(
                COL_STATUS,
                Align2::LEFT_CENTER,
                tr("Note"),
                COL_CH - COL_STATUS - 26.0,
            );
            at(COL_NUM, Align2::RIGHT_CENTER, &note_name(e.pitch), 40.0);
            at(
                COL_LEN,
                Align2::LEFT_CENTER,
                &format!("{} {} {} {:>3}", l[0], l[1], l[2], l[3]),
                info_room,
            );
        }
    }
    // The playhead line, between the notes that have sounded and those still to come.
    let ly = list.top() + line_y - scroll;
    if ly >= list.top() && ly <= list.bottom() && !events.is_empty() {
        gl.hline(list.x_range(), ly, Stroke::new(2.0, theme::ORANGE));
    }
    // The seam towards the tracks.
    hw::seam_v(&g, area.left(), area.y_range());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_count_bars_beats_divisions_and_ticks_from_one() {
        assert_eq!(position_parts(0.0, 4.0), ([1, 1, 1], 1));
        // Bar 29, beat 2, division 3, 10 ticks in: Logic shows "29 2 3 11".
        let beats = 28.0 * 4.0 + 1.0 + 0.5 + 10.0 / 960.0;
        assert_eq!(position_parts(beats, 4.0), ([29, 2, 3], 11));
    }

    #[test]
    fn lengths_count_from_zero() {
        assert_eq!(length_parts(0.0, 4.0), [0, 0, 0, 0]);
        // Two divisions and 214 ticks, as in "0 0 2 214".
        let len = (2.0 * 240.0 + 214.0) / 960.0;
        assert_eq!(length_parts(len, 4.0), [0, 0, 2, 214]);
        assert_eq!(length_parts(4.0 + 1.0, 4.0), [1, 1, 0, 0]);
    }

    #[test]
    fn notes_are_named_like_logic() {
        assert_eq!(note_name(60), "C3");
        assert_eq!(note_name(63), "D#3");
        assert_eq!(note_name(0), "C-2");
    }

    #[test]
    fn the_line_sits_between_played_and_coming_notes() {
        let ev = |beats| Event {
            beats,
            dim: false,
            control: false,
            value: 0,
            text: String::new(),
            pitch: 60,
            vel: 100,
            len: 1.0,
            note: None,
        };
        let events = [ev(0.0), ev(1.0), ev(1.0), ev(2.0)];
        assert_eq!(playhead_row(&events, -0.5), 0);
        assert_eq!(playhead_row(&events, 0.0), 1);
        assert_eq!(playhead_row(&events, 1.0), 3);
        assert_eq!(playhead_row(&events, 9.0), 4);
    }

    #[test]
    fn the_list_pages_along_with_the_line() {
        // In view: stays.
        assert_eq!(followed_scroll(100.0, 200.0, 400.0, 1000.0), 100.0);
        // Past the bottom: the line goes back near the top.
        assert_eq!(followed_scroll(100.0, 480.0, 400.0, 1000.0), 420.0);
        // Above the view (after a loop): follows up.
        assert!((followed_scroll(500.0, 100.0, 400.0, 1000.0) - 40.0).abs() < 1e-3);
        // Never past the end.
        assert_eq!(followed_scroll(0.0, 2000.0, 400.0, 600.0), 600.0);
    }

    #[test]
    fn faster_notes_move_the_line_faster() {
        let ev = |beats| Event {
            beats,
            dim: false,
            control: false,
            value: 0,
            text: String::new(),
            pitch: 60,
            vel: 100,
            len: 0.25,
            note: None,
        };
        let fast: Vec<Event> = (0..32).map(|i| ev(i as f64 * 0.25)).collect();
        let slow: Vec<Event> = (0..32).map(|i| ev(i as f64 * 1.0)).collect();
        assert!(playhead_row(&fast, 2.0) > playhead_row(&slow, 2.0));
    }

    #[test]
    fn a_sysex_message_is_one_row_and_a_bend_keeps_its_14_bits() {
        use crate::model::{Control, CHANNEL_PRESSURE, PITCH_BEND, SYSEX_BYTE};
        let mut p = Project::demo();
        let controls = &mut p.tracks[0].regions[0].controls;
        controls.clear();
        controls.push(Control {
            start: 1.0,
            num: PITCH_BEND,
            val: 12345,
        });
        controls.push(Control {
            start: 2.0,
            num: CHANNEL_PRESSURE,
            val: 90,
        });
        for b in [0xf0u16, 0x41, 0xf7] {
            controls.push(Control {
                start: 3.0,
                num: SYSEX_BYTE,
                val: b,
            });
        }
        let mut s = UiState::new();
        s.tracks.selected = Some((0, 0));
        let ev = events(&p, &s);
        let sysex: Vec<&Event> = ev.iter().filter(|e| e.pitch == SYSEX_BYTE).collect();
        assert_eq!(sysex.len(), 1, "three bytes, one row");
        assert_eq!(sysex[0].text, "F0 41 F7");
        assert_eq!(sysex[0].value, 3, "its number of bytes");
        let bend = ev.iter().find(|e| e.pitch == PITCH_BEND).expect("the bend");
        assert_eq!(bend.value, 12345, "all 14 bits");
        let pressure = ev
            .iter()
            .find(|e| e.pitch == CHANNEL_PRESSURE)
            .expect("the pressure");
        assert_eq!(pressure.value, 90);
    }
}
