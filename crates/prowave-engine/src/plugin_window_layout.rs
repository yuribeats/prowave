//! What every plug-in window (Audio Unit or VST 3) shares: the green button (full screen),
//! a corner that drags, and the plug-in's view laid out in whatever size the window has.
//!
//! The plug-in's view keeps its own size (its "natural" size) inside a container that fills
//! the window. When the window is larger or smaller than that, the container's bounds are
//! scaled (`setBounds`) so the view draws scaled and centred, with the window's black
//! background in the margins; clicks go through the same bounds transform, so they land on
//! the control drawn under the pointer. A view that takes sizes of its own (a VST 3 whose
//! view can resize) is given the window's size instead and is never scaled up, only centred
//! (and scaled down if it cannot shrink enough). Audio Unit views are always scaled.
//!
//! Plug-in windows float above the main window; a floating window and full screen do not
//! mix, so entering full screen drops it to the normal level (and stops it hiding while
//! another app is in front), and leaving restores both and the window's exact frame.
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{
    NSColor, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowDidEnterFullScreenNotification,
    NSWindowDidExitFullScreenNotification, NSWindowDidResizeNotification, NSWindowStyleMask,
    NSWindowWillEnterFullScreenNotification,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSObjectProtocol, NSPoint, NSRect,
    NSSize,
};
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;

/// Sizes closer than this (points) are the same.
const SAME: f64 = 0.5;

fn differ(a: NSSize, b: NSSize) -> bool {
    (a.width - b.width).abs() > SAME || (a.height - b.height).abs() > SAME
}

/// The container's bounds that show a `natural`-sized view at the origin, scaled uniformly
/// to fit `content` and centred. `grow` lets the view be drawn larger than 1:1.
pub(crate) fn letterbox(content: NSSize, natural: NSSize, grow: bool) -> NSRect {
    let (nw, nh) = (natural.width.max(1.0), natural.height.max(1.0));
    let (cw, ch) = (content.width.max(1.0), content.height.max(1.0));
    let mut s = (cw / nw).min(ch / nh);
    if !grow {
        s = s.min(1.0);
    }
    if (s - 1.0).abs() < 1e-3 {
        s = 1.0;
    }
    let (bw, bh) = (cw / s, ch / s);
    let mut x = -(bw - nw) / 2.0;
    let mut y = -(bh - nh) / 2.0;
    if s == 1.0 {
        // Whole points: a 1:1 view stays pixel-aligned.
        x = x.round();
        y = y.round();
    }
    NSRect::new(NSPoint::new(x, y), NSSize::new(bw, bh))
}

struct State {
    window: Retained<NSWindow>,
    container: Retained<NSView>,
    /// The plug-in view's size, in the container's bounds units.
    natural: Cell<NSSize>,
    /// The view takes sizes of its own: given the window's size, never scaled up.
    native: bool,
    full: Cell<bool>,
    /// Before full screen: the window's frame, level and hiding.
    saved: Cell<Option<(NSRect, isize, bool)>>,
    /// The content size the view was last laid out for (or given, for a native view).
    given: Cell<NSSize>,
    /// When full screen was asked for: if the window never got there (AppKit refuses while
    /// another space animation runs), the floating level comes back.
    asked: Cell<Option<std::time::Instant>>,
    /// The frame to put back once more after leaving full screen (AppKit's own restore can
    /// land a point off).
    restore: Cell<Option<NSRect>>,
    observers: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
}

impl State {
    fn content(&self) -> NSSize {
        self.container.frame().size
    }

    fn relayout(&self) {
        let want = letterbox(self.content(), self.natural.get(), !self.native);
        let have = self.container.bounds();
        if (have.origin.x - want.origin.x).abs() > 1e-3
            || (have.origin.y - want.origin.y).abs() > 1e-3
            || (have.size.width - want.size.width).abs() > 1e-3
            || (have.size.height - want.size.height).abs() > 1e-3
        {
            self.container.setBounds(want);
            self.container.setNeedsDisplay(true);
        }
    }

    fn will_enter(&self) {
        let w = &self.window;
        self.saved
            .set(Some((w.frame(), w.level(), w.hidesOnDeactivate())));
        // NSNormalWindowLevel: a floating window cannot have a space of its own.
        w.setLevel(0);
        w.setHidesOnDeactivate(false);
        // Free proportions: the screen's shape, not the plug-in's.
        w.setContentResizeIncrements(NSSize::new(1.0, 1.0));
        self.full.set(true);
        self.asked.set(Some(std::time::Instant::now()));
    }

    fn did_enter(&self) {
        self.asked.set(None);
        self.relayout();
    }

