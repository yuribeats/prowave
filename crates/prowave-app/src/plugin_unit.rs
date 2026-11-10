//! The host's frame around a third-party plug-in (Audio Unit or VST 3), as a PW-1 rack unit:
//! its format and name on the faceplate, the ON key (bypass), a preset LCD with its steppers,
//! A/B compare keys, and two view keys: the plug-in's own window, or an editor the host draws
//! from the plug-in's parameter list (knobs, keys and menus with LCD readouts, grouped by the
//! parameters' names, with a search field).
use crate::gear::{self, Cap, Change, Control, Glyph, LedAt};
use crate::i18n::tr;
use egui::{pos2, vec2, Align2, Rect, Sense, Ui};
use prowave_engine::plugin_handle::{HostParam, PluginHandle};

/// What the host keeps for an open unit.
#[derive(Default)]
pub struct UnitState {
    /// The host-drawn editor instead of the plug-in's window.
    pub generic: bool,
    /// The plug-in window's title.
    pub title: String,
    /// Compare: the settings of A and B, and the one in use (0 A, 1 B).
    pub ab: [Option<Vec<u8>>; 2],
    pub side: usize,
    /// The editor's search text.
    pub search: String,
    /// The parameter list, read once.
    pub params: Option<Vec<HostParam>>,
}

/// What the unit's keys asked for this frame.
#[derive(Default, Debug, PartialEq)]
pub struct UnitOut {
    pub close: bool,
    /// Switch to the host-drawn editor (true) or the plug-in's window (false).
    pub view: Option<bool>,
    pub bypass: bool,
    /// Compare A (0) or B (1).
    pub compare: Option<usize>,
    pub preset: Option<i32>,
    /// Parameters moved on the host-drawn editor: (key, value).
    pub edits: Vec<(u32, f32)>,
}

/// Where the unit sits: its strip's plate and the automation of its parameters.
pub struct Where<'a> {
    pub badge: &'a str,
    pub strip: &'a str,
    /// The slot's name ("Maker: Name").
    pub name: &'a str,
    /// The slot is an effect (it can be bypassed); `Some(bypassed)`.
    pub bypassed: Option<bool>,
    /// Automation of parameter `key`: `Some(true)` an envelope plays it, `Some(false)`
    /// overridden by hand.
    pub automated: &'a dyn Fn(u32) -> Option<bool>,
}

const PAD: f32 = 26.0;
const PER_ROW: usize = 7;
const WIDTH: f32 = PER_ROW as f32 * gear::CELL_W + PAD * 2.0;
/// Most height the editor's knobs take before they scroll.
const EDITOR_MAX_H: f32 = 560.0;

/// A plug-in's name without its maker ("Apple: AUDelay" gives "AUDelay") and its maker.
fn split_name(name: &str) -> (&str, &str) {
    match name.split_once(": ") {
        Some((maker, n)) => (n, maker),
        None => (name, ""),
    }
}

/// The parameters matching `search`, grouped: runs of parameters whose names start with the
/// same word form a labelled group; the rest go together unlabelled.
pub(crate) fn groups(params: &[HostParam], search: &str) -> Vec<(String, Vec<usize>)> {
    let needle = search.trim().to_lowercase();
    let head = |s: &str| -> String {
        let mut w = s.split_whitespace();
        match (w.next(), w.next()) {
            (Some(a), Some(_)) => a.trim_end_matches([':', '-', '.']).to_string(),
            _ => String::new(),
        }
    };
    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, p) in params.iter().enumerate() {
        if !needle.is_empty() && !p.name.to_lowercase().contains(&needle) {
            continue;
        }
        let h = head(&p.name);
        match out.last_mut() {
            Some((g, list)) if *g == h => list.push(i),
            _ => out.push((h, vec![i])),
        }
    }
    // Groups of one join the unlabelled run around them.
    let mut merged: Vec<(String, Vec<usize>)> = Vec::new();
    for (g, list) in out {
        let label = if list.len() > 1 { g } else { String::new() };
        match merged.last_mut() {
            Some((lg, ll)) if lg.is_empty() && label.is_empty() => ll.extend(list),
            _ => merged.push((label, list)),
        }
    }
    merged
}

