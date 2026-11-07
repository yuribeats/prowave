//! The RECORD / MIX switch in the control bar: one click sets the whole audio engine up for
//! playing and recording (the lowest latency) or for mixing (the most headroom for heavy
//! plug-ins).
//!
//! - RECORD: the record I/O buffer (64 frames unless changed), Low Latency Mode on and the
//!   record process buffer (Small unless changed).
//! - MIX: the mix I/O buffer (1024 frames unless changed), Low Latency Mode off (full delay
//!   compensation everywhere) and the mix process buffer (Large unless changed).
//!
//! Each mode keeps its own I/O buffer and process buffer, saved on this computer; Record >
//! I/O Buffer Size and Record > Process Buffer change the current mode's. Arming a track in
//! MIX switches to RECORD (Record > Switch to Record Mode When Arming, on unless turned off).
use crate::i18n::tr;
use crate::model::Project;
use prowave_engine::{PROCESS_BUFFER_LARGE, PROCESS_BUFFER_SMALL};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineMode {
    Record,
    Mix,
}

impl EngineMode {
    fn index(self) -> usize {
        match self {
            EngineMode::Record => 0,
            EngineMode::Mix => 1,
        }
    }

    /// The other mode.
    pub fn other(self) -> Self {
        match self {
            EngineMode::Record => EngineMode::Mix,
            EngineMode::Mix => EngineMode::Record,
        }
    }

    /// The label on the switch.
    pub fn label(self) -> &'static str {
        match self {
            EngineMode::Record => tr("RECORD"),
            EngineMode::Mix => tr("MIX"),
        }
    }
}

/// Both modes' settings, and which one is on.
#[derive(Clone, Debug, PartialEq)]
pub struct Modes {
    pub mode: EngineMode,
    /// I/O buffer in frames, by mode (RECORD, MIX).
    pub io: [u32; 2],
    /// Process buffer in frames, by mode.
    pub process: [u32; 2],
    /// Arming a track in MIX switches to RECORD.
    pub auto_record: bool,
    /// The tracks that were armed last frame (to see a track being armed).
    armed: Vec<bool>,
}

impl Default for Modes {
    fn default() -> Self {
        Self {
            mode: EngineMode::Record,
            io: [64, 1024],
            process: [PROCESS_BUFFER_SMALL, PROCESS_BUFFER_LARGE],
            auto_record: true,
            armed: Vec::new(),
        }
    }
}

/// The process buffers offered (Record > Process Buffer), as (entry, frames).
pub const PROCESS_BUFFERS: [(&str, u32); 3] = [
    ("Small", PROCESS_BUFFER_SMALL),
    ("Medium", prowave_engine::PROCESS_BUFFER_MEDIUM),
    ("Large", PROCESS_BUFFER_LARGE),
];

/// The frames of a Process Buffer entry.
pub fn process_frames(label: &str) -> Option<u32> {
    PROCESS_BUFFERS
        .iter()
        .find(|(l, _)| *l == label)
        .map(|(_, f)| *f)
}

const FILE: &str = "engine-modes";

impl Modes {
    pub fn io_of(&self, mode: EngineMode) -> u32 {
        self.io[mode.index()]
    }

    pub fn process_of(&self, mode: EngineMode) -> u32 {
        self.process[mode.index()]
    }

    /// The settings as saved: one `key = value` line each.
    pub fn to_text(&self) -> String {
        format!(
            "mode = {}\nrecord-io = {}\nmix-io = {}\nrecord-process = {}\nmix-process = {}\nauto-record = {}\n",
            match self.mode {
                EngineMode::Record => "record",
                EngineMode::Mix => "mix",
            },
            self.io[0],
            self.io[1],
            self.process[0],
            self.process[1],
            self.auto_record,
        )
    }

