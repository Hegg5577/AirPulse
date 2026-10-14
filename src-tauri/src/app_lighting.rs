use crate::hid::{Air75Controller, LightingIo, connect_air75};
use crate::lighting::{Air75MainState, Air75SideState};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

pub type LightingService = Arc<Mutex<LightingSession>>;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LightingZone {
    Main,
    Side,
}

struct SavedRegion {
    device_path: String,
    handle: u8,
    state: [u8; 17],
}

#[derive(Default)]
pub struct LightingSession {
    main: Option<SavedRegion>,
    side: Option<SavedRegion>,
    pub(crate) automatic: crate::runtime::RuntimeSnapshot,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LightingView {
    handle: u8,
    pub(crate) state: [u8; 17],
    pub(crate) main_restore_available: bool,
    pub(crate) side_restore_available: bool,
}

pub(crate) enum Operation {
    Read,
    Set { zone: LightingZone, color: [u8; 3], brightness: u8 },
    Restore(LightingZone),
}

impl LightingSession {
    /// 自动模式只查询所选区域，避免把另一分区的手动恢复点误算为自己拥有。
    pub(crate) fn has_snapshot(&self, zone: LightingZone) -> bool {
        match zone {
            LightingZone::Main => self.main.is_some(),
            LightingZone::Side => self.side.is_some(),
        }
    }

    /// 分区保存恢复点，避免恢复主键区时覆盖后来设置的侧灯。
    fn saved(&mut self, zone: LightingZone) -> &mut Option<SavedRegion> {
        match zone {
            LightingZone::Main => &mut self.main,
            LightingZone::Side => &mut self.side,
        }
    }

    /// 只有设备接口和当前模式一致时，才允许使用对应恢复点。
    fn view(&self, device_path: &str, handle: u8, state: [u8; 17]) -> LightingView {
        let matches = |saved: &Option<SavedRegion>| {
            saved.as_ref().is_some_and(|saved| {
                saved.device_path == device_path && saved.handle == handle
            })
        };
        LightingView {
            handle,
            state,
            main_restore_available: matches(&self.main),
            side_restore_available: matches(&self.side),
        }
    }

