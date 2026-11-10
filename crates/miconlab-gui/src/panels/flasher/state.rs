//! flasher 子模块：FlasherState / ChipType / 通用辅助函数。
//!
//! 状态定义不依赖 mod.rs 的其他导出，因此不引入 `use super::*;`。

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ChipType {
    #[default]
    Stm32,
    Esp32,
    /// Arduino (AVR ATmega 系列通过 STK500 v1 / Optiboot 烧录)
    Arduino,
    /// ESP8266 (乐鑫 ROM Bootloader, 与 ESP32 类似但部分命令不同)
    Esp8266,
    /// STM8 (8 位 MCU, UART Bootloader UM0560)
    Stm8,
    /// NXP LPC (ARM Cortex-M/ARM7, ISP UART Bootloader UM10295)
    NxpLpc,
    /// GD32 (国产 STM32 兼容, 复用 STM32 协议)
    Gd32,
    /// CH32 (WCH 国产芯片, 自定义 ISP 协议 0x57 0xAB)
    Ch32,
    /// Microchip PIC (ICSP 协议, 需 PicKit/ICD 硬件)
    Pic,
    /// 开发板 SD 卡镜像烧录 (树莓派/Jetson Nano/Orange Pi 等)
    SdCard,
    /// SPI Flash (外挂 NOR Flash, 通过 CH341A/FT232H USB 编程器直连)
    SpiFlash,
}

impl ChipType {
    pub(crate) fn label(self, _is_zh: bool) -> &'static str {
        match self {
            ChipType::Stm32 => "STM32",
            ChipType::Esp32 => "ESP32",
            ChipType::Arduino => { crate::i18n::t("Arduino (AVR/STK500)") },
            ChipType::Esp8266 => "ESP8266",
            ChipType::Stm8 => "STM8",
            ChipType::NxpLpc => { crate::i18n::t("NXP LPC (ISP)") },
            ChipType::Gd32 => "GD32",
            ChipType::Ch32 => "CH32 (WCH)",
            ChipType::Pic => "PIC (ICSP)",
            ChipType::SdCard => crate::i18n::t("SBC SD Card Image"),
            ChipType::SpiFlash => crate::i18n::t("SPI Flash (USB Programmer)"),
        }
    }

    /// 芯片默认 Flash 容量（字节），用于自动设置读取长度
    pub(crate) fn default_flash_size(self) -> u32 {
        match self {
            ChipType::Stm32 | ChipType::Gd32 => 64 * 1024,      // 64KB (STM32F103C8 典型值)
            ChipType::Ch32 => 64 * 1024,                         // 64KB (CH32F103 兼容)
            ChipType::Esp32 => 4 * 1024 * 1024,                  // 4MB (ESP32 典型 Flash)
            ChipType::Esp8266 => 2 * 1024 * 1024,                // 2MB (ESP8266 典型 Flash)
            ChipType::Arduino => 32 * 1024,                      // 32KB (ATmega328P)
            ChipType::Stm8 => 32 * 1024,                         // 32KB (STM8S103)
            ChipType::NxpLpc => 64 * 1024,                       // 64KB (LPC11xx 典型值)
            ChipType::Pic => 32 * 1024,                          // 32KB
            ChipType::SdCard => 4 * 1024 * 1024,                 // 4MB (最小 SD 镜像)
            ChipType::SpiFlash => 1024 * 1024,                   // 1MB (典型 SPI Flash)
        }
    }

    /// Flash 起始地址（用于自动设置读取地址）
    pub(crate) fn flash_base_addr(self) -> u32 {
        match self {
            ChipType::Stm32 | ChipType::Gd32 | ChipType::Ch32 => 0x0800_0000,  // Cortex-M Flash 基址
            ChipType::Esp32 | ChipType::Esp8266 => 0x0000_0000,                // ESP 系列 Flash 从 0 开始
            ChipType::Arduino => 0x0000_0000,                                  // AVR Flash 从 0 开始
            ChipType::Stm8 => 0x0080_0000,                                     // STM8 Flash 基址
            ChipType::NxpLpc => 0x0000_0000,                                   // LPC Flash 基址
            _ => 0x0000_0000,
        }
    }

    /// 默认 Flash 容量标签（用于显示）
    pub(crate) fn default_flash_label(self, _is_zh: bool) -> String {
        let size = self.default_flash_size();
        if size >= 1024 * 1024 {
            format!("{}MB", size / (1024 * 1024))
        } else {
            format!("{}KB", size / 1024)
        }
    }
}

