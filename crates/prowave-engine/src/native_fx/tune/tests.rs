//! Pro Wave Tune's tests: correction accuracy on sung-like tones with a known pitch, retune
//! timing, the MIDI target, formants, delay, presets and real-time safety. The measurements
//! for the report (CPU per block, real vocals) are the ignored tests at the end.
use super::tracker::tests::{cents, voice};
use super::*;
use crate::native_fx::test_signals::{noise, run_blocks};

const RATE: u32 = 48000;

/// An independent pitch measure for checking the output: the normalised squared difference
/// over the whole of `x` at the full rate, its first clear dip, refined with a parabola.
pub fn f0_of(x: &[f32], rate: f32, lo_hz: f32, hi_hz: f32) -> f32 {
    let tmin = (rate / hi_hz).floor().max(2.0) as usize;
    let tmax = (rate / lo_hz).ceil() as usize + 1;
    if x.len() < 2 * tmax + 2 {
        return 0.0;
    }
    let w = x.len() - tmax - 1;
    let d: Vec<f64> = (0..=tmax + 1)
        .map(|tau| {
            if tau < tmin.saturating_sub(1) {
                return 1.0;
            }
            let (mut num, mut den) = (0.0f64, 0.0f64);
            for j in 0..w {
                let (a, b) = (f64::from(x[j]), f64::from(x[j + tau]));
                num += (a - b) * (a - b);
                den += a * a + b * b;
            }
            if den > 0.0 {
                num / den
            } else {
                1.0
            }
        })
        .collect();
    let lowest = d[tmin..=tmax].iter().cloned().fold(f64::INFINITY, f64::min);
    let mut pick = 0;
    for tau in tmin..=tmax {
        if d[tau] <= lowest + 0.02 && d[tau] <= d[tau - 1] && d[tau] <= d[tau + 1] {
            pick = tau;
            break;
        }
    }
    if pick == 0 {
        return 0.0;
    }
    let (a, b, c) = (d[pick - 1], d[pick], d[pick + 1]);
    let den = a - 2.0 * b + c;
    let off = if den.abs() > 1e-15 {
        (0.5 * (a - c) / den).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    rate / (pick as f64 + off) as f32
}

fn hz_of(note: f32) -> f32 {
    scale::midi_to_hz(note, 440.0)
}

/// Runs a mono signal through `fx` in 64-frame blocks (each inside `assert_no_alloc`) and
/// returns the left output with the effect's delay taken off the front.
fn through(fx: &mut ProWaveTune, x: &[f32]) -> Vec<f32> {
    let (out, _) = run_blocks(fx, x, x, 64);
    out[fx.latency_samples().min(out.len())..].to_vec()
}

fn tune(values: &[(usize, f32)]) -> ProWaveTune {
    let mut fx = ProWaveTune::new(RATE, &[]);
    for (k, v) in values {
        fx.set_param(*k, *v);
    }
    fx
}

fn hard_chromatic() -> ProWaveTune {
    tune(&[
        (KEY, 1.0),
        (SCALE, scale::SCALE_CHROMATIC as f32),
        (RETUNE, 0.0),
        (VIBRATO, 0.0),
    ])
}

/// The output pitch in windows of `win` seconds every `step` seconds from `from` to `to`.
fn track_out(y: &[f32], from: f32, to: f32, win: f32, step: f32) -> Vec<(f32, f32)> {
    let r = RATE as f32;
    let mut out = Vec::new();
    let mut t = from;
    while t + win <= to {
        let a = (t * r) as usize;
        let b = ((t + win) * r) as usize;
        if b <= y.len() {
            out.push((t + win / 2.0, f0_of(&y[a..b], r, 110.0, 1000.0)));
        }
        t += step;
    }
    out
}

#[test]
fn hard_tune_puts_a_flat_note_on_pitch() {
    // A4 sung 30 cents flat, and C4 sung 18 cents sharp.
    for (note, off) in [(69.0f32, -0.30f32), (60.0, 0.18)] {
        let n = RATE as usize;
        let x = voice(&vec![hz_of(note + off); n], RATE as f32);
        let mut fx = hard_chromatic();
        let y = through(&mut fx, &x);
        let mut worst = 0.0f32;
        for (t, f) in track_out(&y, 0.2, 0.95, 0.05, 0.1) {
            let c = cents(f, hz_of(note));
            assert!(c.abs() < 5.0, "note {note} at {t}s: {c} cents off");
            worst = worst.max(c.abs());
        }
        println!("flat/sharp note {note} {off:+}: output within {worst:.2} cents");
    }
}

#[test]
fn hard_tune_flattens_vibrato() {
    // A3 with a 5.5 Hz vibrato of +-40 cents (wider would cross into the next note).
    let n = RATE as usize;
    let hz: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            hz_of(57.0 + 0.4 * (std::f32::consts::TAU * 5.5 * t).sin())
        })
        .collect();
    let x = voice(&hz, RATE as f32);
    let mut fx = hard_chromatic();
    let y = through(&mut fx, &x);
    let mut worst = 0.0f32;
    for (_, f) in track_out(&y, 0.2, 0.95, 0.03, 0.03) {
        worst = worst.max(cents(f, hz_of(57.0)).abs());
    }
    assert!(worst < 5.0, "vibrato left: {worst} cents");
    println!("vibrato +-40 cents at hard tune: output within {worst:.2} cents");
    // At a slow retune with Vibrato 100% the vibrato stays.
    let mut fx = tune(&[(KEY, 1.0), (SCALE, 7.0), (RETUNE, 400.0), (VIBRATO, 100.0)]);
    let y = through(&mut fx, &x);
    let spread = track_out(&y, 0.2, 0.95, 0.03, 0.03)
        .iter()
        .map(|(_, f)| cents(*f, hz_of(57.0)).abs())
        .fold(0.0f32, f32::max);
    assert!(
        spread > 25.0,
        "the vibrato should pass at slow retune: {spread}"
    );
}