    fn did_exit(&self) {
        self.full.set(false);
        self.asked.set(None);
        let w = &self.window;
        if let Some((frame, level, hides)) = self.saved.take() {
            w.setLevel(level);
            w.setHidesOnDeactivate(hides);
            w.setFrame_display(frame, true);
            self.restore.set(Some(frame));
        }
        if !self.native {
            w.setContentAspectRatio(self.natural.get());
        }
        self.relayout();
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        for o in self.observers.borrow_mut().drain(..) {
            // SAFETY: an observer token this center returned.
            unsafe {
                center.removeObserver(o.as_ref());
            }
        }
    }
}

/// A plug-in window's layout. Cheap to clone; main thread only.
#[derive(Clone)]
pub(crate) struct WindowLayout(Rc<State>);

impl WindowLayout {
    /// Makes `window` (whose content view is `container`, holding a view of `natural` size)
    /// resizable and full-screen capable. `native`: the view takes sizes of its own.
    pub(crate) fn new(
        window: &Retained<NSWindow>,
        container: &Retained<NSView>,
        natural: NSSize,
        native: bool,
    ) -> Self {
        window.setStyleMask(window.styleMask() | NSWindowStyleMask::Resizable);
        window.setCollectionBehavior(
            window.collectionBehavior() | NSWindowCollectionBehavior::FullScreenPrimary,
        );
        window.setBackgroundColor(Some(&NSColor::blackColor()));
        if !native {
            window.setContentAspectRatio(natural);
        }
        // The plug-in's view is sized by us, never dragged along by the container.
        container.setAutoresizesSubviews(false);
        // A view larger than the screen opens in a window that fits it (scaled down, or a
        // native view asked to take that size), not one hanging off the screen.
        let mut given = container.frame().size;
        if let Some(screen) = window.screen() {
            let room = window.contentRectForFrameRect(screen.visibleFrame()).size;
            let have = container.frame().size;
            let k = (room.width / have.width).min(room.height / have.height);
            if k < 1.0 {
                if !native {
                    window.setContentAspectRatio(natural);
                }
                window.setContentSize(NSSize::new(
                    (have.width * k).floor(),
                    (have.height * k).floor(),
                ));
                window.center();
                // A native view is given the new size at the next fit.
                given = if native { have } else { container.frame().size };
            }
        }
        let state = Rc::new(State {
            window: window.clone(),
            container: container.clone(),
            natural: Cell::new(natural),
            native,
            full: Cell::new(false),
            saved: Cell::new(None),
            given: Cell::new(given),
            asked: Cell::new(None),
            restore: Cell::new(None),
            observers: RefCell::new(Vec::new()),
        });
        let weak = Rc::downgrade(&state);
        let observe = |name: &NSNotificationName, f: fn(&State)| {
            let weak = weak.clone();
            let block = block2::RcBlock::new(move |_n: NonNull<NSNotification>| {
                if let Some(s) = weak.upgrade() {
                    f(&s);
                }
            });
            let obj: &AnyObject = window.as_ref();
            // SAFETY: no queue: the block runs on the thread that posts, the main thread
            // (window notifications); it holds only a weak reference to the state.
            unsafe {
                NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                    Some(name),
                    Some(obj),
                    None,
                    &block,
                )
            }
        };
        // SAFETY: AppKit's notification name constants.
        let tokens = unsafe {
            vec![
                observe(NSWindowWillEnterFullScreenNotification, State::will_enter),
                observe(NSWindowDidEnterFullScreenNotification, State::did_enter),
                observe(NSWindowDidExitFullScreenNotification, State::did_exit),
                observe(NSWindowDidResizeNotification, State::relayout),
            ]
        };
        *state.observers.borrow_mut() = tokens;
        state.relayout();
        Self(state)
    }

    /// Once a UI frame, before the rest: recovers from full screen that never began, and
    /// puts the window's frame back exactly after leaving it.
    pub(crate) fn tend(&self) {
        let s = &self.0;
        if let Some(t) = s.asked.get() {
            let there = s.window.styleMask().contains(NSWindowStyleMask::FullScreen);
            if there {
                s.asked.set(None);
            } else if t.elapsed() > std::time::Duration::from_secs(3) {
                s.did_exit();
            }
        }
        if let Some(frame) = s.restore.take() {
            let f = s.window.frame();
            if f.origin != frame.origin || f.size != frame.size {
                s.window.setFrame_display(frame, true);
            }
        }
    }

    /// In full screen (or going there).
    pub(crate) fn is_full(&self) -> bool {
        self.0.full.get()
    }

    /// The plug-in view's size as last told.
    pub(crate) fn natural(&self) -> NSSize {
        self.0.natural.get()
    }

    /// The plug-in changed its own size: outside full screen the window follows at 1:1.
    pub(crate) fn view_resized(&self, size: NSSize) {
        let s = &self.0;
        if !differ(size, s.natural.get()) {
            return;
        }
        s.natural.set(size);
        if !s.full.get() {
            if !s.native {
                s.window.setContentAspectRatio(size);
            }
            s.window.setContentSize(size);
            s.given.set(s.content());
        }
        s.relayout();
    }

    /// We gave the view `size` (it took what it could): lay it out, the window unchanged.
    pub(crate) fn view_took(&self, size: NSSize) {
        self.0.natural.set(size);
        self.0.relayout();
    }

    /// The window's content size, when it changed since the last call (the user dragged the
    /// corner, or full screen began or ended): a native view should be given it.
    pub(crate) fn new_content(&self) -> Option<NSSize> {
        let c = self.0.content();
        if differ(c, self.0.given.get()) {
            self.0.given.set(c);
            Some(c)
        } else {
            None
        }
    }

    /// Sets the window to `size` (a native view that could not take the size dragged to).
    pub(crate) fn set_content(&self, size: NSSize) {
        let s = &self.0;
        if !s.full.get() {
            s.window.setContentSize(size);
            s.given.set(s.content());
        }
        s.relayout();
    }
}

