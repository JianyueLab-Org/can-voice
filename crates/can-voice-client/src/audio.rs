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
#[cfg(test)]
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
#[cfg(test)]
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

    struct CountingAllocator;
    thread_local! {
        static COUNT_ALLOCATIONS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static ALLOCATION_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
            COUNT_ALLOCATIONS
                .try_with(|on| {
                    if on.get() {
                        let _ = ALLOCATION_COUNT.try_with(|count| count.set(count.get() + 1));
                    }
                })
                .ok();
            std::alloc::System.alloc(layout)
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
            std::alloc::System.dealloc(ptr, layout);
        }
    }

    #[global_allocator]
    static TEST_ALLOCATOR: CountingAllocator = CountingAllocator;

    #[test]
    fn callback_rendering_reuses_its_buffers() {
        let ring = Arc::new(Mutex::new(VecDeque::from(vec![1000i16; 6000])));
        let clock = PlaybackClock::new();
        clock.on_rebuild(48_000);
        let mut output = vec![0f32; 480];
        let mut pcm = vec![0i16; 480];
        let mut mono = Vec::with_capacity(481);
        render_f32(&mut output, 1, &ring, &clock, &mut pcm, &mut mono);
        ALLOCATION_COUNT.with(|count| count.set(0));
        COUNT_ALLOCATIONS.with(|on| on.set(true));
        render_f32(&mut output, 1, &ring, &clock, &mut pcm, &mut mono);
        COUNT_ALLOCATIONS.with(|on| on.set(false));
        let allocations = ALLOCATION_COUNT.with(std::cell::Cell::get);
        assert_eq!(allocations, 0, "audio output callback allocated");
        assert!(output.iter().all(|sample| *sample > 0.0));
    }

    #[test]
    fn f32_capture_reuses_its_buffer() {
        let ring = Arc::new(Mutex::new(VecDeque::with_capacity(9600)));
        let samples = vec![0.5f32; 960];
        let mut pcm = vec![0i16; 960];
        capture_f32(&samples, 2, &ring, 48_000, &mut pcm);
        ALLOCATION_COUNT.with(|count| count.set(0));
        COUNT_ALLOCATIONS.with(|on| on.set(true));
        capture_f32(&samples, 2, &ring, 48_000, &mut pcm);
        COUNT_ALLOCATIONS.with(|on| on.set(false));
        let allocations = ALLOCATION_COUNT.with(std::cell::Cell::get);
        assert_eq!(allocations, 0, "audio input callback allocated");
        assert_eq!(ring.lock().unwrap().len(), 960);
    }

    // ——— 挑一个我们建得出来的采样格式 ———

    /// **设备报的第一条能用的采样率，不一定是我们建得出流的格式。**
    ///
    /// 这条是一份真实日志换来的：一台 Windows 机器上，`audio-for-can` 从
    /// v27.0.4 到 v27.0.8 九次启动、三天，每一次都是
    /// `could not open the audio devices; running deaf and mute
    /// error=unsupported sample format U8`，再每 5 秒重试一次、45 次全败。
    /// 连得上、界面全绿、频率认领正常，就是**又聋又哑**。
    ///
    /// 原因不是"不支持 U8"，是**我们自己挑了一个自己不接受的**：挑选只看采样率
    /// 覆不覆盖 48 kHz，而建流只认 I16 / F32。那台设备同时提供 F32，我们没看。
    /// 重试也救不了——同一份候选表挑出同一条，注定每次都失败。
    #[test]
    fn a_format_we_cannot_build_is_skipped_for_one_we_can() {
        use cpal::SampleFormat::{F32, U8};
        let ranges = [(U8, 8_000, 48_000), (F32, 8_000, 48_000)];
        assert_eq!(choose_config(&ranges, 48_000), Some(1));
    }

    /// 48 kHz 仍然优先——拿到它就完全不必重采样。
    #[test]
    fn a_buildable_range_covering_48k_beats_one_that_does_not() {
        use cpal::SampleFormat::{F32, I16};
        let ranges = [(F32, 8_000, 44_100), (I16, 8_000, 48_000)];
        assert_eq!(choose_config(&ranges, 48_000), Some(1));
    }

    /// 都覆盖 48 kHz 时按格式偏好挑，而不是按设备报的顺序。
    #[test]
    fn among_buildable_formats_the_preference_order_decides() {
        use cpal::SampleFormat::{F32, I16};
        let ranges = [(I16, 8_000, 48_000), (F32, 8_000, 48_000)];
        assert_eq!(choose_config(&ranges, 48_000), Some(1));
    }

    /// 一条都建不出来时说不出来，而不是随便挑一条回去让建流阶段再炸一次。
    /// **`None` 和"挑了一条建不出的"是两件事**：前者上层可以去问默认配置，
    /// 后者是这个 bug 本身。
    #[test]
    fn nothing_buildable_is_none() {
        use cpal::SampleFormat::{U16, U8};
        let ranges = [(U8, 8_000, 48_000), (U16, 8_000, 48_000)];
        assert_eq!(choose_config(&ranges, 48_000), None);
    }

    /// 没有一条覆盖 48 kHz 时，退到建得出的那一条里最高的采样率，
    /// 两端加重采样。**不要在这里放弃**——旧的 Python 版就是这样，
    /// 注释写着"回退采样率会产生变调音频"，也就是说它知道会变调还是放了出去。
    #[test]
    fn without_48k_the_best_buildable_range_still_wins() {
        use cpal::SampleFormat::{F32, U8};
        let ranges = [(U8, 8_000, 48_000), (F32, 8_000, 44_100)];
        assert_eq!(choose_config(&ranges, 48_000), Some(1));
    }

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
            assert!(
                retry_delay(a) <= retry_delay(a + 1),
                "第 {a} 次退避不该变短"
            );
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

    /// 点名的设备不在了，用系统默认。一直钉着旧名字的表现是拔掉耳机之后
    /// 整场会话又聋又哑，插上别的设备也不恢复。
    #[test]
    fn a_missing_named_device_falls_back_to_the_default() {
        assert_eq!(
            fallback_name(Some("AirPods"), &["Built-in"], Some("Built-in")).as_deref(),
            Some("Built-in")
        );
        assert_eq!(
            fallback_name(Some("AirPods"), &["AirPods", "Built-in"], Some("Built-in")).as_deref(),
            Some("AirPods")
        );
        assert_eq!(
            fallback_name(None, &["AirPods"], Some("Built-in")).as_deref(),
            Some("Built-in")
        );
        assert_eq!(fallback_name(Some("gone"), &[], None), None);
    }

    #[test]
    fn a_speaker_test_tone_is_audible_and_the_right_length() {
        let pcm = test_tone(100);
        assert_eq!(pcm.len(), 48_000 / 10);
        let peak = pcm.iter().map(|s| s.abs()).max().unwrap_or(0);
        assert!(peak > 1000, "tone was effectively silent: peak {peak}");
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

    // ——— 播放环的时钟对账 ———
    //
    // 这一段是**可测的**，尽管下面那段注释说 CI 的 runner 没有声卡：
    // `fill_output` 是一个纯函数（一个环加一片缓冲），`push_playback` 是生产者
    // 那一侧的全部。把两者按各自的节拍交替叫一遍，就能在没有声卡的机器上把两个
    // 时钟之间的任意偏差演出来——而这正是这一段代码唯一要处理的事。

    /// 一段定值音频，长 `n` 个样本。
    fn tone(n: usize) -> Vec<i16> {
        vec![1000i16; n]
    }

    /// 造一个跑在 `rate` 上的空环与它的时钟。
    fn ring_and_clock(rate: u32) -> (Arc<Mutex<VecDeque<i16>>>, Arc<PlaybackClock>) {
        let clock = Arc::new(PlaybackClock::new());
        clock.on_rebuild(rate);
        (Arc::new(Mutex::new(VecDeque::new())), clock)
    }

    /// 按 `ratio` 的速率比跑 `callbacks` 次回调。
    ///
    /// 生产者每次推 `frames * ratio` 个样本（小数用累加器摊平，模拟一个稳定
    /// 但和声卡晶振对不上的时钟），消费者每次要 `frames` 个。
    fn drift(rate: u32, frames: usize, callbacks: usize, ratio: f64) -> PlaybackStats {
        let (ring, clock) = ring_and_clock(rate);
        let mut buf = vec![0i16; frames];
        let chunk = tone(frames * 2);
        let mut owed = 0.0f64;
        for _ in 0..callbacks {
            owed += frames as f64 * ratio;
            let n = (owed.floor() as usize).min(chunk.len());
            owed -= n as f64;
            push_playback(&ring, &chunk[..n], &clock);
            fill_output(&mut buf, 1, &ring, &clock);
        }
        clock.stats()
    }

    /// 环没攒够水位之前必须是静音。
    ///
    /// 流是立刻开始播的，而环是空的，所以第一瞬间回调就在一个空环上取数——
    /// 旧代码在那里 `unwrap_or(0)`，于是每一次连接的开头都先播一段静音里
    /// 夹着零星样本的东西，听感就是"电音"。
    #[test]
    fn the_output_is_silent_until_the_ring_is_primed() {
        let (ring, clock) = ring_and_clock(48_000);
        let mut buf = vec![0i16; 960];

        // 只有一帧，离 60 毫秒的目标水位还差两帧。
        push_playback(&ring, &tone(960), &clock);
        fill_output(&mut buf, 1, &ring, &clock);
        assert!(buf.iter().all(|&s| s == 0), "没攒够就出声了");

        push_playback(&ring, &tone(960), &clock);
        fill_output(&mut buf, 1, &ring, &clock);
        assert!(buf.iter().all(|&s| s == 0), "没攒够就出声了");

        // 第三帧攒够了，这一拍开始出声。
        push_playback(&ring, &tone(960), &clock);
        fill_output(&mut buf, 1, &ring, &clock);
        assert!(buf.iter().all(|&s| s == 1000), "攒够了还不出声");
    }

    /// 两端速率一致时一个样本都不该动。
    ///
    /// 转向改的是波形本身，所以它只在水位跑出死区时才动手；稳态下动手就是在和
    /// 相位较劲——生产者每 20 毫秒一次性推一整帧，回调落在推之前还是之后，
    /// 测到的水位本来就差一整帧。
    #[test]
    fn a_matched_producer_and_consumer_neither_add_nor_drop_samples() {
        // 3000 次回调 = 60 秒。
        let s = drift(48_000, 960, 3_000, 1.0);
        assert_eq!(s.steer_added, 0, "稳态下补了样本: {s:?}");
        assert_eq!(s.steer_removed, 0, "稳态下丢了样本: {s:?}");
        assert_eq!(s.underruns, 0, "{s:?}");
        assert_eq!(s.trimmed_ms, 0, "{s:?}");
    }

    /// 生产者慢 0.5% 要被转向吃掉，环不能空。
    ///
    /// **回调缓冲取 128 帧是有意的。** 每个回调只改一个样本，所以转向的权限
    /// 恰好是 `1/frames`：128 帧的缓冲上是 0.78%，够吃 0.5%；而 960 帧的缓冲上
    /// 只有 0.1%，够吃的是这个缺陷真正针对的 ±100 ppm 晶振偏差（0.01%），
    /// 留了 10 倍余量。偏差超出权限时不是靠改一个样本能补的——那时新加的这几个
    /// 计数器就是用来把它认出来的东西。
    #[test]
    fn a_slow_producer_is_absorbed_without_the_ring_running_dry() {
        // 50000 次回调 × 128 帧 ≈ 133 秒。
        let s = drift(48_000, 128, 50_000, 0.995);
        assert_eq!(s.underruns, 0, "环被吃空了: {s:?}");
        assert_eq!(s.silence_ms, 0, "补进了静音: {s:?}");
        assert!(s.steer_added > 0, "转向根本没动手: {s:?}");
        assert!(s.depth_ms > 0, "{s:?}");
    }

    /// 生产者快 0.5% 也要被转向吃掉，不能撞上 200 毫秒那一刀。
    ///
    /// 那一刀是硬接（丢掉最旧的一整段），听感是"卡顿"；转向是每个回调多吃一个
    /// 样本，听不出来。
    #[test]
    fn a_fast_producer_is_absorbed_without_hitting_the_hard_ceiling() {
        let s = drift(48_000, 128, 50_000, 1.005);
        assert_eq!(s.trimmed_ms, 0, "撞上了硬上限: {s:?}");
        assert!(s.steer_removed > 0, "转向根本没动手: {s:?}");
        assert!(s.depth_ms <= RING_MS as u32, "水位超过了硬上限: {s:?}");
    }

    /// 一段欠载是**一段**，不是它跨过的每一个回调各算一次。
    ///
    /// 回调每秒跑几十次，按回调计数的话这个数字只能说明"回调在跑"。
    #[test]
    fn an_underrun_is_one_episode_however_many_callbacks_it_spans() {
        let (ring, clock) = ring_and_clock(48_000);
        let mut buf = vec![0i16; 960];
        // 攒够水位、放起来。
        push_playback(&ring, &tone(2_880), &clock);
        fill_output(&mut buf, 1, &ring, &clock);

        // 生产者停了：接下来每个回调都在补静音，但这只是一段欠载。
        for _ in 0..50 {
            fill_output(&mut buf, 1, &ring, &clock);
        }
        let s = clock.stats();
        assert_eq!(s.underruns, 1, "50 个回调报成了 {} 段", s.underruns);
        assert!(s.silence_ms > 0, "补了静音却没记下来: {s:?}");
        assert_eq!(s.depth_ms, 0, "{s:?}");
    }

    /// 环空了要**重新攒水位**，而不是空着硬撑。
    ///
    /// 空着硬撑的表现就是测试员报上来的那一对症状：每个回调补一点静音（电音），
    /// 而攒不出连续的一段（卡顿）。
    #[test]
    fn a_drained_ring_primes_again_instead_of_limping_along_empty() {
        let (ring, clock) = ring_and_clock(48_000);
        let mut buf = vec![0i16; 960];
        push_playback(&ring, &tone(2_880), &clock);
        for _ in 0..8 {
            fill_output(&mut buf, 1, &ring, &clock);
        }
        // 吃空之后生产者只回来一帧：还不够，必须继续静音。
        push_playback(&ring, &tone(960), &clock);
        fill_output(&mut buf, 1, &ring, &clock);
        assert!(buf.iter().all(|&s| s == 0), "没重新攒够就又出声了");

        // 攒够三帧才重新出声。
        push_playback(&ring, &tone(1_920), &clock);
        fill_output(&mut buf, 1, &ring, &clock);
        assert!(buf.iter().all(|&s| s == 1000), "重新攒够了还不出声");
    }

    /// 欠载只报第一行，清除时报一行汇总；中间一行都不许有。
    ///
    /// 这条路在实时音频线程上，每个回调打一行的后果参见 `stack.rs` 上一次修的
    /// 那个缺陷：一份 1 MB 的日志里 7545 行有 7536 行是同一句。
    #[test]
    fn a_dry_ring_is_reported_once_and_summarised_when_it_refills() {
        let log = capture_log(|| {
            let (ring, clock) = ring_and_clock(48_000);
            let mut buf = vec![0i16; 960];
            push_playback(&ring, &tone(2_880), &clock);
            clock.report();
            for _ in 0..200 {
                fill_output(&mut buf, 1, &ring, &clock);
                // 生产者那一侧每一拍都会看一眼计数器。
                clock.report();
            }
            // 生产者回来了，水位攒回去，汇总应当在这里出现。
            for _ in 0..4 {
                push_playback(&ring, &tone(960), &clock);
                clock.report();
                fill_output(&mut buf, 1, &ring, &clock);
            }
            clock.report();
        });
        assert_eq!(
            log.matches("playback ring ran dry").count(),
            1,
            "欠载报了不止一次:\n{log}"
        );
        assert_eq!(
            log.matches("playback ring refilled").count(),
            1,
            "汇总行不是恰好一行:\n{log}"
        );
    }

    /// 水位被一次突发顶高之后，转向要把它拉回死区。
    ///
    /// 只在 200 毫秒处裁是不够的：裁不到的那一段（比如 150 毫秒）会一直留着，
    /// 而它就是永久的额外延迟。
    #[test]
    fn the_depth_is_steered_back_toward_target_after_a_disturbance() {
        let (ring, clock) = ring_and_clock(48_000);
        let frames = 960usize;
        let mut buf = vec![0i16; frames];
        let burst = tone(9_600);
        // 一次突发把环灌到 200 毫秒。
        push_playback(&ring, &burst, &clock);
        // 之后两端速率完全一致：能把水位拉回来的只有转向。
        for _ in 0..20_000 {
            push_playback(&ring, &burst[..frames], &clock);
            fill_output(&mut buf, 1, &ring, &clock);
        }
        let s = clock.stats();
        assert!((40..=80).contains(&s.depth_ms), "水位没回到目标附近: {s:?}");
        assert_eq!(s.underruns, 0, "{s:?}");
    }

    /// 缓冲长度不是声道数的整数倍时，余下那几个位置也要被写成静音。
    ///
    /// 旧代码只写 `buf[..interleaved.len()]`，剩下的保持原样——而 cpal 交上来的
    /// 缓冲是复用的，"原样"是上一轮的音频。
    #[test]
    fn a_ragged_output_buffer_leaves_no_stale_samples() {
        let (ring, clock) = ring_and_clock(48_000);
        push_playback(&ring, &tone(9_600), &clock);
        // 两声道、5 个位置：只有两帧属于它，第 5 个位置不属于任何一帧。
        let mut buf = vec![7i16; 5];
        fill_output(&mut buf, 2, &ring, &clock);
        assert_eq!(&buf[..4], &[1000, 1000, 1000, 1000], "帧没被填满");
        assert_eq!(buf[4], 0, "残留了上一轮的样本");
    }

    /// 溢出裁掉多少要记下来。它和欠载是同一枚硬币的两面。
    #[test]
    fn the_overflow_trim_counts_what_it_drops() {
        let (ring, clock) = ring_and_clock(48_000);
        // 250 毫秒，比 200 毫秒的硬上限多 50 毫秒。
        push_playback(&ring, &tone(12_000), &clock);
        let s = clock.stats();
        assert_eq!(s.trimmed_ms, 50, "{s:?}");
        assert_eq!(s.depth_ms, RING_MS as u32, "{s:?}");
    }

    /// 重复或跳过的那一个样本挑**能量最低**的地方下手。
    ///
    /// 贴着零点改，波形上多出来或少掉的那一点接近 0；在波峰上改则是一个台阶，
    /// 而台阶是听得见的。这一步是 O(n) 的一遍扫描，回调里付得起。
    #[test]
    fn the_steering_edit_lands_on_the_quietest_sample() {
        assert_eq!(quietest(&[900, -20, 800]), 1);
        assert_eq!(quietest(&[0, 900, 800]), 0);
        assert_eq!(quietest(&[]), 0);
        // `i16::MIN` 的绝对值溢出 i16，别让它 panic。
        assert_eq!(quietest(&[i16::MIN, 5]), 1);
    }

    /// 把一段代码期间的日志收下来。
    ///
    /// `with_default` 是**按线程**装的，所以并行跑的测试互不干扰；装全局的那个
    /// 一个进程只许装一次，第二个测试就会 panic。
    fn capture_log(f: impl FnOnce()) -> String {
        use std::io::Write;

        #[derive(Clone, Default)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("sink").extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Sink {
            type Writer = Sink;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let sink = Sink::default();
        let sub = tracing_subscriber::fmt()
            .with_writer(sink.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(sub, f);
        let bytes = sink.0.lock().expect("sink").clone();
        String::from_utf8(bytes).expect("utf-8 log")
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

/// 播放环要稳住的水位。
///
/// 60 毫秒正好是三个 Opus 帧：够吃掉一次调度抖动，又短到没人会把它当成延迟。
/// 它和 [`RING_MS`] 是两件事——这个是**要稳在**的水位，那个是**绝不能超过**的
/// 延迟。只有上限没有目标，就是这个缺陷本身：水位在 0 和 200 毫秒之间自由漂，
/// 而两端都难听。
const TARGET_MS: usize = 60;

/// 一拍的长度，等于一个 Opus 帧。
///
/// 生产者每 20 毫秒一次性推这么多，所以它也是转向死区的宽度——见
/// [`PlaybackClock::take_for`]。
const FRAME_MS: usize = 20;

/// 播放环的对账数字。上层每隔几秒把它折进 `Event::Health`。
///
/// 时长一律用毫秒而不是样本数：样本数要配着设备采样率才有意义，而界面不知道
/// 设备采样率。转向那两个数例外——它们每次只动一个样本，换成毫秒会全部变成 0。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct PlaybackStats {
    /// 此刻的水位。
    pub depth_ms: u32,
    /// 欠载时补进去的静音总时长。
    pub silence_ms: u64,
    /// 欠载发生过几**段**。不是几个回调：回调每秒跑几十次，按回调数只能说明
    /// "回调在跑"。
    pub underruns: u64,
    /// 溢出时从环头裁掉的总时长。
    pub trimmed_ms: u64,
    /// 对齐时钟时补进去的样本数。
    pub steer_added: u64,
    /// 对齐时钟时拿掉的样本数。
    pub steer_removed: u64,
}

/// 播放环两端的时钟对账。
///
/// # 为什么需要它
///
/// 环的两侧是两个互不相识的时钟。生产者是 tokio 的 20 毫秒节拍（`pump.rs`，
/// 而且它的 `MissedTickBehavior::Delay` 意味着误了点永远补不回来，只会慢不会
/// 快）；消费者是声卡自己的晶振。没有任何东西把这两个时钟拉齐，而典型的
/// ±100 ppm 晶振偏差每约 200 秒就攒出一整帧的差。
///
/// 攒到哪一侧都难听，而往空里攒尤其难听：环空了就是每个回调都往里补静音
/// （听感是"电音"），补出来的静音又永远攒不出连续的一段（听感是"卡顿"）——
/// 测试员报的正是这两样**同时**出现。
///
/// # 它做两件事
///
/// **攒够了再出声。** 环是空着建起来的而流是立刻开始播的，所以第一瞬间回调就
/// 在空环上取数。被吃空之后也一样：退回去重新攒到目标水位，而不是空着硬撑。
///
/// **每个回调多吃或少吃一个样本**，把水位拉回目标。一个样本对 960 个样本的
/// 缓冲是 0.1%（约 1.7 音分），听不出来；而在 200 毫秒处一刀切掉最旧的一整段
/// 是听得出来的。代价是权限有限：转向的相对速率恰好是 `1/回调缓冲长度`——
/// 960 帧的缓冲上是 0.1%，对 ±100 ppm 留了 10 倍余量；偏差超出这个数就补不
/// 回来，而那时这里的计数器就是把它认出来的唯一东西。
///
/// # 回调里只碰原子量
///
/// 除了那把已有的环锁，实时音频线程上一行 I/O 都没有，也不分配无界的东西。
/// 要说的话由生产者那一侧照着计数器说，见 [`PlaybackClock::report`]。
#[derive(Debug, Default)]
pub(crate) struct PlaybackClock {
    /// 设备采样率。流会被重建，而重建出来的可能是另一个数。
    rate: AtomicU32,
    /// 攒够目标水位之前不出声。
    primed: AtomicBool,
    /// 出过声没有。第一次攒水位补的静音不算欠载——那是开场，不是故障。
    ever_primed: AtomicBool,
    /// 此刻正在一段欠载里。
    dry: AtomicBool,
    silence: AtomicU64,
    underruns: AtomicU64,
    trimmed: AtomicU64,
    added: AtomicU64,
    removed: AtomicU64,
    depth: AtomicU64,
    /// 这一段欠载的第一行报过了吗。
    reported: AtomicBool,
    /// 报第一行时的段数与静音数。汇总行减掉它们才是"这之后又发生了多少"。
    mark_underruns: AtomicU64,
    mark_silence: AtomicU64,
}

impl PlaybackClock {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 流建起来了（第一次，或者换设备之后重建）。
    ///
    /// **计数器不清**：它们是这一场会话的总账，清掉就看不出"换了三次设备之后
    /// 才开始欠载"。清的是水位状态——重建之后要重新攒，而这一次重新攒不算欠载。
    pub(crate) fn on_rebuild(&self, rate: u32) {
        self.rate.store(rate.max(1), Ordering::Relaxed);
        self.primed.store(false, Ordering::Relaxed);
        self.ever_primed.store(false, Ordering::Relaxed);
        self.dry.store(false, Ordering::Relaxed);
        self.depth.store(0, Ordering::Relaxed);
    }

    fn rate(&self) -> usize {
        self.rate.load(Ordering::Relaxed).max(1) as usize
    }

    /// 目标水位，样本数。
    fn target(&self) -> usize {
        self.rate() * TARGET_MS / 1000
    }

    /// 硬上限，样本数。
    fn cap(&self) -> usize {
        self.rate() * RING_MS / 1000
    }

    fn primed(&self) -> bool {
        self.primed.load(Ordering::Relaxed)
    }

    /// 这一轮该从环里吃几个样本：深了多吃一个，浅了少吃一个。
    ///
    /// 死区至少要有**一个生产帧**那么宽。生产者每 20 毫秒一次性推一整帧，
    /// 回调落在推之前还是推之后，测到的水位本来就差这么多；死区窄于此，转向就
    /// 成了跟相位较劲——每个回调都在改波形，却什么也没纠正。
    fn take_for(&self, depth: usize, frames: usize) -> usize {
        let target = self.target();
        let slack = (self.rate() * FRAME_MS / 1000).max(frames);
        if depth > target + slack && depth > frames {
            frames + 1
        } else if depth + slack < target && depth >= frames && frames >= 2 {
            frames - 1
        } else {
            frames
        }
    }

    fn set_depth(&self, depth: usize) {
        self.depth.store(depth as u64, Ordering::Relaxed);
    }

    fn note_trimmed(&self, n: u64) {
        if n > 0 {
            self.trimmed.fetch_add(n, Ordering::Relaxed);
        }
    }

    fn note_added(&self, n: u64) {
        self.added.fetch_add(n, Ordering::Relaxed);
    }

    fn note_removed(&self, n: u64) {
        self.removed.fetch_add(n, Ordering::Relaxed);
    }

    /// 补了 `n` 个静音样本。**开场那一段攒水位不算**：那不是故障。
    fn note_silence(&self, n: usize) {
        if self.ever_primed.load(Ordering::Relaxed) {
            self.silence.fetch_add(n as u64, Ordering::Relaxed);
        }
    }

    /// 环被吃空了：退回"攒水位"，并把这一段记成一段欠载。
    fn note_drained(&self) {
        self.primed.store(false, Ordering::Relaxed);
        if self.ever_primed.load(Ordering::Relaxed) && !self.dry.swap(true, Ordering::Relaxed) {
            self.underruns.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 攒够了，这一拍开始出声。
    fn note_primed(&self) {
        self.primed.store(true, Ordering::Relaxed);
        self.ever_primed.store(true, Ordering::Relaxed);
        self.dry.store(false, Ordering::Relaxed);
    }

    pub(crate) fn stats(&self) -> PlaybackStats {
        let rate = self.rate() as u64;
        let ms = |samples: u64| samples.saturating_mul(1000) / rate;
        PlaybackStats {
            depth_ms: ms(self.depth.load(Ordering::Relaxed)).min(u32::MAX as u64) as u32,
            silence_ms: ms(self.silence.load(Ordering::Relaxed)),
            underruns: self.underruns.load(Ordering::Relaxed),
            trimmed_ms: ms(self.trimmed.load(Ordering::Relaxed)),
            steer_added: self.added.load(Ordering::Relaxed),
            steer_removed: self.removed.load(Ordering::Relaxed),
        }
    }

    /// 把欠载说出来。**在生产者那一侧叫**，不在回调里——回调是实时线程。
    ///
    /// 照 `stack.rs` 那条路走：第一段报一行，之后闷着数，等这个状况过去了再报
    /// 一行汇总。每个回调报一行的后果那边有现成的账：一份 1 MB 的日志里 7545 行
    /// 有 7536 行是同一句，把文件里唯一那条 ERROR 挤了出去。
    pub(crate) fn report(&self) {
        if self.dry.load(Ordering::Relaxed) {
            if !self.reported.swap(true, Ordering::Relaxed) {
                let s = self.stats();
                self.mark_underruns.store(s.underruns, Ordering::Relaxed);
                self.mark_silence.store(s.silence_ms, Ordering::Relaxed);
                tracing::warn!(
                    target_ms = TARGET_MS,
                    trimmed_ms = s.trimmed_ms,
                    steer_added = s.steer_added,
                    steer_removed = s.steer_removed,
                    "the playback ring ran dry; holding silence until it fills again"
                );
            }
        } else if self.reported.swap(false, Ordering::Relaxed) {
            let s = self.stats();
            tracing::info!(
                underruns = s.underruns,
                // 第一行报出去之后又欠载了几段、又补了多少静音。中间那些回调
                // 一行都没打。
                episodes = s
                    .underruns
                    .saturating_sub(self.mark_underruns.load(Ordering::Relaxed)),
                silence_ms = s
                    .silence_ms
                    .saturating_sub(self.mark_silence.load(Ordering::Relaxed)),
                depth_ms = s.depth_ms,
                "the playback ring refilled"
            );
        }
    }
}

/// 缓冲里能量最低的那个位置。空缓冲返回 0。
///
/// 转向要复制或跳过一个样本，挑这里下手：贴着零点改，多出来或少掉的那一点接近
/// 0；在波峰上改则是一个台阶，而台阶是听得见的。O(n) 的一遍扫描，回调里付得
/// 起——它和 `spread_mono` 那一遍是同一个量级。
fn quietest(buf: &[i16]) -> usize {
    buf.iter()
        .enumerate()
        .min_by_key(|(_, s)| s.unsigned_abs())
        .map_or(0, |(i, _)| i)
}

/// 把一段设备采样率的样本推进播放环，超过 [`RING_MS`] 就丢最旧的。
///
/// 丢**最旧**的：留着只会让听到的话越来越落后于说出来的话。这一刀是硬接，
/// 所以它是上限而不是手段——真正把水位稳住的是 [`PlaybackClock::take_for`]。
fn push_playback(ring: &Arc<Mutex<VecDeque<i16>>>, samples: &[i16], clock: &PlaybackClock) {
    let cap = clock.cap();
    let mut trimmed = 0u64;
    {
        let mut r = match ring.lock() {
            Ok(r) => r,
            Err(p) => p.into_inner(),
        };
        r.extend(samples.iter().copied());
        while r.len() > cap {
            r.pop_front();
            trimmed += 1;
        }
        clock.set_depth(r.len());
    }
    clock.note_trimmed(trimmed);
}

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
    /// 播放环两端的时钟对账。回调那一侧只碰它的原子量。
    clock: Arc<PlaybackClock>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AudioIo {
    /// 打开输入与输出。`None` 表示用系统默认设备。
    ///
    /// 第一次建不起来也把 `AudioIo` 交出去：线程留下重试。否则连上之后才插
    /// 耳机，整场会话又聋又哑，只能重连。
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
        let clock = Arc::new(PlaybackClock::new());

        let (tx, rx) = std::sync::mpsc::channel::<Result<(), Error>>();
        let (pb, cap, st) = (playback.clone(), capture.clone(), stop.clone());
        let (run, fail) = (running.clone(), failed.clone());
        let (orate, irate) = (output_rate.clone(), input_rate.clone());
        let (want, gen) = (wanted.clone(), generation.clone());
        let clk = clock.clone();

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
                        clk.clone(),
                    );
                    let (out_stream, in_stream, (o, i)) = match built {
                        Ok(v) => v,
                        Err(e) => {
                            run.store(false, Ordering::Relaxed);
                            if let Some(tx) = first.take() {
                                let _ = tx.send(Ok(()));
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
                clock,
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
        let at_device_rate = resample_from_48k(pcm48, self.output_rate());
        push_playback(&self.playback, &at_device_rate, &self.clock);
        // **报告放在这一侧。** 回调是实时音频线程，那里一行日志都不能打；
        // 而这个方法每 20 毫秒被 `pump.rs` 叫一次，正好是看一眼计数器的地方。
        self.clock.report();
    }

    /// 播放环此刻的对账数字。上层每隔几秒把它折进 `Event::Health`。
    ///
    /// 没有它的时候，环跑干这件事在客户端里是**完全看不见的**：`fill_output`
    /// 往外补静音，不计数、不打日志、不出事件，而用户听到的是"电音加卡顿"。
    pub fn playback_stats(&self) -> PlaybackStats {
        self.clock.stats()
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

/// 喇叭试音：440 Hz，约 `ms` 毫秒，48 kHz 单声道。
pub fn test_tone(ms: u32) -> Vec<i16> {
    let n = SAMPLE_RATE as usize * ms as usize / 1000;
    (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            (0.25 * (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 32767.0) as i16
        })
        .collect()
}

fn scale_pcm(samples: &mut [i16], gain: f32) {
    if (gain - 1.0).abs() < f32::EPSILON {
        return;
    }
    for s in samples {
        *s = (*s as f32 * gain)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    }
}

fn wait_running(io: &AudioIo, ms: u64) -> bool {
    let steps = (ms / 50).max(1);
    for _ in 0..steps {
        if io.running() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    io.running()
}

/// 喇叭试音。走和通话同一条开流路径。
pub fn speaker_test(output: Option<&str>, gain: f32) -> Result<(), Error> {
    let io = AudioIo::start(None, output)?;
    if !wait_running(&io, 2000) {
        return Err(Error::NoDevice("output"));
    }
    let mut pcm = test_tone(600);
    scale_pcm(&mut pcm, gain.clamp(0.0, 2.0));
    io.play(&pcm);
    std::thread::sleep(std::time::Duration::from_millis(700));
    Ok(())
}

/// 麦克风试音：录约 1.5 秒再放出来。
pub fn mic_test(
    input: Option<&str>,
    output: Option<&str>,
    mic_gain: f32,
    speaker_gain: f32,
) -> Result<(), Error> {
    let io = AudioIo::start(input, output)?;
    if !wait_running(&io, 2000) {
        return Err(Error::NoDevice("input"));
    }
    let _ = io.take_capture();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let mut pcm = io.take_capture();
    scale_pcm(&mut pcm, mic_gain.clamp(0.0, 2.0));
    scale_pcm(&mut pcm, speaker_gain.clamp(0.0, 2.0));
    let ms = (pcm.len() as u64).saturating_mul(1000) / u64::from(SAMPLE_RATE);
    io.play(&pcm);
    std::thread::sleep(std::time::Duration::from_millis(ms.max(200)));
    Ok(())
}

/// 点名的设备还在不在。不在就用 `default`。
#[cfg(test)]
fn fallback_name(want: Option<&str>, names: &[&str], default: Option<&str>) -> Option<String> {
    match want {
        None => default.map(str::to_string),
        Some(n) if names.contains(&n) => Some(n.to_string()),
        Some(_) => default.map(str::to_string),
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

const MAX_CALLBACK_FRAMES: usize = 8192;

fn build_streams(
    input: Option<&str>,
    output: Option<&str>,
    playback: Arc<Mutex<VecDeque<i16>>>,
    capture: Arc<Mutex<VecDeque<i16>>>,
    failed: Arc<AtomicBool>,
    clock: Arc<PlaybackClock>,
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
    let output_chunk = MAX_CALLBACK_FRAMES * usize::from(out_ch.max(1));
    let input_chunk = MAX_CALLBACK_FRAMES * usize::from(in_ch.max(1));
    // Input callbacks never grow this ring; capacity is reserved before streams start.
    if let Ok(mut ring) = capture.lock() {
        let cap = in_rate as usize * RING_MS / 1000;
        let additional = cap.saturating_sub(ring.len());
        ring.reserve(additional);
    }

    // **重建出来的设备采样率可能和原来那个不一样**，而目标水位、硬上限和死区
    // 都是按采样率算的。顺带把"攒水位"重新置上：重建之后环里那点东西是按旧
    // 采样率重采样过的，不该被当成已经攒好的水位。
    clock.on_rebuild(out_rate);
    let out_clock = clock;

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
        cpal::SampleFormat::I16 => {
            let mut mono = Vec::with_capacity(MAX_CALLBACK_FRAMES + 1);
            out_dev.build_output_stream(
                &out_cfg.config(),
                move |buf: &mut [i16], _| {
                    for chunk in buf.chunks_mut(output_chunk) {
                        fill_output_reuse(chunk, out_ch, &playback, &out_clock, &mut mono);
                    }
                },
                make_err_fn(),
                None,
            )
        }
        cpal::SampleFormat::F32 => {
            let mut pcm = vec![0i16; output_chunk];
            let mut mono = Vec::with_capacity(MAX_CALLBACK_FRAMES + 1);
            out_dev.build_output_stream(
                &out_cfg.config(),
                move |buf: &mut [f32], _| {
                    for chunk in buf.chunks_mut(output_chunk) {
                        render_f32(chunk, out_ch, &playback, &out_clock, &mut pcm, &mut mono);
                    }
                },
                make_err_fn(),
                None,
            )
        }
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
        cpal::SampleFormat::F32 => {
            let mut pcm = vec![0i16; input_chunk];
            in_dev.build_input_stream(
                &in_cfg.config(),
                move |buf: &[f32], _| {
                    for chunk in buf.chunks(input_chunk) {
                        capture_f32(chunk, in_ch, &capture, in_rate, &mut pcm);
                    }
                },
                make_err_fn(),
                None,
            )
        }
        other => return Err(Error::SampleFormat(other)),
    }
    .map_err(|e| Error::Build(e.to_string()))?;

    out_stream.play().map_err(|e| Error::Play(e.to_string()))?;
    in_stream.play().map_err(|e| Error::Play(e.to_string()))?;
    tracing::info!(out_rate, in_rate, out_ch, in_ch, "audio devices opened");
    Ok((out_stream, in_stream, (out_rate, in_rate)))
}

/// 回调里做的全部事情：按水位决定这一轮吃几个样本，取出来铺到声道上。
///
/// **这条路在实时音频线程上。** 除了那把已有的环锁，这里没有 I/O、没有日志、
/// 没有无界的分配；要说的话由 [`PlaybackClock::report`] 在生产者那一侧说。
///
/// 三件事按顺序发生：没攒够水位就整片静音、按水位多吃或少吃一个样本、
/// 环被吃空就退回去重新攒。见 [`PlaybackClock`] 的说明。
#[cfg(test)]
fn fill_output(
    buf: &mut [i16],
    channels: u16,
    ring: &Arc<Mutex<VecDeque<i16>>>,
    clock: &PlaybackClock,
) {
    let mut mono = Vec::with_capacity(buf.len() + 1);
    fill_output_reuse(buf, channels, ring, clock, &mut mono);
}

fn render_f32(
    buf: &mut [f32],
    channels: u16,
    ring: &Arc<Mutex<VecDeque<i16>>>,
    clock: &PlaybackClock,
    pcm: &mut [i16],
    mono: &mut Vec<i16>,
) {
    let samples = &mut pcm[..buf.len()];
    fill_output_reuse(samples, channels, ring, clock, mono);
    for (target, sample) in buf.iter_mut().zip(samples) {
        *target = *sample as f32 / 32768.0;
    }
}

fn fill_output_reuse(
    buf: &mut [i16],
    channels: u16,
    ring: &Arc<Mutex<VecDeque<i16>>>,
    clock: &PlaybackClock,
    mono: &mut Vec<i16>,
) {
    // 先整片写静音。`buf.len()` 不一定是声道数的整数倍，而尾巴上那几个位置不
    // 属于任何一帧；cpal 交上来的缓冲是复用的，不写就是上一轮的残响。
    buf.fill(0);
    if channels == 0 {
        return;
    }
    let frames = buf.len() / channels.max(1) as usize;
    if frames == 0 {
        return;
    }

    mono.clear();
    let take;
    let emptied;
    {
        let mut r = match ring.lock() {
            Ok(r) => r,
            Err(p) => p.into_inner(),
        };
        let depth = r.len();
        if !clock.primed() {
            if depth < clock.target() {
                // 还没攒够：整片静音。**一个样本一个样本地凑不行**——空环上凑
                // 出来的是静音里夹着零星样本的东西，那就是"电音"。
                clock.set_depth(depth);
                clock.note_silence(frames);
                return;
            }
            clock.note_primed();
        }
        take = clock.take_for(depth, frames);
        for _ in 0..take.min(depth) {
            mono.push(r.pop_front().unwrap_or(0));
        }
        emptied = r.is_empty();
        clock.set_depth(r.len());
    }

    // 转向：多吃了一个就把能量最低的那个丢掉，少吃了一个就把它复制一份。
    // 两者都只动一个样本，而且动在贴着零点的地方——这比在 200 毫秒处一刀切掉
    // 20 毫秒安静得多。
    if mono.len() == take && !mono.is_empty() {
        match take.cmp(&frames) {
            std::cmp::Ordering::Greater => {
                mono.remove(quietest(mono));
                clock.note_removed(1);
            }
            std::cmp::Ordering::Less => {
                let i = quietest(mono);
                let v = mono[i];
                mono.insert(i, v);
                clock.note_added(1);
            }
            std::cmp::Ordering::Equal => {}
        }
    }

    if mono.len() < frames {
        clock.note_silence(frames - mono.len());
        mono.resize(frames, 0);
    }
    if emptied {
        // 吃空了就退回攒水位，别空着硬撑：硬撑的表现正是"电音加卡顿"同时出现。
        clock.note_drained();
    }

    for (&sample, frame) in mono
        .iter()
        .zip(buf.chunks_exact_mut(channels.max(1) as usize))
    {
        frame.fill(sample);
    }
}

fn take_input(buf: &[i16], channels: u16, ring: &Arc<Mutex<VecDeque<i16>>>, rate: u32) {
    let cap = rate as usize * RING_MS / 1000;
    if channels == 0 || cap == 0 {
        return;
    }
    let mut r = match ring.lock() {
        Ok(r) => r,
        Err(p) => p.into_inner(),
    };
    for frame in buf.chunks_exact(channels as usize) {
        if r.len() == cap {
            r.pop_front();
        }
        r.push_back(frame[0]);
    }
}

fn capture_f32(
    buf: &[f32],
    channels: u16,
    ring: &Arc<Mutex<VecDeque<i16>>>,
    rate: u32,
    pcm: &mut [i16],
) {
    for (target, sample) in pcm.iter_mut().zip(buf) {
        *target = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
    }
    take_input(&pcm[..buf.len()], channels, ring, rate);
}

fn pick(host: &cpal::Host, name: Option<&str>, input: bool) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let default = || {
        if input {
            host.default_input_device()
        } else {
            host.default_output_device()
        }
    };
    match name {
        None => default(),
        Some(want) => {
            let list = if input {
                host.input_devices()
            } else {
                host.output_devices()
            };
            if let Some(d) = list
                .ok()
                .and_then(|mut it| it.find(|d| d.name().map(|n| n == want).unwrap_or(false)))
            {
                return Some(d);
            }
            tracing::warn!(
                want,
                input,
                "audio device is gone; falling back to the system default"
            );
            default()
        }
    }
}

/// 优先要 48 kHz：那是 Opus 的采样率，拿到它就完全不必重采样。
///
/// 拿不到才退回设备默认值并在两端加重采样——旧的 Python 版在这里直接放弃，
/// 注释写着"48 kHz 是理想路径，回退采样率会产生变调音频"，也就是说设备不支持时
/// 用户听到的是变调的声音。
/// 我们真的建得出流的采样格式，**按偏好排序**。
///
/// 这张表和 `build_streams` 里的 `match` 是同一件事的两半，改一处要改两处——
/// 而它们分开的后果正是这个模块修过的那个 bug：挑选只看采样率，建流只认格式，
/// 于是挑出一条自己不接受的，表现为"连得上、界面全绿、又聋又哑"。
///
/// 顺序：F32 是 Windows WASAPI 共享模式的原生格式，走它不经过格式转换；
/// I16 是我们内部的样本类型。
const BUILDABLE: [cpal::SampleFormat; 2] = [cpal::SampleFormat::F32, cpal::SampleFormat::I16];

/// 在设备报的候选里挑一条我们建得出流的，返回下标。
///
/// `(格式, 最低采样率, 最高采样率)` 而不是 cpal 的类型，是为了能单测——
/// `SupportedStreamConfigRange` 在测试里造不出来，而这条挑选逻辑正是出过事的
/// 那一处。
///
/// `None` 表示这台设备一条都建不出来。**这和"挑一条建不出的回去"是两件事**：
/// 前者上层还能去问默认配置、还能把设备报了什么记进日志，后者只会在建流那一步
/// 再炸一次，而且每次重试都炸在同一个地方。
fn choose_config(ranges: &[(cpal::SampleFormat, u32, u32)], wanted: u32) -> Option<usize> {
    let rank = |f: cpal::SampleFormat| BUILDABLE.iter().position(|b| *b == f);
    // 覆盖 wanted 的那一批优先（拿到 48 kHz 就完全不必重采样），各自内部按格式
    // 偏好排；一条都不覆盖时退到采样率最高的那条，两端加重采样。
    let best = |covering: bool| {
        ranges
            .iter()
            .enumerate()
            .filter(|(_, (f, lo, hi))| {
                rank(*f).is_some() && ((*lo <= wanted && wanted <= *hi) == covering)
            })
            .min_by_key(|(_, (f, _, hi))| (rank(*f).unwrap_or(usize::MAX), std::cmp::Reverse(*hi)))
            .map(|(i, _)| i)
    };
    best(true).or_else(|| best(false))
}

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
    let keys: Vec<(cpal::SampleFormat, u32, u32)> = ranges
        .iter()
        .map(|r| {
            (
                r.sample_format(),
                r.min_sample_rate().0,
                r.max_sample_rate().0,
            )
        })
        .collect();
    let wanted = cpal::SampleRate(SAMPLE_RATE);
    if let Some(i) = choose_config(&keys, SAMPLE_RATE) {
        let r = ranges[i];
        return Ok(
            if r.min_sample_rate() <= wanted && wanted <= r.max_sample_rate() {
                r.with_sample_rate(wanted)
            } else {
                r.with_max_sample_rate()
            },
        );
    }

    // 一条都建不出来。**先把设备报了什么记下来再失败**：这条日志是下一份缺陷
    // 报告唯一能带走的东西，而没有它，"unsupported sample format U8" 读起来像
    // 设备只会 U8，实际上那台机器同时提供 F32。
    tracing::warn!(
        offered = ?keys.iter().map(|(f, ..)| *f).collect::<Vec<_>>(),
        input,
        "no audio format this client can build; falling back to the device default"
    );
    let cfg = if input {
        dev.default_input_config()
    } else {
        dev.default_output_config()
    }
    .map_err(|e| Error::Config(e.to_string()))?;
    if BUILDABLE.contains(&cfg.sample_format()) {
        Ok(cfg)
    } else {
        Err(Error::SampleFormat(cfg.sample_format()))
    }
}
