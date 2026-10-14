use crate::device::{Bus, DeviceDescriptor, select_control_interface};
use crate::lighting::{Air75MainState, Air75SideState};
use crate::protocol::{
    NuPhyRequest, NuPhySessionKey, build_session_request, decode_led_count, validate_response,
    validate_session_response,
};
use anyhow::{Context, Result, bail};
use hidapi::{BusType, HidApi, HidDevice};
use std::time::Duration;

const SIDE_LIGHT_HANDLE: u8 = 0;

#[derive(Clone, Debug)]
pub struct ProbeReport {
    pub descriptor: DeviceDescriptor,
    pub state: [u8; 17],
}

pub trait LightingIo {
    /// 读取指定键盘模式下完整的主键区与侧灯状态。
    fn read_state(&mut self, handle: u8) -> Result<[u8; 17]>;

    /// 向指定键盘模式写入协议数据。
    fn write(&mut self, address: u16, payload: &[u8], handle: u8) -> Result<()>;
}

pub struct Air75Controller<T> {
    io: T,
    mode_handle: u8,
    protected_side: Option<[u8; 8]>,
    protected_main: Option<[u8; 9]>,
}

impl<T: LightingIo> Air75Controller<T> {
    /// 创建控制器；首次写入前会读取并锁定侧灯状态。
    pub fn new(io: T) -> Self {
        Self {
            io,
            mode_handle: SIDE_LIGHT_HANDLE,
            protected_side: None,
            protected_main: None,
        }
    }

    /// 切换后续灯光读写使用的键盘模式 handle。
    pub fn set_mode_handle(&mut self, handle: u8) {
        self.mode_handle = handle;
        self.protected_side = None;
        self.protected_main = None;
    }

    /// 返回实体灯光测试的固定顺序。
    pub fn exercise_states() -> [Air75MainState; 4] {
        [
            Air75MainState::blue(),
            Air75MainState::orange(),
            Air75MainState::green(),
            Air75MainState::off(),
        ]
    }

    /// 返回侧灯实体测试的固定顺序。
    pub fn side_exercise_states() -> [Air75SideState; 4] {
        [
            Air75SideState::blue(),
            Air75SideState::orange(),
            Air75SideState::green(),
            Air75SideState::off(),
        ]
    }

    /// 读取当前完整灯光状态。
    pub fn read_state(&mut self) -> Result<[u8; 17]> {
        self.io.read_state(self.mode_handle)
    }

    /// 读取指定键盘模式的完整灯光状态，用于核对 NuPhyIO 的模式 handle。
    pub fn read_state_for_handle(&mut self, handle: u8) -> Result<[u8; 17]> {
        self.io.read_state(handle)
    }

