//! 收到管制消息时的提示音。
//!
//! **飞行员盯着的是窗外，消息区多出来的那一行谁也看不见。** 这个 crate 只管出声；
//! 该不该出声是 `can_voice_sim::chat::wants_alert` 的事，那是一个纯函数。
//!
//! 三条从 `can-audio/xpc/chime.py` 带过来的规矩，每一条都对应一次真实的失败：
//!
//! 1. **声音走客户端自己选的那块输出设备**，不是系统默认设备。飞行员的耳机和系统
//!    默认输出常常不是同一个，提示音响在桌面音箱里等于没响。这也是它不走前端
//!    `AudioContext` 的原因——浏览器那条路只认系统默认设备，正是这条规矩要躲开的。
//! 2. **波形是现场合成的，不带 wav 资源。** `opus.dll` 和 `SimConnect.dll` 那两个
//!    "文件没跟着打包走、程序照样启动、功能静默失效"的坑已经够多了。
//! 3. **放不出来只写一行日志。** 设备被独占、耳机拔了、机器上根本没有声卡——
//!    这些都不该影响收消息本身。

/// 合成用的采样率。
pub const RATE: u32 = 48_000;

/// 两声短促的**上行**音（A5 → E6）。上行听起来是"来消息了"，下行听起来像出错。
pub const TONES: [(f64, f64); 2] = [(880.0, 0.085), (1318.5, 0.110)];

/// 每一声之后都跟这么长的静音（秒）。注意是**每一声之后**，所以末尾也有一段。
pub const GAP: f64 = 0.02;

/// 淡入淡出（秒）。不是装饰：直接切会带一声"啪"的爆音。
pub const FADE: f64 = 0.006;

/// 相对满量程的幅度。无线电可能正在响，给它留够余量。
pub const GAIN: f64 = 0.22;

/// 两声之间的最短间隔（秒）。五条消息一起到的时候，用户要的是"有消息"这一个信息，
/// 不是连响五声。
pub const MIN_INTERVAL: f64 = 1.0;

/// 指定设备开不出来时按这个顺序退。**蓝牙耳机常常只吃 44.1 kHz。**
pub const FALLBACK_RATES: [u32; 3] = [48_000, 44_100, 22_050];

/// 合成一段提示音：16 位有符号单声道 PCM。
///
/// `volume` 是百分比，和麦克风/扬声器那两根滑条同一个量纲（0–200）。
pub fn waveform(rate: u32, volume: u32) -> Vec<i16> {
    let rate_f = f64::from(rate);
    let scale = GAIN * f64::from(volume) / 100.0;
    // 至少一个采样，免得采样率低到 fade 取整成 0 之后除出 inf。
    let fade = (rate_f * FADE).max(1.0);
    let gap = (rate_f * GAP) as usize;
    let mut out = Vec::new();
    for (freq, secs) in TONES {
        let count = (rate_f * secs) as usize;
        for i in 0..count {
            let at = i as f64;
            let envelope = 1.0_f64.min(at / fade).min((count as f64 - at) / fade);
            let value = (2.0 * std::f64::consts::PI * freq * at / rate_f).sin();
            out.push((value * envelope * scale).clamp(-1.0, 1.0) * 32767.0);
        }
        // 间隔跟在**每一声**后面，所以末尾也有一段——两声之间才分得开。
        out.resize(out.len() + gap, 0.0);
    }
    out.into_iter().map(|v| v as i16).collect()
}

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 放一声提示音，阻塞到放完。**从任意线程调都行，永远不 panic。**
///
/// `device_name` 是客户端自己选的那块输出设备的名字；给 `None` 或者找不到时退回
/// 系统默认设备——耳机在客户端启动之后被拔掉是常有的事，那时候退回默认总比
/// 一声不响强，无线电那条链路自己会报错。
///
/// 返回是否真的放出来了。放不出来**只写一行日志**：设备被独占、耳机拔了、
/// 机器上根本没有声卡，这些都不该影响收消息本身。
pub fn play_blocking(device_name: Option<&str>, volume: u32) -> bool {
    match try_play(device_name, volume) {
        Ok(()) => true,
        Err(e) => {
            tracing::info!("could not play the message chime: {e}");
            false
        }
    }
}