    /// 写入前保存首次快照；失败时恢复本次操作前状态并保留手动恢复点。
    pub(crate) fn operate<T: LightingIo>(
        &mut self,
        controller: &mut Air75Controller<T>,
        device_path: &str,
        handle: u8,
        operation: Operation,
    ) -> Result<LightingView> {
        if let Operation::Set { brightness, .. } = &operation {
            ensure!(*brightness <= 100, "亮度必须是 0 到 100 的整数");
        }
        controller.set_mode_handle(handle);
        let before = controller.read_state()?;
        let zone = match &operation {
            Operation::Read => return Ok(self.view(device_path, handle, before)),
            Operation::Set { zone, .. } | Operation::Restore(zone) => *zone,
        };
        let saved = self.saved(zone);
        if let Some(saved) = saved.as_ref() {
            ensure!(
                saved.device_path == device_path && saved.handle == handle,
                "此区域的恢复点属于其他设备或模式，请接回原设备并切回原模式后操作"
            );
        }
        let restoring = matches!(&operation, Operation::Restore(_));
        let mut target = before;
        match operation {
            Operation::Set { color: [red, green, blue], brightness, .. } => {
                match zone {
                    LightingZone::Main => target[..9].copy_from_slice(
                        Air75MainState::steady(brightness, red, green, blue)
                            .expect("亮度已校验")
                            .as_bytes(),
                    ),
                    LightingZone::Side => {
                        let mut side = *Air75SideState::steady(red, green, blue).as_bytes();
                        side[1] = brightness;
                        target[9..].copy_from_slice(&side);
                    }
                }
                if saved.is_none() {
                    *saved = Some(SavedRegion {
                        device_path: device_path.to_owned(),
                        handle,
                        state: before,
                    });
                }
            }
            Operation::Restore(_) => {
                target = saved.as_ref().ok_or_else(|| anyhow::anyhow!("此区域没有恢复点"))?.state;
            }
            Operation::Read => unreachable!("读取已提前返回"),
        }
        let after = match apply_region(controller, zone, target) {
            Ok(after) => after,
            Err(error) => match apply_region(controller, zone, before) {
                Ok(_) => return Err(error.context("操作失败；该区域已恢复到本次操作前状态并回读确认")),
                Err(restore_error) => {
                    bail!("操作失败: {error:#}；恢复本次操作前状态也失败: {restore_error:#}。请重新连接后恢复，必要时使用 NuPhyIO")
                }
            },
        };
        if restoring {
            *self.saved(zone) = None;
        }
        Ok(self.view(device_path, handle, after))
    }
}

/// 复用实机已验证的分区提交方式，始终严格核对另一分区未改变。
fn apply_region<T: LightingIo>(
    controller: &mut Air75Controller<T>,
    zone: LightingZone,
    state: [u8; 17],
) -> Result<[u8; 17]> {
    match zone {
        LightingZone::Main => controller.apply_and_verify(Air75MainState::from_bytes(
            state[..9].try_into().expect("固定主键区状态长度"),
        )),
        LightingZone::Side => controller.apply_side_and_verify(Air75SideState::from_bytes(
            state[9..].try_into().expect("固定侧灯状态长度"),
        )),
    }
}

/// 手动与自动模式共用锁；自动恢复前禁止手动覆盖快照和灯光。
async fn execute(service: LightingService, operation: Operation) -> Result<LightingView, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<LightingView> {
        let mut session = service.lock().map_err(|_| anyhow::anyhow!("灯光服务状态异常，请重启应用"))?;
        if !matches!(&operation, Operation::Read) {
            ensure!(!session.automatic.enabled && !session.automatic.restore_pending,
                "请先停止 Codex 联动并恢复，再使用手动控灯");
        }
        let (descriptor, mut controller) = connect_air75()?;
        let handle = controller.select_current_mode()?;
        session.operate(&mut controller, &descriptor.path, handle, operation)
    })
    .await
    .map_err(|error| format!("灯光后台任务异常: {error}"))?
    .map_err(|error| format!("{error:#}"))
}

/// 只读取当前模式和灯光参数，不创建恢复点或修改灯光。
#[tauri::command]
pub async fn read_lighting(service: tauri::State<'_, LightingService>) -> Result<LightingView, String> {
    execute(service.inner().clone(), Operation::Read).await
}

/// 校验界面输入后，设置单个区域的静态颜色和整数亮度。
#[tauri::command]
pub async fn set_lighting(
    service: tauri::State<'_, LightingService>,
    zone: LightingZone,
    color: [u8; 3],
    brightness: u8,
) -> Result<LightingView, String> {
    if brightness > 100 {
        return Err("亮度必须是 0 到 100 的整数".to_owned());
    }
    execute(service.inner().clone(), Operation::Set { zone, color, brightness }).await
}

