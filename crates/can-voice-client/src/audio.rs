//! 音频设备枚举与采样率适配。

use crate::rx::decode::SAMPLE_RATE;

/// 一个音频设备。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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

/// 单声道铺到 `channels` 个声道（每个采样重复 N 次）。
pub(crate) fn spread_mono(mono: &[i16], channels: u16) -> Vec<i16> {
    if channels == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(mono.len() * channels as usize);
    for &s in mono {
        for _ in 0..channels {
            out.push(s);
        }
    }
    out
}

/// 交错的多声道折成单声道：**取第一个声道，不取平均**。
///
/// 立体声接口上麦克风常常只接在一个声道上，另一个是静音的；取平均会让电平掉一半，
/// 症状是"我的声音很小"，而增益旋钮看起来一切正常。
pub(crate) fn fold_to_mono(interleaved: &[i16], channels: u16) -> Vec<i16> {
    if channels == 0 {
        return Vec::new();
    }
    interleaved
        .chunks_exact(channels as usize)
        .map(|c| c[0])
        .collect()
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
    let list = if input {
        host.input_devices()
    } else {
        host.output_devices()
    };
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

    // ——— 音频线程什么时候重建 ———

    /// **叫停压过一切。** 一条正在失败的流不该拦住关闭：那会让
    /// `AudioIo::drop` 在 join 上等到重建成功为止。
    #[test]
    fn stopping_wins_over_a_failed_stream() {
        assert_eq!(next_action(true, true, 7, 3), Next::Stop);
    }

    /// 设备报错就重开。不重开的表现是"突然听不见了，而界面全绿"——
    /// 拔一次耳机就是这样。
    #[test]
    fn a_failed_stream_is_rebuilt() {
        assert_eq!(next_action(false, true, 3, 3), Next::Rebuild);
    }

    /// 换了设备也重开，而且**不必等到下一次连接**。
    #[test]
    fn a_new_device_choice_is_rebuilt() {
        assert_eq!(next_action(false, false, 4, 3), Next::Rebuild);
    }

    /// 什么都没发生就别动它：重建会让声音断一下。
    #[test]
    fn a_healthy_stream_is_left_alone() {
        assert_eq!(next_action(false, false, 3, 3), Next::Keep);
    }

    /// **第一次重试要快**：拔掉的耳机常常马上插回来。
    /// 而一台根本没有声卡的机器不该每 200 毫秒被扫一次，所以有上界。
    #[test]
    fn the_retry_delay_starts_short_and_is_bounded() {
        assert_eq!(retry_delay(0), std::time::Duration::from_millis(200));
        let far = retry_delay(99);
        assert!(far <= MAX_RETRY_DELAY, "{far:?} 超过了上界");
        for a in 0..10 {
            assert!(retry_delay(a) <= retry_delay(a + 1), "第 {a} 次退避不该变短");
        }
    }

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
        assert!(
            (out.len() as i32 - 480).abs() <= 1,
            "441 samples at 44.1k is 10 ms = 480 at 48k, got {}",
            out.len()
        );
    }

    #[test]
    fn resampling_preserves_a_constant_signal() {
        // 常数信号重采样后还该是同一个常数 —— 插值出别的值说明算错了。
        let input = vec![1000i16; 480];
        let out = resample_to_48k(&input, 24_000);
        assert!(
            out.iter().all(|&v| (v - 1000).abs() <= 1),
            "a constant signal must survive resampling, got {:?}",
            &out[..8]
        );
    }

    #[test]
    fn the_two_directions_round_trip_approximately() {
        let input: Vec<i16> = (0..480)
            .map(|i| ((i as f32 / 10.0).sin() * 8000.0) as i16)
            .collect();
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
        assert!(
            tail.iter().all(|&v| (v - 12_000).abs() <= 1),
            "the tail slid away from the signal: {tail:?}"
        );
    }

    /// 设备枚举不该在没有声卡的机器上把客户端弄崩——CI 就是那种机器。
    #[test]
    fn enumerating_devices_never_panics() {
        let _ = input_devices();
        let _ = output_devices();
    }

    // ——— 声道映射 ———

    #[test]
    fn mono_is_replicated_to_every_output_channel() {
        assert_eq!(spread_mono(&[1, 2], 2), vec![1, 1, 2, 2]);
        assert_eq!(spread_mono(&[1, 2], 1), vec![1, 2]);
        assert_eq!(spread_mono(&[5], 4), vec![5, 5, 5, 5]);
    }

    /// 采集取**第一个声道**，不取平均。
    ///
    /// 立体声接口上麦克风常常只接在一个声道上，另一个是静音的；取平均会让电平
    /// 掉一半，症状是"我的声音很小"，而增益旋钮看起来一切正常。
    #[test]
    fn capture_takes_the_first_channel_rather_than_averaging() {
        // 左 1000、右 0 —— 只有一侧接了麦克风。
        assert_eq!(fold_to_mono(&[1000, 0, 2000, 0], 2), vec![1000, 2000]);
        assert_eq!(fold_to_mono(&[7, 8, 9], 1), vec![7, 8, 9]);
    }

    #[test]
    fn channel_mapping_survives_a_ragged_tail() {
        // 回调给的缓冲不一定是声道数的整数倍。
        assert_eq!(fold_to_mono(&[1, 2, 3], 2), vec![1]);
        assert!(fold_to_mono(&[], 2).is_empty());
        assert!(spread_mono(&[], 2).is_empty());
    }

    #[test]
    fn a_zero_channel_count_does_not_divide_by_zero() {
        // 设备报 0 声道是不该发生的，但除零会让整个客户端当场崩掉。
        assert!(fold_to_mono(&[1, 2], 0).is_empty());
        assert!(spread_mono(&[1, 2], 0).is_empty());
    }
}

