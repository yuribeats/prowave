//! macOS: a running plug-in's handle and window, whatever its format (Audio Unit or VST 3),
//! so the app keeps one map of each.
use crate::au::{AuHandle, AuWindow};
pub use crate::plugin_window_layout::ClickProbe;
use crate::vst3::{Vst3Handle, Vst3Window};

/// The UI thread's way to a running plug-in: its settings and its window.
#[derive(Clone)]
pub enum PluginHandle {
    Au(AuHandle),
    Vst3(Vst3Handle),
}

impl PluginHandle {
    /// Its settings, for saving in a project.
    pub fn state(&self) -> Option<Vec<u8>> {
        match self {
            Self::Au(h) => h.state(),
            Self::Vst3(h) => h.state(),
        }
    }

    /// Restores settings from [`PluginHandle::state`].
    pub fn set_state(&self, bytes: &[u8]) -> bool {
        match self {
            Self::Au(h) => h.set_state(bytes),
            Self::Vst3(h) => h.set_state(bytes),
        }
    }

    /// Opens its window. Main thread only.
    pub fn open_window(&self, title: &str) -> Result<PluginWindow, String> {
        match self {
            Self::Au(h) => h.open_window(title).map(PluginWindow::Au),
            Self::Vst3(h) => h.open_window(title).map(PluginWindow::Vst3),
        }
    }
}

/// One parameter of a running plug-in, for an editor the host draws itself.
#[derive(Clone, Debug, PartialEq)]
pub struct HostParam {
    /// The Audio Unit's parameter id, or the VST 3 parameter's index.
    pub key: u32,
    pub name: String,
    /// Its range (VST 3: 0..1, normalized) and default.
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// Steps between `min` and `max` (0: continuous).
    pub steps: u32,
    /// The names of a stepped parameter's values, from `min` up (empty when it gives none).
    pub names: Vec<String>,
}

/// Most values a stepped VST 3 parameter has its names read for.
const MAX_NAMED_STEPS: i32 = 32;

