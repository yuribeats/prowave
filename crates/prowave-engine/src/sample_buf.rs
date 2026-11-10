//! A decoded sound held in memory, as the sample tools, the audition player and the built-in
//! sample instruments pass it around.

/// A sound ready to play: interleaved stereo at its own rate.
#[derive(Debug, Default, PartialEq)]
pub struct SampleBuf {
    pub rate: u32,
    pub data: Vec<f32>,
}

impl SampleBuf {
    pub fn new(rate: u32, data: Vec<f32>) -> Self {
        Self { rate, data }
    }

    pub fn frames(&self) -> usize {
        self.data.len() / 2
    }

    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / f64::from(self.rate.max(1))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    pub(crate) fn sine(hz: f64, frames: usize, rate: u32) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let s = (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin() as f32 * 0.5;
            v.push(s);
            v.push(s);
        }
        v
    }

    /// The strongest frequency of a mono signal (FFT peak with parabolic refinement).
    pub(crate) fn peak_hz(mono: &[f32], rate: u32) -> f64 {
        let n = mono.len().next_power_of_two() / 2;
        let fft = crate::spectral::Fft::new(n);
        let mut re: Vec<f32> = (0..n)
            .map(|i| {
                let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos();
                mono[i] * w
            })
            .collect();
        let mut im = vec![0.0; n];
        fft.process(&mut re, &mut im, false);
        let mag: Vec<f64> = (0..n / 2).map(|i| f64::from(re[i].hypot(im[i]))).collect();
        let k = (1..n / 2 - 1)
            .max_by(|a, b| mag[*a].total_cmp(&mag[*b]))
            .unwrap();
        let (a, b, c) = (mag[k - 1].ln(), mag[k].ln(), mag[k + 1].ln());
        let shift = 0.5 * (a - c) / (a - 2.0 * b + c);
        (k as f64 + shift) * f64::from(rate) / n as f64
    }
}
