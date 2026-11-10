//! SPI Flash 协议驱动
//!
//! 通过 USB 桥接器（CH341A/FT232H）的 SPI 总线直接读写外挂 SPI Flash 芯片。
//!
//! ## 支持的命令
//! - `0x9F` JEDEC ID 读取
//! - `0x03` Read Data（低速读取）
//! - `0x0B` Fast Read（带 dummy 字节）
//! - `0x05` Read Status Register-1
//! - `0x35` Read Status Register-2
//! - `0x15` Read Status Register-3 / Read Function Register
//! - `0x01` Write Status Register
//! - `0x06` Write Enable
//! - `0x04` Write Disable
//! - `0x20` Sector Erase (4KB)
//! - `0x52` Block Erase (32KB)
//! - `0xD8` Block Erase (64KB)
//! - `0xC7` Chip Erase
//! - `0x02` Page Program (256 字节页)
//! - `0xAB` Release Power-down / ID
//! - `0xB9` Power-down
//! - `0xB7` Enter 4-byte Mode（大容量 Flash ≥32MB）
//! - `0xE9` Exit 4-byte Mode
//!
//! ## 4-byte 地址模式
//! 对于 ≥32MB（256Mbit）的大容量 Flash（如 W25Q256），需要启用 4-byte 地址模式，
//! 地址字段从 3 字节扩展为 4 字节。`read_jedec_id` 后会自动检测并启用。
//!
//! ## 限制
//! - 单次 Page Program 最大 256 字节（跨页自动切分）
//! - 必须先擦除再写入（Page Program 只能 1→0，不能 0→1）
//! - Write Enable 在每次 Erase/Program 前必须发送

use crate::bus_probe::{lookup_spi_flash, SpiFlashInfo, SpiFlashVoltage};
use crate::usb_bridge::{UsbBridge, UsbBridgeError, UsbResult};
use std::time::{Duration, Instant};

/// 数据库里查不到的芯片型号占位符（界面据此给出提示，必须与语言无关）
pub const UNKNOWN_PART_NUMBER: &str = "Unknown part";

/// SPI Flash 命令码
pub mod cmd {
    pub const READ_JEDEC_ID: u8 = 0x9F;
    pub const READ_DATA: u8 = 0x03;
    pub const FAST_READ: u8 = 0x0B;
    pub const READ_STATUS: u8 = 0x05;
    pub const READ_STATUS_SR2: u8 = 0x35;
    pub const READ_STATUS_SR3: u8 = 0x15;
    pub const READ_FUNCTION_REGISTER: u8 = 0x15;
    pub const WRITE_STATUS: u8 = 0x01;
    pub const WRITE_ENABLE: u8 = 0x06;
    pub const WRITE_DISABLE: u8 = 0x04;
    pub const SECTOR_ERASE_4K: u8 = 0x20;
    pub const BLOCK_ERASE_32K: u8 = 0x52;
    pub const BLOCK_ERASE_64K: u8 = 0xD8;
    pub const CHIP_ERASE: u8 = 0xC7;
    pub const PAGE_PROGRAM: u8 = 0x02;
    pub const RELEASE_POWER_DOWN: u8 = 0xAB;
    pub const POWER_DOWN: u8 = 0xB9;
    pub const READ_MANUFACTURER_ID: u8 = 0x90;
    pub const ENTER_4BYTE_MODE: u8 = 0xB7;
    pub const EXIT_4BYTE_MODE: u8 = 0xE9;
    /// Read SFDP（Serial Flash Discoverable Parameters）
    pub const READ_SFDP: u8 = 0x5A;
}

/// 状态寄存器位
pub mod status {
    pub const WIP: u8 = 0x01; // Write In Progress
    pub const WEL: u8 = 0x02; // Write Enable Latch
    pub const BP0: u8 = 0x04;
    pub const BP1: u8 = 0x08;
    pub const BP2: u8 = 0x10;
    pub const TB: u8 = 0x20; // Top/Bottom bit
    pub const SEC: u8 = 0x40; // Sector Protect
    pub const SRP: u8 = 0x80; // Status Register Protect
}

/// 状态寄存器选择
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusRegister {
    /// Status Register-1（0x05）
    Sr1,
    /// Status Register-2（0x35）
    Sr2,
    /// Status Register-3 / Function Register（0x15）
    Sr3,
}

/// 进度回调类型：`current` 为已处理字节数，`total` 为总字节数
pub type ProgressFn = Box<dyn FnMut(u64, u64) + Send>;

/// SPI Flash 驱动
pub struct SpiFlashDriver {
    bridge: Box<dyn UsbBridge>,
    /// 已识别的芯片信息（None 表示尚未读取 JEDEC ID）
    chip: Option<SpiFlashInfo>,
    /// 页大小（默认 256 字节，几乎所有 SPI Flash 都是这个值）
    pub page_size: usize,
    /// 扇区大小（默认 4KB）
    pub sector_size: usize,
    /// 是否启用 4-byte 地址模式（用于 ≥32MB 大容量 Flash）
    addr_4byte: bool,
    /// 进度回调（可选，通过 `with_progress` 设置）
    progress_cb: Option<ProgressFn>,
}

impl SpiFlashDriver {
    /// 创建 SPI Flash 驱动
    pub fn new(bridge: Box<dyn UsbBridge>) -> Self {
        Self {
            bridge,
            chip: None,
            page_size: 256,
            sector_size: 4096,
            addr_4byte: false,
            progress_cb: None,
        }
    }

