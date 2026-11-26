//! Split to Tracks (the Channel Rack's key, `rack.split_tracks`): each channel of a rack that
//! has a sample goes onto an instrument track of its own, named for its class, right under the rack's track, playing
//! a Channel Rack that holds only that channel (its sample, level, pan, mute and class, with the
//! rack's groove, amounts and loaded pattern's hits of that channel). The channel's notes move
//! there unchanged, in regions that start and end where the rack's do, on the new rack's
//! first channel. The rack's track stays, muted, named "<name> (split)". One call, one Undo
//! step.
use crate::channel_rack::{self, LoadedPattern, RackState};
use crate::i18n::tr;
use crate::kit_classify::DrumClass;
use crate::model::{Project, Region, Track, TrackKind};
use crate::plugins::PluginSlot;
use prowave_engine::RACK_BASE_NOTE;

/// One track made by [`split`]: its index, the rack channel (from 0) it holds, its name and
/// the notes moved onto it.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitTrack {
    pub track: usize,
    pub channel: usize,
    pub name: String,
    pub notes: usize,
}

/// A region whose notes the rack plays as they are: MIDI, not generated, not a take folder.
fn is_plain(r: &Region) -> bool {
    !r.is_audio() && r.generated.is_none() && r.folder.is_none()
}

/// The name of a drum class on a track (None: a class without one; the channel's name serves).
fn class_name(c: DrumClass) -> Option<&'static str> {
    Some(match c {
        DrumClass::Kick => tr("Kick"),
        DrumClass::Snare => tr("Snare"),
        DrumClass::Clap => tr("Clap"),
        DrumClass::ClosedHat => tr("Closed Hat"),
        DrumClass::OpenHat => tr("Open Hat"),
        DrumClass::Rim => tr("Rim"),
        DrumClass::TomLow => tr("Low Tom"),
        DrumClass::TomMid => tr("Mid Tom"),
        DrumClass::TomHigh => tr("High Tom"),
        DrumClass::Perc => tr("Perc"),
        DrumClass::Shaker => tr("Shaker"),
        DrumClass::Crash => tr("Crash"),
        DrumClass::Ride => tr("Ride"),
        _ => return None,
    })
}

/// What each channel in `chans` is called on its track: its class's name ("Kick", a second
/// one "Kick 2"), else the channel's own name (its sample's).
fn track_names(st: &RackState, chans: &[usize]) -> Vec<String> {
    let base: Vec<String> = chans
        .iter()
        .map(|&c| {
            let ch = &st.channels[c];
            match class_name(ch.class) {
                Some(n) => n.to_string(),
                None if !ch.name.trim().is_empty() => ch.name.clone(),
                None => ch.class.tag().to_string(),
            }
        })
        .collect();
    base.iter()
        .enumerate()
        .map(|(k, n)| {
            let nth = base[..k].iter().filter(|b| *b == n).count();
            if nth == 0 {
                n.clone()
            } else {
                format!("{n} {}", nth + 1)
            }
        })
        .collect()
}

/// The rack of channel `c` of `st` alone, on its first channel.
fn one_channel(st: &RackState, c: usize) -> RackState {
    RackState {
        channels: vec![st.channels[c].clone()],
        kit: st.kit.clone(),
        kit_dir: st.kit_dir.clone(),
        groove: st.groove.clone(),
        velocity: st.velocity,
        velocity_amount: st.velocity_amount,
        pattern: LoadedPattern {
            name: st.pattern.name.clone(),
            source: st.pattern.source.clone(),
            steps: st.pattern.steps,
            hits: st
                .pattern
                .hits
                .iter()
                .filter(|h| h.channel == c)
                .map(|h| channel_rack::PatternHit { channel: 0, ..*h })
                .collect(),
            ..st.pattern.clone()
        },
    }
}

