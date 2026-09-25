//! Per-stream receive loudness control.

/// Tunable receive loudness parameters.
pub const TARGET_DBFS: f32 = -20.0;
pub const GATE_DBFS: f32 = -50.0;
pub const MIN_GAIN_DB: f32 = -12.0;
pub const MAX_GAIN_DB: f32 = 12.0;
pub const ATTACK_SECS: f32 = 0.5;
pub const RELEASE_SECS: f32 = 0.05;
pub const FRAME_SECS: f32 = 0.02;

#[derive(Debug, Clone, Copy)]
pub struct Agc {
    gain_db: f32,
}

impl Default for Agc {
    fn default() -> Self {
        Self { gain_db: 0.0 }
    }
}

impl Agc {
    pub fn gain_db(self) -> f32 {
        self.gain_db
    }

    pub fn process(&mut self, samples: &mut [i16]) {
        let level = rms_dbfs(samples);
        if level <= GATE_DBFS {
            return;
        }
        let wanted = (TARGET_DBFS - level).clamp(MIN_GAIN_DB, MAX_GAIN_DB);
        let tau = if wanted > self.gain_db {
            ATTACK_SECS
        } else {
            RELEASE_SECS
        };
        let alpha = 1.0 - (-FRAME_SECS / tau).exp();
        self.gain_db += (wanted - self.gain_db) * alpha;

        let gain = db_to_linear(self.gain_db);
        if (gain - 1.0).abs() > f32::EPSILON {
            for sample in samples {
                *sample = (*sample as f32 * gain)
                    .round()
                    .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            }
        }
    }
}

pub fn rms_dbfs(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return f32::NEG_INFINITY;
    }
    let sum: f64 = samples
        .iter()
        .map(|&sample| {
            let normalized = sample as f64 / i16::MAX as f64;
            normalized * normalized
        })
        .sum();
    let rms = (sum / samples.len() as f64).sqrt();
    if rms <= f64::MIN_POSITIVE {
        f32::NEG_INFINITY
    } else {
        (20.0 * rms.log10()) as f32
    }
}

fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

pub fn limit(samples: &mut [i16]) {
    for sample in samples {
        let value = *sample as f32;
        let sign = value.signum();
        let magnitude = value.abs();
        let knee = 0.9 * i16::MAX as f32;
        let limited = if magnitude <= knee {
            magnitude
        } else {
            knee + (i16::MAX as f32 - knee)
                * (1.0 - (-(magnitude - knee) / (i16::MAX as f32 - knee)).exp())
        };
        *sample = (limited.min(i16::MAX as f32) * sign) as i16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amplitude: f32, n: usize) -> Vec<i16> {
        (0..n)
            .map(|i| {
                (amplitude
                    * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin()
                    * i16::MAX as f32) as i16
            })
            .collect()
    }

    #[test]
    fn quiet_tone_moves_toward_target_without_exceeding_attack_limit() {
        let mut agc = Agc::default();
        let mut frame = tone(0.05, 960);
        agc.process(&mut frame);
        assert!(agc.gain_db() > 0.0);
        assert!(
            agc.gain_db() < 1.0,
            "gain changed too quickly: {}",
            agc.gain_db()
        );
    }

    #[test]
    fn gated_silence_does_not_raise_gain() {
        let mut agc = Agc::default();
        let mut frame = vec![0; 960];
        agc.process(&mut frame);
        assert_eq!(agc.gain_db(), 0.0);
    }

    #[test]
    fn below_gate_noise_does_not_raise_gain() {
        let mut agc = Agc::default();
        let mut frame = tone(0.002, 960);
        assert!(rms_dbfs(&frame) < GATE_DBFS);
        agc.process(&mut frame);
        assert_eq!(agc.gain_db(), 0.0);
    }

    #[test]
    fn below_gate_frame_is_not_amplified_by_previous_gain() {
        let mut agc = Agc::default();
        for _ in 0..120 {
            let mut speech = tone(0.05, 960);
            agc.process(&mut speech);
        }
        assert!(agc.gain_db() > 0.0);
        let mut noise = tone(0.001, 960);
        let before = noise.clone();
        agc.process(&mut noise);
        assert_eq!(noise, before);
    }

    #[test]
    fn gain_is_bounded() {
        let mut agc = Agc::default();
        let mut frame = tone(0.01, 960);
        for _ in 0..200 {
            agc.process(&mut frame);
        }
        assert!(agc.gain_db() <= MAX_GAIN_DB);
        assert!(agc.gain_db() >= MIN_GAIN_DB);
    }

    #[test]
    fn sustained_quiet_tone_converges_near_target_loudness() {
        let mut agc = Agc::default();
        let mut output = Vec::new();
        for _ in 0..120 {
            let mut frame = tone(0.05, 960);
            agc.process(&mut frame);
            output = frame;
        }
        assert!((rms_dbfs(&output) - TARGET_DBFS).abs() < 2.0);
    }

    #[test]
    fn limiter_never_wraps_or_exceeds_pcm_range() {
        let mut samples = vec![i16::MAX, i16::MIN, 0];
        limit(&mut samples);
        assert!(samples[0] < i16::MAX);
        assert!(samples[1] > i16::MIN);
        assert_eq!(samples[2], 0);
    }
}
