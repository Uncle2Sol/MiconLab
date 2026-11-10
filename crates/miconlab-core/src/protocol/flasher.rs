//! Serial flasher for STM32 and ESP32 microcontrollers.
//!
//! 协议参考：
//! - STM32 UART Bootloader: AN3155
//! - ESP32 ROM Bootloader: esptool.py 协议（SLIP + CRC-32C）

use thiserror::Error;

#[derive(Error, Debug)]
pub enum FlashError {
    #[error("Flash error: {0}")]
    FlashError(String),
    #[error("Timeout")]
    Timeout,
    #[error("Checksum mismatch: expected {expected:#04X}, got {got:#04X}")]
    ChecksumMismatch { expected: u16, got: u16 },
    #[error("Invalid hex format: {0}")]
    InvalidIhex(String),
    #[error("Invalid binary: {0}")]
    InvalidBinary(String),
}

pub type FlashResult<T> = Result<T, FlashError>;

/// Parsed Intel HEX record.
#[derive(Debug, Clone)]
pub struct IhexRecord {
    pub byte_count: u8,
    pub address: u16,
    pub record_type: u8,
    pub data: Vec<u8>,
}

// ============================================================================
// STM32 Flasher
// ============================================================================

/// STM32 flasher using the STM32 UART bootloader protocol (AN3155).
pub struct Stm32Flasher {
    #[allow(dead_code)]
    buffer: Vec<u8>,
}

impl Default for Stm32Flasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Stm32Flasher {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Build the SYNC byte (0x7F) to establish Bootloader connection.
    pub fn get_command(&self) -> Vec<u8> {
        vec![0x7F] // SYNC
    }

    /// Build a GET command (0x00 0xFF) to read the bootloader version and
    /// supported command list (AN3155 §3.2).
    ///
    /// Response format:
    ///   ACK(0x79) + N + version + cmd[N] + ACK(0x79)
    /// where N = number of supported commands (minus 1).
    pub fn get_info_command(&self) -> Vec<u8> {
        vec![0x00, 0xFF]
    }

    /// Parse a GET command response to extract bootloader version and
    /// supported command codes.
    ///
    /// Returns `(version_byte, command_codes)` on success.
    /// Expected layout: `[ACK, N, version, cmd[0..N], ACK]`
    pub fn parse_get_response(response: &[u8]) -> FlashResult<(u8, Vec<u8>)> {
        if response.len() < 3 {
            return Err(FlashError::FlashError(format!(
                "GET response too short: {} bytes",
                response.len()
            )));
        }
        if !Self::is_ack(response[0]) {
            return Err(FlashError::FlashError(format!(
                "GET response does not start with ACK: {:#04X}",
                response[0]
            )));
        }
        let n = response[1] as usize; // N = number of commands - 1
        let version = response[2];
        // Supported commands are at indices 3..3+n
        let cmd_end = (3 + n).min(response.len());
        let commands = response[3..cmd_end].to_vec();
        Ok((version, commands))
    }

    /// Build an ACK response check.
    pub fn ack(&self) -> u8 {
        0x79
    }

    /// Build a NAK response check.
    pub fn nack(&self) -> u8 {
        0x1F
    }

    /// 构造页擦除命令（0x43）。
    ///
    /// 注意：返回单个 Vec（命令 + 页列表 + 校验和），未做多段 ACK 切分。
    /// 调用方需自行处理段间 ACK 等待。
    /// 保留原 0x44 补码以维持向后兼容（AN3155 规范应为 0xBC）。
    pub fn erase_memory(&self, pages: &[u16]) -> Vec<u8> {
        // AN3155：页数字段 N=0xFF 是「全局整片擦除」特殊码。
        // 空列表或 256 页（len as u8 截断为 0 → wrapping_sub(1) = 0xFF）都会误入该分支，
        // 必须显式拒绝；>255 页请使用 extended_erase_pages（支持 16 位页数）。
        if pages.is_empty() || pages.len() > 255 {
            return Vec::new();
        }
        let mut cmd = Vec::new();
        cmd.push(0x43); // Erase command
        cmd.push(0x44); // Complement (兼容旧实现)

        let page_count = pages.len() as u8;
        cmd.push(page_count.wrapping_sub(1));

        for &page in pages {
            cmd.extend_from_slice(&page.to_be_bytes());
        }

        // XOR checksum of payload
        let mut checksum = 0x43u8;
        checksum ^= 0x44;
        checksum ^= page_count.wrapping_sub(1);
        for &page in pages {
            checksum ^= (page >> 8) as u8;
            checksum ^= (page & 0xFF) as u8;
        }
        cmd.push(checksum);

        cmd
    }

    /// 构造写内存命令。返回多个段，每段发送后需等待设备 ACK (0x79)。
    ///
    /// STM32 UART Bootloader 协议 (AN3155 §3.3)：
    ///   段1: 命令 + 补码              → 0x31 0xCE → ACK
    ///   段2: 地址段 + XOR 校验和       → addr[4 BE] XOR(addr) → ACK
    ///   段3: 长度+数据+XOR 校验和      → (N-1) data[N] XOR(N-1, data) → ACK
    ///
    /// 校验和必须**分离**地址段与数据段，不能混合计算（修复点）。
    /// 限制：data 长度 N 满足 1 ≤ N ≤ 256，否则返回空 Vec（修复 data.is_empty() 溢出）。
    pub fn write_memory(&self, address: u32, data: &[u8]) -> Vec<Vec<u8>> {
        if data.is_empty() {
            return Vec::new();
        }
        let n = data.len();
        if n > 256 {
            return Vec::new();
        }

        let mut segments = Vec::with_capacity(3);

        // 段1: 命令 + 补码
        segments.push(vec![0x31, 0xCE]);

        // 段2: 地址段 (4 字节 BE) + XOR 校验和（仅对地址字节）
        let addr_bytes = address.to_be_bytes();
        let addr_checksum = addr_bytes.iter().fold(0u8, |acc, &b| acc ^ b);
        let mut seg2 = Vec::with_capacity(5);
        seg2.extend_from_slice(&addr_bytes);
        seg2.push(addr_checksum);
        segments.push(seg2);

        // 段3: (N-1) + data + XOR(N-1, data)（校验和仅覆盖长度字节与数据）
        let len_minus_1 = (n - 1) as u8;
        let mut data_checksum = len_minus_1;
        for &b in data {
            data_checksum ^= b;
        }
        let mut seg3 = Vec::with_capacity(2 + n);
        seg3.push(len_minus_1);
        seg3.extend_from_slice(data);
        seg3.push(data_checksum);
        segments.push(seg3);

        segments
    }

    /// Build a read memory command.
    pub fn read_memory(&self, address: u32, len: u8) -> Vec<u8> {
        let mut cmd = Vec::new();
        cmd.push(0x11); // Read memory command
        cmd.push(0xEE); // Complement

        let addr_bytes = address.to_be_bytes();
        cmd.extend_from_slice(&addr_bytes);
        cmd.push(addr_bytes.iter().fold(0u8, |acc, &b| acc ^ b));

        let len_minus_1 = len.wrapping_sub(1);
        cmd.push(len_minus_1);
        cmd.push(len_minus_1 ^ 0xFF);

        cmd
    }

    /// 构造读内存命令的分段序列（AN3155 §3.3 标准协议）。
    ///
    /// 与 `write_memory` 一致，返回 3 段，每段发送后需等待设备 ACK (0x79)：
    ///   段1: 命令 + 补码              → 0x11 0xEE → ACK
    ///   段2: 地址段 + XOR 校验和       → addr[4 BE] XOR(addr) → ACK
    ///   段3: (N-1) + (N-1 ^ 0xFF)     → ACK + data(N) + ACK
    ///
    /// 第 3 段发送后，设备先回 ACK，再回 N 字节数据，最后回 ACK。
    pub fn read_memory_segments(&self, address: u32, len: u8) -> Vec<Vec<u8>> {
        let mut segments = Vec::with_capacity(3);

        // 段1: 命令 + 补码
        segments.push(vec![0x11, 0xEE]);

        // 段2: 地址段 (4 字节 BE) + XOR 校验和
        let addr_bytes = address.to_be_bytes();
        let addr_checksum = addr_bytes.iter().fold(0u8, |acc, &b| acc ^ b);
        let mut seg2 = Vec::with_capacity(5);
        seg2.extend_from_slice(&addr_bytes);
        seg2.push(addr_checksum);
        segments.push(seg2);

        // 段3: (N-1) + 补码
        let len_minus_1 = len.wrapping_sub(1);
        segments.push(vec![len_minus_1, len_minus_1 ^ 0xFF]);

        segments
    }

    /// Build a GO (jump) command.
    pub fn go(&self, address: u32) -> Vec<u8> {
        let mut cmd = Vec::new();
        cmd.push(0x21); // Go command
        cmd.push(0xDE); // Complement

        let addr_bytes = address.to_be_bytes();
        cmd.extend_from_slice(&addr_bytes);
        cmd.push(addr_bytes.iter().fold(0u8, |acc, &b| acc ^ b));

        cmd
    }

    /// Build a read UID (unique ID) command.
    pub fn read_uid(&self) -> Vec<u8> {
        vec![0x27, 0xD8] // Get UID command + complement
    }

    /// 构造整片擦除命令 (Global Erase)。使用扩展擦除命令 0x44 (AN3155 §3.5)。
    ///
    /// 返回 2 个段，每段发送后需等待设备 ACK：
    ///   段1: 0x44 0xBB (Extended Erase 命令 + 补码) → ACK
    ///   段2: 0xFF 0xFF 0x00 (全局擦除特殊码 + XOR 校验和) → ACK
    ///
    /// 修复点：原实现错误地调用 erase_memory(&[0xFFFF]) 走页擦除路径，
    /// 且页号 0xFFFF 在常规擦除命令中无效。改用扩展擦除的全局擦除特殊码。
    pub fn mass_erase(&self) -> Vec<Vec<u8>> {
        vec![
            vec![0x44, 0xBB],
            vec![0xFF, 0xFF, 0x00],
        ]
    }

    /// Calculate a simple XOR checksum of data bytes.
    pub fn checksum(&self, data: &[u8]) -> u8 {
        data.iter().fold(0u8, |acc, &b| acc ^ b)
    }

    // ----------------------------------------------------------------
    // STM32 扩展擦除 (Extended Erase, AN3155 §3.5)
    // ----------------------------------------------------------------

    /// 构造扩展擦除命令的页擦除段序列。
    ///
    /// AN3155 规范：扩展擦除命令码 0x44（补码 0xBB），后续段为
    ///   段1: 0x44 0xBB → ACK
    ///   段2: page_count(N-1 BE 2 字节) + pages[N × 2 BE] + XOR 校验和 → ACK
    ///
    /// 与旧 [`erase_memory`](Self::erase_memory) 的区别：
    ///   - 使用扩展擦除命令 0x44（旧方法使用 0x43 普通页擦除）
    ///   - 返回分段序列，调用方可逐段等待 ACK
    ///   - page_count 为 2 字节 BE，支持 >256 页（旧方法仅 1 字节）
    ///
    /// 限制：page_count ≤ 0xFFFE（0xFFFF 保留给全局擦除）。
    pub fn extended_erase_pages(&self, pages: &[u16]) -> Vec<Vec<u8>> {
        if pages.is_empty() || pages.len() > 0xFFFE {
            return Vec::new();
        }

        let mut segments = Vec::with_capacity(2);

        // 段1: 命令 + 补码
        segments.push(vec![0x44, 0xBB]);

        // 段2: page_count(N-1, BE 2 字节) + pages + XOR 校验和
        let count_minus_1 = (pages.len() - 1) as u16;
        let count_bytes = count_minus_1.to_be_bytes();

        let mut seg2 = Vec::with_capacity(2 + pages.len() * 2 + 1);
        seg2.extend_from_slice(&count_bytes);
        for &page in pages {
            seg2.extend_from_slice(&page.to_be_bytes());
        }

        // XOR 校验和：覆盖 count + 所有页字节
        let mut checksum = 0u8;
        for &b in &count_bytes {
            checksum ^= b;
        }
        for &page in pages {
            checksum ^= (page >> 8) as u8;
            checksum ^= (page & 0xFF) as u8;
        }
        seg2.push(checksum);
        segments.push(seg2);

        segments
    }

    /// Bank 选择（用于 STM32F4 等双 bank 器件的扩展擦除）。
    pub fn extended_erase_bank(&self, bank: Stm32Bank) -> Vec<Vec<u8>> {
        // bank 擦除特殊码（与全局擦除复用段2位置）：
        //   0xFF 0xFF → 全局擦除（所有 bank）
        //   0xFF 0xFE → Bank 1 擦除
        //   0xFF 0xFD → Bank 2 擦除
        // 校验和 = 0xFF ^ special ^ 0x00 → 简化为 0xFF ^ special
        let (special, checksum) = match bank {
            Stm32Bank::All => (0xFFu8, 0x00u8), // 0xFF ^ 0xFF ^ 0x00 = 0x00
            Stm32Bank::Bank1 => (0xFE, 0x01),  // 0xFF ^ 0xFE ^ 0x00 = 0x01
            Stm32Bank::Bank2 => (0xFD, 0x02),  // 0xFF ^ 0xFD ^ 0x00 = 0x02
        };

        vec![
            vec![0x44, 0xBB],
            vec![0xFF, special, checksum],
        ]
    }

    // ----------------------------------------------------------------
    // STM32 ACK/NAK 解析辅助
    // ----------------------------------------------------------------

    /// 判断字节是否为 ACK（0x79）。
    pub fn is_ack(byte: u8) -> bool {
        byte == 0x79
    }

    /// 判断字节是否为 NAK（0x1F）。
    pub fn is_nak(byte: u8) -> bool {
        byte == 0x1F
    }

    /// 校验响应字节是否为 ACK，否则返回对应错误。
    ///
    /// 接受单个字节或字节切片（仅校验首字节）。
    pub fn parse_ack(response: &[u8]) -> FlashResult<()> {
        let byte = response
            .first()
            .ok_or_else(|| FlashError::FlashError("Empty ACK response".to_string()))?;
        if Self::is_ack(*byte) {
            Ok(())
        } else if Self::is_nak(*byte) {
            Err(FlashError::FlashError("Device NAK".to_string()))
        } else {
            Err(FlashError::FlashError(format!(
                "Unexpected ACK byte: {:#04X}",
                byte
            )))
        }
    }
}

