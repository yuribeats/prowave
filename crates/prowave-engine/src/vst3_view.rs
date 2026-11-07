//! A VST 3 plug-in's window: its `IPlugView` attached to an `NSView` in a window of its own,
//! sized to the view. Like an Audio Unit's window (`au.rs`), the plug-in's view sits in a
//! container that fills the window, and the window follows the view's size: when the plug-in
//! asks (`IPlugFrame::resizeView`), and once a UI frame in [`Vst3Window::fit`], so a view
//! that grows after it is attached is not cropped. The green button fills the screen and
//! the corner drags (see `plugin_window_layout`): a view that can resize is given the size,
//! as it constrains it; a fixed-size view is scaled to it, keeping its proportions.
use crate::plugin_window_layout::WindowLayout;
use crate::vst3::Vst3Handle;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::{Cell, RefCell};
use vst3::Steinberg::Vst::{IEditControllerTrait, ViewType::kEditor};
use vst3::Steinberg::{
    kInvalidArgument, kPlatformTypeNSView, kResultFalse, kResultOk, tresult, IPlugFrame,
    IPlugFrameTrait, IPlugView, IPlugViewTrait, ViewRect,
};
use vst3::{Class, ComPtr, ComWrapper};

fn size_of_rect(r: &ViewRect) -> NSSize {
    NSSize::new(
        f64::from((r.right - r.left).max(1)),
        f64::from((r.bottom - r.top).max(1)),
    )
}

fn rect_of_size(s: NSSize) -> ViewRect {
    ViewRect {
        left: 0,
        top: 0,
        right: s.width.round() as i32,
        bottom: s.height.round() as i32,
    }
}

/// `IPlugFrame`: the plug-in asks for a new size; the window takes it, then the view.
struct PlugFrame {
    window: RefCell<Option<Retained<NSWindow>>>,
    layout: RefCell<Option<WindowLayout>>,
    /// The content size last given to the view.
    size: Cell<(f64, f64)>,
    busy: Cell<bool>,
}

impl Class for PlugFrame {
    type Interfaces = (IPlugFrame,);
}

impl IPlugFrameTrait for PlugFrame {
    unsafe fn resizeView(&self, view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
        let (Some(rect), false) = (new_size.as_mut(), view.is_null()) else {
            return kInvalidArgument;
        };
        if self.busy.get() {
            return kResultFalse;
        }
        self.busy.set(true);
        let size = size_of_rect(rect);
        // Outside full screen the window follows at 1:1; in it, the view is centred.
        let layout = self.layout.borrow().clone();
        match &layout {
            Some(l) if l.is_full() => l.view_took(size),
            Some(l) => l.view_resized(size),
            None => {
                if let Some(w) = self.window.borrow().as_ref() {
                    w.setContentSize(size);
                }
            }
        }
        self.size.set((size.width, size.height));
        // The view learns the size it now has (`view` is borrowed from the plug-in).
        if let Some(v) = ComPtr::<IPlugView>::from_raw(view) {
            let v = std::mem::ManuallyDrop::new(v);
            v.onSize(rect);
        }
        self.busy.set(false);
        kResultOk
    }
}

/// A plug-in's open window. Dropping it closes it (and detaches the view first).
pub struct Vst3Window {
    window: Retained<NSWindow>,
    _container: Retained<NSView>,
    nsview: Retained<NSView>,
    layout: WindowLayout,
    view: ComPtr<IPlugView>,
    frame: ComWrapper<PlugFrame>,
    can_resize: bool,
    _handle: Vst3Handle,
}

