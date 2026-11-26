//! Logic's Marker List (List Editors, the Marker tab): every marker of the song with its
//! position, name and length (up to the next marker; the last one to the end of the song).
//! A click on a row selects the marker and moves the playhead to it (as a click on the Marker
//! track does); a double-click on it renames it in place (Return keeps the name, Escape
//! leaves it); Delete then removes the selected marker (`tracks_area::delete_selected`); the
//! plus key runs Create Marker. The EVENT and MARKER tabs switch the List Editors between the
//! Event list (`event_list.rs`) and this one.
use crate::event_list::{length_parts, position_in, rows_well, BAR_H, COLS_H, ROW_H};
use crate::hw::{self, Cap, Legend};
use crate::i18n::tr;
use crate::model::Project;
use crate::theme::{self, Face};
use crate::UiState;
use egui::{pos2, vec2, Align2, Id, Painter, Rect, Sense, Ui};

/// The columns, from the list's left: the right edge of the position, the left edges of the
/// name and of the length.
const COL_POS: f32 = 104.0;
const COL_NAME: f32 = 116.0;
const COL_LEN: f32 = 286.0;

/// The id of the key of tab `markers` (false: EVENT).
pub(crate) fn tab_id(markers: bool) -> Id {
    Id::unique(("list_editors_tab", markers))
}

/// The id of row `i`.
pub(crate) fn row_id(i: usize) -> Id {
    Id::unique(("marker_list_row", i))
}

/// The id of the + key (Create Marker).
pub(crate) fn add_id() -> Id {
    Id::unique("marker_list_add")
}

/// The marker being renamed and its text so far.
fn rename_id() -> Id {
    Id::unique("marker_list_rename")
}

/// The EVENT and MARKER tabs at the left of the List Editors' title row `bar`. Returns the
/// room they take.
pub(crate) fn tabs(ui: &Ui, g: &Painter, bar: Rect, s: &mut UiState) -> Rect {
    let cy = bar.center().y - 1.0;
    let mut x = bar.left() + 10.0;
    let mut used = Rect::from_min_size(pos2(x, cy - 14.0), vec2(0.0, 28.0));
    for (markers, label) in [(false, tr("Event")), (true, tr("Marker"))] {
        let text = label.to_uppercase();
        let w = hw::galley(g, &text, Face::Bold, 12.0, theme::INK())
            .size()
            .x
            + 30.0;
        let rect = Rect::from_min_size(pos2(x, cy - 14.0), vec2(w, 28.0));
        let on = s.lists.markers == markers;
        let (resp, _) = hw::key(
            ui,
            g,
            rect,
            tab_id(markers),
            Cap::Bone,
            on,
            Some(on),
            Legend::Text(&text),
        );
        if resp.clicked() {
            s.lists.markers = markers;
        }
        used = used.union(rect);
        x = rect.right() + 4.0;
    }
    used
}

