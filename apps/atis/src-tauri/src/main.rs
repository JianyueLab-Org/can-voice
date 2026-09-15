// Windows 上不要在 release 里开一个控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    atis_for_can_lib::run()
}