    /// 先按实机验证的满亮度基准设置颜色，再独立提交目标亮度并严格回读。
    pub fn apply_and_verify(&mut self, main: Air75MainState) -> Result<[u8; 17]> {
        let side = match self.protected_side {
            Some(side) => side,
            None => {
                let before = self.io.read_state(self.mode_handle)?;
                let side: [u8; 8] = before[9..].try_into().expect("固定侧灯状态长度") ;
                self.protected_side = Some(side);
                side
            }
        };
        // 低亮度颜色块曾导致提交后 RGB 缩放，沿用已通过诊断的 100% 基准。
        let mut color = *main.as_bytes();
        color[1] = 100;
        self.io.write(0, &color, self.mode_handle)?;
        std::thread::sleep(Duration::from_millis(200));
        self.io.write(1, &main.as_bytes()[1..2], self.mode_handle)?;
        // 最后提交亮度后不再覆盖颜色，等待实体控制与回读状态稳定。
        std::thread::sleep(Duration::from_millis(200));
        let mut after = self.io.read_state(self.mode_handle)?;
        // 实机确认 ACK 后可能仍读到上一模式；只轮询回读，不重复写入。
        for _ in 0..5 {
            if after[..9] == main.as_bytes()[..] || after[9..] != side {
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
            after = self.io.read_state(self.mode_handle)?;
        }
        if after[..9] != main.as_bytes()[..] {
            bail!(
                "主键区灯光回读不匹配: expected={}, actual={}",
                format_bytes(main.as_bytes()),
                format_bytes(&after[..9])
            );
        }
        if after[9..] != side {
            bail!("侧灯状态发生变化，已停止后续写入");
        }
        Ok(after)
    }

    /// 分步写入 Air75 V3 侧灯状态和亮度提交位，然后验证主键区未改变。
    pub fn apply_side_and_verify(&mut self, side: Air75SideState) -> Result<[u8; 17]> {
        let main = match self.protected_main {
            Some(main) => main,
            None => {
                let before = self.io.read_state(self.mode_handle)?;
                let main: [u8; 9] = before[..9].try_into().expect("固定主键区状态长度");
                self.protected_main = Some(main);
                main
            }
        };
        // 官方机械协议在侧灯块写入后，单独提交绝对地址 10 的亮度字节。
        self.io.write(9, side.as_bytes(), self.mode_handle)?;
        self.io.write(10, &side.as_bytes()[1..2], self.mode_handle)?;
        let after = self.io.read_state(self.mode_handle)?;
        if after[..9] != main {
            bail!(
                "主键区状态发生变化，已停止侧灯写入: before={}, after={}",
                format_bytes(&main),
                format_bytes(&after[..9])
            );
        }
        if after[9..] != side.as_bytes()[..] {
            bail!(
                "侧灯回读不匹配: expected={}, actual={}",
                format_bytes(side.as_bytes()),
                format_bytes(&after[9..])
            );
        }
        Ok(after)
    }

    /// 测试蓝、橙、绿和熄灭侧灯，结束或失败时恢复原侧灯状态。
    pub fn exercise_side<F>(&mut self, mut verified: F) -> Result<()>
    where
        F: FnMut(Air75SideState),
    {
        self.test_side_states(&Self::side_exercise_states(), |side| {
            verified(side);
            Ok(())
        })
    }

    /// 保持单个侧灯颜色供人工观察，无论观察或回读是否成功都尝试恢复。
    pub fn test_side_once<F>(&mut self, side: Air75SideState, observed: F) -> Result<()>
    where
        F: FnMut(Air75SideState) -> Result<()>,
    {
        self.test_side_states(&[side], observed)
    }

    /// 分步观察侧灯并恢复初始快照，任一步失败都会停止并报告恢复结果。
    pub fn test_side_states<F>(&mut self, states: &[Air75SideState], mut observed: F) -> Result<()>
    where
        F: FnMut(Air75SideState) -> Result<()>,
    {
        let initial = self.io.read_state(self.mode_handle)?;
        self.protected_main = Some(initial[..9].try_into().expect("固定主键区状态长度"));
        let original = Air75SideState::from_bytes(
            initial[9..].try_into().expect("固定侧灯状态长度"),
        );
        let result: Result<()> = (|| {
            for &side in states {
                self.apply_side_and_verify(side)?;
                observed(side)?;
            }
            Ok(())
        })();
        let restore = self.apply_side_and_verify(original);
        match (result, restore) {
            (Ok(()), Ok(_)) => Ok(()),
            (Err(error), Ok(_)) => Err(error.context("测试未通过；原侧灯状态已恢复并回读确认")),
            (Ok(()), Err(error)) => Err(error.context("恢复原侧灯状态失败")),
            (Err(error), Err(restore_error)) => {
                bail!("测试失败: {error:#}；恢复原侧灯状态也失败: {restore_error:#}")
            }
        }
    }

    /// 主键区单色观察结束或失败后恢复原灯效，并保护侧灯完整状态。
    pub fn test_main_once<F>(&mut self, main: Air75MainState, observed: F) -> Result<()>
    where
        F: FnOnce(Air75MainState) -> Result<()>,
    {
        // 单步入口保留 FnOnce 兼容性，多步流程只会消费该回调一次。
        let mut observed = Some(observed);
        self.test_main_states(&[main], |main| {
            observed.take().expect("单步观察仅调用一次")(main)
        })
    }

    /// 分步观察主键区状态，失败时停止后续步骤并恢复最初快照。
    pub fn test_main_states<F>(&mut self, states: &[Air75MainState], mut observed: F) -> Result<()>
    where
        F: FnMut(Air75MainState) -> Result<()>,
    {
        let initial = self.io.read_state(self.mode_handle)?;
        self.protected_side = Some(initial[9..].try_into().expect("固定侧灯状态长度"));
        let original = Air75MainState::from_bytes(
            initial[..9].try_into().expect("固定主键区状态长度"),
        );
        let result: Result<()> = (|| {
            for &main in states {
                self.apply_and_verify(main)?;
                observed(main)?;
            }
            Ok(())
        })();
        let restore = self.apply_and_verify(original);
        match (result, restore) {
            (Ok(()), Ok(_)) => Ok(()),
            (Err(error), Ok(_)) => Err(error.context("测试未通过；原主键区状态已恢复并回读确认")),
            (Ok(()), Err(error)) => Err(error.context("恢复原主键区状态失败")),
            (Err(error), Err(restore_error)) => {
                bail!("测试失败: {error:#}；恢复原主键区状态也失败: {restore_error:#}")
            }
        }
    }

    /// 分阶段采样主键区写入，区分颜色块和亮度提交的影响，最后恢复快照。
    pub fn diagnose_main_once<F>(&mut self, main: Air75MainState, mut observed: F) -> Result<()>
    where
        F: FnMut(&str, [u8; 17]) -> Result<()>,
    {
        let initial = self.io.read_state(self.mode_handle)?;
        let side: [u8; 8] = initial[9..].try_into().expect("固定侧灯状态长度");
        self.protected_side = Some(side);
        let original = Air75MainState::from_bytes(
            initial[..9].try_into().expect("固定主键区状态长度"),
        );
        let result: Result<()> = (|| {
            self.io.write(0, main.as_bytes(), self.mode_handle)?;
            for stage in ["block", "brightness", "settled"] {
                if stage == "brightness" {
                    self.io.write(1, &main.as_bytes()[1..2], self.mode_handle)?;
                }
                // 分开等待两次提交生效；诊断只记录实际值，不放宽正常写入的校验。
                std::thread::sleep(Duration::from_millis(200));
                let actual = self.io.read_state(self.mode_handle)?;
                anyhow::ensure!(actual[9..] == side, "侧灯状态发生变化，停止诊断");
                observed(stage, actual)?;
            }
            Ok(())
        })();
        let restore = self.apply_and_verify(original);
        match (result, restore) {
            (Ok(()), Ok(_)) => Ok(()),
            (Err(error), Ok(_)) => Err(error.context("诊断未完成；原主键区状态已恢复并回读确认")),
            (Ok(()), Err(error)) => Err(error.context("恢复原主键区状态失败")),
            (Err(error), Err(restore_error)) => {
                bail!("诊断失败: {error:#}；恢复原主键区状态也失败: {restore_error:#}")
            }
        }
    }

    /// 固定蓝色后仅提交亮度，保留固件实际回读，结束或失败时恢复原快照。
    pub fn diagnose_main_brightness<F>(&mut self, mut observed: F) -> Result<()>
    where
        F: FnMut(u8, [u8; 17]) -> Result<()>,
    {
        let initial = self.io.read_state(self.mode_handle)?;
        let side: [u8; 8] = initial[9..].try_into().expect("固定侧灯状态长度");
        self.protected_side = Some(side);
        let original = Air75MainState::from_bytes(
            initial[..9].try_into().expect("固定主键区状态长度"),
        );
        let blue = Air75MainState::steady(100, 0, 0, 255).expect("固定测试亮度有效");
        let result: Result<()> = (|| {
            self.io.write(0, blue.as_bytes(), self.mode_handle)?;
            std::thread::sleep(Duration::from_millis(200));
            for brightness in [100, 10, 60, 0] {
                // 提交后不再覆盖 RGB，用实体观察验证亮度是否真正生效。
                self.io.write(1, &[brightness], self.mode_handle)?;
                std::thread::sleep(Duration::from_millis(200));
                let actual = self.io.read_state(self.mode_handle)?;
                anyhow::ensure!(actual[9..] == side, "侧灯状态发生变化，停止亮度诊断");
                observed(brightness, actual)?;
            }
            Ok(())
        })();
        // 恢复与正常控制共用提交和严格校验，实体恢复效果另行确认。
        let restore = self.apply_and_verify(original);
        match (result, restore) {
            (Ok(()), Ok(_)) => Ok(()),
            (Err(error), Ok(_)) => Err(error.context("亮度诊断未完成；原主键区状态已恢复并回读确认")),
            (Ok(()), Err(error)) => Err(error.context("恢复原主键区状态失败")),
            (Err(error), Err(restore_error)) => {
                bail!("亮度诊断失败: {error:#}；恢复原主键区状态也失败: {restore_error:#}")
            }
        }
    }

    /// 依次验证蓝、橙、绿和熄灭；失败时仍尝试熄灭。
    pub fn exercise<F>(&mut self, mut verified: F) -> Result<()>
    where
        F: FnMut(Air75MainState),
    {
        for state in Self::exercise_states() {
            if let Err(error) = self.apply_and_verify(state) {
                if state != Air75MainState::off() {
                    let _ = self.apply_and_verify(Air75MainState::off());
                }
                return Err(error);
            }
            verified(state);
        }
        Ok(())
    }

    /// 取回底层传输，主要用于验证实际写入序列。
    pub fn into_inner(self) -> T {
        self.io
    }
}

fn format_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct HidLightingIo {
    device: HidDevice,
    key: NuPhySessionKey,
}

#[derive(Clone, Debug)]
pub struct CustomLightingReport {
    pub firmware: Vec<u8>,
    pub led_count: u16,
}

impl Air75Controller<HidLightingIo> {
    /// 根据基础信息选择当前模式，避免把颜色写入未启用的模式。
    pub fn select_current_mode(&mut self) -> Result<u8> {
        let base: [u8; 8] = self.io.exchange(&NuPhyRequest::base_info())?;
        if base[0] > 1 {
            bail!("键盘返回未知模式 {}，停止写入", base[0]);
        }
        self.set_mode_handle(base[0]);
        Ok(base[0])
    }