// ——— 设备 I/O ———
//
// 这一段是**唯一碰声卡的地方**，也是唯一在 CI 上跑不到的地方（runner 没有声卡）。
// 所以能挤出来的纯函数都挤出来了：声道映射和采样率适配在上面，各自带测试；
// 这里只剩"把 cpal 的回调接到两个环形缓冲上"。
//
// # 为什么要一个自己的线程
//
// **`cpal::Stream` 在 macOS 上是 `!Send`。** 它不能被搬进 tokio 的任务里，也不能
// 跨线程持有。所以这里起一条自己的 OS 线程：它建流、播放、然后停在那儿直到被叫停，
// 而与 tokio 那一侧的全部往来都经由两个 `Arc<Mutex<VecDeque<i16>>>`。
//
// 锁在音频回调里只持一次 memcpy 的时间。所有 DSP（抖动缓冲、解码、混音、编码）
// 都在 tokio 那一侧，回调里一行都没有。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// 环形缓冲最多存多少毫秒。
///
/// 播放侧攒多了就是延迟，而延迟在无线电通话里比偶尔一次欠载难受得多；
/// 采集侧攒多了是同一回事，而且落后的音频追不回来。
const RING_MS: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no {0} device")]
    NoDevice(&'static str),
    #[error("device config: {0}")]
    Config(String),
    #[error("build stream: {0}")]
    Build(String),
    #[error("play stream: {0}")]
    Play(String),
    #[error("unsupported sample format {0:?}")]
    SampleFormat(cpal::SampleFormat),
}

/// 重建失败之后最多等多久再试。
const MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

/// 音频线程这一轮之后该做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// 上层叫停了。
    Stop,
    /// 重建两条流。
    Rebuild,
    /// 什么都不做。
    Keep,
}

/// 决定下一步。
///
/// **叫停压过一切**：一条正在失败的流不该拦住关闭，否则 `AudioIo::drop`
/// 会在 join 上一直等到重建成功为止。
fn next_action(stop: bool, failed: bool, wanted: u64, built: u64) -> Next {
    if stop {
        Next::Stop
    } else if failed || wanted != built {
        Next::Rebuild
    } else {
        Next::Keep
    }
}

/// 第 `attempt` 次重建失败之后等多久。
///
/// 从 200 毫秒翻倍退到 [`MAX_RETRY_DELAY`]：拔掉的耳机常常马上插回来，所以
/// 第一次要快；而一台根本没有声卡的机器不该每 200 毫秒被扫一次。
fn retry_delay(attempt: u32) -> std::time::Duration {
    let ms = 200u64.saturating_mul(1u64 << attempt.min(10));
    std::time::Duration::from_millis(ms).min(MAX_RETRY_DELAY)
}

