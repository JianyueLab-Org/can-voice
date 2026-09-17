//! UDP 收包出错时怎么归类。
//!
//! # Windows 把上一次发送的失败报在下一次收包上
//!
//! 往一个没人在听的端口发数据报（X-Plane 还没开、正在重启、还在读盘），对方回一个
//! ICMP port unreachable。Windows 把它挂在**发出那一包的 socket** 上，下一次
//! `recv`/`recv_from` 就返回 WSAECONNRESET（os error 10054，
//! [`ErrorKind::ConnectionReset`]）——哪怕这个 UDP socket 从没 `connect` 过。
//! 它不致命：一个 ICMP 报一次，报完 socket 照常能收。macOS/Linux 对没 connect 的
//! UDP socket 不这么做，所以它在开发机上永远不会露头。
//!
//! 它说的是"对面这会儿没人"，和"等了一轮什么都没收到"是同一件事，所以收包循环
//! 把它**当成超时**：超时了怎么办，它就怎么办。当成别的错误的话，X-Plane 一次
//! 重启就拆掉订阅、停掉监听；当成没发生直接再收的话，又跳过了循环在超时那一支
//! 里做的事（标记断开、判断该不该放弃）。
//!
//! 按 [`ErrorKind`] 判断，而不是用 Windows 专有的 `SIO_UDP_CONNRESET` ioctl
//! 把它关掉：前者到处编得过、在 Linux CI 上测得到，也不用每开一个 socket 都
//! 记得调一次。QUIC 那条链路不走这里——quinn 自己就忽略这个错误。

use std::io::{self, ErrorKind};

/// 这次收包错误是不是上面说的那种，该按超时处理。其余错误照各个循环原来的办法。
pub fn counts_as_timeout(err: &io::Error) -> bool {
    err.kind() == ErrorKind::ConnectionReset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_reset_counts_as_a_timeout() {
        assert!(counts_as_timeout(&io::Error::from(
            ErrorKind::ConnectionReset
        )));
    }

    /// 只认这一种。`ConnectionRefused` 是 Linux 上**connect 过的** UDP socket
    /// 报 ICMP 的方式，而这里的 socket 一个都没 connect，出现了就是别的事。
    #[test]
    fn every_other_error_keeps_its_own_handling() {
        for kind in [
            ErrorKind::ConnectionRefused,
            ErrorKind::ConnectionAborted,
            ErrorKind::NotConnected,
            ErrorKind::AddrNotAvailable,
            ErrorKind::PermissionDenied,
            ErrorKind::InvalidInput,
            ErrorKind::Other,
        ] {
            assert!(!counts_as_timeout(&io::Error::from(kind)), "{kind:?}");
        }
    }

    /// 整个修复押在"标准库把 10054 翻译成 `ConnectionReset`"上。
    /// CI 的测试只在 Linux 上跑，这条只有在 Windows 上手动跑才会执行。
    #[cfg(windows)]
    #[test]
    fn wsaeconnreset_is_what_the_standard_library_calls_a_connection_reset() {
        assert!(counts_as_timeout(&io::Error::from_raw_os_error(10054)));
    }
}
