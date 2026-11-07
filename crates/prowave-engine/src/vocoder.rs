//! The WORLD vocoder (Masanori Morise, modified BSD; the C++ is in `world/`, built by
//! `build.rs`). It takes a voice apart into its pitch (f0), its spectral envelope (the
//! formants: the shape of the mouth) and its aperiodicity (breath and noise), and puts it back
//! together from them, so each can be changed on its own. The Pitch Editor resynthesizes the
//! notes it changes with it (see `note_render.rs`). Offline only: nothing here runs on the
//! audio thread.
use std::os::raw::c_int;

#[repr(C)]
struct HarvestOption {
    f0_floor: f64,
    f0_ceil: f64,
    frame_period: f64,
}

#[repr(C)]
struct CheapTrickOption {
    q1: f64,
    f0_floor: f64,
    fft_size: c_int,
}

#[repr(C)]
struct D4COption {
    threshold: f64,
}

extern "C" {
    fn InitializeHarvestOption(option: *mut HarvestOption);
    fn GetSamplesForHarvest(fs: c_int, x_length: c_int, frame_period: f64) -> c_int;
    fn Harvest(
        x: *const f64,
        x_length: c_int,
        fs: c_int,
        option: *const HarvestOption,
        temporal_positions: *mut f64,
        f0: *mut f64,
    );
    fn StoneMask(
        x: *const f64,
        x_length: c_int,
        fs: c_int,
        temporal_positions: *const f64,
        f0: *const f64,
        f0_length: c_int,
        refined_f0: *mut f64,
    );
    fn InitializeCheapTrickOption(fs: c_int, option: *mut CheapTrickOption);
    fn GetFFTSizeForCheapTrick(fs: c_int, option: *const CheapTrickOption) -> c_int;
    fn CheapTrick(
        x: *const f64,
        x_length: c_int,
        fs: c_int,
        temporal_positions: *const f64,
        f0: *const f64,
        f0_length: c_int,
        option: *const CheapTrickOption,
        spectrogram: *mut *mut f64,
    );
    fn InitializeD4COption(option: *mut D4COption);
    fn D4C(
        x: *const f64,
        x_length: c_int,
        fs: c_int,
        temporal_positions: *const f64,
        f0: *const f64,
        f0_length: c_int,
        fft_size: c_int,
        option: *const D4COption,
        aperiodicity: *mut *mut f64,
    );
    fn Synthesis(
        f0: *const f64,
        f0_length: c_int,
        spectrogram: *const *const f64,
        aperiodicity: *const *const f64,
        fft_size: c_int,
        frame_period: f64,
        fs: c_int,
        y_length: c_int,
        y: *mut f64,
    );
}

/// The longest signal handed to WORLD at once, in samples (its lengths are C `int`s).
const MAX_SAMPLES: usize = 1 << 28;

/// A voice taken apart: one row per frame, `frame_ms` apart, the first at time 0.
#[derive(Clone, Debug)]
pub struct VoiceParams {
    pub sample_rate: u32,
    pub frame_ms: f64,
    pub fft_size: usize,
    /// Hz per frame, 0 where unvoiced.
    pub f0: Vec<f64>,
    /// Power spectral envelope, `fft_size / 2 + 1` bins per frame.
    pub spectrum: Vec<Vec<f64>>,
    /// Aperiodicity (0 = harmonic, 1 = noise), the same shape.
    pub aperiodicity: Vec<Vec<f64>>,
}

impl VoiceParams {
    pub fn bins(&self) -> usize {
        self.fft_size / 2 + 1
    }
}

/// The number of frames WORLD uses for `samples` at `fs`, `frame_ms` apart.
pub fn frames_for(samples: usize, fs: u32, frame_ms: f64) -> usize {
    (samples as f64 / f64::from(fs.max(1)) * 1000.0 / frame_ms) as usize + 1
}

fn positions(frames: usize, frame_ms: f64) -> Vec<f64> {
    (0..frames).map(|i| i as f64 * frame_ms / 1000.0).collect()
}

