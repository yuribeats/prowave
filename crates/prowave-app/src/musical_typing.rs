//! Musical Typing (Logic: Window > Show Musical Typing, Caps Lock): the computer keyboard
//! plays the selected instrument like a MIDI keyboard, and records like one.
//!
//! A W S E D F T G Y H U J K O L P ; ' are the keys of an octave and a half from C (the
//! black keys on the row above); Z / X move an octave down / up, C / V lower / raise the
//! velocity. While it is open those keys play notes and do nothing else.
use crate::i18n::tr;
use crate::recorder::LiveNote;
use egui::Key;

/// The keys, in order of the semitones they play from the octave's C.
const KEYS: [(Key, u8); 18] = [
    (Key::A, 0),
    (Key::W, 1),
    (Key::S, 2),
    (Key::E, 3),
    (Key::D, 4),
    (Key::F, 5),
    (Key::T, 6),
    (Key::G, 7),
    (Key::Y, 8),
    (Key::H, 9),
    (Key::U, 10),
    (Key::J, 11),
    (Key::K, 12),
    (Key::O, 13),
    (Key::L, 14),
    (Key::P, 15),
    (Key::Semicolon, 16),
    (Key::Quote, 17),
];

pub struct Typing {
    pub open: bool,
    /// macOS: catches the keys app-wide, so they still play while a plug-in window has the
    /// keyboard. Installed on the first frame.
    #[cfg(target_os = "macos")]
    monitor: Option<app_keys::Monitor>,
    /// The C the A key plays (MIDI note).
    base: u8,
    vel: u8,
    /// Notes held down, by key.
    held: Vec<(Key, u8)>,
    notes: Vec<LiveNote>,
}

impl Default for Typing {
    fn default() -> Self {
        Self {
            open: false,
            #[cfg(target_os = "macos")]
            monitor: None,
            base: 60,
            vel: 100,
            held: Vec::new(),
            notes: Vec::new(),
        }
    }
}

impl Typing {
    /// The notes played since the last call.
    pub fn take(&mut self) -> Vec<LiveNote> {
        std::mem::take(&mut self.notes)
    }

    fn release_all(&mut self) {
        for (_, pitch) in self.held.drain(..) {
            self.notes.push(LiveNote::Off { pitch });
        }
    }

    fn set_open(&mut self, open: bool) {
        if self.open != open {
            self.open = open;
            if !open {
                self.release_all();
            }
        }
    }
}

/// Whether Caps Lock is on, read from the system. On macOS the window library never passes
/// the Caps Lock key on (it is a modifier it does not track), so its state is asked for here.
#[cfg(target_os = "macos")]
fn caps_lock_on() -> Option<bool> {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceFlagsState(state_id: i32) -> u64;
    }
    // kCGEventSourceStateCombinedSessionState, kCGEventFlagMaskAlphaShift.
    const COMBINED_SESSION: i32 = 0;
    const ALPHA_SHIFT: u64 = 0x0001_0000;
    // SAFETY: a plain query of the keyboard flags; no pointers.
    Some(unsafe { CGEventSourceFlagsState(COMBINED_SESSION) } & ALPHA_SHIFT != 0)
}

#[cfg(not(target_os = "macos"))]
fn caps_lock_on() -> Option<bool> {
    None
}