/// Parameter `p` at `v` as a panel control.
fn control(
    h: &PluginHandle,
    p: &HostParam,
    v: f32,
    group: &str,
    automated: Option<bool>,
) -> Control {
    let span = (p.max - p.min).max(1e-9);
    let mut label = p.name.clone();
    if !group.is_empty() {
        if let Some(rest) = label.strip_prefix(group) {
            let rest = rest.trim_start_matches([':', '-', '.', ' ']);
            if !rest.is_empty() {
                label = rest.to_string();
            }
        }
    }
    let choices = if p.steps > 0 && p.names.len() == p.steps as usize + 1 {
        p.names.clone()
    } else {
        Vec::new()
    };
    let norm = ((v - p.min) / span).clamp(0.0, 1.0);
    Control {
        label,
        norm,
        default: ((p.default - p.min) / span).clamp(0.0, 1.0),
        text: h.text(p.key, v).unwrap_or_else(|| format!("{v:.2}")),
        choice: (norm * p.steps as f32).round() as usize,
        choices,
        automated,
        enabled: true,
    }
}

/// The value a panel change sets on `p`.
fn value_of(p: &HostParam, ch: &Change) -> f32 {
    let n = match ch.choice {
        Some(i) if p.steps > 0 => i as f32 / p.steps as f32,
        _ => ch.norm,
    };
    let n = if p.steps > 0 {
        (n * p.steps as f32).round() / p.steps as f32
    } else {
        n
    };
    p.min + (p.max - p.min) * n
}

/// Draws the unit for the plug-in `handle` (None: not running, e.g. bypassed).
pub fn draw(
    ctx: &egui::Context,
    uid: u64,
    state: &mut UnitState,
    handle: Option<&PluginHandle>,
    at: &Where,
) -> UnitOut {
    let mut out = UnitOut::default();
    if state.params.is_none() {
        if let Some(h) = handle {
            state.params = Some(h.host_params());
        }
    }
    let format = handle.map_or("FX", |h| h.format());
    let (name, maker) = split_name(at.name);
    let plate = gear::Plate {
        model: format,
        name,
        strip: Some((at.badge, at.strip)),
        reset: false,
    };
    let id = egui::Id::unique("plugin_unit").with(uid);
    let on = at.bypassed.map(|b| !b);
    let (head, _) = gear::unit_window(ctx, id, WIDTH, &plate, on, |ui| {
        transport_row(ui, id, state, handle, maker, &mut out);
        if state.generic {
            if let (Some(h), Some(params)) = (handle, state.params.as_ref()) {
                editor(ui, id, h, params, &mut state.search, at, &mut out);
            }
        }
    });
    out.close |= head.close;
    out.bypass |= head.power;
    out
}

