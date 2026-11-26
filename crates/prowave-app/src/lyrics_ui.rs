//! Typing lyrics in the Piano Roll, as in Synthesizer V and UTAU: select a note and press
//! Return to type its lyric; Tab or Space keeps it and goes on to the next note (Shift-Tab to
//! the one before), Return keeps it and stops, Escape leaves the note as it was. Typing several
//! words spreads them over the notes from this one on. Edit > Paste Lyrics… spreads a whole
//! line over the selected notes (or all of them). See `lyrics.rs` for `+` and `-`.
use crate::hw;
use crate::i18n::tr;
use crate::lyrics::{self, Lyric};
use crate::model::Note;
use crate::spectrogram_view::hw_vocal;
use crate::theme::{self, Face};
use egui::{Key, Modifiers, Rect, Ui};

/// The lyric being typed and the Paste Lyrics window.
#[derive(Default)]
pub struct LyricEditor {
    /// The note whose lyric is typed (an index into the region's notes) and the text so far.
    editing: Option<(usize, String)>,
    /// The Paste Lyrics window is open, with its text.
    paste: Option<String>,
    /// The window has just opened: its sheet takes the keyboard.
    paste_focus: bool,
}

impl LyricEditor {
    /// Edit > Paste Lyrics…
    pub fn open_paste(&mut self) {
        if self.paste.is_none() {
            self.paste_focus = true;
        }
        self.paste.get_or_insert_with(String::new);
    }

    /// Starts typing the lyric of note `i`.
    pub fn start(&mut self, notes: &[Note], i: usize) {
        if let Some(n) = notes.get(i) {
            self.editing = Some((i, n.lyric.as_str().to_string()));
        }
    }

    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }
}

/// What a key does to the lyric being typed.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    Next,
    Previous,
    Done,
    Cancel,
}

/// Keeps `text` as the lyric of note `i`: one word, or several spread from there on.
fn commit(notes: &mut [Note], i: usize, text: &str, selected: &[usize]) {
    if text.split_whitespace().count() > 1 {
        let only = if selected.len() > 1 { selected } else { &[] };
        lyrics::spread(notes, text, Some(i), only);
    } else if let Some(n) = notes.get_mut(i) {
        n.lyric = Lyric::new(text);
    }
}

/// The note sung after (or before) `i`.
fn neighbour(notes: &[Note], i: usize, forward: bool) -> Option<usize> {
    let order = lyrics::sung_order(notes);
    let at = order.iter().position(|x| *x == i)?;
    if forward {
        order.get(at + 1).copied()
    } else {
        at.checked_sub(1).map(|k| order[k])
    }
}

/// Applies one step to the editor; returns whether a lyric changed.
fn apply(ed: &mut LyricEditor, notes: &mut [Note], selected: &[usize], step: Step) -> bool {
    let Some((i, text)) = ed.editing.take() else {
        return false;
    };
    if step == Step::Cancel {
        return false;
    }
    let before = notes.get(i).map(|n| n.lyric);
    commit(notes, i, &text, selected);
    let changed = notes.get(i).map(|n| n.lyric) != before || text.split_whitespace().count() > 1;
    let next = match step {
        Step::Next => neighbour(notes, i, true),
        Step::Previous => neighbour(notes, i, false),
        _ => None,
    };
    if let Some(j) = next {
        ed.start(notes, j);
    }
    changed
}

