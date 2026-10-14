// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// 桌面入口统一交给共享库，避免窗口与后台状态服务重复初始化。
fn main() {
    airpulse_lib::run()
}