/// The row under the header: preset steppers and LCD, A/B, and the view keys.
fn transport_row(
    ui: &mut Ui,
    id: egui::Id,
    state: &UnitState,
    handle: Option<&PluginHandle>,
    maker: &str,
    out: &mut UnitOut,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(WIDTH, 78.0), Sense::hover());
    let p = ui.painter().clone();
    let cy = rect.top() + 40.0;
    let presets = handle.map(|h| h.presets()).unwrap_or_default();
    let current = handle.and_then(|h| h.preset());
    // Steppers and the preset LCD.
    let k = 30.0;
    let left = rect.left() + PAD;
    let prev = Rect::from_min_size(pos2(left, cy - k / 2.0), vec2(k, k));
    let lcd = Rect::from_min_size(pos2(prev.right() + 8.0, cy - 25.0), vec2(300.0, 50.0));
    let next = Rect::from_min_size(pos2(lcd.right() + 8.0, cy - k / 2.0), vec2(k, k));
    let step = |d: i32| -> Option<i32> {
        if presets.is_empty() {
            return None;
        }
        let i = current
            .as_ref()
            .and_then(|(n, _)| presets.iter().position(|(m, _)| m == n))
            .map_or(if d > 0 { -1 } else { 0 }, |i| i as i32);
        let j = (i + d).rem_euclid(presets.len() as i32) as usize;
        Some(presets[j].0)
    };
    if gear::glyph_key(ui, prev, id.with("prev"), Cap::Bone, false, Glyph::Left).clicked() {
        out.preset = step(-1);
    }
    if gear::glyph_key(ui, next, id.with("next"), Cap::Bone, false, Glyph::Right).clicked() {
        out.preset = step(1);
    }
    let glass = gear::lcd(&p, lcd);
    gear::lcd_label(
        &p,
        glass.left_top() + vec2(7.0, 5.0),
        Align2::LEFT_TOP,
        tr("Presets"),
    );
    if !maker.is_empty() {
        gear::lcd_label(
            &p,
            pos2(glass.right() - 7.0, glass.top() + 5.0),
            Align2::RIGHT_TOP,
            maker,
        );
    }
    let shown = match &current {
        Some((_, n)) => n.clone(),
        None => "--".to_string(),
    };
    let t = gear::fit(&p, &shown, 15.0, glass.width() - 16.0);
    gear::lcd_text(
        &p,
        pos2(glass.left() + 7.0, glass.bottom() - 4.0),
        Align2::LEFT_BOTTOM,
        &t,
        15.0,
    );
    let resp = ui.interact(lcd, id.with("preset_lcd"), Sense::click());
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(lcd.width());
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                for (n, name) in &presets {
                    let sel = current.as_ref().is_some_and(|(c, _)| c == n);
                    if ui.selectable_label(sel, name).clicked() {
                        out.preset = Some(*n);
                    }
                }
            });
    });
    // A/B compare.
    let mut x = next.right() + 26.0;
    gear::silk(
        &p,
        pos2(x, rect.top() + 12.0),
        Align2::LEFT_CENTER,
        "Compare",
        10.5,
        gear::PRINT,
    );
    for (i, t) in ["A", "B"].iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, cy - k / 2.0), vec2(48.0, k));
        let on = state.side == i;
        if gear::key(ui, r, id.with(("ab", i)), t, Cap::Bone, LedAt::Left, on, on).clicked()
            && !on
            && handle.is_some()
        {
            out.compare = Some(i);
        }
        x += 56.0;
    }
    // The views: the plug-in's window or the host's knobs.
    let right = rect.right() - PAD;
    let gen = Rect::from_min_size(pos2(right - 46.0, cy - k / 2.0), vec2(46.0, k));
    let gui = Rect::from_min_size(pos2(gen.left() - 54.0, cy - k / 2.0), vec2(46.0, k));
    gear::silk(
        &p,
        pos2(gui.left(), rect.top() + 12.0),
        Align2::LEFT_CENTER,
        "View",
        10.5,
        gear::PRINT,
    );
    if gear::glyph_key(
        ui,
        gui,
        id.with("view_gui"),
        Cap::Bone,
        !state.generic,
        Glyph::Window,
    )
    .clicked()
        && state.generic
    {
        out.view = Some(false);
    }
    if gear::glyph_key(
        ui,
        gen,
        id.with("view_knobs"),
        Cap::Bone,
        state.generic,
        Glyph::Knobs,
    )
    .clicked()
        && !state.generic
    {
        out.view = Some(true);
    }
    for (r, lit) in [(gui, !state.generic), (gen, state.generic)] {
        gear::led(&p, pos2(r.center().x, r.bottom() + 9.0), 2.5, lit);
    }
}

