//! Window > MIDI Monitor: every MIDI port on this computer and what goes through it, to watch
//! and troubleshoot the rig from inside Pro Wave.
//!
//! - Ports: outputs and inputs, online or not, which outputs get clock and which tracks send
//!   to them.
//! - Activity: the messages going out (with how far ahead of their time CoreMIDI got them;
//!   negative is late) and coming in, decoded.
//! - Clock: per port, the tempo the clock gives and how steady it is.
//! - Send All Notes Off and a test note to any port and channel.
use crate::i18n::tr;
use crate::model::{Project, TrackKind};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

/// Messages kept in the log.
const KEEP: usize = 2000;
/// Clock intervals kept per port (four beats).
const CLOCK_KEEP: usize = 96;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    In,
    Out,
}

/// One message seen.
#[derive(Clone, Debug)]
pub struct Entry {
    pub dir: Dir,
    pub port: String,
    pub bytes: [u8; 3],
    pub len: u8,
    /// Host time it was sent or arrived, nanoseconds.
    pub at_ns: u64,
    /// Outgoing: how far ahead of its time it was handed over, ms (negative: late). 0: sent
    /// unstamped.
    pub lead_ms: f32,
}

#[derive(Default)]
struct ClockStat {
    last_ns: u64,
    intervals: VecDeque<f64>,
    ticks: u64,
}

#[derive(Default)]
struct Log {
    entries: VecDeque<Entry>,
    paused: bool,
    clock: HashMap<(Dir, String), ClockStat>,
    /// Messages per port since the window last counted.
    counts: HashMap<(Dir, String), u64>,
}

fn log() -> &'static Mutex<Log> {
    static LOG: OnceLock<Mutex<Log>> = OnceLock::new();
    LOG.get_or_init(Default::default)
}

