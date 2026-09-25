//! Lightweight per-stream voice filtering.

const SAMPLE_RATE: f32 = 48_000.0;
pub const HIGH_PASS_HZ: f32 = 300.0;
pub const LOW_PASS_HZ: f32 = 3_400.0;

const HP_ALPHA: f32 = {
    let rc = 1.0 / (2.0 * std::f32::consts::PI * HIGH_PASS_HZ);
    rc / (rc + 1.0 / SAMPLE_RATE)
};
const LP_ALPHA: f32 = {
    let rc = 1.0 / (2.0 * std::f32::consts::PI * LOW_PASS_HZ);
    (1.0 / SAMPLE_RATE) / (rc + 1.0 / SAMPLE_RATE)
};

/// Stateful first-order high-pass plus low-pass filter.
///
/// The state lives with one receive stream. `process` never allocates.
#[derive(Debug, Clone, Copy)]
pub struct VoiceFilter {
    previous_input: f32,
    high_pass_state: f32,
    low_pass_state: f32,
}

impl Default for VoiceFilter {
    fn default() -> Self {
        Self {
            previous_input: 0.0,
            high_pass_state: 0.0,
            low_pass_state: 0.0,
        }
    }
}

impl VoiceFilter {
    pub fn process(&mut self, samples: &mut [i16]) {
        for sample in samples {
            let input = *sample as f32;
            let high_pass = HP_ALPHA * (self.high_pass_state + input - self.previous_input);
            self.previous_input = input;
            self.high_pass_state = high_pass;
            self.low_pass_state += LP_ALPHA * (high_pass - self.low_pass_state);
            *sample = self
                .low_pass_state
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, n: usize, amplitude: f32) -> Vec<i16> {
        (0..n)
            .map(|i| {
                (amplitude
                    * 32767.0
                    * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE).sin())
                    as i16
            })
            .collect()
    }

    fn rms(samples: &[i16]) -> f32 {
        let sum: f64 = samples
            .iter()
            .map(|&sample| (sample as f64) * (sample as f64))
            .sum();
        (sum / samples.len() as f64).sqrt() as f32
    }

    #[test]
    fn removes_steady_dc_after_settling() {
        let mut filter = VoiceFilter::default();
        let mut frame = vec![2_000; 960];
        for _ in 0..20 {
            filter.process(&mut frame);
        }
        assert!(rms(&frame) < 20.0, "dc residue too high: {}", rms(&frame));
    }

    #[test]
    fn preserves_voice_band_and_reduces_ultrasonic_tone() {
        let mut voice = tone(1_000.0, 9_600, 0.2);
        let voice_input = rms(&voice);
        let mut high = tone(10_000.0, 9_600, 0.2);
        let high_input = rms(&high);
        let mut filter = VoiceFilter::default();
        filter.process(&mut voice);
        let mut filter = VoiceFilter::default();
        filter.process(&mut high);
        assert!(rms(&voice) > voice_input * 0.5);
        assert!(rms(&high) < high_input * 0.35);
    }
}
