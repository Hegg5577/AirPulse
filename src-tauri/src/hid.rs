use crate::device::{Bus, DeviceDescriptor, select_control_interface};
use crate::protocol::{
    NuPhyRequest, NuPhySessionKey, build_session_request, validate_response,
    validate_session_response,
};
use anyhow::{Context, Result, bail};
use hidapi::{BusType, HidApi};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ProbeReport {
    pub descriptor: DeviceDescriptor,
    pub state: [u8; 17],
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
    let api = HidApi::new().context("无法枚举 HID 设备")?;
    let descriptors = enumerate_air75_candidates()?;
    let selected = select_control_interface(&descriptors)
        .map_err(|e| anyhow::anyhow!("控制接口选择失败: {e:?}"))?
        .clone();
    let info = api
        .device_list()
        .find(|d| d.path().to_string_lossy() == selected.path)
        .context("控制接口已断开")?;
    let device = info
        .open_device(&api)
        .context("无法打开 Air75 V3 控制接口")?;
    let challenge = std::array::from_fn(|index| (index as u8).wrapping_mul(37).wrapping_add(11));
    send(&device, &build_session_request(&challenge))?;
    let response = receive(&device)?;
    let key = validate_session_response(&response, &challenge)
        .map_err(|e| anyhow::anyhow!("会话响应无效: {e:?}"))?;
    let request = NuPhyRequest::read(0, 17).map_err(|e| anyhow::anyhow!("读取请求无效: {e:?}"))?;
    send(&device, &request.encode(key))?;
    let payload = validate_response(
        &receive(&device)?,
        &request,
        NuPhySessionKey::new(key.value()),
    )
    .map_err(|e| anyhow::anyhow!("灯光状态响应无效: {e:?}"))?;
    let state: [u8; 17] = payload
        .try_into()
        .map_err(|_| anyhow::anyhow!("灯光状态长度不是 17"))?;
    Ok(ProbeReport {
        descriptor: selected,
        state,
    })
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