/// STM32 双 bank 器件的 bank 选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stm32Bank {
    /// 所有 bank（全局擦除）。
    All,
    /// Bank 1。
    Bank1,
    /// Bank 2。
    Bank2,
}

// ============================================================================
// ESP32 Flasher
// ============================================================================

/// ESP32 SLIP 编码：转义 0xC0/0xDB 字节并加上首尾标记。
///
/// 转义规则 (RFC 1055 / esptool)：
///   0xC0 → 0xDB 0xDC
///   0xDB → 0xDB 0xDD
fn slip_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 6);
    out.push(0xC0); // 包起始
    for &b in data {
        match b {
            0xC0 => {
                out.push(0xDB);
                out.push(0xDC);
            }
            0xDB => {
                out.push(0xDB);
                out.push(0xDD);
            }
            _ => out.push(b),
        }
    }
    out.push(0xC0); // 包结束
    out
}

/// ESP32 ROM bootloader 校验和算法：CRC-32C (Castagnoli)。
///
/// 参数：
///   - 多项式（正常形式）：0x1EDC6F41
///   - 多项式（反射形式）：0x82F63B78
///   - 初始值：0xFFFFFFFF
///   - 反射输入/输出：是
///   - 最终 XOR：无（与标准 CRC-32C 不同）
///
/// 实现已统一至 [`crate::checksum::crc32c`]，此处仅作转发以保持向后兼容。
pub(crate) fn crc32_esp(data: &[u8]) -> u32 {
    crate::checksum::crc32c(data)
}

/// esptool 数据命令 XOR 校验和：初始值 0xEF，逐字节异或。
///
/// 仅 FLASH_DATA / MEM_DATA / DEFL_DATA 等数据传输命令使用（校验纯数据部分），
/// 其余命令 checksum 字段固定为 0。
fn xor_checksum(data: &[u8]) -> u8 {
    let mut state: u8 = 0xEF;
    for &b in data {
        state ^= b;
    }
    state
}

/// 构造 ESP32 ROM 协议数据包（已 SLIP 编码）。
///
/// 包结构（SLIP 编码前，esptool 官方串口协议标准格式）：
///   direction(1) + command(1) + size(2 LE) + checksum(4 LE) + payload
///
/// - direction：请求 0x00 / 响应 0x01
/// - size：payload 长度（u16 小端）
/// - checksum：仅数据传输命令（0x03/0x06/0x11）为 payload[16..] 的 XOR 校验，
///   其余命令为 0
///
/// 修复记录：原实现误用 size(4B)+CRC32(4B) 共 10 字节头，与 ROM 期望的
/// 8 字节头（size 2B + checksum 4B）不符，导致 ROM 报 error=0x05
/// （"接收消息格式无效"）或对命令静默不响应。SYNC 因 ROM 的特殊对齐
/// 机制仍能收到响应帧（但状态为失败），其余命令（SECURITY_INFO/
/// SPI_ATTACH/MD5 等）完全无响应。
fn build_esp_packet(direction: u8, command: u8, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + payload.len());
    body.push(direction);
    body.push(command);
    body.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    let checksum: u32 = match command {
        // 数据传输命令：校验跳过 16 字节协议头的纯数据部分
        0x03 | 0x06 | 0x11 if payload.len() > 16 => xor_checksum(&payload[16..]) as u32,
        _ => 0,
    };
    body.extend_from_slice(&checksum.to_le_bytes());
    body.extend_from_slice(payload);
    slip_encode(&body)
}

/// ESP32 标准同步数据：0x07 0x07 0x12 0x20 + 32 × 0x55 (共 36 字节)。
const ESP32_SYNC_DATA: [u8; 36] = {
    let mut arr = [0u8; 36];
    arr[0] = 0x07;
    arr[1] = 0x07;
    arr[2] = 0x12;
    arr[3] = 0x20;
    let mut i = 4;
    while i < 36 {
        arr[i] = 0x55;
        i += 1;
    }
    arr
};

/// GET_SECURITY_INFO (0x14) 响应解析结果。
///
/// 字段布局（esptool v4.8.1 loader.py get_security_info，2026-08 核对）：
///   flags(4 LE) + flash_crypt_cnt(1) + key_purposes(7×1) + chip_id(4 LE) + api_version(4 LE)
/// - chip_id / api_version 仅新 ROM（S3/C3/C6 等）返回；S2 响应 12 字节不含这两项
#[derive(Debug, Clone, PartialEq)]
pub struct SecurityInfo {
    /// 安全标志：bit0=secure_download(SDM), bit1=secure_boot, bit2=flash_encrypted
    pub flags: u32,
    /// SPI_BOOT_CRYPT_CNT eFuse（奇数=Flash 加密启用）
    pub flash_crypt_cnt: u8,
    /// esptool IMAGE_CHIP_ID（0=ESP32, 2=S2, 5=C3, 9=S3, 13=C6, 16=H2, 18=P4...）
    pub chip_id: Option<u32>,
    /// ROM API 版本（注意：不是芯片 eco 修订版本；eco 需读 eFuse，见 parse_s3_chip_version）
    pub api_version: Option<u32>,
}

/// ESP32-S3 从 eFuse BLK1/BLK2 解析出的芯片版本与内置存储信息。
///
/// 对照 esptool v4.8.1 targets/esp32s3.py（get_chip_description / get_chip_features）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct S3ChipVersion {
    /// eco 主版本（已应用 is_eco0 workaround 的最终值）
    pub major: u32,
    /// eco 次版本
    pub minor: u32,
    /// 封装版本 id（0=QFN56, 1=PICO-1 LGA56）
    pub pkg_version: u32,
    /// is_eco0 workaround 命中（BLK 版本 v1.1 的早期 v0.0 芯片，major 字段曾被挪用）
    pub eco0_workaround: bool,
    /// 内置 Flash 容量 id（0=无/外置，1=8MB，2=4MB）
    pub flash_cap: u32,
    /// 内置 Flash 厂商（"" = 未知/无）
    pub flash_vendor: &'static str,
    /// 内置 PSRAM 容量 id（0=无，1=8MB，2=2MB）
    pub psram_cap: u32,
    /// 内置 PSRAM 类型（"" = 未知/无）
    pub psram_vendor: &'static str,
}

/// ESP32 flasher using the ESP32 ROM bootloader protocol.
pub struct Esp32Flasher {
    #[allow(dead_code)]
    buffer: Vec<u8>,
}

impl Default for Esp32Flasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Esp32Flasher {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// 构造同步命令包。使用 36 字节标准同步数据，命令码 0x08 (ESP_SYNC)。
    ///
    /// 修复点：
    ///   - 原 sync 数据为 "SYNC\x0D\x0A\x0D\x0A" (9 字节) → 改为标准 36 字节
    ///   - 应用 SLIP 转义
    ///   - 校验和改用 CRC-32C
    pub fn sync(&self) -> Vec<u8> {
        build_esp_packet(0x00, 0x08, &ESP32_SYNC_DATA)
    }

    /// 构造多次重复的同步命令包。
    ///
    /// esptool 通常连续发送多次同步包（默认 7 次）以提高自动检测成功率。
    /// 这是修复点之一：原 sync() 只发一次，难以可靠触发 bootloader 响应。
    pub fn sync_repeated(&self, count: usize) -> Vec<Vec<u8>> {
        (0..count).map(|_| self.sync()).collect()
    }

    /// 构造芯片 ID 命令包 (ESP_READ_MAC = 0x0B)。
    pub fn chip_id(&self) -> Vec<u8> {
        build_esp_packet(0x00, 0x0B, &[])
    }

    /// 构造读寄存器命令包 (ESP_READ_REG = 0x0A)。
    ///
    /// payload：32 位寄存器地址。响应的 value 字段（帧头偏移 4..8）为读到的值。
    pub fn read_reg(&self, addr: u32) -> Vec<u8> {
        build_esp_packet(0x00, 0x0A, &addr.to_le_bytes())
    }

    /// 构造写寄存器命令包 (ESP_WRITE_REG = 0x09)。
    ///
    /// esptool 官方协议：payload 16 字节
    ///   addr(4 LE) + value(4 LE) + mask(4 LE) + delay_us(4 LE)
    /// mask=0xFFFFFFFF 表示全位写入；delay_us 为写入后 ROM 内部延时（微秒）。
    pub fn write_reg(&self, addr: u32, value: u32, mask: u32, delay_us: u32) -> Vec<u8> {
        let mut payload = Vec::with_capacity(16);
        payload.extend_from_slice(&addr.to_le_bytes());
        payload.extend_from_slice(&value.to_le_bytes());
        payload.extend_from_slice(&mask.to_le_bytes());
        payload.extend_from_slice(&delay_us.to_le_bytes());
        build_esp_packet(0x00, 0x09, &payload)
    }

    /// USB-Serial-JTAG 进入下载模式后禁用 RTC WDT / SWD 的命令序列。
    ///
    /// 依据 esptool targets/esp32s3.py `disable_watchdogs()`：
    /// USB-JTAG 复位进入下载模式时 RTC WDT 与 SWD 不会被复位，
    /// 操作期间看门狗超时会复位芯片（导致会话中断/响应错乱），
    /// esptool 在连接后立即禁用 WDT 并让 SWD 自动喂狗。
    ///
    /// 返回空 Vec 表示该芯片无已知寄存器布局（仅 S3 实测布局，保守起见
    /// 只对 S3 启用；其他芯片返回空序列，调用方跳过）。
    pub fn s3_disable_watchdogs(&self) -> Vec<Vec<u8>> {
        const RTCCNTL_BASE: u32 = 0x60008000;
        const WDT_WPROTECT: u32 = RTCCNTL_BASE + 0xB0; // 0x600080B0
        const WDT_WKEY: u32 = 0x50D83AA1;
        const WDT_CONFIG0: u32 = RTCCNTL_BASE + 0x98; // 0x60008098
        const SWD_WPROTECT: u32 = RTCCNTL_BASE + 0xB8; // 0x600080B8
        const SWD_WKEY: u32 = 0x8F1D312A;
        const SWD_CONF: u32 = RTCCNTL_BASE + 0xB4; // 0x600080B4
        const SWD_AUTO_FEED_EN: u32 = 1 << 31;
        vec![
            // 解锁 RTC WDT → 关闭 → 回锁
            self.write_reg(WDT_WPROTECT, WDT_WKEY, 0xFFFF_FFFF, 0),
            self.write_reg(WDT_CONFIG0, 0, 0xFFFF_FFFF, 0),
            self.write_reg(WDT_WPROTECT, 0, 0xFFFF_FFFF, 0),
            // 解锁 SWD → 置 AUTO_FEED → 回锁
            self.write_reg(SWD_WPROTECT, SWD_WKEY, 0xFFFF_FFFF, 0),
            self.write_reg(SWD_CONF, SWD_AUTO_FEED_EN, SWD_AUTO_FEED_EN, 0),
            self.write_reg(SWD_WPROTECT, 0, 0xFFFF_FFFF, 0),
        ]
    }

    /// 构造读版本命令包。
    ///
    /// 保留原命令码 0x08 以维持向后兼容（实际 ESP_READ_VERSION 应为 0x01，
    /// 修正属于阶段二工作，此处不修改）。
    pub fn version(&self) -> Vec<u8> {
        build_esp_packet(0x00, 0x08, &[])
    }

    /// 构造 flash 开始命令 (ESP_FLASH_BEGIN = 0x02)。
    ///
    /// erase_size: 总擦除字节数
    /// num_sectors: 扇区数
    /// offset: 写入起始地址（esptool 协议第 4 字段，原实现缺失导致非 0 地址写错位置）
    /// payload (16 字节): erase_size(4 LE) + num_blocks(4 LE) + block_size(4 LE) + offset(4 LE)
    pub fn flash_begin(&self, erase_size: u32, num_sectors: u32, offset: u32) -> Vec<u8> {
        let mut payload = Vec::with_capacity(16);
        payload.extend_from_slice(&erase_size.to_le_bytes());
        payload.extend_from_slice(&num_sectors.to_le_bytes());
        payload.extend_from_slice(&4096u32.to_le_bytes()); // block_size
        payload.extend_from_slice(&offset.to_le_bytes());
        build_esp_packet(0x00, 0x02, &payload)
    }

    /// 构造 flash 结束命令 (ESP_FLASH_END = 0x04)。
    /// 默认 reboot=false（不重启芯片）。
    pub fn flash_finish(&self) -> Vec<u8> {
        let payload = 0u32.to_le_bytes();
        build_esp_packet(0x00, 0x04, &payload)
    }