/// 一条打开着的音频通路。
///
/// 两个环形缓冲都存**设备采样率的单声道**样本：把重采样放在 tokio 那一侧，
/// 是因为那里才知道自己要的是一整帧还是一段任意长度。
///
/// # 流会被重建，所以采样率是原子量
///
/// 设备掉了要重开，而重开出来的设备采样率可能和原来那个不一样。把它存成
/// 一个普通的 `u32` 的话，重建之后 `play()` 会按旧采样率重采样，结果是
/// 一路变调的声音——比听不见更难查。
pub struct AudioIo {
    playback: Arc<Mutex<VecDeque<i16>>>,
    capture: Arc<Mutex<VecDeque<i16>>>,
    output_rate: Arc<AtomicU32>,
    input_rate: Arc<AtomicU32>,
    /// 此刻有没有活着的流。
    running: Arc<AtomicBool>,
    /// 想用哪两个设备。
    wanted: Arc<Mutex<(Option<String>, Option<String>)>>,
    /// 换设备的代数。变了就重建。
    generation: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AudioIo {
    /// 打开输入与输出。`None` 表示用系统默认设备。
    ///
    /// **第一次建不起来仍然是错误**：那一刻上层要据此说"声卡打不开"。
    /// 建起来之后这条线程就不再放手——设备掉了会退避重连，换设备会重建。
    pub fn start(input: Option<&str>, output: Option<&str>) -> Result<Self, Error> {
        let playback: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::new()));
        let capture: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let output_rate = Arc::new(AtomicU32::new(48_000));
        let input_rate = Arc::new(AtomicU32::new(48_000));
        let wanted = Arc::new(Mutex::new((
            input.map(str::to_string),
            output.map(str::to_string),
        )));
        let generation = Arc::new(AtomicU64::new(0));

        let (tx, rx) = std::sync::mpsc::channel::<Result<(), Error>>();
        let (pb, cap, st) = (playback.clone(), capture.clone(), stop.clone());
        let (run, fail) = (running.clone(), failed.clone());
        let (orate, irate) = (output_rate.clone(), input_rate.clone());
        let (want, gen) = (wanted.clone(), generation.clone());

        let thread = std::thread::Builder::new()
            .name("can-voice-audio".into())
            .spawn(move || {
                let mut first = Some(tx);
                let mut attempt = 0u32;
                loop {
                    if st.load(Ordering::Relaxed) {
                        return;
                    }
                    let (want_in, want_out) = match want.lock() {
                        Ok(w) => w.clone(),
                        Err(p) => p.into_inner().clone(),
                    };
                    // 每一轮各读一次：这一轮建的就是这一代，之后代数再变
                    // 才算"换了设备"。
                    let built_gen = gen.load(Ordering::Relaxed);
                    let built = build_streams(
                        want_in.as_deref(),
                        want_out.as_deref(),
                        pb.clone(),
                        cap.clone(),
                        fail.clone(),
                    );
                    let (out_stream, in_stream, (o, i)) = match built {
                        Ok(v) => v,
                        Err(e) => {
                            run.store(false, Ordering::Relaxed);
                            // **第一次失败就报给调用方**，语义和从前一样：
                            // 上层要在那一刻说"声卡打不开，听不见也发不出"。
                            if let Some(tx) = first.take() {
                                let _ = tx.send(Err(e));
                                return;
                            }
                            tracing::warn!(error = %e, "could not reopen the audio devices; will retry");
                            std::thread::park_timeout(retry_delay(attempt));
                            attempt = attempt.saturating_add(1);
                            continue;
                        }
                    };
                    attempt = 0;
                    orate.store(o, Ordering::Relaxed);
                    irate.store(i, Ordering::Relaxed);
                    fail.store(false, Ordering::Relaxed);
                    run.store(true, Ordering::Relaxed);
                    if let Some(tx) = first.take() {
                        let _ = tx.send(Ok(()));
                    }

                    // 流必须活在这条线程上（`!Send`），所以就停在这儿。
                    let mut stopping = false;
                    loop {
                        std::thread::park_timeout(std::time::Duration::from_millis(100));
                        match next_action(
                            st.load(Ordering::Relaxed),
                            fail.load(Ordering::Relaxed),
                            gen.load(Ordering::Relaxed),
                            built_gen,
                        ) {
                            Next::Keep => continue,
                            Next::Stop => {
                                stopping = true;
                                break;
                            }
                            Next::Rebuild => break,
                        }
                    }
                    run.store(false, Ordering::Relaxed);
                    drop(in_stream);
                    drop(out_stream);
                    if stopping {
                        return;
                    }
                }
            })
            .map_err(|e| Error::Build(e.to_string()))?;