/// WORLD's own pitch tracker (Harvest): Hz per frame, 0 where unvoiced. The Pitch Editor uses
/// it as an independent reference when its own tracker is measured.
pub fn harvest(x: &[f64], fs: u32, frame_ms: f64, floor: f64, ceil: f64) -> Vec<f64> {
    if x.is_empty() || x.len() > MAX_SAMPLES || fs == 0 {
        return Vec::new();
    }
    let mut option = HarvestOption {
        f0_floor: 0.0,
        f0_ceil: 0.0,
        frame_period: 0.0,
    };
    // SAFETY: the option struct matches WORLD's layout (repr(C), the same fields).
    unsafe { InitializeHarvestOption(&mut option) };
    option.f0_floor = floor;
    option.f0_ceil = ceil;
    option.frame_period = frame_ms;
    // SAFETY: plain arithmetic on the lengths.
    let n = unsafe { GetSamplesForHarvest(fs as c_int, x.len() as c_int, frame_ms) };
    let n = usize::try_from(n).unwrap_or(0);
    let mut tp = vec![0.0; n];
    let mut f0 = vec![0.0; n];
    // SAFETY: `x` has `x.len()` samples; `tp` and `f0` have the length WORLD asks for.
    unsafe {
        Harvest(
            x.as_ptr(),
            x.len() as c_int,
            fs as c_int,
            &option,
            tp.as_mut_ptr(),
            f0.as_mut_ptr(),
        )
    };
    f0
}

/// Takes `x` apart around the pitch `f0` (one value per frame, 0 where unvoiced; refined here
/// with WORLD's StoneMask). `f0` is cut or padded to the frames `x` has.
pub fn analyze(x: &[f64], fs: u32, frame_ms: f64, f0: &[f64]) -> Option<VoiceParams> {
    if x.len() < 64 || x.len() > MAX_SAMPLES || fs < 8000 {
        return None;
    }
    let frames = frames_for(x.len(), fs, frame_ms);
    let tp = positions(frames, frame_ms);
    let mut f0: Vec<f64> = (0..frames)
        .map(|i| {
            f0.get(i)
                .copied()
                .filter(|v| v.is_finite() && *v > 0.0)
                .unwrap_or(0.0)
        })
        .collect();
    let mut refined = vec![0.0; frames];
    let (xp, xl, fsi, fl) = (x.as_ptr(), x.len() as c_int, fs as c_int, frames as c_int);
    // SAFETY: every buffer has the length passed with it.
    unsafe {
        StoneMask(
            xp,
            xl,
            fsi,
            tp.as_ptr(),
            f0.as_ptr(),
            fl,
            refined.as_mut_ptr(),
        )
    };
    for (a, r) in f0.iter_mut().zip(&refined) {
        // StoneMask can wander on a weak frame; only small corrections are kept.
        if *a > 0.0 && r.is_finite() && (*r / *a - 1.0).abs() < 0.03 {
            *a = *r;
        }
    }
    let mut ct = CheapTrickOption {
        q1: 0.0,
        f0_floor: 0.0,
        fft_size: 0,
    };
    // SAFETY: the option struct matches WORLD's layout.
    unsafe { InitializeCheapTrickOption(fsi, &mut ct) };
    // SAFETY: as above.
    let fft_size = unsafe { GetFFTSizeForCheapTrick(fsi, &ct) };
    ct.fft_size = fft_size;
    let fft = usize::try_from(fft_size).ok()?;
    let bins = fft / 2 + 1;
    let mut spectrum = vec![vec![0.0f64; bins]; frames];
    let mut aperiodicity = vec![vec![0.0f64; bins]; frames];
    let mut sp_rows: Vec<*mut f64> = spectrum.iter_mut().map(|r| r.as_mut_ptr()).collect();
    let mut ap_rows: Vec<*mut f64> = aperiodicity.iter_mut().map(|r| r.as_mut_ptr()).collect();
    let mut d4 = D4COption { threshold: 0.0 };
    // SAFETY: `frames` rows of `bins` values each, as CheapTrick and D4C write them; the rows
    // stay where they are (nothing resizes them) until the calls return.
    unsafe {
        CheapTrick(
            xp,
            xl,
            fsi,
            tp.as_ptr(),
            f0.as_ptr(),
            fl,
            &ct,
            sp_rows.as_mut_ptr(),
        );
        InitializeD4COption(&mut d4);
        D4C(
            xp,
            xl,
            fsi,
            tp.as_ptr(),
            f0.as_ptr(),
            fl,
            fft_size,
            &d4,
            ap_rows.as_mut_ptr(),
        );
    }
    Some(VoiceParams {
        sample_rate: fs,
        frame_ms,
        fft_size: fft,
        f0,
        spectrum,
        aperiodicity,
    })
}

