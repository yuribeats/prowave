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