#[test]
fn a_slide_is_held_on_the_notes_of_the_scale() {
    // A slide from A3 to C#4 over a second, corrected hard to C major: every window sits on
    // a note of C major within 5 cents (windows across a change of note are left out).
    let n = (RATE as f32 * 1.2) as usize;
    let hz: Vec<f32> = (0..n)
        .map(|i| hz_of(57.0 + 4.0 * (i as f32 / RATE as f32).min(1.0)))
        .collect();
    let x = voice(&hz, RATE as f32);
    let mut fx = tune(&[(KEY, 1.0), (SCALE, 1.0), (RETUNE, 0.0), (VIBRATO, 0.0)]);
    let y = through(&mut fx, &x);
    let c_major = scale::mask(1, 1, 0, 0, false);
    let mut seen = std::collections::BTreeSet::new();
    let mut on = 0;
    let mut total = 0;
    for (_, f) in track_out(&y, 0.1, 1.15, 0.02, 0.02) {
        let m = scale::hz_to_midi(f, 440.0);
        let near = m.round() as i32;
        total += 1;
        if (m - near as f32).abs() * 100.0 < 5.0 && c_major & (1 << near.rem_euclid(12)) != 0 {
            on += 1;
            seen.insert(near);
        }
    }
    // A3 B3 C4 (and D4 at the top) are each held; a few windows straddle a jump.
    assert!(
        seen.contains(&57) && seen.contains(&59) && seen.contains(&60),
        "{seen:?}"
    );
    assert!(
        on as f32 >= total as f32 * 0.8,
        "{on} of {total} windows on a note"
    );
    println!("slide: {on} of {total} 20 ms windows within 5 cents of a C major note");
}

/// The output pitch the logic asks for, every hop, on a steady tone.
fn asked(fx: &mut ProWaveTune, x: &[f32]) -> Vec<f32> {
    let hop = fx.hop();
    let mut out = Vec::new();
    for chunk in x.chunks(hop) {
        let mut l = chunk.to_vec();
        let mut r = chunk.to_vec();
        fx.run(&mut l, &mut r);
        out.push(fx.meter().read().1);
    }
    out
}

#[test]
fn transitions_follow_the_retune_speed() {
    // A3 sung 40 cents flat: the correction starts from the sung pitch and covers 95% of the
    // way in the Retune Speed.
    let x = voice(&vec![hz_of(56.6); RATE as usize / 2], RATE as f32);
    for retune in [0.0f32, 60.0, 200.0] {
        let mut fx = tune(&[(KEY, 1.0), (SCALE, 7.0), (RETUNE, retune), (VIBRATO, 100.0)]);
        let hop_ms = fx.hop() as f32 / RATE as f32 * 1000.0;
        let out = asked(&mut fx, &x);
        let Some(start) = out.iter().position(|v| v.is_finite()) else {
            panic!("never voiced")
        };
        let at = |ms: f32| out[start + (ms / hop_ms).round() as usize] - 56.6;
        if retune == 0.0 {
            assert!((at(0.0) - 0.4).abs() < 0.03, "{}", at(0.0));
        } else {
            // Partly there at a third of the time, 95% there at the time.
            let third = at(retune / 3.0) / 0.4;
            let full = at(retune) / 0.4;
            assert!(
                (third - 0.63).abs() < 0.12,
                "{retune} ms: {third} at a third"
            );
            assert!(
                (full - 0.95).abs() < 0.05,
                "{retune} ms: {full} at the time"
            );
            println!("retune {retune} ms: {third:.2} of the way at a third, {full:.2} at the time");
        }
    }
}

#[test]
fn humanize_slows_the_retune_on_held_notes() {
    let n = RATE as usize;
    let hz: Vec<f32> = (0..n)
        .map(|i| hz_of(57.0 + 0.3 * (std::f32::consts::TAU * 5.0 * i as f32 / RATE as f32).sin()))
        .collect();
    let x = voice(&hz, RATE as f32);
    let spread = |humanize: f32| {
        let mut fx = tune(&[
            (KEY, 1.0),
            (SCALE, 7.0),
            (RETUNE, 0.0),
            (HUMANIZE, humanize),
        ]);
        let out = asked(&mut fx, &x);
        let tail = &out[out.len() / 2..];
        tail.iter().map(|v| (v - 57.0).abs()).fold(0.0f32, f32::max)
    };
    assert!(spread(0.0) < 0.01);
    assert!(spread(100.0) > 0.1, "{}", spread(100.0));
}