    /// 读取指定模式下完整的 104 颗 LED 颜色表。
    pub fn read_color_table(&mut self, handle: u8) -> Result<Vec<u8>> {
        let mut colors = Vec::with_capacity(104 * 4);
        while colors.len() < 104 * 4 {
            let remaining = 104 * 4 - colors.len();
            let length = remaining.min(crate::protocol::PAYLOAD_LEN) as u8;
            let request = NuPhyRequest::read_colors(colors.len() as u16, length, handle)
                .map_err(|error| anyhow::anyhow!("颜色表读取请求无效: {error:?}"))?;
            let chunk: Vec<u8> = self.io.exchange(&request)?;
            colors.extend_from_slice(&chunk);
        }
        Ok(colors)
    }

    /// 读取支持自定义渲染所需的固件信息与 LED 数量。
    pub fn custom_lighting_report(&mut self) -> Result<CustomLightingReport> {
        let firmware: Vec<u8> = self.io.exchange(&NuPhyRequest::firmware_info())?;
        let count: [u8; 2] = self.io.exchange(&NuPhyRequest::led_count())?;
        Ok(CustomLightingReport {
            firmware,
            led_count: decode_led_count(count, self.io.key),
        })
    }

    /// 使用隐藏灯效 21 直接测试主键区 RGB，并在结束后恢复原灯效。
    pub fn exercise_custom<F>(&mut self, mut verified: F) -> Result<CustomLightingReport>
    where
        F: FnMut(&'static str),
    {
        let report = self.custom_lighting_report()?;
        let saved = self.io.read_state(self.mode_handle)?;
        let mut armed = saved;
        armed[0] = 21;
        armed[1] = 100;
        armed[4] = 1;
        self.io.write_state(&armed)?;
        std::thread::sleep(Duration::from_millis(120));
        let active = self.io.read_state(self.mode_handle)?;
        if active[0] != 21 {
            let _ = self.io.write_state(&saved);
            bail!("固件拒绝隐藏灯效 21，请先通过 NuPhyIO 更新 Air75 V3 固件");
        }

        let result = (|| {
            for (name, color) in [
                ("蓝色", [0, 0, 255]),
                ("橙色", [255, 132, 0]),
                ("绿色", [0, 255, 0]),
                ("熄灭", [0, 0, 0]),
            ] {
                self.io.paint_uniform(report.led_count, color)?;
                verified(name);
                std::thread::sleep(Duration::from_millis(800));
            }
            Ok(())
        })();
        let restore = self.io.write_state(&saved);
        result.and(restore)?;
        Ok(report)
    }
}

impl LightingIo for HidLightingIo {
    /// 读取指定键盘模式的完整灯光状态。
    fn read_state(&mut self, handle: u8) -> Result<[u8; 17]> {
        let request = NuPhyRequest::read_for_handle(0, 17, handle)
            .map_err(|e| anyhow::anyhow!("读取请求无效: {e:?}"))?;
        exchange(&self.device, &request, self.key)
    }