/// The Marker list, filling `ui`.
pub fn show(ui: &mut Ui, p: &mut Project, s: &mut UiState) {
    let area = ui.max_rect();
    let g = ui.painter_at(area);
    hw::chassis(&g, area);

    // Title row: the tabs, then the + key (Create Marker) at the right.
    let bar = Rect::from_min_size(area.min, vec2(area.width(), BAR_H));
    crate::piano_roll::menu_row(&g, bar);
    tabs(ui, &g, bar, s);
    let cy = bar.center().y - 1.0;
    let add = Rect::from_min_size(pos2(bar.right() - 46.0, cy - 14.0), vec2(36.0, 28.0));
    if crate::menus::enabled("Create Marker", p, s) {
        let (resp, _) = hw::key(
            ui,
            &g,
            add,
            add_id(),
            Cap::Bone,
            false,
            None,
            Legend::Text("+"),
        );
        if resp.on_hover_text(tr("Create Marker")).clicked() {
            crate::menus::apply("Create Marker", p, s);
        }
    }
    let mut y = area.top() + BAR_H;

    // Column titles.
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
            theme::INK2(),
        );
    };
    head(COL_POS, Align2::RIGHT_CENTER, tr("Position"), 90.0);
    head(
        COL_NAME,
        Align2::LEFT_CENTER,
        tr("Name"),
        COL_LEN - COL_NAME - 8.0,
    );
    head(
        COL_LEN,
        Align2::LEFT_CENTER,
        tr("Length"),
        area.right() - (lx + COL_LEN) - 8.0,
    );
    y += COLS_H;

    // The rows.
    let list = rows_well(&g, area, y);
    let n = p.markers.len();
    let max_scroll = (n as f32 * ROW_H - list.height()).max(0.0);
    let hover = ui.interact(list, Id::unique("marker_list_rows"), Sense::hover());
    if hover.hovered() {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        s.lists.marker_scroll -= dy;
    }
    s.lists.marker_scroll = s.lists.marker_scroll.clamp(0.0, max_scroll);
    let scroll = (s.lists.marker_scroll / ROW_H).round() * ROW_H;
    let gl = ui.painter_at(list);
    let first = (scroll / ROW_H).floor() as usize;
    let last = (((scroll + list.height()) / ROW_H).floor() as usize).min(n);
    let renaming = ui
        .ctx()
        .data(|d| d.get_temp::<(usize, String)>(rename_id()));
    let mut clicked = None;
    let mut rename = None;
    for i in first..last {
        let row = Rect::from_min_size(
            pos2(list.left(), list.top() + i as f32 * ROW_H - scroll),
            vec2(list.width(), ROW_H),
        );
        let resp = ui.interact(row, row_id(i), Sense::click());
        if resp.double_clicked() {
            rename = Some(i);
        } else if resp.clicked() {
            clicked = Some(i);
        }
        let selected = s.tracks.selected_marker == Some(i);
        if selected {
            gl.rect_filled(row, 0.0, theme::ORANGE().gamma_multiply(0.25));
            gl.rect_filled(
                Rect::from_min_size(row.min, vec2(4.0, ROW_H)),
                0.0,
                theme::ORANGE(),
            );
        } else if i % 2 == 1 {
            gl.rect_filled(row, 0.0, theme::ON_DARK().gamma_multiply(0.035));
        }
        let m = &p.markers[i];
        let cy = row.center().y;
        let at = |x: f32, align: Align2, text: &str, room: f32| {
            let shown = hw::fit(&gl, text, Face::Bold, 10.5, room);
            hw::text(
                &gl,
                pos2(lx + x, cy),
                align,
                &shown,
                Face::Bold,
                10.5,
                theme::ON_DARK(),
            );
        };
        // The marker's colour, as a chip before its name.
        gl.rect_filled(
            Rect::from_center_size(pos2(lx + COL_NAME - 6.0, cy), vec2(4.0, ROW_H - 6.0)),
            1.0,
            m.color,
        );
        let ([bar_n, beat, div], tick) = position_in(p, m.beat);
        at(
            COL_POS,
            Align2::RIGHT_CENTER,
            &format!("{bar_n:>3} {beat} {div} {tick:>3}"),
            100.0,
        );
        if renaming.as_ref().is_none_or(|(r, _)| *r != i) {
            at(
                COL_NAME,
                Align2::LEFT_CENTER,
                &m.name,
                COL_LEN - COL_NAME - 8.0,
            );
        }
        if let Some((start, end)) = crate::marker_edit::span(p, i) {
            let l = length_parts(end - start, p.bar_len_at(start));
            at(
                COL_LEN,
                Align2::LEFT_CENTER,
                &format!("{} {} {} {:>3}", l[0], l[1], l[2], l[3]),
                list.right() - (lx + COL_LEN) - 6.0,
            );
        }
    }
    if let Some(i) = clicked {
        select(p, s, i);
    }
    if let Some(i) = rename {
        select(p, s, i);
        let name = p.markers[i].name.clone();
        ui.ctx().data_mut(|d| d.insert_temp(rename_id(), (i, name)));
    }
    rename_field(ui, p, list, scroll, lx);
    hw::seam_v(&g, area.left(), area.y_range());
}

