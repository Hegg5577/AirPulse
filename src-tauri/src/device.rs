#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Bus {
    Usb,
    Bluetooth,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceDescriptor {
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product: String,
    pub bus: Bus,
    pub interface_number: i32,
    pub usage_page: u16,
    pub usage: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceSelectionError {
    NoVerifiedControlInterface,
    Ambiguous,
}

/// 从枚举结果中选择唯一、经过验证的 Air75 V3 USB 控制接口。
pub fn select_control_interface(
    devices: &[DeviceDescriptor],
) -> Result<&DeviceDescriptor, DeviceSelectionError> {
    let matches: Vec<_> = devices
        .iter()
        .filter(|device| {
            device.vendor_id == 0x19f5
                && device.product_id == 0x1028
                && matches!(device.product.as_str(), "Air75 V3" | "NuPhy Air75 V3")
                && device.bus == Bus::Usb
                && device.interface_number == 3
                && device.usage_page == 1
                && device.usage == 0
        })
        .collect();
    match matches.as_slice() {
        [device] => Ok(*device),
        [] => Err(DeviceSelectionError::NoVerifiedControlInterface),
        _ => Err(DeviceSelectionError::Ambiguous),
    }
}
