use crate::storage::{StatusStore, now_ms};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::Value;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::Manager;

// 保留既有插件标识，避免改名导致重复安装或丢失已有插件的检测结果。
const PLUGIN_ID: &str = "keyphore-windows@keyphore-local";
const PAYLOAD_FILES: [&str; 5] = [
    ".agents/plugins/marketplace.json",
    "plugin/.codex-plugin/plugin.json",
    "plugin/hooks/hooks.json",
    "plugin/hooks/receive.ps1",
    "plugin/bin/keyphore-hook.exe",
];
static INSTALL_LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationStatus {
    cli_available: bool,
    installed: bool,
    enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventStatus {
    last_event_ms: Option<u64>,
    received: bool,
}

/// 优先使用 PATH 中的原生 CLI，再查找 Windows 桌面版随附的版本目录。
fn find_codex() -> Option<PathBuf> {
    if !cfg!(windows) { return None; }
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            if !directory.is_absolute() { continue; }
            let candidate = directory.join("codex.exe");
            if candidate.is_file() { return Some(candidate); }
        }
    }
    let root = PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
        .join("OpenAI/Codex/bin");
    let mut candidates = Vec::new();
    if root.join("codex.exe").is_file() { candidates.push(root.join("codex.exe")); }
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("codex.exe");
            if candidate.is_file() { candidates.push(candidate); }
        }
    }
    candidates.sort_by_key(|path| fs::metadata(path).and_then(|item| item.modified()).ok());
    candidates.pop()
}

/// 并行排空两个管道，限制输出大小并设置超时，避免 CLI 卡住整个安装流程。
fn run_codex(cli: &Path, args: &[&OsStr]) -> Result<Value> {
    let mut command = Command::new(cli);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().context("无法启动 Codex，请更新 Codex 后重试")?;
    let stdout = child.stdout.take().context("无法读取 Codex 输出")?;
    let stderr = child.stderr.take().context("无法读取 Codex 错误信息")?;
    let output_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes).map(|_| bytes)
    });
    let error_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(256 * 1024 + 1).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                if let Err(error) = result { return Err(error).context("等待 Codex 失败"); }
                bail!("Codex 操作超过 60 秒，请检查 Codex 是否正常运行后重试");
            }
        }
    };
    let output = output_reader.join().map_err(|_| anyhow::anyhow!("读取 Codex 输出失败"))??;
    let errors = error_reader.join().map_err(|_| anyhow::anyhow!("读取 Codex 错误信息失败"))??;
    ensure!(output.len() <= 4 * 1024 * 1024 && errors.len() <= 256 * 1024,
        "Codex 返回数据过大，请在 Codex 中检查插件配置");
    if !status.success() {
        let detail: String = String::from_utf8_lossy(&errors).chars().take(1200).collect();
        bail!("Codex 操作失败（{status}）：{detail}");
    }
    serde_json::from_slice(&output).context("Codex 返回格式不兼容，请更新 Codex 后重试")
}

/// 只认 installed 集合中的精确插件标识，不能把可安装或同名插件视为已安装。
fn installed_flags(list: &Value) -> Result<(bool, bool)> {
    let entries = list.get("installed").and_then(Value::as_array)
        .context("Codex 插件列表格式不兼容，请更新 Codex 后重试")?;
    for entry in entries {
        if entry.get("pluginId").and_then(Value::as_str) != Some(PLUGIN_ID) { continue; }
        let installed = entry.get("installed").and_then(Value::as_bool)
            .context("Codex 插件列表缺少安装状态")?;
        let enabled = entry.get("enabled").and_then(Value::as_bool)
            .context("Codex 插件列表缺少启用状态")?;
        return Ok((installed, installed && enabled));
    }
    Ok((false, false))
}

/// 使用官方 CLI 检测，不解析或改写用户的 Codex 配置与信任记录。
fn inspect(cli: &Path) -> Result<IntegrationStatus> {
    let list = run_codex(cli, &[OsStr::new("plugin"), OsStr::new("list"), OsStr::new("--json")])?;
    let (installed, enabled) = installed_flags(&list)?;
    Ok(IntegrationStatus { cli_available: true, installed, enabled })
}

/// 资源缺失时提前报错，防止注册一个没有接收器的插件源。
fn validate_payload(root: &Path) -> Result<()> {
    for file in PAYLOAD_FILES {
        ensure!(root.join(file).is_file(), "应用缺少联动组件 {file}，请重新安装完整版本");
    }
    let manifest: Value = serde_json::from_slice(&fs::read(root.join(PAYLOAD_FILES[1]))?)?;
    ensure!(manifest["name"] == "keyphore-windows" && manifest["hooks"] == "./hooks/hooks.json",
        "应用内的联动插件清单不匹配");
    let marketplace: Value = serde_json::from_slice(&fs::read(root.join(PAYLOAD_FILES[0]))?)?;
    ensure!(marketplace["name"] == "keyphore-local"
        && marketplace["plugins"][0]["name"] == "keyphore-windows"
        && marketplace["plugins"][0]["source"]["source"] == "local"
        && marketplace["plugins"][0]["source"]["path"] == "./plugin",
        "应用内的联动插件源不匹配");
    let mut signature = [0; 2];
    File::open(root.join(PAYLOAD_FILES[4]))?.read_exact(&mut signature)?;
    ensure!(&signature == b"MZ", "联动接收器不是有效的 Windows 程序");
    Ok(())
}

/// 所有写入限定在应用自己的当前用户目录，不依赖安装位置或源码目录。
fn integration_directory(app: &tauri::AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_local_data_dir()?.join("codex-integration"))
}