/// The Paste Lyrics panel: how the words are spread, the lyric sheet (typed on LCD glass), how
/// many notes it fills, and Apply and Cancel.
fn paste_panel(
    ui: &mut Ui,
    text: &mut String,
    selected: &[usize],
    focus: &mut bool,
    applied: &mut Option<String>,
    close: &mut bool,
) {
    use egui::{pos2, vec2};
    // How the words are spread, as legend rows printed on the panel (one per rule).
    rules(
        ui,
        tr(
            "One word per note, in order. Hyphens split a word into syllables (gent-ly); a - holds the vowel before, a + sings the next syllable.",
        ),
    );
    hw_vocal::gap(ui, 10.0);
    // The sheet: LCD glass in its bezel, four lines of words typed on it; it takes the
    // keyboard when the panel opens, ringed in orange while it has it.
    let (sheet, _) = ui.allocate_exact_size(vec2(ui.available_width(), 92.0), egui::Sense::hover());
    let g = ui.painter().clone();
    let glass = hw::lcd(&g, sheet);
    let inner = glass.shrink2(vec2(10.0, 7.0));
    let sheet_id = egui::Id::unique("prowave_paste_lyrics_sheet");
    // Asked until it has it (a new window's first frame only sizes it).
    if *focus {
        ui.ctx().memory_mut(|m| m.request_focus(sheet_id));
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    let edit = egui::ScrollArea::vertical()
        .max_height(inner.height())
        .show(&mut child, |ui| {
            ui.add(
                egui::TextEdit::multiline(text)
                    .id(sheet_id)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .font(theme::font(Face::Bold, 12.5))
                    .text_color(theme::LCD_INK())
                    .desired_rows(4)
                    .desired_width(inner.width() - 12.0),
            )
        })
        .inner;
    if edit.has_focus() {
        *focus = false;
        hw_vocal::focus_ring(&g, glass);
    }
    hw_vocal::gap(ui, 12.0);
    // How many notes the words fill, then the keys.
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), egui::Sense::hover());
    let g = ui.painter().clone();
    let n = lyrics::tokens(text).len();
    let chip = egui::Rect::from_min_size(row.min, vec2(118.0, 34.0));
    hw::lcd_glass(&g, chip);
    let count = n.to_string();
    let h = 17.0;
    hw::seg(
        &g,
        pos2(chip.left() + 10.0, chip.center().y - h / 2.0),
        &count,
        h,
        theme::LCD_INK(),
        false,
    );
    let words = crate::i18n::trf("{} notes", &[&n]);
    let unit = words.replace(&count, "");
    hw_vocal::lcd_text(
        &g,
        egui::Rect::from_min_max(
            pos2(chip.left() + 16.0 + hw::seg_width(&count, h), chip.top()),
            chip.max - vec2(6.0, 0.0),
        ),
        unit.trim(),
        10.0,
        1,
        theme::LCD_INK().gamma_multiply(0.8),
    );
    let label = if selected.len() > 1 {
        tr("Apply to Selected Notes")
    } else {
        tr("Apply to All Notes")
    };
    let cancel_w = hw::galley(
        &g,
        &tr("Cancel").to_uppercase(),
        Face::Bold,
        11.0,
        theme::INK(),
    )
    .size()
    .x + 32.0;
    let cancel = egui::Rect::from_min_size(
        pos2(row.right() - cancel_w, row.top()),
        vec2(cancel_w, 34.0),
    );
    let apply = egui::Rect::from_min_max(
        pos2(chip.right() + 12.0, row.top()),
        pos2(cancel.left() - 8.0, row.top() + 34.0),
    );
    let id = egui::Id::unique("prowave_paste_lyrics_keys");
    if hw_vocal::key(ui, apply, id.with(0), hw::Cap::Dark, label, None, true).clicked() {
        *applied = Some(text.clone());
    }
    if hw_vocal::key(
        ui,
        cancel,
        id.with(1),
        hw::Cap::Bone,
        tr("Cancel"),
        None,
        true,
    )
    .clicked()
    {
        *close = true;
    }
}

/// `text`'s rules (its sentences and clauses) as rows of small silkscreen print.
fn rules(ui: &mut Ui, text: &str) {
    let width = ui.available_width();
    let size = 10.5;
    let mut parts: Vec<String> = Vec::new();
    let mut part = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let ends =
            matches!(c, '.' | ';' | '。' | '；') && chars.peek().is_none_or(|n| n.is_whitespace());
        if ends {
            parts.push(std::mem::take(&mut part));
        } else {
            part.push(c);
        }
    }
    parts.push(part);
    for p in parts.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        let g = ui.painter().clone();
        let mut job = egui::text::LayoutJob::single_section(
            p.to_uppercase(),
            egui::TextFormat {
                font_id: theme::font(Face::Silk, size),
                color: theme::INK2(),
                extra_letter_spacing: theme::tracking(Face::Silk) * size,
                ..Default::default()
            },
        );
        job.wrap.max_width = width - 14.0;
        let gal = g.layout_job(job);
        let (r, _) =
            ui.allocate_exact_size(egui::vec2(width, gal.size().y + 4.0), egui::Sense::hover());
        // A small printed tick before each rule.
        let tick_y = r.top() + 2.0 + size * 0.62;
        g.hline(
            r.left()..=r.left() + 6.0,
            tick_y,
            egui::Stroke::new(1.0, theme::INK2().gamma_multiply(0.6)),
        );
        g.galley(
            egui::pos2(r.left() + 14.0, r.top() + 2.0),
            gal,
            theme::INK2(),
        );
    }
}