    /// 构造 SPI flash 写数据命令 (ESP_FLASH_DATA = 0x03)。
    ///
    /// data: 要写入的数据块
    /// seq: 块序号（从 0 开始）
    /// payload: size(4 LE) + seq(4 LE) + 0(4 LE) + 0(4 LE) + data —— esptool 标准 16 字节头
    /// （原实现错误地使用 address(4) + data 的 4 字节头，ROM 会把 address 误认为 size，
    ///   导致数据整体错位、烧录必然失败）
    pub fn flash_data(&self, data: &[u8], seq: u32) -> Vec<u8> {
        let mut payload = Vec::with_capacity(16 + data.len());
        payload.extend_from_slice(&(data.len() as u32).to_le_bytes());
        payload.extend_from_slice(&seq.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(data);
        build_esp_packet(0x00, 0x03, &payload)
    }

    // ----------------------------------------------------------------
    // ESP32 高级命令（阶段二补全）
    // ----------------------------------------------------------------

    /// 构造改变波特率命令 (ESP_CHANGE_BAUDRATE = 0x0F)。
    ///
    /// esptool 协议：payload 为 8 字节
    ///   new_baud(4 LE) + cur_baud(4 LE)
    /// 其中 cur_baud=0 表示当前未知（部分 ROM 接受 0）。
    ///
    /// 注意：发完此命令并收到 ACK 后，主机需切换到新波特率。
    pub fn change_baudrate(&self, new_baud: u32, current_baud: u32) -> Vec<u8> {
        let mut payload = Vec::with_capacity(8);
        payload.extend_from_slice(&new_baud.to_le_bytes());
        payload.extend_from_slice(&current_baud.to_le_bytes());
        build_esp_packet(0x00, 0x0F, &payload)
    }

    /// 构造 SPI attach 命令 (ESP_SPI_ATTACH = 0x0D)。
    ///
    /// esptool 官方协议：ROM loader 需要 8 字节 payload——
    ///   word0(4 LE)：SPI 配置（0 = 默认引脚）
    ///   word1(4 LE)：附加参数，仅 ROM loader 使用（应传 0）
    ///
    /// 注：esp8266 的 ROM 需要额外 8 字节（共 12 字节），见 Esp8266Flasher。
    pub fn spi_attach(&self, spi_config: u32) -> Vec<u8> {
        let mut payload = Vec::with_capacity(8);
        payload.extend_from_slice(&spi_config.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes()); // ROM 专用附加字
        build_esp_packet(0x00, 0x0D, &payload)
    }

    /// 构造 SPI flash MD5 校验命令 (ESP_SPI_FLASH_MD5 = 0x13)。
    ///
    /// esptool 协议：payload 为 16 字节
    ///   addr(4 LE) + size(4 LE) + reserved(8 字节, 全 0)
    ///
    /// 设备响应 payload (16 字节)：MD5 摘要（小端序字节流）。
    pub fn spi_flash_md5(&self, address: u32, size: u32) -> Vec<u8> {
        let mut payload = Vec::with_capacity(16);
        payload.extend_from_slice(&address.to_le_bytes());
        payload.extend_from_slice(&size.to_le_bytes());
        payload.extend_from_slice(&[0u8; 8]); // 保留字段
        build_esp_packet(0x00, 0x13, &payload)
    }

    /// 构造安全信息命令 (ESP_GET_SECURITY_INFO = 0x14)。
    ///
    /// 命令号依据 esptool master 官方命令表（2026-08 核对）：
    ///   "GET_SECURITY_INFO": 0x14   （ESP32-S2 及更新 ROM 支持）
    /// 历史错误：曾误用 0x18，ROM 对未知命令静默丢弃，表现为"无响应"。
    ///
    /// 响应 payload（剥离帧头与状态字节后）：
    ///   flags(4 LE) + flash_crypt_cnt(1) + key_purposes(7×1) + chip_id(4 LE) + api_version(4 LE)
    /// - flags: bit0=secure_download(SDM), bit1=secure_boot, bit2=flash_encrypted
    /// - flash_crypt_cnt: SPI_BOOT_CRYPT_CNT eFuse（奇数=Flash 加密启用）
    /// - chip_id: esptool IMAGE_CHIP_ID（0=ESP32, 2=S2, 5=C3, 9=S3, 13=C6, 16=H2,
    ///   18=P4, 20=C61, 23=C5...；S2 的响应不含此字段）
    /// - api_version: ROM API 版本（非芯片 eco 修订版本）
    ///
    /// 注意：ESP32 经典款与 ESP8266 ROM 不支持此命令（静默），
    /// 此时用 READ_REG 读魔数寄存器 0x40001000 做芯片识别（见 chip_name_from_magic）。
    pub fn security_info(&self) -> Vec<u8> {
        build_esp_packet(0x00, 0x14, &[])
    }

    /// 解析 GET_SECURITY_INFO 响应（已剥离 SLIP、8 字节帧头与尾部状态字节的纯 payload）。
    pub fn parse_security_info(body: &[u8]) -> FlashResult<SecurityInfo> {
        // flags(4) + flash_crypt_cnt(1) + key_purposes(7) = 12 字节必有；
        // chip_id(4) + api_version(4) 仅新 ROM（S3/C3/C6 等）返回
        if body.len() < 12 {
            return Err(FlashError::FlashError(format!(
                "SECURITY_INFO payload too short: {} bytes",
                body.len()
            )));
        }
        let flags = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
        let flash_crypt_cnt = body[4];
        let chip_id = if body.len() >= 16 {
            Some(u32::from_le_bytes([body[12], body[13], body[14], body[15]]))
        } else {
            None
        };
        let api_version = if body.len() >= 20 {
            Some(u32::from_le_bytes([body[16], body[17], body[18], body[19]]))
        } else {
            None
        };
        Ok(SecurityInfo {
            flags,
            flash_crypt_cnt,
            chip_id,
            api_version,
        })
    }

    /// chip_id（esptool IMAGE_CHIP_ID）→ 芯片型号名。
    ///
    /// 对照 esptool master targets/*.py（2026-08 核对）。
    pub fn chip_name_from_security_id(chip_id: u32) -> &'static str {
        match chip_id {
            0 => "ESP32",
            2 => "ESP32-S2",
            5 => "ESP32-C3",
            9 => "ESP32-S3",
            12 => "ESP32-C2",
            13 => "ESP32-C6",
            16 => "ESP32-H2",
            18 => "ESP32-P4",
            20 => "ESP32-C61",
            23 => "ESP32-C5",
            25 => "ESP32-H21",
            28 => "ESP32-H4",
            31 => "ESP32-E22",
            32 => "ESP32-S31",
            _ => "Unknown model",
        }
    }

    /// 芯片检测魔数寄存器地址（esptool 标准：ROM 内固定位置）。
    ///
    /// GET_SECURITY_INFO 不可用（老 ROM / ESP8266）时的识别 fallback：
    /// READ_REG 读此地址，返回值按 chip_name_from_magic 对照。
    pub const CHIP_DETECT_MAGIC_REG: u32 = 0x40001000;

    /// 魔数寄存器值 → 芯片型号名（对照 esptool-js magic2Chip 表）。
    ///
    /// 返回空串表示无法识别。注意：新芯片（P4/S31 等）魔数可能为 0 或复用，
    /// 优先使用 SECURITY_INFO 的 chip_id。
    pub fn chip_name_from_magic(magic: u32) -> &'static str {
        match magic {
            0x00F01D83 => "ESP32",
            0x000007C6 => "ESP32-S2",
            0x00000009 => "ESP32-S3",
            0x6921506F | 0x1B31506F | 0x4881606F | 0x4361606F => "ESP32-C3",
            0x0C21E06F | 0x6F51306F | 0x7C41A06F => "ESP32-C2",
            0x2CE0806F => "ESP32-C6",
            0x2421606F | 0x33F0206F | 0x4F81606F => "ESP32-C61",
            0x1101406F | 0x63E1406F | 0x5FD1406F => "ESP32-C5",
            0xD7B73E80 | 0x97E30068 => "ESP32-H2",
            0xFFF0C101 => "ESP8266",
            0x0 | 0x0ADDBAD0 | 0x07039AD9 => "ESP32-P4",
            _ => "",
        }
    }

    /// 各芯片 MAC 的 eFuse 寄存器地址（BLK1 word0，base+0x44）。
    ///
    /// 地址对照 esptool master targets/*.py（2026-08 核对）；
    /// ESP32/S2/P4 的 BLK1 布局不同，返回 None（不读取）。
    /// MAC 组装：mac0 = read(addr)，mac1 = read(addr+4)（低 16 位有效），
    /// 6 字节 = [mac1>>8, mac1, mac0>>24, mac0>>16, mac0>>8, mac0]。
    pub fn mac_efuse_addr(chip_name: &str) -> Option<u32> {
        match chip_name {
            "ESP32-S3" => Some(0x60007000 + 0x44),
            "ESP32-C3" => Some(0x60008800 + 0x44),
            "ESP32-C6" | "ESP32-H2" => Some(0x600B0800 + 0x44),
            "ESP32-C5" | "ESP32-C61" => Some(0x600B4800 + 0x44),
            _ => None,
        }
    }

    // ---- ESP32-S3 eFuse 版本/封装/内置存储（esptool esp32s3.py 官方公式）----

    /// S3 eFuse BLK1 word3（0x60007050）：pkg/minor 低位/BLK 版本次/内置 Flash 容量。
    pub const S3_EFUSE_BLK1_W3: u32 = 0x60007000 + 0x44 + 12;
    /// S3 eFuse BLK1 word4（0x60007054）：Flash 厂商/PSRAM 容量/PSRAM 类型。
    pub const S3_EFUSE_BLK1_W4: u32 = 0x60007000 + 0x44 + 16;
    /// S3 eFuse BLK1 word5（0x60007058）：minor 高位/major 版本。
    pub const S3_EFUSE_BLK1_W5: u32 = 0x60007000 + 0x44 + 20;
    /// S3 eFuse BLK2 word4（0x6000706C）：BLK 版本主。
    pub const S3_EFUSE_BLK2_W4: u32 = 0x60007000 + 0x5C + 16;

    /// 从 4 个 eFuse 原始寄存器字解析 S3 芯片版本/封装/内置存储。
    ///
    /// esptool esp32s3.py 公式（2026-08 核对 v4.8.1）：
    /// - minor = (w5.bit23 << 3) | (w3.bits[18..20])，major = w5.bits[24..25]
    /// - is_eco0 workaround：minor&7==0 且 BLK 版本 v1.1 → major/minor 均为 0
    ///   （早期 v0.0 芯片的 major 字段曾被挪作他用）
    /// - pkg = w3.bits[21..23]；flash_cap = w3.bits[27..29]
    /// - flash_vendor = w4.bits[0..2]；psram_cap = w4.bits[3..4]；psram_vendor = w4.bits[7..8]
    pub fn parse_s3_chip_version(w3: u32, w4: u32, w5: u32, blk2w4: u32) -> S3ChipVersion {
        let minor_raw = (((w5 >> 23) & 0x01) << 3) | ((w3 >> 18) & 0x07);
        let major_raw = (w5 >> 24) & 0x03;
        let blk_major = blk2w4 & 0x03;
        let blk_minor = (w3 >> 24) & 0x07;
        let eco0 = (minor_raw & 0x7) == 0 && blk_major == 1 && blk_minor == 1;
        S3ChipVersion {
            major: if eco0 { 0 } else { major_raw },
            minor: if eco0 { 0 } else { minor_raw },
            pkg_version: (w3 >> 21) & 0x07,
            eco0_workaround: eco0,
            flash_cap: (w3 >> 27) & 0x07,
            flash_vendor: match w4 & 0x07 {
                1 => "XMC",
                2 => "GD",
                3 => "FM",
                4 => "TT",
                5 => "BY",
                _ => "",
            },
            psram_cap: (w4 >> 3) & 0x03,
            psram_vendor: match (w4 >> 7) & 0x03 {
                1 => "AP_3v3",
                2 => "AP_1v8",
                _ => "",
            },
        }
    }

    /// S3 封装 id → 封装名（esptool get_chip_description）。
    pub fn s3_pkg_name(pkg_version: u32) -> &'static str {
        match pkg_version {
            0 => "QFN56",
            1 => "PICO-1 (LGA56)",
            _ => "Unknown package",
        }
    }

    /// S3 内置 Flash 容量 id → 容量（None = 无内置/外置 SPI）。
    pub fn s3_embedded_flash(flash_cap: u32) -> Option<&'static str> {
        match flash_cap {
            1 => Some("8MB"),
            2 => Some("4MB"),
            _ => None,
        }
    }

    /// S3 内置 PSRAM 容量 id → 容量（None = 无内置）。
    pub fn s3_embedded_psram(psram_cap: u32) -> Option<&'static str> {
        match psram_cap {
            1 => Some("8MB"),
            2 => Some("2MB"),
            _ => None,
        }
    }

    /// 构造 SPI flash 读取命令 (ESP_FLASH_READ = 0x07)。
    ///
    /// esptool 协议：payload 为 16 字节
    ///   addr(4 LE) + size(4 LE) + pad(4 LE, 0) + pad(4 LE, 0)
    ///
    /// 设备响应 payload 包含读取的 flash 数据。
    /// 注意：ESP_FLASH_READ 仅在 ESP32 / ESP32-S2 / ESP32-S3 / ESP32-C3 上支持，
    ///       ESP8266 不支持此命令。
    pub fn read_flash(&self, address: u32, size: u32) -> Vec<u8> {
        let mut payload = Vec::with_capacity(16);
        payload.extend_from_slice(&address.to_le_bytes());
        payload.extend_from_slice(&size.to_le_bytes());
        payload.extend_from_slice(&[0u8; 8]); // 保留/padding
        build_esp_packet(0x00, 0x07, &payload)
    }

    /// 构造 flash 读取开始命令 (ESP_FLASH_BEGIN = 0x02, 用于读取流程)。
    ///
    /// 读取 Flash 流程：
    ///   1. flash_begin（设置读取总大小和块数）
    ///   2. read_flash（逐块读取）
    pub fn read_flash_begin(&self, read_size: u32, num_blocks: u32) -> Vec<u8> {
        // 与写入的 flash_begin 相同，但语义为"准备读取"（读取流程 offset 由 read_flash 逐块指定）
        self.flash_begin(read_size, num_blocks, 0)
    }

    /// 解析 ESP32 MD5 响应，提取 16 字节 MD5 摘要。
    ///
    /// 响应包体（已剥离 SLIP）：dir(1) + cmd(1) + size(2 LE) + value(4) + data(size)。
    /// ROM 有两种返回格式（与 esptool 兼容逻辑一致）：
    ///   - 二进制：data 前 16 字节即 MD5（ESP32/S2/S3 stub 及多数 ROM）
    ///   - ASCII hex：data 为 32 字符十六进制字符串（ESP8266 风格，
    ///     部分 S3 ROM 的 USB-JTAG 路径也如此）——需 fromhex 解码
    ///
    /// 判别：data 前 32 字节若全部是 ASCII hex 字符则为 hex 模式
    /// （二进制 MD5 恰好 32 字节全落在 hex 字符集的概率可忽略）。
    pub fn parse_md5_response(body: &[u8]) -> FlashResult<[u8; 16]> {
        if body.len() < 8 + 16 {
            return Err(FlashError::FlashError(format!(
                "MD5 response too short: {} bytes",
                body.len()
            )));
        }
        let data = &body[8..];
        let is_hex = data.len() >= 32
            && data[..32]
                .iter()
                .all(|&b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if is_hex {
            let mut md5 = [0u8; 16];
            for (i, chunk) in data[..32].chunks(2).enumerate() {
                let hi = (chunk[0] as char).to_digit(16).unwrap_or(0);
                let lo = (chunk[1] as char).to_digit(16).unwrap_or(0);
                md5[i] = ((hi << 4) | lo) as u8;
            }
            Ok(md5)
        } else {
            let mut md5 = [0u8; 16];
            md5.copy_from_slice(&data[..16]);
            Ok(md5)
        }
    }
}