    /// 向指定键盘模式写入灯光状态。
    fn write(&mut self, address: u16, payload: &[u8], handle: u8) -> Result<()> {
        let request = NuPhyRequest::write_for_handle(address, payload, handle)
            .map_err(|e| anyhow::anyhow!("写入请求无效: {e:?}"))?;
        let _: Vec<u8> = exchange(&self.device, &request, self.key)?;
        Ok(())
    }
}

impl HidLightingIo {
    fn exchange<T>(&self, request: &NuPhyRequest) -> Result<T>
    where
        T: TryFrom<Vec<u8>>,
    {
        exchange(&self.device, request, self.key)
    }

    /// 按 NuPhyIO 的 Windows 模式协议重复提交完整灯光状态。
    fn write_state(&mut self, state: &[u8; 17]) -> Result<()> {
        for _ in 0..2 {
            self.write(0, state, SIDE_LIGHT_HANDLE)?;
            std::thread::sleep(Duration::from_millis(40));
            self.write(1, &[state[1]], SIDE_LIGHT_HANDLE)?;
            std::thread::sleep(Duration::from_millis(40));
        }
        Ok(())
    }

    fn paint_uniform(&mut self, led_count: u16, color: [u8; 3]) -> Result<()> {
        let mut records = Vec::with_capacity(usize::from(led_count) * 4);
        for index in 0..led_count {
            let index = u8::try_from(index)
                .with_context(|| format!("LED 数量 {led_count} 超过协议索引范围"))?;
            records.extend_from_slice(&[index, color[0], color[1], color[2]]);
        }
        for chunk in records.chunks(52) {
            let request = NuPhyRequest::write_colors(chunk)
                .map_err(|e| anyhow::anyhow!("RGB 表请求无效: {e:?}"))?;
            let _: Vec<u8> = self.exchange(&request)?;
        }
        Ok(())
    }
}

/// 打开唯一验证过的 Air75 V3 控制接口并建立临时会话。
pub fn connect_air75() -> Result<(DeviceDescriptor, Air75Controller<HidLightingIo>)> {
    let api = HidApi::new().context("无法枚举 HID 设备")?;
    let descriptors = enumerate_air75_candidates()?;
    let selected = select_control_interface(&descriptors)
        .map_err(|e| anyhow::anyhow!("控制接口选择失败: {e:?}"))?
        .clone();
    let info = api
        .device_list()
        .find(|d| d.path().to_string_lossy() == selected.path)
        .context("控制接口已断开")?;
    let device = info.open_device(&api).context("无法打开 Air75 V3 控制接口")?;
    let challenge = std::array::from_fn(|index| (index as u8).wrapping_mul(37).wrapping_add(11));
    send(&device, &build_session_request(&challenge))?;
    let key = validate_session_response(&receive(&device)?, &challenge)
        .map_err(|e| anyhow::anyhow!("会话响应无效: {e:?}"))?;
    Ok((selected, Air75Controller::new(HidLightingIo { device, key })))
}

/// 枚举当前连接的 Air75 V3 HID 集合。
pub fn enumerate_air75_candidates() -> Result<Vec<DeviceDescriptor>> {
    let api = HidApi::new().context("无法枚举 HID 设备")?;
    Ok(api
        .device_list()
        .filter(|d| d.vendor_id() == 0x19f5 && d.product_id() == 0x1028)
        .map(|d| DeviceDescriptor {
            path: d.path().to_string_lossy().into_owned(),
            vendor_id: d.vendor_id(),
            product_id: d.product_id(),
            product: d.product_string().unwrap_or_default().to_owned(),
            bus: match d.bus_type() {
                BusType::Usb => Bus::Usb,
                BusType::Bluetooth => Bus::Bluetooth,
                _ => Bus::Other,
            },
            interface_number: d.interface_number(),
            usage_page: d.usage_page(),
            usage: d.usage(),
        })
        .collect())
}

/// 建立只读会话并读取 17 字节灯光状态。
pub fn probe_read_only() -> Result<ProbeReport> {
    let (selected, mut controller) = connect_air75()?;
    let state = controller.read_state()?;
    Ok(ProbeReport {
        descriptor: selected,
        state,
    })
}

fn exchange<T>(device: &HidDevice, request: &NuPhyRequest, key: NuPhySessionKey) -> Result<T>
where
    T: TryFrom<Vec<u8>>,
{
    send(device, &request.encode(key))?;
    let payload = validate_response(&receive(device)?, request, key)
        .map_err(|e| anyhow::anyhow!("关联响应无效: {e:?}"))?;
    payload.try_into().map_err(|_| anyhow::anyhow!("响应长度异常"))
}

fn send(device: &hidapi::HidDevice, report: &[u8; 64]) -> Result<()> {
    let mut wire = [0_u8; 65];
    wire[1..].copy_from_slice(report);
    device.write(&wire).context("HID 发送失败")?;
    Ok(())
}

fn receive(device: &hidapi::HidDevice) -> Result<[u8; 64]> {
    let mut wire = [0_u8; 65];
    let count = device
        .read_timeout(&mut wire, Duration::from_secs(1).as_millis() as i32)
        .context("HID 读取失败")?;
    match count {
        64 => Ok(wire[..64].try_into().unwrap()),
        65 => Ok(wire[1..].try_into().unwrap()),
        _ => bail!("HID 响应长度异常: {count}"),
    }
}