fn try_play(want: Option<&str>, volume: u32) -> Result<(), String> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();
    let mut devices = Vec::new();
    if let Some(want) = want.filter(|n| !n.is_empty()) {
        if let Ok(list) = host.output_devices() {
            if let Some(d) = list.into_iter().find(|d| d.name().is_ok_and(|n| n == want)) {
                devices.push(d);
            }
        }
    }
    let chosen = devices.len();
    if let Some(d) = host.default_output_device() {
        devices.push(d);
    }
    if devices.is_empty() {
        return Err("there is no output device at all".into());
    }

    let mut last = String::from("no configuration worked");
    for (idx, device) in devices.iter().enumerate() {
        for rate in rates_for(device) {
            match play_on(device, rate, volume) {
                Ok(()) => {
                    if chosen > 0 && idx >= chosen {
                        tracing::info!("the chime fell back to the default output device");
                    }
                    return Ok(());
                }
                Err(e) => last = e,
            }
        }
    }
    Err(last)
}

/// 先试设备自己的默认采样率，再按 [`FALLBACK_RATES`] 退。
fn rates_for(device: &cpal::Device) -> Vec<u32> {
    use cpal::traits::DeviceTrait;
    let mut rates = Vec::new();
    if let Ok(cfg) = device.default_output_config() {
        rates.push(cfg.sample_rate().0);
    }
    for r in FALLBACK_RATES {
        if !rates.contains(&r) {
            rates.push(r);
        }
    }
    rates
}