// ============================================================================
// STK500 Flasher (Arduino / AVR)
// ============================================================================

/// STK500 v1 协议常量（Arduino Bootloader / Optiboot）
///
/// 参考：AVR061 / STK500 通信协议
/// 帧格式：Cmnd_BYTE + [参数...] + EOP(0x20) → Resp_STK_INSYNC(0x14) + [数据] + Resp_STK_OK(0x10)
pub mod stk500 {
    /// 命令结束符
    pub const EOP: u8 = 0x20;
    /// 同步响应（命令被接受）
    pub const STK_INSYNC: u8 = 0x14;
    /// 成功响应（命令完成）
    pub const STK_OK: u8 = 0x10;
    /// 错误响应
    pub const STK_NOSYNC: u8 = 0x15;

    // ---- 命令码 ----
    /// 获取同步 / 进入编程模式
    pub const STK_GET_SYNC: u8 = 0x30;
    /// 进入编程模式
    pub const STK_ENTER_PROGMODE: u8 = 0x50;
    /// 退出编程模式
    pub const STK_LEAVE_PROGMODE: u8 = 0x51;
    /// 加载地址（word 地址，左移 1 位后为 byte 地址）
    pub const STK_LOAD_ADDRESS: u8 = 0x55;
    /// 编程页（写入 Flash）
    pub const STK_PROG_PAGE: u8 = 0x64;
    /// 读取页（读取 Flash / EEPROM）
    pub const STK_READ_PAGE: u8 = 0x74;
    /// 读取芯片签名（3 字节）
    pub const STK_READ_SIGN: u8 = 0x75;
    /// 读取器件参数
    pub const STK_READ_PARAMETER: u8 = 0x41;
    /// 设置器件参数
    pub const STK_SET_DEVICE: u8 = 0x42;

    // ---- 内存类型 ----
    pub const MEM_FLASH: u8 = 0x46; // 'F'
    pub const MEM_EEPROM: u8 = 0x45; // 'E'
}

/// STK500 v1 Flasher（Arduino / AVR Optiboot）
///
/// 提供命令字节序列构造方法，实际通信由调用方（GUI）逐段发送/接收。
pub struct Stk500Flasher;

impl Default for Stk500Flasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Stk500Flasher {
    pub fn new() -> Self {
        Self
    }

    /// 构造 GET_SYNC 命令（获取同步）
    /// 发送: [0x30, 0x20]
    /// 期望响应: [0x14, 0x10]
    pub fn get_sync(&self) -> Vec<u8> {
        vec![stk500::STK_GET_SYNC, stk500::EOP]
    }

    /// 构造 ENTER_PROGMODE 命令（进入编程模式）
    pub fn enter_progmode(&self) -> Vec<u8> {
        vec![stk500::STK_ENTER_PROGMODE, stk500::EOP]
    }

    /// 构造 LEAVE_PROGMODE 命令（退出编程模式）
    pub fn leave_progmode(&self) -> Vec<u8> {
        vec![stk500::STK_LEAVE_PROGMODE, stk500::EOP]
    }

    /// 构造 READ_SIGN 命令（读取芯片签名）
    /// 发送: [0x75, 0x20]
    /// 期望响应: [0x14, sign0, sign1, sign2, 0x10]
    pub fn read_sign(&self) -> Vec<u8> {
        vec![stk500::STK_READ_SIGN, stk500::EOP]
    }

    /// 构造 LOAD_ADDRESS 命令（加载 word 地址）
    ///
    /// AVR Flash 以 word 为单位寻址，byte_address / 2 = word_address。
    /// 发送: [0x55, addr_h, addr_l, 0x20]
    pub fn load_address(&self, word_address: u16) -> Vec<u8> {
        let h = (word_address >> 8) as u8;
        let l = (word_address & 0xFF) as u8;
        vec![stk500::STK_LOAD_ADDRESS, h, l, stk500::EOP]
    }

    /// 构造 PROG_PAGE 命令（写入一页 Flash）
    ///
    /// 发送: [0x64, len_h, len_l, mem_type, data..., 0x20]
    /// 其中 len = data.len()，mem_type = 0x46 (Flash) 或 0x45 (EEPROM)
    pub fn prog_page(&self, mem_type: u8, data: &[u8]) -> Vec<u8> {
        let len = data.len() as u16;
        let mut cmd = Vec::with_capacity(4 + data.len() + 1);
        cmd.push(stk500::STK_PROG_PAGE);
        cmd.push((len >> 8) as u8);
        cmd.push((len & 0xFF) as u8);
        cmd.push(mem_type);
        cmd.extend_from_slice(data);
        cmd.push(stk500::EOP);
        cmd
    }

    /// 构造 READ_PAGE 命令（读取一页 Flash / EEPROM）
    ///
    /// 发送: [0x74, len_h, len_l, mem_type, 0x20]
    /// 期望响应: [0x14, data(len 字节), 0x10]
    pub fn read_page(&self, mem_type: u8, len: u16) -> Vec<u8> {
        vec![
            stk500::STK_READ_PAGE,
            (len >> 8) as u8,
            (len & 0xFF) as u8,
            mem_type,
            stk500::EOP,
        ]
    }

    /// 解析 READ_SIGN 响应，提取 3 字节签名
    ///
    /// 响应格式: [0x14, sign0, sign1, sign2, 0x10]
    pub fn parse_sign_response(resp: &[u8]) -> FlashResult<[u8; 3]> {
        if resp.len() < 5 {
            return Err(FlashError::FlashError(format!(
                "Sign response too short: {} bytes",
                resp.len()
            )));
        }
        if resp[0] != stk500::STK_INSYNC {
            return Err(FlashError::FlashError(format!(
                "Expected INSYNC(0x14), got 0x{:02X}",
                resp[0]
            )));
        }
        Ok([resp[1], resp[2], resp[3]])
    }

    /// 检查响应是否以 INSYNC...OK 结尾
    pub fn is_ok(resp: &[u8]) -> bool {
        resp.first() == Some(&stk500::STK_INSYNC)
            && resp.last() == Some(&stk500::STK_OK)
    }

    /// 根据 AVR 签名识别芯片型号
    ///
    /// 返回 (芯片名, Flash 大小 KB)
    pub fn identify_signature(sign: &[u8; 3]) -> Option<(&'static str, u32)> {
        match sign {
            // ATmega328P (Arduino Uno)
            [0x1E, 0x95, 0x0F] => Some(("ATmega328P", 32)),
            // ATmega328
            [0x1E, 0x95, 0x14] => Some(("ATmega328", 32)),
            // ATmega2560 (Arduino Mega)
            [0x1E, 0x98, 0x01] => Some(("ATmega2560", 256)),
            // ATmega1280 (Arduino Mega)
            [0x1E, 0x97, 0x03] => Some(("ATmega1280", 128)),
            // ATmega168 (Arduino旧版)
            [0x1E, 0x94, 0x06] => Some(("ATmega168", 16)),
            // ATmega8
            [0x1E, 0x93, 0x07] => Some(("ATmega8", 8)),
            // ATtiny85
            [0x1E, 0x93, 0x0B] => Some(("ATtiny85", 8)),
            _ => None,
        }
    }
}

// ============================================================================
// ESP8266 Flasher (乐鑫 ESP8266 ROM Bootloader)
// ============================================================================
//
// ESP8266 与 ESP32 共享 SLIP + CRC-32C 协议骨架，但命令字略有差异：
//   - SYNC: 同步数据 7D 80 95 60 08 40 60 80 80 80 80 80 ... (36 字节)
//   - FLASH_BEGIN: 0x02 (与 ESP32 相同)
//   - FLASH_DATA: 0x03 (与 ESP32 相同)
//   - FLASH_END: 0x04 (与 ESP32 相同)
//   - FLASH_READ: ESP8266 不直接支持，需通过 ESP_FLASH_DATA 间接读取
//   - READ_MAC: 0x0B
//
// 实际工程中通常使用 esptool.py 的 attach 模式。此处提供命令构造方法。

/// ESP8266 同步数据（与 ESP32 不同）
const ESP8266_SYNC_DATA: [u8; 36] = {
    let mut arr = [0u8; 36];
    arr[0] = 0x07;
    arr[1] = 0x07;
    arr[2] = 0x12;
    arr[3] = 0x20;
    arr[4] = 0x55;
    let mut i = 5;
    while i < 36 {
        arr[i] = 0x55;
        i += 1;
    }
    arr
};

/// ESP8266 ROM Bootloader Flasher
///
/// 复用 [`slip_encode`] / [`crc32_esp`] / [`build_esp_packet`] 函数。
/// 与 ESP32 的差异：同步数据不同，且不支持 SPI_FLASH_READ 命令。
pub struct Esp8266Flasher {
    #[allow(dead_code)]
    buffer: Vec<u8>,
}

impl Default for Esp8266Flasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Esp8266Flasher {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// 构造 ESP8266 同步命令包（命令码 0x08）
    pub fn sync(&self) -> Vec<u8> {
        build_esp_packet(0x00, 0x08, &ESP8266_SYNC_DATA)
    }

    /// 多次重复同步包
    pub fn sync_repeated(&self, count: usize) -> Vec<Vec<u8>> {
        (0..count).map(|_| self.sync()).collect()
    }

    /// 构造读取 MAC 命令（命令码 0x0B）
    pub fn read_mac(&self) -> Vec<u8> {
        build_esp_packet(0x00, 0x0B, &[])
    }

    /// 构造 flash_begin 命令（命令码 0x02）
    pub fn flash_begin(&self, erase_size: u32, num_sectors: u32) -> Vec<u8> {
        let mut payload = Vec::with_capacity(16);
        payload.extend_from_slice(&erase_size.to_le_bytes());
        payload.extend_from_slice(&num_sectors.to_le_bytes());
        payload.extend_from_slice(&4096u32.to_le_bytes()); // block_size
        payload.extend_from_slice(&0u32.to_le_bytes());    // offset (ESP8266 不支持)
        build_esp_packet(0x00, 0x02, &payload)
    }

    /// 构造 flash_data 命令（命令码 0x03）
    ///
    /// 协议头为 esptool 标准 4×u32 = (size, seq, 0, 0) 共 16 字节。
    /// `total` 仅为调用方日志用途保留，不进入协议字段。
    pub fn flash_data(&self, data: &[u8], seq: u32, _total: u32) -> Vec<u8> {
        let mut payload = Vec::with_capacity(16 + data.len());
        payload.extend_from_slice(&(data.len() as u32).to_le_bytes());
        payload.extend_from_slice(&seq.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(data);
        build_esp_packet(0x00, 0x03, &payload)
    }

    /// 构造 flash_end 命令（命令码 0x04）
    pub fn flash_end(&self, reboot: bool) -> Vec<u8> {
        let payload = if reboot { [1u8] } else { [0u8] };
        build_esp_packet(0x00, 0x04, &payload)
    }

    /// 构造 ESP8266 读取 Flash 命令（兼容模式：用 SPI_FLASH_DATA 配合读取）
    ///
    /// 注意：ESP8266 ROM 不支持 ESP_FLASH_READ (0x07)，需借助附加的
    /// read_flash 命令（来自 esptool.py 的非标准扩展），命令码 0x0E。
    pub fn read_flash(&self, address: u32, size: u32) -> Vec<u8> {
        let mut payload = Vec::with_capacity(16);
        payload.extend_from_slice(&address.to_le_bytes());
        payload.extend_from_slice(&size.to_le_bytes());
        payload.extend_from_slice(&4096u32.to_le_bytes()); // block_size
        payload.extend_from_slice(&0u32.to_le_bytes());    // max_in_flight
        build_esp_packet(0x00, 0x0E, &payload)
    }
}

// ============================================================================
// STM8 Flasher (STM8 UART Bootloader, UM0560)
// ============================================================================
//
// STM8 Bootloader 协议（UM0560）：
//   - 同步: 0x7F → ACK 0x79
//   - GET: 0x00 0xFF → 返回 bootloader 版本和支持的命令列表
//   - READ: 0x11 0xEE → addr(4 BE) + XOR(addr) + N + (N XOR 0xFF) → ACK + data(N) + ACK
//   - WRITE: 0x31 0xCE → addr(4 BE) + XOR(addr) + N + data(N) + XOR(N, data) → ACK
//   - ERASE: 0x43 0xBC → page_count + pages + XOR → ACK
//   - GO: 0x21 0xDE → addr(4 BE) + XOR(addr) → ACK
//
// 与 STM32 UART Bootloader 非常相似，但补码字节不同（STM8 用 0xFF/0xEE/0xCE/0xDE/0xBC）。

/// STM8 Flasher（UART Bootloader, UM0560）
pub struct Stm8Flasher;

impl Default for Stm8Flasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Stm8Flasher {
    pub fn new() -> Self {
        Self
    }

    /// 同步字节
    pub fn sync(&self) -> Vec<u8> {
        vec![0x7F]
    }

    /// GET 命令（读取 bootloader 版本和支持命令）
    pub fn get_command(&self) -> Vec<u8> {
        vec![0x00, 0xFF]
    }

    /// 构造 READ 命令（读取 N 字节，N ≤ 256）
    pub fn read_memory(&self, address: u32, len: u8) -> Vec<u8> {
        let mut cmd = Vec::with_capacity(8);
        cmd.push(0x11); // READ
        cmd.push(0xEE); // 补码
        let addr_bytes = address.to_be_bytes();
        cmd.extend_from_slice(&addr_bytes);
        cmd.push(addr_bytes.iter().fold(0u8, |acc, &b| acc ^ b));
        let len_minus_1 = len.wrapping_sub(1);
        cmd.push(len_minus_1);
        cmd.push(len_minus_1 ^ 0xFF);
        cmd
    }

