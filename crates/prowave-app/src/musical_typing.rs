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
    /// The key of the keybed held down with the mouse.
    mouse: Option<Key>,
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
            mouse: None,
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

/// The note names, from C.
const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// A pitch as its name and octave ("C3", "F#4").
fn note_name(pitch: u8) -> String {
    format!(
        "{}{}",
        NAMES[usize::from(pitch % 12)],
        i32::from(pitch / 12) - 2
    )
}

/// The letter printed on a key cap for an egui key.
fn cap_letter(key: Key) -> &'static str {
    match key {
        Key::Semicolon => ";",
        Key::Quote => "'",
        other => other.name(),
    }
}

/// Size of the window's keys.
const WHITE: egui::Vec2 = egui::vec2(40.0, 104.0);
const BLACK: egui::Vec2 = egui::vec2(26.0, 62.0);
const WHITE_GAP: f32 = 4.0;

/// The keybed's keys and where they sit inside `bed`: (key, semitone, black, rect). Black keys
/// come last, so they are drawn over the white ones and found first under the pointer.
fn keybed(bed: egui::Rect) -> Vec<(Key, u8, bool, egui::Rect)> {
    let mut whites = Vec::new();
    let mut blacks = Vec::new();
    let mut x = bed.left();
    for (key, semi) in KEYS {
        if matches!(semi % 12, 1 | 3 | 6 | 8 | 10) {
            // Between the white key just placed and the next.
            let cx = x - WHITE_GAP / 2.0;
            let r = egui::Rect::from_min_size(egui::pos2(cx - BLACK.x / 2.0, bed.top()), BLACK);
            blacks.push((key, semi, true, r));
        } else {
            let r = egui::Rect::from_min_size(egui::pos2(x, bed.top()), WHITE);
            whites.push((key, semi, false, r));
            x += WHITE.x + WHITE_GAP;
        }
    }
    whites.extend(blacks);
    whites
}

/// Width of the keybed.
fn keybed_width() -> f32 {
    let whites = KEYS
        .iter()
        .filter(|(_, s)| !matches!(s % 12, 1 | 3 | 6 | 8 | 10))
        .count() as f32;
    whites * WHITE.x + (whites - 1.0) * WHITE_GAP
}