#[test]
fn flex_tune_leaves_pitches_far_from_the_note() {
    // 40 cents flat with Flex-Tune 30 cents: not corrected; 10 cents flat: corrected.
    for (off, corrected) in [(-0.40f32, false), (-0.10, true)] {
        let x = voice(&vec![hz_of(57.0 + off); RATE as usize / 3], RATE as f32);
        let mut fx = tune(&[(KEY, 1.0), (SCALE, 7.0), (RETUNE, 0.0), (FLEX, 30.0)]);
        let out = asked(&mut fx, &x);
        let last = out[out.len() - 1];
        if corrected {
            assert!((last - 57.0).abs() < 0.03, "{off}: {last}");
        } else {
            assert!((last - (57.0 + off)).abs() < 0.03, "{off}: {last}");
        }
    }
}

#[test]
fn midi_mode_follows_the_notes() {
    // The voice holds 300 Hz; the MIDI notes say D4 for the first beat and F4 for the
    // second (120 bpm: half a second each).
    let n = RATE as usize;
    let x = voice(&vec![300.0; n], RATE as f32);
    let mut fx = tune(&[(TARGET, TARGET_MIDI as f32), (RETUNE, 0.0), (VIBRATO, 0.0)]);
    fx.set_targets(Arc::new(TuneTargets {
        notes: vec![
            TargetNote {
                start: 0.0,
                end: 1.0,
                pitch: 62,
            },
            TargetNote {
                start: 1.0,
                end: 2.0,
                pitch: 65,
            },
        ],
        ..TuneTargets::default()
    }));
    let mut out = vec![0.0f32; n];
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for (k, chunk) in x.as_chunks::<64>().0.iter().enumerate() {
        let start = k * 64;
        fx.block_transport(&BlockTransport {
            playing: true,
            beat: start as f64 / f64::from(RATE) * 2.0,
            tempo: 120.0,
            ..BlockTransport::default()
        });
        assert_no_alloc::assert_no_alloc(|| fx.process((&chunk[..], &chunk[..]), (&mut l, &mut r)));
        out[start..start + 64].copy_from_slice(&l);
    }
    let y = &out[fx.latency_samples()..];
    let first = f0_of(
        &y[(0.2 * 48000.0) as usize..(0.45 * 48000.0) as usize],
        48000.0,
        100.0,
        800.0,
    );
    let second = f0_of(
        &y[(0.7 * 48000.0) as usize..(0.95 * 48000.0) as usize],
        48000.0,
        100.0,
        800.0,
    );
    assert!(cents(first, hz_of(62.0)).abs() < 5.0, "D4: {first}");
    assert!(cents(second, hz_of(65.0)).abs() < 5.0, "F4: {second}");
    println!(
        "MIDI mode: {:+.2} and {:+.2} cents from the two notes",
        cents(first, hz_of(62.0)),
        cents(second, hz_of(65.0))
    );
    // Stopped: the scale is used instead (chromatic default is the project's C major: 300
    // Hz is nearest D4).
    let mut fx2 = tune(&[(TARGET, TARGET_MIDI as f32), (RETUNE, 0.0), (VIBRATO, 0.0)]);
    let out = asked(&mut fx2, &x[..RATE as usize / 3]);
    assert!((out[out.len() - 1] - 62.0).abs() < 0.01);
}

#[test]
fn pitch_override_wins_over_the_scale() {
    let x = voice(&vec![hz_of(57.0); RATE as usize / 3], RATE as f32);
    let mut fx = tune(&[(RETUNE, 0.0), (VIBRATO, 0.0), (OVERRIDE, 6450.0)]);
    let out = asked(&mut fx, &x);
    assert!(
        (out[out.len() - 1] - 64.5).abs() < 0.01,
        "{}",
        out[out.len() - 1]
    );
    // Off again: the scale.
    fx.set_param(OVERRIDE, 0.0);
    let out = asked(&mut fx, &x[..4800]);
    assert!((out[out.len() - 1] - 57.0).abs() < 0.01);
    // A value seen for one hop only (an automation step passing through) is not followed.
    fx.set_param(OVERRIDE, 3000.0);
    let hop = fx.hop();
    let out = asked(&mut fx, &x[..hop]);
    assert!((out[0] - 57.0).abs() < 0.01, "{}", out[0]);
    assert_eq!(override_text(6450), "E3 +50c");
    assert_eq!(override_text(6900), "A3");
    assert_eq!(override_text(0), "Off");
}

