#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use anyhow::{Result, ensure};
use airpulse_lib::status::MAX_HOOK_BYTES;
use airpulse_lib::storage::{StatusStore, handle_hook};
use std::io::{Read, Write};

/// 仅从标准输入接收一条事件，不启动窗口、不读取聊天记录、不连接键盘。
fn receive() -> Result<()> {
    let mut input = Vec::new();
    std::io::stdin().take((MAX_HOOK_BYTES + 1) as u64).read_to_end(&mut input)?;
    ensure!(input.len() <= MAX_HOOK_BYTES, "Hook 输入超过 1 MiB");
    handle_hook(&input, &StatusStore::local()?)?;
    // Stop 和 SubagentStop 要求 JSON；空对象不审批、不阻止或继续任务。
    std::io::stdout().write_all(b"{}\n")?;
    Ok(())
}

/// 错误只输出状态处理原因，避免把包含用户正文的输入回显到日志。
fn main() {
    if let Err(error) = receive() {
        eprintln!("AirPulse Hook: {error}");
        std::process::exit(1);
    }
}