    /// 返回桥接器类型名称
    pub fn bridge_name(&self) -> &'static str {
        self.bridge.kind().name()
    }

    /// 当前是否处于 4-byte 地址模式
    pub fn is_4byte_addr(&self) -> bool {
        self.addr_4byte
    }

    /// 设置进度回调（builder 风格）
    pub fn with_progress(&mut self, cb: ProgressFn) -> &mut Self {
        self.progress_cb = Some(cb);
        self
    }

    /// 内部：调用进度回调
    fn report_progress(&mut self, current: u64, total: u64) {
        if let Some(cb) = self.progress_cb.as_mut() {
            cb(current, total);
        }
    }

    /// 内部：获取地址字段长度（3 或 4 字节）
    fn addr_len(&self) -> usize {
        if self.addr_4byte {
            4
        } else {
            3
        }
    }

    /// 内部：将地址按大端序写入命令缓冲区（3 或 4 字节）
    fn push_addr(&self, cmd: &mut Vec<u8>, addr: u32) {
        if self.addr_4byte {
            cmd.push((addr >> 24) as u8);
            cmd.push((addr >> 16) as u8);
            cmd.push((addr >> 8) as u8);
            cmd.push(addr as u8);
        } else {
            cmd.push((addr >> 16) as u8);
            cmd.push((addr >> 8) as u8);
            cmd.push(addr as u8);
        }
    }

    // ====================================================================
    // 4-byte 地址模式
    // ====================================================================

    /// 进入 4-byte 地址模式（发送 0xB7）
    ///
    /// 适用于 W25Q256 等 ≥32MB 大容量 Flash。启用后所有地址相关命令
    /// 自动切换为 4 字节传输。
    pub fn enable_4byte_addr(&mut self) -> UsbResult<()> {
        self.bridge.spi_write(&[cmd::ENTER_4BYTE_MODE])?;
        self.addr_4byte = true;
        Ok(())
    }

    /// 退出 4-byte 地址模式（发送 0xE9）
    pub fn disable_4byte_addr(&mut self) -> UsbResult<()> {
        self.bridge.spi_write(&[cmd::EXIT_4BYTE_MODE])?;
        self.addr_4byte = false;
        Ok(())
    }

    /// 读取 4-byte 地址模式状态（通过 0x15 Read Function Register 的 ADS 位 bit0）
    pub fn read_4byte_addr_mode(&mut self) -> UsbResult<bool> {
        let cmd = [cmd::READ_FUNCTION_REGISTER, 0x00];
        let resp = self.bridge.spi_xfer(&cmd)?;
        if resp.len() < 2 {
            return Err(UsbBridgeError::InvalidResponse(format!(
                "Function register response too short: {} bytes",
                resp.len()
            )));
        }
        // ADS 位为 bit0
        Ok(resp[1] & 0x01 != 0)
    }

    // ====================================================================
    // 状态寄存器
    // ====================================================================

    /// 读取状态寄存器（SR1/SR2/SR3）
    ///
    /// - Sr1 → 0x05
    /// - Sr2 → 0x35
    /// - Sr3 → 0x15（部分芯片为 Function Register）
    pub fn read_status_register(&mut self, reg: StatusRegister) -> UsbResult<u8> {
        let cmd_byte = match reg {
            StatusRegister::Sr1 => cmd::READ_STATUS,
            StatusRegister::Sr2 => cmd::READ_STATUS_SR2,
            StatusRegister::Sr3 => cmd::READ_STATUS_SR3,
        };
        let cmd = [cmd_byte, 0x00];
        let resp = self.bridge.spi_xfer(&cmd)?;
        if resp.len() < 2 {
            return Err(UsbBridgeError::InvalidResponse(format!(
                "Status register response too short: {} bytes",
                resp.len()
            )));
        }
        Ok(resp[1])
    }

    /// 读取状态寄存器 SR1（兼容旧 API，等价于 `read_status_register(Sr1)`）
    pub fn read_status(&mut self) -> UsbResult<u8> {
        self.read_status_register(StatusRegister::Sr1)
    }

    /// 写入状态寄存器（0x01 Write Status Register）
    ///
    /// `data` 长度 1-3 字节，分别对应 SR1 / SR1+SR2 / SR1+SR2+SR3。
    /// 注意：调用前需先执行 `write_enable()`。
    pub fn write_status(&mut self, data: &[u8]) -> UsbResult<()> {
        if data.is_empty() || data.len() > 3 {
            return Err(UsbBridgeError::InvalidResponse(format!(
                "Write status data length must be 1-3 bytes, got {}",
                data.len()
            )));
        }
        let mut cmd = Vec::with_capacity(1 + data.len());
        cmd.push(cmd::WRITE_STATUS);
        cmd.extend_from_slice(data);
        self.bridge.spi_write(&cmd)?;
        Ok(())
    }

    /// 清除块保护位（BP/BP2/BP3/TB/BP4 等）
    ///
    /// 流程：WREN → write_status(0x00) → wait_ready
    pub fn clear_block_protect(&mut self) -> UsbResult<()> {
        self.write_enable()?;
        self.write_status(&[0x00])?;
        self.wait_ready(Duration::from_millis(50))?;
        Ok(())
    }

    /// 读取 WIP（Write In Progress）位，判断芯片是否忙碌
    pub fn is_busy(&mut self) -> UsbResult<bool> {
        let s = self.read_status_register(StatusRegister::Sr1)?;
        Ok(s & status::WIP != 0)
    }

    // ====================================================================
    // JEDEC ID 识别
    // ====================================================================

    /// 读取 JEDEC ID 并识别芯片
    ///
    /// 识别后若芯片容量 ≥32MB 且支持 4-byte 地址模式，自动启用。
    pub fn read_jedec_id(&mut self) -> UsbResult<SpiFlashInfo> {
        // 发送 0x9F，读取 3 字节：MFG + CAP_HI + CAP_LO
        let cmd = [cmd::READ_JEDEC_ID, 0x00, 0x00, 0x00];
        let resp = self.bridge.spi_xfer(&cmd)?;
        if resp.len() < 4 {
            return Err(UsbBridgeError::InvalidResponse(format!(
                "JEDEC ID response too short: {} bytes (need 4)",
                resp.len()
            )));
        }
        let mfg = resp[1];
        let cap_hi = resp[2];
        let cap_lo = resp[3];
        let cap = ((cap_hi as u16) << 8) | (cap_lo as u16);

        let chip = lookup_spi_flash(mfg, cap).unwrap_or(SpiFlashInfo {
            manufacturer: crate::bus_probe::spi_flash_vendor_name(mfg),
            part_number: UNKNOWN_PART_NUMBER,
            capacity_mbit: 0,
            jedec_id: ((mfg as u32) << 16) | (cap as u32),
            supports_4byte_addr: false,
            voltage: SpiFlashVoltage::V3_3,
        });

        self.chip = Some(chip.clone());

        // 自动检测并启用 4-byte 地址模式（≥32MB 且芯片支持）
        let capacity_bytes = (chip.capacity_mbit as u64) * 1024 * 1024 / 8;
        if chip.supports_4byte_addr && capacity_bytes >= 32 * 1024 * 1024 {
            self.enable_4byte_addr()?;
        }

        Ok(chip)
    }

    /// 读取 SFDP（Serial Flash Discoverable Parameters）
    ///
    /// SFDP 是 JESD216 标准，现代 SPI Flash（2013 年后）均支持。
    /// 通过 0x5A 命令读取，可获取芯片容量、扇区大小、写入粒度等参数。
    ///
    /// 返回原始 SFDP 数据（从 SFDP Header 开始）。
    pub fn read_sfdp(&mut self, addr: u32, len: usize) -> UsbResult<Vec<u8>> {
        let mut cmd = Vec::with_capacity(5 + len);
        cmd.push(cmd::READ_SFDP);
        cmd.push((addr >> 16) as u8);
        cmd.push((addr >> 8) as u8);
        cmd.push(addr as u8);
        cmd.push(0x00); // Dummy byte
        cmd.extend(std::iter::repeat(0u8).take(len));
        let resp = self.bridge.spi_xfer(&cmd)?;
        // 返回的数据前 5 字节是命令回显，跳过
        if resp.len() < 5 + len {
            return Err(UsbBridgeError::InvalidResponse(format!(
                "SFDP response too short: {} bytes (need {})",
                resp.len(),
                5 + len
            )));
        }
        Ok(resp[5..5 + len].to_vec())
    }

    /// 从 SFDP 解析芯片容量（JESD216B+）
    ///
    /// 读取 SFDP Header → 找到 JEDEC Parameter Header →
    /// 读取 Parameter Table → 解析第 4-7 字节的 Density 字段
    ///
    /// Density 字段：
    ///   - bit 31 = 0：容量 = 2^N 字节（N = density & 0x7FFFFFFF）
    ///   - bit 31 = 1：容量 = (density & 0x7FFFFFFF) * 2 字节
    pub fn read_sfdp_capacity(&mut self) -> UsbResult<u64> {
        // 1. 读取 SFDP Header（8 字节）
        let header = self.read_sfdp(0x0000, 8)?;
        // 验证签名 "SFDP" (0x50 0x44 0x46 0x53)
        if header.len() < 8 || header[0..4] != [b'S', b'F', b'D', b'P'] {
            return Err(UsbBridgeError::InvalidResponse(
                "SFDP signature not found".to_string(),
            ));
        }
        // header[4] = SFDP Major Revision (应 ≥ 1)
        // header[5] = SFDP Minor Revision
        // header[6] = Number of Parameter Headers (NPH)，0 表示 1 个
        let nph = header[6] as usize;
        // header[7] = SFDP Access Protocol

        // 2. 读取第一个 Parameter Header（8 字节，偏移 0x08）
        let ph = self.read_sfdp(0x08, 8)?;
        if ph.len() < 8 {
            return Err(UsbBridgeError::InvalidResponse(
                "SFDP parameter header too short".to_string(),
            ));
        }
        // ph[0] = Parameter ID LSB (0x00 = JEDEC)
        // ph[1] = Parameter Minor Revision
        // ph[2] = Parameter Major Revision (应 ≥ 1)
        // ph[3] = Parameter Table Length (16-bit words)
        // ph[4..7] = Parameter Table Pointer (3 字节 LE，指向 Table 的字节地址)
        let table_ptr = (ph[4] as u32) | ((ph[5] as u32) << 8) | ((ph[6] as u32) << 16);

        // 3. 读取 Parameter Table（至少 8 字节以获取 Density）
        let table = self.read_sfdp(table_ptr, 8)?;
        if table.len() < 8 {
            return Err(UsbBridgeError::InvalidResponse(
                "SFDP parameter table too short".to_string(),
            ));
        }

        // 4. 解析 Density（Table 的第 4-7 字节，小端序 u32）
        let density = u32::from_le_bytes([table[4], table[5], table[6], table[7]]);

        let capacity = if density & 0x8000_0000 != 0 {
            // bit 31 = 1：非 2 幂容量，JESD216 规定密度 = (bits[30:0] + 1) 比特
            ((density & 0x7FFF_FFFF) as u64 + 1) / 8
        } else {
            // bit 31 = 0：密度 = 2^N **比特**（N = bits[30:0]），换算字节需右移 3 位
            let n = density & 0x7FFF_FFFF;
            if !(3..=40).contains(&n) {
                return Err(UsbBridgeError::InvalidResponse(format!(
                    "SFDP density exponent out of range: {}",
                    n
                )));
            }
            1u64 << (n - 3)
        };

        let _ = nph; // 保留 NPH 信息
        Ok(capacity)
    }

    /// 读取 RES（Read Electronic Signature，0xAB 命令）
    ///
    /// 旧式 SPI Flash（如 AMIC A25L 系列）可能不支持 JEDEC ID (0x9F)，
    /// 但支持 0xAB 返回 1 字节电子签名（Electronic Signature）。
    ///
    /// 命令格式：0xAB + 3 字节 dummy + 1 字节 ES
    pub fn read_electronic_signature(&mut self) -> UsbResult<u8> {
        let cmd = [cmd::RELEASE_POWER_DOWN, 0x00, 0x00, 0x00, 0x00];
        let resp = self.bridge.spi_xfer(&cmd)?;
        if resp.len() < 5 {
            return Err(UsbBridgeError::InvalidResponse(format!(
                "RES response too short: {} bytes (need 5)",
                resp.len()
            )));
        }
        Ok(resp[4])
    }

    /// 读取 Manufacturer ID 和 Device ID（0x90 命令）
    ///
    /// 0x90 是另一种芯片识别命令，返回 2 字节：
    ///   - 第 1 字节：厂商 ID（与 JEDEC ID 的 MFG 相同）
    ///   - 第 2 字节：设备 ID（与 JEDEC ID 的容量 ID 可能不同）
    ///
    /// 可用作 JEDEC ID (0x9F) 的交叉验证。
    pub fn read_manufacturer_id_90(&mut self) -> UsbResult<(u8, u8)> {
        let cmd = [cmd::READ_MANUFACTURER_ID, 0x00, 0x00, 0x00, 0x00, 0x00];
        let resp = self.bridge.spi_xfer(&cmd)?;
        if resp.len() < 6 {
            return Err(UsbBridgeError::InvalidResponse(format!(
                "0x90 response too short: {} bytes (need 6)",
                resp.len()
            )));
        }
        Ok((resp[4], resp[5]))
    }

    /// 综合芯片识别（多策略后备链）
    ///
    /// 识别顺序：
    /// 1. JEDEC ID (0x9F) → 查表识别厂商/型号/容量
    /// 2. SFDP (0x5A) → 自动检测容量（即使 JEDEC ID 不在表中也能获取容量）
    /// 3. RES (0xAB) → 旧式芯片电子签名
    ///
    /// 返回识别结果和使用的识别方法描述。
    pub fn identify_chip(&mut self) -> UsbResult<(SpiFlashInfo, &'static str)> {
        // 策略 1：JEDEC ID（最可靠）
        let chip = self.read_jedec_id()?;
        if chip.capacity_mbit > 0 {
            return Ok((chip, "JEDEC ID (0x9F)"));
        }

        // JEDEC ID 不在表中，尝试 SFDP 补充容量信息
        if let Ok(sfdp_capacity) = self.read_sfdp_capacity() {
            if sfdp_capacity > 0 {
                let sfdp_mbit = (sfdp_capacity * 8) / (1024 * 1024);
                let mut chip_with_sfdp = chip.clone();
                chip_with_sfdp.capacity_mbit = sfdp_mbit as u32;
                // SFDP 检测到大容量芯片，假设支持 4-byte 地址
                if sfdp_capacity >= 32 * 1024 * 1024 {
                    chip_with_sfdp.supports_4byte_addr = true;
                    self.enable_4byte_addr()?;
                }
                self.chip = Some(chip_with_sfdp.clone());
                return Ok((chip_with_sfdp, "JEDEC ID + SFDP (0x5A)"));
            }
        }

        // 策略 2：仅 SFDP（JEDEC ID 全 0x00 或 0xFF 的极端情况）
        // 上面已尝试，此处不再重复

        // 策略 3：RES（旧式芯片）
        // RES 仅返回 1 字节，信息量有限，但至少能确认芯片存在
        let _es = self.read_electronic_signature().ok();

        Ok((chip, "JEDEC ID (unrecognized capacity)"))
    }

    /// 写使能
    pub fn write_enable(&mut self) -> UsbResult<()> {
        self.bridge.spi_write(&[cmd::WRITE_ENABLE])?;
        // 验证 WEL 位
        std::thread::sleep(Duration::from_millis(1));
        let status = self.read_status()?;
        if status & status::WEL == 0 {
            return Err(UsbBridgeError::InvalidResponse(
                "WEL not set after Write Enable".to_string(),
            ));
        }
        Ok(())
    }

    /// 写禁止
    pub fn write_disable(&mut self) -> UsbResult<()> {
        self.bridge.spi_write(&[cmd::WRITE_DISABLE])?;
        Ok(())
    }

    /// 等待 WIP（Write In Progress）位清零
    /// 超时返回错误
    pub fn wait_ready(&mut self, timeout: Duration) -> UsbResult<()> {
        let start = Instant::now();
        loop {
            if !self.is_busy()? {
                return Ok(());
            }
            if start.elapsed() > timeout {
                return Err(UsbBridgeError::TransferFailed(format!(
                    "Timeout waiting for WIP clear ({}ms)",
                    timeout.as_millis()
                )));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    // ====================================================================
    // 读取
    // ====================================================================

    /// 读取 Flash 数据（普通 Read Data 命令 0x03）
    ///
    /// 内部分块传输（每块 ≤ 4KB），避免单次传输过大。
    /// 4-byte 地址模式启用时，地址字段自动扩展为 4 字节。
    pub fn read_flash(&mut self, addr: u32, len: usize) -> UsbResult<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let mut result = Vec::with_capacity(len);
        let mut current = addr;
        // CH341A 单次 spi_xfer 上限 0x1000 字节（含命令头），分块必须扣掉头部
        let header = 1 + self.addr_len();
        let chunk_max = 0x1000 - header;
        let mut read = 0usize;
        while read < len {
            let chunk_len = (len - read).min(chunk_max);
            let mut cmd = Vec::with_capacity(header + chunk_len);
            cmd.push(cmd::READ_DATA);
            self.push_addr(&mut cmd, current);
            cmd.extend(std::iter::repeat(0x00u8).take(chunk_len));

            let resp = self.bridge.spi_xfer(&cmd)?;
            if resp.len() < header + chunk_len {
                return Err(UsbBridgeError::InvalidResponse(format!(
                    "Read response too short at addr={:#010X}: got {} bytes, need {}",
                    current,
                    resp.len(),
                    header + chunk_len
                )));
            }
            result.extend_from_slice(&resp[header..header + chunk_len]);

            current = current.wrapping_add(chunk_len as u32);
            read += chunk_len;
            self.report_progress(read as u64, len as u64);
        }

        Ok(result)
    }

    /// Fast Read（0x0B）—带 dummy 字节的高速读取
    /// 适用于需要 >33MHz 时钟的场景
    pub fn fast_read(&mut self, addr: u32, len: usize) -> UsbResult<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let mut result = Vec::with_capacity(len);
        let mut current = addr;
        // CH341A 单次 spi_xfer 上限 0x1000 字节（含命令头），分块必须扣掉头部
        let header = 1 + self.addr_len() + 1; // cmd + addr + dummy
        let chunk_max = 0x1000 - header;
        let mut read = 0usize;
        while read < len {
            let chunk_len = (len - read).min(chunk_max);
            // 0x0B + N 字节地址 + 1 dummy + 0x00 × chunk_len
            let mut cmd = Vec::with_capacity(header + chunk_len);
            cmd.push(cmd::FAST_READ);
            self.push_addr(&mut cmd, current);
            cmd.push(0x00); // dummy byte
            cmd.extend(std::iter::repeat(0x00u8).take(chunk_len));

            let resp = self.bridge.spi_xfer(&cmd)?;
            if resp.len() < header + chunk_len {
                return Err(UsbBridgeError::InvalidResponse(format!(
                    "Fast Read response too short at addr={:#010X}",
                    current
                )));
            }
            result.extend_from_slice(&resp[header..header + chunk_len]);
            current = current.wrapping_add(chunk_len as u32);
            read += chunk_len;
            self.report_progress(read as u64, len as u64);
        }
        Ok(result)
    }

    // ====================================================================
    // 擦除
    // ====================================================================

    /// 扇区擦除（4KB）
    pub fn sector_erase(&mut self, addr: u32) -> UsbResult<()> {
        self.write_enable()?;
        let mut cmd = Vec::with_capacity(1 + self.addr_len());
        cmd.push(cmd::SECTOR_ERASE_4K);
        self.push_addr(&mut cmd, addr);
        self.bridge.spi_write(&cmd)?;
        // 4KB 扇区擦除典型 400ms，最大 6s（W25Q128）
        self.wait_ready(Duration::from_secs(6))?;
        Ok(())
    }

    /// 32KB 块擦除
    pub fn block_erase_32k(&mut self, addr: u32) -> UsbResult<()> {
        self.write_enable()?;
        let mut cmd = Vec::with_capacity(1 + self.addr_len());
        cmd.push(cmd::BLOCK_ERASE_32K);
        self.push_addr(&mut cmd, addr);
        self.bridge.spi_write(&cmd)?;
        self.wait_ready(Duration::from_secs(25))?;
        Ok(())
    }

    /// 64KB 块擦除
    pub fn block_erase_64k(&mut self, addr: u32) -> UsbResult<()> {
        self.write_enable()?;
        let mut cmd = Vec::with_capacity(1 + self.addr_len());
        cmd.push(cmd::BLOCK_ERASE_64K);
        self.push_addr(&mut cmd, addr);
        self.bridge.spi_write(&cmd)?;
        self.wait_ready(Duration::from_secs(50))?;
        Ok(())
    }

    /// 整片擦除（耗时较长，可能数十秒到数分钟）
    pub fn chip_erase(&mut self) -> UsbResult<()> {
        self.report_progress(0, 1);
        self.write_enable()?;
        self.bridge.spi_write(&[cmd::CHIP_ERASE])?;
        // W25Q128 整片擦除典型 40s，最大 200s
        self.wait_ready(Duration::from_secs(240))?;
        self.report_progress(1, 1);
        Ok(())
    }

    /// 按地址范围擦除（自动选择扇区/块）
    /// 会从 start_addr 开始，每次擦除一个 sector/block，直到覆盖 end_addr
    pub fn erase_range(&mut self, start_addr: u32, end_addr: u32) -> UsbResult<()> {
        if end_addr < start_addr {
            return Err(UsbBridgeError::InvalidResponse(format!(
                "end_addr {:#010X} < start_addr {:#010X}",
                end_addr, start_addr
            )));
        }
        let total = (end_addr - start_addr + 1) as u64;
        let mut addr = start_addr;
        while addr <= end_addr {
            // 优先用 64KB 块擦除（如果地址对齐且剩余足够）
            if addr % 0x10000 == 0 && addr + 0x10000 - 1 <= end_addr {
                self.block_erase_64k(addr)?;
                addr += 0x10000;
            } else if addr % 0x8000 == 0 && addr + 0x8000 - 1 <= end_addr {
                self.block_erase_32k(addr)?;
                addr += 0x8000;
            } else {
                self.sector_erase(addr)?;
                addr += self.sector_size as u32;
            }
            let done = (addr - start_addr) as u64;
            self.report_progress(done.min(total), total);
        }
        Ok(())
    }

    // ====================================================================
    // 编程
    // ====================================================================

    /// 页编程（Page Program，0x02）
    /// 单次最大 256 字节，且不能跨页（自动切分）
    pub fn page_program(&mut self, addr: u32, data: &[u8]) -> UsbResult<()> {
        if data.is_empty() {
            return Ok(());
        }
        let mut offset = 0usize;
        let mut current = addr;
        while offset < data.len() {
            // 计算当前页剩余空间
            let page_offset = (current % self.page_size as u32) as usize;
            let page_remaining = self.page_size - page_offset;
            let chunk_len = (data.len() - offset).min(page_remaining).min(self.page_size);

            self.write_enable()?;
            // 构造命令：0x02 + N 字节地址 + data
            let header = 1 + self.addr_len();
            let mut cmd = Vec::with_capacity(header + chunk_len);
            cmd.push(cmd::PAGE_PROGRAM);
            self.push_addr(&mut cmd, current);
            cmd.extend_from_slice(&data[offset..offset + chunk_len]);
            self.bridge.spi_write(&cmd)?;

            // 等待页编程完成（典型 0.7ms，最大 3ms）
            self.wait_ready(Duration::from_millis(10))?;

            offset += chunk_len;
            current = current.wrapping_add(chunk_len as u32);
            self.report_progress(offset as u64, data.len() as u64);
        }
        Ok(())
    }

    /// 写入固件数据（擦除 + 编程）
    /// 自动按 sector 边界擦除，再按 page 编程
    pub fn write_flash(&mut self, addr: u32, data: &[u8]) -> UsbResult<()> {
        if data.is_empty() {
            return Ok(());
        }
        let end_addr = addr + data.len() as u32 - 1;
        // 1. 擦除覆盖范围
        self.erase_range(addr, end_addr)?;
        // 2. 按页写入
        self.page_program(addr, data)?;
        Ok(())
    }

    // ====================================================================
    // 写入校验
    // ====================================================================

    /// 校验 Flash 数据（read-back verify）
    ///
    /// 读取 `addr` 处数据与 `expected` 逐字节对比，
    /// 首个不匹配返回 `UsbBridgeError::VerifyFailed`。
    pub fn verify_flash(&mut self, addr: u32, expected: &[u8]) -> UsbResult<()> {
        if expected.is_empty() {
            return Ok(());
        }
        let actual = self.read_flash(addr, expected.len())?;
        for (offset, (&exp, &got)) in expected.iter().zip(actual.iter()).enumerate() {
            if exp != got {
                return Err(UsbBridgeError::VerifyFailed {
                    offset,
                    expected: exp,
                    got,
                });
            }
        }
        Ok(())
    }

    /// 写入并校验（write_flash + verify_flash）
    ///
    /// 失败时返回 `UsbBridgeError::VerifyFailed { offset, expected, got }`。
    pub fn write_flash_with_verify(&mut self, addr: u32, data: &[u8]) -> UsbResult<()> {
        self.write_flash(addr, data)?;
        self.verify_flash(addr, data)?;
        Ok(())
    }

    /// 通用写入（可选校验）
    ///
    /// `verify=true` 等价于 `write_flash_with_verify`，
    /// `verify=false` 等价于 `write_flash`。
    pub fn write_flash_verified(&mut self, addr: u32, data: &[u8], verify: bool) -> UsbResult<()> {
        if verify {
            self.write_flash_with_verify(addr, data)
        } else {
            self.write_flash(addr, data)
        }
    }

    // ====================================================================
    // 电源管理
    // ====================================================================

    /// 进入 Power-down 模式（低功耗）
    pub fn power_down(&mut self) -> UsbResult<()> {
        self.bridge.spi_write(&[cmd::POWER_DOWN])?;
        Ok(())
    }

    /// 唤醒（释放 Power-down）
    pub fn release_power_down(&mut self) -> UsbResult<()> {
        self.bridge.spi_write(&[cmd::RELEASE_POWER_DOWN])?;
        std::thread::sleep(Duration::from_millis(3));
        Ok(())
    }

    // ====================================================================
    // 其他
    // ====================================================================

    /// 读取制造商 ID（0x90，旧式接口）
    /// 返回 (manufacturer_id, device_id)
    pub fn read_manufacturer_id(&mut self) -> UsbResult<(u8, u8)> {
        // 0x90 + 3 字节地址(0x000000) + 2 字节读取
        let cmd = [cmd::READ_MANUFACTURER_ID, 0x00, 0x00, 0x00, 0x00, 0x00];
        let resp = self.bridge.spi_xfer(&cmd)?;
        if resp.len() < 6 {
            return Err(UsbBridgeError::InvalidResponse(format!(
                "Manufacturer ID response too short: {} bytes",
                resp.len()
            )));
        }
        Ok((resp[4], resp[5]))
    }

    /// 已识别的芯片信息
    pub fn chip_info(&self) -> Option<&SpiFlashInfo> {
        self.chip.as_ref()
    }

    /// 估算芯片容量（字节）
    pub fn capacity_bytes(&self) -> Option<u64> {
        self.chip
            .as_ref()
            .map(|c| (c.capacity_mbit as u64) * 1024 * 1024 / 8)
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb_bridge::BridgeKind;
    use std::sync::{Arc, Mutex};

    /// 模拟桥接器：用于单元测试（不依赖真实硬件）
    struct MockBridge {
        responses: Vec<Vec<u8>>,
        call_index: usize,
    }

    impl MockBridge {
        fn new(responses: Vec<Vec<u8>>) -> Self {
            Self {
                responses,
                call_index: 0,
            }
        }
    }

    impl UsbBridge for MockBridge {
        fn kind(&self) -> BridgeKind {
            BridgeKind::Ch341A
        }

        fn spi_xfer(&mut self, _data_out: &[u8]) -> UsbResult<Vec<u8>> {
            if self.call_index < self.responses.len() {
                let r = self.responses[self.call_index].clone();
                self.call_index += 1;
                Ok(r)
            } else {
                Ok(vec![0xFF; 32])
            }
        }

        fn set_cs(&mut self, _asserted: bool) -> UsbResult<()> {
            Ok(())
        }

        fn close(&mut self) {}
    }

    /// 可记录命令的模拟桥接器：用于验证发出的 SPI 命令字节
    struct RecordingBridge {
        responses: Vec<Vec<u8>>,
        call_index: usize,
        recorded: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl RecordingBridge {
        fn new(responses: Vec<Vec<u8>>, recorded: Arc<Mutex<Vec<Vec<u8>>>>) -> Self {
            Self {
                responses,
                call_index: 0,
                recorded,
            }
        }
    }

    impl UsbBridge for RecordingBridge {
        fn kind(&self) -> BridgeKind {
            BridgeKind::Ch341A
        }

        fn spi_xfer(&mut self, data_out: &[u8]) -> UsbResult<Vec<u8>> {
            self.recorded.lock().unwrap().push(data_out.to_vec());
            if self.call_index < self.responses.len() {
                let r = self.responses[self.call_index].clone();
                self.call_index += 1;
                Ok(r)
            } else {
                Ok(vec![0xFF; 32])
            }
        }

        fn set_cs(&mut self, _asserted: bool) -> UsbResult<()> {
            Ok(())
        }

        fn close(&mut self) {}
    }

    // ---- 原有命令码/状态位测试 ----

    #[test]
    fn test_command_codes() {
        assert_eq!(cmd::READ_JEDEC_ID, 0x9F);
        assert_eq!(cmd::READ_DATA, 0x03);
        assert_eq!(cmd::FAST_READ, 0x0B);
        assert_eq!(cmd::READ_STATUS, 0x05);
        assert_eq!(cmd::WRITE_ENABLE, 0x06);
        assert_eq!(cmd::SECTOR_ERASE_4K, 0x20);
        assert_eq!(cmd::BLOCK_ERASE_32K, 0x52);
        assert_eq!(cmd::BLOCK_ERASE_64K, 0xD8);
        assert_eq!(cmd::CHIP_ERASE, 0xC7);
        assert_eq!(cmd::PAGE_PROGRAM, 0x02);
        assert_eq!(cmd::POWER_DOWN, 0xB9);
        assert_eq!(cmd::RELEASE_POWER_DOWN, 0xAB);
    }

    #[test]
    fn test_4byte_command_codes() {
        assert_eq!(cmd::ENTER_4BYTE_MODE, 0xB7);
        assert_eq!(cmd::EXIT_4BYTE_MODE, 0xE9);
        assert_eq!(cmd::READ_FUNCTION_REGISTER, 0x15);
        assert_eq!(cmd::READ_STATUS_SR2, 0x35);
        assert_eq!(cmd::READ_STATUS_SR3, 0x15);
    }

    #[test]
    fn test_status_bits() {
        assert_eq!(status::WIP, 0x01);
        assert_eq!(status::WEL, 0x02);
        assert_eq!(status::SRP, 0x80);
    }

    // ---- 原有 JEDEC ID 测试 ----

    #[test]
    fn test_read_jedec_id_w25q128() {
        // 模拟 W25Q128FV：MFG=0xEF, CAP_HI=0x40, CAP_LO=0x17
        let mock = MockBridge::new(vec![vec![0x9F, 0xEF, 0x40, 0x17]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let chip = driver.read_jedec_id().unwrap();
        assert_eq!(chip.manufacturer, "Winbond");
        assert_eq!(chip.part_number, "W25Q128FV");
        assert_eq!(chip.capacity_mbit, 128);
        assert_eq!(chip.jedec_id, 0xEF4017);
        // 128Mbit = 16MB < 32MB，不应自动启用 4-byte 模式
        assert!(!driver.is_4byte_addr());
    }

    #[test]
    fn test_read_jedec_id_unknown_chip() {
        // 未知芯片：MFG=0xAA, CAP=0xBBCC
        let mock = MockBridge::new(vec![vec![0x9F, 0xAA, 0xBB, 0xCC]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let chip = driver.read_jedec_id().unwrap();
        assert_eq!(chip.part_number, UNKNOWN_PART_NUMBER);
        assert_eq!(chip.jedec_id, 0xAABBCC);
        assert!(!chip.supports_4byte_addr);
    }

    #[test]
    fn test_read_status() {
        // 模拟返回 [0x05, 0x00]：状态寄存器 = 0（无 WIP/WEL）
        let mock = MockBridge::new(vec![vec![0x05, 0x00]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let s = driver.read_status().unwrap();
        assert_eq!(s, 0x00);
    }

    #[test]
    fn test_capacity_bytes() {
        let mock = MockBridge::new(vec![vec![0x9F, 0xEF, 0x40, 0x17]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver.read_jedec_id().unwrap();

        // 128 Mbit = 16 MB = 16 * 1024 * 1024 bytes
        assert_eq!(driver.capacity_bytes(), Some(16 * 1024 * 1024));
    }

    #[test]
    fn test_read_flash_empty_len() {
        let mock = MockBridge::new(vec![]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let data = driver.read_flash(0x000000, 0).unwrap();
        assert!(data.is_empty());
    }

    #[test]
    fn test_page_program_empty() {
        let mock = MockBridge::new(vec![]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        driver.page_program(0x000000, &[]).unwrap();
    }

    #[test]
    fn test_erase_range_invalid() {
        let mock = MockBridge::new(vec![]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        // end < start 应返回错误
        let r = driver.erase_range(0x1000, 0x0000);
        assert!(r.is_err());
    }

    #[test]
    fn test_bridge_name() {
        let mock = MockBridge::new(vec![]);
        let driver = SpiFlashDriver::new(Box::new(mock));
        assert_eq!(driver.bridge_name(), "CH341A");
    }

    #[test]
    fn test_chip_info_initially_none() {
        let mock = MockBridge::new(vec![]);
        let driver = SpiFlashDriver::new(Box::new(mock));
        assert!(driver.chip_info().is_none());
    }

    // ---- 4-byte 地址模式测试 ----

    #[test]
    fn test_enable_4byte_addr() {
        let mock = MockBridge::new(vec![vec![0xB7]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        assert!(!driver.is_4byte_addr());
        driver.enable_4byte_addr().unwrap();
        assert!(driver.is_4byte_addr());
    }

    #[test]
    fn test_disable_4byte_addr() {
        let mock = MockBridge::new(vec![vec![0xB7], vec![0xE9]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver.enable_4byte_addr().unwrap();
        assert!(driver.is_4byte_addr());
        driver.disable_4byte_addr().unwrap();
        assert!(!driver.is_4byte_addr());
    }

    #[test]
    fn test_read_4byte_addr_mode() {
        // ADS=1（4-byte 模式已启用）
        let mock = MockBridge::new(vec![vec![0x15, 0x01]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        assert!(driver.read_4byte_addr_mode().unwrap());

        // ADS=0（3-byte 模式）
        let mock = MockBridge::new(vec![vec![0x15, 0x00]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        assert!(!driver.read_4byte_addr_mode().unwrap());
    }

    #[test]
    fn test_read_jedec_id_w25q256_auto_4byte() {
        // W25Q256FV: MFG=0xEF, CAP_HI=0x40, CAP_LO=0x18 → 256Mbit = 32MB
        // JEDEC 响应 + Enter 4-byte mode 响应（spi_write 内部调用 spi_xfer）
        let mock = MockBridge::new(vec![
            vec![0x9F, 0xEF, 0x40, 0x18],
            vec![0xB7],
        ]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let chip = driver.read_jedec_id().unwrap();
        assert_eq!(chip.part_number, "W25Q256FV");
        assert_eq!(chip.capacity_mbit, 256);
        assert!(chip.supports_4byte_addr);
        // 容量 ≥32MB 且支持 4-byte → 自动启用
        assert!(driver.is_4byte_addr());
    }

    #[test]
    fn test_read_jedec_id_w25q256_no_auto_when_unsupported() {
        // 未知 256Mbit 芯片（不在表中，supports_4byte_addr=false）
        let mock = MockBridge::new(vec![vec![0x9F, 0xFF, 0x40, 0x18]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let chip = driver.read_jedec_id().unwrap();
        assert_eq!(chip.part_number, UNKNOWN_PART_NUMBER);
        assert!(!chip.supports_4byte_addr);
        // 不支持 4-byte → 不自动启用
        assert!(!driver.is_4byte_addr());
    }

    #[test]
    fn test_read_flash_4byte_addr() {
        // 启用 4-byte 模式后读取 4 字节
        // 命令长度: 1(cmd) + 4(addr) + 4(data) = 9 字节
        // 响应前 5 字节为 echo，后 4 字节为数据
        let mock = MockBridge::new(vec![
            vec![0xB7], // Enter 4-byte mode
            vec![0x03, 0x00, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF],
        ]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver.enable_4byte_addr().unwrap();

        let data = driver.read_flash(0x00000000, 4).unwrap();
        assert_eq!(data, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_read_flash_3byte_addr() {
        // 3-byte 模式读取 4 字节
        // 命令长度: 1(cmd) + 3(addr) + 4(data) = 8 字节
        let mock = MockBridge::new(vec![
            vec![0x03, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF],
        ]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        assert!(!driver.is_4byte_addr());

        let data = driver.read_flash(0x000000, 4).unwrap();
        assert_eq!(data, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_read_flash_4byte_addr_command_structure() {
        // 使用 RecordingBridge 验证 4-byte 模式下命令字节结构
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let mock = RecordingBridge::new(
            vec![
                vec![0xB7], // Enter 4-byte mode
                // read_flash 响应: cmd + 4-byte addr + 4 data bytes = 9 bytes
                vec![0x03, 0x01, 0x02, 0x03, 0x04, 0xDE, 0xAD, 0xBE, 0xEF],
            ],
            recorded.clone(),
        );
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver.enable_4byte_addr().unwrap();
        let data = driver.read_flash(0x01020304, 4).unwrap();
        assert_eq!(data, vec![0xDE, 0xAD, 0xBE, 0xEF]);

        let cmds = recorded.lock().unwrap();
        // cmds[0] = [0xB7] (Enter 4-byte mode)
        // cmds[1] = [0x03, 0x01, 0x02, 0x03, 0x04, 0x00, 0x00, 0x00, 0x00]
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[1][0], 0x03); // READ_DATA
        assert_eq!(cmds[1][1], 0x01); // addr MSB
        assert_eq!(cmds[1][2], 0x02);
        assert_eq!(cmds[1][3], 0x03);
        assert_eq!(cmds[1][4], 0x04); // addr LSB
        assert_eq!(cmds[1].len(), 9); // 1 + 4 + 4
    }

    #[test]
    fn test_read_flash_3byte_addr_command_structure() {
        // 验证 3-byte 模式下命令字节结构
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let mock = RecordingBridge::new(
            vec![
                // read_flash 响应: cmd + 3-byte addr + 4 data bytes = 8 bytes
                vec![0x03, 0x01, 0x02, 0x03, 0xDE, 0xAD, 0xBE, 0xEF],
            ],
            recorded.clone(),
        );
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        let data = driver.read_flash(0x010203, 4).unwrap();
        assert_eq!(data, vec![0xDE, 0xAD, 0xBE, 0xEF]);

        let cmds = recorded.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0][0], 0x03); // READ_DATA
        assert_eq!(cmds[0][1], 0x01); // addr MSB
        assert_eq!(cmds[0][2], 0x02);
        assert_eq!(cmds[0][3], 0x03); // addr LSB
        assert_eq!(cmds[0].len(), 8); // 1 + 3 + 4
    }

    // ---- 状态寄存器测试 ----

    #[test]
    fn test_read_status_register_sr1() {
        let mock = MockBridge::new(vec![vec![0x05, 0x42]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        assert_eq!(
            driver.read_status_register(StatusRegister::Sr1).unwrap(),
            0x42
        );
    }

    #[test]
    fn test_read_status_register_sr2() {
        let mock = MockBridge::new(vec![vec![0x35, 0x55]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        assert_eq!(
            driver.read_status_register(StatusRegister::Sr2).unwrap(),
            0x55
        );
    }

    #[test]
    fn test_read_status_register_sr3() {
        let mock = MockBridge::new(vec![vec![0x15, 0xAA]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        assert_eq!(
            driver.read_status_register(StatusRegister::Sr3).unwrap(),
            0xAA
        );
    }

    #[test]
    fn test_status_register_enum() {
        assert_ne!(StatusRegister::Sr1, StatusRegister::Sr2);
        assert_ne!(StatusRegister::Sr2, StatusRegister::Sr3);
        assert_ne!(StatusRegister::Sr1, StatusRegister::Sr3);
    }

    #[test]
    fn test_write_status() {
        let mock = MockBridge::new(vec![vec![0x01, 0x00]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver.write_status(&[0x00]).unwrap();
    }

    #[test]
    fn test_write_status_multi_byte() {
        // 写入 SR1 + SR2（2 字节）
        let mock = MockBridge::new(vec![vec![0x01, 0x00, 0x00]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver.write_status(&[0x00, 0x00]).unwrap();
    }

    #[test]
    fn test_write_status_invalid_length() {
        let mock = MockBridge::new(vec![]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        // 空数据
        assert!(driver.write_status(&[]).is_err());
        // 超过 3 字节
        assert!(driver.write_status(&[0x00, 0x01, 0x02, 0x03]).is_err());
    }

    #[test]
    fn test_clear_block_protect() {
        // 流程: WREN → read_status(WEL) → write_status(0x00) → wait_ready
        let mock = MockBridge::new(vec![
            vec![0x06],            // WREN spi_xfer
            vec![0x05, 0x02],      // read_status → WEL=1
            vec![0x01, 0x00],      // write_status spi_xfer
            vec![0x05, 0x00],      // wait_ready → WIP=0
        ]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver.clear_block_protect().unwrap();
    }

    #[test]
    fn test_is_busy() {
        // WIP=1（忙碌）
        let mock = MockBridge::new(vec![vec![0x05, 0x01]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        assert!(driver.is_busy().unwrap());

        // WIP=0（空闲）
        let mock = MockBridge::new(vec![vec![0x05, 0x00]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        assert!(!driver.is_busy().unwrap());
    }

    // ---- 写入校验测试 ----

    #[test]
    fn test_verify_flash_ok() {
        let mock = MockBridge::new(vec![vec![
            0x03, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF,
        ]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver
            .verify_flash(0x000000, &[0xDE, 0xAD, 0xBE, 0xEF])
            .unwrap();
    }

    #[test]
    fn test_verify_flash_empty() {
        let mock = MockBridge::new(vec![]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver.verify_flash(0x000000, &[]).unwrap();
    }

    #[test]
    fn test_verify_flash_mismatch() {
        let mock = MockBridge::new(vec![vec![
            0x03, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0x00, 0xEF,
        ]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        let err = driver
            .verify_flash(0x000000, &[0xDE, 0xAD, 0xBE, 0xEF])
            .unwrap_err();
        match err {
            UsbBridgeError::VerifyFailed {
                offset,
                expected,
                got,
            } => {
                assert_eq!(offset, 2);
                assert_eq!(expected, 0xBE);
                assert_eq!(got, 0x00);
            }
            _ => panic!("Expected VerifyFailed error"),
        }
    }

    #[test]
    fn test_write_flash_with_verify_ok() {
        // write_flash: erase_range + page_program
        // erase_range → sector_erase → WREN + read_status + erase_cmd + wait_ready
        // page_program → WREN + read_status + program_cmd + wait_ready
        // verify_flash → read_flash
        let mock = MockBridge::new(vec![
            vec![0x06], // 1. WREN
            vec![0x05, 0x02], // 2. read_status (WEL=1)
            vec![0x20, 0x00, 0x00, 0x00], // 3. sector_erase
            vec![0x05, 0x00], // 4. wait_ready (WIP=0)
            vec![0x06], // 5. WREN
            vec![0x05, 0x02], // 6. read_status (WEL=1)
            vec![0x02, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF], // 7. page_program
            vec![0x05, 0x00], // 8. wait_ready (WIP=0)
            vec![0x03, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF], // 9. read_flash (verify)
        ]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver
            .write_flash_with_verify(0x000000, &[0xDE, 0xAD, 0xBE, 0xEF])
            .unwrap();
    }

    #[test]
    fn test_write_flash_with_verify_fail() {
        let mock = MockBridge::new(vec![
            vec![0x06], // 1. WREN
            vec![0x05, 0x02], // 2. read_status (WEL=1)
            vec![0x20, 0x00, 0x00, 0x00], // 3. sector_erase
            vec![0x05, 0x00], // 4. wait_ready (WIP=0)
            vec![0x06], // 5. WREN
            vec![0x05, 0x02], // 6. read_status (WEL=1)
            vec![0x02, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF], // 7. page_program
            vec![0x05, 0x00], // 8. wait_ready (WIP=0)
            // 9. read_flash 返回不匹配数据（offset 2 处 0x00 ≠ 0xBE）
            vec![0x03, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0x00, 0xEF],
        ]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        let err = driver
            .write_flash_with_verify(0x000000, &[0xDE, 0xAD, 0xBE, 0xEF])
            .unwrap_err();
        match err {
            UsbBridgeError::VerifyFailed {
                offset,
                expected,
                got,
            } => {
                assert_eq!(offset, 2);
                assert_eq!(expected, 0xBE);
                assert_eq!(got, 0x00);
            }
            _ => panic!("Expected VerifyFailed error"),
        }
    }

    #[test]
    fn test_write_flash_verified_no_verify() {
        // verify=false → 与 write_flash 行为一致（无额外 read_flash）
        let mock = MockBridge::new(vec![
            vec![0x06], // WREN
            vec![0x05, 0x02], // read_status (WEL=1)
            vec![0x20, 0x00, 0x00, 0x00], // sector_erase
            vec![0x05, 0x00], // wait_ready (WIP=0)
            vec![0x06], // WREN
            vec![0x05, 0x02], // read_status (WEL=1)
            vec![0x02, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF], // page_program
            vec![0x05, 0x00], // wait_ready (WIP=0)
        ]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver
            .write_flash_verified(0x000000, &[0xDE, 0xAD, 0xBE, 0xEF], false)
            .unwrap();
    }

    #[test]
    fn test_write_flash_verified_with_verify() {
        let mock = MockBridge::new(vec![
            vec![0x06], // WREN
            vec![0x05, 0x02], // read_status (WEL=1)
            vec![0x20, 0x00, 0x00, 0x00], // sector_erase
            vec![0x05, 0x00], // wait_ready (WIP=0)
            vec![0x06], // WREN
            vec![0x05, 0x02], // read_status (WEL=1)
            vec![0x02, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF], // page_program
            vec![0x05, 0x00], // wait_ready (WIP=0)
            vec![0x03, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF], // read_flash (verify)
        ]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver
            .write_flash_verified(0x000000, &[0xDE, 0xAD, 0xBE, 0xEF], true)
            .unwrap();
    }

    // ---- 进度回调测试 ----

    #[test]
    fn test_progress_callback_read() {
        let calls = Arc::new(Mutex::new(Vec::<(u64, u64)>::new()));
        let calls_clone = calls.clone();

        // 读取 8KB 数据：每块上限 0x1000 含 4 字节命令头 → 分 3 块（4092 + 4092 + 8）
        let mut resp1 = vec![0x03, 0x00, 0x00, 0x00];
        resp1.extend(std::iter::repeat(0xAA).take(4092));
        let mut resp2 = vec![0x03, 0x00, 0x0F, 0xFC];
        resp2.extend(std::iter::repeat(0xBB).take(4092));
        let mut resp3 = vec![0x03, 0x00, 0x1F, 0xF8];
        resp3.extend(std::iter::repeat(0xCC).take(8));

        let mock = MockBridge::new(vec![resp1, resp2, resp3]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver.with_progress(Box::new(move |current, total| {
            calls_clone.lock().unwrap().push((current, total));
        }));

        let data = driver.read_flash(0x000000, 8192).unwrap();
        assert_eq!(data.len(), 8192);

        let calls = calls.lock().unwrap();
        assert!(!calls.is_empty());
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0], (4092, 8192));
        assert_eq!(calls[1], (8184, 8192));
        assert_eq!(calls[2], (8192, 8192));
    }

    #[test]
    fn test_progress_callback_no_cb() {
        // 未设置回调时不应 panic
        let mut resp = vec![0x03, 0x00, 0x00, 0x00];
        resp.extend(std::iter::repeat(0xAA).take(4));
        let mock = MockBridge::new(vec![resp]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver.read_flash(0x000000, 4).unwrap();
    }

    #[test]
    fn test_progress_callback_chip_erase() {
        let calls = Arc::new(Mutex::new(Vec::<(u64, u64)>::new()));
        let calls_clone = calls.clone();

        let mock = MockBridge::new(vec![
            vec![0x06],         // WREN
            vec![0x05, 0x02],   // read_status (WEL=1)
            vec![0xC7],         // chip_erase
            vec![0x05, 0x00],   // wait_ready (WIP=0)
        ]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver.with_progress(Box::new(move |current, total| {
            calls_clone.lock().unwrap().push((current, total));
        }));

        driver.chip_erase().unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], (0, 1));
        assert_eq!(calls[1], (1, 1));
    }

    #[test]
    fn test_progress_callback_write_flash() {
        let calls = Arc::new(Mutex::new(Vec::<(u64, u64)>::new()));
        let calls_clone = calls.clone();

        // write_flash: erase_range + page_program
        let mock = MockBridge::new(vec![
            vec![0x06], // WREN
            vec![0x05, 0x02], // read_status (WEL=1)
            vec![0x20, 0x00, 0x00, 0x00], // sector_erase
            vec![0x05, 0x00], // wait_ready (WIP=0)
            vec![0x06], // WREN
            vec![0x05, 0x02], // read_status (WEL=1)
            vec![0x02, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF], // page_program
            vec![0x05, 0x00], // wait_ready (WIP=0)
        ]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));
        driver.with_progress(Box::new(move |current, total| {
            calls_clone.lock().unwrap().push((current, total));
        }));

        driver.write_flash(0x000000, &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();

        let calls = calls.lock().unwrap();
        // erase_range 1 次进度 + page_program 1 次进度 = 至少 2 次
        assert!(calls.len() >= 2);
    }

    // ---- SFDP / RES / 0x90 / 综合识别测试 ----

    #[test]
    fn test_read_sfdp_capacity() {
        // 模拟 W25Q128JV 的 SFDP 响应
        // spi_xfer 返回前 5 字节为命令回显填充，后 8 字节为实际数据
        let header = vec![b'S', b'F', b'D', b'P', 0x01, 0x00, 0x00, 0x00];
        let header_resp = [vec![0x00; 5], header].concat();
        // Parameter Header: ID=0x00 + minor=0 + major=1 + length=14 + ptr=0x10
        let ph = vec![0x00, 0x00, 0x01, 0x0E, 0x10, 0x00, 0x00, 0x00];
        let ph_resp = [vec![0x00; 5], ph].concat();
        // Parameter Table: 前 4 字节保留，第 4-7 字节是 Density
        // JESD216：Density = N 表示密度为 2^N **比特**
        // W25Q128 = 128Mbit = 2^27 比特，Density = 27（换算为 16MB 字节）
        let table = vec![0x00, 0x00, 0x00, 0x00, 27, 0x00, 0x00, 0x00];
        let table_resp = [vec![0x00; 5], table].concat();

        let mock = MockBridge::new(vec![header_resp, ph_resp, table_resp]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let capacity = driver.read_sfdp_capacity().unwrap();
        assert_eq!(capacity, 16 * 1024 * 1024); // 16 MB
    }

    #[test]
    fn test_read_sfdp_capacity_large_chip() {
        // 模拟 W25Q512 (512Mbit = 2^29 比特 → Density = 29，换算为 64MB 字节)
        let header = vec![b'S', b'F', b'D', b'P', 0x01, 0x06, 0x00, 0x00];
        let header_resp = [vec![0x00; 5], header].concat();
        let ph = vec![0x00, 0x06, 0x01, 0x0E, 0x10, 0x00, 0x00, 0x00];
        let ph_resp = [vec![0x00; 5], ph].concat();
        let table = vec![0x00, 0x00, 0x00, 0x00, 29, 0x00, 0x00, 0x00];
        let table_resp = [vec![0x00; 5], table].concat();

        let mock = MockBridge::new(vec![header_resp, ph_resp, table_resp]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let capacity = driver.read_sfdp_capacity().unwrap();
        assert_eq!(capacity, 64 * 1024 * 1024); // 64 MB
    }

    #[test]
    fn test_read_sfdp_invalid_signature() {
        // 错误的 SFDP 签名
        let bad_header = vec![0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x00, 0x00, 0x00];
        let bad_resp = [vec![0x00; 5], bad_header].concat();
        let mock = MockBridge::new(vec![bad_resp]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let result = driver.read_sfdp_capacity();
        assert!(result.is_err());
    }

    #[test]
    fn test_read_electronic_signature() {
        // RES 命令返回 0xAB + 3 dummy + ES=0x13 (W25Q80 的 ES)
        let mock = MockBridge::new(vec![vec![0xAB, 0x00, 0x00, 0x00, 0x13]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let es = driver.read_electronic_signature().unwrap();
        assert_eq!(es, 0x13);
    }

    #[test]
    fn test_read_manufacturer_id_90() {
        // 0x90 命令返回厂商 ID + 设备 ID
        // Winbond W25Q80: MFG=0xEF, Device=0x13
        let mock = MockBridge::new(vec![vec![0x90, 0x00, 0x00, 0x00, 0xEF, 0x13]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let (mfg, dev) = driver.read_manufacturer_id_90().unwrap();
        assert_eq!(mfg, 0xEF);
        assert_eq!(dev, 0x13);
    }

    #[test]
    fn test_identify_chip_jedec_known() {
        // 已知芯片：W25Q128JV (0xEF, 0x6017)
        let mock = MockBridge::new(vec![vec![0x9F, 0xEF, 0x60, 0x17]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let (chip, method) = driver.identify_chip().unwrap();
        assert_eq!(chip.manufacturer, "Winbond");
        assert_eq!(chip.part_number, "W25Q128JV");
        assert_eq!(chip.capacity_mbit, 128);
        assert_eq!(chip.voltage, SpiFlashVoltage::V3_3);
        assert!(method.contains("JEDEC"));
    }

    #[test]
    fn test_identify_chip_mx25u_1_8v() {
        // MX25U12833E (Macronix 1.8V 系列)
        let mock = MockBridge::new(vec![vec![0x9F, 0xC2, 0x25, 0x39]]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let (chip, method) = driver.identify_chip().unwrap();
        assert_eq!(chip.manufacturer, "Macronix");
        assert_eq!(chip.part_number, "MX25U12833E");
        assert_eq!(chip.capacity_mbit, 128);
        assert_eq!(chip.voltage, SpiFlashVoltage::V1_8); // 1.8V 低电压系列
        assert!(method.contains("JEDEC"));
    }

    #[test]
    fn test_identify_chip_unknown_with_sfdp() {
        // 未知芯片（JEDEC ID 不在表中），但 SFDP 可读取容量
        // MFG=0xAA (未知), CAP=0x1234 (不在表中)
        let jedec_resp = vec![0x9F, 0xAA, 0x12, 0x34];
        // SFDP 响应：256Mbit = 2^28 比特 → Density = 28（换算为 32MB 字节）
        // 注意：spi_xfer 返回前 5 字节为命令回显填充
        let header = vec![b'S', b'F', b'D', b'P', 0x01, 0x00, 0x00, 0x00];
        let header_resp = [vec![0x00; 5], header].concat();
        let ph = vec![0x00, 0x00, 0x01, 0x0E, 0x10, 0x00, 0x00, 0x00];
        let ph_resp = [vec![0x00; 5], ph].concat();
        let table = vec![0x00, 0x00, 0x00, 0x00, 28, 0x00, 0x00, 0x00];
        let table_resp = [vec![0x00; 5], table].concat();

        let mock = MockBridge::new(vec![jedec_resp, header_resp, ph_resp, table_resp]);
        let mut driver = SpiFlashDriver::new(Box::new(mock));

        let (chip, method) = driver.identify_chip().unwrap();
        assert_eq!(chip.capacity_mbit, 256); // SFDP 检测到 256Mbit
        assert_eq!(chip.part_number, UNKNOWN_PART_NUMBER);
        assert!(method.contains("SFDP"));
    }

    #[test]
    fn test_spi_flash_voltage_helpers() {
        assert_eq!(SpiFlashVoltage::V3_3.typical(), 3.3);
        assert_eq!(SpiFlashVoltage::V1_8.typical(), 1.8);
        assert_eq!(SpiFlashVoltage::Wide.typical(), 3.3);

        assert_eq!(SpiFlashVoltage::V3_3.range_str(), "2.7V~3.6V");
        assert_eq!(SpiFlashVoltage::V1_8.range_str(), "1.65V~1.95V");
        assert_eq!(SpiFlashVoltage::Wide.range_str(), "1.65V~3.6V");

        assert_eq!(SpiFlashVoltage::V3_3.name(), "3.3V");
        assert!(SpiFlashVoltage::V1_8.name().contains("1.8V"));
    }

    // ---- 扩充芯片表查表测试 ----

    #[test]
    fn test_lookup_mx25u_series() {
        // MX25U1633E (1.8V, 16Mbit)
        let info = crate::bus_probe::lookup_spi_flash(0xC2, 0x2536).unwrap();
        assert_eq!(info.manufacturer, "Macronix");
        assert_eq!(info.part_number, "MX25U1633E");
        assert_eq!(info.capacity_mbit, 16);
        assert_eq!(info.voltage, SpiFlashVoltage::V1_8);
    }

    #[test]
    fn test_lookup_w25q_jv_series() {
        // W25Q128JV (3.3V, 128Mbit)
        let info = crate::bus_probe::lookup_spi_flash(0xEF, 0x6017).unwrap();
        assert_eq!(info.manufacturer, "Winbond");
        assert_eq!(info.part_number, "W25Q128JV");
        assert_eq!(info.capacity_mbit, 128);
        assert_eq!(info.voltage, SpiFlashVoltage::V3_3);
    }

    #[test]
    fn test_lookup_w25q256_jv_4byte() {
        // W25Q256JV (3.3V, 256Mbit, 支持 4-byte 地址)
        let info = crate::bus_probe::lookup_spi_flash(0xEF, 0x6018).unwrap();
        assert_eq!(info.part_number, "W25Q256JV");
        assert_eq!(info.capacity_mbit, 256);
        assert!(info.supports_4byte_addr);
    }

    #[test]
    fn test_lookup_gd25b_1_8v() {
        // GD25B128 (GigaDevice 1.8V, 128Mbit)
        let info = crate::bus_probe::lookup_spi_flash(0xC8, 0x6017).unwrap();
        assert_eq!(info.manufacturer, "GigaDevice");
        assert_eq!(info.part_number, "GD25B128");
        assert_eq!(info.voltage, SpiFlashVoltage::V1_8);
    }

    #[test]
    fn test_lookup_xtx_wide_voltage() {
        // XT25Q128D (XTX 宽电压)
        let info = crate::bus_probe::lookup_spi_flash(0x0B, 0x4017).unwrap();
        assert_eq!(info.manufacturer, "XTX");
        assert_eq!(info.part_number, "XT25Q128D");
        assert_eq!(info.voltage, SpiFlashVoltage::Wide);
    }

    #[test]
    fn test_lookup_is25wp_wide_voltage() {
        // IS25WP064 (ISSI 宽电压)
        let info = crate::bus_probe::lookup_spi_flash(0x9D, 0x7016).unwrap();
        assert_eq!(info.part_number, "IS25WP064");
        assert_eq!(info.voltage, SpiFlashVoltage::Wide);
    }

    #[test]
    fn test_lookup_esmt() {
        // ESMT EN25S128
        let info = crate::bus_probe::lookup_spi_flash(0x8C, 0x4017).unwrap();
        assert_eq!(info.manufacturer, "ESMT");
        assert_eq!(info.part_number, "EN25S128");
        assert_eq!(info.capacity_mbit, 128);
    }

    #[test]
    fn test_extended_vendor_names() {
        use crate::bus_probe::spi_flash_vendor_name;
        assert_eq!(spi_flash_vendor_name(0x8C), "ESMT");
        assert_eq!(spi_flash_vendor_name(0x0B), "XTX");
        assert_eq!(spi_flash_vendor_name(0x62), "Fremont Micro");
        assert_eq!(spi_flash_vendor_name(0x68), "Boya");
        assert_eq!(spi_flash_vendor_name(0xE0), "Dofuly");
        assert_eq!(spi_flash_vendor_name(0xA1), "Fudan Micro");
        assert_eq!(spi_flash_vendor_name(0x61), "Paragon");
        assert_eq!(
            spi_flash_vendor_name(0xFF),
            crate::bus_probe::UNKNOWN_NAME
        );
    }
}
