//! 混音、同频干扰音与射程衰减。
//!
//! 纯 DSP，无 I/O —— 和 `stack` 一样，是全库最值得单测的部分。
//!
//! 射程过滤本身在服务端做（省带宽、防作弊）；这里做的是边缘带的平滑衰减，
//! 因为硬截断听起来像 bug（spec 7.1、7.4）。客户端完全不知道任何人的位置，
//! 它只拿到服务端算好的一个 `qual` 字节。

/// `qual` 到增益。
///
/// **下行的 `qual` 恒在 1–255，永远不会是 0**（射程之外服务端根本不投递，
/// 衰减带最外侧四舍五入本来会得到 0 的那一小段被夹到 1）。所以这里没有、
/// 也不要加一条 `qual == 0` 的分支：那条分支永远不会执行，也就永远不会被发现
/// 写错了。0 在这个公式里只是自然延续，不是哨兵值。
pub fn quality_gain(qual: u8) -> f32 {
    // 从 0.35 到 1.0：边缘信号明显更小声，但仍然听得清内容。
    // 直接线性到 0 会让边缘带最后一点变成听不见的耳语，
    // 那和硬截断没有区别。
    0.35 + 0.65 * (qual as f32 / 255.0)
}

/// `qual` 到静噪强度。满格无噪。
pub fn squelch_level(qual: u8) -> f32 {
    // `qual == 255` 而不是 `>= 255`：后者在 u8 上是
    // `clippy::absurd_extreme_comparisons`，deny-by-default，过不了 `-D warnings`。
    if qual == u8::MAX {
        return 0.0;
    }
    // 越弱噪声越大，最强约为满量程的 6%。
    0.06 * (1.0 - qual as f32 / 255.0)
}

/// 确定性伪随机，供静噪使用。
///
/// 刻意不用 `rand`：测试需要可复现的噪声，否则断言会时灵时不灵，
/// 而一个偶尔失败的音频测试最终会被人关掉。
#[derive(Debug, Clone)]
pub struct NoiseGen {
    state: u32,
}

impl NoiseGen {
    pub fn new(seed: u32) -> Self {
        Self { state: seed | 1 }
    }

    /// 下一个 -1.0..1.0 的样本。
    ///
    /// **叫 `sample` 而不是 `next`**：`clippy::should_implement_trait` 是
    /// warn-by-default，而门禁是 `-D warnings`，一个叫 `next` 的固有方法会让
    /// 这个模块过不了自己的门。
    pub fn sample(&mut self) -> f32 {
        // xorshift32
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        (self.state as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// 按信号质量做衰减并混入静噪。
pub fn apply_quality(samples: &mut [i16], qual: u8, noise: &mut NoiseGen) {
    let gain = quality_gain(qual);
    let squelch = squelch_level(qual);
    for s in samples.iter_mut() {
        let mut v = *s as f32 * gain;
        if squelch > 0.0 {
            v += noise.sample() * squelch * 32767.0;
        }
        *s = v.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    }
}

/// 拍频啸叫的频率，单位赫兹。真实 AM 无线电上两个载波差频落在这个量级。
const BEAT_HZ: f32 = 1200.0;

/// 同频多路信号叠加成干扰音。
///
/// 一路时原样通过 —— 一个人在讲话不是干扰。
/// 两路及以上时相加后注入拍频啸叫并轻微削波，听感上就是
/// "有人在压我的话"，而不是两段可分辨的语音。
pub fn interfere(sources: &[&[i16]], out: &mut [i16], phase: &mut f32) {
    if sources.is_empty() {
        out.fill(0);
        return;
    }
    if sources.len() == 1 {
        let src = sources[0];
        for (i, o) in out.iter_mut().enumerate() {
            *o = src.get(i).copied().unwrap_or(0);
        }
        return;
    }

    let step = 2.0 * std::f32::consts::PI * BEAT_HZ / 48_000.0;
    for (i, o) in out.iter_mut().enumerate() {
        let sum: f32 = sources.iter().map(|s| s.get(i).copied().unwrap_or(0) as f32).sum();
        // 拍频调制：幅度随啸叫起伏，这是"两个载波在打架"的听感来源。
        let beat = phase.sin();
        *phase += step;
        if *phase > 2.0 * std::f32::consts::PI {
            *phase -= 2.0 * std::f32::consts::PI;
        }
        let modulated = sum * (1.0 + 0.45 * beat) + beat * 0.08 * 32767.0;
        // 轻微削波：过载失真是真实无线电互相压制时的另一半听感。
        *o = soft_clip(modulated * 1.15);
    }
}

/// 把一路信号按增益混进目标缓冲，带软限幅。
///
/// i16 直接相加溢出会回绕，产生刺耳的爆音 —— 那是最容易被误报成
/// "语音系统坏了"的故障。
pub fn mix_into(dst: &mut [i16], src: &[i16], gain: f32) {
    for (i, d) in dst.iter_mut().enumerate() {
        let v = *d as f32 + src.get(i).copied().unwrap_or(0) as f32 * gain;
        *d = soft_clip(v);
    }
}

/// 软限幅：接近满量程时逐渐压缩而不是硬切。
fn soft_clip(v: f32) -> i16 {
    const LIMIT: f32 = 32767.0;
    const KNEE: f32 = 0.8 * LIMIT;
    let a = v.abs();
    if a <= KNEE {
        return v as i16;
    }
    let over = (a - KNEE) / (LIMIT - KNEE);
    let compressed = KNEE + (LIMIT - KNEE) * (1.0 - (-over).exp());
    (compressed.min(LIMIT) * v.signum()) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一段正弦，48 kHz 采样。
    fn tone(freq: f32, n: usize, amp: f32) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                (amp * 32767.0 * (2.0 * std::f32::consts::PI * freq * t).sin()) as i16
            })
            .collect()
    }