    /// 构造 WRITE 命令段序列（3 段，每段等待 ACK）
    pub fn write_memory(&self, address: u32, data: &[u8]) -> Vec<Vec<u8>> {
        if data.is_empty() || data.len() > 256 {
            return Vec::new();
        }
        let mut segments = Vec::with_capacity(3);
        segments.push(vec![0x31, 0xCE]); // WRITE + 补码

        let addr_bytes = address.to_be_bytes();
        let mut seg2 = Vec::with_capacity(5);
        seg2.extend_from_slice(&addr_bytes);
        seg2.push(addr_bytes.iter().fold(0u8, |acc, &b| acc ^ b));
        segments.push(seg2);

        let len_minus_1 = (data.len() - 1) as u8;
        let mut data_checksum = len_minus_1;
        for &b in data {
            data_checksum ^= b;
        }
        let mut seg3 = Vec::with_capacity(2 + data.len());
        seg3.push(len_minus_1);
        seg3.extend_from_slice(data);
        seg3.push(data_checksum);
        segments.push(seg3);

        segments
    }

    /// 构造 GO 命令（跳转执行）
    pub fn go(&self, address: u32) -> Vec<u8> {
        let mut cmd = Vec::with_capacity(7);
        cmd.push(0x21);
        cmd.push(0xDE);
        let addr_bytes = address.to_be_bytes();
        cmd.extend_from_slice(&addr_bytes);
        cmd.push(addr_bytes.iter().fold(0u8, |acc, &b| acc ^ b));
        cmd
    }

    /// 构造全局擦除命令段（2 段）
    pub fn mass_erase(&self) -> Vec<Vec<u8>> {
        vec![
            vec![0x43, 0xBC],
            vec![0xFF, 0x00], // 全局擦除特殊码 + XOR
        ]
    }

    /// ACK 字节
    pub const ACK: u8 = 0x79;
    /// NAK 字节
    pub const NAK: u8 = 0x1F;

    pub fn is_ack(byte: u8) -> bool { byte == Self::ACK }
    pub fn is_nak(byte: u8) -> bool { byte == Self::NAK }
}

// ============================================================================
// NXP LPC Flasher (LPC ISP UART Bootloader, UM10295)
// ============================================================================
//
// LPC ISP 协议（UM10295）：
//   - 同步: '?' × 12 → 'Synchronized\r\n' → OK
//   - 命令格式: <命令> <参数...> \r\n
//   - 应答: <返回码> [<数据>]\r\n
//
// 关键命令：
//   - U 23130: 解锁
//   - P <sector> <sector>: 准备写操作的扇区
//   - C <bank> <part>: 选择 bank
//   - E <sector> <sector>: 擦除扇区
//   - W <ram_addr> <num_bytes>: 写 RAM
//   - R <addr> <num_bytes>: 读 RAM/Flash
//   - A <sector>: 选择扇区用于页写入
//   - I <offset> <num_bytes>: 烧录 Flash (从 RAM)
//   - J: 读取 Part ID
//   - N: 读取 UID
//   - M: 读取 Bootloader 版本
//   - G <addr> <mode>: 跳转执行

/// NXP LPC ISP Flasher (UART Bootloader)
///
/// 命令为 ASCII 格式，以 \r\n 结尾。
/// 应答首行为 0 (成功) 或非零错误码。
pub struct LpcIspFlasher;

impl Default for LpcIspFlasher {
    fn default() -> Self { Self::new() }
}

impl LpcIspFlasher {
    pub fn new() -> Self { Self }

    /// 同步序列：发送 12 个 '?' 字符
    pub fn sync(&self) -> Vec<u8> {
        b"????????????\r\n".to_vec()
    }

    /// 解锁命令: U 23130
    pub fn unlock(&self) -> Vec<u8> {
        b"U 23130\r\n".to_vec()
    }

    /// 读取 Part ID: J
    pub fn read_part_id(&self) -> Vec<u8> {
        b"J\r\n".to_vec()
    }

    /// 读取 UID: N
    pub fn read_uid(&self) -> Vec<u8> {
        b"N\r\n".to_vec()
    }

    /// 读取 Bootloader 版本: M
    pub fn read_version(&self) -> Vec<u8> {
        b"M\r\n".to_vec()
    }

    /// 准备扇区写操作: P <start> <end>
    pub fn prepare_sectors(&self, start: u32, end: u32) -> Vec<u8> {
        format!("P {}\r\n", format_args!("{} {}", start, end)).into_bytes()
    }

    /// 擦除扇区: E <start> <end>
    pub fn erase_sectors(&self, start: u32, end: u32) -> Vec<u8> {
        format!("E {} {}\r\n", start, end).into_bytes()
    }

    /// 写 RAM: W <addr> <num_bytes>
    pub fn write_ram(&self, addr: u32, num_bytes: u32) -> Vec<u8> {
        format!("W {}\r\n", format_args!("{} {}", addr, num_bytes)).into_bytes()
    }

    /// 读内存: R <addr> <num_bytes>
    pub fn read_memory(&self, addr: u32, num_bytes: u32) -> Vec<u8> {
        format!("R {} {}\r\n", addr, num_bytes).into_bytes()
    }

    /// 烧录 Flash: I <offset> <num_bytes>
    pub fn program_flash(&self, offset: u32, num_bytes: u32) -> Vec<u8> {
        format!("I {} {}\r\n", offset, num_bytes).into_bytes()
    }

    /// 跳转执行: G <addr> <mode>（mode=0: ARM mode, T=Thumb mode）
    pub fn go(&self, addr: u32, thumb_mode: bool) -> Vec<u8> {
        let mode = if thumb_mode { "T" } else { "A" };
        format!("G {} {}\r\n", addr, mode).into_bytes()
    }

    /// 设置波特率: B <baud> <stop_bit>
    pub fn set_baudrate(&self, baud: u32, stop_bit: u8) -> Vec<u8> {
        format!("B {} {}\r\n", baud, stop_bit).into_bytes()
    }

    /// 解析应答首行，提取返回码
    ///
    /// 返回 Ok(0) 表示成功，Ok(code) 表示错误码，Err 表示无法解析
    pub fn parse_return_code(resp: &[u8]) -> Result<i32, FlashError> {
        let text = std::str::from_utf8(resp)
            .map_err(|e| FlashError::FlashError(format!("UTF-8 decode error: {}", e)))?;
        let first_line = text.lines().next().unwrap_or("");
        let code = first_line
            .split_whitespace()
            .next()
            .ok_or_else(|| FlashError::FlashError("Empty response".to_string()))?
            .trim()
            .parse::<i32>()
            .map_err(|e| FlashError::FlashError(format!("Invalid return code: {}", e)))?;
        Ok(code)
    }

    /// 检查返回码是否为 0（成功）
    pub fn is_ok(resp: &[u8]) -> bool {
        Self::parse_return_code(resp).unwrap_or(-1) == 0
    }
}

// ============================================================================
// CH32 Flasher (WCH 自定义 ISP 协议, CH32/CH552/CH573/CH582 等)
// ============================================================================
//
// WCH ISP 协议（参考 WCHISPTool）：
//   - 同步: 通过 USB 或串口发送特定握手包
//   - 命令码: 0xA1 (CMD_IDENTIFY) / 0xA2 (CMD_ISP_KEY) / 0xA3 (CMD_ERASE)
//             0xA4 (CMD_WRITE) / 0xA5 (CMD_VERIFY) / 0xA6 (CMD_READ)
//             0xA7 (CMD_ISP_END) / 0xA8 (CMD_SET_BAUD) / 0xA9 (CMD_RESET)
//   - 包格式: 0x57 0xAB <cmd> <len_hi> <len_lo> <payload...> <sum>
//
// CH32 系列包括：CH32F103/CH32V103/CH32F203/CH32V203/CH552/CH55x/CH573/CH582

/// WCH ISP 协议常量
pub mod wch_isp {
    /// 包起始标志 0
    pub const FRAME_MARKER_0: u8 = 0x57;
    /// 包起始标志 1
    pub const FRAME_MARKER_1: u8 = 0xAB;
    /// 设备识别
    pub const CMD_IDENTIFY: u8 = 0xA1;
    /// ISP Key 协商
    pub const CMD_ISP_KEY: u8 = 0xA2;
    /// 擦除 Flash
    pub const CMD_ERASE: u8 = 0xA3;
    /// 写入 Flash
    pub const CMD_WRITE: u8 = 0xA4;
    /// 校验 Flash
    pub const CMD_VERIFY: u8 = 0xA5;
    /// 读取 Flash
    pub const CMD_READ: u8 = 0xA6;
    /// ISP 结束
    pub const CMD_ISP_END: u8 = 0xA7;
    /// 设置波特率
    pub const CMD_SET_BAUD: u8 = 0xA8;
    /// 复位
    pub const CMD_RESET: u8 = 0xA9;
    /// 读芯片 ID
    pub const CMD_READ_CHIP_ID: u8 = 0xAA;
    /// 写配置
    pub const CMD_WRITE_CONFIG: u8 = 0xAB;
    /// 读配置
    pub const CMD_READ_CONFIG: u8 = 0xAC;
}

/// CH32 系列 Flasher（WCH 自定义 ISP 协议）
pub struct Ch32Flasher;

impl Default for Ch32Flasher {
    fn default() -> Self { Self::new() }
}

impl Ch32Flasher {
    pub fn new() -> Self { Self }

    /// 构造 ISP 数据包
    ///
    /// 包格式: 0x57 0xAB <cmd> <len_hi> <len_lo> <payload> <sum>
    /// sum = (cmd + len_hi + len_lo + payload_sum) & 0xFF
    ///
    /// 限制：payload 长度必须 ≤ 0xFFFF（u16 上限），否则静默截断会导致
    /// 接收端只读取部分数据并产生校验和错误。此处改为返回空 Vec 标记失败，
    /// 调用方应保证分块大小（典型 WCH ISP 单次写入 ≤ 256 字节）。
    fn build_packet(cmd: u8, payload: &[u8]) -> Vec<u8> {
        if payload.len() > 0xFFFF {
            return Vec::new();
        }
        let len = payload.len() as u16;
        let len_hi = (len >> 8) as u8;
        let len_lo = (len & 0xFF) as u8;
        let mut sum = cmd.wrapping_add(len_hi).wrapping_add(len_lo);
        for &b in payload {
            sum = sum.wrapping_add(b);
        }
        let mut packet = Vec::with_capacity(5 + payload.len() + 1);
        packet.push(wch_isp::FRAME_MARKER_0);
        packet.push(wch_isp::FRAME_MARKER_1);
        packet.push(cmd);
        packet.push(len_hi);
        packet.push(len_lo);
        packet.extend_from_slice(payload);
        packet.push(sum);
        packet
    }

    /// 构造 IDENTIFY 命令（设备识别）
    pub fn identify(&self) -> Vec<u8> {
        Self::build_packet(wch_isp::CMD_IDENTIFY, &[])
    }

    /// 构造 READ_CHIP_ID 命令
    pub fn read_chip_id(&self) -> Vec<u8> {
        Self::build_packet(wch_isp::CMD_READ_CHIP_ID, &[])
    }

    /// 构造 ERASE 命令（擦除指定地址和大小的 Flash）
    pub fn erase(&self, addr: u32, size: u32) -> Vec<u8> {
        let mut payload = Vec::with_capacity(8);
        payload.extend_from_slice(&addr.to_le_bytes());
        payload.extend_from_slice(&size.to_le_bytes());
        Self::build_packet(wch_isp::CMD_ERASE, &payload)
    }

    /// 构造 WRITE 命令（写入 Flash）
    pub fn write(&self, addr: u32, data: &[u8]) -> Vec<u8> {
        let mut payload = Vec::with_capacity(4 + data.len());
        payload.extend_from_slice(&addr.to_le_bytes());
        payload.extend_from_slice(data);
        Self::build_packet(wch_isp::CMD_WRITE, &payload)
    }

    /// 构造 READ 命令（读取 Flash）
    pub fn read(&self, addr: u32, size: u32) -> Vec<u8> {
        let mut payload = Vec::with_capacity(8);
        payload.extend_from_slice(&addr.to_le_bytes());
        payload.extend_from_slice(&size.to_le_bytes());
        Self::build_packet(wch_isp::CMD_READ, &payload)
    }

    /// 构造 VERIFY 命令（校验 Flash）
    pub fn verify(&self, addr: u32, data: &[u8]) -> Vec<u8> {
        let mut payload = Vec::with_capacity(4 + data.len());
        payload.extend_from_slice(&addr.to_le_bytes());
        payload.extend_from_slice(data);
        Self::build_packet(wch_isp::CMD_VERIFY, &payload)
    }

    /// 构造 ISP_END 命令（退出 ISP 模式）
    pub fn isp_end(&self, reset: bool) -> Vec<u8> {
        let payload = if reset { [0x01] } else { [0x00] };
        Self::build_packet(wch_isp::CMD_ISP_END, &payload)
    }

    /// 构造 RESET 命令（复位芯片）
    pub fn reset(&self) -> Vec<u8> {
        Self::build_packet(wch_isp::CMD_RESET, &[])
    }

    /// 校验包是否有效（开头 0x57 0xAB + 校验和）
    pub fn validate_packet(resp: &[u8]) -> bool {
        if resp.len() < 6 {
            return false;
        }
        if resp[0] != wch_isp::FRAME_MARKER_0 || resp[1] != wch_isp::FRAME_MARKER_1 {
            return false;
        }
        let len = ((resp[3] as u16) << 8) | (resp[4] as u16);
        if resp.len() != 5 + len as usize + 1 {
            return false;
        }
        let mut sum = resp[2].wrapping_add(resp[3]).wrapping_add(resp[4]);
        for i in 0..len as usize {
            sum = sum.wrapping_add(resp[5 + i]);
        }
        sum == resp[5 + len as usize]
    }

    /// 从响应包中提取 payload
    pub fn extract_payload(resp: &[u8]) -> Result<&[u8], FlashError> {
        if !Self::validate_packet(resp) {
            return Err(FlashError::FlashError("Invalid WCH packet".to_string()));
        }
        let len = ((resp[3] as u16) << 8) | (resp[4] as u16) as u16;
        Ok(&resp[5..5 + len as usize])
    }

