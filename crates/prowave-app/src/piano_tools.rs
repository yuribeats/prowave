//! The Piano Roll's tools (Logic Pro: Piano Roll tools), their icons and the Tool menu that
//! the T key opens at the pointer, where one more key picks a tool (T again: Pointer), with
//! Logic's letters: <https://support.apple.com/guide/logicpro/tool-menu-lgcp0182c4e5/mac>.
//! Left-click and Command-click tools:
//! <https://support.apple.com/guide/logicpro/tools-overview-lgcp9b92f74c/mac>.
use crate::i18n::tr;
use crate::widgets::{BLUE, TEXT};
use egui::{pos2, vec2, Align2, Color32, FontId, Key, Painter, Pos2, Rect, Shape, Stroke};

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
            g.text(c, Align2::CENTER_CENTER, t, FontId::proportional(13.0), col);
        }
    }
}

/// The Tool menu that T opens at the pointer: a click or the tool's key picks a tool, Esc or
/// a click elsewhere closes it. Returns the tool picked, and whether the menu stays open.
pub fn tool_menu(ctx: &egui::Context, at: Pos2, current: Tool) -> (Option<Tool>, bool) {
    let mut picked = None;
    let mut open = true;
    let keys = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.command => Some(*key),
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    for key in keys {
        if key == Key::Escape {
            open = false;
        } else if let Some(t) = Tool::for_key(key) {
            picked = Some(t);
        }
    }
    let area = egui::Area::new(egui::Id::unique("piano_tool_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_min_width(190.0);
                for (tool, _, letter) in Tool::ALL {
                    let (r, resp) = ui.allocate_exact_size(vec2(190.0, 22.0), egui::Sense::click());
                    let g = ui.painter();
                    let on = tool == current;
                    if resp.hovered() || on {
                        g.rect_filled(
                            r,
                            3.0,
                            if resp.hovered() {
                                BLUE
                            } else {
                                Color32::from_white_alpha(18)
                            },
                        );
                    }
                    let col = if resp.hovered() { Color32::WHITE } else { TEXT };
                    draw_icon(g, pos2(r.left() + 14.0, r.center().y), tool, col);
                    g.text(
                        pos2(r.left() + 30.0, r.center().y),
                        Align2::LEFT_CENTER,
                        tool.name(),
                        FontId::proportional(12.5),
                        col,
                    );
                    g.text(
                        pos2(r.right() - 8.0, r.center().y),
                        Align2::RIGHT_CENTER,
                        letter,
                        FontId::proportional(12.0),
                        col,
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