    fn rms(s: &[i16]) -> f32 {
        if s.is_empty() {
            return 0.0;
        }
        let sum: f64 = s.iter().map(|&v| (v as f64) * (v as f64)).sum();
        (sum / s.len() as f64).sqrt() as f32
    }

    #[test]
    fn full_quality_is_unity_gain() {
        assert!((quality_gain(255) - 1.0).abs() < 0.01, "qual 255 = {}", quality_gain(255));
    }

    #[test]
    fn gain_falls_as_quality_falls() {
        let g255 = quality_gain(255);
        let g128 = quality_gain(128);
        let g1 = quality_gain(1);
        assert!(g255 > g128 && g128 > g1, "gains: {g255} {g128} {g1}");
        assert!(g1 > 0.0, "an in-range signal must still be audible");
    }

    #[test]
    fn squelch_rises_as_quality_falls() {
        assert!(squelch_level(255) < squelch_level(128));
        assert!(squelch_level(128) < squelch_level(1));
        assert_eq!(squelch_level(255), 0.0, "a full-quality signal carries no squelch");
    }

    /// 修订件 §八.2：**下行 `qual` 恒在 1–255，永远不会是 0。**
    ///
    /// 射程之外服务端根本不投递，衰减带最外侧四舍五入本来会得到 0 的那一小段被
    /// 夹到 1（`geo.Quality`，钉子是 `TestQualityNeverReportsZeroWhileStillInRange`）。
    /// 所以**不要**给 0 写"最弱信号"或"静音"的分支：那条分支永远不会执行，
    /// 也就永远不会被发现写错了。这两个函数保持是全域的普通公式。
    #[test]
    fn there_is_no_special_case_for_a_quality_of_zero() {
        // 0 只是公式的自然延续，不是一个哨兵值。
        assert!((quality_gain(0) - 0.35).abs() < 0.001, "qual 0 = {}", quality_gain(0));
        assert!(quality_gain(0) > 0.0, "a zero branch that silences audio would never be exercised");
        assert!(squelch_level(0) > squelch_level(1));
    }

    #[test]
    fn apply_quality_attenuates_a_weak_signal() {
        let mut strong = tone(1000.0, 960, 0.5);
        let mut weak = strong.clone();
        let mut n = NoiseGen::new(1);
        apply_quality(&mut strong, 255, &mut n);
        let mut n2 = NoiseGen::new(1);
        apply_quality(&mut weak, 40, &mut n2);
        assert!(rms(&weak) < rms(&strong),
            "a low-quality signal must be quieter: weak={} strong={}", rms(&weak), rms(&strong));
    }

