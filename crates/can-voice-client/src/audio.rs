//! 音频设备枚举与采样率适配。

use crate::rx::decode::SAMPLE_RATE;

/// 一个音频设备。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// 线性插值重采样到 48 kHz。
///
/// 这一步是**必须的，不是可选的**。旧的 Python 实现没有做，注释里写着
/// "48 kHz 是理想路径，回退采样率会产生变调音频" —— 也就是说设备不支持
/// 48 kHz 时用户听到的是变调的声音，而那看起来像"语音系统坏了"。
///
/// 线性插值对 8 kHz 带宽的语音足够：它会在高频引入一点混叠，
/// 但无线电语音本来就被限制在 300–3400 Hz。
pub fn resample_to_48k(input: &[i16], from_rate: u32) -> Vec<i16> {
    resample(input, from_rate, SAMPLE_RATE)
}

/// 从 48 kHz 重采样到设备采样率。
pub fn resample_from_48k(input: &[i16], to_rate: u32) -> Vec<i16> {
    resample(input, SAMPLE_RATE, to_rate)
}

fn resample(input: &[i16], from: u32, to: u32) -> Vec<i16> {
    if input.is_empty() || from == 0 || to == 0 {
        return Vec::new();
    }
    if from == to {
        return input.to_vec();
    }
    let ratio = to as f64 / from as f64;
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 / ratio;
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        let a = input.get(idx).copied().unwrap_or(0) as f32;
        // 越过末尾时**保持最后一个样本**，不要拿 0 去插值：那会在每个缓冲的
        // 末尾插进一小段冲向静音的斜坡，听感上是每 20 毫秒一次的规律咔哒声，
        // 而且只在非 48 kHz 的设备上出现——报上来会是"某些人的声音有电流声"。
        let b = input.get(idx + 1).copied().map(f32::from).unwrap_or(a);
        out.push((a + (b - a) * frac).round() as i16);
    }
    out
}

/// 列出输入设备。
pub fn input_devices() -> Vec<DeviceInfo> {
    devices(true)
}

/// 列出输出设备。
pub fn output_devices() -> Vec<DeviceInfo> {
    devices(false)
}

fn devices(input: bool) -> Vec<DeviceInfo> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default_name = if input {
        host.default_input_device().and_then(|d| d.name().ok())
    } else {
        host.default_output_device().and_then(|d| d.name().ok())
    };
    let list = if input { host.input_devices() } else { host.output_devices() };
    let Ok(list) = list else {
        // 枚举失败不该让客户端起不来：没有设备也能听不能说，
        // 或者反过来，总比整个应用打不开好。
        tracing::warn!(input, "could not enumerate audio devices");
        return Vec::new();
    };
    list.filter_map(|d| {
        let name = d.name().ok()?;
        Some(DeviceInfo {
            is_default: Some(&name) == default_name.as_ref(),
            id: name.clone(),
            name,
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_from_48k_to_48k_is_a_no_op() {
        let input: Vec<i16> = (0..960).map(|i| i as i16).collect();
        assert_eq!(resample_to_48k(&input, 48_000), input);
    }

    #[test]
    fn upsampling_from_24k_doubles_the_sample_count() {
        let input = vec![100i16; 480];
        let out = resample_to_48k(&input, 24_000);
        assert_eq!(out.len(), 960, "24 kHz to 48 kHz must double the samples");
    }

    #[test]
    fn downsampling_from_96k_halves_the_sample_count() {
        let input = vec![100i16; 1920];
        let out = resample_to_48k(&input, 96_000);
        assert_eq!(out.len(), 960);
    }

    #[test]
    fn resampling_from_44100_produces_the_right_length() {
        // 44.1 kHz 是最常见的非 48 kHz 设备采样率，而且比例不是整数。
        let input = vec![0i16; 441];
        let out = resample_to_48k(&input, 44_100);
        assert!((out.len() as i32 - 480).abs() <= 1, "441 samples at 44.1k is 10 ms = 480 at 48k, got {}", out.len());
    }

    #[test]
    fn resampling_preserves_a_constant_signal() {
        // 常数信号重采样后还该是同一个常数 —— 插值出别的值说明算错了。
        let input = vec![1000i16; 480];
        let out = resample_to_48k(&input, 24_000);
        assert!(out.iter().all(|&v| (v - 1000).abs() <= 1),
            "a constant signal must survive resampling, got {:?}", &out[..8]);
    }

    #[test]
    fn the_two_directions_round_trip_approximately() {
        let input: Vec<i16> = (0..480).map(|i| ((i as f32 / 10.0).sin() * 8000.0) as i16).collect();
        let up = resample_to_48k(&input, 24_000);
        let back = resample_from_48k(&up, 24_000);
        assert_eq!(back.len(), input.len());
    }

    #[test]
    fn resampling_an_empty_buffer_does_not_panic() {
        assert!(resample_to_48k(&[], 44_100).is_empty());
    }

    /// 上一个样本之后没有"下一个"时，不要拿 0 去插值——那会在每个缓冲的末尾
    /// 插进一小段冲向静音的斜坡，听感上是每 20 毫秒一次的规律咔哒声，
    /// 而且它**只在非 48 kHz 的设备上出现**，报上来会是"某些人的声音有电流声"。
    #[test]
    fn the_tail_of_a_buffer_holds_its_last_value_instead_of_sliding_to_silence() {
        let input = vec![12_000i16; 240];
        let out = resample_to_48k(&input, 44_100);
        let tail = &out[out.len().saturating_sub(4)..];
        assert!(tail.iter().all(|&v| (v - 12_000).abs() <= 1),
            "the tail slid away from the signal: {tail:?}");
    }

    /// 设备枚举不该在没有声卡的机器上把客户端弄崩——CI 就是那种机器。
    #[test]
    fn enumerating_devices_never_panics() {
        let _ = input_devices();
        let _ = output_devices();
    }
}
