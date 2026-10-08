#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Air75MainState([u8; 9]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LightingError {
    InvalidBrightness,
}

impl Air75MainState {
    /// 创建 Air75 V3 稳定单色主键区状态。
    pub fn steady(brightness: u8, red: u8, green: u8, blue: u8) -> Result<Self, LightingError> {
        if brightness > 100 {
            return Err(LightingError::InvalidBrightness);
        }
        Ok(Self([3, brightness, 3, 0, 1, 0, red, green, blue]))
    }
}

/// 替换主键区状态并保留侧边灯八个字节。
pub fn merge_main_preserving_side(existing: &[u8; 17], main: &Air75MainState) -> [u8; 17] {
    let mut merged = *existing;
    merged[..9].copy_from_slice(&main.0);
    merged
}