    /// Settings read back from [`Modes::to_text`]; anything missing or not offered keeps its
    /// default.
    pub fn from_text(text: &str) -> Self {
        let mut m = Self::default();
        let io_ok = |n: &u32| crate::audio_rec::BUFFER_SIZES.contains(n);
        let pb_ok = |n: &u32| PROCESS_BUFFERS.iter().any(|(_, f)| f == n);
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            let num = v.parse::<u32>().ok();
            match k {
                "mode" if v == "mix" => m.mode = EngineMode::Mix,
                "mode" if v == "record" => m.mode = EngineMode::Record,
                "record-io" => m.io[0] = num.filter(io_ok).unwrap_or(m.io[0]),
                "mix-io" => m.io[1] = num.filter(io_ok).unwrap_or(m.io[1]),
                "record-process" => m.process[0] = num.filter(pb_ok).unwrap_or(m.process[0]),
                "mix-process" => m.process[1] = num.filter(pb_ok).unwrap_or(m.process[1]),
                "auto-record" => m.auto_record = v != "false",
                _ => {}
            }
        }
        m
    }

    /// As saved on this computer. Before the modes existed, the one I/O buffer saved becomes
    /// RECORD's.
    pub fn load() -> Self {
        let saved =
            crate::audio_rec::setting_file(FILE).and_then(|f| std::fs::read_to_string(f).ok());
        match saved {
            Some(text) => Self::from_text(&text),
            None => {
                let mut m = Self::default();
                m.io[0] = crate::audio_rec::io_buffer();
                m
            }
        }
    }

    /// Saves them on this computer (not from the tests).
    pub fn save(&self) {
        if cfg!(test) {
            return;
        }
        if let Some(file) = crate::audio_rec::setting_file(FILE) {
            if let Some(dir) = file.parent() {
                let _ =
                    std::fs::create_dir_all(dir).and_then(|_| std::fs::write(file, self.to_text()));
            }
        }
    }
}

/// Switches the engine setup to `mode`: its I/O buffer and process buffer, and Low Latency
/// Mode on for RECORD, off for MIX. Playback goes on; the playhead stays.
pub fn set_mode(s: &mut crate::UiState, mode: EngineMode) {
    s.modes.mode = mode;
    s.io_buffer = s.modes.io_of(mode);
    s.process_buffer = s.modes.process_of(mode);
    s.low_latency = mode == EngineMode::Record;
    crate::audio_rec::set_low_latency(s.low_latency);
    s.modes.save();
}

/// The RECORD / MIX switch was clicked (or its key command pressed).
pub fn toggle(s: &mut crate::UiState) {
    let next = s.modes.mode.other();
    set_mode(s, next);
}

/// Record > I/O Buffer Size: the current mode's.
pub fn set_io_buffer(s: &mut crate::UiState, frames: u32) {
    s.io_buffer = frames;
    s.modes.io[s.modes.mode.index()] = frames;
    if s.modes.mode == EngineMode::Record {
        // Kept for a build from before the modes.
        crate::audio_rec::set_io_buffer(frames);
    }
    s.modes.save();
}

/// Record > Process Buffer: the current mode's.
pub fn set_process_buffer(s: &mut crate::UiState, frames: u32) {
    s.process_buffer = frames;
    s.modes.process[s.modes.mode.index()] = frames;
    s.modes.save();
}

/// Record > Switch to Record Mode When Arming.
pub fn toggle_auto_record(s: &mut crate::UiState) {
    s.modes.auto_record = !s.modes.auto_record;
    s.modes.save();
}

/// Once per frame: a track armed in MIX switches to RECORD (when the preference is on), as
/// engineers do by hand, and says so in the status line.
pub fn follow_arming(p: &Project, s: &mut crate::UiState) {
    let armed: Vec<bool> = p.tracks.iter().map(|t| t.rec).collect();
    let newly = armed
        .iter()
        .enumerate()
        .any(|(i, &on)| on && !s.modes.armed.get(i).copied().unwrap_or(false));
    // Only a change of arming counts: a project opened with armed tracks does not switch.
    let first = s.modes.armed.is_empty() && !p.tracks.is_empty();
    s.modes.armed = armed;
    if newly && !first && s.modes.auto_record && s.modes.mode == EngineMode::Mix {
        set_mode(s, EngineMode::Record);
        s.status = Some(tr("Switched to RECORD mode: a track was armed for recording").into());
    }
}

