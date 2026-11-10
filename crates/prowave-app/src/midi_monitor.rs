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

#[path = "hw_utilities.rs"]
pub(crate) mod hw_utilities;

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
    let mut panic = false;
    let (open, _) = hw_utilities::window(
        ctx,
        egui::Id::unique("midi_monitor"),
        tr("MIDI Monitor"),
        "",
        760.0,
        Some((egui::vec2(780.0, 600.0), egui::vec2(600.0, 420.0))),
        |ui| {
            panel(ui, p, m, &outputs, &inputs, &clock, &mut panic, test_note);
        },
    );
    if !open {
        s.show_midi_monitor = false;
    }
    if panic {
        s.midi_panic = true;
    }
}

/// What the clock gives on each output: (bpm, jitter ms, last tick ns).
type ClockStats = HashMap<String, (f64, f64, u64)>;

/// The clock figures and the message counts since the last look (the counts start again).
fn take_stats() -> (ClockStats, HashMap<(Dir, String), u64>) {
    let Ok(mut l) = log().lock() else {
        return (HashMap::new(), HashMap::new());
    };
    let stats = l
        .clock
        .iter()
        .filter(|((d, _), _)| *d == Dir::Out)
        .map(|((_, port), c)| {
            let n = c.intervals.len().max(1) as f64;
            let mean = c.intervals.iter().sum::<f64>() / n;
            let sd = (c.intervals.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt();
            let bpm = if mean > 0.0 {
                60_000.0 / (mean * 24.0)
            } else {
                0.0
            };
            (port.clone(), (bpm, sd, c.last_ns))
        })
        .collect();
    let counts = std::mem::take(&mut l.counts);
    (stats, counts)
}

/// The inside of the window: the test-note row, the ports and the message log.
#[allow(clippy::too_many_arguments)]
fn panel(
    ui: &mut egui::Ui,
    p: &Project,
    m: &mut Monitor,
    outputs: &[String],
    inputs: &[String],
    clock: &[String],
    panic: &mut bool,
    test_note: &mut Option<(String, u8)>,
) {
    use crate::hw::{self, Cap};
    use crate::theme::{self, Face};
    use egui::{pos2, vec2, Align2, Rect, Sense};
    use hw_utilities as hu;
    let id = egui::Id::unique("midi_monitor_panel");
    let w = ui.available_width();

    // Test note and All Notes Off.
    let (row, _) = ui.allocate_exact_size(vec2(w, 62.0), Sense::hover());
    let g = ui.painter().clone();
    let key_y = row.top() + 22.0;
    let panic_label = tr("Send All Notes Off");
    let pw = hu::key_width(&g, panic_label, 11.0, false).max(150.0);
    let panic_r = Rect::from_min_size(pos2(row.right() - pw, key_y), vec2(pw, 32.0));
    hw::bracket(&g, panic_r.left(), panic_r.right(), row.top() + 8.0, "MIDI");
    if hu::key(
        ui,
        panic_r,
        id.with("panic"),
        Cap::Dark,
        false,
        None,
        panic_label,
        true,
    )
    .clicked()
    {
        *panic = true;
    }
    let send_label = tr("Send");
    let sw = hu::key_width(&g, send_label, 11.0, false).max(70.0);
    let ch_w = 74.0;
    let test_right = panic_r.left() - 24.0;
    let port_w = (test_right - row.left() - ch_w - sw - 20.0).clamp(140.0, 300.0);
    hw::bracket(
        &g,
        row.left(),
        row.left() + port_w + ch_w + sw + 20.0,
        row.top() + 8.0,
        tr("Test note"),
    );
    let field_y = key_y + 4.0;
    let port_r = Rect::from_min_size(pos2(row.left(), field_y), vec2(port_w, 24.0));
    let shown = match (&m.test_port, outputs.is_empty()) {
        (Some(port), _) => port.clone(),
        (None, true) => tr("No Port").to_string(),
        (None, false) => tr("Port").to_string(),
    };
    let sel = m
        .test_port
        .as_ref()
        .and_then(|t| outputs.iter().position(|o| o == t));
    if let Some(i) = hu::dropdown(
        ui,
        id.with("port"),
        port_r,
        &shown,
        outputs,
        sel,
        !outputs.is_empty(),
    ) {
        m.test_port = outputs.get(i).cloned();
    }
    let ch_r = Rect::from_min_size(pos2(port_r.right() + 10.0, field_y), vec2(ch_w, 24.0));
    let ch_text = format!("{} {:02}", tr("Ch"), m.test_channel);
    hu::stepper(
        ui,
        id.with("channel"),
        ch_r,
        &mut m.test_channel,
        1,
        16,
        &ch_text,
    );
    let send_r = Rect::from_min_size(pos2(ch_r.right() + 10.0, key_y), vec2(sw, 32.0));
    let can_send = m.test_port.is_some();
    if hu::key(
        ui,
        send_r,
        id.with("send"),
        Cap::Bone,
        false,
        None,
        send_label,
        can_send,
    )
    .clicked()
    {
        if let Some(port) = m.test_port.clone() {
            *test_note = Some((port, m.test_channel));
        }
    }

    // Ports: every output (with the ones the project or the clock still names), then the
    // inputs.
    let (clock_stats, counts) = take_stats();
    let now = now_ns();
    hu::section(ui, tr("Ports"));
    let mut all: Vec<String> = outputs.to_vec();
    for t in &p.tracks {
        if let Some(port) = &t.midi_port {
            if !all.contains(port) {
                all.push(port.clone());
            }
        }
    }
    for c in clock {
        if !all.contains(c) {
            all.push(c.clone());
        }
    }
    let row_h = 32.0;
    let cols = |r: Rect| {
        let name = r.left() + 26.0;
        let tempo_w = 178.0;
        let tempo = r.right() - tempo_w - 4.0;
        let clock = name + ((tempo - name) * 0.42).max(150.0);
        let tracks = clock + 72.0;
        (name, clock, tracks, tempo, tempo_w)
    };
    let (head, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
    {
        let (name, clock_x, tracks, tempo, _) = cols(head);
        for (x, h) in [
            (name, "Port"),
            (clock_x, "Clock"),
            (tracks, "Tracks"),
            (tempo, "Clock tempo"),
        ] {
            let label = tr(h);
            hw::silk(
                &g,
                pos2(x, head.center().y),
                Align2::LEFT_CENTER,
                label,
                10.0,
                theme::PRINT,
            );
        }
    }
    hw::seam_h(&g, head.x_range(), head.bottom() - 1.0);
    let max_rows = ((ui.available_height() - 200.0) / row_h).floor().max(2.0) as usize;
    let rows: Vec<(String, Dir)> = all
        .into_iter()
        .map(|n| (n, Dir::Out))
        .chain(inputs.iter().map(|n| (n.clone(), Dir::In)))
        .collect();
    let draw_row = |ui: &mut egui::Ui, port: &str, dir: Dir| {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
        let g = ui.painter().clone();
        let (name_x, clock_x, tracks_x, tempo_x, tempo_w) = cols(r);
        let busy = counts.get(&(dir, port.to_string())).copied().unwrap_or(0) > 0;
        hw::led_sized(&g, pos2(r.left() + 10.0, r.center().y), 3.0, busy);
        let online = dir == Dir::In || outputs.iter().any(|o| o == port);
        let label = if dir == Dir::Out {
            tr("Output")
        } else {
            tr("input")
        };
        let c = hu::chip(
            &g,
            pos2(name_x, r.center().y),
            &label.to_uppercase(),
            8.5,
            false,
        );
        let mut name_room = clock_x - 10.0 - (c.right() + 7.0);
        let off_label = tr("offline").to_uppercase();
        let off_w = hw::galley(&g, &off_label, Face::Bold, 8.5, theme::INK)
            .size()
            .x
            + 14.0;
        if !online {
            name_room -= off_w;
        }
        let shown = hw::fit(&g, port, Face::Bold, 11.0, name_room);
        let nr = hw::text(
            &g,
            pos2(c.right() + 7.0, r.center().y),
            Align2::LEFT_CENTER,
            &shown,
            Face::Bold,
            11.0,
            if online { theme::INK } else { theme::PRINT },
        );
        if !online {
            hu::tag(&g, pos2(nr.right() + 6.0, r.center().y), &off_label, 8.5);
        }
        if dir == Dir::Out {
            let on = clock.iter().any(|c| c == port);
            let kr = Rect::from_min_size(pos2(clock_x, r.center().y - 11.0), vec2(60.0, 22.0));
            let legend = if on { tr("On") } else { tr("Off") };
            if hu::key(
                ui,
                kr,
                egui::Id::unique(("monitor_clock", port)),
                Cap::Bone,
                on,
                Some(on),
                legend,
                true,
            )
            .clicked()
            {
                crate::midi_out::set_clock_port(port, !on);
            }
            let tracks: Vec<String> = p
                .tracks
                .iter()
                .filter(|t| {
                    t.kind == TrackKind::ExternalMidi && t.midi_port.as_deref() == Some(port)
                })
                .map(|t| format!("{} {} {}", t.name, tr("Ch"), t.midi_channel))
                .collect();
            let shown = hw::fit(
                &g,
                &tracks.join(", "),
                Face::Body,
                10.5,
                tempo_x - tracks_x - 10.0,
            );
            hw::text(
                &g,
                pos2(tracks_x, r.center().y),
                Align2::LEFT_CENTER,
                &shown,
                Face::Body,
                10.5,
                theme::INK2,
            );
            // The clock's tempo on a small LCD: its jitter at the left, the bpm at the right.
            let lr = Rect::from_min_size(pos2(tempo_x, r.center().y - 12.0), vec2(tempo_w, 24.0));
            hw::lcd_glass(&g, lr);
            match clock_stats.get(port) {
                Some((bpm, sd, last)) if now.saturating_sub(*last) < 500_000_000 => {
                    let v = format!("{bpm:.2}");
                    let vw = hw::seg_width(&v, 13.0);
                    hw::seg(
                        &g,
                        pos2(lr.right() - 8.0 - vw, lr.top() + 5.5),
                        &v,
                        13.0,
                        theme::LCD_INK,
                        true,
                    );
                    hw::text(
                        &g,
                        pos2(lr.left() + 6.0, lr.center().y),
                        Align2::LEFT_CENTER,
                        &format!("±{sd:.3}MS"),
                        Face::Lcd,
                        8.5,
                        theme::LCD_INK.gamma_multiply(0.75),
                    );
                }
                _ => {
                    let v = "888.88";
                    let vw = hw::seg_width(v, 13.0);
                    hw::seg(
                        &g,
                        pos2(lr.right() - 8.0 - vw, lr.top() + 5.5),
                        v,
                        13.0,
                        theme::LCD_GHOST,
                        true,
                    );
                }
            }
        }
        hw::seam_h(&g, r.x_range(), r.bottom() - 1.0);
    };
    if rows.is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(w, row_h), Sense::hover());
        hw::silk(
            &g,
            pos2(r.left() + 26.0, r.center().y),
            Align2::LEFT_CENTER,
            tr("No Port"),
            11.0,
            theme::PRINT,
        );
        hw::seam_h(&g, r.x_range(), r.bottom() - 1.0);
    } else if rows.len() <= max_rows {
        for (port, dir) in &rows {
            draw_row(ui, port, *dir);
        }
    } else {
        egui::ScrollArea::vertical()
            .id_salt("monitor_ports")
            .max_height(max_rows as f32 * row_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                for (port, dir) in &rows {
                    draw_row(ui, port, *dir);
                }
            });
    }

    // Activity: what to show, then the log on an LCD.
    ui.add_space(8.0);
    let (bar, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
    let paused = log().lock().map(|l| l.paused).unwrap_or(false);
    let keys: [(&str, Option<bool>, bool); 4] = [
        (tr("Out"), Some(m.show_out), m.show_out),
        (tr("In"), Some(m.show_in), m.show_in),
        (
            if paused { tr("Resume") } else { tr("Pause") },
            Some(paused),
            paused,
        ),
        (tr("Clear"), None, false),
    ];
    let widths: Vec<f32> = keys
        .iter()
        .map(|(l, led, _)| hu::key_width(&g, l, 11.0, led.is_some()).max(62.0))
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 8.0 * (widths.len() - 1) as f32;
    let mut x = bar.right() - total;
    hw::bracket(&g, bar.left(), x - 14.0, bar.center().y, tr("Activity"));
    for (i, ((label, led, latched), kw)) in keys.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(pos2(x, bar.top() + 5.0), vec2(*kw, 28.0));
        x += kw + 8.0;
        let cap = Cap::Bone;
        if hu::key(ui, r, id.with(("act", i)), cap, *latched, *led, label, true).clicked() {
            match i {
                0 => m.show_out = !m.show_out,
                1 => m.show_in = !m.show_in,
                2 => {
                    if let Ok(mut l) = log().lock() {
                        l.paused = !l.paused;
                    }
                }
                _ => {
                    if let Ok(mut l) = log().lock() {
                        l.entries.clear();
                        l.clock.clear();
                    }
                }
            }
        }
    }
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
    ui.add_space(4.0);
    let h = ui.available_height().max(140.0);
    let (lcd_r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let glass = hw::lcd(&g, lcd_r);
    // Columns on the glass.
    let gx = glass.left() + 10.0;
    let gw = glass.width() - 20.0;
    let c_dir = gx + 74.0;
    let c_port = c_dir + 18.0;
    let c_ch = c_port + (gw * 0.24).clamp(110.0, 220.0);
    let c_msg = c_ch + 34.0;
    let c_ahead = glass.right() - 10.0;
    for (x, h) in [
        (gx, "Time"),
        (c_port, "Port"),
        (c_ch, "Ch"),
        (c_msg, "Message"),
    ] {
        hu::lcd_label(&g, pos2(x, glass.top() + 6.0), tr(h));
    }
    let ahead_key = "Ahead";
    let ahead = tr(ahead_key).to_uppercase();
    hw::text(
        &g,
        pos2(c_ahead, glass.top() + 6.0),
        Align2::RIGHT_TOP,
        &ahead,
        Face::Lcd,
        8.5,
        theme::LCD_INK.gamma_multiply(0.72),
    );
    g.hline(
        glass.left() + 6.0..=glass.right() - 6.0,
        glass.top() + 20.5,
        egui::Stroke::new(1.0, theme::LCD_INK.gamma_multiply(0.25)),
    );
    let body = Rect::from_min_max(
        pos2(glass.left(), glass.top() + 23.0),
        glass.max - vec2(0.0, 3.0),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
    egui::ScrollArea::vertical()
        .id_salt("monitor_log")
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let size = 10.5;
            for e in entries {
                let (r, _) = ui.allocate_exact_size(vec2(body.width(), 18.0), Sense::hover());
                let g = ui.painter();
                let cy = r.center().y;
                let ink = theme::LCD_INK;
                let age = now.saturating_sub(e.at_ns) as f64 / 1e9;
                let put = |x: f32, s: &str, room: f32| {
                    let shown = hw::fit(g, &s.to_uppercase(), Face::Lcd, size, room);
                    hw::text(
                        g,
                        pos2(x, cy),
                        Align2::LEFT_CENTER,
                        &shown,
                        Face::Lcd,
                        size,
                        ink,
                    );
                };
                put(gx, &format!("-{age:.3}s"), 70.0);
                hu::arrow(g, pos2(c_dir + 5.0, cy), 8.0, e.dir == Dir::Out, ink);
                put(c_port, &e.port, c_ch - c_port - 8.0);
                let (text, ch) = describe(&e.bytes[..usize::from(e.len)]);
                put(c_ch, &ch.map_or(String::new(), |c| format!("{c:02}")), 30.0);
                put(c_msg, &text, (c_ahead - 90.0 - c_msg).max(40.0));
                if e.dir == Dir::Out && e.lead_ms != 0.0 {
                    let txt = format!("{:+.2}MS", e.lead_ms);
                    if e.lead_ms < 0.0 {
                        // Late: printed inverted, as the LCD's CYC flag.
                        let gal = hw::galley(g, &txt, Face::Lcd, size, theme::LCD);
                        let br = Rect::from_min_max(
                            pos2(c_ahead - gal.size().x - 8.0, cy - 7.5),
                            pos2(c_ahead, cy + 7.5),
                        );
                        g.rect_filled(br, 1.5, ink);
                        hw::text(
                            g,
                            br.center(),
                            Align2::CENTER_CENTER,
                            &txt,
                            Face::Lcd,
                            size,
                            theme::LCD,
                        );
                    } else {
                        hw::text(
                            g,
                            pos2(c_ahead, cy),
                            Align2::RIGHT_CENTER,
                            &txt,
                            Face::Lcd,
                            size,
                            ink,
                        );
                    }
                }
            }
        });
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

    /// `midi-monitor.png`: a project with an External MIDI track on a port that is not
    /// connected, its clock running, and a few messages (one late).
    #[test]
    #[ignore = "pictures"]
    fn picture_midi_monitor() {
        let mut p = Project::demo();
        let ti = p.add_track(TrackKind::ExternalMidi);
        p.tracks[ti].name = "Subsequent 25".into();
        p.tracks[ti].midi_port = Some("Subsequent 25".into());
        p.tracks[ti].midi_channel = 2;
        let port = "Subsequent 25";
        let now = now_ns();
        let tick = (60_000.0 / (120.0 * 24.0) * 1e6) as u64;
        // Ticks due up to a few seconds ahead, so the clock still reads as running while the
        // picture is drawn.
        let last = now + 4_000_000_000;
        for i in (0..48u64).rev() {
            record(Dir::Out, port, &[0xF8], last - i * tick - (i % 3) * 9_000);
        }
        record(Dir::Out, port, &[0xFA], now + 2_000_000);
        for (k, b) in [
            [0x91, 48, 100],
            [0x81, 48, 0],
            [0xB1, 74, 92],
            [0x91, 55, 87],
            [0xE1, 0, 72],
            [0x91, 60, 110],
        ]
        .iter()
        .enumerate()
        {
            let ahead = if k == 3 {
                -1_400_000
            } else {
                2_500_000 + k as i64 * 100_000
            };
            record(Dir::Out, port, b, (now as i64 + ahead) as u64);
        }
        record(Dir::In, "Komplete Kontrol A49", &[0x90, 64, 96], 0);
        record(Dir::In, "Komplete Kontrol A49", &[0xB0, 1, 40], 0);
        let mut s = crate::UiState::new();
        s.show_midi_monitor = true;
        let mut test_note = None;
        crate::ui_test::render_png(
            900.0,
            680.0,
            2.0,
            crate::theme::BONE3,
            |ui| {
                crate::theme::apply(ui.ctx());
                show(ui.ctx(), &p, &mut s, &mut test_note);
            },
            &hw_utilities::picture_path("midi-monitor.png"),
        )
        .unwrap();
    }
}
