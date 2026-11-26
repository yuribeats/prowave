//! The Piano Roll's tools (Logic Pro: Piano Roll tools), their icons and the Tool menu that
//! the T key opens at the pointer, where one more key picks a tool (T again: Pointer), with
//! Logic's letters: <https://support.apple.com/guide/logicpro/tool-menu-lgcp0182c4e5/mac>.
//! Left-click and Command-click tools:
//! <https://support.apple.com/guide/logicpro/tools-overview-lgcp9b92f74c/mac>.
use crate::hw;
use crate::i18n::tr;
use crate::theme::{self, Face};
use egui::{pos2, vec2, Align2, Color32, Key, Painter, Pos2, Rect, Shape, Stroke};

/// A Piano Roll tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    /// Select, move, copy (Option-drag) and resize notes; a click on empty space adds a note.
    Pointer,
    /// Add notes: click, or click and drag to set the length.
    Pencil,
    /// Delete the notes clicked or dragged over.
    Eraser,
    /// Cut a note in two where it is clicked (at the snap).
    Scissors,
    /// Join (Logic's Glue): the selected notes of each pitch become one.
    Glue,
    /// Mute or unmute a note (or the selection): muted notes are grey and do not play.
    Mute,
    /// Drag a note up or down to change its velocity (and the selection's; Option-Shift:
    /// all to the same value).
    Velocity,
    /// Quantize the notes clicked or dragged across (or the selection) with Time Quantize.
    Quantize,
    /// Paint a note on every Time Quantize step the pointer passes (Shift: one pitch).
    Brush,
    /// Change a note's length from anywhere on it.
    Finger,
    /// Click to zoom in, Option-click to zoom out.
    Zoom,
}

impl Tool {
    /// The tools in the order of Logic's Tool menu, with the key that picks each in it.
    pub const ALL: [(Tool, Key, &'static str); 11] = [
        (Tool::Pointer, Key::T, "T"),
        (Tool::Pencil, Key::P, "P"),
        (Tool::Eraser, Key::E, "E"),
        (Tool::Scissors, Key::I, "I"),
        (Tool::Glue, Key::J, "J"),
        (Tool::Mute, Key::M, "M"),
        (Tool::Zoom, Key::Y, "Y"),
        (Tool::Quantize, Key::Q, "Q"),
        (Tool::Velocity, Key::V, "V"),
        (Tool::Brush, Key::B, "B"),
        (Tool::Finger, Key::F, "F"),
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Pointer => tr("Pointer Tool"),
            Tool::Pencil => tr("Pencil Tool"),
            Tool::Eraser => tr("Eraser Tool"),
            Tool::Scissors => tr("Scissors Tool"),
            Tool::Glue => tr("Join Tool"),
            Tool::Mute => tr("Mute Tool"),
            Tool::Velocity => tr("Velocity Tool"),
            Tool::Quantize => tr("Quantize Tool"),
            Tool::Brush => tr("Brush Tool"),
            Tool::Finger => tr("Finger Tool"),
            Tool::Zoom => tr("Zoom Tool"),
        }
    }

    /// The tool a key picks in the open Tool menu.
    pub fn for_key(key: Key) -> Option<Tool> {
        Self::ALL
            .iter()
            .find(|(_, k, _)| *k == key)
            .map(|(t, _, _)| *t)
    }

    /// The pointer's look over the grid with this tool.
    pub fn cursor(self) -> egui::CursorIcon {
        match self {
            Tool::Pointer | Tool::Velocity => egui::CursorIcon::Default,
            Tool::Pencil | Tool::Brush => egui::CursorIcon::Crosshair,
            Tool::Eraser | Tool::Mute | Tool::Glue | Tool::Quantize => {
                egui::CursorIcon::PointingHand
            }
            Tool::Scissors => egui::CursorIcon::VerticalText,
            Tool::Finger => egui::CursorIcon::ResizeHorizontal,
            Tool::Zoom => egui::CursorIcon::ZoomIn,
        }
    }
}