fn play_on(device: &cpal::Device, rate: u32, volume: u32) -> Result<(), String> {
    use cpal::traits::{DeviceTrait, StreamTrait};

    let supported = device
        .supported_output_configs()
        .map_err(|e| e.to_string())?
        .find(|c| c.min_sample_rate().0 <= rate && rate <= c.max_sample_rate().0)
        .ok_or_else(|| format!("no output config covers {rate} Hz"))?
        .with_sample_rate(cpal::SampleRate(rate));
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels as usize;

    // 合成在设备真正跑的那个采样率上，而不是重采样：这一段只有两声正弦，
    // 换个频率重算比把它重采样一遍简单得多，也不会有重采样的杂音。
    let pcm = Arc::new(waveform(rate, volume));
    let at = Arc::new(AtomicUsize::new(0));
    let total = pcm.len();

    macro_rules! fill {
        ($ty:ty, $conv:expr) => {{
            let pcm = Arc::clone(&pcm);
            let at = Arc::clone(&at);
            let conv: fn(i16) -> $ty = $conv;
            device.build_output_stream(
                &config,
                move |out: &mut [$ty], _: &cpal::OutputCallbackInfo| {
                    for frame in out.chunks_mut(channels) {
                        let i = at.fetch_add(1, Ordering::Relaxed);
                        let s = pcm.get(i).copied().unwrap_or(0);
                        for slot in frame.iter_mut() {
                            *slot = conv(s);
                        }
                    }
                },
                |e| tracing::info!("the chime stream faulted: {e}"),
                None,
            )
        }};
    }

    let stream = match format {
        cpal::SampleFormat::F32 => fill!(f32, |s| f32::from(s) / 32768.0),
        cpal::SampleFormat::I16 => fill!(i16, |s| s),
        cpal::SampleFormat::U16 => fill!(u16, |s| (i32::from(s) + 32768) as u16),
        other => return Err(format!("unsupported sample format {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;

    // 等它放完。**带上界**：设备可能开出来了却永远不回调，那时候宁可早点收摊,
    // 也不要把调用方的那条线程挂在这里。
    let limit = std::time::Duration::from_millis(1500);
    let began = std::time::Instant::now();
    while at.load(Ordering::Relaxed) < total && began.elapsed() < limit {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}

/// 放不放这一声的四道闸。
///
/// 和 [`waveform`] 一样是纯的：**不读时钟**，`now` 由调用方给，不然没法测。
#[derive(Debug, Default)]
pub struct Gate {
    last: Option<f64>,
    playing: bool,
}

impl Gate {
    pub fn new() -> Self {
        Self::default()
    }

    /// 这一声放不放。`now` 是单调秒。
    ///
    /// `force` 是设置里的"试听"：用户自己点的，**既不看开关也不受最短间隔限制**，
    /// 否则连点两下第二下没反应，看着像按钮坏了。但它**不**绕过音量 0
    /// （那是用户明确要静音），也**不**绕过重叠。
    pub fn allow(&mut self, now: f64, force: bool, enabled: bool, volume: u32) -> bool {
        if !force && !enabled {
            return false;
        }
        if volume == 0 {
            return false;
        }
        if self.playing {
            return false;
        }
        if !force && self.last.is_some_and(|last| now - last < MIN_INTERVAL) {
            return false;
        }
        self.playing = true;
        self.last = Some(now);
        true
    }

    /// 这一声放完了。
    pub fn finished(&mut self) {
        self.playing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total_seconds() -> f64 {
        TONES.iter().map(|(_, secs)| secs + GAP).sum()
    }

    /// 长度就是两声加两段间隔，约 235 毫秒。
    #[test]
    fn the_waveform_is_the_documented_length() {
        let pcm = waveform(RATE, 100);
        let want = (f64::from(RATE) * total_seconds()) as usize;
        // 每一声各自取整，误差不超过声音的条数。
        assert!(
            pcm.len().abs_diff(want) <= TONES.len() * 2,
            "{} 个采样，期望约 {want}",
            pcm.len()
        );
    }

    /// **淡入淡出不是装饰：直接切会有爆音。** 所以两端都必须从静音起步。
    #[test]
    fn it_fades_in_so_there_is_no_click() {
        let pcm = waveform(RATE, 100);
        let peak = pcm.iter().map(|s| s.unsigned_abs()).max().expect("非空");
        assert!(
            pcm[0].unsigned_abs() * 20 < peak,
            "第一个采样 {} 相对峰值 {peak} 太大，听起来就是一声啪",
            pcm[0]
        );
    }

    /// 音量 0 是静音，而且是合法取值——不是"没设置过"。
    #[test]
    fn volume_zero_is_silence() {
        assert!(waveform(RATE, 0).iter().all(|&s| s == 0));
    }

    /// 留余量：无线电可能正在响。
    #[test]
    fn the_peak_leaves_headroom_for_the_radio() {
        let peak = waveform(RATE, 100)
            .iter()
            .map(|s| s.unsigned_abs())
            .max()
            .expect("非空");
        let want = (GAIN * 32767.0) as u16;
        assert!(
            peak.abs_diff(want) < want / 10,
            "峰值 {peak}，期望约 {want}"
        );
        // 音量拉到头也不能削波。
        let loud = waveform(RATE, 200)
            .iter()
            .map(|s| s.unsigned_abs())
            .max()
            .expect("非空");
        assert!(loud <= 32767, "削波了：{loud}");
    }

    /// 每一声之后都有静音，所以结尾一定是静音——两声之间也才分得开。
    #[test]
    fn each_tone_is_followed_by_silence() {
        let pcm = waveform(RATE, 100);
        let gap = (f64::from(RATE) * GAP) as usize;
        assert!(
            pcm[pcm.len() - gap..].iter().all(|&s| s == 0),
            "结尾那段不是静音"
        );
    }

    /// 退避采样率要覆盖 44.1 kHz：蓝牙耳机常常只吃那一个。
    #[test]
    fn the_fallback_rates_cover_bluetooth() {
        assert!(FALLBACK_RATES.contains(&44_100));
        assert_eq!(FALLBACK_RATES[0], RATE, "第一个该是首选那一个");
    }

    /// 上行而不是下行。下行听起来像出错。
    #[test]
    fn the_two_tones_rise() {
        assert!(TONES[1].0 > TONES[0].0);
    }

    /// 关掉了就不响。
    #[test]
    fn the_switch_turns_it_off() {
        let mut g = Gate::new();
        assert!(!g.allow(0.0, false, false, 100));
        assert!(g.allow(0.0, false, true, 100));
    }

    /// **音量 0 连设备都不开**：开一次设备是有代价的，而且 0 是用户明确要静音。
    /// 试听也不能绕过它。
    #[test]
    fn volume_zero_opens_no_device_even_for_the_preview() {
        let mut g = Gate::new();
        assert!(!g.allow(0.0, false, true, 0));
        assert!(!g.allow(0.0, true, true, 0));
    }

    /// **上一声还没响完就不排队。** 五条消息一起到的时候，用户要的是"有消息"
    /// 这一个信息，不是连响五声。
    #[test]
    fn a_burst_collapses_into_one_chime() {
        let mut g = Gate::new();
        assert!(g.allow(0.0, false, true, 100));
        assert!(!g.allow(0.0, false, true, 100));
        assert!(
            !g.allow(0.0, true, true, 100),
            "试听也不能插队到正在响的那一声上"
        );
        g.finished();
        assert!(g.allow(MIN_INTERVAL + 0.1, false, true, 100));
    }

    /// 最短间隔之内不再响；过了就响。
    #[test]
    fn the_minimum_interval_holds() {
        let mut g = Gate::new();
        assert!(g.allow(10.0, false, true, 100));
        g.finished();
        assert!(!g.allow(10.0 + MIN_INTERVAL / 2.0, false, true, 100));
        g.finished();
        assert!(g.allow(10.0 + MIN_INTERVAL, false, true, 100));
    }

    /// 试听不受开关和最短间隔限制——连点两下要两下都响。
    #[test]
    fn the_preview_ignores_the_switch_and_the_interval() {
        let mut g = Gate::new();
        assert!(g.allow(0.0, true, false, 100));
        g.finished();
        assert!(g.allow(0.01, true, false, 100));
    }
}