/// The small window: an LCD with the octave, the velocity and the notes held, the keybed (the
/// computer keys printed on the caps; the mouse plays it too) and the octave and velocity keys.
pub fn show(ctx: &egui::Context, t: &mut Typing) {
    use crate::hw::{self, Cap};
    use crate::midi_monitor::hw_utilities as hu;
    use crate::theme::{self, Face};
    use egui::{pos2, vec2, Align2, Rect, Sense};
    if !t.open {
        return;
    }
    let width = keybed_width() + 16.0;
    let id = egui::Id::unique("musical_typing");
    let outer = width + 2.0 * hu::PAD;
    let (open, _) = hu::window(ctx, id, tr("Musical Typing"), "", outer, None, |ui| {
        // The LCD: octave | velocity | notes held.
        let (lcd_r, _) = ui.allocate_exact_size(vec2(width, 66.0), Sense::hover());
        let g = ui.painter().clone();
        let glass = hw::lcd(&g, lcd_r);
        let z1 = glass.left() + 112.0;
        let z2 = z1 + 168.0;
        for x in [z1, z2] {
            hu::lcd_divider(&g, x, glass.top() + 6.0..=glass.bottom() - 6.0);
        }
        hu::lcd_label(&g, glass.left_top() + vec2(9.0, 6.0), tr("Octave"));
        let oct = format!("C{}", i32::from(t.base / 12) - 2);
        hw::seg(
            &g,
            pos2(glass.left() + 12.0, glass.top() + 21.0),
            &oct,
            26.0,
            theme::LCD_INK,
            true,
        );
        hu::lcd_label(&g, pos2(z1 + 9.0, glass.top() + 6.0), tr("Velocity"));
        let vel = format!("{:>3}", t.vel);
        hw::seg(
            &g,
            pos2(z1 + 12.0, glass.top() + 21.0),
            &vel,
            26.0,
            theme::LCD_INK,
            true,
        );
        // A ladder of the velocity: ten bars rising.
        for i in 0..10 {
            let h = 4.0 + i as f32 * 2.4;
            let x = z1 + 84.0 + i as f32 * 7.0;
            let r = Rect::from_min_max(
                pos2(x, glass.bottom() - 9.0 - h),
                pos2(x + 4.5, glass.bottom() - 9.0),
            );
            let lit = f32::from(t.vel) / 127.0 * 10.0 > i as f32 + 0.25;
            g.rect_filled(
                r,
                0.0,
                if lit {
                    theme::LCD_INK
                } else {
                    theme::LCD_GHOST
                },
            );
        }
        hu::lcd_label(&g, pos2(z2 + 9.0, glass.top() + 6.0), tr("Notes"));
        let mut held: Vec<u8> = t.held.iter().map(|(_, p)| *p).collect();
        held.sort_unstable();
        let names = held
            .iter()
            .map(|p| note_name(*p))
            .collect::<Vec<_>>()
            .join(" ");
        let room = glass.right() - z2 - 18.0;
        let shown = hw::fit(&g, &names, Face::Lcd, 15.0, room);
        hw::text(
            &g,
            pos2(z2 + 10.0, glass.top() + 36.0),
            Align2::LEFT_CENTER,
            &shown,
            Face::Lcd,
            15.0,
            theme::LCD_INK,
        );

        // The keybed in its dark slot.
        ui.add_space(10.0);
        let (slot, resp) =
            ui.allocate_exact_size(vec2(width, WHITE.y + 14.0), Sense::click_and_drag());
        g.rect_filled(slot.translate(vec2(0.0, 1.0)), 4.0, theme::BONE_HI);
        hw::gradient(
            &g,
            slot,
            4.0,
            theme::METER_WELL,
            egui::Color32::from_rgb(0x0f, 0x0f, 0x0e),
        );
        hw::inner_shadow(&g, slot, 4.0, 0.6);
        let bed = Rect::from_min_size(
            pos2(
                slot.left() + (slot.width() - keybed_width()) / 2.0,
                slot.top() + 5.0,
            ),
            vec2(keybed_width(), WHITE.y),
        );
        let keys = keybed(bed);
        // The mouse plays the key under it; dragging slides from key to key.
        let under = resp
            .interact_pointer_pos()
            .filter(|_| resp.is_pointer_button_down_on())
            .and_then(|p| {
                keys.iter()
                    .rev()
                    .find(|(_, _, _, r)| r.contains(p))
                    .map(|(k, _, _, _)| *k)
            });
        if under != t.mouse {
            if let Some(k) = t.mouse.take() {
                handle(t, k, false, false);
            }
            if let Some(k) = under {
                handle(t, k, true, false);
                t.mouse = Some(k);
            }
        }
        let held: Vec<Key> = t.held.iter().map(|(k, _)| *k).collect();
        for (key, semi, black, r) in &keys {
            let pitch = (t.base + semi).min(127);
            let on = held.contains(key);
            let cap = match (on, black) {
                (true, _) => Cap::Orange,
                (false, true) => Cap::Dark,
                (false, false) => Cap::Bone,
            };
            let face = hw::paint_key(&g, *r, cap, on, false);
            let ink = cap.ink();
            let print = if cap == Cap::Dark {
                theme::ON_DARK_DIM
            } else if on {
                theme::INK
            } else {
                theme::PRINT
            };
            // The note over the computer key, below where the black keys reach.
            let note_y = if *black {
                face.top() + 12.0
            } else {
                face.bottom() - 34.0
            };
            hw::text(
                &g,
                pos2(face.center().x, note_y),
                Align2::CENTER_CENTER,
                &note_name(pitch),
                Face::Bold,
                if *black { 8.5 } else { 9.5 },
                print,
            );
            hw::text(
                &g,
                pos2(face.center().x, face.bottom() - 13.0),
                Align2::CENTER_CENTER,
                cap_letter(*key),
                Face::Bold,
                if *black { 12.0 } else { 14.0 },
                ink,
            );
        }

        // Octave and velocity keys (Z X, C V).
        ui.add_space(6.0);
        let (row, _) = ui.allocate_exact_size(vec2(width, 72.0), Sense::hover());
        let half = (width - 16.0) / 2.0;
        let groups = [
            (tr("Octave"), [(Key::Z, "Z", "-"), (Key::X, "X", "+")]),
            (tr("Velocity"), [(Key::C, "C", "-"), (Key::V, "V", "+")]),
        ];
        for (gi, (label, pair)) in groups.into_iter().enumerate() {
            let left = row.left() + gi as f32 * (half + 16.0);
            hw::bracket(&g, left, left + half, row.top() + 8.0, label);
            let kw = (half - 10.0) / 2.0;
            for (ki, (key, letter, sign)) in pair.into_iter().enumerate() {
                let r = Rect::from_min_size(
                    pos2(left + ki as f32 * (kw + 10.0), row.top() + 22.0),
                    vec2(kw, 38.0),
                );
                let (resp, face) = hw::key(
                    ui,
                    &g,
                    r,
                    id.with(("step", gi, ki)),
                    Cap::Bone,
                    false,
                    None,
                    hw::Legend::Custom,
                );
                hw::text(
                    &g,
                    pos2(face.left() + 14.0, face.center().y),
                    Align2::CENTER_CENTER,
                    letter,
                    Face::Bold,
                    14.0,
                    theme::INK,
                );
                // The sign, drawn (a minus or a plus).
                let c = pos2(face.right() - 16.0, face.center().y);
                let st = egui::Stroke::new(2.2, theme::INK);
                g.line_segment([c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)], st);
                if sign == "+" {
                    g.line_segment([c + vec2(0.0, -5.0), c + vec2(0.0, 5.0)], st);
                }
                if resp.clicked() {
                    handle(t, key, true, false);
                    handle(t, key, false, false);
                }
            }
        }
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

    /// `musical-typing.png`: the window with a chord held.
    #[test]
    #[ignore = "pictures"]
    fn picture_musical_typing() {
        let mut t = Typing::default();
        t.set_open(true);
        for k in [Key::A, Key::E, Key::G] {
            handle(&mut t, k, true, false);
        }
        crate::ui_test::render_png(
            600.0,
            420.0,
            2.0,
            crate::theme::BONE3,
            |ui| {
                crate::theme::apply(ui.ctx());
                show(ui.ctx(), &mut t);
            },
            &crate::midi_monitor::hw_utilities::picture_path("musical-typing.png"),
        )
        .unwrap();
    }
}