/// Draws the icon of `tool` centred on `c` in `col`.
pub fn draw_icon(g: &Painter, c: Pos2, tool: Tool, col: Color32) {
    let s = Stroke::new(1.4, col);
    match tool {
        Tool::Pointer => {
            g.add(Shape::line(
                vec![
                    c + vec2(-3.0, -6.0),
                    c + vec2(5.0, 2.0),
                    c + vec2(1.0, 2.5),
                    c + vec2(-1.0, 7.0),
                    c + vec2(-3.0, -6.0),
                ],
                s,
            ));
        }
        Tool::Pencil | Tool::Brush => {
            g.line_segment(
                [c + vec2(5.0, -6.0), c + vec2(-4.0, 3.0)],
                Stroke::new(if tool == Tool::Brush { 3.4 } else { 2.4 }, col),
            );
            g.add(Shape::convex_polygon(
                vec![
                    c + vec2(-4.0, 3.0),
                    c + vec2(-6.0, 7.0),
                    c + vec2(-2.0, 5.0),
                ],
                col,
                Stroke::NONE,
            ));
        }
        Tool::Eraser => {
            let r = Rect::from_center_size(c, vec2(12.0, 7.0));
            g.rect_stroke(r, 1.5, s, egui::StrokeKind::Inside);
            g.rect_filled(
                Rect::from_min_max(r.min, pos2(r.left() + 5.0, r.bottom())),
                1.5,
                col,
            );
        }
        Tool::Scissors => {
            g.circle_stroke(c + vec2(-3.5, 4.0), 2.4, s);
            g.circle_stroke(c + vec2(3.5, 4.0), 2.4, s);
            g.line_segment([c + vec2(-2.0, 2.0), c + vec2(4.0, -7.0)], s);
            g.line_segment([c + vec2(2.0, 2.0), c + vec2(-4.0, -7.0)], s);
        }
        Tool::Velocity => {
            g.line_segment([c + vec2(-5.0, 6.0), c + vec2(-5.0, -2.0)], s);
            g.line_segment([c + vec2(0.0, 6.0), c + vec2(0.0, -6.0)], s);
            g.line_segment([c + vec2(5.0, 6.0), c + vec2(5.0, 1.0)], s);
        }
        Tool::Finger => {
            g.line_segment([c + vec2(-6.0, 0.0), c + vec2(6.0, 0.0)], s);
            g.line_segment([c + vec2(6.0, 0.0), c + vec2(3.0, -3.0)], s);
            g.line_segment([c + vec2(6.0, 0.0), c + vec2(3.0, 3.0)], s);
            g.line_segment([c + vec2(-6.0, -5.0), c + vec2(-6.0, 5.0)], s);
        }
        Tool::Zoom => {
            g.circle_stroke(c + vec2(-1.5, -1.5), 4.5, s);
            g.line_segment(
                [c + vec2(2.0, 2.0), c + vec2(6.0, 6.0)],
                Stroke::new(2.0, col),
            );
        }
        Tool::Glue | Tool::Mute | Tool::Quantize => {
            let t = match tool {
                Tool::Glue => "J",
                Tool::Mute => "M",
                _ => "Q",
            };
            hw::text(g, c, Align2::CENTER_CENTER, t, Face::Bold, 12.0, col);
        }
    }
}

/// A tool that the Tool menu (T) offers: the Piano Roll's [`Tool`] and the Tracks area's
/// `TrackTool` share the menu, its keys and its look.
pub trait MenuTool: Copy + PartialEq + 'static {
    /// The tools in menu order, each with its key, whether Shift goes with the key, and the
    /// legend printed on the key cap.
    fn entries() -> &'static [(Self, Key, bool, &'static str)];
    fn label(self) -> &'static str;
    fn paint(self, g: &Painter, c: Pos2, col: Color32);

    /// The tool a key (with Shift held or not) picks in the open Tool menu.
    fn for_key(key: Key, shift: bool) -> Option<Self> {
        Self::entries()
            .iter()
            .find(|(_, k, sh, _)| *k == key && *sh == shift)
            .map(|(t, ..)| *t)
    }
}

