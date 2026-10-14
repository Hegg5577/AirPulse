use crate::app_lighting::{LightingService, LightingSession, LightingView, LightingZone, Operation};
use crate::hid::{Air75Controller, LightingIo, connect_air75};
use crate::lighting::{Air75MainState, Air75SideState};
use crate::status::TaskStatus;
use crate::storage::{StatusStore, now_ms};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DisplayZone {
    Side,
    Main,
    Both,
}

impl DisplayZone {
    /// 返回固定分区切片，统一接管、回读和恢复的执行顺序。
    fn zones(self) -> &'static [LightingZone] {
        match self {
            Self::Side => &[LightingZone::Side],
            Self::Main => &[LightingZone::Main],
            Self::Both => &[LightingZone::Side, LightingZone::Main],
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusColors {
    pub tool_failed: [u8; 3],
    pub permission_waiting: [u8; 3],
    pub running: [u8; 3],
    pub completed: [u8; 3],
}

impl Default for StatusColors {
    /// 默认色使用系统语义色，用户可在启动前分别覆盖。
    fn default() -> Self {
        Self {
            tool_failed: [255, 59, 48],
            permission_waiting: [255, 59, 48],
            running: [255, 214, 10],
            completed: [52, 199, 89],
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    pub enabled: bool,
    pub status: TaskStatus,
    pub brightness: u8,
    pub zone: DisplayZone,
    pub colors: StatusColors,
    pub error: Option<String>,
    pub restore_pending: bool,
    pub last_event_ms: Option<u64>,
    #[serde(skip)]
    applied: Option<TaskStatus>,
    #[serde(skip)]
    shutdown: bool,
}

impl Default for RuntimeSnapshot {
    /// 每次启动都等待用户主动开启，避免应用启动时直接写键盘。
    fn default() -> Self {
        Self {
            enabled: false,
            status: TaskStatus::Idle,
            brightness: 30,
            zone: DisplayZone::Side,
            colors: StatusColors::default(),
            error: None,
            restore_pending: false,
            last_event_ms: None,
            applied: None,
            shutdown: false,
        }
    }
}

/// 活跃状态映射到用户颜色；空闲没有目标色，只恢复原灯效。
fn color_for(status: TaskStatus, colors: StatusColors) -> Option<[u8; 3]> {
    match status {
        TaskStatus::ToolFailed => Some(colors.tool_failed),
        TaskStatus::PermissionWaiting => Some(colors.permission_waiting),
        TaskStatus::Running => Some(colors.running),
        TaskStatus::Completed => Some(colors.completed),
        TaskStatus::Idle => None,
    }
}

impl LightingSession {
    /// 判断当前读数是否已经匹配所选区域，避免状态不变时重复写入闪灯。
    fn automatic_matches(&self, view: &LightingView, color: [u8; 3]) -> bool {
        self.automatic.zone.zones().iter().all(|zone| match zone {
            LightingZone::Main => {
                let desired = Air75MainState::steady(
                    self.automatic.brightness, color[0], color[1], color[2],
                ).expect("启动时已校验亮度");
                view.state[..9] == desired.as_bytes()[..]
            }
            LightingZone::Side => {
                let mut desired = *Air75SideState::steady(color[0], color[1], color[2]).as_bytes();
                desired[1] = self.automatic.brightness;
                view.state[9..] == desired[..]
            }
        })
    }

    /// 恢复自动模式所选区域；每个区域独立尝试，尽量不留下部分接管状态。
    fn restore_automatic<T: LightingIo>(
        &mut self,
        controller: &mut Air75Controller<T>,
        path: &str,
        handle: u8,
    ) -> Result<()> {
        let mut errors = Vec::new();
        for &zone in self.automatic.zone.zones().iter().rev() {
            if self.has_snapshot(zone)
                && let Err(error) = self.operate(controller, path, handle, Operation::Restore(zone))
            {
                errors.push(format!("{zone:?}: {error:#}"));
            }
        }
        self.automatic.restore_pending = self.automatic.zone.zones().iter()
            .any(|zone| self.has_snapshot(*zone));
        if !errors.is_empty() {
            bail!("恢复原灯效失败: {}", errors.join("；"));
        }
        Ok(())
    }

    /// 状态变化时写入所选区域；空闲时恢复原灯效而不是写黑色。
    fn sync_automatic<T: LightingIo>(
        &mut self,
        controller: &mut Air75Controller<T>,
        path: &str,
        handle: u8,
    ) -> Result<()> {
        let Some(color) = color_for(self.automatic.status, self.automatic.colors) else {
            self.restore_automatic(controller, path, handle)?;
            self.automatic.applied = Some(TaskStatus::Idle);
            return Ok(());
        };
        let view = self.operate(controller, path, handle, Operation::Read)?;
        for zone in self.automatic.zone.zones() {
            let restore_available = match zone {
                LightingZone::Main => view.main_restore_available,
                LightingZone::Side => view.side_restore_available,
            };
            ensure!(!self.has_snapshot(*zone) || restore_available,
                "自动恢复点属于其他设备或模式，请接回原设备并切回原模式");
        }
        if self.automatic.applied == Some(self.automatic.status)
            && self.automatic_matches(&view, color)
        {
            return Ok(());
        }
        for &zone in self.automatic.zone.zones() {
            if let Err(error) = self.operate(controller, path, handle, Operation::Set {
                zone,
                color,
                brightness: self.automatic.brightness,
            }) {
                let rollback = self.restore_automatic(controller, path, handle);
                if let Err(restore_error) = rollback {
                    bail!("自动写灯失败: {error:#}；{restore_error:#}");
                }
                bail!("自动写灯失败，所选区域已恢复原灯效: {error:#}");
            }
        }
        self.automatic.restore_pending = true;
        self.automatic.applied = Some(self.automatic.status);
        Ok(())
    }

    /// 写入或状态文件异常时暂停联动，并保留可重试的恢复入口。
    fn pause_automatic(&mut self, error: anyhow::Error) {
        self.automatic.enabled = false;
        self.automatic.error = Some(format!("{error:#}"));
        self.automatic.restore_pending = self.automatic.zone.zones().iter()
            .any(|zone| self.has_snapshot(*zone));
    }
}

/// 聚合使用接收器的本地状态，完成状态持续到任务被移出跟踪范围。
fn refresh_status(session: &mut LightingSession, store: &StatusStore) -> Result<()> {
    let core = store.read()?;
    session.automatic.status = core.aggregate_at(now_ms());
    session.automatic.last_event_ms = core.sessions.values().map(|item| item.updated_ms).max();
    Ok(())
}

/// 应用内唯一工作线程；每 250 ms 聚合，活跃灯效每 2 秒健康回读。
pub fn spawn(service: LightingService) -> std::io::Result<()> {
    std::thread::Builder::new().name("airpulse-status".into()).spawn(move || {
        let mut last_probe = Instant::now() - Duration::from_secs(2);
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let Ok(mut session) = service.lock() else { return; };
            if session.automatic.shutdown { return; }
            if !session.automatic.enabled { continue; }
            let result = StatusStore::local().and_then(|store| refresh_status(&mut session, &store));
            if let Err(error) = result {
                session.pause_automatic(error);
                continue;
            }
            let changed = session.automatic.applied != Some(session.automatic.status);
            if !changed && (session.automatic.status == TaskStatus::Idle
                || last_probe.elapsed() < Duration::from_secs(2))
            {
                continue;
            }
            if session.automatic.error.is_some() && last_probe.elapsed() < Duration::from_secs(2) {
                continue;
            }
            last_probe = Instant::now();
            let connection = connect_air75().and_then(|(descriptor, mut controller)| {
                let handle = controller.select_current_mode()?;
                Ok((descriptor, controller, handle))
            });
            match connection {
                Ok((descriptor, mut controller, handle)) => {
                    if let Err(error) = session.sync_automatic(&mut controller, &descriptor.path, handle) {
                        session.pause_automatic(error);
                    } else {
                        session.automatic.error = None;
                    }
                }
                Err(error) => {
                    session.automatic.error = Some(format!("等待键盘连接: {error:#}"));
                }
            }
        }
    }).map(|_| ())
}

/// 获取软件状态不访问 HID，界面轮询不会触发额外写灯。
#[tauri::command]
pub async fn get_status(service: tauri::State<'_, LightingService>) -> Result<RuntimeSnapshot, String> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.lock().map(|session| session.automatic.clone())
            .map_err(|_| "灯光服务状态异常，请重启应用".to_owned())
    }).await.map_err(|error| format!("状态后台任务异常: {error}"))?
}

/// 启用前拒绝覆盖所选区域的手动恢复点；空闲启动不会写键盘。
#[tauri::command]
pub async fn start_automatic(
    service: tauri::State<'_, LightingService>,
    brightness: u8,
    zone: DisplayZone,
    colors: StatusColors,
) -> Result<RuntimeSnapshot, String> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<RuntimeSnapshot> {
        ensure!(brightness <= 100, "亮度必须是 0 到 100 的整数");
        let mut session = service.lock().map_err(|_| anyhow::anyhow!("灯光服务状态异常"))?;
        ensure!(!session.automatic.shutdown, "应用正在退出");
        ensure!(!session.automatic.enabled, "Codex 联动已经开启");
        ensure!(!session.automatic.restore_pending, "请先重试恢复原灯效，再修改展示区域或重新启动联动");
        ensure!(zone.zones().iter().all(|item| !session.has_snapshot(*item)),
            "请先恢复所选区域的手动修改，再启动联动");
        let store = StatusStore::local()?;
        refresh_status(&mut session, &store)?;
        session.automatic.brightness = brightness;
        session.automatic.zone = zone;
        session.automatic.colors = colors;
        session.automatic.applied = None;
        session.automatic.error = None;
        if session.automatic.status == TaskStatus::Idle {
            session.automatic.applied = Some(TaskStatus::Idle);
        } else {
            let (descriptor, mut controller) = connect_air75()?;
            let handle = controller.select_current_mode()?;
            if let Err(error) = session.sync_automatic(&mut controller, &descriptor.path, handle) {
                session.pause_automatic(error);
                bail!("{}", session.automatic.error.as_deref().unwrap_or("自动写入失败"));
            }
        }
        session.automatic.enabled = true;
        Ok(session.automatic.clone())
    }).await.map_err(|error| format!("联动后台任务异常: {error}"))?
        .map_err(|error| format!("{error:#}"))
}