/// The spectral envelope of `x` as sampled by its harmonics: (Hz, dB) of each harmonic of
/// `f0` up to 4 kHz.
fn envelope(x: &[f32], f0: f32, rate: f32) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut k = 1;
    while k as f32 * f0 < 4000.0 {
        let f = k as f32 * f0;
        let w = std::f64::consts::TAU * f64::from(f) / f64::from(rate);
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, s) in x.iter().enumerate() {
            let hann = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / x.len() as f64).cos();
            re += f64::from(*s) * hann * (w * i as f64).cos();
            im += f64::from(*s) * hann * (w * i as f64).sin();
        }
        out.push((f, (10.0 * (re * re + im * im).max(1e-20).log10()) as f32));
        k += 1;
    }
    out
}

/// The envelope at `f` (linear between harmonics, in dB).
fn env_at(e: &[(f32, f32)], f: f32) -> f32 {
    let i = e.partition_point(|(h, _)| *h <= f).clamp(1, e.len() - 1);
    let ((f0, a), (f1, b)) = (e[i - 1], e[i]);
    a + (b - a) * ((f - f0) / (f1 - f0)).clamp(0.0, 1.0)
}

/// How different two envelopes are between 400 Hz and 2.5 kHz (mean absolute dB, levels
/// matched), `b` read with its frequencies multiplied by `stretch`.
fn env_distance(a: &[(f32, f32)], b: &[(f32, f32)], stretch: f32) -> f32 {
    let grid: Vec<f32> = (0..=42).map(|i| 400.0 + 50.0 * i as f32).collect();
    let d: Vec<f32> = grid
        .iter()
        .map(|f| env_at(a, *f) - env_at(b, f / stretch))
        .collect();
    let mean = d.iter().sum::<f32>() / d.len() as f32;
    d.iter().map(|x| (x - mean).abs()).sum::<f32>() / d.len() as f32
}

#[test]
fn formants_stay_put_when_the_pitch_moves_and_move_on_their_own() {
    let rate = RATE as f32;
    let x = voice(&vec![220.0; RATE as usize / 2], rate);
    let seg = |y: &[f32]| y[(0.2 * rate) as usize..(0.45 * rate) as usize].to_vec();
    let input = envelope(&seg(&x), 220.0, rate);
    // Up four semitones: the pitch moves, the envelope stays (it is nearer the input's own
    // than the input's moved up with the pitch, as plain resampling would give).
    let ratio = 2f32.powf(4.0 / 12.0);
    let mut fx = tune(&[(KEY, 1.0), (SCALE, 7.0), (RETUNE, 0.0), (TRANSPOSE, 4.0)]);
    let y = seg(&through(&mut fx, &x));
    let f = f0_of(&y, rate, 100.0, 800.0);
    assert!(cents(f, 220.0 * ratio).abs() < 5.0, "pitch {f}");
    let out = envelope(&y, f, rate);
    let kept = env_distance(&out, &input, 1.0);
    let moved = env_distance(&out, &input, ratio);
    assert!(
        kept < 3.0 && kept < moved * 0.6,
        "kept {kept} dB, moved {moved} dB"
    );
    // A formant shift of +5 semitones moves the envelope up that much and leaves the pitch.
    let up = 2f32.powf(5.0 / 12.0);
    let mut fx = tune(&[(KEY, 1.0), (SCALE, 7.0), (RETUNE, 0.0), (FORMANT, 500.0)]);
    let y = seg(&through(&mut fx, &x));
    let f = f0_of(&y, rate, 100.0, 800.0);
    assert!(cents(f, 220.0).abs() < 5.0, "pitch {f}");
    let out = envelope(&y, f, rate);
    let shifted = env_distance(&out, &input, up);
    let same = env_distance(&out, &input, 1.0);
    assert!(
        shifted < same * 0.6,
        "shifted {shifted} dB, unshifted {same} dB"
    );
    println!("envelope: +4 st kept {kept:.2} dB (moved {moved:.2}); formant +5 st {shifted:.2} dB (unshifted {same:.2})");
}

#[test]
fn unvoiced_sound_passes_delayed_and_untouched() {
    // Noise is not corrected: the output is the input, delayed by exactly the reported
    // latency.
    let x = noise(RATE as usize / 2, 0.3, 11);
    let mut fx = tune(&[(RETUNE, 0.0)]);
    let lat = fx.latency_samples();
    let (y, _) = run_blocks(&mut fx, &x, &x, 64);
    let err = x[..x.len() - lat]
        .iter()
        .zip(&y[lat..])
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(err < 1e-6, "noise changed by {err}");
}

#[test]
fn the_delay_is_reported_for_compensation() {
    let fx = ProWaveTune::new(RATE, &[]);
    let lat = fx.latency_seconds();
    assert!(lat > 0.0 && lat <= 0.010, "{lat}");
    assert_eq!(fx.latency_samples(), 384);
    let fx44 = ProWaveTune::new(44100, &[]);
    assert!((fx44.latency_seconds() - LATENCY_SECONDS).abs() < 1.0 / 44100.0);
    // In a track's effect chain it is part of the track's delay.
    let Some(block) = crate::open_native_block(ID, RATE, &[]) else {
        panic!("tune does not open")
    };
    let track = crate::PluginTrack::new(None, vec![block], 64);
    assert!((track.latency_seconds() - lat).abs() < 1e-9);
    assert!((track.effect_latencies()[0] - lat).abs() < 1e-9);
}

