pub const REPORT_LEN: usize = 64;
pub const PAYLOAD_LEN: usize = 56;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NuPhySessionKey(u8);

impl NuPhySessionKey {
    /// 创建 NuPhy 临时会话密钥。
    pub const fn new(value: u8) -> Self {
        Self(value)
    }
    /// 返回会话密钥原始值。
    pub const fn value(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Read = 0xd5,
    Write = 0xd6,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NuPhyRequest {
    command: Command,
    address: u16,
    response_len: u8,
    payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    PayloadTooLarge,
    InvalidDirection,
    CommandMismatch,
    ChecksumMismatch,
    SessionMismatch,
    ResponseLengthMismatch,
    ResponseAddressMismatch,
}

impl NuPhyRequest {
    /// 创建写请求。
    pub fn write(address: u16, payload: &[u8]) -> Result<Self, ProtocolError> {
        if payload.len() > PAYLOAD_LEN {
            return Err(ProtocolError::PayloadTooLarge);
        }
        Ok(Self {
            command: Command::Write,
            address,
            response_len: payload.len() as u8,
            payload: payload.to_vec(),
        })
    }
    /// 创建读请求。
    pub fn read(address: u16, len: u8) -> Result<Self, ProtocolError> {
        if usize::from(len) > PAYLOAD_LEN {
            return Err(ProtocolError::PayloadTooLarge);
        }
        Ok(Self {
            command: Command::Read,
            address,
            response_len: len,
            payload: Vec::new(),
        })
    }
    /// 编码固定长度 HID 报文。
    pub fn encode(&self, key: NuPhySessionKey) -> [u8; REPORT_LEN] {
        let mut out = [0_u8; REPORT_LEN];
        out[0] = 0x55;
        out[1] = self.command as u8;
        out[4] = self.response_len ^ key.0;
        out[5] = self.address as u8 ^ key.0;
        out[6] = (self.address >> 8) as u8 ^ key.0;
        out[7] = key.0;
        for (target, source) in out[8..].iter_mut().zip(&self.payload) {
            *target = *source ^ key.0;
        }
        out[3] = checksum(&out);
        out
    }
}

fn checksum(report: &[u8; REPORT_LEN]) -> u8 {
    report[4..]
        .iter()
        .fold(0, |sum, byte| sum.wrapping_add(*byte))
}

/// 构造不会改变设备状态的临时会话握手请求。
pub fn build_session_request(challenge: &[u8; PAYLOAD_LEN]) -> [u8; REPORT_LEN] {
    let mut report = [0_u8; REPORT_LEN];
    report[0] = 0x55;
    report[1] = 0xee;
    report[8..].copy_from_slice(challenge);
    report[3] = checksum(&report);
    report
}

fn validate_envelope(response: &[u8; REPORT_LEN], command: u8) -> Result<(), ProtocolError> {
    if response[0] != 0xaa {
        return Err(ProtocolError::InvalidDirection);
    }
    if response[1] != command {
        return Err(ProtocolError::CommandMismatch);
    }
    if response[3] != checksum(response) {
        return Err(ProtocolError::ChecksumMismatch);
    }
    Ok(())
}

/// 验证临时会话响应。
pub fn validate_session_response(
    response: &[u8; REPORT_LEN],
    challenge: &[u8; PAYLOAD_LEN],
) -> Result<NuPhySessionKey, ProtocolError> {
    validate_envelope(response, 0xee)?;
    let key = response[4];
    if !response[4..8].iter().all(|byte| *byte == key)
        || response[8..]
            .iter()
            .zip(challenge)
            .any(|(actual, source)| *actual != (*source ^ key))
    {
        return Err(ProtocolError::SessionMismatch);
    }
    Ok(NuPhySessionKey(key))
}

/// 验证关联响应并解密载荷。
pub fn validate_response(
    response: &[u8; REPORT_LEN],
    request: &NuPhyRequest,
    key: NuPhySessionKey,
) -> Result<Vec<u8>, ProtocolError> {
    validate_envelope(response, request.command as u8)?;
    let len = response[4] ^ key.0;
    let address = u16::from(response[5] ^ key.0) | (u16::from(response[6] ^ key.0) << 8);
    if len != request.response_len {
        return Err(ProtocolError::ResponseLengthMismatch);
    }
    if address != request.address {
        return Err(ProtocolError::ResponseAddressMismatch);
    }
    if response[7] ^ key.0 != 0 {
        return Err(ProtocolError::SessionMismatch);
    }
    Ok(response[8..8 + usize::from(len)]
        .iter()
        .map(|byte| *byte ^ key.0)
        .collect())
}
