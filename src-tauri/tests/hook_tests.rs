use airpulse_lib::status::{HookEvent, TaskStatus};
use airpulse_lib::storage::StatusStore;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);

impl Directory {
    /// 仅使用本测试创建的独立目录，不读写真实用户状态。
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("keyphore-hook-{}-{}-{}",
            std::process::id(), airpulse_lib::storage::now_ms(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&directory).unwrap();
        Self(directory)
    }
}

impl Drop for Directory {
    /// 路径来自本测试的 create_dir，清理范围不包含用户目录。
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 检查真实二进制的 stdin/stdout 契约，并验证正文不会进入状态文件。
#[test]
fn receiver_outputs_json_and_does_not_persist_conversation() {
    let directory = Directory::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_keyphore-hook"))
        .env("LOCALAPPDATA", &directory.0).stdin(Stdio::piped())
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(br#"{"hook_event_name":"UserPromptSubmit","session_id":"a","turn_id":"1","prompt":"PRIVATE_PROMPT","tool_input":{"secret":"PRIVATE_INPUT"},"last_assistant_message":"PRIVATE_REPLY"}"#).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.stdout, b"{}\n");
    let path = directory.0.join("Keyphore");
    assert_eq!(StatusStore::new(path.clone()).read().unwrap().aggregate_at(airpulse_lib::storage::now_ms()), TaskStatus::Running);
    let persisted = fs::read_to_string(path.join("status.json")).unwrap();
    assert!(!persisted.contains("PRIVATE_"));
}

/// 执行插件实际命令，覆盖带空格路径、UTF-8 输入及 PowerShell 原始字节转发。
#[cfg(windows)]
#[test]
fn windows_plugin_command_forwards_utf8_input() {
    let directory = Directory::new();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../plugin");
    let plugin = directory.0.join("plugin with spaces");
    fs::create_dir_all(plugin.join("hooks")).unwrap();
    fs::create_dir(plugin.join("bin")).unwrap();
    fs::copy(source.join("hooks/receive.ps1"), plugin.join("hooks/receive.ps1")).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_keyphore-hook"), plugin.join("bin/keyphore-hook.exe")).unwrap();
    let definitions: serde_json::Value = serde_json::from_slice(&fs::read(source.join("hooks/hooks.json")).unwrap()).unwrap();
    let command = definitions["hooks"]["UserPromptSubmit"][0]["hooks"][0]["commandWindows"].as_str().unwrap();
    let parts: Vec<_> = command.split_whitespace().collect();
    let mut child = Command::new(parts[0]).args(&parts[1..])
        .env("PLUGIN_ROOT", &plugin).env("LOCALAPPDATA", &directory.0)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(
        "{\"hook_event_name\":\"UserPromptSubmit\",\"session_id\":\"中文会话\",\"turn_id\":\"1\"}".as_bytes(),
    ).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response, serde_json::json!({}));
    let core = StatusStore::new(directory.0.join("Keyphore")).read().unwrap();
    assert!(core.sessions.contains_key("中文会话"));
}

/// 并发更新保留两个会话，锁超时快速退出后上层可有限重试。
#[test]
fn concurrent_hooks_preserve_all_sessions() {
    let directory = Directory::new();
    let workers: Vec<_> = ["a", "b"].into_iter().map(|id| {
        let path = directory.0.clone();
        std::thread::spawn(move || {
            let event = HookEvent::parse(format!(r#"{{"hook_event_name":"UserPromptSubmit","session_id":"{id}","turn_id":"1"}}"#).as_bytes()).unwrap();
            let store = StatusStore::new(path);
            for attempt in 0..5 {
                match store.update(&event, 100) {
                    Ok(()) => return,
                    Err(error) if error.to_string().contains("锁等待") && attempt < 4 => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error:#}"),
                }
            }
        })
    }).collect();
    for worker in workers { worker.join().unwrap(); }
    let core = StatusStore::new(directory.0.clone()).read().unwrap();
    assert_eq!(core.sessions.len(), 2);
}

/// 锁超时不改变文件；损坏 JSON 先备份，未来格式拒绝覆盖。
#[test]
fn locked_corrupt_and_future_files_are_not_silently_overwritten() {
    let directory = Directory::new();
    let store = StatusStore::new(directory.0.clone());
    let event = HookEvent::parse(br#"{"hook_event_name":"UserPromptSubmit","session_id":"a","turn_id":"1"}"#).unwrap();
    let lock = OpenOptions::new().read(true).write(true).create_new(true)
        .open(directory.0.join("status.lock")).unwrap();
    lock.lock().unwrap();
    assert!(store.update(&event, 100).unwrap_err().to_string().contains("100 ms"));
    assert!(!directory.0.join("status.json").exists());
    drop(lock);
    let path = directory.0.join("status.json");
    fs::write(&path, b"broken json").unwrap();
    store.update(&event, 100).unwrap();
    assert_eq!(store.read().unwrap().sessions.len(), 1);
    let backup = fs::read_dir(&directory.0).unwrap().map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(".corrupt.json")).unwrap();
    assert_eq!(fs::read(backup).unwrap(), b"broken json");
    let future = br#"{"version":99,"sessions":{}}"#;
    fs::write(&path, future).unwrap();
    assert!(store.update(&event, 200).is_err());
    assert_eq!(fs::read(path).unwrap(), future);
}