#[test]
fn parameters_change_while_it_runs_and_amount_zero_is_bypass() {
    // Automation sets parameters between blocks: Amount 0 leaves a flat note flat.
    let x = voice(&vec![hz_of(56.7); RATE as usize / 2], RATE as f32);
    let mut fx = hard_chromatic();
    assert_no_alloc::assert_no_alloc(|| fx.set_param(AMOUNT, 0.0));
    assert_eq!(fx.param(AMOUNT), 0.0);
    let y = through(&mut fx, &x);
    let f = f0_of(
        &y[RATE as usize / 5..RATE as usize * 2 / 5],
        RATE as f32,
        100.0,
        800.0,
    );
    assert!(cents(f, hz_of(56.7)).abs() < 5.0, "{f}");
    // Out of range values are kept in range, rounded.
    fx.set_param(RETUNE, 9999.0);
    assert_eq!(fx.param(RETUNE), 400.0);
    fx.set_param(FORMANT, 12.6);
    assert_eq!(fx.param(FORMANT), 13.0);
    // Through a track's chain, as automation reaches it.
    let Some(block) = crate::open_native_block(ID, RATE, &[]) else {
        panic!()
    };
    let mut track = crate::PluginTrack::new(None, vec![block], 64);
    track.set_effect_param(0, RETUNE, 123.0);
    track.push_param(0, 1, AMOUNT as u32, 50.0);
}

#[test]
fn presets_set_their_parameters_and_keep_the_key() {
    let Some(info) = crate::native_info(ID) else {
        panic!("not listed")
    };
    assert_eq!(info.presets.len(), 5);
    let names: Vec<&str> = info.presets.iter().map(|p| p.name).collect();
    assert_eq!(
        names,
        [
            "Hard Tune (trap)",
            "Natural",
            "Subtle",
            "Formant Up",
            "Robot"
        ]
    );
    for p in info.presets {
        let mut v = info.defaults();
        v[KEY] = 5.0;
        p.apply(&mut v);
        assert_eq!(v[KEY], 5.0, "{} keeps the key", p.name);
        let fx = ProWaveTune::new(RATE, &v);
        for (k, x) in p.values {
            assert_eq!(fx.param(*k), *x, "{} {}", p.name, PARAMS[*k].name);
            let q = &PARAMS[*k];
            assert!(q.min <= *x && *x <= q.max);
        }
    }
    let hard = &info.presets[0];
    assert!(hard.values.contains(&(RETUNE, 0.0)));
    for (k, p) in PARAMS.iter().enumerate() {
        assert!(!value_text(k, p.default as i32).is_empty());
    }
}

#[test]
fn targets_arrive_without_allocation_and_old_ones_go_back() {
    let (mut fx, mut remote) = open_tune(RATE, &[]);
    let t = Arc::new(TuneTargets {
        root: 4,
        minor: true,
        a4: 442.0,
        notes: vec![],
    });
    assert!(remote.send(t.clone()));
    let x = vec![0.0f32; 64];
    let mut l = x.clone();
    let mut r = x.clone();
    assert_no_alloc::assert_no_alloc(|| fx.process((&x, &x), (&mut l, &mut r)));
    assert_eq!(*fx.targets, *t);
    assert!(remote.send(Arc::new(TuneTargets::default())));
    assert_no_alloc::assert_no_alloc(|| fx.process((&x, &x), (&mut l, &mut r)));
    // The first targets came back to the app to be freed there.
    assert!(remote.send(Arc::new(TuneTargets::default())));
    assert_eq!(
        Arc::strong_count(&t),
        1,
        "the old targets were freed on the app's side"
    );
    assert!(!remote.is_abandoned());
    drop(fx);
    assert!(remote.is_abandoned());
}

#[test]
fn notes_are_found_by_beat() {
    let t = TuneTargets {
        notes: vec![
            TargetNote {
                start: 0.0,
                end: 4.0,
                pitch: 60,
            },
            TargetNote {
                start: 1.0,
                end: 2.0,
                pitch: 64,
            },
        ],
        ..TuneTargets::default()
    };
    assert_eq!(t.note_at(0.5), Some(60));
    assert_eq!(t.note_at(1.5), Some(64));
    assert_eq!(t.note_at(2.5), Some(60));
    assert_eq!(t.note_at(4.5), None);
    assert_eq!(t.note_at(-1.0), None);
}

// ------------------------------------------------------------------ measurements