/// 持久化面板状态（通过 ctx.data_mut 存储）
#[derive(Clone)]
pub(crate) struct FlasherState {
    pub(crate) chip_type: ChipType,   // 芯片类型
    pub(crate) file_path: String,     // 固件文件路径
    pub(crate) address: u32,          // 读写起始地址
    pub(crate) pages: String,         // STM32 页擦除列表（逗号分隔的页号）
    pub(crate) log: Vec<String>,      // 操作日志
    pub(crate) progress: f32,         // 进度（0.0-1.0）
    pub(crate) baud_new: u32,         // ESP32 新波特率
    pub(crate) baud_current: u32,     // ESP32 当前波特率
    pub(crate) read_len: u32,         // 读取长度
    pub(crate) response: String,      // 最近一次响应（HEX 格式）
    /// 最近一次读取到的数据（用于备份到文件）
    pub(crate) last_read_data: Vec<u8>,
    /// 备份文件保存路径
    pub(crate) backup_path: String,
    /// Arduino 读取/写入的页大小（字节）
    pub(crate) arduino_page_size: u16,
    /// Arduino Flash 总大小（KB），由签名识别自动填充
    pub(crate) arduino_flash_kb: u32,
    /// Arduino 芯片名（由签名识别填充）
    pub(crate) arduino_chip_name: String,
    /// NXP LPC 起始扇区号
    pub(crate) lpc_sector_start: u32,
    /// NXP LPC 结束扇区号
    pub(crate) lpc_sector_end: u32,
    /// NXP LPC RAM 缓冲区地址（用于先写 RAM 再烧 Flash）
    pub(crate) lpc_ram_addr: u32,
    /// SD 卡设备路径（如 /dev/disk2，macOS）或盘符（如 E:，Windows）
    pub(crate) sd_card_disk: String,
    /// SD 卡镜像文件路径
    pub(crate) sd_card_image_path: String,
    /// 扫描得到的磁盘列表（disk list 输出，用于显示）
    pub(crate) sd_card_disk_list: String,
    /// SD 卡烧录是否需要 root/sudo
    pub(crate) sd_card_need_sudo: bool,
    /// SPI Flash：选中的 USB 桥接器类型（"CH341A" / "FT232H"）
    pub(crate) spi_bridge_kind: String,
    /// SPI Flash：JEDEC ID 识别结果（显示用）
    pub(crate) spi_jedec_info: String,
    /// SPI Flash：读取起始地址
    pub(crate) spi_flash_addr: u32,
    /// SPI Flash：读取长度（KB）
    pub(crate) spi_flash_len_kb: u32,
    /// SPI Flash：读取到的数据（用于备份保存）
    pub(crate) spi_flash_data: Vec<u8>,
    /// SPI Flash：待写入的固件文件路径
    pub(crate) spi_flash_image_path: String,
    /// SPI Flash：备份保存路径
    pub(crate) spi_flash_backup_path: String,
    /// SPI Flash：已识别的芯片容量（字节，0 表示未识别）
    pub(crate) spi_flash_capacity: u64,
    /// 上次连接会话 ID，用于检测连接变化并清除设备派生数据
    pub(crate) last_session_id: u64,
    /// === 非阻塞读取状态机 ===
    /// 是否正在执行读取操作
    pub(crate) read_running: bool,
    /// 当前读取地址
    pub(crate) read_addr: u32,
    /// 剩余字节数
    pub(crate) read_remaining: usize,
    /// 总字节数
    pub(crate) read_total: usize,
    /// 读取芯片类型（用于状态机派发）
    pub(crate) read_chip: ChipType,
    /// 当前读取步骤等待截止时刻
    pub(crate) read_wait_until: Option<std::time::Instant>,
    /// 读取阶段：0=发送命令, 1=等待响应
    pub(crate) read_phase: u8,
    /// 原始读取模式（设备未进入 Bootloader 时回退到直接读取串口数据）
    pub(crate) read_raw_mode: bool,
    /// 读取响应累积缓冲（跨帧累积分段响应）
    pub(crate) read_resp_buf: Vec<u8>,
    /// Bootloader 是否已连接（收到 ACK 后置 true，断开/失败时置 false）
    pub(crate) bootloader_connected: bool,
    /// 上次芯片类型（用于检测切换并自动设置读取长度/地址）
    pub(crate) prev_chip_type: ChipType,
    /// === esptool.py 后备读取（非阻塞） ===
    /// 后台线程结果共享槽
    pub(crate) esptool_result_slot: Option<std::sync::Arc<std::sync::Mutex<Option<Result<Vec<u8>, String>>>>>,
    /// 后台线程日志共享槽（实时传递 esptool 输出）
    pub(crate) esptool_log_slot: Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
    /// 临时文件路径（完成后清理）
    pub(crate) esptool_temp_file: Option<String>,
    /// 是否正在运行
    pub(crate) esptool_running: bool,
    /// 上次进度日志时间（避免刷屏）
    pub(crate) esptool_last_log: Option<std::time::Instant>,
    /// 启动时刻（用于超时检测）
    pub(crate) esptool_started_at: Option<std::time::Instant>,
    /// 端口释放标志（通知后台线程端口已释放）
    pub(crate) esptool_port_released: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl Default for FlasherState {
    fn default() -> Self {
        let chip = ChipType::default();
        Self {
            chip_type: chip,
            file_path: String::new(),
            address: chip.flash_base_addr(), // 自动匹配芯片 Flash 起始地址
            pages: String::new(),
            log: Vec::new(),
            progress: 0.0,
            baud_new: 115200,
            baud_current: 115200,
            read_len: chip.default_flash_size(), // 自动匹配芯片 Flash 容量
            response: String::new(),
            last_read_data: Vec::new(),
            backup_path: String::new(),
            arduino_page_size: 128, // ATmega328P 默认页大小
            arduino_flash_kb: 32,
            arduino_chip_name: String::new(),
            lpc_sector_start: 0,
            lpc_sector_end: 7,
            lpc_ram_addr: 0x1000_0000, // LPC1768 RAM 起始
            sd_card_disk: String::new(),
            sd_card_image_path: String::new(),
            sd_card_disk_list: String::new(),
            sd_card_need_sudo: true,
            spi_bridge_kind: "CH341A".to_string(),
            spi_jedec_info: String::new(),
            spi_flash_addr: 0x000000,
            spi_flash_len_kb: 16, // 默认 16KB
            spi_flash_data: Vec::new(),
            spi_flash_image_path: String::new(),
            spi_flash_backup_path: String::new(),
            spi_flash_capacity: 0,
            last_session_id: 0,
            read_running: false,
            read_addr: 0,
            read_remaining: 0,
            read_total: 0,
            read_chip: ChipType::default(),
            read_wait_until: None,
            read_phase: 0,
            read_raw_mode: false,
            read_resp_buf: Vec::new(),
            bootloader_connected: false,
            prev_chip_type: ChipType::default(),
            esptool_result_slot: None,
            esptool_log_slot: None,
            esptool_temp_file: None,
            esptool_running: false,
            esptool_last_log: None,
            esptool_started_at: None,
            esptool_port_released: None,
        }
    }
}

/// 向日志追加一条消息
pub(crate) fn log_push(fs: &mut FlasherState, msg: impl Into<String>) {
    fs.log.push(msg.into());
    // 限制日志长度
    if fs.log.len() > 200 {
        fs.log.drain(0..fs.log.len() - 200);
    }
}

/// 解析页号列表字符串（逗号分隔，如 "0,1,2,10"）
pub(crate) fn parse_pages(s: &str) -> Result<Vec<u16>, String> {
    s.split(',')
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let p = p.trim();
            let val = if p.starts_with("0x") || p.starts_with("0X") {
                u16::from_str_radix(&p[2..], 16)
            } else {
                u16::from_str_radix(p, 10)
            };
            val.map_err(|e| {
                crate::i18n::tf(
                    "Invalid page number '{}': {}",
                    &[("{}", "{}", &format!("{}", p)), ("{}", "{}", &format!("{}", e))],
                )
            })
        })
        .collect()
}