    #[test]
    fn apply_quality_adds_noise_to_a_weak_signal() {
        // 全静音输入下，低 qual 应当产生非零输出 —— 那就是静噪。
        let mut silence = vec![0i16; 960];
        let mut n = NoiseGen::new(7);
        apply_quality(&mut silence, 40, &mut n);
        assert!(rms(&silence) > 0.0, "a weak signal must carry audible squelch noise");
    }

    #[test]
    fn apply_quality_leaves_a_full_signal_noise_free() {
        let mut silence = vec![0i16; 960];
        let mut n = NoiseGen::new(7);
        apply_quality(&mut silence, 255, &mut n);
        assert_eq!(rms(&silence), 0.0, "a full-quality signal must not have noise added");
    }

    #[test]
    fn a_single_source_passes_through_interfere_unchanged() {
        let src = tone(1000.0, 960, 0.4);
        let mut out = vec![0i16; 960];
        let mut phase = 0.0;
        interfere(&[&src], &mut out, &mut phase);
        assert_eq!(out, src, "one speaker on a frequency is not interference");
    }

    #[test]
    fn two_sources_produce_a_heterodyne_beat() {
        // 两路同频信号相加会产生拍频啸叫——这正是真实 AM 无线电上
        // 两个载波差频的声音，也是"听得出有两个人在压"的依据。
        let a = tone(500.0, 4800, 0.3);
        let b = tone(700.0, 4800, 0.3);
        let plain: Vec<i16> =
            a.iter().zip(&b).map(|(x, y)| x.saturating_add(*y)).collect();
        let mut out = vec![0i16; 4800];
        let mut phase = 0.0;
        interfere(&[&a, &b], &mut out, &mut phase);
        assert_ne!(out, plain, "interfere must do more than add the two sources");
        assert!(rms(&out) > 0.0);
    }

    #[test]
    fn interference_is_louder_than_either_source_alone() {
        let a = tone(500.0, 960, 0.3);
        let b = tone(700.0, 960, 0.3);
        let mut out = vec![0i16; 960];
        let mut phase = 0.0;
        interfere(&[&a, &b], &mut out, &mut phase);
        assert!(rms(&out) > rms(&a), "two people talking over each other should be more, not less");
    }

    #[test]
    fn interfere_handles_sources_of_different_lengths() {
        let a = tone(500.0, 960, 0.3);
        let b = tone(700.0, 480, 0.3);
        let mut out = vec![0i16; 960];
        let mut phase = 0.0;
        interfere(&[&a, &b], &mut out, &mut phase);
        // 不 panic 即可；短的那一路后半段按静音处理。
        assert_eq!(out.len(), 960);
    }

    #[test]
    fn mix_into_clips_softly_rather_than_wrapping() {
        // i16 溢出回绕会产生刺耳的爆音。
        let loud = vec![i16::MAX; 960];
        let mut dst = vec![i16::MAX; 960];
        mix_into(&mut dst, &loud, 1.0);
        assert!(dst.iter().all(|&v| v > 0),
            "mixing two loud signals must not wrap around to negative");
    }

    #[test]
    fn mix_into_respects_gain() {
        let src = tone(1000.0, 960, 0.5);
        let mut half = vec![0i16; 960];
        mix_into(&mut half, &src, 0.5);
        let mut full = vec![0i16; 960];
        mix_into(&mut full, &src, 1.0);
        assert!(rms(&half) < rms(&full));
    }

    /// 修订件 H5：这个方法叫 `sample` 而不是 `next`。
    ///
    /// `clippy::should_implement_trait` 是 warn-by-default，而每个任务的门禁是
    /// `cargo clippy --workspace -- -D warnings`——一个叫 `next` 的固有方法会让
    /// 这个任务过不了它自己的门。
    #[test]
    fn noise_is_deterministic_for_a_given_seed() {
        let mut a = NoiseGen::new(42);
        let mut b = NoiseGen::new(42);
        let xs: Vec<f32> = (0..100).map(|_| a.sample()).collect();
        let ys: Vec<f32> = (0..100).map(|_| b.sample()).collect();
        assert_eq!(xs, ys, "noise must be reproducible so these tests are not flaky");
    }

    #[test]
    fn noise_stays_inside_the_unit_range() {
        let mut n = NoiseGen::new(3);
        for _ in 0..10_000 {
            let v = n.sample();
            assert!((-1.0..=1.0).contains(&v), "noise sample {v} escaped -1..1");
        }
    }
}