/// The host-drawn editor: a search field, then the parameters in groups.
fn editor(
    ui: &mut Ui,
    id: egui::Id,
    h: &PluginHandle,
    params: &[HostParam],
    search: &mut String,
    at: &Where,
    out: &mut UnitOut,
) {
    // The search field, a black field with its own text.
    let (row, _) = ui.allocate_exact_size(vec2(WIDTH, 44.0), Sense::hover());
    let p = ui.painter().clone();
    gear::seam_h(&p, row.left() + 1.0, row.right() - 1.0, row.top());
    let field = Rect::from_min_size(pos2(row.left() + PAD, row.top() + 10.0), vec2(260.0, 26.0));
    p.rect_filled(field.translate(vec2(0.0, 1.0)), 2.0, gear::BONE_HI);
    crate::hw::gradient(
        &p,
        field,
        2.0,
        egui::Color32::from_rgb(0x24, 0x22, 0x20),
        egui::Color32::from_rgb(0x16, 0x15, 0x14),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field.shrink2(vec2(8.0, 4.0))));
    child.add(
        egui::TextEdit::singleline(search)
            .id(id.with("search"))
            .frame(egui::Frame::NONE)
            .font(gear::mono_font(11.0))
            .text_color(crate::theme::ON_DARK)
            .desired_width(field.width() - 16.0),
    );
    let groups = groups(params, search);
    let count: usize = groups.iter().map(|(_, l)| l.len()).sum();
    gear::lcd_chip(
        &p,
        Rect::from_min_size(pos2(field.right() + 12.0, field.top()), vec2(70.0, 26.0)),
        "",
        &format!("{count}"),
    );

    // The groups, seven knobs to a row, scrolling past a height.
    let heights: Vec<f32> = groups
        .iter()
        .map(|(g, l)| {
            l.len().div_ceil(PER_ROW) as f32 * gear::CELL_H + if g.is_empty() { 4.0 } else { 26.0 }
        })
        .collect();
    let total: f32 = heights.iter().sum::<f32>() + 8.0;
    let mut changes: Vec<(usize, Change)> = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt(id.with("scroll"))
        .max_height(EDITOR_MAX_H)
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(WIDTH, total), Sense::hover());
            let p = ui.painter().clone();
            let mut y = rect.top() + 4.0;
            for ((g, list), hgt) in groups.iter().zip(&heights) {
                if !ui.is_rect_visible(Rect::from_min_size(pos2(rect.left(), y), vec2(WIDTH, *hgt)))
                {
                    y += hgt;
                    continue;
                }
                let mut top = y;
                if !g.is_empty() {
                    gear::bracket(
                        &p,
                        pos2(rect.left() + PAD + 6.0, y + 12.0),
                        WIDTH - PAD * 2.0 - 12.0,
                        g,
                    );
                    top += 26.0;
                }
                let controls: Vec<Control> = list
                    .iter()
                    .map(|i| {
                        let hp = &params[*i];
                        let v = h.value(hp.key).unwrap_or(hp.default);
                        control(h, hp, v, g, (at.automated)(hp.key))
                    })
                    .collect();
                let idx: Vec<usize> = (0..controls.len()).collect();
                for ch in gear::control_grid(
                    ui,
                    pos2(rect.left() + PAD, top),
                    PER_ROW,
                    id.with(("g", list[0])),
                    &controls,
                    &idx,
                    44.0,
                ) {
                    changes.push((list[ch.index], ch));
                }
                y += hgt;
            }
        });
    for (i, ch) in changes {
        let hp = &params[i];
        out.edits.push((hp.key, value_of(hp, &ch)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hp(key: u32, name: &str) -> HostParam {
        HostParam {
            key,
            name: name.into(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            steps: 0,
            names: Vec::new(),
        }
    }

    #[test]
    fn parameters_group_by_their_first_word_and_filter_by_search() {
        let ps = vec![
            hp(0, "Mix"),
            hp(1, "Osc1 Level"),
            hp(2, "Osc1 Pitch"),
            hp(3, "Gain"),
            hp(4, "Filter Cutoff"),
            hp(5, "Filter Resonance"),
        ];
        let g = groups(&ps, "");
        assert_eq!(
            g,
            vec![
                (String::new(), vec![0]),
                ("Osc1".into(), vec![1, 2]),
                (String::new(), vec![3]),
                ("Filter".into(), vec![4, 5]),
            ]
        );
        let g = groups(&ps, "res");
        assert_eq!(g, vec![(String::new(), vec![5])]);
    }

    #[test]
    fn stepped_values_snap_to_their_steps() {
        let mut p = hp(0, "Mode");
        p.min = 0.0;
        p.max = 3.0;
        p.steps = 3;
        let ch = Change {
            index: 0,
            norm: 0.4,
            choice: None,
            ended: false,
        };
        assert_eq!(value_of(&p, &ch), 1.0);
        let ch = Change {
            choice: Some(2),
            ..ch
        };
        assert_eq!(value_of(&p, &ch), 2.0);
    }
}