    /// 根据 Chip ID 识别 CH32 系列芯片
    ///
    /// 返回 (芯片名, Flash 大小 KB)
    pub fn identify_chip_id(chip_id: u32) -> Option<(&'static str, u32)> {
        match chip_id {
            0x1030_0030 => Some(("CH32F103C8", 64)),
            0x1030_0031 => Some(("CH32F103CB", 128)),
            0x1030_0032 => Some(("CH32F103RC", 256)),
            0x1031_0030 => Some(("CH32V103C8", 64)),
            0x1031_0031 => Some(("CH32V103CB", 128)),
            0x2030_0030 => Some(("CH32F203C8", 64)),
            0x2030_0031 => Some(("CH32F203CB", 128)),
            0x2031_0030 => Some(("CH32V203C8", 64)),
            0x2031_0031 => Some(("CH32V203CB", 128)),
            0x5520_0030 => Some(("CH552", 16)),
            0x5520_0031 => Some(("CH552T", 16)),
            0x5730_0030 => Some(("CH573", 448)),
            0x5820_0030 => Some(("CH582", 448)),
            _ => None,
        }
    }
}

// ============================================================================
// PIC Flasher (Microchip PIC ICSP 协议)
// ============================================================================
//
// PIC 单片机 ICSP (In-Circuit Serial Programming) 协议：
//   - 6 引脚：VDD / VPP / MCLR / PGD / PGC / GND
//   - 需要 PicKit / ICD / SNAP 等编程器硬件
//   - 命令通过 PGC 时钟逐位移入 PGD
//
// 主要命令（6-bit 命令字，LSB first）：
//   - 000000: Load Configuration
//   - 000001: Load Data for Program Memory
//   - 000010: Read Data from Program Memory
//   - 000011: Increment Address
//   - 000100: Begin Programming
//   - 000101: End Programming
//   - 001000: Bulk Erase Program Memory
//   - 001001: Bulk Erase Data Memory
//   - 010000: Load Data for Data Memory
//   - 010001: Read Data from Data Memory
//
// 本实现提供 ICSP 命令构造，实际硬件时序需通过 PicKit 等适配器驱动。
// 也可通过 STK500 v2 协议与 AVR ISP mkII 等适配器对接。

/// PIC ICSP 协议常量（6-bit 命令字，LSB first）
pub mod pic_icsp {
    pub const CMD_LOAD_CONFIG: u8 = 0x00;
    pub const CMD_LOAD_DATA_PM: u8 = 0x01;
    pub const CMD_READ_DATA_PM: u8 = 0x02;
    pub const CMD_INC_ADDRESS: u8 = 0x03;
    pub const CMD_BEGIN_PROG: u8 = 0x04;
    pub const CMD_END_PROG: u8 = 0x05;
    pub const CMD_BULK_ERASE_PM: u8 = 0x08;
    pub const CMD_BULK_ERASE_DM: u8 = 0x09;
    pub const CMD_LOAD_DATA_DM: u8 = 0x10;
    pub const CMD_READ_DATA_DM: u8 = 0x11;
}

/// PIC ICSP Flasher（命令构造与签名识别）
///
/// 注意：ICSP 需要专用硬件时序（PGC/PGD），miconlab 通过 UART 串口
/// 无法直接驱动。本模块仅提供命令字节序列构造和 PIC 签名识别，
/// 实际烧录需借助 PicKit/ICD 等编程器。
pub struct PicIcspFlasher;

impl Default for PicIcspFlasher {
    fn default() -> Self { Self::new() }
}

impl PicIcspFlasher {
    pub fn new() -> Self { Self }

    /// 构造 6-bit 命令字（LSB first）
    ///
    /// 返回 6 位命令字，可由适配器逐位移入 PGD
    pub fn build_command(cmd: u8) -> u8 {
        cmd & 0x3F
    }

    /// 构造 Load Configuration 命令 + 14-bit 数据
    pub fn load_config(&self, config: u16) -> Vec<u8> {
        let cmd = Self::build_command(pic_icsp::CMD_LOAD_CONFIG);
        let data = config & 0x3FFF; // 14-bit
        // 命令 6 bit + 数据 16 bit (含 2 stop bits)
        vec![cmd, (data & 0xFF) as u8, ((data >> 8) & 0x3F) as u8]
    }

    /// 构造 Load Data for Program Memory + 14-bit 数据
    pub fn load_data_pm(&self, data: u16) -> Vec<u8> {
        let cmd = Self::build_command(pic_icsp::CMD_LOAD_DATA_PM);
        let data = data & 0x3FFF;
        vec![cmd, (data & 0xFF) as u8, ((data >> 8) & 0x3F) as u8]
    }

    /// 构造 Read Data from Program Memory 命令
    pub fn read_data_pm(&self) -> Vec<u8> {
        vec![Self::build_command(pic_icsp::CMD_READ_DATA_PM)]
    }

    /// 构造 Increment Address 命令
    pub fn inc_address(&self) -> Vec<u8> {
        vec![Self::build_command(pic_icsp::CMD_INC_ADDRESS)]
    }

    /// 构造 Begin Programming 命令
    pub fn begin_prog(&self) -> Vec<u8> {
        vec![Self::build_command(pic_icsp::CMD_BEGIN_PROG)]
    }

    /// 构造 End Programming 命令
    pub fn end_prog(&self) -> Vec<u8> {
        vec![Self::build_command(pic_icsp::CMD_END_PROG)]
    }

    /// 构造 Bulk Erase Program Memory 命令
    pub fn bulk_erase_pm(&self) -> Vec<u8> {
        vec![Self::build_command(pic_icsp::CMD_BULK_ERASE_PM)]
    }

    /// 构造 Bulk Erase Data Memory 命令
    pub fn bulk_erase_dm(&self) -> Vec<u8> {
        vec![Self::build_command(pic_icsp::CMD_BULK_ERASE_DM)]
    }

    /// 解析 14-bit 数据响应（去掉 stop bits）
    pub fn parse_14bit_response(resp: &[u8]) -> Result<u16, FlashError> {
        if resp.len() < 2 {
            return Err(FlashError::FlashError("PIC response too short".to_string()));
        }
        let data = (resp[0] as u16) | ((resp[1] as u16 & 0x3F) << 8);
        Ok(data & 0x3FFF)
    }

    /// 根据 PIC 器件 ID 识别芯片型号
    ///
    /// PIC18 系列 Device ID 通常是 5-bit (高 5 位) + 2-bit revision (低 2 位)
    pub fn identify_device_id(device_id: u16) -> Option<(&'static str, u32)> {
        let family = (device_id >> 5) & 0x1F; // 高 5 位
        let rev = device_id & 0x1F;            // 低 5 位 (revision)
        let _ = rev;
        match family {
            0x04 => Some(("PIC18F2420/2520/4420/4520", 32)),
            0x06 => Some(("PIC18F2455/2550/4455/4550", 32)),
            0x0C => Some(("PIC18F2480/2580/4480/4580", 32)),
            0x0E => Some(("PIC18F2680/4680", 64)),
            0x10 => Some(("PIC18F25K80/26K80/45K80/46K80", 64)),
            0x12 => Some(("PIC18F26J11/46J11", 64)),
            0x14 => Some(("PIC18F27J53/47J53", 128)),
            0x16 => Some(("PIC18F27K40/47K40", 128)),
            _ => None,
        }
    }
}

// ============================================================================
// Intel HEX / 二进制解析
// ============================================================================

/// Parse an Intel HEX file into a list of records.
pub fn parse_ihex(content: &str) -> FlashResult<Vec<IhexRecord>> {
    let mut records = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if !line.starts_with(':') {
            return Err(FlashError::InvalidIhex(format!(
                "Line does not start with ':' : {}",
                line
            )));
        }

        let hex = &line[1..];

        if hex.len() < 10 {
            return Err(FlashError::InvalidIhex(format!(
                "Line too short: {}",
                line
            )));
        }

        let byte_count = u8::from_str_radix(&hex[0..2], 16)
            .map_err(|e| FlashError::InvalidIhex(format!("Invalid byte count: {}", e)))?;

        let address = u16::from_str_radix(&hex[2..6], 16)
            .map_err(|e| FlashError::InvalidIhex(format!("Invalid address: {}", e)))?;

        let record_type = u8::from_str_radix(&hex[6..8], 16)
            .map_err(|e| FlashError::InvalidIhex(format!("Invalid record type: {}", e)))?;

        // 数据区必须完整：截断行静默接受会导致 data.len() < byte_count，下游按 byte_count 处理时错位
        let data_hex_len = byte_count as usize * 2;
        if hex.len() < 8 + data_hex_len + 2 {
            return Err(FlashError::InvalidIhex(format!(
                "Truncated line (need {} chars, got {}): {}",
                8 + data_hex_len + 2,
                hex.len(),
                line
            )));
        }

        let mut data = Vec::with_capacity(byte_count as usize);
        for i in (8..8 + data_hex_len).step_by(2) {
            let byte = u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| FlashError::InvalidIhex(format!("Invalid data byte: {}", e)))?;
            data.push(byte);
        }

        // Verify checksum
        let mut sum = 0u8;
        sum = sum.wrapping_add(byte_count);
        sum = sum.wrapping_add((address >> 8) as u8);
        sum = sum.wrapping_add((address & 0xFF) as u8);
        sum = sum.wrapping_add(record_type);
        for &b in &data {
            sum = sum.wrapping_add(b);
        }

        // 行长度已在前面的截断检查中保证，校验和必定存在
        let checksum = &hex[8 + data_hex_len..8 + data_hex_len + 2];
        let expected = u8::from_str_radix(checksum, 16)
            .map_err(|e| FlashError::InvalidIhex(format!("Invalid checksum: {}", e)))?;
        if sum.wrapping_add(expected) != 0 {
            return Err(FlashError::InvalidIhex(format!(
                "Checksum mismatch at address {:#06X}",
                address
            )));
        }

        records.push(IhexRecord {
            byte_count,
            address,
            record_type,
            data,
        });

        // Stop at end-of-file record
        if record_type == 0x01 {
            break;
        }
    }

    Ok(records)
}

/// Parse binary data into a simple record (all data at offset 0).
pub fn parse_binary(data: &[u8]) -> FlashResult<Vec<IhexRecord>> {
    if data.is_empty() {
        return Err(FlashError::InvalidBinary("Empty binary data".to_string()));
    }

    Ok(vec![IhexRecord {
        byte_count: data.len().min(255) as u8,
        address: 0,
        record_type: 0x00,
        data: data.to_vec(),
    }])
}