/// A check of where a click on a plug-in's view lands (see `probe_click`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClickProbe {
    /// Where the view's point shows, in window coordinates.
    pub shown: (f64, f64),
    /// A click there reaches the plug-in's view (or one of its subviews).
    pub hits_plugin: bool,
    /// The point that click is at in the view's coordinates (from its bottom left).
    pub arrives: (f64, f64),
}

/// The plug-in view's size and the window's content size, in points (for checks).
pub(crate) fn sizes(window: &NSWindow, natural: NSSize) -> ((f64, f64), (f64, f64)) {
    let c = window
        .contentView()
        .map_or(NSSize::new(0.0, 0.0), |v| v.frame().size);
    ((natural.width, natural.height), (c.width, c.height))
}

/// Where point (`x`, `y`) of `view` (its coordinates) shows in `window`; whether a click
/// there reaches `view` or one of its subviews; and the point that click is at in `view`'s
/// coordinates (the round trip through the container's bounds).
pub(crate) fn probe_click(window: &NSWindow, view: &NSView, x: f64, y: f64) -> Option<ClickProbe> {
    let content = window.contentView()?;
    // From the bottom left, whichever way the view counts.
    let h = view.bounds().size.height;
    let flip = |y: f64| if view.isFlipped() { h - y } else { y };
    let at = view.convertPoint_toView(NSPoint::new(x, flip(y)), None);
    // hitTest takes a point in the superview's coordinates: the window's, for a content view.
    // SAFETY: the content view's superview is the window's frame view, alive with it.
    let frame_view = unsafe { content.superview() };
    let base = frame_view.map_or(at, |f| f.convertPoint_fromView(at, None));
    let hit = content
        .hitTest(base)
        .is_some_and(|h| h.isDescendantOf(view));
    let back = view.convertPoint_fromView(at, None);
    Some(ClickProbe {
        shown: (at.x, at.y),
        hits_plugin: hit,
        arrives: (back.x, flip(back.y)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox_fits_and_centres() {
        let n = NSSize::new(800.0, 500.0);
        // 1:1 is the identity.
        let b = letterbox(n, n, true);
        assert_eq!((b.origin.x, b.origin.y), (0.0, 0.0));
        assert_eq!((b.size.width, b.size.height), (800.0, 500.0));
        // A wide screen: scaled by height, centred left to right.
        let c = NSSize::new(1600.0, 900.0);
        let b = letterbox(c, n, true);
        let s = 900.0 / 500.0;
        assert!((b.size.height - 500.0).abs() < 1e-9);
        assert!((b.size.width - 1600.0 / s).abs() < 1e-9);
        assert!((b.origin.x + (1600.0 / s - 800.0) / 2.0).abs() < 1e-9);
        assert!(b.origin.y.abs() < 1e-9);
        // A view that may not grow is centred at 1:1.
        let b = letterbox(c, n, false);
        assert_eq!((b.size.width, b.size.height), (1600.0, 900.0));
        assert_eq!((b.origin.x, b.origin.y), (-400.0, -200.0));
        // Smaller than the view: scaled down even if it may not grow.
        let b = letterbox(NSSize::new(400.0, 250.0), n, false);
        assert!((b.size.width - 800.0).abs() < 1e-9);
    }
}