/// Once per frame, before anything else reads the keyboard: Caps Lock opens and closes it;
/// while open, its keys become notes and are taken out of the input.
pub fn poll(ctx: &egui::Context, t: &mut Typing) {
    if let Some(on) = caps_lock_on() {
        t.set_open(on);
        // Nothing wakes the window when only Caps Lock changes, so look again shortly.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    #[cfg(target_os = "macos")]
    {
        let m = t
            .monitor
            .get_or_insert_with(|| app_keys::Monitor::install(ctx.clone()));
        m.set_open(t.open);
        for (key, pressed, repeat) in m.take() {
            handle(t, key, pressed, repeat);
        }
    }
    poll_events(ctx, t);
}

/// Space was pressed in a plug-in window since the last call (the transport should toggle).
pub fn take_plugin_space() -> bool {
    #[cfg(target_os = "macos")]
    {
        app_keys::Monitor::take_space()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// macOS: an app-wide key monitor. While Musical Typing is open, its keys are taken from
/// every window of the app, the plug-in windows too (they live in this process), so the
/// keyboard keeps playing whichever window is in front.
#[cfg(target_os = "macos")]
mod app_keys {
    use egui::Key;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    type Queue = Arc<Mutex<Vec<(Key, bool, bool)>>>;

    const SPACE: u16 = 49;
    /// Space was pressed in a plug-in window: the transport should start or stop.
    pub static SPACE_PLAY: AtomicBool = AtomicBool::new(false);

    /// Whether the window with the keyboard is a plug-in's (not one of Pro Wave's own, which
    /// are drawn by winit's view) and its focus is not on a text field.
    fn plugin_window_has_keys() -> bool {
        use objc2::runtime::{AnyClass, AnyObject};
        use objc2::{class, msg_send};
        // SAFETY: plain AppKit queries on the main thread (monitors run there).
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let window: *mut AnyObject = msg_send![app, keyWindow];
            if window.is_null() {
                return false;
            }
            let responder: *mut AnyObject = msg_send![window, firstResponder];
            if responder.is_null() {
                return true;
            }
            let name = (*responder).class().name().to_string_lossy().into_owned();
            if name.contains("Winit") {
                return false;
            }
            let text: &AnyClass = class!(NSText);
            let is_text: bool = msg_send![responder, isKindOfClass: text];
            !is_text
        }
    }

    pub struct Monitor {
        open: Arc<AtomicBool>,
        queue: Queue,
        // Kept so the monitor stays installed for the life of the app.
        _token: Option<objc2::rc::Retained<objc2::runtime::AnyObject>>,
    }

    /// The Musical Typing key for a macOS virtual key code (the physical key, whatever the
    /// keyboard layout), as the egui keys in `KEYS` and Z X C V.
    pub fn key_for(code: u16) -> Option<Key> {
        Some(match code {
            0 => Key::A,
            1 => Key::S,
            2 => Key::D,
            3 => Key::F,
            4 => Key::H,
            5 => Key::G,
            6 => Key::Z,
            7 => Key::X,
            8 => Key::C,
            9 => Key::V,
            13 => Key::W,
            14 => Key::E,
            16 => Key::Y,
            17 => Key::T,
            31 => Key::O,
            32 => Key::U,
            35 => Key::P,
            37 => Key::L,
            38 => Key::J,
            39 => Key::Quote,
            40 => Key::K,
            41 => Key::Semicolon,
            _ => return None,
        })
    }

    impl Monitor {
        pub fn install(ctx: egui::Context) -> Self {
            let open = Arc::new(AtomicBool::new(false));
            let queue: Queue = Arc::default();
            let (o, q) = (open.clone(), queue.clone());
            let space_ctx = ctx.clone();
            let block = block2::RcBlock::new(move |ev: NonNull<NSEvent>| -> *mut NSEvent {
                // SAFETY: AppKit hands the monitor a live event for the length of the call.
                let e = unsafe { ev.as_ref() };
                // Space in a plug-in window starts and stops the transport, as in Logic, without
                // bringing the main window forward. Pro Wave's own windows handle Space
                // themselves, and a plug-in's text field keeps its spaces.
                if e.keyCode() == SPACE
                    && !e.modifierFlags().intersects(
                        NSEventModifierFlags::Command
                            | NSEventModifierFlags::Control
                            | NSEventModifierFlags::Option,
                    )
                    && plugin_window_has_keys()
                {
                    if e.r#type() == NSEventType::KeyDown && !e.isARepeat() {
                        SPACE_PLAY.store(true, Ordering::Relaxed);
                        space_ctx.request_repaint();
                    }
                    return std::ptr::null_mut();
                }
                if !o.load(Ordering::Relaxed) {
                    return ev.as_ptr();
                }
                let held = NSEventModifierFlags::Command
                    | NSEventModifierFlags::Control
                    | NSEventModifierFlags::Option;
                let (kind, flags, code) = (e.r#type(), e.modifierFlags(), e.keyCode());
                if flags.intersects(held) {
                    return ev.as_ptr();
                }
                let Some(key) = key_for(code) else {
                    return ev.as_ptr();
                };
                let pressed = kind == NSEventType::KeyDown;
                let repeat = pressed && e.isARepeat();
                if let Ok(mut v) = q.lock() {
                    v.push((key, pressed, repeat));
                }
                ctx.request_repaint();
                // Swallowed: the key plays a note and does nothing else, in any window.
                std::ptr::null_mut()
            });
            // SAFETY: the block lives as long as the monitor (AppKit copies it).
            let token = unsafe {
                NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                    NSEventMask::KeyDown | NSEventMask::KeyUp,
                    &block,
                )
            };
            Self {
                open,
                queue,
                _token: token,
            }
        }

        /// Space came from a plug-in window since the last call.
        pub fn take_space() -> bool {
            SPACE_PLAY.swap(false, Ordering::Relaxed)
        }

        pub fn set_open(&self, open: bool) {
            self.open.store(open, Ordering::Relaxed);
        }

        pub fn take(&self) -> Vec<(Key, bool, bool)> {
            self.queue
                .lock()
                .map(|mut v| std::mem::take(&mut *v))
                .unwrap_or_default()
        }
    }
}

/// One key going down or up while Musical Typing is open. False when it is not one of its keys.
fn handle(t: &mut Typing, key: Key, pressed: bool, repeat: bool) -> bool {
    match key {
        Key::Z if pressed && !repeat => {
            t.release_all();
            t.base = t.base.saturating_sub(12).max(12);
        }
        Key::X if pressed && !repeat => {
            t.release_all();
            t.base = (t.base + 12).min(108);
        }
        Key::C if pressed => t.vel = t.vel.saturating_sub(10).max(1),
        Key::V if pressed => t.vel = (t.vel + 10).min(127),
        Key::Z | Key::X | Key::C | Key::V => {}
        _ => {
            let Some(&(_, semi)) = KEYS.iter().find(|(k, _)| *k == key) else {
                return false;
            };
            let pitch = (t.base + semi).min(127);
            if pressed && !repeat && !t.held.iter().any(|(k, _)| *k == key) {
                t.held.push((key, pitch));
                t.notes.push(LiveNote::On { pitch, vel: t.vel });
            } else if !pressed {
                if let Some(i) = t.held.iter().position(|(k, _)| *k == key) {
                    let (_, p) = t.held.remove(i);
                    t.notes.push(LiveNote::Off { pitch: p });
                }
            }
        }
    }
    true
}

fn poll_events(ctx: &egui::Context, t: &mut Typing) {
    ctx.input_mut(|i| {
        let mut keep = Vec::with_capacity(i.events.len());
        for ev in std::mem::take(&mut i.events) {
            let egui::Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } = &ev
            else {
                keep.push(ev);
                continue;
            };
            if *key == Key::CapsLock {
                t.set_open(*pressed);
                continue;
            }
            if !t.open || modifiers.command || modifiers.ctrl || modifiers.alt {
                keep.push(ev);
                continue;
            }
            if !handle(t, *key, *pressed, *repeat) {
                keep.push(ev);
            }
        }
        // The letters also arrive as typed text: drop those while playing.
        if t.open {
            keep.retain(|e| !matches!(e, egui::Event::Text(_)));
        }
        i.events = keep;
    });
}