/// Selects marker `i` (the regions are deselected) and moves the playhead to it.
fn select(p: &mut Project, s: &mut UiState, i: usize) {
    let Some(m) = p.markers.get(i) else {
        return;
    };
    p.position_beats = m.beat;
    s.tracks.clear_selection();
    s.tracks.range.clear();
    s.tracks.selected_marker = Some(i);
}

/// The text field over the name of the marker being renamed. Return or clicking away keeps
/// the name (an empty one is not taken), Escape leaves the old one.
fn rename_field(ui: &mut Ui, p: &mut Project, list: Rect, scroll: f32, lx: f32) {
    let Some((i, mut text)) = ui
        .ctx()
        .data(|d| d.get_temp::<(usize, String)>(rename_id()))
    else {
        return;
    };
    let close = |ui: &Ui| {
        ui.ctx()
            .data_mut(|d| d.remove::<(usize, String)>(rename_id()))
    };
    if i >= p.markers.len() {
        close(ui);
        return;
    }
    let top = list.top() + i as f32 * ROW_H - scroll;
    let r = Rect::from_min_max(
        pos2(lx + COL_NAME - 2.0, top),
        pos2(lx + COL_LEN - 6.0, top + ROW_H),
    );
    if !list.intersects(r) {
        close(ui);
        return;
    }
    let edit = ui.put(
        r,
        egui::TextEdit::singleline(&mut text)
            .id(rename_id().with("field"))
            .char_limit(64)
            .font(theme::font(Face::Body, 11.0)),
    );
    if !edit.has_focus() && !edit.lost_focus() {
        edit.request_focus();
    }
    let (enter, escape) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    if escape {
        close(ui);
    } else if enter || edit.lost_focus() {
        let name = text.trim();
        if !name.is_empty() {
            p.markers[i].name = name.to_string();
        }
        close(ui);
    } else {
        ui.ctx().data_mut(|d| d.insert_temp(rename_id(), (i, text)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_test::Harness;
    use egui::{Event, Key, Modifiers, Pos2};

    fn frame(h: &mut Harness, p: &mut Project, s: &mut UiState, events: Vec<Event>) {
        let meters = crate::mixer::Meters::default();
        h.frame(events, |ui| {
            let ctx = ui.ctx().clone();
            crate::key_commands::begin_frame(&ctx);
            crate::menus::handle_keys(&ctx, p, s);
            egui::Panel::right("lists")
                .exact_size(crate::event_list::WIDTH)
                .frame(egui::Frame::new())
                .show(ui, |ui| crate::event_list::show(ui, p, s));
            egui::CentralPanel::default()
                .show(ui, |ui| crate::tracks_area::show(ui, p, s, &meters));
            crate::key_commands::handle(&ctx, p, s);
        });
    }

    fn click(h: &mut Harness, p: &mut Project, s: &mut UiState, at: Pos2) {
        h.time_passes(1.0);
        let down = |pressed| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(h, p, s, vec![Event::PointerMoved(at)]);
        frame(h, p, s, vec![down(true)]);
        frame(h, p, s, vec![down(false)]);
        frame(h, p, s, vec![]);
    }

    fn double_click(h: &mut Harness, p: &mut Project, s: &mut UiState, at: Pos2) {
        h.time_passes(1.0);
        let down = |pressed| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(h, p, s, vec![Event::PointerMoved(at)]);
        frame(h, p, s, vec![down(true)]);
        frame(h, p, s, vec![down(false)]);
        frame(h, p, s, vec![down(true)]);
        frame(h, p, s, vec![down(false)]);
        frame(h, p, s, vec![]);
    }

    fn key(h: &mut Harness, p: &mut Project, s: &mut UiState, key: Key) {
        let ev = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        frame(h, p, s, vec![ev(true), ev(false)]);
        frame(h, p, s, vec![]);
    }

    fn rect(h: &Harness, id: Id) -> Rect {
        h.ctx.read_response(id).expect("on screen").rect
    }

    fn rig() -> (Project, UiState, Harness) {
        let mut p = Project::empty();
        p.add_marker(0.0);
        p.add_marker(16.0);
        p.add_marker(32.0);
        p.markers[1].name = "Verse".into();
        let mut s = UiState::new();
        s.show_lists = true;
        let mut h = Harness::new(1400.0, 600.0);
        frame(&mut h, &mut p, &mut s, vec![]);
        (p, s, h)
    }

    #[test]
    fn the_marker_tab_lists_the_markers_and_a_click_goes_to_one() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        assert!(!s.lists.markers);
        let tab = rect(h, tab_id(true)).center();
        click(h, p, s, tab);
        assert!(s.lists.markers, "the MARKER tab shows the Marker list");
        frame(h, p, s, vec![]);
        for i in 0..3 {
            assert!(h.ctx.read_response(row_id(i)).is_some(), "row {i}");
        }
        s.tracks.select_only(0, 0);
        let row = rect(h, row_id(1)).center();
        click(h, p, s, row);
        assert_eq!(s.tracks.selected_marker, Some(1));
        assert_eq!(p.position_beats, 16.0, "the playhead goes to it");
        assert!(s.tracks.selection().is_empty());
        // Back to the Event list.
        let tab = rect(h, tab_id(false)).center();
        click(h, p, s, tab);
        assert!(!s.lists.markers);
    }

    #[test]
    fn a_double_click_renames_and_delete_removes_the_selected_marker_one_undo_step_each() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        s.lists.markers = true;
        frame(h, p, s, vec![]);
        let mut history = crate::undo::History::new(p);
        let row = rect(h, row_id(1)).center();
        double_click(h, p, s, row);
        frame(h, p, s, vec![]);
        assert!(
            h.ctx.egui_wants_keyboard_input(),
            "the name field has the keys"
        );
        // Delete while typing is text, not Delete Marker.
        key(h, p, s, Key::Backspace);
        assert_eq!(p.markers.len(), 3);
        frame(h, p, s, vec![Event::Text("Chorus".into())]);
        assert_eq!(p.markers[1].name, "Verse", "not taken before Return");
        key(h, p, s, Key::Enter);
        assert_eq!(p.markers[1].name, "VersChorus");
        assert!(history.record(p));
        // Escape leaves the name.
        double_click(h, p, s, row);
        frame(h, p, s, vec![Event::Text("X".into())]);
        key(h, p, s, Key::Escape);
        assert_eq!(p.markers[1].name, "VersChorus");
        // Delete removes the selected marker.
        frame(h, p, s, vec![]);
        assert_eq!(s.tracks.selected_marker, Some(1));
        key(h, p, s, Key::Delete);
        let beats: Vec<f64> = p.markers.iter().map(|m| m.beat).collect();
        assert_eq!(beats, vec![0.0, 32.0]);
        assert!(history.record(p));
        assert!(history.undo(p));
        assert_eq!(p.markers.len(), 3);
        assert!(history.undo(p));
        assert_eq!(p.markers[1].name, "Verse");
    }

    #[test]
    fn the_plus_key_creates_a_marker_at_the_playhead() {
        let (mut p, mut s, mut h) = rig();
        let (p, s, h) = (&mut p, &mut s, &mut h);
        s.lists.markers = true;
        frame(h, p, s, vec![]);
        p.position_beats = 45.0;
        let add = rect(h, add_id()).center();
        click(h, p, s, add);
        let beats: Vec<f64> = p.markers.iter().map(|m| m.beat).collect();
        assert_eq!(
            beats,
            vec![0.0, 16.0, 32.0, 44.0],
            "rounded to the nearest bar"
        );
    }
}
