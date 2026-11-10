//! Small widgets shared by the local toolbars (Tracks area, Mixer), in the Hardware look (the
//! painters behind them are in `hw.rs`).
use crate::i18n::tr;
use egui::{pos2, vec2, Align2, Color32, Id, Painter, Rect, Response, Sense, Ui};

pub const TEXT: Color32 = crate::theme::TEXT;

/// Whether `resp` asks for its right-click menu this frame: a right-click (or long touch), and
/// on macOS also a Control-click, as with a one-button mouse or a trackpad.
pub fn context_click(resp: &Response) -> bool {
    let control_click =
        cfg!(target_os = "macos") && resp.clicked() && resp.ctx.input(|i| i.modifiers.ctrl);
    resp.secondary_clicked() || control_click
}

/// Right-click menu at the pointer for `resp` (see [`context_click`]). Closes when an entry is
/// clicked, the pointer clicks elsewhere or Escape is pressed.
pub fn context_menu(resp: &Response, add_contents: impl FnOnce(&mut Ui)) {
    context_menu_shown(resp, false, add_contents);
}

/// [`context_menu`], or with `shown` the menu open over the middle of `resp` without a click
/// (only for the layout pictures, `--shots`).
pub fn context_menu_shown(resp: &Response, shown: bool, add_contents: impl FnOnce(&mut Ui)) {
    let open = if context_click(resp) || shown {
        Some(egui::SetOpenCommand::Bool(true))
    } else if resp.clicked() {
        Some(egui::SetOpenCommand::Bool(false))
    } else {
        None
    };
    let popup = egui::Popup::menu(resp).open_memory(open);
    let popup = if shown {
        popup.at_position(resp.rect.center())
    } else {
        popup.at_pointer_fixed()
    };
    popup.show(|ui| {
        ui.set_min_width(190.0);
        add_contents(ui);
    });
}

/// A menu title in mono caps with a small down triangle ("EDIT ▾"). Returns the response and
/// width.
pub fn menu_title(
    ui: &Ui,
    g: &Painter,
    left: f32,
    cy: f32,
    label: &str,
    id: Id,
) -> (Response, f32) {
    use crate::theme::Face;
    let label = tr(label).to_uppercase();
    let size = 12.0;
    let tw = crate::hw::galley(g, &label, Face::Bold, size, TEXT)
        .size()
        .x;
    let w = tw + 24.0;
    let r = Rect::from_min_size(pos2(left, cy - 10.0), vec2(w, 20.0));
    let resp = ui.interact(r, id, Sense::click());
    if resp.hovered() {
        g.rect_filled(r, 3.0, Color32::from_black_alpha(14));
    }
    crate::hw::text(
        g,
        pos2(r.left() + 6.0, cy),
        Align2::LEFT_CENTER,
        &label,
        Face::Bold,
        size,
        TEXT,
    );
    crate::hw::tri_down(g, pos2(r.right() - 9.0, cy + 0.5), crate::theme::INK2);
    (resp, w)
}

/// Popup button: a dark value field with up/down arrows (the Hardware dropdown).
pub fn popup_button(ui: &Ui, g: &Painter, rect: Rect, id: Id, text: &str) -> Response {
    crate::hw::field(ui, g, rect, id, text, true)
}

/// Undo and Redo entries for the local Edit menus, greyed when there is nothing to undo or
/// redo. The one picked goes to `pick` (and from there to `UiState::undo_cmd`).
pub fn undo_items(
    ui: &mut Ui,
    can_undo: bool,
    can_redo: bool,
    pick: &mut Option<crate::undo::UndoCmd>,
) {
    use egui::{Key, KeyboardShortcut, Modifiers};
    let key = |m, k| ui.ctx().format_shortcut(&KeyboardShortcut::new(m, k));
    let (undo_key, redo_key) = (
        key(Modifiers::COMMAND, Key::Z),
        key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z),
    );
    if ui
        .add_enabled(
            can_undo,
            egui::Button::new(tr("Undo")).shortcut_text(undo_key),
        )
        .clicked()
    {
        *pick = Some(crate::undo::UndoCmd::Undo);
        ui.close();
    }
    if ui
        .add_enabled(
            can_redo,
            egui::Button::new(tr("Redo")).shortcut_text(redo_key),
        )
        .clicked()
    {
        *pick = Some(crate::undo::UndoCmd::Redo);
        ui.close();
    }
}

/// Horizontal slider: a black slot and a bone cap with a black line. Returns the new 0..1
/// value when dragged.
pub fn slider_h(
    ui: &Ui,
    g: &Painter,
    rect: Rect,
    id: Id,
    value: f32,
    enabled: bool,
) -> Option<f32> {
    let cap = vec2(9.0, (rect.height() + 4.0).clamp(12.0, 18.0));
    let shown = if enabled {
        g.clone()
    } else {
        let mut f = g.clone();
        f.set_opacity(0.45);
        f
    };
    crate::hw::paint_fader_h(&shown, rect, value, cap);
    if !enabled {
        return None;
    }
    let resp = ui.interact(rect.expand2(vec2(2.0, 5.0)), id, Sense::click_and_drag());
    resp.interact_pointer_pos()
        .map(|p| ((p.x - rect.left() - cap.x / 2.0) / (rect.width() - cap.x)).clamp(0.0, 1.0))
}

/// Rename in place (Logic: double-click a name): a double-click on `resp` within `rect`
/// opens a text field over `rect` holding `current`. Return or clicking away gives the new
/// name, Escape keeps the old one. `key` tells the fields apart.
pub fn rename_field(
    ui: &Ui,
    resp: &Response,
    rect: Rect,
    key: Id,
    current: &str,
    font: egui::FontId,
) -> Option<String> {
    rename_field_opened(ui, resp, rect, key, current, font, false)
}

/// [`rename_field`], also opened at once when `open` (the Text tool's single click).
pub fn rename_field_opened(
    ui: &Ui,
    resp: &Response,
    rect: Rect,
    key: Id,
    current: &str,
    font: egui::FontId,
    open: bool,
) -> Option<String> {
    let (buf_id, edit_id) = (key.with("rename_buf"), key.with("rename_edit"));
    let on_name = resp
        .interact_pointer_pos()
        .is_some_and(|at| rect.contains(at));
    if (resp.double_clicked() && on_name) || open {
        ui.data_mut(|d| d.insert_temp(buf_id, current.to_string()));
        ui.memory_mut(|m| m.request_focus(edit_id));
    }
    let mut text = ui.data(|d| d.get_temp::<String>(buf_id))?;
    let mut result = None;
    egui::Area::new(key.with("rename_area"))
        .fixed_pos(rect.min)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut text)
                    .id(edit_id)
                    .font(font)
                    .desired_width(rect.width()),
            );
            let cancel = ui.input(|inp| inp.key_pressed(egui::Key::Escape));
            if edit.lost_focus() || cancel {
                let name = text.trim();
                if !cancel && !name.is_empty() && name != current {
                    result = Some(name.to_string());
                }
                ui.data_mut(|d| d.remove::<String>(buf_id));
            } else {
                ui.data_mut(|d| d.insert_temp(buf_id, text.clone()));
            }
        });
    result
}

/// Whether [`rename_field`] for `key` is open (the name under it need not be drawn).
pub fn renaming(ui: &Ui, key: Id) -> bool {
    ui.data(|d| d.get_temp::<String>(key.with("rename_buf")).is_some())
}
