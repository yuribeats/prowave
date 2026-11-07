//! Interface size. The layout is designed for windows at least [`DESIGN_WIDTH`] points wide;
//! narrower windows (a 13-inch MacBook Air, a small laptop screen, a window not at full size)
//! scale the whole interface down to fit instead of letting panels overlap. On top of that the
//! user can make everything bigger or smaller (View > Bigger / Smaller / Actual Interface Size,
//! Cmd/Ctrl + = / - / 0), remembered between launches.
//!
//! A new zoom makes egui draw every font again, which costs a whole frame, so the fit moves in
//! 5 % steps and waits until the window has stopped changing size (a maximize animation, a
//! drag of the window's edge) before it changes: the animation itself stays smooth.
use std::path::PathBuf;
use std::time::Duration;

/// Narrowest window, in points at 100 %, that the control bar and Tracks area fit in.
pub const DESIGN_WIDTH: f32 = 1480.0;
/// Range and step of the user's interface size.
pub const MIN: f32 = 0.6;
pub const MAX: f32 = 2.0;
pub const STEP: f32 = 0.1;

/// Steps of the automatic fit (rounded down, so the layout still fits).
const FIT_STEP: f32 = 0.05;
/// How long the window's size must stay put before the fit changes.
const SETTLE: Duration = Duration::from_millis(200);

/// Zoom that fits a window `width` points wide (at 100 %) and applies the user's `size`.
pub fn zoom_for(width: f32, size: f32) -> f32 {
    let fit = (width / DESIGN_WIDTH).clamp(MIN, 1.0);
    let fit = ((fit / FIT_STEP + 1e-4).floor() * FIT_STEP).max(MIN);
    (fit * size).clamp(MIN, MAX)
}

/// Whether the window changed size too recently (at `changed_at`, seconds) to change the zoom.
fn settling(now: f64, changed_at: f64) -> bool {
    now - changed_at < SETTLE.as_secs_f64()
}

/// Sets egui's zoom for the current window size. Call once per frame.
pub fn apply(ctx: &egui::Context, size: f32) {
    // The window's width at 100 %, from the scale this frame was laid out with (the zoom just
    // set takes effect next frame, so `zoom_factor()` would not match the window's size yet).
    let (width, now) = ctx.input(|i| {
        let native = i
            .viewport()
            .native_pixels_per_point
            .unwrap_or(i.pixels_per_point);
        (
            i.content_rect().width() * i.pixels_per_point / native,
            i.time,
        )
    });
    let id = egui::Id::unique("ui_scale_window_width");
    let (last_width, changed_at) = ctx
        .data(|d| d.get_temp::<(f32, f64)>(id))
        .unwrap_or((width, f64::NEG_INFINITY));
    let changed_at = if (width - last_width).abs() > 0.5 {
        now
    } else {
        changed_at
    };
    ctx.data_mut(|d| d.insert_temp(id, (width, changed_at)));
    if settling(now, changed_at) {
        ctx.request_repaint_after(SETTLE);
        return;
    }
    let want = zoom_for(width, size);
    if (want - ctx.zoom_factor()).abs() > 0.001 {
        ctx.set_zoom_factor(want);
    }
}

/// One step bigger (`dir` = 1), smaller (-1), or back to 100 % (0).
pub fn step(size: f32, dir: i32) -> f32 {
    if dir == 0 {
        return 1.0;
    }
    let steps = (size / STEP).round() + dir as f32;
    (steps * STEP).clamp(MIN, MAX)
}

fn file() -> Option<PathBuf> {
    Some(crate::recent_files::config_dir()?.join("ui-scale.txt"))
}

/// The interface size saved by [`save`], or 100 %.
pub fn load() -> f32 {
    file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| t.trim().parse::<f32>().ok())
        .filter(|s| s.is_finite())
        .map_or(1.0, |s| s.clamp(MIN, MAX))
}

pub fn save(size: f32) {
    // Tests run menu entries; they must not change the user's settings.
    if cfg!(test) {
        return;
    }
    let Some(f) = file() else { return };
    let written = f
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|_| std::fs::write(&f, format!("{size:.2}\n")));
    if let Err(e) = written {
        tracing::warn!("cannot save the interface size to {}: {e}", f.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_windows_are_scaled_down_to_fit() {
        assert_eq!(zoom_for(1920.0, 1.0), 1.0, "wide enough: 100 %");
        assert_eq!(zoom_for(DESIGN_WIDTH, 1.0), 1.0);
        let air = zoom_for(1280.0, 1.0);
        assert!(
            (air - 0.85).abs() < 1e-4,
            "1280 / 1480 = 0.865, in 5 % steps"
        );
        assert_eq!(zoom_for(300.0, 1.0), MIN, "never smaller than the minimum");
        assert!(
            (zoom_for(1280.0, 1.2) - air * 1.2).abs() < 1e-4,
            "the user's size on top"
        );
    }

    #[test]
    fn the_fit_moves_in_steps_and_waits_for_the_window_to_settle() {
        // 1300 / 1480 = 0.878: rounded down to 0.85, so the layout still fits.
        assert!((zoom_for(1300.0, 1.0) - 0.85).abs() < 1e-4);
        assert!(
            (zoom_for(1301.0, 1.0) - 0.85).abs() < 1e-4,
            "small changes keep the zoom"
        );
        assert!(settling(10.1, 10.0), "changed 0.1 s ago: wait");
        assert!(!settling(10.3, 10.0), "0.3 s ago: apply");
        assert!(
            !settling(0.0, f64::NEG_INFINITY),
            "first frame: apply at once"
        );
    }

    #[test]
    fn interface_size_steps_by_ten_percent_within_range() {
        assert!((step(1.0, 1) - 1.1).abs() < 1e-4);
        assert!((step(1.0, -1) - 0.9).abs() < 1e-4);
        assert_eq!(step(1.7, 0), 1.0);
        assert_eq!(step(MAX, 1), MAX);
        assert_eq!(step(MIN, -1), MIN);
    }
}