/// CPU per 64-frame block and the delay, for the report: run with
/// `cargo test --release -p prowave-engine tune_cpu -- --ignored --nocapture`.
#[test]
#[ignore = "measurement"]
fn tune_cpu_per_block() {
    for (name, values) in [
        ("hard tune", vec![(RETUNE, 0.0f32)]),
        (
            "formant +5 st, transpose +7",
            vec![(FORMANT, 500.0), (TRANSPOSE, 7.0)],
        ),
        ("Low Male voice", vec![(VOICE, 2.0)]),
    ] {
        let mut fx = tune(&values);
        let n = RATE as usize * 10;
        let hz: Vec<f32> = (0..n)
            .map(|i| hz_of(55.0 + 7.0 * ((i / 24000) % 3) as f32))
            .collect();
        let x = voice(&hz, RATE as f32);
        let mut l = vec![0.0f32; 64];
        let mut r = vec![0.0f32; 64];
        let mut worst = 0u128;
        let started = std::time::Instant::now();
        for chunk in x.as_chunks::<64>().0 {
            let t = std::time::Instant::now();
            fx.process((&chunk[..], &chunk[..]), (&mut l, &mut r));
            worst = worst.max(t.elapsed().as_nanos());
        }
        let blocks = (n / 64) as f64;
        let mean = started.elapsed().as_secs_f64() / blocks * 1e6;
        let budget = 64.0 / f64::from(RATE) * 1e6;
        println!(
            "{name}: mean {mean:.2} us per 64-frame block ({:.2}% of the block's {budget:.0} us), worst {:.1} us; latency {:.2} ms",
            mean / budget * 100.0,
            worst as f64 / 1000.0,
            fx.latency_seconds() * 1000.0
        );
    }
}

/// Correction accuracy and artifacts on real lead vocals, for the report. `PROWAVE_TUNE_STEMS`
/// names a folder of song folders holding `lead.flac`; outputs go to `PROWAVE_TUNE_OUT`.
/// Hard tune, chromatic, A440.
///
/// Accuracy: the output's pitch (an independent measure over 20 ms) against the note the
/// effect aimed at, in windows where that note held still (a window across a change of note,
/// or while the input is not voiced, is left out). Artifacts: the level change, how much the
/// unvoiced parts changed, and output sample jumps far beyond the input's largest (clicks).
#[test]
#[ignore = "measurement"]
fn tune_real_vocals() {
    let Ok(dir) = std::env::var("PROWAVE_TUNE_STEMS") else {
        println!("PROWAVE_TUNE_STEMS not set");
        return;
    };
    let out_dir = std::env::var("PROWAVE_TUNE_OUT").ok();
    let max_songs: usize = std::env::var("PROWAVE_TUNE_SONGS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        panic!("no folder {dir}")
    };
    let mut songs: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path().join("lead.flac"))
        .filter(|p| p.exists())
        .collect();
    songs.sort();
    let mut all = Vec::new();
    for path in songs.into_iter().take(max_songs) {
        let Ok(file) = crate::AudioFile::open(&path) else {
            continue;
        };
        let Ok(st) = file.read_stereo() else { continue };
        let rate = file.sample_rate;
        // 40 seconds from a third of the way in.
        let mono: Vec<f32> = st.chunks(2).map(|c| 0.5 * (c[0] + c[1])).collect();
        let a = mono.len() / 3;
        let b = (a + rate as usize * 40).min(mono.len());
        let x = &mono[a..b];
        let mut fx = ProWaveTune::new(rate, &[]);
        for (k, v) in [(KEY, 1.0), (SCALE, 7.0), (RETUNE, 0.0), (VIBRATO, 0.0)] {
            fx.set_param(k, v);
        }
        let hop = fx.hop();
        let mut y = Vec::with_capacity(x.len());
        let mut targets = Vec::with_capacity(x.len() / hop + 1);
        let started = std::time::Instant::now();
        for c in x.chunks(hop) {
            let (mut l, mut r) = (c.to_vec(), c.to_vec());
            fx.run(&mut l, &mut r);
            y.extend_from_slice(&l);
            targets.push(fx.meter().read().2);
        }
        let secs = started.elapsed().as_secs_f64();
        let lat = fx.latency_samples();
        let y = &y[lat..];
        let x = &x[..y.len()];
        let r = report(x, y, rate, &targets, hop, lat);
        let name = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        println!(
            "{name}: voiced {:.0}% of hops, {} windows on a held note: within 5 cents {:.1}%, within 10 cents {:.1}%, median {:.2} cents; note changes {:.1}/s voiced; level {:+.2} dB; unvoiced parts changed by {:.1} dB; clicks {}; {:.0}x real time",
            r.voiced * 100.0,
            r.windows,
            r.within5 * 100.0,
            r.within10 * 100.0,
            r.median,
            r.changes,
            r.level_db,
            r.unvoiced_db,
            r.clicks,
            (x.len() as f64 / f64::from(rate)) / secs,
        );
        if let Some(o) = &out_dir {
            let file = name.replace(' ', "_");
            let _ = write_wav(&format!("{o}/{file}-in.wav"), x, rate);
            let _ = write_wav(&format!("{o}/{file}-tuned.wav"), y, rate);
        }
        all.push(r);
    }
    if !all.is_empty() {
        let w: usize = all.iter().map(|r| r.windows).sum();
        let wsum = |f: fn(&Report) -> f32| {
            all.iter().map(|r| f(r) * r.windows as f32).sum::<f32>() / w.max(1) as f32
        };
        println!(
            "ALL {} songs, {w} windows: within 5 cents {:.1}%, within 10 cents {:.1}%, median {:.2} cents; clicks {}",
            all.len(),
            wsum(|r| r.within5) * 100.0,
            wsum(|r| r.within10) * 100.0,
            wsum(|r| r.median),
            all.iter().map(|r| r.clicks).sum::<usize>(),
        );
    }
}