/// The Piano Roll's tools for the menu: [`Tool::ALL`], none with Shift.
const PIANO_ENTRIES: [(Tool, Key, bool, &str); 11] = {
    let mut out = [(Tool::Pointer, Key::T, false, "T"); 11];
    let mut i = 0;
    while i < 11 {
        let (t, k, l) = Tool::ALL[i];
        out[i] = (t, k, false, l);
        i += 1;
    }
    out
};

impl MenuTool for Tool {
    fn entries() -> &'static [(Self, Key, bool, &'static str)] {
        &PIANO_ENTRIES
    }
    fn label(self) -> &'static str {
        self.name()
    }
    fn paint(self, g: &Painter, c: Pos2, col: Color32) {
        draw_icon(g, c, self, col);
    }
}

/// The Tool menu that T opens at the pointer: a click or the tool's key picks a tool, Esc or
/// a click elsewhere closes it. Returns the tool picked, and whether the menu stays open.
/// The keys it uses are taken out of the frame's input, so that (say) R picking a tool does
/// not also start recording.
pub fn tool_menu<T: MenuTool>(
    ctx: &egui::Context,
    salt: &str,
    at: Pos2,
    current: T,
) -> (Option<T>, bool) {
    let mut picked = None;
    let mut open = true;
    ctx.input_mut(|i| {
        i.events.retain(|e| {
            let egui::Event::Key {
                key,
                pressed,
                modifiers,
                ..
            } = e
            else {
                return true;
            };
            if modifiers.command || modifiers.alt || modifiers.ctrl {
                return true;
            }
            if *key == Key::Escape {
                open &= !*pressed;
                return false;
            }
            match T::for_key(*key, modifiers.shift) {
                Some(t) => {
                    if *pressed {
                        picked = Some(t);
                    }
                    false
                }
                None => true,
            }
        });
    });
    let area = egui::Area::new(egui::Id::unique(("tool_menu", salt.to_owned())))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            // The dropdown material: a dark field, the current tool in orange, each tool's key
            // on a small bone cap.
            egui::Frame::menu(ui.style())
                .fill(theme::CAP())
                .stroke(egui::Stroke::new(1.0, theme::CAP_EDGE()))
                .show(ui, |ui| {
                    ui.set_min_width(200.0);
                    for &(tool, _, _, letter) in T::entries() {
                        let (r, resp) =
                            ui.allocate_exact_size(vec2(200.0, 24.0), egui::Sense::click());
                        let g = ui.painter();
                        let on = tool == current;
                        if resp.hovered() {
                            g.rect_filled(r, 3.0, theme::CAP_TOP());
                        }
                        let col = if on {
                            theme::ORANGE()
                        } else {
                            theme::ON_DARK()
                        };
                        tool.paint(g, pos2(r.left() + 14.0, r.center().y), col);
                        hw::text(
                            g,
                            pos2(r.left() + 32.0, r.center().y),
                            Align2::LEFT_CENTER,
                            &tool.label().to_uppercase(),
                            Face::Bold,
                            11.5,
                            col,
                        );
                        let cap_w = if letter.chars().count() > 1 {
                            26.0
                        } else {
                            18.0
                        };
                        let cap = Rect::from_center_size(
                            pos2(r.right() - 7.0 - cap_w / 2.0, r.center().y - 1.0),
                            vec2(cap_w, 17.0),
                        );
                        let face = hw::paint_key(g, cap, hw::Cap::Bone, false, false);
                        hw::text(
                            g,
                            face.center(),
                            Align2::CENTER_CENTER,
                            letter,
                            Face::Bold,
                            10.0,
                            theme::INK(),
                        );
                        if resp.clicked() {
                            picked = Some(tool);
                        }
                    }
                });
        });
    let clicked_out = ctx.input(|i| i.pointer.any_pressed())
        && ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| !area.response.rect.contains(p));
    if clicked_out {
        open = false;
    }
    (picked, open && picked.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_its_own_key() {
        for (i, (t, k, _)) in Tool::ALL.iter().enumerate() {
            assert_eq!(Tool::for_key(*k), Some(*t));
            assert!(Tool::ALL[i + 1..].iter().all(|(u, j, _)| u != t && j != k));
        }
    }
}