/// 停止后只恢复自动模式拥有的所选区域快照，失败时仍可点击重试。
fn stop(session: &mut LightingSession) -> Result<()> {
    session.automatic.enabled = false;
    if session.automatic.restore_pending {
        let result = connect_air75().and_then(|(descriptor, mut controller)| {
            let handle = controller.select_current_mode()?;
            session.restore_automatic(&mut controller, &descriptor.path, handle)
        });
        if let Err(error) = result {
            session.automatic.error = Some(format!("停止联动时{error:#}"));
            return Err(error);
        }
    }
    session.automatic.restore_pending = false;
    session.automatic.applied = None;
    session.automatic.error = None;
    Ok(())
}

/// 停止与自动写入串行执行，返回前已完成恢复回读。
#[tauri::command]
pub async fn stop_automatic(service: tauri::State<'_, LightingService>) -> Result<RuntimeSnapshot, String> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<RuntimeSnapshot> {
        let mut session = service.lock().map_err(|_| anyhow::anyhow!("灯光服务状态异常"))?;
        stop(&mut session)?;
        Ok(session.automatic.clone())
    }).await.map_err(|error| format!("恢复后台任务异常: {error}"))?
        .map_err(|error| format!("{error:#}"))
}

/// 正常退出尽力恢复，强制结束进程不保证能执行此清理。
pub fn shutdown(service: &LightingService) -> Result<()> {
    let mut session = service.lock().map_err(|_| anyhow::anyhow!("灯光服务状态异常"))?;
    session.automatic.shutdown = true;
    stop(&mut session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_lighting::tests::controller;

    /// 双区写入使用自定义色，空闲时恢复两区首次快照。
    #[test]
    fn automatic_applies_both_zones_and_restores_on_idle() {
        let mut session = LightingSession::default();
        let mut device = controller(None);
        let initial = device.read_state().unwrap();
        session.automatic.zone = DisplayZone::Both;
        session.automatic.status = TaskStatus::Running;
        session.automatic.colors.running = [12, 34, 56];
        session.sync_automatic(&mut device, "a", 1).unwrap();
        let active = device.read_state().unwrap();
        assert_eq!(&active[6..9], &[12, 34, 56]);
        assert_eq!(&active[14..17], &[12, 34, 56]);
        session.automatic.status = TaskStatus::Idle;
        session.sync_automatic(&mut device, "a", 1).unwrap();
        assert_eq!(device.read_state().unwrap(), initial);
        assert!(!session.automatic.restore_pending);
    }

    /// 双区第二次写入失败时恢复两区，不能留下半完成的状态。
    #[test]
    fn automatic_rolls_back_both_zones_after_write_failure() {
        let mut session = LightingSession::default();
        let mut device = controller(Some(4));
        let initial = device.read_state().unwrap();
        session.automatic.zone = DisplayZone::Both;
        session.automatic.status = TaskStatus::Running;
        assert!(session.sync_automatic(&mut device, "a", 1).is_err());
        assert_eq!(device.read_state().unwrap(), initial);
    }

    /// 单区接管不改变另一分区，状态匹配时健康回读必须零写入。
    #[test]
    fn automatic_preserves_other_zone_and_deduplicates_writes() {
        for zone in [DisplayZone::Side, DisplayZone::Main] {
            let mut session = LightingSession::default();
            let mut device = controller(None);
            let initial = device.read_state().unwrap();
            session.automatic.zone = zone;
            session.automatic.status = TaskStatus::Running;
            session.sync_automatic(&mut device, "a", 1).unwrap();
            let active = device.read_state().unwrap();
            if zone == DisplayZone::Side {
                assert_eq!(active[..9], initial[..9]);
            } else {
                assert_eq!(active[9..], initial[9..]);
            }
            let io = device.into_inner();
            let writes = io.writes;
            device = Air75Controller::new(io);
            session.sync_automatic(&mut device, "a", 1).unwrap();
            assert_eq!(device.into_inner().writes, writes);
        }
    }

    /// 重连后实际参数丢失需重放当前状态，设备或模式改变则拒绝旧恢复点。
    #[test]
    fn automatic_replays_lost_state_but_rejects_other_device_or_mode() {
        let mut session = LightingSession::default();
        let mut device = controller(None);
        let initial = device.read_state().unwrap();
        session.automatic.status = TaskStatus::Running;
        session.sync_automatic(&mut device, "a", 1).unwrap();
        let desired = device.read_state().unwrap();
        let mut io = device.into_inner();
        let writes = io.writes;
        io.state = initial;
        device = Air75Controller::new(io);
        session.sync_automatic(&mut device, "a", 1).unwrap();
        assert_eq!(device.read_state().unwrap(), desired);
        let io = device.into_inner();
        assert!(io.writes > writes);
        let writes = io.writes;
        device = Air75Controller::new(io);
        for (path, handle) in [("b", 1), ("a", 0)] {
            assert!(session.sync_automatic(&mut device, path, handle).is_err());
        }
        assert_eq!(device.into_inner().writes, writes);
    }
}
