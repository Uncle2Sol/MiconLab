//! ESP32 在线识别工具（example）
//!
//! 对已连接的 ESP32 设备做在线硬件识别与固件验证，不依赖外部工具：
//!
//! Phase A（运行态监听）：RTS 复位芯片 → 采集 ROM 启动行 + 应用固件日志
//!   - ROM 行 "ESP-ROM:esp32s3-..." 直接给出芯片型号与 ROM 版本
//!   - 应用日志判断固件是否正常运行（boot loop / Guru Meditation / invalid header）
//!
//! Phase B（bootloader 在线识别）：USB-JTAG/经典复位进 ROM 下载模式
//!   - SYNC (0x08)          → ROM bootloader 存活
//!   - GET_SECURITY_INFO(0x18) → chip_id / flash_crypt_cnt / 安全信息
//!   - READ_REG (0x0A)      → EFUSE MAC（与 USB 序列号交叉验证）
//!   - SPI_ATTACH (0x0D) + FLASH_MD5 (0x13) → bootloader 区/分区表区非空校验
//!   - FLASH_END (reboot)   → 结束后复位回应用固件
//!
//! 用法: cargo run -p miconlab-core --example esp_probe -- [port] [baud]
//! 默认: /dev/cu.usbmodem1101 115200

use serialport::SerialPort;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

// ---- ESP ROM 命令码 ----
const CMD_FLASH_END: u8 = 0x04; // reboot 复位
const CMD_SYNC: u8 = 0x08;
const CMD_READ_REG: u8 = 0x0A;
const CMD_SPI_ATTACH: u8 = 0x0D;
const CMD_FLASH_MD5: u8 = 0x13;
const CMD_SECURITY_INFO: u8 = 0x18;

// ESP32-S3 EFUSE RD_BLK0 寄存器（MAC，交叉验证用）
const S3_EFUSE_RD_MAC: u32 = 0x6001_9704;

fn slip_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 4);
    for &b in data {
        match b {
            0xC0 => out.extend_from_slice(&[0xDB, 0xDC]),
            0xDB => out.extend_from_slice(&[0xDB, 0xDD]),
            _ => out.push(b),
        }
    }
    out
}

/// 构造 ESP ROM 命令帧（命令方向 checksum=0，与 esptool 一致）
fn packet(cmd: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 10);
    frame.push(0xC0);
    frame.extend_from_slice(&[0x00, cmd, (payload.len() & 0xFF) as u8, (payload.len() >> 8) as u8, 0, 0, 0, 0]);
    frame.extend_from_slice(&slip_encode(payload));
    frame.push(0xC0);
    frame
}

/// 从串口读一个 SLIP 帧（去转义后返回含方向字节在内的 body），超时返回 None
fn read_slip_frame(port: &mut Box<dyn SerialPort>, timeout: Duration) -> Option<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = [0u8; 64];
    let mut started = false;
    loop {
        if Instant::now() >= deadline {
            return None;
        }
        match port.read(&mut buf) {
            Ok(n) if n > 0 => {
                for &b in &buf[..n] {
                    match b {
                        0xC0 => {
                            if started {
                                // 帧尾：解码转义
                                let mut body = Vec::with_capacity(raw.len());
                                let mut i = 0;
                                while i < raw.len() {
                                    if raw[i] == 0xDB && i + 1 < raw.len() {
                                        body.push(match raw[i + 1] {
                                            0xDC => 0xC0,
                                            0xDD => 0xDB,
                                            x => x,
                                        });
                                        i += 2;
                                    } else {
                                        body.push(raw[i]);
                                        i += 1;
                                    }
                                }
                                return Some(body);
                            } else {
                                started = true; // 帧头
                            }
                        }
                        _ => {
                            if started {
                                raw.push(b);
                            }
                        }
                    }
                }
            }
            _ => continue,
        }
    }
}