fn now_ns() -> u64 {
    #[cfg(target_os = "macos")]
    {
        prowave_engine::duplex::host_now_ns()
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

/// Notes a message (from the MIDI sender, or an input). Clock ticks only count toward the
/// clock figures, so they do not flood the log.
pub fn record(dir: Dir, port: &str, bytes: &[u8], scheduled_ns: u64) {
    let Ok(mut l) = log().lock() else { return };
    let now = now_ns();
    *l.counts.entry((dir, port.to_string())).or_default() += 1;
    if bytes.first() == Some(&0xF8) {
        // Outgoing ticks are measured at the time they are due, incoming ones on arrival.
        let at = if scheduled_ns > 0 { scheduled_ns } else { now };
        let c = l.clock.entry((dir, port.to_string())).or_default();
        if c.last_ns > 0 && at > c.last_ns {
            c.intervals.push_back((at - c.last_ns) as f64 / 1e6);
            if c.intervals.len() > CLOCK_KEEP {
                c.intervals.pop_front();
            }
        }
        c.last_ns = at;
        c.ticks += 1;
        return;
    }
    if l.paused {
        return;
    }
    let mut b = [0u8; 3];
    let n = bytes.len().min(3);
    b[..n].copy_from_slice(&bytes[..n]);
    let lead_ms = if scheduled_ns > 0 {
        (scheduled_ns as f64 - now as f64) as f32 / 1e6
    } else {
        0.0
    };
    l.entries.push_back(Entry {
        dir,
        port: port.to_string(),
        bytes: b,
        len: n as u8,
        at_ns: now,
        lead_ms,
    });
    if l.entries.len() > KEEP {
        l.entries.pop_front();
    }
}

/// A message in words: "Note On C3 vel 100", "CC 64 = 127", "Clock", ...
pub fn describe(bytes: &[u8]) -> (String, Option<u8>) {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let note = |n: u8| format!("{}{}", NAMES[usize::from(n % 12)], i32::from(n / 12) - 2);
    let Some(&s) = bytes.first() else {
        return (String::new(), None);
    };
    let d1 = bytes.get(1).copied().unwrap_or(0);
    let d2 = bytes.get(2).copied().unwrap_or(0);
    let ch = Some((s & 0x0f) + 1);
    match s & 0xf0 {
        0x90 if d2 > 0 => (format!("Note On {} vel {d2}", note(d1)), ch),
        0x90 | 0x80 => (format!("Note Off {}", note(d1)), ch),
        0xa0 => (format!("Poly Pressure {} {d2}", note(d1)), ch),
        0xb0 => (format!("CC {d1} = {d2}"), ch),
        0xc0 => (format!("Program {d1}"), ch),
        0xd0 => (format!("Channel Pressure {d1}"), ch),
        0xe0 => (
            format!("Pitch Bend {}", (i32::from(d2) << 7 | i32::from(d1)) - 8192),
            ch,
        ),
        _ => (
            match s {
                0xF8 => "Clock".into(),
                0xFA => "Start".into(),
                0xFB => "Continue".into(),
                0xFC => "Stop".into(),
                0xF2 => format!("Song Position {}", i32::from(d2) << 7 | i32::from(d1)),
                0xF1 => "MTC Quarter Frame".into(),
                0xFE => "Active Sensing".into(),
                0xFF => "Reset".into(),
                other => format!("System {other:02X}"),
            },
            None,
        ),
    }
}

/// The monitor's own connections to the inputs (macOS), opened while the window is open.
#[derive(Default)]
pub struct Monitor {
    #[cfg(target_os = "macos")]
    inputs: Option<Inputs>,
    test_port: Option<String>,
    test_channel: u8,
    show_out: bool,
    show_in: bool,
    started: bool,
}

#[cfg(target_os = "macos")]
struct Inputs {
    client: coremidi::Client,
    ports: Vec<(String, coremidi::InputPort)>,
    looked: std::time::Instant,
}

#[cfg(target_os = "macos")]
impl Inputs {
    fn new() -> Option<Self> {
        Some(Self {
            client: coremidi::Client::new("Pro Wave Monitor").ok()?,
            ports: Vec::new(),
            looked: std::time::Instant::now() - std::time::Duration::from_secs(10),
        })
    }

    /// Listens to every source not listened to yet (looks again every two seconds).
    fn refresh(&mut self) {
        if self.looked.elapsed() < std::time::Duration::from_secs(2) {
            return;
        }
        self.looked = std::time::Instant::now();
        for source in coremidi::Sources {
            let Some(name) = source.display_name() else {
                continue;
            };
            if self.ports.iter().any(|(n, _)| n == &name) {
                continue;
            }
            let label = name.clone();
            let Ok(port) = self
                .client
                .input_port(&format!("Monitor {name}"), move |packets| {
                    for p in packets.iter() {
                        record(Dir::In, &label, p.data(), 0);
                    }
                })
            else {
                continue;
            };
            if port.connect_source(&source).is_ok() {
                self.ports.push((name, port));
            }
        }
    }
}

fn input_names() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        coremidi::Sources
            .into_iter()
            .filter_map(|s| s.display_name())
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// The window (Window > MIDI Monitor). `panic` and `test_note` are what the user asked for.
pub fn show(
    ctx: &egui::Context,
    p: &Project,
    s: &mut crate::UiState,
    test_note: &mut Option<(String, u8)>,
) {
    if !s.show_midi_monitor {
        // Closed: stop listening.
        #[cfg(target_os = "macos")]
        {
            s.midi_monitor.inputs = None;
        }
        return;
    }
    let m = &mut s.midi_monitor;
    if !m.started {
        m.started = true;
        m.show_out = true;
        m.show_in = true;
        m.test_channel = 1;
    }
    #[cfg(target_os = "macos")]
    {
        if m.inputs.is_none() {
            m.inputs = Inputs::new();
        }
        if let Some(i) = m.inputs.as_mut() {
            i.refresh();
        }
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    let outputs = crate::midi_out::ports();
    let inputs = input_names();
    let clock = crate::midi_out::clock_ports();
    let mut open = true;
    let mut panic = false;
    egui::Window::new(tr("MIDI Monitor"))
        .id(egui::Id::unique("midi_monitor"))
        .open(&mut open)
        .default_size([720.0, 520.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button(tr("Send All Notes Off")).clicked() {
                    panic = true;
                }
                ui.separator();
                ui.label(tr("Test note"));
                let current = m.test_port.clone().unwrap_or_default();
                egui::ComboBox::from_id_salt("monitor_test_port")
                    .selected_text(if current.is_empty() {
                        tr("Port")
                    } else {
                        &current
                    })
                    .show_ui(ui, |ui| {
                        for name in &outputs {
                            ui.selectable_value(&mut m.test_port, Some(name.clone()), name);
                        }
                    });
                ui.add(
                    egui::DragValue::new(&mut m.test_channel)
                        .range(1..=16)
                        .prefix("ch "),
                );
                if ui
                    .add_enabled(m.test_port.is_some(), egui::Button::new(tr("Send")))
                    .clicked()
                {
                    if let Some(port) = m.test_port.clone() {
                        *test_note = Some((port, m.test_channel));
                    }
                }
            });
            ui.separator();
            // Ports.
            let (clock_stats, counts) = match log().lock() {
                Ok(mut l) => {
                    let stats: HashMap<String, (f64, f64, f64, u64, u64)> = l
                        .clock
                        .iter()
                        .filter(|((d, _), _)| *d == Dir::Out)
                        .map(|((_, port), c)| {
                            let n = c.intervals.len().max(1) as f64;
                            let mean = c.intervals.iter().sum::<f64>() / n;
                            let sd = (c.intervals.iter().map(|x| (x - mean).powi(2)).sum::<f64>()
                                / n)
                                .sqrt();
                            let bpm = if mean > 0.0 {
                                60_000.0 / (mean * 24.0)
                            } else {
                                0.0
                            };
                            (port.clone(), (bpm, sd, mean, c.ticks, c.last_ns))
                        })
                        .collect();
                    let counts = std::mem::take(&mut l.counts);
                    (stats, counts)
                }
                Err(_) => (HashMap::new(), HashMap::new()),
            };
            let now = now_ns();
            egui::CollapsingHeader::new(tr("Ports"))
                .default_open(true)
                .show(ui, |ui| {
                    egui::Grid::new("monitor_ports")
                        .striped(true)
                        .num_columns(5)
                        .show(ui, |ui| {
                            for h in ["Output", "Clock", "Tracks", "Clock tempo", "Activity"] {
                                ui.strong(tr(h));
                            }
                            ui.end_row();
                            let mut all: Vec<String> = outputs.clone();
                            // Ports the project or the clock names but the system no longer has.
                            for t in &p.tracks {
                                if let Some(port) = &t.midi_port {
                                    if !all.contains(port) {
                                        all.push(port.clone());
                                    }
                                }
                            }
                            for c in &clock {
                                if !all.contains(c) {
                                    all.push(c.clone());
                                }
                            }
                            for port in all {
                                let online = outputs.contains(&port);
                                let name = if online {
                                    egui::RichText::new(&port)
                                } else {
                                    egui::RichText::new(format!("{port} ({})", tr("offline")))
                                        .color(egui::Color32::from_rgb(0xe0, 0x60, 0x50))
                                };
                                ui.label(name);
                                let mut on = clock.contains(&port);
                                if ui.checkbox(&mut on, "").changed() {
                                    crate::midi_out::set_clock_port(&port, on);
                                }
                                let tracks: Vec<String> = p
                                    .tracks
                                    .iter()
                                    .filter(|t| {
                                        t.kind == TrackKind::ExternalMidi
                                            && t.midi_port.as_deref() == Some(port.as_str())
                                    })
                                    .map(|t| format!("{} (ch {})", t.name, t.midi_channel))
                                    .collect();
                                ui.label(tracks.join(", "));
                                match clock_stats.get(&port) {
                                    Some((bpm, sd, _, _, last))
                                        if now.saturating_sub(*last) < 500_000_000 =>
                                    {
                                        ui.label(format!("{bpm:.2} bpm, ±{sd:.3} ms"));
                                    }
                                    _ => {
                                        ui.label("—");
                                    }
                                }
                                let busy =
                                    counts.get(&(Dir::Out, port.clone())).copied().unwrap_or(0);
                                activity_dot(ui, busy > 0);
                                ui.end_row();
                            }
                            for port in &inputs {
                                ui.label(format!("{port} ({})", tr("input")));
                                ui.label("");
                                ui.label("");
                                ui.label("");
                                let busy =
                                    counts.get(&(Dir::In, port.clone())).copied().unwrap_or(0);
                                activity_dot(ui, busy > 0);
                                ui.end_row();
                            }
                        });
                });
            ui.separator();
            // Activity.
            ui.horizontal(|ui| {
                ui.strong(tr("Activity"));
                ui.checkbox(&mut m.show_out, tr("Out"));
                ui.checkbox(&mut m.show_in, tr("In"));
                if let Ok(mut l) = log().lock() {
                    let label = if l.paused { tr("Resume") } else { tr("Pause") };
                    if ui.button(label).clicked() {
                        l.paused = !l.paused;
                    }
                    if ui.button(tr("Clear")).clicked() {
                        l.entries.clear();
                        l.clock.clear();
                    }
                }
            });
            let entries: Vec<Entry> = log()
                .lock()
                .map(|l| {
                    l.entries
                        .iter()
                        .rev()
                        .filter(|e| match e.dir {
                            Dir::Out => m.show_out,
                            Dir::In => m.show_in,
                        })
                        .take(400)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("monitor_log")
                        .striped(true)
                        .num_columns(6)
                        .show(ui, |ui| {
                            for h in ["Time", "", "Port", "Ch", "Message", "Ahead"] {
                                ui.strong(tr(h));
                            }
                            ui.end_row();
                            for e in entries {
                                let age = now.saturating_sub(e.at_ns) as f64 / 1e9;
                                ui.monospace(format!("-{age:.3} s"));
                                ui.label(if e.dir == Dir::Out { "→" } else { "←" });
                                ui.label(&e.port);
                                let (text, ch) = describe(&e.bytes[..usize::from(e.len)]);
                                ui.label(ch.map_or(String::new(), |c| c.to_string()));
                                ui.label(text);
                                if e.dir == Dir::Out && e.lead_ms != 0.0 {
                                    let txt = format!("{:+.2} ms", e.lead_ms);
                                    if e.lead_ms < 0.0 {
                                        ui.colored_label(
                                            egui::Color32::from_rgb(0xe0, 0x60, 0x50),
                                            txt,
                                        );
                                    } else {
                                        ui.monospace(txt);
                                    }
                                } else {
                                    ui.label("");
                                }
                                ui.end_row();
                            }
                        });
                });
        });
    if !open {
        s.show_midi_monitor = false;
    }
    if panic {
        s.midi_panic = true;
    }
}

fn activity_dot(ui: &mut egui::Ui, busy: bool) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    let c = if busy {
        egui::Color32::from_rgb(0x50, 0xd0, 0x60)
    } else {
        egui::Color32::from_gray(0x50)
    };
    ui.painter().circle_filled(r.center(), 4.0, c);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_put_in_words() {
        assert_eq!(
            describe(&[0x91, 60, 100]),
            ("Note On C3 vel 100".into(), Some(2))
        );
        assert_eq!(describe(&[0x90, 60, 0]).0, "Note Off C3");
        assert_eq!(describe(&[0xB0, 64, 127]).0, "CC 64 = 127");
        assert_eq!(describe(&[0xE0, 0, 64]).0, "Pitch Bend 0");
        assert_eq!(describe(&[0xF2, 4, 1]).0, "Song Position 132");
        assert_eq!(describe(&[0xF8]), ("Clock".into(), None));
    }
}