        match rx.recv() {
            Ok(Ok(())) => Ok(Self {
                playback,
                capture,
                output_rate,
                input_rate,
                running,
                wanted,
                generation,
                stop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Error::Build("the audio thread died during setup".into())),
        }
    }

    pub fn output_rate(&self) -> u32 {
        self.output_rate.load(Ordering::Relaxed)
    }

    pub fn input_rate(&self) -> u32 {
        self.input_rate.load(Ordering::Relaxed)
    }

    /// 此刻有没有活着的音频流。
    ///
    /// 上层每一拍读它：从 true 变 false 要对用户说一句。"能连上、状态绿、
    /// 说话没人听见"是这个项目反复要躲开的那类故障。
    pub fn running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// 换设备。**立刻生效**，不必等到下一次连接。
    ///
    /// 只是记下想要哪两个并叫醒音频线程；真正的重建在那条线程上做，
    /// 因为 `cpal::Stream` 是 `!Send`。
    pub fn set_devices(&self, input: Option<&str>, output: Option<&str>) {
        {
            let mut w = match self.wanted.lock() {
                Ok(w) => w,
                Err(p) => p.into_inner(),
            };
            let next = (input.map(str::to_string), output.map(str::to_string));
            if *w == next {
                // 没变就别重建：重建会让声音断一下。
                return;
            }
            *w = next;
        }
        self.generation.fetch_add(1, Ordering::Relaxed);
        if let Some(t) = self.thread.as_ref() {
            t.thread().unpark();
        }
    }

    /// 送一帧 48 kHz 单声道 PCM 去播放。
    pub fn play(&self, pcm48: &[i16]) {
        let rate = self.output_rate();
        let at_device_rate = resample_from_48k(pcm48, rate);
        let cap = rate as usize * RING_MS / 1000;
        let mut ring = match self.playback.lock() {
            Ok(r) => r,
            Err(p) => p.into_inner(),
        };
        ring.extend(at_device_rate);
        while ring.len() > cap {
            // 溢出时丢**最旧**的：留着只会让听到的话越来越落后于说出来的话。
            ring.pop_front();
        }
    }

    /// 取走采集到的全部音频，重采样到 48 kHz 单声道。
    pub fn take_capture(&self) -> Vec<i16> {
        let raw: Vec<i16> = {
            let mut ring = match self.capture.lock() {
                Ok(r) => r,
                Err(p) => p.into_inner(),
            };
            ring.drain(..).collect()
        };
        resample_to_48k(&raw, self.input_rate())
    }
}

impl Drop for AudioIo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

type Streams = (cpal::Stream, cpal::Stream, (u32, u32));

fn build_streams(
    input: Option<&str>,
    output: Option<&str>,
    playback: Arc<Mutex<VecDeque<i16>>>,
    capture: Arc<Mutex<VecDeque<i16>>>,
    failed: Arc<AtomicBool>,
) -> Result<Streams, Error> {
    use cpal::traits::{DeviceTrait, StreamTrait};

    let host = cpal::default_host();
    let out_dev = pick(&host, output, false).ok_or(Error::NoDevice("output"))?;
    let in_dev = pick(&host, input, true).ok_or(Error::NoDevice("input"))?;

    let out_cfg = preferred_config(&out_dev, false)?;
    let in_cfg = preferred_config(&in_dev, true)?;
    let out_rate = out_cfg.sample_rate().0;
    let in_rate = in_cfg.sample_rate().0;
    let out_ch = out_cfg.channels();
    let in_ch = in_cfg.channels();

    // **回调报错要被记下来**，光 warn 一行的后果是"突然听不见了而界面全绿"：
    // 拔一次耳机就是这样。音频线程看这个标志决定要不要重开。
    //
    // 是一个工厂而不是一个闭包：四种采样格式各建一条流，而闭包不是 Copy。
    let make_err_fn = || {
        let failed = failed.clone();
        move |e| {
            tracing::warn!(error = %e, "audio stream error");
            failed.store(true, Ordering::Relaxed);
        }
    };

    let out_stream = match out_cfg.sample_format() {
        cpal::SampleFormat::I16 => out_dev.build_output_stream(
            &out_cfg.config(),
            move |buf: &mut [i16], _| fill_output(buf, out_ch, &playback),
            make_err_fn(),
            None,
        ),
        cpal::SampleFormat::F32 => out_dev.build_output_stream(
            &out_cfg.config(),
            move |buf: &mut [f32], _| {
                let mut tmp = vec![0i16; buf.len()];
                fill_output(&mut tmp, out_ch, &playback);
                for (d, s) in buf.iter_mut().zip(tmp) {
                    *d = s as f32 / 32768.0;
                }
            },
            make_err_fn(),
            None,
        ),
        other => return Err(Error::SampleFormat(other)),
    }
    .map_err(|e| Error::Build(e.to_string()))?;

    let in_stream = match in_cfg.sample_format() {
        cpal::SampleFormat::I16 => in_dev.build_input_stream(
            &in_cfg.config(),
            move |buf: &[i16], _| take_input(buf, in_ch, &capture, in_rate),
            make_err_fn(),
            None,
        ),
        cpal::SampleFormat::F32 => in_dev.build_input_stream(
            &in_cfg.config(),
            move |buf: &[f32], _| {
                let tmp: Vec<i16> = buf
                    .iter()
                    .map(|v| (v.clamp(-1.0, 1.0) * 32767.0) as i16)
                    .collect();
                take_input(&tmp, in_ch, &capture, in_rate);
            },
            make_err_fn(),
            None,
        ),
        other => return Err(Error::SampleFormat(other)),
    }
    .map_err(|e| Error::Build(e.to_string()))?;

    out_stream.play().map_err(|e| Error::Play(e.to_string()))?;
    in_stream.play().map_err(|e| Error::Play(e.to_string()))?;
    tracing::info!(out_rate, in_rate, out_ch, in_ch, "audio devices opened");
    Ok((out_stream, in_stream, (out_rate, in_rate)))
}