/// 新安装之后只接受新的真实事件，避免旧 status.json 造成已就绪的误报。
fn event_received(last: Option<u64>, since: u64, now: u64) -> bool {
    last.is_some_and(|time| time >= since && time <= now && now - time < 24 * 60 * 60 * 1000)
}

/// 状态检查不访问键盘，联动关闭时也能验证接收链路。
#[tauri::command]
pub async fn get_codex_integration() -> Result<IntegrationStatus, String> {
    tauri::async_runtime::spawn_blocking(|| -> Result<IntegrationStatus> {
        match find_codex() {
            Some(cli) => inspect(&cli),
            None => Ok(IntegrationStatus { cli_available: false, installed: false, enabled: false }),
        }
    }).await.map_err(|error| format!("插件检测后台任务异常：{error}"))?
        .map_err(|error| format!("{error:#}"))
}

/// 只读轮询本地事件，不反复启动 CLI，也不把安装成功当成信任成功。
#[tauri::command]
pub async fn get_codex_event(app: tauri::AppHandle) -> Result<EventStatus, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<EventStatus> {
        let receipt = integration_directory(&app)?.join("verification-since.json");
        let since = match File::open(receipt) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(33).read_to_end(&mut bytes)?;
                ensure!(bytes.len() <= 32, "联动安装记录损坏，请重新安装联动组件");
                serde_json::from_slice::<u64>(&bytes).context("联动安装记录损坏")?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error).context("无法读取联动安装记录"),
        };
        let core = StatusStore::local()?.read()?;
        let last_event_ms = core.sessions.values().map(|session| session.updated_ms).max();
        Ok(EventStatus { last_event_ms, received: event_received(last_event_ms, since, now_ms()) })
    }).await.map_err(|error| format!("事件检测后台任务异常：{error}"))?
        .map_err(|error| format!("{error:#}"))
}

/// 用户点击后才安装；已安装直接复用，首次信任仍交由 Codex 的审核入口处理。
#[tauri::command]
pub async fn install_codex_integration(app: tauri::AppHandle) -> Result<IntegrationStatus, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<IntegrationStatus> {
        let _guard = INSTALL_LOCK.try_lock().map_err(|_| anyhow::anyhow!("正在安装联动组件，请稍候"))?;
        let cli = find_codex().context("未找到 Codex，请安装 Windows 桌面版或将 codex.exe 加入 PATH 后重试")?;
        let current = inspect(&cli)?;
        if current.installed { return Ok(current); }
        let source = app.path().resource_dir()?.join("codex-integration");
        // 开发模式直接使用仓库资源；发布版只认安装包随附的资源目录。
        #[cfg(debug_assertions)]
        let source = if source.is_dir() { source } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
        };
        validate_payload(&source)?;
        let destination = integration_directory(&app)?;
        for file in PAYLOAD_FILES {
            let target = destination.join(file);
            fs::create_dir_all(target.parent().context("联动组件路径无效")?)?;
            fs::copy(source.join(file), target).with_context(|| format!("无法准备联动组件 {file}"))?;
        }
        fs::write(destination.join("verification-since.json"), now_ms().to_string())
            .context("无法保存联动验证起点，尚未安装插件")?;
        run_codex(&cli, &[
            OsStr::new("plugin"), OsStr::new("marketplace"), OsStr::new("add"),
            destination.as_os_str(), OsStr::new("--json"),
        ]).context("登记 AirPulse 本地插件源失败，可修复后重试")?;
        run_codex(&cli, &[
            OsStr::new("plugin"), OsStr::new("add"), OsStr::new(PLUGIN_ID), OsStr::new("--json"),
        ]).context("安装 AirPulse 插件失败，已保留本地组件供重试")?;
        let installed = inspect(&cli)?;
        ensure!(installed.installed, "Codex 尚未确认插件已安装，请重试检测");
        Ok(installed)
    }).await.map_err(|error| format!("插件安装后台任务异常：{error}"))?
        .map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 未安装、同名不同源和仅出现在 available 中的条目都不能通过检测。
    #[test]
    fn detection_requires_exact_installed_plugin() {
        let available = json!({"installed": [], "available": [{"pluginId": PLUGIN_ID}]});
        assert_eq!(installed_flags(&available).unwrap(), (false, false));
        let other = json!({"installed": [{"pluginId": "keyphore-windows@other", "installed": true, "enabled": true}]});
        assert_eq!(installed_flags(&other).unwrap(), (false, false));
        let disabled = json!({"installed": [{"pluginId": PLUGIN_ID, "installed": true, "enabled": false}]});
        assert_eq!(installed_flags(&disabled).unwrap(), (true, false));
        let enabled = json!({"installed": [{"pluginId": PLUGIN_ID, "installed": true, "enabled": true}]});
        assert_eq!(installed_flags(&enabled).unwrap(), (true, true));
    }

    /// CLI 格式变化应显式报错，不能误判成需要重新安装。
    #[test]
    fn detection_rejects_unknown_schema() {
        assert!(installed_flags(&json!({"plugins": []})).is_err());
        assert!(installed_flags(&json!({"installed": [{"pluginId": PLUGIN_ID}]})).is_err());
    }

    /// 旧事件、过期事件和未来时间都不能让新安装误报为已就绪。
    #[test]
    fn verification_requires_fresh_event_after_install() {
        let now = 100_000_000;
        assert!(!event_received(None, 100, now));
        assert!(!event_received(Some(now - 20), now - 10, now));
        assert!(!event_received(Some(100), 0, now));
        assert!(!event_received(Some(now + 1), 0, now));
        assert!(event_received(Some(now), now - 10, now));
    }
}
