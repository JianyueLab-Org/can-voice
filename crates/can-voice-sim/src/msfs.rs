//! MSFS 数据链路（SimConnect）。
//!
//! **SimConnect 是 Windows-only 的 C API**，所以这个模块在别的平台上是空的。
//! 非 Windows 上编译得过、但 [`available`] 返回 false —— 这样上层只有一份代码，
//! 而不是用 `#[cfg]` 把整条链路切成两份。
//!
//! 换算、应答机模式、气压修正量都在 [`crate`] 根上，和 X-Plane 那条共用。

/// 这个平台上有没有 SimConnect。
pub const fn available() -> bool {
    cfg!(windows)
}

#[cfg(test)]
mod tests {
    #[test]
    fn simconnect_is_windows_only() {
        assert_eq!(super::available(), cfg!(windows));
    }
}