/// 将二进制数据转换为 Intel HEX 格式字符串
///
/// - 每行 16 字节数据记录（type=0x00）
/// - 地址超过 0xFFFF 时自动插入扩展线性地址记录（type=0x04）
/// - 末尾追加 EOF 记录（type=0x01）
/// - `base_addr` 为数据起始地址（如 0x08000000）
pub fn bin_to_ihex(data: &[u8], base_addr: u32) -> String {
    let mut out = String::with_capacity(data.len() * 4);
    let mut current_upper = (base_addr >> 16) & 0xFFFF;
    // 输出初始扩展线性地址记录
    out.push_str(&format!(
        ":02000004{:02X}{:02X}{:02X}\r\n",
        (current_upper >> 8) as u8,
        (current_upper & 0xFF) as u8,
        // Intel HEX 校验和 = 全部字节求和后取补码（不能用 XOR，XOR 不产生进位）
        (0x02u8
            .wrapping_add((current_upper >> 8) as u8)
            .wrapping_add((current_upper & 0xFF) as u8)
            .wrapping_add(0x04))
        .wrapping_neg()
    ));

    let mut addr = base_addr;
    let mut offset = 0usize;
    const LINE_BYTES: usize = 16;

    while offset < data.len() {
        // 计算当前行的地址偏移（16 位）
        let upper = (addr >> 16) & 0xFFFF;
        if upper != current_upper {
            current_upper = upper;
            out.push_str(&format!(
                ":02000004{:02X}{:02X}{:02X}\r\n",
                (current_upper >> 8) as u8,
                (current_upper & 0xFF) as u8,
                (0x02u8
                    .wrapping_add((current_upper >> 8) as u8)
                    .wrapping_add((current_upper & 0xFF) as u8)
                    .wrapping_add(0x04))
                .wrapping_neg()
            ));
        }
        let lower = (addr & 0xFFFF) as u16;
        let chunk_len = (data.len() - offset).min(LINE_BYTES);
        let bytes = &data[offset..offset + chunk_len];

        // 构造一行：byte_count(1) + addr(2) + type(1) + data(n) + checksum(1)
        let mut sum = 0u8;
        sum = sum.wrapping_add(chunk_len as u8);
        sum = sum.wrapping_add((lower >> 8) as u8);
        sum = sum.wrapping_add((lower & 0xFF) as u8);
        sum = sum.wrapping_add(0x00); // record type
        for &b in bytes {
            sum = sum.wrapping_add(b);
        }
        let checksum = sum.wrapping_neg();

        out.push_str(&format!(":{:02X}{:04X}00", chunk_len as u8, lower));
        for &b in bytes {
            out.push_str(&format!("{:02X}", b));
        }
        out.push_str(&format!("{:02X}\r\n", checksum));

        offset += chunk_len;
        addr = addr.wrapping_add(chunk_len as u32);
    }

    // EOF 记录
    out.push_str(":00000001FF\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // STM32 测试
    // -------------------------------------------------------------------

    #[test]
    fn test_stm32_get_command() {
        let flasher = Stm32Flasher::new();
        let cmd = flasher.get_command();
        assert_eq!(cmd, vec![0x7F]);
    }

    #[test]
    fn test_stm32_ack_nack() {
        let flasher = Stm32Flasher::new();
        assert_eq!(flasher.ack(), 0x79);
        assert_eq!(flasher.nack(), 0x1F);
    }

    #[test]
    fn test_stm32_erase_memory() {
        let flasher = Stm32Flasher::new();
        let cmd = flasher.erase_memory(&[0, 1]);
        assert_eq!(cmd[0], 0x43);
        assert_eq!(cmd[1], 0x44);
    }

    #[test]
    fn test_stm32_mass_erase_global_format() {
        // 整片擦除使用扩展擦除命令 (0x44 0xBB) + 全局擦除特殊码 (0xFF 0xFF 0x00)
        let flasher = Stm32Flasher::new();
        let segs = flasher.mass_erase();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], vec![0x44, 0xBB]); // Extended Erase + complement
        assert_eq!(segs[1], vec![0xFF, 0xFF, 0x00]); // global special code + XOR checksum
    }

    #[test]
    fn test_stm32_write_memory_segments() {
        // 写内存命令应返回 3 段，校验和分离
        let flasher = Stm32Flasher::new();
        let data = vec![0xAA, 0xBB, 0xCC];
        let segs = flasher.write_memory(0x08000000, &data);

        assert_eq!(segs.len(), 3);

        // 段1: 命令 + 补码
        assert_eq!(segs[0], vec![0x31, 0xCE]);

        // 段2: 地址段 (0x08000000 BE) + XOR 校验和
        // 0x08 ^ 0x00 ^ 0x00 ^ 0x00 = 0x08
        assert_eq!(segs[1], vec![0x08, 0x00, 0x00, 0x00, 0x08]);

        // 段3: (N-1) + data + XOR(N-1, data)
        // N=3 → N-1=0x02, XOR(0x02, 0xAA, 0xBB, 0xCC)
        let expected_cksum = 0x02 ^ 0xAA ^ 0xBB ^ 0xCC;
        assert_eq!(segs[2], vec![0x02, 0xAA, 0xBB, 0xCC, expected_cksum]);
    }

    #[test]
    fn test_stm32_write_memory_empty_safe() {
        // 修复点：data.is_empty() 不再触发 (0-1 as u8) 溢出
        let flasher = Stm32Flasher::new();
        let segs = flasher.write_memory(0x08000000, &[]);
        assert!(segs.is_empty());
    }

    #[test]
    fn test_stm32_write_memory_too_large() {
        // 超过 256 字节拒绝
        let flasher = Stm32Flasher::new();
        let data = vec![0u8; 257];
        let segs = flasher.write_memory(0x08000000, &data);
        assert!(segs.is_empty());
    }

    #[test]
    fn test_stm32_read_memory() {
        let flasher = Stm32Flasher::new();
        let cmd = flasher.read_memory(0x08000000, 16);
        assert_eq!(cmd[0], 0x11);
        assert_eq!(cmd[1], 0xEE);
    }

    #[test]
    fn test_stm32_go() {
        let flasher = Stm32Flasher::new();
        let cmd = flasher.go(0x08000000);
        assert_eq!(cmd[0], 0x21);
        assert_eq!(cmd[1], 0xDE);
    }

    #[test]
    fn test_stm32_read_uid() {
        let flasher = Stm32Flasher::new();
        let cmd = flasher.read_uid();
        assert_eq!(cmd, vec![0x27, 0xD8]);
    }

    #[test]
    fn test_stm32_checksum() {
        let flasher = Stm32Flasher::new();
        let data = [0x01, 0x02, 0x03];
        assert_eq!(flasher.checksum(&data), 0x01 ^ 0x02 ^ 0x03);
    }

    // -------------------------------------------------------------------
    // ESP32 SLIP 编码测试
    // -------------------------------------------------------------------

    #[test]
    fn test_slip_encode_no_escape() {
        assert_eq!(
            slip_encode(&[0x01, 0x02, 0x03]),
            vec![0xC0, 0x01, 0x02, 0x03, 0xC0]
        );
    }

    #[test]
    fn test_slip_encode_escape_c0() {
        // 0xC0 → 0xDB 0xDC
        assert_eq!(slip_encode(&[0xC0]), vec![0xC0, 0xDB, 0xDC, 0xC0]);
    }

    #[test]
    fn test_slip_encode_escape_db() {
        // 0xDB → 0xDB 0xDD
        assert_eq!(slip_encode(&[0xDB]), vec![0xC0, 0xDB, 0xDD, 0xC0]);
    }

    #[test]
    fn test_slip_encode_mixed() {
        assert_eq!(
            slip_encode(&[0xC0, 0xDB, 0x55]),
            vec![0xC0, 0xDB, 0xDC, 0xDB, 0xDD, 0x55, 0xC0]
        );
    }

    // -------------------------------------------------------------------
    // ESP32 CRC-32C 测试
    // -------------------------------------------------------------------

    #[test]
    fn test_crc32_esp_empty() {
        // 空 data: init=0xFFFFFFFF, 无最终 XOR → 返回 0xFFFFFFFF
        assert_eq!(crc32_esp(b""), 0xFFFFFFFF);
    }

    #[test]
    fn test_crc32_esp_known_vector() {
        // ESP32 CRC32 (CRC-32C 多项式，无最终 XOR) of "123456789"
        // = 标准 CRC-32C (0xE3069283) ^ 0xFFFFFFFF = 0x1CF96D7C
        assert_eq!(crc32_esp(b"123456789"), 0x1CF96D7C);
    }

    // -------------------------------------------------------------------
    // ESP32 命令包测试
    // -------------------------------------------------------------------

    #[test]
    fn test_esp32_sync_packet_structure() {
        let flasher = Esp32Flasher::new();
        let cmd = flasher.sync();

        // SLIP framing
        assert_eq!(cmd[0], 0xC0);
        assert_eq!(cmd[cmd.len() - 1], 0xC0);

        // direction + command（在 SLIP 转义之前，索引固定）
        assert_eq!(cmd[1], 0x00); // direction = request
        assert_eq!(cmd[2], 0x08); // command = ESP_SYNC

        // size = 36 (LE 4 字节)。size 字节 (0x24 0x00 0x00 0x00) 不含特殊字符
        assert_eq!(cmd[3], 36);
        assert_eq!(cmd[4], 0);
        assert_eq!(cmd[5], 0);
        assert_eq!(cmd[6], 0);

        // sync 数据 (0x07 0x07 0x12 0x20 + 32×0x55) 不含 0xC0/0xDB，不会展开。
        // 但 checksum 字节可能含 0xC0/0xDB 导致 SLIP 转义后偏移变化，
        // 因此用搜索定位 sync 数据起始位置。
        let sync_prefix = [0x07, 0x07, 0x12, 0x20];
        let sync_start = cmd
            .windows(4)
            .position(|w| w == sync_prefix)
            .expect("sync data not found in encoded packet");

        // 验证 sync_start 后续 32 字节为 0x55
        for i in 0..32 {
            assert_eq!(
                cmd[sync_start + 4 + i],
                0x55,
                "expected 0x55 at index {}",
                sync_start + 4 + i
            );
        }

        // 最后一个 0x55 之后紧跟 0xC0 (包结束)
        assert_eq!(cmd[sync_start + 4 + 32], 0xC0);
    }

    #[test]
    fn test_esp32_sync_repeated() {
        let flasher = Esp32Flasher::new();
        let packets = flasher.sync_repeated(7);
        assert_eq!(packets.len(), 7);
        for p in &packets {
            assert_eq!(p[0], 0xC0);
            assert_eq!(p[1], 0x00); // direction
            assert_eq!(p[2], 0x08); // command
        }
    }

    #[test]
    fn test_esp32_chip_id() {
        let flasher = Esp32Flasher::new();
        let cmd = flasher.chip_id();
        assert_eq!(cmd[0], 0xC0);
        assert_eq!(cmd[1], 0x00);
        assert_eq!(cmd[2], 0x0B);
    }

    #[test]
    fn test_esp32_version() {
        let flasher = Esp32Flasher::new();
        let cmd = flasher.version();
        assert_eq!(cmd[0], 0xC0);
        assert_eq!(cmd[1], 0x00);
        assert_eq!(cmd[2], 0x08);
    }

    #[test]
    fn test_esp32_flash_begin() {
        let flasher = Esp32Flasher::new();
        let cmd = flasher.flash_begin(4096, 100, 0x10000);
        assert_eq!(cmd[0], 0xC0);
        assert_eq!(cmd[1], 0x00);
        assert_eq!(cmd[2], 0x02); // command
        // size = 16 (LE)
        assert_eq!(cmd[3], 16);
        assert_eq!(cmd[4], 0);
        assert_eq!(cmd[5], 0);
        assert_eq!(cmd[6], 0);
    }

    #[test]
    fn test_esp32_flash_finish() {
        let flasher = Esp32Flasher::new();
        let cmd = flasher.flash_finish();
        assert_eq!(cmd[0], 0xC0);
        assert_eq!(cmd[1], 0x00);
        assert_eq!(cmd[2], 0x04); // command
        // size = 4 (LE)
        assert_eq!(cmd[3], 4);
    }

    #[test]
    fn test_esp32_flash_data() {
        let flasher = Esp32Flasher::new();
        let data = vec![0xAA, 0xBB];
        let cmd = flasher.flash_data(&data, 3);
        assert_eq!(cmd[0], 0xC0);
        assert_eq!(cmd[1], 0x00);
        assert_eq!(cmd[2], 0x03); // command
        // size = 16 (标准头) + 2 (data) = 18 (LE)
        assert_eq!(cmd[3], 18);
    }

    #[test]
    fn test_esp32_flash_data_with_escape() {
        // 包含需转义的字节，验证 SLIP 编码正确
        let flasher = Esp32Flasher::new();
        let data = vec![0xC0, 0xDB, 0x55];
        let cmd = flasher.flash_data(&data, 0);
        // 起止 framing
        assert_eq!(cmd[0], 0xC0);
        assert_eq!(cmd[cmd.len() - 1], 0xC0);
        // 包体应包含转义序列 0xDB 0xDC 和 0xDB 0xDD
        assert!(cmd.windows(2).any(|w| w == [0xDB, 0xDC]));
        assert!(cmd.windows(2).any(|w| w == [0xDB, 0xDD]));
    }

    // -------------------------------------------------------------------
    // Intel HEX / 二进制解析测试
    // -------------------------------------------------------------------

    #[test]
    fn test_parse_ihex_valid() {
        let ihex = ":0400000001020304F2\n:00000001FF\n";
        let records = parse_ihex(ihex).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].byte_count, 4);
        assert_eq!(records[0].data, vec![0x01, 0x02, 0x03, 0x04]);
        assert_eq!(records[0].record_type, 0x00);
        assert_eq!(records[1].record_type, 0x01); // EOF
    }

    #[test]
    fn test_parse_ihex_invalid() {
        let result = parse_ihex("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_binary() {
        let data = [0x01, 0x02, 0x03];
        let records = parse_binary(&data).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].data, vec![0x01, 0x02, 0x03]);
        assert_eq!(records[0].address, 0);
    }

    #[test]
    fn test_parse_binary_empty() {
        let result = parse_binary(&[]);
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------
    // STM32 扩展擦除测试
    // -------------------------------------------------------------------

    #[test]
    fn test_stm32_extended_erase_pages_structure() {
        // 段1: 0x44 0xBB；段2: page_count(N-1 BE) + pages + XOR 校验和
        let flasher = Stm32Flasher::new();
        let segs = flasher.extended_erase_pages(&[0x0000, 0x0001, 0x0002]);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], vec![0x44, 0xBB]);

        // 段2: count(N-1=2, BE 2 字节: 0x00 0x02) + 3 pages (BE) + checksum
        // pages = [0x00,0x00, 0x00,0x01, 0x00,0x02]
        // XOR = 0x00 ^ 0x02 ^ 0x00 ^ 0x00 ^ 0x00 ^ 0x01 ^ 0x00 ^ 0x02 = 0x01
        assert_eq!(segs[1], vec![0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x02, 0x01]);
    }

    #[test]
    fn test_stm32_extended_erase_pages_checksum() {
        // 校验和 = XOR(count_bytes ++ page_bytes)
        let flasher = Stm32Flasher::new();
        let pages = [0x1234u16, 0x5678];
        let segs = flasher.extended_erase_pages(&pages);
        assert_eq!(segs.len(), 2);

        let count_bytes = (pages.len() as u16 - 1).to_be_bytes();
        let mut expected_cksum = 0u8;
        for &b in &count_bytes {
            expected_cksum ^= b;
        }
        for &p in &pages {
            expected_cksum ^= (p >> 8) as u8;
            expected_cksum ^= (p & 0xFF) as u8;
        }
        assert_eq!(*segs[1].last().unwrap(), expected_cksum);
    }

    #[test]
    fn test_stm32_extended_erase_pages_empty() {
        let flasher = Stm32Flasher::new();
        assert!(flasher.extended_erase_pages(&[]).is_empty());
    }

    #[test]
    fn test_stm32_extended_erase_pages_too_many() {
        // 0xFFFF + 1 页超限（0xFFFF 保留给全局擦除）
        let flasher = Stm32Flasher::new();
        let pages = vec![0u16; 0xFFFF];
        assert!(flasher.extended_erase_pages(&pages).is_empty());
    }

    #[test]
    fn test_stm32_extended_erase_pages_max_boundary() {
        // 边界：0xFFFE 页应接受
        let flasher = Stm32Flasher::new();
        let pages = vec![0u16; 0xFFFE];
        let segs = flasher.extended_erase_pages(&pages);
        assert_eq!(segs.len(), 2);
    }

    #[test]
    fn test_stm32_extended_erase_bank_all() {
        // All = 全局擦除，等价于 mass_erase
        let flasher = Stm32Flasher::new();
        let segs = flasher.extended_erase_bank(Stm32Bank::All);
        assert_eq!(segs[0], vec![0x44, 0xBB]);
        assert_eq!(segs[1], vec![0xFF, 0xFF, 0x00]);
    }

    #[test]
    fn test_stm32_extended_erase_bank1() {
        let flasher = Stm32Flasher::new();
        let segs = flasher.extended_erase_bank(Stm32Bank::Bank1);
        assert_eq!(segs[0], vec![0x44, 0xBB]);
        assert_eq!(segs[1], vec![0xFF, 0xFE, 0x01]);
    }

    #[test]
    fn test_stm32_extended_erase_bank2() {
        let flasher = Stm32Flasher::new();
        let segs = flasher.extended_erase_bank(Stm32Bank::Bank2);
        assert_eq!(segs[0], vec![0x44, 0xBB]);
        assert_eq!(segs[1], vec![0xFF, 0xFD, 0x02]);
    }

    #[test]
    fn test_stm32_extended_erase_bank_checksums() {
        // 校验和 = 0xFF ^ special ^ 0x00（实际是 0xFF ^ special）
        let flasher = Stm32Flasher::new();
        for (bank, special, expected_ck) in [
            (Stm32Bank::All, 0xFF, 0x00),
            (Stm32Bank::Bank1, 0xFE, 0x01),
            (Stm32Bank::Bank2, 0xFD, 0x02),
        ] {
            let segs = flasher.extended_erase_bank(bank);
            assert_eq!(segs[1][0], 0xFF);
            assert_eq!(segs[1][1], special);
            assert_eq!(segs[1][2], expected_ck);
            assert_eq!(0xFF ^ special ^ 0x00, expected_ck);
        }
    }

    // -------------------------------------------------------------------
    // STM32 ACK 解析辅助测试
    // -------------------------------------------------------------------

    #[test]
    fn test_stm32_is_ack_nak() {
        assert!(Stm32Flasher::is_ack(0x79));
        assert!(!Stm32Flasher::is_ack(0x1F));
        assert!(Stm32Flasher::is_nak(0x1F));
        assert!(!Stm32Flasher::is_nak(0x79));
        assert!(!Stm32Flasher::is_ack(0x00));
    }

    #[test]
    fn test_stm32_parse_ack_ok() {
        assert!(Stm32Flasher::parse_ack(&[0x79]).is_ok());
    }

    #[test]
    fn test_stm32_parse_ack_nak_error() {
        let err = Stm32Flasher::parse_ack(&[0x1F]).unwrap_err();
        assert!(matches!(err, FlashError::FlashError(_)));
        assert_eq!(err.to_string(), "Flash error: Device NAK");
    }

    #[test]
    fn test_stm32_parse_ack_unexpected() {
        let err = Stm32Flasher::parse_ack(&[0xAB]).unwrap_err();
        assert!(matches!(err, FlashError::FlashError(_)));
        assert!(err.to_string().contains("0xAB"));
    }

    #[test]
    fn test_stm32_parse_ack_empty() {
        let err = Stm32Flasher::parse_ack(&[]).unwrap_err();
        assert!(matches!(err, FlashError::FlashError(_)));
        assert!(err.to_string().contains("Empty"));
    }

    #[test]
    fn test_stm32_parse_ack_only_first_byte() {
        // 仅校验首字节，后续字节忽略
        assert!(Stm32Flasher::parse_ack(&[0x79, 0xAA, 0xBB]).is_ok());
        assert!(Stm32Flasher::parse_ack(&[0x1F, 0x79]).is_err());
    }

    // -------------------------------------------------------------------
    // ESP32 高级命令测试
    // -------------------------------------------------------------------

    #[test]
    fn test_esp32_change_baudrate_structure() {
        let flasher = Esp32Flasher::new();
        let cmd = flasher.change_baudrate(460800, 115200);
        assert_eq!(cmd[0], 0xC0);
        assert_eq!(cmd[1], 0x00); // direction
        assert_eq!(cmd[2], 0x0F); // ESP_CHANGE_BAUDRATE
        // size = 8 (LE)
        assert_eq!(cmd[3], 8);
        assert_eq!(cmd[4], 0);
        assert_eq!(cmd[5], 0);
        assert_eq!(cmd[6], 0);
    }

    #[test]
    fn test_esp32_change_baudrate_payload() {
        // 验证 payload 字段（SLIP 帧内，无转义时）：
        // C0 + direction(1) + command(1) + size(2 LE) + checksum(4) + new_baud(4 LE) + cur_baud(4 LE)
        let flasher = Esp32Flasher::new();
        let cmd = flasher.change_baudrate(460800, 115200);

        // cmd[9..13] = new_baud, cmd[13..17] = cur_baud
        // 460800 = 0x00070800，115200 = 0x0001C200，均不含 0xC0/0xDB，无 SLIP 转义
        let new_baud_bytes = &cmd[9..13];
        let cur_baud_bytes = &cmd[13..17];
        assert_eq!(new_baud_bytes, 460800u32.to_le_bytes());
        assert_eq!(cur_baud_bytes, 115200u32.to_le_bytes());
    }

    #[test]
    fn test_esp32_change_baudrate_with_escape() {
        // 选取含 0xC0/0xDB 的波特率，验证 SLIP 转义生效
        let flasher = Esp32Flasher::new();
        // 0xC0 = 192；0xDB = 219；可作 LE 字节
        let new_baud = 0x0000DBC0u32; // LE 字节: 0xC0 0xDB 0x00 0x00
        let cmd = flasher.change_baudrate(new_baud, 0);
        // 包体应包含 0xDB 0xDC（来自 0xC0）和 0xDB 0xDD（来自 0xDB）
        assert!(cmd.windows(2).any(|w| w == [0xDB, 0xDC]));
        assert!(cmd.windows(2).any(|w| w == [0xDB, 0xDD]));
    }

    #[test]
    fn test_esp32_spi_attach_default() {
        let flasher = Esp32Flasher::new();
        let cmd = flasher.spi_attach(0);
        assert_eq!(cmd[0], 0xC0);
        assert_eq!(cmd[1], 0x00);
        assert_eq!(cmd[2], 0x0D); // ESP_SPI_ATTACH
        // size = 8 (LE)：ROM loader 需要两个 32 位字
        assert_eq!(cmd[3], 8);
        assert_eq!(cmd[4], 0);
        // payload 全 0（默认 SPI 配置 + ROM 附加字）
        assert_eq!(&cmd[9..17], &[0u8; 8]);
    }

    #[test]
    fn test_esp32_spi_attach_custom_config() {
        let flasher = Esp32Flasher::new();
        let config = 0xDEADBEEFu32;
        let cmd = flasher.spi_attach(config);
        // size = 8 (LE)
        assert_eq!(cmd[3], 8);
        // payload = config(LE) + 附加字 0：0xEF 0xBE 0xAD 0xDE 00 00 00 00
        // 0xDEADBEEF 的 LE 字节不含 0xC0/0xDB，无 SLIP 转义
        let config_bytes = &cmd[9..13];
        assert_eq!(config_bytes, config.to_le_bytes());
        assert_eq!(&cmd[13..17], &0u32.to_le_bytes());
    }

    #[test]
    fn test_esp32_spi_flash_md5_structure() {
        let flasher = Esp32Flasher::new();
        let cmd = flasher.spi_flash_md5(0x10000, 0x10000);
        assert_eq!(cmd[0], 0xC0);
        assert_eq!(cmd[1], 0x00);
        assert_eq!(cmd[2], 0x13); // ESP_SPI_FLASH_MD5
        // size = 16 (LE)
        assert_eq!(cmd[3], 16);
    }

    #[test]
    fn test_esp32_spi_flash_md5_payload() {
        // payload: addr(4 LE) + size(4 LE) + reserved(8 字节 0)
        let flasher = Esp32Flasher::new();
        let cmd = flasher.spi_flash_md5(0x10000, 0x10000);
        // 0x10000 = 65536，无 0xC0/0xDB
        // cmd[9..13] = addr, cmd[13..17] = size, cmd[17..25] = reserved
        let addr_bytes = &cmd[9..13];
        let size_bytes = &cmd[13..17];
        let reserved_bytes = &cmd[17..25];
        assert_eq!(addr_bytes, 0x10000u32.to_le_bytes());
        assert_eq!(size_bytes, 0x10000u32.to_le_bytes());
        assert_eq!(reserved_bytes, &[0u8; 8]);
    }

    #[test]
    fn test_esp32_parse_md5_response_ok() {
        // 构造合法响应：direction(1) + command(1) + size(2) + value(4) + md5(16) + status(2) = 26
        let mut body = Vec::with_capacity(26);
        body.push(0x01); // direction = response
        body.push(0x13); // command
        body.extend_from_slice(&18u16.to_le_bytes()); // size = md5(16) + status(2)
        body.extend_from_slice(&0u32.to_le_bytes()); // value
        let expected_md5 = [
            0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8,
            0x42, 0x7e,
        ];
        body.extend_from_slice(&expected_md5);
        body.extend_from_slice(&[0x00, 0x00]); // status = OK

        let md5 = Esp32Flasher::parse_md5_response(&body).unwrap();
        assert_eq!(md5, expected_md5);
    }

    #[test]
    fn test_esp32_parse_md5_response_too_short() {
        let body = vec![0u8; 23]; // 比 24 少 1 字节
        let err = Esp32Flasher::parse_md5_response(&body).unwrap_err();
        assert!(matches!(err, FlashError::FlashError(_)));
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn test_esp32_parse_md5_response_extra_data_ignored() {
        // 响应可携带额外字节（部分 ROM 在尾部补 0），解析应只取偏移 8..24 的 16 字节 MD5
        let mut body = Vec::with_capacity(30);
        body.push(0x01);
        body.push(0x13);
        body.extend_from_slice(&18u16.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&[0xAA; 16]); // MD5
        body.extend_from_slice(&[0xBB; 4]); // status + 额外数据
        let md5 = Esp32Flasher::parse_md5_response(&body).unwrap();
        assert_eq!(md5, [0xAA; 16]);
    }

    // -------------------------------------------------------------------
    // ESP32 read_flash 测试
    // -------------------------------------------------------------------

    #[test]
    fn test_esp32_read_flash_structure() {
        let flasher = Esp32Flasher::new();
        let cmd = flasher.read_flash(0x10000, 4096);
        assert_eq!(cmd[0], 0xC0);
        assert_eq!(cmd[1], 0x00); // direction
        assert_eq!(cmd[2], 0x07); // ESP_FLASH_READ
        // size = 16 (LE)
        assert_eq!(cmd[3], 16);
        assert_eq!(cmd[4], 0);
        assert_eq!(cmd[5], 0);
        assert_eq!(cmd[6], 0);
    }

    #[test]
    fn test_esp32_read_flash_begin() {
        let flasher = Esp32Flasher::new();
        let cmd = flasher.read_flash_begin(8192, 2);
        // 与 flash_begin 相同
        assert_eq!(cmd[0], 0xC0);
        assert_eq!(cmd[2], 0x02); // ESP_FLASH_BEGIN
    }

    // -------------------------------------------------------------------
    // STK500 测试
    // -------------------------------------------------------------------

    #[test]
    fn test_stk500_get_sync() {
        let f = Stk500Flasher::new();
        assert_eq!(f.get_sync(), vec![0x30, 0x20]);
    }

    #[test]
    fn test_stk500_enter_progmode() {
        let f = Stk500Flasher::new();
        assert_eq!(f.enter_progmode(), vec![0x50, 0x20]);
    }

    #[test]
    fn test_stk500_leave_progmode() {
        let f = Stk500Flasher::new();
        assert_eq!(f.leave_progmode(), vec![0x51, 0x20]);
    }

    #[test]
    fn test_stk500_read_sign() {
        let f = Stk500Flasher::new();
        assert_eq!(f.read_sign(), vec![0x75, 0x20]);
    }

    #[test]
    fn test_stk500_load_address() {
        let f = Stk500Flasher::new();
        // word_address = 0x1234
        let cmd = f.load_address(0x1234);
        assert_eq!(cmd, vec![0x55, 0x12, 0x34, 0x20]);
    }

    #[test]
    fn test_stk500_prog_page() {
        let f = Stk500Flasher::new();
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let cmd = f.prog_page(stk500::MEM_FLASH, &data);
        // 0x64 + len_h(0x00) + len_l(0x04) + 0x46 + data + 0x20
        assert_eq!(cmd, vec![0x64, 0x00, 0x04, 0x46, 0xDE, 0xAD, 0xBE, 0xEF, 0x20]);
    }

    #[test]
    fn test_stk500_read_page() {
        let f = Stk500Flasher::new();
        let cmd = f.read_page(stk500::MEM_FLASH, 128);
        // 0x74 + len_h(0x00) + len_l(0x80) + 0x46 + 0x20
        assert_eq!(cmd, vec![0x74, 0x00, 0x80, 0x46, 0x20]);
    }

    #[test]
    fn test_stk500_parse_sign_response_ok() {
        let resp = vec![0x14, 0x1E, 0x95, 0x0F, 0x10];
        let sign = Stk500Flasher::parse_sign_response(&resp).unwrap();
        assert_eq!(sign, [0x1E, 0x95, 0x0F]);
    }

    #[test]
    fn test_stk500_parse_sign_response_too_short() {
        let resp = vec![0x14, 0x1E, 0x95];
        assert!(Stk500Flasher::parse_sign_response(&resp).is_err());
    }

    #[test]
    fn test_stk500_parse_sign_response_no_insync() {
        let resp = vec![0x15, 0x1E, 0x95, 0x0F, 0x10];
        assert!(Stk500Flasher::parse_sign_response(&resp).is_err());
    }

    #[test]
    fn test_stk500_is_ok() {
        assert!(Stk500Flasher::is_ok(&[0x14, 0x10]));
        assert!(Stk500Flasher::is_ok(&[0x14, 0xAA, 0xBB, 0x10]));
        assert!(!Stk500Flasher::is_ok(&[0x15, 0x10]));
        assert!(!Stk500Flasher::is_ok(&[0x14]));
    }

    #[test]
    fn test_stk500_identify_atmega328p() {
        let sign = [0x1E, 0x95, 0x0F];
        let (name, kb) = Stk500Flasher::identify_signature(&sign).unwrap();
        assert_eq!(name, "ATmega328P");
        assert_eq!(kb, 32);
    }

    #[test]
    fn test_stk500_identify_atmega2560() {
        let sign = [0x1E, 0x98, 0x01];
        let (name, kb) = Stk500Flasher::identify_signature(&sign).unwrap();
        assert_eq!(name, "ATmega2560");
        assert_eq!(kb, 256);
    }

    #[test]
    fn test_stk500_identify_unknown() {
        let sign = [0xFF, 0xFF, 0xFF];
        assert!(Stk500Flasher::identify_signature(&sign).is_none());
    }
}
