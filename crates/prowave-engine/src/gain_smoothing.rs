//! Finite linear gain ramps for click-free fader changes, independent of callback size.

#[derive(Clone, Copy)]
pub(crate) struct GainRamp {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
}

impl GainRamp {
    pub(crate) fn new(value: f32) -> Self {
        Self {
            current: value,
            target: value,
            step: 0.0,
            remaining: 0,
        }
    }

    pub(crate) fn set(&mut self, target: f32, samples: u32) {
        if !target.is_finite() {
            return;
        }
        if samples == 0 {
            self.current = target;
            self.target = target;
            self.remaining = 0;
        } else if target != self.target {
            self.target = target;
            self.remaining = samples;
            self.step = (target - self.current) / samples as f32;
        }
    }

    /// The value now, without moving on.
    pub(crate) fn current(&self) -> f32 {
        self.current
    }

    pub(crate) fn next(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            self.current = if self.remaining == 0 {
                self.target
            } else {
                self.current + self.step
            };
        }
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaches_exact_target_at_requested_sample() {
        for samples in [1, 221, 240, 480, 960] {
            let mut ramp = GainRamp::new(1.0);
            ramp.set(0.0, samples);
            let mut previous = 1.0;
            for _ in 0..samples {
                let value = ramp.next();
                assert!(value >= 0.0 && value <= previous);
                previous = value;
            }
            assert_eq!(previous, 0.0);
            assert_eq!(ramp.next(), 0.0);
        }
    }

    #[test]
    fn retargets_from_current_gain_and_repeated_target_does_not_restart() {
        let mut ramp = GainRamp::new(1.0);
        ramp.set(0.0, 4);
        assert_eq!(ramp.next(), 0.75);
        ramp.set(0.0, 4);
        assert_eq!(ramp.next(), 0.5);
        ramp.set(1.0, 2);
        assert_eq!(ramp.next(), 0.75);
        assert_eq!(ramp.next(), 1.0);
    }

    #[test]
    fn immediate_changes_and_nonfinite_targets() {
        let mut ramp = GainRamp::new(0.8);
        ramp.set(0.25, 0);
        for target in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            ramp.set(target, 240);
            assert_eq!(ramp.next(), 0.25);
        }
    }
}