/// The lyric field over the note being typed, the Return key that starts it, and the Paste
/// Lyrics window. `keys` is whether the editor has the keyboard (pointer over it, no menu
/// open). Returns whether any lyric changed.
pub fn show(
    ui: &mut Ui,
    ed: &mut LyricEditor,
    notes: &mut [Note],
    selected: &[usize],
    grid: Rect,
    keys: bool,
    rect_of: &dyn Fn(&Note) -> Rect,
) -> bool {
    let mut changed = false;
    if ed.editing.is_none() && keys && ui.input(|i| i.key_pressed(Key::Enter)) {
        let order = lyrics::sung_order(notes);
        if let Some(first) = order.into_iter().find(|i| selected.contains(i)) {
            ed.start(notes, first);
        }
    }
    if let Some((i, _)) = &ed.editing {
        if *i >= notes.len() {
            ed.editing = None;
        }
    }
    if ed.editing.is_some() {
        // The keys that move on are taken before the field sees them.
        let step = ui.input_mut(|i| {
            let step = if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                Some(Step::Previous)
            } else if i.consume_key(Modifiers::NONE, Key::Tab)
                || i.consume_key(Modifiers::NONE, Key::Space)
            {
                Some(Step::Next)
            } else if i.consume_key(Modifiers::NONE, Key::Enter) {
                Some(Step::Done)
            } else if i.consume_key(Modifiers::NONE, Key::Escape) {
                Some(Step::Cancel)
            } else {
                None
            };
            if step.is_some() {
                i.events
                    .retain(|e| !matches!(e, egui::Event::Text(t) if t == " " || t == "\t"));
            }
            step
        });
        if let Some(step) = step {
            changed |= apply(ed, notes, selected, step);
        }
    }
    if let Some((i, text)) = ed.editing.as_mut() {
        // The field takes the note's own place (only as wide as a short word needs, at least),
        // so the lyric stays on the line of the notes around it.
        let r = rect_of(&notes[*i]);
        let w = r.width().max(84.0);
        let field = Rect::from_min_size(r.left_top(), egui::vec2(w, r.height().max(16.0)));
        let field = field.translate(egui::vec2(
            (grid.right() - field.right()).min(0.0) + (grid.left() - field.left()).max(0.0),
            (grid.bottom() - field.bottom()).min(0.0) + (grid.top() - field.top()).max(0.0),
        ));
        let id = egui::Id::unique("prowave_lyric_field");
        // A dark value field ringed in orange (it has the keyboard), the lyric in bone.
        hw_vocal::edit_well(ui.painter(), field);
        let font = theme::font(Face::Bold, 11.0);
        let row = ui.fonts_mut(|f| f.row_height(&font));
        let edit = ui.put(
            Rect::from_min_size(
                // The text edit sets its line 2.5 points lower than a painted label: raised by
                // that, the lyric sits on the same line as those of the notes around it.
                egui::pos2(field.left() + 6.0, field.center().y - row / 2.0 - 2.5),
                egui::vec2(field.width() - 12.0, row),
            ),
            egui::TextEdit::singleline(text)
                .id(id)
                .char_limit(200)
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::ZERO)
                .align(egui::Align2::LEFT_CENTER)
                .font(font)
                .text_color(theme::ON_DARK()),
        );
        if !edit.has_focus() && !edit.lost_focus() {
            edit.request_focus();
        }
        // Clicking elsewhere keeps what was typed.
        if edit.lost_focus() {
            changed |= apply(ed, notes, selected, Step::Done);
        }
    }

    let mut close = false;
    let mut applied = None;
    let focus = &mut ed.paste_focus;
    if let Some(text) = ed.paste.as_mut() {
        let panel = egui::Id::unique("prowave_paste_lyrics");
        hw_vocal::scrim(ui.ctx(), panel);
        let (open, _) =
            hw_vocal::window(ui.ctx(), panel, tr("Paste Lyrics"), "", 460.0, None, |ui| {
                paste_panel(ui, text, selected, focus, &mut applied, &mut close)
            });
        close |= !open;
    }
    if let Some(text) = applied {
        let only = if selected.len() > 1 { selected } else { &[] };
        changed |= lyrics::spread(notes, &text, None, only) > 0;
        close = true;
    }
    if close {
        ed.paste = None;
        ed.paste_focus = false;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes(n: usize) -> Vec<Note> {
        (0..n)
            .map(|i| Note {
                start: i as f32,
                len: 1.0,
                pitch: 60,
                vel: 100,
                ..Note::default()
            })
            .collect()
    }

    #[test]
    fn tab_keeps_the_lyric_and_moves_to_the_next_note() {
        let mut ed = LyricEditor::default();
        let mut n = notes(3);
        ed.start(&n, 0);
        ed.editing.as_mut().unwrap().1 = "row".into();
        assert!(apply(&mut ed, &mut n, &[], Step::Next));
        assert_eq!(n[0].lyric.as_str(), "row");
        assert_eq!(ed.editing.as_ref().unwrap().0, 1);
        ed.editing.as_mut().unwrap().1 = "your".into();
        apply(&mut ed, &mut n, &[], Step::Previous);
        assert_eq!(ed.editing.as_ref().unwrap().0, 0);
        assert_eq!(
            ed.editing.as_ref().unwrap().1,
            "row",
            "the field shows the lyric"
        );
        apply(&mut ed, &mut n, &[], Step::Cancel);
        assert!(!ed.is_editing());
        assert_eq!(n[1].lyric.as_str(), "your");
    }

    #[test]
    fn escape_leaves_the_note_and_several_words_spread() {
        let mut ed = LyricEditor::default();
        let mut n = notes(4);
        ed.start(&n, 1);
        ed.editing.as_mut().unwrap().1 = "nope".into();
        assert!(!apply(&mut ed, &mut n, &[], Step::Cancel));
        assert!(n[1].lyric.is_empty());
        ed.start(&n, 1);
        ed.editing.as_mut().unwrap().1 = "row your boat".into();
        apply(&mut ed, &mut n, &[], Step::Done);
        let got: Vec<&str> = n.iter().map(|x| x.lyric.as_str()).collect();
        assert_eq!(got, ["", "row", "your", "boat"]);
        // The last note: Tab stops.
        ed.start(&n, 3);
        apply(&mut ed, &mut n, &[], Step::Next);
        assert!(!ed.is_editing());
    }

    /// A Piano Roll-like stand-in drawn in `full` (keyboard, ruler, region header, lanes and
    /// notes with their lyrics, as the Piano Roll draws them).
    fn roll(g: &egui::Painter, full: Rect, notes: &[Note], rect_of: &dyn Fn(&Note) -> Rect) {
        use egui::{pos2, vec2, Align2, Stroke};
        g.rect_filled(full, 0.0, theme::BONE());
        let keys_w = 60.0;
        let ruler = Rect::from_min_max(
            pos2(full.left() + keys_w, full.top()),
            pos2(full.right(), full.top() + 22.0),
        );
        let header = Rect::from_min_max(
            pos2(ruler.left(), ruler.bottom()),
            pos2(full.right(), ruler.bottom() + 16.0),
        );
        let grid = Rect::from_min_max(pos2(ruler.left(), header.bottom()), full.max);
        // Lanes: darker rows for the black keys, at the pitches `rect_of` puts notes on.
        let row_h = 18.0;
        for pitch in (30..=100).rev() {
            let y = 560.0 - (pitch as f32 - 56.0) * row_h;
            let lane = Rect::from_min_size(pos2(grid.left(), y), vec2(grid.width(), row_h));
            if lane.bottom() <= grid.top() || lane.top() >= full.bottom() {
                continue;
            }
            let lane = lane.intersect(grid);
            let black = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
            g.rect_filled(
                lane,
                0.0,
                if black {
                    theme::GRID_BG()
                } else {
                    theme::BONE()
                },
            );
            let key = Rect::from_min_max(
                pos2(full.left(), lane.top()),
                pos2(full.left() + keys_w, lane.bottom()),
            );
            if black {
                g.rect_filled(
                    Rect::from_min_size(key.min, vec2(keys_w * 0.62, key.height() - 1.0)),
                    1.0,
                    theme::CAP(),
                );
            } else {
                g.rect_filled(key, 0.0, theme::BONE_HI());
                g.hline(key.x_range(), key.bottom(), Stroke::new(1.0, theme::SEAM()));
            }
            if pitch % 12 == 0 {
                hw::text(
                    g,
                    pos2(key.right() - 4.0, key.center().y),
                    Align2::RIGHT_CENTER,
                    &format!("C{}", pitch / 12 - 1),
                    Face::Bold,
                    8.5,
                    theme::INK2(),
                );
            }
        }
        hw::seam_v(g, grid.left() - 1.0, full.y_range());
        // Bars and beats.
        for k in 0..40 {
            let x = grid.left() + 30.0 + k as f32 * 118.0;
            if x > full.right() {
                break;
            }
            let bar = k % 4 == 0;
            g.vline(
                x,
                grid.y_range(),
                Stroke::new(
                    1.0,
                    theme::INK().gamma_multiply(if bar { 0.22 } else { 0.07 }),
                ),
            );
            if bar {
                g.vline(
                    x,
                    ruler.top() + 6.0..=ruler.bottom(),
                    Stroke::new(1.0, theme::INK2()),
                );
                hw::text(
                    g,
                    pos2(x + 4.0, ruler.top() + 4.0),
                    Align2::LEFT_TOP,
                    &format!("{:03}", k / 4 + 1),
                    Face::Bold,
                    11.0,
                    theme::INK(),
                );
            } else {
                g.vline(
                    x,
                    ruler.bottom() - 5.0..=ruler.bottom(),
                    Stroke::new(1.0, theme::INK2().gamma_multiply(0.6)),
                );
            }
        }
        hw::seam_h(g, ruler.x_range(), ruler.bottom());
        // The region's header strip (a region painted taller, clipped to its header).
        let look = theme::region_look(theme::TRACK_PALETTE[0], false, true, false);
        let strip = header
            .shrink2(vec2(0.0, 1.0))
            .with_max_y(header.top() + 14.0);
        hw::paint_region(
            &g.with_clip_rect(strip),
            Rect::from_min_size(strip.min, vec2(strip.width(), 40.0)),
            &look,
            "IN",
            "Top Line 1",
        );
        // The notes and their lyrics.
        for note in notes {
            let r = rect_of(note);
            g.rect_filled(r, 2.0, theme::REGION());
            g.rect_stroke(
                r,
                2.0,
                Stroke::new(1.0, theme::CAP_EDGE()),
                egui::StrokeKind::Inside,
            );
            hw::text(
                g,
                r.left_center() + vec2(6.0, 0.0),
                Align2::LEFT_CENTER,
                note.lyric.as_str(),
                Face::Bold,
                11.0,
                theme::ON_DARK(),
            );
        }
    }

    /// Pictures of a lyric being typed over its note in a Piano Roll-like grid, and of the
    /// Paste Lyrics window over it: `MANUAL_PNG_DIR=… cargo test -p prowave-app lyrics_pictures
    /// -- --ignored`.
    #[test]
    #[ignore = "pictures of the window"]
    fn lyrics_pictures() {
        use egui::{pos2, vec2};
        for (name, paste) in [("lyrics-field.png", false), ("lyrics.png", true)] {
            let mut ed = LyricEditor::default();
            let mut n = notes(8);
            for (note, (w, pitch)) in n.iter_mut().zip([
                ("one", 60u8),
                ("two", 60),
                ("three", 60),
                ("four", 62),
                ("five", 64),
                ("", 64),
                ("", 62),
                ("", 64),
            ]) {
                note.lyric = Lyric::new(w);
                note.pitch = pitch;
            }
            if paste {
                ed.open_paste();
                if let Some(t) = ed.paste.as_mut() {
                    *t =
                        "one two three four five six se-ven eight, nine ten e-le-ven twelve".into();
                }
            } else {
                ed.start(&n, 5);
                if let Some((_, t)) = ed.editing.as_mut() {
                    *t = "gent".into();
                }
            }
            let full = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 620.0));
            let rect_of = |note: &Note| {
                egui::Rect::from_min_size(
                    pos2(
                        92.0 + note.start * 118.0,
                        560.0 - (f32::from(note.pitch) - 56.0) * 18.0,
                    ),
                    vec2(112.0, 16.0),
                )
            };
            let grid = full.with_min_x(60.0).with_min_y(38.0);
            // Two frames: the sheet takes the keyboard on the first.
            let mut frame = 0;
            crate::ui_test::render_png(
                full.width(),
                full.height(),
                2.0,
                theme::BONE(),
                |ui| {
                    theme::apply(ui.ctx());
                    roll(ui.painter(), full, &n, &rect_of);
                    let selected = [5usize];
                    show(ui, &mut ed, &mut n, &selected, grid, false, &rect_of);
                    frame += 1;
                },
                &hw_vocal::picture_path(name),
            )
            .unwrap();
            assert!(frame > 0);
        }
    }
}
