use crate::status::{HookEvent, MAX_HOOK_BYTES, StatusCore};
use anyhow::{Context, Result, bail, ensure};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

pub struct StatusStore {
    directory: PathBuf,
}

impl StatusStore {
    /// 沿用原状态目录，让已安装的旧 Hook 与改名后的应用继续共享状态。
    pub fn local() -> Result<Self> {
        let base = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| anyhow::anyhow!("无法获取 LOCALAPPDATA"))?;
        Ok(Self::new(PathBuf::from(base).join("Keyphore")))
    }

    /// 显式目录供无硬件检查使用，不修改真实用户数据。
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// 读取原子发布的完整文件；缺失视为空状态，损坏必须上报而不能当成熄灭。
    pub fn read(&self) -> Result<StatusCore> {
        let path = self.directory.join("status.json");
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(StatusCore::default()),
            Err(error) => return Err(error).context("读取任务状态失败"),
        };
        let mut bytes = Vec::new();
        file.take((MAX_HOOK_BYTES + 1) as u64).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= MAX_HOOK_BYTES, "状态文件超过 1 MiB");
        let state: StatusCore = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("状态文件 JSON 损坏"))?;
        state.validate()?;
        Ok(state)
    }

    /// 用内核文件锁串行更新，进程异常退出时由系统释放锁。
    pub fn update(&self, event: &HookEvent, now_ms: u64) -> Result<()> {
        fs::create_dir_all(&self.directory).context("创建任务状态目录失败")?;
        let _lock = self.lock()?;
        let path = self.directory.join("status.json");
        let mut state = match self.read() {
            Ok(state) => state,
            Err(error) => {
                // 只恢复损坏 JSON；权限错误和不支持的版本不能被静默覆盖。
                if error.to_string() != "状态文件 JSON 损坏" { return Err(error); }
                fs::copy(&path, self.unique_path("corrupt.json"))
                    .context("备份损坏任务状态失败，保留原文件")?;
                StatusCore::default()
            }
        };
        state.apply(event, now_ms)?;
        let bytes = serde_json::to_vec(&state)?;
        ensure!(bytes.len() <= MAX_HOOK_BYTES, "状态文件超过 1 MiB");
        let temporary = self.unique_path("tmp");
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            replace_file(&temporary, &path).context("原子发布任务状态失败")
        })();
        if result.is_err() { let _ = fs::remove_file(&temporary); }
        result
    }

    /// 100 ms 内无法取得锁就快速退出，不长时间阻塞 Codex。
    fn lock(&self) -> Result<File> {
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .open(self.directory.join("status.lock"))?;
        let deadline = Instant::now() + Duration::from_millis(100);
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(TryLockError::WouldBlock) => bail!("任务状态锁等待超过 100 ms"),
                Err(error) => bail!("任务状态加锁失败: {error}"),
            }
        }
    }

    /// 临时文件不使用事件中的标识，防止路径注入并避免并发覆盖。
    fn unique_path(&self, suffix: &str) -> PathBuf {
        self.directory.join(format!("status-{}-{}-{}.{}", std::process::id(), now_ms(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed), suffix))
    }
}

/// 使用接收时刻计算到期，不相信事件提供的任意时间戳。
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// Windows rename 不能覆盖已有文件，使用系统原子替换避免先删除造成空窗。
#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination.as_os_str().encode_wide().chain(Some(0)).collect();
    // 两个字符串都在调用期间存活，文件位于同一目录和磁盘。
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 1 | 8) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// 非 Windows 的检查环境仍使用同目录原子重命名。
#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

/// Hook 与测试复用同一边界校验，接收器本身不访问 HID。
pub fn handle_hook(input: &[u8], store: &StatusStore) -> Result<()> {
    store.update(&HookEvent::parse(input)?, now_ms())
}