struct Report {
    voiced: f32,
    windows: usize,
    within5: f32,
    within10: f32,
    median: f32,
    changes: f32,
    level_db: f32,
    unvoiced_db: f32,
    clicks: usize,
}

fn report(x: &[f32], y: &[f32], rate: u32, targets: &[f32], hop: usize, lat: usize) -> Report {
    let r = rate as f32;
    let win = (0.02 * r) as usize;
    let step = (0.01 * r) as usize;
    let voiced_hops = targets.iter().filter(|t| t.is_finite()).count();
    let changes = targets
        .windows(2)
        .filter(|w| w[0].is_finite() && w[1].is_finite() && w[0] != w[1])
        .count();
    if std::env::var("PROWAVE_TUNE_DEBUG").is_ok() {
        let mut h = std::collections::BTreeMap::new();
        for w in targets.windows(2) {
            if w[0].is_finite() && w[1].is_finite() && w[0] != w[1] {
                *h.entry((w[1] - w[0]).round() as i32).or_insert(0) += 1;
            }
        }
        println!("  jumps {h:?}");
    }
    let mut errs = Vec::new();
    let (mut un_err, mut un_sig) = (0.0f64, 0.0f64);
    let margin = (0.003 * r) as usize;
    let mut at = 0;
    while at + win + lat + margin < x.len() {
        // The output here was made with the controls of the input `lat` samples later.
        let h0 = (at + lat).saturating_sub(margin) / hop;
        let h1 = (at + win + lat + margin) / hop;
        let span = &targets[h0.min(targets.len())..(h1 + 1).min(targets.len())];
        let held = span
            .first()
            .copied()
            .filter(|t| t.is_finite() && span.iter().all(|s| s == t));
        if let Some(n) = held {
            let want = scale::midi_to_hz(n, 440.0);
            let fo = f0_of(&y[at..at + win], r, want * 0.75, want * 1.33);
            if fo > 0.0 {
                errs.push(cents(fo, want).abs());
            }
        } else if span.iter().all(|t| !t.is_finite()) {
            for (a, b) in x[at..at + win].iter().zip(&y[at..at + win]) {
                un_err += f64::from(a - b).powi(2);
                un_sig += f64::from(*a).powi(2);
            }
        }
        at += step;
    }
    let n = errs.len().max(1) as f32;
    let within5 = errs.iter().filter(|c| **c < 5.0).count() as f32 / n;
    let within10 = errs.iter().filter(|c| **c < 10.0).count() as f32 / n;
    errs.sort_by(|a, b| a.total_cmp(b));
    let median = errs.get(errs.len() / 2).copied().unwrap_or(0.0);
    let rms = |v: &[f32]| {
        (v.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt()
    };
    let level_db = (20.0 * (rms(y) / rms(x).max(1e-12)).log10()) as f32;
    let unvoiced_db = (10.0 * (un_err.max(1e-20) / un_sig.max(1e-20)).log10()) as f32;
    let jump = |v: &[f32]| {
        v.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let limit = jump(x) * 1.5;
    let clicks = y.windows(2).filter(|w| (w[1] - w[0]).abs() > limit).count();
    Report {
        voiced: voiced_hops as f32 / targets.len().max(1) as f32,
        windows: errs.len(),
        within5,
        within10,
        median,
        changes: changes as f32 / (voiced_hops as f32 * hop as f32 / r).max(1e-3),
        level_db,
        unvoiced_db,
        clicks,
    }
}

fn write_wav(path: &str, x: &[f32], rate: u32) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data = (x.len() * 2) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * 2).to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data.to_le_bytes())?;
    for s in x {
        f.write_all(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
    }
    Ok(())
}

