//! Dark-gray look modelled on the reference screenshots (Logic Pro-style chrome).
use egui::{Color32, CornerRadius, Stroke};

pub const BG: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3c);
pub const PANEL: Color32 = Color32::from_rgb(0x4a, 0x4a, 0x4c);
pub const GRID_BG: Color32 = Color32::from_rgb(0x2b, 0x2b, 0x2b);
pub const TEXT: Color32 = Color32::from_rgb(0xdc, 0xdc, 0xdc);
pub const ACCENT_BLUE: Color32 = Color32::from_rgb(0x2f, 0x6f, 0xd8);

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = GRID_BG;
    v.override_text_color = Some(TEXT);
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x5a, 0x5a, 0x5c);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x5a, 0x5a, 0x5c);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x6a, 0x6a, 0x6c);
    v.widgets.active.weak_bg_fill = ACCENT_BLUE;
    v.selection.bg_fill = ACCENT_BLUE;
    let r = CornerRadius::same(4);
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x22, 0x22, 0x22));
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(6.0, 2.0);
        s.text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(12.5));
        s.text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(12.5));
    });
}