/// 恢复所选区域首次修改前的完整灯效参数。
#[tauri::command]
pub async fn restore_lighting(
    service: tauri::State<'_, LightingService>,
    zone: LightingZone,
) -> Result<LightingView, String> {
    execute(service.inner().clone(), Operation::Restore(zone)).await
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct FakeIo {
        pub(crate) state: [u8; 17],
        pub(crate) writes: usize,
        fail_on_write: Option<usize>,
    }

    impl LightingIo for FakeIo {
        /// 模拟设备真实存储状态，让恢复测试覆盖分区之间的影响。
        fn read_state(&mut self, _handle: u8) -> Result<[u8; 17]> {
            Ok(self.state)
        }

        /// 在写入生效后模拟丢失确认，以验证部分成功时仍能回滚。
        fn write(&mut self, address: u16, payload: &[u8], _handle: u8) -> Result<()> {
            self.writes += 1;
            let start = usize::from(address);
            self.state[start..start + payload.len()].copy_from_slice(payload);
            ensure!(self.fail_on_write != Some(self.writes), "模拟确认失败");
            Ok(())
        }
    }

    /// 使用不同的主键区动态灯效与侧灯状态，防止恢复被静态默认值蒙混通过。
    pub(crate) fn controller(fail_on_write: Option<usize>) -> Air75Controller<FakeIo> {
        Air75Controller::new(FakeIo {
            state: [4, 60, 3, 0, 0, 0, 240, 0, 255, 2, 30, 3, 0, 0, 0, 255, 0],
            writes: 0,
            fail_on_write,
        })
    }

    /// 分区恢复只能恢复该分区首次状态，不能覆盖之后修改的另一分区。
    #[test]
    fn restoring_main_preserves_later_side_change() {
        let mut session = LightingSession::default();
        let mut controller = controller(None);
        let initial = controller.read_state().unwrap();
        for brightness in [10, 60] {
            session.operate(&mut controller, "device-a", 1, Operation::Set {
                zone: LightingZone::Main, color: [0, 0, 255], brightness,
            }).unwrap();
        }
        let side = session.operate(&mut controller, "device-a", 1, Operation::Set {
            zone: LightingZone::Side, color: [18, 180, 200], brightness: 20,
        }).unwrap();
        let restored = session.operate(&mut controller, "device-a", 1,
            Operation::Restore(LightingZone::Main)).unwrap();
        assert_eq!(restored.state[..9], initial[..9]);
        assert_eq!(restored.state[9..], side.state[9..]);
        assert!(!restored.main_restore_available);
        assert!(restored.side_restore_available);
    }

    /// 设备路径或模式改变后，旧快照对应区域的修改与恢复都必须零写入。
    #[test]
    fn stale_snapshot_rejects_set_and_restore() {
        let mut session = LightingSession::default();
        let mut controller = controller(None);
        session.operate(&mut controller, "device-a", 1, Operation::Set {
            zone: LightingZone::Side, color: [0, 0, 255], brightness: 30,
        }).unwrap();
        let io = controller.into_inner();
        let writes = io.writes;
        let mut controller = Air75Controller::new(io);
        for (path, handle) in [("device-a", 0), ("device-b", 1)] {
            let read = session.operate(&mut controller, path, handle, Operation::Read).unwrap();
            assert!(!read.side_restore_available);
            assert!(session.operate(&mut controller, path, handle,
                Operation::Restore(LightingZone::Side)).is_err());
            assert!(session.operate(&mut controller, path, handle, Operation::Set {
                zone: LightingZone::Side, color: [255, 0, 0], brightness: 30,
            }).is_err());
            let io = controller.into_inner();
            assert_eq!(io.writes, writes);
            controller = Air75Controller::new(io);
        }
    }

    /// 写入已部分生效后失败，需要恢复本次操作前状态并保留首次恢复点。
    #[test]
    fn failed_write_rolls_back_and_retains_snapshot() {
        let mut session = LightingSession::default();
        let mut controller = controller(Some(2));
        let initial = controller.read_state().unwrap();
        let error = session.operate(&mut controller, "device-a", 1, Operation::Set {
            zone: LightingZone::Side, color: [160, 32, 240], brightness: 10,
        }).err().expect("模拟写入应失败");
        assert!(format!("{error:#}").contains("已恢复到本次操作前状态"));
        assert_eq!(controller.read_state().unwrap(), initial);
        assert!(session.view("device-a", 1, initial).side_restore_available);
    }

    /// 越界亮度在写灯和建立恢复点之前被拒绝。
    #[test]
    fn invalid_brightness_does_not_write_or_save() {
        let mut session = LightingSession::default();
        let mut controller = controller(None);
        assert!(session.operate(&mut controller, "device-a", 1, Operation::Set {
            zone: LightingZone::Main, color: [255, 255, 255], brightness: 101,
        }).is_err());
        assert_eq!(controller.into_inner().writes, 0);
        assert!(session.main.is_none());
    }
}
