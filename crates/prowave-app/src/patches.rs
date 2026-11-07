//! The sound the engine's built-in synthesizer plays for a track: none.
//!
//! Pro Wave offers no built-in instruments. An instrument track plays its notes only through
//! an instrument the user loads: a SoundFont (`.sf2`) today, a third-party Audio Unit once
//! plug-in hosting is wired into the channel strip's instrument slot
//! (`mixer::instrument_slot`). Until then the track is silent, so it never sounds like an
//! instrument that is not there. The engine's synth code is left in place; it is only sent
//! a patch with no level.
use crate::model::Track;
use prowave_engine::SynthPatch;

/// The built-in synth patch for `t`: silent for every track.
pub fn synth_patch(_t: &Track) -> SynthPatch {
    SynthPatch {
        gain: 0.0,
        ..SynthPatch::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Project;

    #[test]
    fn no_track_plays_a_built_in_instrument() {
        for t in &Project::demo().tracks {
            assert_eq!(synth_patch(t).gain, 0.0, "{}", t.name);
        }
    }

    /// The demo song's notes through the silent patch: no sample and no meter goes NaN or
    /// infinite, and nothing is heard.
    #[test]
    fn instrument_tracks_without_an_instrument_are_silent() {
        let mut p = Project::demo();
        p.tracks
            .retain(|t| t.kind != crate::model::TrackKind::Audio);
        for t in &mut p.tracks {
            t.inserts.clear();
        }
        let r = crate::bench::render(&p, 3.0);
        assert_eq!(r.non_finite, 0);
        assert!(r.bad_meters.is_empty());
        assert_eq!(r.peak, 0.0, "no built-in instrument is heard");
    }
}