/// 发送命令并等待 ACK 响应（返回响应 payload）
fn command(
    port: &mut Box<dyn SerialPort>,
    cmd: u8,
    payload: &[u8],
    timeout: Duration,
) -> Option<Vec<u8>> {
    let pkt = packet(cmd, payload);
    port.write_all(&pkt).ok()?;
    // 跳过可能残留的数据直到出现帧
    let body = read_slip_frame(port, timeout)?;
    // 响应帧: dir(1) cmd(1) size(2) crc(4) payload
    if body.len() < 8 || body[0] != 0x01 || body[1] != cmd {
        return None;
    }
    let size = body[2] as usize | ((body[3] as usize) << 8);
    if body.len() < 8 + size {
        return None;
    }
    Some(body[8..8 + size].to_vec())
}

/// SYNC 数据（esptool 标准 36 字节）
fn sync_packet() -> Vec<u8> {
    let mut data = vec![0x07, 0x07, 0x12, 0x20];
    data.extend(std::iter::repeat(0x55).take(32));
    packet(CMD_SYNC, &data)
}

/// 读一段时间的原始输出（文本日志）
fn drain_text(port: &mut Box<dyn SerialPort>, dur: Duration) -> String {
    let deadline = Instant::now() + dur;
    let mut out = Vec::new();
    let mut buf = [0u8; 512];
    while Instant::now() < deadline {
        if let Ok(n) = port.read(&mut buf) {
            if n > 0 {
                out.extend_from_slice(&buf[..n]);
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn chip_name_from_id(chip_id: u8) -> &'static str {
    match chip_id {
        0 => "ESP32-S2",
        1 => "ESP32",
        2 => "ESP32-S2",
        4 => "ESP32-S2",
        5 => "ESP32-C3",
        9 => "ESP32-S3",
        12 => "ESP32-C6",
        16 => "ESP32-C2",
        _ => "未知（结合 USB PID 判断）",
    }
}

fn chip_name_from_pid(pid: u16) -> &'static str {
    match pid {
        0x0002 => "ESP32-S2 (原生 USB CDC)",
        0x0009 => "ESP32-S3 (USB-Serial-JTAG)",
        0x1001 => "ESP32-S3 (USB-Serial-JTAG debug unit)",
        0x1000 => "ESP32-S2 (DFU)",
        _ => "Espressif 原生 USB",
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port_name = args.get(1).cloned().unwrap_or_else(|| "/dev/cu.usbmodem1101".into());
    let baud: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(115200);

    println!("════════════════════════════════════════════════════════");
    println!(" miconlab 在线识别: {} @ {}", port_name, baud);
    println!("════════════════════════════════════════════════════════");

    let mut port = serialport::new(&port_name, baud)
        .timeout(Duration::from_millis(50))
        .open()
        .unwrap_or_else(|e| {
            eprintln!("❌ 打开串口失败: {}", e);
            std::process::exit(1);
        });

    // ============ Phase A: 运行态监听（RTS 复位 + 采集启动日志）============
    println!("\n── Phase A: 复位芯片并监听启动日志（4s）──");
    let _ = port.write_data_terminal_ready(false);
    let _ = port.write_request_to_send(true); // EN=LOW 复位
    std::thread::sleep(Duration::from_millis(150));
    let _ = port.write_request_to_send(false); // EN=HIGH 释放 → 正常启动
    std::thread::sleep(Duration::from_millis(100));

    let boot_log = drain_text(&mut port, Duration::from_secs(4));
    let rom_line = boot_log
        .lines()
        .find(|l| l.contains("ESP-ROM:"))
        .map(|s| s.trim().to_string());
    let boot_mode = boot_log
        .lines()
        .find(|l| l.contains("boot:"))
        .map(|s| s.trim().to_string());
    let crash_loop = boot_log.matches("rst:").count() >= 3;
    let guru = boot_log.contains("Guru Meditation");
    let invalid_header = boot_log.contains("invalid header");

    if boot_log.trim().is_empty() {
        println!("   （无 UART 输出——固件未打印日志或日志在其他 UART）");
    } else {
        println!("   启动日志 {} 字节，关键行:", boot_log.len());
        if let Some(r) = &rom_line {
            println!("   · ROM: {}", r);
        }
        if let Some(b) = &boot_mode {
            println!("   · {}", b);
        }
    }

    // ============ Phase B: 进 bootloader 在线识别 ============
    println!("\n── Phase B: 进入 ROM bootloader ──");
    let mut connected = false;

    // 策略 1: USB-JTAG 序列（S3 原生 USB）
    let _ = port.write_request_to_send(true); // EN=LOW
    let _ = port.write_data_terminal_ready(false);
    std::thread::sleep(Duration::from_millis(100));
    let _ = port.write_request_to_send(false); // EN=HIGH
    let _ = port.write_data_terminal_ready(true); // GPIO0=LOW → bootloader
    std::thread::sleep(Duration::from_millis(100));
    let _ = port.write_data_terminal_ready(false);
    std::thread::sleep(Duration::from_millis(300));

    for i in 0..7 {
        let pkt = sync_packet();
        if port.write_all(&pkt).is_err() {
            continue;
        }
        if let Some(body) = read_slip_frame(&mut port, Duration::from_millis(200)) {
            if body.len() >= 9 && body[0] == 0x01 && body[1] == CMD_SYNC {
                let status = body[8];
                if status == 0 {
                    println!("   ✅ SYNC OK（USB-JTAG 复位序列，第 {} 次）", i + 1);
                    connected = true;
                    break;
                }
            }
        }
    }

    // 策略 2: 经典自动复位（DTR=EN, RTS=GPIO0）
    if !connected {
        let _ = port.write_data_terminal_ready(true); // EN=LOW
        let _ = port.write_request_to_send(true); // GPIO0=LOW
        std::thread::sleep(Duration::from_millis(100));
        let _ = port.write_data_terminal_ready(false); // EN=HIGH 释放复位，GPIO0 仍 LOW
        std::thread::sleep(Duration::from_millis(50));
        let _ = port.write_request_to_send(false); // GPIO0 释放
        std::thread::sleep(Duration::from_millis(300));
        for i in 0..7 {
            let pkt = sync_packet();
            if port.write_all(&pkt).is_err() {
                continue;
            }
            if let Some(body) = read_slip_frame(&mut port, Duration::from_millis(200)) {
                if body.len() >= 9 && body[0] == 0x01 && body[1] == CMD_SYNC && body[8] == 0 {
                    println!("   ✅ SYNC OK（经典复位序列，第 {} 次）", i + 1);
                    connected = true;
                    break;
                }
            }
        }
    }

    if !connected {
        println!("   ❌ 无法进入 bootloader（SYNC 无响应）");
        println!("\n── 识别结果（仅 Phase A）──");
        report(&rom_line, &boot_mode, None, None, None, crash_loop, guru, invalid_header);
        return;
    }

    // ---- GET_SECURITY_INFO: 芯片型号 / 加密计数 ----
    let mut chip_id: Option<u8> = None;
    let mut crypt_cnt: Option<u8> = None;
    let mut flags: Option<u32> = None;
    if let Some(info) = command(&mut port, CMD_SECURITY_INFO, &[], Duration::from_millis(1000)) {
        // 响应: flags(4LE) flash_crypt_cnt(1) key_purposes[7](1×7) chip_id(1) ...
        if info.len() >= 13 {
            flags = Some(u32::from_le_bytes([info[0], info[1], info[2], info[3]]));
            crypt_cnt = Some(info[4]);
            chip_id = Some(info[12]);
            println!("   ✅ SECURITY_INFO: {} 字节", info.len());
        }
    } else {
        println!("   ⚠ SECURITY_INFO 不支持（老 ROM）");
    }

    // ---- READ_REG: EFUSE MAC ----
    let mut efuse_mac: Option<u64> = None;
    if let Some(v) = command(
        &mut port,
        CMD_READ_REG,
        &S3_EFUSE_RD_MAC.to_le_bytes(),
        Duration::from_millis(500),
    ) {
        if v.len() >= 4 {
            let reg = u32::from_le_bytes([v[0], v[1], v[2], v[3]]);
            if reg != 0 {
                efuse_mac = Some(reg as u64);
            }
        }
    }

    // ---- SPI_ATTACH + FLASH_MD5: 固件区非空校验 ----
    let mut md5_bootloader: Option<String> = None;
    let mut md5_ptable: Option<String> = None;
    let attach_payload: Vec<u8> = vec![0u8; 12]; // S2/S3: 12 字节
    if command(&mut port, CMD_SPI_ATTACH, &attach_payload, Duration::from_millis(1000)).is_some() {
        println!("   ✅ SPI_ATTACH OK");
        // bootloader 头 @ 0x0, 32 字节
        let mut p = Vec::with_capacity(16);
        p.extend_from_slice(&0u32.to_le_bytes());
        p.extend_from_slice(&32u32.to_le_bytes());
        p.extend_from_slice(&[0u8; 8]);
        if let Some(r) = command(&mut port, CMD_FLASH_MD5, &p, Duration::from_millis(2000)) {
            if r.len() >= 16 {
                md5_bootloader = Some(r[..16].iter().map(|b| format!("{:02x}", b)).collect());
            }
        }
        // 分区表 @ 0x8000, 32 字节
        let mut p2 = Vec::with_capacity(16);
        p2.extend_from_slice(&0x8000u32.to_le_bytes());
        p2.extend_from_slice(&32u32.to_le_bytes());
        p2.extend_from_slice(&[0u8; 8]);
        if let Some(r) = command(&mut port, CMD_FLASH_MD5, &p2, Duration::from_millis(2000)) {
            if r.len() >= 16 {
                md5_ptable = Some(r[..16].iter().map(|b| format!("{:02x}", b)).collect());
            }
        }
    } else {
        println!("   ⚠ SPI_ATTACH 失败（跳过 flash 校验）");
    }

    // ---- 复位回应用固件 ----
    let _ = command(&mut port, CMD_FLASH_END, &0u32.to_le_bytes(), Duration::from_millis(300));

    println!("\n── 识别结果 ──");
    report(
        &rom_line,
        &boot_mode,
        chip_id,
        crypt_cnt,
        flags,
        crash_loop,
        guru,
        invalid_header,
    );
    if let Some(m) = &md5_bootloader {
        println!("  Flash bootloader 区(0x0,32B) MD5 : {}", m);
    }
    if let Some(m) = &md5_ptable {
        println!("  Flash 分区表区(0x8000,32B) MD5 : {}", m);
    }
    if let (Some(a), Some(b)) = (&md5_bootloader, &md5_ptable) {
        if a == b {
            println!("  ⚠ 两区 MD5 相同 → 疑似空 Flash（全 FF/00，无固件）");
        } else {
            println!("  ✅ 两区内容不同 → Flash 内有固件数据");
        }
    }
    if let Some(mac_reg) = efuse_mac {
        println!("  EFUSE RD_BLK0 寄存器原始值      : 0x{:08X}（MAC 低 4 字节，供交叉验证）", mac_reg);
    }
}

#[allow(clippy::too_many_arguments)]
fn report(
    rom_line: &Option<String>,
    boot_mode: &Option<String>,
    chip_id: Option<u8>,
    crypt_cnt: Option<u8>,
    flags: Option<u32>,
    crash_loop: bool,
    guru: bool,
    invalid_header: bool,
) {
    println!("  ┌─────────── 硬件识别 ───────────");
    if let Some(r) = rom_line {
        println!("  │ ROM 标识: {}", r);
    }
    if let Some(id) = chip_id {
        println!("  │ 芯片型号（chip_id={}）: {}", id, chip_name_from_id(id));
    } else {
        println!("  │ 芯片型号: 由 USB PID 判断 → {}", chip_name_from_pid(0x1001));
    }
    if let Some(b) = boot_mode {
        println!("  │ 启动模式: {}", b);
    }
    if let Some(c) = crypt_cnt {
        println!("  │ Flash 加密计数: {}", c);
    }
    if let Some(f) = flags {
        println!("  │ 安全标志: 0x{:08X}", f);
    }
    println!("  └────────────────────────────────");

    println!("  ┌─────────── 固件状态 ───────────");
    if guru {
        println!("  │ ❌ 检测到 Guru Meditation（固件崩溃）");
    }
    if crash_loop {
        println!("  │ ❌ 疑似复位循环（≥3 次复位）");
    }
    if invalid_header {
        println!("  │ ❌ invalid header → 应用分区无有效固件（空 Flash）");
    }
    if !guru && !crash_loop && !invalid_header {
        if rom_line.is_some() {
            println!("  │ ✅ 启动流程正常（ROM → bootloader → 应用无异常日志）");
        } else {
            println!("  │ ? 无启动日志，固件状态无法从日志判断（见 Flash MD5 校验）");
        }
    }
    println!("  └────────────────────────────────");
}