/// The small window that shows it is on, the octave and the velocity.
pub fn show(ctx: &egui::Context, t: &mut Typing) {
    if !t.open {
        return;
    }
    let mut open = true;
    egui::Window::new(tr("Musical Typing"))
        .id(egui::Id::unique("musical_typing"))
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            const NAMES: [&str; 12] = [
                "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
            ];
            let held: Vec<u8> = t.held.iter().map(|(_, p)| *p).collect();
            ui.horizontal(|ui| {
                for (key, semi) in KEYS {
                    let pitch = t.base + semi;
                    let black = matches!(semi % 12, 1 | 3 | 6 | 8 | 10);
                    let on = held.contains(&pitch);
                    let fill = match (on, black) {
                        (true, _) => egui::Color32::from_rgb(0xf0, 0xc8, 0x40),
                        (false, true) => egui::Color32::from_gray(0x30),
                        (false, false) => egui::Color32::from_gray(0xe8),
                    };
                    let text = if black && !on {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::BLACK
                    };
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(26.0, 46.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 3.0, fill);
                    ui.painter().text(
                        r.center_bottom() - egui::vec2(0.0, 8.0),
                        egui::Align2::CENTER_CENTER,
                        key.name(),
                        egui::FontId::proportional(11.0),
                        text,
                    );
                    ui.painter().text(
                        r.center_top() + egui::vec2(0.0, 9.0),
                        egui::Align2::CENTER_CENTER,
                        NAMES[usize::from(pitch % 12)],
                        egui::FontId::proportional(9.0),
                        text,
                    );
                }
            });
            ui.label(format!(
                "C{}  ·  {} {}  ·  Z / X {}  ·  C / V {}",
                i32::from(t.base / 12) - 2,
                tr("Velocity"),
                t.vel,
                tr("Octave"),
                tr("Velocity"),
            ));
        });
    if !open {
        t.set_open(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn caps_lock_turns_the_keys_into_notes_and_hides_them_from_the_rest() {
        let ctx = egui::Context::default();
        let mut t = Typing::default();
        let input = egui::RawInput {
            events: vec![
                key(Key::CapsLock, true),
                key(Key::A, true),
                key(Key::R, true),
            ],
            ..Default::default()
        };
        let mut left = Vec::new();
        let mut out = ctx.run_ui(input, |ui| {
            poll_events(ui.ctx(), &mut t);
            left = ui.ctx().input(|i| i.events.clone());
        });
        out.textures_delta.clear();
        assert!(t.open);
        assert_eq!(
            t.take(),
            vec![LiveNote::On {
                pitch: 60,
                vel: 100
            }]
        );
        // R is not a Musical Typing key: it still reaches the rest of the program.
        assert_eq!(left.len(), 1);
        let input = egui::RawInput {
            events: vec![key(Key::CapsLock, false)],
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| poll_events(ui.ctx(), &mut t));
        out.textures_delta.clear();
        assert!(!t.open);
        assert_eq!(
            t.take(),
            vec![LiveNote::Off { pitch: 60 }],
            "held notes are let go"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn every_typing_key_has_a_mac_key_code() {
        let mut want: Vec<Key> = KEYS.iter().map(|(k, _)| *k).collect();
        want.extend([Key::Z, Key::X, Key::C, Key::V]);
        let mapped: Vec<Key> = (0..128u16).filter_map(app_keys::key_for).collect();
        for k in want {
            assert!(mapped.contains(&k), "{k:?} has no key code");
        }
        assert_eq!(mapped.len(), 22, "no other key is taken");
    }

    #[test]
    fn keys_from_another_window_play_like_the_main_one() {
        let mut t = Typing::default();
        t.set_open(true);
        assert!(handle(&mut t, Key::A, true, false));
        assert!(
            handle(&mut t, Key::A, true, true),
            "a repeat is still ours, and adds nothing"
        );
        assert!(handle(&mut t, Key::A, false, false));
        assert!(
            !handle(&mut t, Key::R, true, false),
            "R is not a Musical Typing key"
        );
        assert_eq!(
            t.take(),
            vec![
                LiveNote::On {
                    pitch: 60,
                    vel: 100
                },
                LiveNote::Off { pitch: 60 }
            ]
        );
    }
}