impl PluginHandle {
    /// "AU" or "VST3".
    pub fn format(&self) -> &'static str {
        match self {
            Self::Au(_) => "AU",
            Self::Vst3(_) => "VST3",
        }
    }

    /// The parameters a host may move, in the plug-in's order. UI thread.
    pub fn host_params(&self) -> Vec<HostParam> {
        match self {
            Self::Au(h) => h
                .parameters()
                .into_iter()
                .map(|p| {
                    let names = if p.stepped {
                        h.parameter_value_names(p.id)
                    } else {
                        Vec::new()
                    };
                    let steps = if p.stepped {
                        (p.max - p.min).round().max(1.0) as u32
                    } else {
                        0
                    };
                    HostParam {
                        key: p.id,
                        name: p.name,
                        min: p.min,
                        max: p.max,
                        default: p.default,
                        steps,
                        names,
                    }
                })
                .collect(),
            Self::Vst3(h) => (0..h.parameter_count())
                .filter_map(|k| {
                    let (name, free, steps) = h.parameter_info(k)?;
                    if !free {
                        return None;
                    }
                    let names = if steps > 0 && steps <= MAX_NAMED_STEPS {
                        (0..=steps)
                            .map(|i| {
                                h.parameter_text(k, f64::from(i) / f64::from(steps))
                                    .unwrap_or_default()
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    Some(HostParam {
                        key: k as u32,
                        name,
                        min: 0.0,
                        max: 1.0,
                        default: h.parameter_default(k).unwrap_or(0.0) as f32,
                        steps: steps.max(0) as u32,
                        names,
                    })
                })
                .collect(),
        }
    }

    /// Parameter `key`'s value now, in its range. UI thread.
    pub fn value(&self, key: u32) -> Option<f32> {
        match self {
            Self::Au(h) => h.parameter(key),
            Self::Vst3(h) => h.parameter(key as usize).map(|v| v as f32),
        }
    }

    /// Moves parameter `key` to `value` (in its range) as a hand on its knob does: the plug-in
    /// takes it, its own window follows, and the host hears it as a touch (automation).
    /// UI thread.
    pub fn edit(&self, key: u32, value: f32) {
        match self {
            Self::Au(h) => h.edit_parameter(key, value),
            Self::Vst3(h) => h.edit_parameter(key as usize, f64::from(value)),
        }
    }

    /// The text the plug-in shows for parameter `key` at `value`. UI thread.
    pub fn text(&self, key: u32, value: f32) -> Option<String> {
        match self {
            Self::Au(h) => h.parameter_text(key, value),
            Self::Vst3(h) => h.parameter_text(key as usize, f64::from(value)),
        }
    }

    /// The plug-in's own presets: (number, name). Audio Units' factory presets; none for
    /// VST 3 (its programs are not read).
    pub fn presets(&self) -> Vec<(i32, String)> {
        match self {
            Self::Au(h) => h.factory_presets(),
            Self::Vst3(_) => Vec::new(),
        }
    }

    /// Loads preset `number` of [`PluginHandle::presets`].
    pub fn set_preset(&self, number: i32) -> bool {
        match self {
            Self::Au(h) => h.set_factory_preset(number),
            Self::Vst3(_) => false,
        }
    }

    /// The preset loaded now, when the plug-in says.
    pub fn preset(&self) -> Option<(i32, String)> {
        match self {
            Self::Au(h) => h.present_preset(),
            Self::Vst3(_) => None,
        }
    }
}

impl From<AuHandle> for PluginHandle {
    fn from(h: AuHandle) -> Self {
        Self::Au(h)
    }
}

impl From<Vst3Handle> for PluginHandle {
    fn from(h: Vst3Handle) -> Self {
        Self::Vst3(h)
    }
}

/// A plug-in's open window. Dropping it closes it.
pub enum PluginWindow {
    Au(AuWindow),
    Vst3(Vst3Window),
}

impl PluginWindow {
    /// Whether the user has not closed it.
    pub fn is_open(&self) -> bool {
        match self {
            Self::Au(w) => w.is_open(),
            Self::Vst3(w) => w.is_open(),
        }
    }

    /// Keeps the window the size of the plug-in's view. Once a UI frame.
    pub fn fit(&self) {
        match self {
            Self::Au(w) => w.fit(),
            Self::Vst3(w) => w.fit(),
        }
    }

    /// The window's number (what `screencapture -l` takes).
    pub fn window_number(&self) -> isize {
        match self {
            Self::Au(w) => w.window_number(),
            Self::Vst3(w) => w.window_number(),
        }
    }

    /// Brings it to the front.
    pub fn show(&self) {
        match self {
            Self::Au(w) => w.show(),
            Self::Vst3(w) => w.show(),
        }
    }

    /// Whether it fills the screen (the green button).
    pub fn is_full_screen(&self) -> bool {
        match self {
            Self::Au(w) => w.is_full_screen(),
            Self::Vst3(w) => w.is_full_screen(),
        }
    }

    /// The green button: into or out of full screen.
    pub fn toggle_full_screen(&self) {
        match self {
            Self::Au(w) => w.toggle_full_screen(),
            Self::Vst3(w) => w.toggle_full_screen(),
        }
    }

    /// The plug-in view's size (its own, unscaled) and the window's content size, in points.
    pub fn sizes(&self) -> ((f64, f64), (f64, f64)) {
        match self {
            Self::Au(w) => w.sizes(),
            Self::Vst3(w) => w.sizes(),
        }
    }

    /// Where a point of the plug-in's view shows in the window, whether a click there reaches
    /// the plug-in's view, and where in its coordinates. See [`AuWindow::probe_click`].
    pub fn probe_click(&self, x: f64, y: f64) -> Option<crate::plugin_window_layout::ClickProbe> {
        match self {
            Self::Au(w) => w.probe_click(x, y),
            Self::Vst3(w) => w.probe_click(x, y),
        }
    }
}
