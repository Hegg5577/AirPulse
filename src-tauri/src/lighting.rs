#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Air75MainState([u8; 9]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Air75SideState([u8; 8]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LightingError {
    InvalidBrightness,
}

impl Air75MainState {
    /// 保留完整主键区协议状态，以便恢复原灯效而不重建参数。
    pub const fn from_bytes(bytes: [u8; 9]) -> Self {
        Self(bytes)
    }

    /// 创建 Air75 V3 稳定单色主键区状态。
    pub fn steady(brightness: u8, red: u8, green: u8, blue: u8) -> Result<Self, LightingError> {
        if brightness > 100 {
            return Err(LightingError::InvalidBrightness);
        }
        // 官方固定颜色使用 isRGB=0，亮度独立提交，RGB 保留原值。
        Ok(Self([3, brightness, 3, 0, 0, 0, red, green, blue]))
    }

    /// 创建实体测试使用的蓝色状态。
    pub fn blue() -> Self {
        Self::steady(30, 0, 0, 255).expect("固定亮度有效")
    }

    /// 创建实体测试使用的橙色状态。
    pub fn orange() -> Self {
        Self::steady(30, 255, 132, 0).expect("固定亮度有效")
    }

    /// 创建实体测试使用的绿色状态。
    pub fn green() -> Self {
        Self::steady(30, 0, 255, 0).expect("固定亮度有效")
    }

    /// 创建关闭主键区灯光的状态。
    pub const fn off() -> Self {
        Self([3, 0, 3, 0, 0, 0, 0, 0, 0])
    }

    /// 返回协议需要的九字节主键区状态。
    pub const fn as_bytes(&self) -> &[u8; 9] {
        &self.0
    }
}

impl Air75SideState {
    /// 从读取到的八字节侧灯状态创建可恢复值。
    pub const fn from_bytes(bytes: [u8; 8]) -> Self {
        Self(bytes)
    }

    /// 创建 Air75 V3 左右侧灯同步显示、亮度为 30% 的静态颜色。
    pub const fn steady(red: u8, green: u8, blue: u8) -> Self {
        Self([2, 30, 3, 0, 0, red, green, blue])
    }

    /// 创建蓝色静态侧灯。
    pub const fn blue() -> Self {
        Self::steady(0, 0, 255)
    }

    /// 创建橙色静态侧灯。
    pub const fn orange() -> Self {
        Self::steady(255, 132, 0)
    }

    /// 创建绿色静态侧灯。
    pub const fn green() -> Self {
        Self::steady(0, 255, 0)
    }

    /// 创建熄灭侧灯状态。
    pub const fn off() -> Self {
        Self([2, 0, 3, 0, 0, 0, 0, 0])
    }

    /// 返回协议需要的八字节侧灯状态。
    pub const fn as_bytes(&self) -> &[u8; 8] {
        &self.0
    }
}

/// 替换主键区状态并保留侧边灯八个字节。
pub fn merge_main_preserving_side(existing: &[u8; 17], main: &Air75MainState) -> [u8; 17] {
    let mut merged = *existing;
    merged[..9].copy_from_slice(&main.0);
    merged
}