/// Puts a voice back together, `samples` long.
pub fn synthesize(p: &VoiceParams, samples: usize) -> Vec<f64> {
    let bins = p.bins();
    let frames = p.f0.len();
    let ok = frames > 0
        && samples > 0
        && samples <= MAX_SAMPLES
        && p.spectrum.len() == frames
        && p.aperiodicity.len() == frames
        && p.spectrum.iter().all(|r| r.len() == bins)
        && p.aperiodicity.iter().all(|r| r.len() == bins);
    if !ok {
        return vec![0.0; samples.min(MAX_SAMPLES)];
    }
    let sp: Vec<*const f64> = p.spectrum.iter().map(|r| r.as_ptr()).collect();
    let ap: Vec<*const f64> = p.aperiodicity.iter().map(|r| r.as_ptr()).collect();
    let mut y = vec![0.0f64; samples];
    // SAFETY: `frames` rows of `bins` values (checked above), `y` is `samples` long.
    unsafe {
        Synthesis(
            p.f0.as_ptr(),
            frames as c_int,
            sp.as_ptr(),
            ap.as_ptr(),
            p.fft_size as c_int,
            p.frame_ms,
            p.sample_rate as c_int,
            samples as c_int,
            y.as_mut_ptr(),
        )
    };
    for v in &mut y {
        if !v.is_finite() {
            *v = 0.0;
        }
    }
    y
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A buzzy vowel at `hz`: a pulse train through two resonances.
    fn vowel(hz: f64, secs: f64, fs: u32) -> Vec<f64> {
        let n = (secs * f64::from(fs)) as usize;
        let mut phase = 0.0;
        let mut out = vec![0.0; n];
        let mut y1 = [0.0f64; 2];
        let mut y2 = [0.0f64; 2];
        for v in out.iter_mut() {
            phase += hz / f64::from(fs);
            let e = if phase >= 1.0 {
                phase -= 1.0;
                1.0
            } else {
                0.0
            };
            let mut s = e;
            for (f, (bw, y)) in [(700.0, (80.0, &mut y1)), (1200.0, (90.0, &mut y2))] {
                let r = (-std::f64::consts::PI * bw / f64::from(fs)).exp();
                let c = 2.0 * r * (std::f64::consts::TAU * f / f64::from(fs)).cos();
                let o = s + c * y[0] - r * r * y[1];
                y[1] = y[0];
                y[0] = o;
                s = o;
            }
            *v = s * 0.05;
        }
        out
    }

    #[test]
    fn a_voice_comes_apart_and_back_together() {
        let fs = 44_100;
        let x = vowel(220.0, 0.6, fs);
        let f0 = harvest(&x, fs, 5.0, 60.0, 800.0);
        let voiced: Vec<f64> = f0.iter().copied().filter(|f| *f > 0.0).collect();
        assert!(voiced.len() > f0.len() / 2, "{f0:?}");
        let mid = voiced[voiced.len() / 2];
        assert!((mid - 220.0).abs() < 2.0, "{mid}");
        let p = analyze(&x, fs, 5.0, &f0).unwrap();
        assert_eq!(p.f0.len(), frames_for(x.len(), fs, 5.0));
        let y = synthesize(&p, x.len());
        assert_eq!(y.len(), x.len());
        let rms = |v: &[f64]| (v.iter().map(|s| s * s).sum::<f64>() / v.len() as f64).sqrt();
        let (a, b) = (rms(&x[4410..22050]), rms(&y[4410..22050]));
        assert!(b > a * 0.5 && b < a * 2.0, "{a} {b}");
        assert!(analyze(&[], fs, 5.0, &[]).is_none());
        assert!(synthesize(&p, 0).is_empty());
    }
}