/// Opens `handle`'s editor in a window titled `title`. Main thread only.
pub(crate) fn open(handle: Vst3Handle, title: &str) -> Result<Vst3Window, String> {
    let mtm = MainThreadMarker::new().ok_or("plug-in windows open on the main thread")?;
    let controller = handle.controller().ok_or("this plug-in has no window")?;
    // SAFETY: UI-thread view calls in the order the VST 3 SDK documents: create, check the
    // platform, set the frame, attach to our NSView, then size.
    unsafe {
        let view = ComPtr::<IPlugView>::from_raw(controller.createView(kEditor))
            .ok_or("this plug-in has no window")?;
        if view.isPlatformTypeSupported(kPlatformTypeNSView) != kResultOk {
            return Err("the plug-in's window does not support macOS views".into());
        }
        let mut r = ViewRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        view.getSize(&mut r);
        let mut size = size_of_rect(&r);
        if size.width < 50.0 || size.height < 50.0 {
            size = NSSize::new(600.0, 400.0);
        }
        let can_resize = view.canResize() == kResultOk;
        let mut style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable;
        if can_resize {
            style |= NSWindowStyleMask::Resizable;
        }
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(200.0, 200.0), size),
            style,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setReleasedWhenClosed(false);
        let container = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), size),
        );
        window.setContentView(Some(&container));
        window.setTitle(&NSString::from_str(title));
        let frame = ComWrapper::new(PlugFrame {
            window: RefCell::new(Some(window.clone())),
            layout: RefCell::new(None),
            size: Cell::new((size.width, size.height)),
            busy: Cell::new(false),
        });
        if let Some(f) = frame.as_com_ref::<IPlugFrame>() {
            view.setFrame(f.as_ptr());
        }
        let parent: *const NSView = &*container;
        if view.attached(parent.cast_mut().cast(), kPlatformTypeNSView) != kResultOk {
            view.setFrame(std::ptr::null_mut());
            window.close();
            return Err("the plug-in's window did not attach".into());
        }
        // Some views know their real size only once attached.
        let mut r2 = r;
        let mut natural = size;
        if view.getSize(&mut r2) == kResultOk {
            let s2 = size_of_rect(&r2);
            if s2.width >= 50.0 && s2.height >= 50.0 {
                window.setContentSize(s2);
                frame.size.set((s2.width, s2.height));
                natural = s2;
            }
        }
        window.center();
        crate::au::float_above_main(&window);
        let layout = WindowLayout::new(&window, &container, natural, can_resize);
        frame.layout.replace(Some(layout.clone()));
        // The plug-in's own view (for checks): the one it put in our container.
        let nsview = container
            .subviews()
            .firstObject()
            .unwrap_or_else(|| container.clone());
        window.makeKeyAndOrderFront(None);
        let w = Vst3Window {
            window,
            _container: container,
            nsview,
            layout,
            view,
            frame,
            can_resize,
            _handle: handle,
        };
        w.fit();
        Ok(w)
    }
}

impl Vst3Window {
    /// Whether the user has not closed it.
    pub fn is_open(&self) -> bool {
        self.window.isVisible()
    }

    /// Keeps the window and the view in step. A window the user resized (or that entered or
    /// left full screen) gives a view that can resize its new size, as the view constrains
    /// it; otherwise the window follows the size the view reports, at 1:1 (a fixed-size view
    /// in a resized or full-screen window is scaled). Call it once a UI frame.
    pub fn fit(&self) {
        let l = &self.layout;
        l.tend();
        // SAFETY: UI-thread view calls with a rect that lives across them.
        unsafe {
            if self.can_resize {
                if let Some(c) = l.new_content() {
                    let mut r = rect_of_size(c);
                    self.view.checkSizeConstraint(&mut r);
                    let want = size_of_rect(&r);
                    self.frame.size.set((want.width, want.height));
                    self.view.onSize(&mut r);
                    l.view_took(want);
                    if !l.is_full()
                        && ((want.width - c.width).abs() > 0.5
                            || (want.height - c.height).abs() > 0.5)
                    {
                        l.set_content(want);
                    }
                    return;
                }
            }
            let mut r = rect_of_size(l.natural());
            if self.view.getSize(&mut r) == kResultOk {
                let want = size_of_rect(&r);
                if want.width >= 2.0 && want.height >= 2.0 {
                    self.frame.size.set((want.width, want.height));
                    l.view_resized(want);
                }
            }
        }
    }

    /// Whether it fills the screen (the green button).
    pub fn is_full_screen(&self) -> bool {
        self.layout.is_full()
    }

    /// The green button: into or out of full screen.
    pub fn toggle_full_screen(&self) {
        self.window.toggleFullScreen(None);
    }

    /// The plug-in view's size (its own, unscaled) and the window's content size, in points.
    pub fn sizes(&self) -> ((f64, f64), (f64, f64)) {
        crate::plugin_window_layout::sizes(&self.window, self.layout.natural())
    }

    /// See [`crate::au::AuWindow::probe_click`].
    pub fn probe_click(&self, x: f64, y: f64) -> Option<crate::plugin_window_layout::ClickProbe> {
        crate::plugin_window_layout::probe_click(&self.window, &self.nsview, x, y)
    }

    /// Whether the plug-in's view takes sizes of its own (`IPlugView::canResize`).
    pub fn can_resize(&self) -> bool {
        self.can_resize
    }

    /// The window's number (what `screencapture -l` takes).
    pub fn window_number(&self) -> isize {
        self.window.windowNumber()
    }

    /// Brings it to the front.
    pub fn show(&self) {
        self.window.makeKeyAndOrderFront(None);
    }
}

/// For a program without an event loop (the scanner's window check): makes sure there is
/// an application, then runs the main run loop for `seconds` so windows draw. Main thread.
pub fn pump_main_loop(seconds: f64) {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::{NSDate, NSRunLoop};
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let until = NSDate::dateWithTimeIntervalSinceNow(seconds);
    NSRunLoop::currentRunLoop().runUntilDate(&until);
}

impl Drop for Vst3Window {
    fn drop(&mut self) {
        // SAFETY: the SDK's order: detach the view, drop its frame, then close the window.
        unsafe {
            self.view.removed();
            self.view.setFrame(std::ptr::null_mut());
        }
        self.frame.window.borrow_mut().take();
        self.frame.layout.borrow_mut().take();
        self.window.close();
    }
}