/// The round trip of a monitored input, in milliseconds: what the devices add on the way in
/// and out, and one I/O buffer each way.
pub fn round_trip_ms(device: (u32, u32), io_buffer: u32, sample_rate: u32) -> f32 {
    if sample_rate == 0 {
        return 0.0;
    }
    let frames = device.0 + device.1 + 2 * io_buffer;
    frames as f32 * 1000.0 / sample_rate as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackKind;

    #[test]
    fn each_mode_sets_its_buffers_and_low_latency_mode() {
        let mut s = crate::UiState::new();
        s.modes = Modes::default();
        set_mode(&mut s, EngineMode::Mix);
        assert_eq!(
            (s.io_buffer, s.process_buffer, s.low_latency),
            (1024, 1024, false)
        );
        set_mode(&mut s, EngineMode::Record);
        assert_eq!(
            (s.io_buffer, s.process_buffer, s.low_latency),
            (64, 256, true)
        );
        // The menus change the current mode's values only.
        set_io_buffer(&mut s, 128);
        set_process_buffer(&mut s, 512);
        toggle(&mut s);
        assert_eq!(s.modes.mode, EngineMode::Mix);
        assert_eq!((s.io_buffer, s.process_buffer), (1024, 1024));
        set_io_buffer(&mut s, 512);
        toggle(&mut s);
        assert_eq!(
            (s.io_buffer, s.process_buffer, s.low_latency),
            (128, 512, true)
        );
        assert_eq!(s.modes.io, [128, 512]);
    }

    #[test]
    fn the_settings_are_saved_and_read_back() {
        let m = Modes {
            mode: EngineMode::Mix,
            io: [32, 512],
            process: [512, 256],
            auto_record: false,
            armed: Vec::new(),
        };
        assert_eq!(Modes::from_text(&m.to_text()), m);
        // Nothing saved, or nonsense: the defaults.
        assert_eq!(Modes::from_text(""), Modes::default());
        let odd = Modes::from_text("mode = loud\nrecord-io = 77\nmix-process = 9\n");
        assert_eq!(odd, Modes::default());
    }

    #[test]
    fn arming_a_track_in_mix_switches_to_record_unless_turned_off() {
        let mut p = Project::empty();
        let a = p.add_track(TrackKind::Audio);
        let mut s = crate::UiState::new();
        s.modes = Modes::default();
        set_mode(&mut s, EngineMode::Mix);
        follow_arming(&p, &mut s);
        assert_eq!(s.modes.mode, EngineMode::Mix);
        p.tracks[a].rec = true;
        follow_arming(&p, &mut s);
        assert_eq!(s.modes.mode, EngineMode::Record);
        assert!(s.low_latency && s.io_buffer == 64);
        assert!(s.status.is_some());
        // Turned off: arming leaves MIX alone.
        p.tracks[a].rec = false;
        follow_arming(&p, &mut s);
        set_mode(&mut s, EngineMode::Mix);
        toggle_auto_record(&mut s);
        s.status = None;
        p.tracks[a].rec = true;
        follow_arming(&p, &mut s);
        assert_eq!(s.modes.mode, EngineMode::Mix);
        assert!(s.status.is_none());
    }

    #[test]
    fn the_round_trip_is_both_device_delays_and_two_buffers() {
        assert!((round_trip_ms((24, 24), 64, 48_000) - 3.666_666_7).abs() < 1e-4);
        assert_eq!(round_trip_ms((0, 0), 64, 0), 0.0);
    }
}