/// Splits the rack of track `ti` (see the module). Returns the tracks made, in order; empty
/// (and nothing changed) when the track plays no rack or no channel has a sample.
pub fn split(p: &mut Project, ti: usize) -> Vec<SplitTrack> {
    let Some(st) = channel_rack::rack_state(p, ti) else {
        return Vec::new();
    };
    let chans: Vec<usize> = (0..st.channels.len())
        .filter(|&c| !st.channels[c].path.is_empty())
        .collect();
    if chans.is_empty() {
        return Vec::new();
    }
    let names = track_names(&st, &chans);
    let mut made = Vec::new();
    for (k, (&c, name)) in chans.iter().zip(names).enumerate() {
        let pitch = RACK_BASE_NOTE + c as u8;
        let src = &mut p.tracks[ti];
        let mut regions = Vec::new();
        let mut moved = 0;
        for r in src.regions.iter_mut().filter(|r| is_plain(r)) {
            if !r.notes.iter().any(|n| n.pitch == pitch) {
                continue;
            }
            let (mine, rest): (Vec<_>, Vec<_>) = r.notes.drain(..).partition(|n| n.pitch == pitch);
            r.notes = rest;
            moved += mine.len();
            let mut copy = r.clone();
            copy.name = name.clone();
            copy.notes = mine
                .into_iter()
                .map(|mut n| {
                    n.pitch = RACK_BASE_NOTE;
                    n
                })
                .collect();
            regions.push(copy);
        }
        // A clean channel strip with the rack track's level, pan, routing and sends.
        let mut t = Track::new(name.clone(), TrackKind::Instrument);
        t.color = src.color;
        t.output = src.output;
        t.volume_db = src.volume_db;
        t.pan = src.pan;
        t.sends = src.sends.clone();
        t.params = src.params;
        // There is one Groove Track; the new tracks follow it as the rack's does.
        t.params.groove = false;
        t.group = src.group;
        t.indent = src.indent;
        t.regions = regions;
        let mut slot = PluginSlot::new(channel_rack::RACK_ID, tr("Channel Rack"));
        slot.state = Some(one_channel(&st, c).bytes());
        t.au_instrument = Some(slot);
        let at = ti + 1 + k;
        p.tracks.insert(at, t);
        p.vca_inserted(at);
        // The shift above moved the new track's VCA too; the rack's is the right one.
        p.tracks[at].vca = p.tracks[ti].vca;
        if p.selected_track >= at {
            p.selected_track += 1;
        }
        made.push(SplitTrack {
            track: at,
            channel: c,
            name,
            notes: moved,
        });
    }
    let src = &mut p.tracks[ti];
    src.mute = true;
    src.name = format!("{} (split)", src.name);
    p.song_rev += 1;
    made
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_rack::{load_kit, PatternHit};
    use crate::model::{Note, RegionParams};
    use crate::rack_groove::Cell;

    fn note(start: f32, len: f32, pitch: u8, vel: u8) -> Note {
        Note {
            start,
            len,
            pitch,
            vel,
            mute: false,
            lyric: crate::lyrics::Lyric::NONE,
        }
    }

    fn region(start_bar: f32, len_bars: f32, notes: Vec<Note>) -> Region {
        Region {
            missing: None,
            name: "Beat".into(),
            start_bar,
            len_bars,
            notes,
            controls: Vec::new(),
            folder: None,
            generated: None,
            audio: None,
            params: RegionParams::default(),
        }
    }

    /// A rack with a kick, a snare, an empty channel and a hat, two regions of notes and an
    /// 808 track below it.
    fn rack() -> (Project, usize) {
        let mut p = Project::empty();
        let pads = vec![
            Some(std::path::PathBuf::from("/lib/k/Kick.wav")),
            Some(std::path::PathBuf::from("/lib/k/Snare.wav")),
            None,
            Some(std::path::PathBuf::from("/lib/k/Hat.wav")),
        ];
        let classes = [
            DrumClass::Kick,
            DrumClass::Snare,
            DrumClass::Other,
            DrumClass::ClosedHat,
        ];
        let ti = load_kit(&mut p, None, "K", &pads, &classes);
        p.tracks[ti].name = "Drums".into();
        let mut st = channel_rack::rack_state(&p, ti).unwrap();
        st.channels[1].gain_db = -6.0;
        st.channels[1].pan = 0.25;
        st.channels[3].mute = true;
        st.groove.swing_amount = 0.5;
        st.velocity_amount = 0.75;
        st.pattern = LoadedPattern {
            name: "P".into(),
            source: "test".into(),
            steps: 16,
            hits: vec![
                PatternHit {
                    channel: 1,
                    cell: Cell::new(4, 0, 1),
                    vel: 90,
                    base: 100,
                    played: Some(0.013),
                },
                PatternHit {
                    channel: 0,
                    cell: Cell::new(0, 0, 1),
                    vel: 120,
                    base: 100,
                    played: None,
                },
            ],
            ..LoadedPattern::default()
        };
        p.tracks[ti].au_instrument.as_mut().unwrap().state = Some(st.bytes());
        let b = RACK_BASE_NOTE;
        p.tracks[ti].regions = vec![
            region(
                1.0,
                1.0,
                vec![
                    note(0.0, 0.25, b, 120),
                    note(0.013 + 1.0, 0.2, b + 1, 77),
                    note(0.5123, 0.1, b + 3, 64),
                    note(1.75, 0.25, b, 101),
                    note(2.0, 0.25, b + 2, 50),
                ],
            ),
            region(5.0, 2.0, vec![note(3.333, 0.5, b + 3, 33)]),
        ];
        let bi = p.add_track(TrackKind::Instrument);
        p.tracks[bi].name = channel_rack::BASS_TRACK.into();
        p.tracks[bi].regions = vec![region(1.0, 1.0, vec![note(0.0, 1.0, 24, 100)])];
        p.selected_track = ti;
        (p, ti)
    }

    #[test]
    fn each_channel_with_a_sample_goes_onto_its_own_track_with_its_notes_as_they_were() {
        let (mut p, ti) = rack();
        let before = p.tracks[ti].clone();
        let made = split(&mut p, ti);
        let names: Vec<&str> = made.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["Kick", "Snare", "Closed Hat"]);
        assert_eq!(
            made.iter()
                .map(|m| (m.track, m.channel, m.notes))
                .collect::<Vec<_>>(),
            [(ti + 1, 0, 2), (ti + 2, 1, 1), (ti + 3, 3, 2)]
        );
        let names: Vec<&str> = p.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            &names[ti..],
            ["Drums (split)", "Kick", "Snare", "Closed Hat", "808"]
        );
        assert!(p.tracks[ti].mute);
        assert_eq!(p.selected_track, ti);
        // The rack keeps the empty channel's note; the others moved.
        assert_eq!(p.tracks[ti].regions[0].notes.len(), 1);
        assert_eq!(p.tracks[ti].regions[0].notes[0].pitch, RACK_BASE_NOTE + 2);
        assert!(p.tracks[ti].regions[1].notes.is_empty());
        // Each note exactly where it was, on the new rack's first channel.
        for m in &made {
            let t = &p.tracks[m.track];
            assert!(!t.mute);
            let pitch = RACK_BASE_NOTE + m.channel as u8;
            let mut got = Vec::new();
            for r in &t.regions {
                let src = before
                    .regions
                    .iter()
                    .find(|s| s.start_bar == r.start_bar && s.len_bars == r.len_bars)
                    .unwrap();
                let want: Vec<_> = src.notes.iter().filter(|n| n.pitch == pitch).collect();
                assert_eq!(r.notes.len(), want.len());
                for (n, w) in r.notes.iter().zip(want) {
                    assert_eq!(n.pitch, RACK_BASE_NOTE);
                    assert_eq!(n.start.to_bits(), w.start.to_bits());
                    assert_eq!(n.len.to_bits(), w.len.to_bits());
                    assert_eq!(n.vel, w.vel);
                }
                got.extend(r.notes.iter().map(|n| n.start));
            }
            assert_eq!(got.len(), m.notes);
        }
        // The hat has notes in both regions; the snare only in the first.
        assert_eq!(p.tracks[ti + 3].regions.len(), 2);
        assert_eq!(p.tracks[ti + 2].regions.len(), 1);
        // Each new rack holds its channel alone, with the groove and its pattern's hits.
        let old = channel_rack::RackState::read(&before.au_instrument.as_ref().unwrap().state);
        for m in &made {
            let st = channel_rack::rack_state(&p, m.track).unwrap();
            assert_eq!(st.channels, vec![old.channels[m.channel].clone()]);
            assert_eq!(st.groove, old.groove);
            assert_eq!(st.velocity_amount, 0.75);
            let hits: Vec<_> = old
                .pattern
                .hits
                .iter()
                .filter(|h| h.channel == m.channel)
                .map(|h| PatternHit { channel: 0, ..*h })
                .collect();
            assert_eq!(st.pattern.hits, hits);
        }
        // The 808 track is left alone.
        let bass = p.tracks.last().unwrap();
        assert_eq!(bass.regions[0].notes.len(), 1);
        assert!(!bass.mute);
    }

    #[test]
    fn a_split_is_one_undo_step() {
        let (mut p, ti) = rack();
        let mut h = crate::undo::History::new(&p);
        let before = p.clone();
        assert_eq!(split(&mut p, ti).len(), 3);
        assert!(h.record(&p));
        assert!(h.undo(&mut p));
        assert!(!h.can_undo());
        assert_eq!(p.tracks.len(), before.tracks.len());
        for (a, b) in p.tracks.iter().zip(&before.tracks) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.mute, b.mute);
            assert_eq!(a.regions.len(), b.regions.len());
            for (ra, rb) in a.regions.iter().zip(&b.regions) {
                assert_eq!(ra.notes, rb.notes);
            }
        }
    }

    #[test]
    fn tracks_take_their_class_names_and_a_second_of_a_class_is_numbered() {
        let mut st = RackState::default();
        for (name, class) in [
            ("k1", DrumClass::Kick),
            ("k2", DrumClass::Kick),
            ("boom", DrumClass::Bass808),
            ("", DrumClass::Other),
        ] {
            let mut ch = channel_rack::RackChannel::empty(class);
            ch.name = name.into();
            st.channels.push(ch);
        }
        assert_eq!(
            track_names(&st, &[0, 1, 2, 3]),
            ["Kick", "Kick 2", "boom", DrumClass::Other.tag()]
        );
    }

    #[test]
    fn a_track_without_a_rack_or_samples_is_not_split() {
        let mut p = Project::empty();
        let i = p.add_track(TrackKind::Instrument);
        let n = p.tracks.len();
        assert!(split(&mut p, i).is_empty());
        assert_eq!(p.tracks.len(), n);
    }
}