/// 回调里只做这一件事：从环里取够，不够就补静音。
fn fill_output(buf: &mut [i16], channels: u16, ring: &Arc<Mutex<VecDeque<i16>>>) {
    let frames = buf.len() / channels.max(1) as usize;
    let mut mono = Vec::with_capacity(frames);
    {
        let mut r = match ring.lock() {
            Ok(r) => r,
            Err(p) => p.into_inner(),
        };
        for _ in 0..frames {
            mono.push(r.pop_front().unwrap_or(0));
        }
    }
    let interleaved = spread_mono(&mono, channels);
    buf[..interleaved.len()].copy_from_slice(&interleaved);
}

fn take_input(buf: &[i16], channels: u16, ring: &Arc<Mutex<VecDeque<i16>>>, rate: u32) {
    let mono = fold_to_mono(buf, channels);
    let cap = rate as usize * RING_MS / 1000;
    let mut r = match ring.lock() {
        Ok(r) => r,
        Err(p) => p.into_inner(),
    };
    r.extend(mono);
    while r.len() > cap {
        r.pop_front();
    }
}

fn pick(host: &cpal::Host, name: Option<&str>, input: bool) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    match name {
        None => {
            if input {
                host.default_input_device()
            } else {
                host.default_output_device()
            }
        }
        Some(want) => {
            let list = if input {
                host.input_devices()
            } else {
                host.output_devices()
            };
            list.ok()?
                .find(|d| d.name().map(|n| n == want).unwrap_or(false))
        }
    }
}

/// 优先要 48 kHz：那是 Opus 的采样率，拿到它就完全不必重采样。
///
/// 拿不到才退回设备默认值并在两端加重采样——旧的 Python 版在这里直接放弃，
/// 注释写着"48 kHz 是理想路径，回退采样率会产生变调音频"，也就是说设备不支持时
/// 用户听到的是变调的声音。
fn preferred_config(dev: &cpal::Device, input: bool) -> Result<cpal::SupportedStreamConfig, Error> {
    use cpal::traits::DeviceTrait;
    // 两个迭代器的类型不同但元素类型相同，所以各自收成 Vec。
    let ranges: Vec<cpal::SupportedStreamConfigRange> = if input {
        dev.supported_input_configs()
            .map(|it| it.collect())
            .unwrap_or_default()
    } else {
        dev.supported_output_configs()
            .map(|it| it.collect())
            .unwrap_or_default()
    };
    {
        let wanted = cpal::SampleRate(SAMPLE_RATE);
        for r in ranges {
            if r.min_sample_rate() <= wanted && wanted <= r.max_sample_rate() {
                return Ok(r.with_sample_rate(wanted));
            }
        }
    }
    if input {
        dev.default_input_config()
    } else {
        dev.default_output_config()
    }
    .map_err(|e| Error::Config(e.to_string()))
}
