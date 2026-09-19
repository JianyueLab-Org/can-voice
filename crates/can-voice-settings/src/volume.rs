//! 麦克风 / 喇叭总音量。0–200，100 是原声。
//!
//! 0 是合法静音，所以不能用 `u32` 的 Default（那是 0）：第一次启动没有设置文件
//! 时 `Store::load` 走 `T::default()`，会把所有人静音。

/// 音量百分比。JSON 就是一个数字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct VolumePercent(u32);

impl Default for VolumePercent {
    fn default() -> Self {
        Self(100)
    }
}

impl VolumePercent {
    pub const MAX: u32 = 200;

    pub fn new(v: u32) -> Self {
        Self(v.min(Self::MAX))
    }

    pub fn get(self) -> u32 {
        self.0
    }

    /// 乘到 PCM 上的增益。100 → 1.0。
    pub fn gain(self) -> f32 {
        self.0 as f32 / 100.0
    }
}