/// PSOLA against the alternatives, for the report: the phase vocoder already in the engine
/// (Signalsmith Stretch, formants compensated) at its cheaper preset and at a small block,
/// and plain resampling through a varying delay (no formant preservation, the classic
/// "harmonizer"). A sung-like A3 is shifted up 4 semitones; each is measured for delay,
/// CPU per 64-frame block, output pitch and how far its envelope moved.
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "measurement"]
fn tune_shifter_alternatives() {
    let rate = RATE as f32;
    let ratio = 2f32.powf(4.0 / 12.0);
    let x = voice(&vec![220.0; RATE as usize * 2], rate);
    let input = envelope(&x[RATE as usize / 2..RATE as usize], 220.0, rate);
    let judge = |name: &str, y: &[f32], latency: usize, us: f64| {
        let seg = &y[latency + RATE as usize / 2..latency + RATE as usize];
        let f = f0_of(seg, rate, 150.0, 600.0);
        let env = envelope(seg, f.max(1.0), rate);
        println!(
            "{name}: latency {:.1} ms, {us:.1} us per 64-frame block, pitch {:+.1} cents from target, envelope {:.2} dB from the input's (moved with pitch {:.2} dB)",
            latency as f32 / rate * 1000.0,
            cents(f, 220.0 * ratio),
            env_distance(&env, &input, 1.0),
            env_distance(&env, &input, ratio),
        );
    };
    let time = |f: &mut dyn FnMut(&[f32], &mut [f32])| {
        let mut y = vec![0.0f32; x.len()];
        let started = std::time::Instant::now();
        for (a, b) in x.chunks(64).zip(y.chunks_mut(64)) {
            f(a, b);
        }
        let us = started.elapsed().as_secs_f64() * 1e6 / (x.len() / 64) as f64;
        (y, us)
    };
    // PSOLA (this effect).
    let mut fx = tune(&[(KEY, 1.0), (SCALE, 7.0), (RETUNE, 0.0), (TRANSPOSE, 4.0)]);
    let lat = fx.latency_samples();
    let (y, us) = time(&mut |a, b| {
        let mut r = a.to_vec();
        fx.process((a, a), (b, &mut r));
    });
    judge("PSOLA (Pro Wave Tune)", &y, lat, us);
    // Signalsmith Stretch.
    for (name, mut s) in [
        (
            "phase vocoder, cheaper preset",
            signalsmith_stretch::Stretch::preset_cheaper(1, RATE),
        ),
        (
            "phase vocoder, 1024/256 block",
            signalsmith_stretch::Stretch::new(1, 1024, 256),
        ),
    ] {
        s.set_transpose_factor(ratio, None);
        s.set_formant_factor(1.0, true);
        let lat = s.input_latency() + s.output_latency();
        let (y, us) = time(&mut |a, b| s.process(a, b));
        judge(name, &y, lat, us);
    }
    // Resampling through a delay line with two crossfaded read heads (a 30 ms window).
    let win = 0.03 * rate;
    let mut buf = vec![0.0f32; 1 << 14];
    let mut w = 0usize;
    let mut phase = 0.0f32;
    let (y, us) = time(&mut |a, b| {
        for (i, s) in a.iter().enumerate() {
            buf[w & 16383] = *s;
            phase = (phase + (1.0 - ratio) / win).rem_euclid(1.0);
            let mut out = 0.0;
            for k in 0..2 {
                let ph = (phase + 0.5 * k as f32) % 1.0;
                let pos = w as f32 - (ph * win + 2.0);
                let j = pos.floor();
                let t = pos - j;
                let j = j as usize;
                let v = buf[j & 16383] * (1.0 - t) + buf[(j + 1) & 16383] * t;
                out += v * (std::f32::consts::PI * ph).sin().powi(2);
            }
            b[i] = out;
            w += 1;
        }
    });
    judge("resampling delay line", &y, (win / 2.0) as usize, us);
}

/// The delay of the corrected sound itself (not only the reported latency), for the report:
/// a voice whose level jumps every 40 ms goes through the effect (corrected to its own
/// note), and the lag at which the output's level follows the input's best is measured.
#[test]
#[ignore = "measurement"]
fn tune_measured_delay() {
    let rate = RATE as f32;
    for hz in [98.0f32, 147.0, 220.0, 440.0] {
        let n = RATE as usize * 2;
        let mut level = 0.5f32;
        let mut seed = 12345u32;
        let env: Vec<f32> = (0..n)
            .map(|i| {
                if i % (RATE as usize / 25) == 0 {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    level = 0.2 + 0.8 * (seed as f32 / u32::MAX as f32);
                }
                level
            })
            .collect();
        let x: Vec<f32> = voice(&vec![hz; n], rate)
            .iter()
            .zip(&env)
            .map(|(a, b)| a * b)
            .collect();
        let mut fx = tune(&[(KEY, 1.0), (SCALE, 7.0), (RETUNE, 0.0), (VOICE, 3.0)]);
        let (y, _) = run_blocks(&mut fx, &x, &x, 64);
        // Level envelopes (1 ms smoothing of the rectified sound).
        let smooth = |v: &[f32]| {
            let k = 1.0 - (-1.0 / (0.001 * rate)).exp();
            let mut s = 0.0f32;
            v.iter()
                .map(|a| {
                    s += k * (a.abs() - s);
                    s
                })
                .collect::<Vec<f32>>()
        };
        let (ex, ey) = (smooth(&x), smooth(&y));
        let from = RATE as usize / 4;
        let to = n - RATE as usize / 10;
        let mut best = (0usize, f64::MIN);
        for lag in 0..(0.04 * rate) as usize {
            let mut c = 0.0f64;
            for i in (from..to).step_by(4) {
                c += f64::from(ex[i]) * f64::from(ey[i + lag]);
            }
            if c > best.1 {
                best = (lag, c);
            }
        }
        println!(
            "{hz} Hz voice: corrected sound follows the input {:.1} ms later (reported latency {:.1} ms)",
            best.0 as f32 / rate * 1000.0,
            fx.latency_seconds() * 1000.0
        );
    }
}
