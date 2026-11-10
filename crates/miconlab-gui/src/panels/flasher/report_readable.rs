//! flasher 子模块：固件易读解析报告（把 bin 翻译成通俗语言）。
//!
//! 与「导出反汇编」（inspect_*.rs，面向工程师：裸地址+指令）互补，
//! 本报告面向普通用户，全部用白话解释：
//!   - 一句话总结这份固件是什么
//!   - ESP-IDF 应用描述符提取（项目名/版本/编译时间/IDF 版本）
//!   - 分区表解析（每个分区是干什么的）
//!   - 段布局说明（代码/只读数据/全局变量初值各在哪）
//!   - Flash 配置白话解读（QIO/DIO、40MHz、4MB 是什么意思）
//!   - 开发框架/组件识别（Arduino/MicroPython/Tasmota/ESPHome/WLED/LVGL...）
//!   - 内置字符串自动分类（版本/路径/网络/错误/调试）
//!   - 健康检查与 Flash 使用率
//!
//! 数据来源：fs.last_read_data（读取 Flash 后可用），基址 fs.address。
//! 输出 Markdown 文件（.md），可直接阅读或分享。

use super::*;

// ---------- ESP-IDF 应用描述符 (esp_app_desc_t) ----------
// 位于应用镜像开头（0x10000 处的应用分区），是固件"自我介绍"的结构体：
// magic(4) + secure_ver(4) + reserved(8) + version[32] + project_name[32]
// + time[16] + date[16] + idf_ver[32] + sha256[32] + reserved[80] = 256B
const APP_DESC_MAGIC: u32 = 0xABCD_5432;

struct AppDesc {
    version: String,
    project_name: String,
    time: String,
    date: String,
    idf_ver: String,
}

fn find_app_descs(data: &[u8]) -> Vec<AppDesc> {
    let mut out = Vec::new();
    if data.len() < 256 {
        return out;
    }
    let take_str = |from: usize, len: usize| -> String {
        let slice = &data[from..from + len];
        let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        String::from_utf8_lossy(&slice[..end]).trim().to_string()
    };
    // 扫描 4 字节魔数（应用镜像里也可能有多份，如 OTA 双分区）
    let mut off = 0usize;
    while off + 256 <= data.len() {
        let magic = u32::from_le_bytes([
            data[off],
            data[off + 1],
            data[off + 2],
            data[off + 3],
        ]);
        if magic == APP_DESC_MAGIC {
            out.push(AppDesc {
                version: take_str(off + 16, 32),
                project_name: take_str(off + 48, 32),
                time: take_str(off + 80, 16),
                date: take_str(off + 96, 16),
                idf_ver: take_str(off + 112, 32),
            });
            if out.len() >= 4 {
                break; // 最多 4 份（ota_0/ota_1/...）
            }
            off += 256;
        } else {
            off += 1;
        }
    }
    out
}

// ---------- 分区表解析（Flash 0x8000 处，每条 32 字节） ----------

struct PartInfo {
    label: String,
    type_name: String,
    offset: u32,
    size: u32,
}

fn parse_partitions(data: &[u8], base_addr: u32) -> Vec<PartInfo> {
    let mut parts = Vec::new();
    // 分区表固定在 Flash 0x8000；计算其在备份数据中的偏移
    if base_addr > 0x8000 {
        return parts;
    }
    let off = (0x8000 - base_addr) as usize;
    for i in 0..100 {
        let e = off + i * 32;
        if e + 32 > data.len() {
            break;
        }
        let magic = u16::from_le_bytes([data[e], data[e + 1]]);
        if magic != 0xAA50 {
            break; // 表结束
        }
        let ptype = data[e + 2];
        let subtype = data[e + 3];
        let poff = u32::from_le_bytes([
            data[e + 4],
            data[e + 5],
            data[e + 6],
            data[e + 7],
        ]);
        let psize = u32::from_le_bytes([
            data[e + 8],
            data[e + 9],
            data[e + 10],
            data[e + 11],
        ]);
        let label_raw = &data[e + 12..e + 28];
        let end = label_raw.iter().position(|&b| b == 0).unwrap_or(label_raw.len());
        let label = String::from_utf8_lossy(&label_raw[..end]).trim().to_string();
        parts.push(PartInfo {
            label,
            type_name: partition_type_name(ptype, subtype),
            offset: poff,
            size: psize,
        });
    }
    parts
}

/// 分区类型/子类型 → 通俗名称（含用途说明）
fn partition_type_name(ptype: u8, subtype: u8) -> String {
    match ptype {
        0x00 => match subtype {
            0x00 => crate::i18n::t("App · factory program").to_string(),
            0x10 => crate::i18n::t("App · test program").to_string(),
            0x20..=0x2F => crate::i18n::tf("App · OTA slot {}",
                &[("{}", "{}", &format!("{}", subtype - 0x20))]),
            _ => crate::i18n::tf("App · unknown (0x{:02X})",
                &[("{:02X}", "{:02X}", &format!("{:02X}", subtype))]),
        },
        0x01 => match subtype {
            0x00 => crate::i18n::t("Data · OTA state").to_string(),
            0x01 => crate::i18n::t("Data · RF calibration").to_string(),
            0x02 => crate::i18n::t("Data · config store (nvs)").to_string(),
            0x03 => crate::i18n::t("Data · crash dump").to_string(),
            0x04 => crate::i18n::t("Data · encryption keys").to_string(),
            0x05 => crate::i18n::t("Data · eFuse image").to_string(),
            0x80 => crate::i18n::t("Data · web server files").to_string(),
            0x81 => crate::i18n::t("Data · FAT filesystem").to_string(),
            0x82 => crate::i18n::t("Data · SPIFFS filesystem").to_string(),
            0x83 => crate::i18n::t("Data · LittleFS filesystem").to_string(),
            _ => crate::i18n::tf("Data · unknown (0x{:02X})",
                &[("{:02X}", "{:02X}", &format!("{:02X}", subtype))]),
        },
        0x80 => crate::i18n::t("Encrypted data").to_string(),
        _ => crate::i18n::tf("Unknown (0x{:02X})",
            &[("{:02X}", "{:02X}", &format!("{:02X}", ptype))]),
    }
}

// ---------- 字符串提取与分类 ----------

struct ClassifiedStrings {
    versions: Vec<(usize, String)>,
    paths: Vec<(usize, String)>,
    urls: Vec<(usize, String)>,
    network: Vec<(usize, String)>,
    at_cmds: Vec<(usize, String)>,
    errors: Vec<(usize, String)>,
    other: Vec<(usize, String)>,
}

fn classify_strings(data: &[u8]) -> ClassifiedStrings {
    let mut cs = ClassifiedStrings {
        versions: Vec::new(),
        paths: Vec::new(),
        urls: Vec::new(),
        network: Vec::new(),
        at_cmds: Vec::new(),
        errors: Vec::new(),
        other: Vec::new(),
    };
    let mut current = String::new();
    let mut start = 0usize;
    let mut total = 0usize;
    let flush = |cs: &mut ClassifiedStrings, s: String, off: usize| {
        let lower = s.to_ascii_lowercase();
        if s.starts_with("AT+") || s.starts_with("at+") {
            cs.at_cmds.push((off, s));
        } else if lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("mqtt://")
            || lower.starts_with("ws://")
            || lower.starts_with("wss://")
        {
            cs.urls.push((off, s));
        } else if lower.contains("version")
            || lower.contains("ver ")
            || looks_like_version(&s)
            || looks_like_date(&s)
            || lower.contains("build")
        {
            cs.versions.push((off, s));
        } else if s.contains('/')
            && (lower.contains(".c")
                || lower.contains(".h")
                || lower.contains(".cpp")
                || lower.contains(".py")
                || lower.contains(".ld")
                || lower.contains(".txt"))
        {
            cs.paths.push((off, s));
        } else if lower.contains("wifi")
            || lower.contains("ssid")
            || lower.contains("hostname")
            || lower.contains("gateway")
            || lower.contains("mqtt")
            || lower.contains("http")
            || lower.contains("socket")
            || lower.contains("tcp")
            || lower.contains("udp")
            || lower.contains("tls")
            || lower.contains("dns")
            || lower.contains("bluetooth")
            || lower.contains("ble")
        {
            cs.network.push((off, s));
        } else if lower.contains("error")
            || lower.contains("err")
            || lower.contains("fail")
            || lower.contains("assert")
            || lower.contains("invalid")
            || lower.contains("timeout")
            || lower.contains("abort")
            || lower.contains("panic")
            || lower.contains("esp_err")
        {
            cs.errors.push((off, s));
        } else {
            cs.other.push((off, s));
        }
    };
    for (i, &b) in data.iter().enumerate() {
        if (0x20..=0x7E).contains(&b) {
            if current.is_empty() {
                start = i;
            }
            current.push(b as char);
        } else {
            if current.len() >= 5 && has_consecutive_alpha(&current, 3) {
                flush(&mut cs, current.clone(), start);
                total += 1;
                if total > 20000 {
                    break; // 防御：超大镜像封顶
                }
            }
            current.clear();
        }
    }
    if current.len() >= 5 && has_consecutive_alpha(&current, 3) {
        flush(&mut cs, current, start);
    }
    cs
}

/// 是否像版本号（v1.2 / v5.1.2 / 1.2.3 之类）
fn looks_like_version(s: &str) -> bool {
    let t = s.trim();
    let t = t.strip_prefix('v').or_else(|| t.strip_prefix('V')).unwrap_or(t);
    let mut parts = t.split('.');
    let first_ok = parts.next().map(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit())).unwrap_or(false);
    if !first_ok {
        return false;
    }
    // 至少还有一段数字
    parts
        .next()
        .map(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false)
}

/// 是否像日期（2024 / Mar 27 2024 / 20240327）
fn looks_like_date(s: &str) -> bool {
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    months.iter().any(|m| s.contains(m)) || s.contains("2023") || s.contains("2024") || s.contains("2025") || s.contains("2026")
}

// ---------- 框架/组件识别 ----------

fn detect_frameworks(data: &[u8]) -> Vec<(&'static str, &'static str)> {
    let markers: &[(&[u8], &'static str, &'static str)] = &[
        (b"Tasmota", "Tasmota", crate::i18n::t("Open-source smart-home firmware (Home Assistant ecosystem)")),
        (b"ESPHome", "ESPHome", crate::i18n::t("Home automation firmware (generated from YAML)")),
        (b"esphome", "ESPHome", crate::i18n::t("Home automation firmware (generated from YAML)")),
        (b"WLED", "WLED", crate::i18n::t("LED strip control firmware")),
        (b"MicroPython", "MicroPython", crate::i18n::t("Firmware that runs Python scripts")),
        (b"micropython", "MicroPython", crate::i18n::t("Firmware that runs Python scripts")),
        (b"NodeMCU", "NodeMCU", crate::i18n::t("Firmware that runs Lua scripts")),
        (b"AT version", crate::i18n::t("ESP-AT official firmware"), crate::i18n::t("Espressif official AT-command firmware (controlled over serial)")),
        (b"esp_matter", "ESP-Matter", crate::i18n::t("Matter smart-home standard protocol")),
        (b"ESP_MATTER", "ESP-Matter", crate::i18n::t("Matter smart-home standard protocol")),
        (b"esp-zigbee", "Zigbee", crate::i18n::t("Zigbee mesh protocol")),
        (b"zephyr", "Zephyr", crate::i18n::t("Zephyr real-time OS")),
        (b"NuttX", "Nuttx", crate::i18n::t("NuttX real-time OS")),
        (b"esp-hal", "Rust (esp-hal)", crate::i18n::t("Developed in Rust")),
        (b"esp_rust", "Rust (esp-hal)", crate::i18n::t("Developed in Rust")),
        (b"LVGL", "LVGL", crate::i18n::t("Embedded GUI library")),
        (b"lvgl", "LVGL", crate::i18n::t("Embedded GUI library")),
        (b"Arduino", crate::i18n::t("Arduino framework"), crate::i18n::t("Arduino ecosystem (simplified development)")),
        (b"arduino", crate::i18n::t("Arduino framework"), crate::i18n::t("Arduino ecosystem (simplified development)")),
        (b"FreeRTOS", "FreeRTOS", crate::i18n::t("Real-time OS (task scheduling kernel)")),
        (b"freertos", "FreeRTOS", crate::i18n::t("Real-time OS (task scheduling kernel)")),
        (b"esp-idf", "ESP-IDF", crate::i18n::t("Espressif official development framework")),
        (b"ESP-IDF", "ESP-IDF", crate::i18n::t("Espressif official development framework")),
    ];
    let mut found: Vec<(&'static str, &'static str)> = Vec::new();
    for (pat, name, desc) in markers {
        if contains_sub(data, pat) && !found.iter().any(|(n, _)| *n == *name) {
            found.push((name, desc));
        }
    }
    found
}

fn contains_sub(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|w| w == needle)
}

// ---------- 工具 ----------

fn fmt_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

// ---------- 主入口 ----------

/// 导出固件易读解析报告（Markdown）
///
/// 数据来源优先级：① 读取的 Flash 数据（last_read_data）
///                ② 备份文件路径（backup_path，直接分析本地 .bin，无需连接设备）
pub(crate) fn export_readable_report(fs: &mut FlasherState, is_zh: bool) {
    let (data, from_file) = if !fs.last_read_data.is_empty() {
        (fs.last_read_data.clone(), false)
    } else {
        let path = fs.backup_path.trim().to_string();
        if path.is_empty() {
            log_push(fs, crate::i18n::t("<< No firmware data: read Flash first, or set a backup .bin path"));
            return;
        }
        match std::fs::read(&path) {
            Ok(d) => {
                log_push(fs, crate::i18n::tf(">> Loaded backup {} ({} bytes)", &[("{}", "{}", &format!("{}", path)), ("{}", "{}", &format!("{}", d.len()))]));
                (d, true)
            }
            Err(e) => {
                log_push(fs, crate::i18n::tf("<< Failed to read backup: {}", &[("{}", "{}", &format!("{}", e))]));
                return;
            }
        }
    };
    // 文件模式按完整 Flash 镜像处理（偏移=Flash 地址，基址 0）；
    // 设备模式用实际读取基址
    let base_addr = if from_file { 0 } else { fs.address };
    let chip_label = fs.chip_type.label(is_zh).to_string();
    let report = build_readable_report(&data, base_addr, &chip_label, is_zh, from_file);

    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let default_name = format!("firmware_report_{}.md", timestamp);
    let path = if let Some(p) = rfd::FileDialog::new()
        .set_file_name(&default_name)
        .add_filter("Markdown", &["md", "txt"])
        .save_file()
    {
        p
    } else {
        let mut tmp = std::env::temp_dir();
        tmp.push(&default_name);
        tmp
    };
    match std::fs::write(&path, &report) {
        Ok(()) => {
            log_push(fs, crate::i18n::tf("<< Readable report exported: {}", &[("{}", "{}", &format!("{}", path.display()))]));
            // macOS 下自动在 Finder 中定位文件
            if cfg!(target_os = "macos") {
                if let Some(dir) = path.parent() {
                    let _ = std::process::Command::new("open")
                        .args(["-R", &path.display().to_string()])
                        .current_dir(dir)
                        .spawn();
                }
            }
        }
        Err(e) => {
            log_push(fs, crate::i18n::tf("<< Export failed: {}", &[("{}", "{}", &format!("{}", e))]));
        }
    }
}

fn short_label(s: &str) -> &str {
    s.split(['（', '(']).next().unwrap_or(s)
}

fn build_readable_report(data: &[u8], base_addr: u32, chip_label: &str, _is_zh: bool, from_file: bool) -> String {
    let mut o = String::new();
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");

    o.push_str(crate::i18n::t("# Firmware Analysis Report (Readable)\n\n"));
    let source = if from_file {
        (crate::i18n::t("Backup file (.bin)")).to_string()
    } else {
        format!("{} @ 0x{:08X}", crate::i18n::t("Flash read"), base_addr)
    };
    o.push_str(&crate::i18n::tf(
        "report.meta",
        &[("{}", "{}", &format!("{}", crate::i18n::t("Generated"))),
          ("{}", "{}", &format!("{}", now)),
          ("{}", "{}", &format!("{}", crate::i18n::t("Source"))),
          ("{}", "{}", &format!("{}", source)),
          ("{}", "{}", &format!("{}", crate::i18n::t("Size"))),
          ("{}", "{}", &format!("{}", data.len())),
          ("{}", "{}", &format!("{}", fmt_size(data.len() as u64)))],
    ));

    // ===== ESP32 镜像解析 =====
    let is_esp_image = data.len() >= 8 && data[0] == 0xE9;
    let mut seg_count = 0u8;
    let mut entry = 0u32;
    let mut spi_mode_str = "";
    let mut spi_speed_str = "";
    let mut spi_size_str = "";
    let mut chip_name = "";
    let mut seg_infos: Vec<(u32, usize, usize)> = Vec::new();
    let esp8266 = chip_label.contains("ESP8266");

    if is_esp_image {
        seg_count = data[1];
        entry = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        spi_mode_str = match data[2] {
            0 => crate::i18n::t("QIO (quad, full duplex, best performance)"),
            1 => crate::i18n::t("QOUT (quad output)"),
            2 => crate::i18n::t("DIO (dual, the most compatible standard mode)"),
            3 => crate::i18n::t("DOUT (dual output, most conservative)"),
            _ => crate::i18n::t("Unknown"),
        };
        let speed = (data[3] >> 4) & 0xF;
        spi_speed_str = match speed {
            0 => crate::i18n::t("40MHz (medium)"),
            1 => crate::i18n::t("26MHz (low speed, most stable)"),
            2 => crate::i18n::t("20MHz (low speed)"),
            3 => crate::i18n::t("80MHz (high speed, best performance)"),
            _ => crate::i18n::t("Unknown"),
        };
        spi_size_str = match data[3] & 0xF {
            0 => "1MB",
            1 => "2MB",
            2 => "4MB",
            3 => "8MB",
            4 => "16MB",
            _ => crate::i18n::t("Unknown"),
        };
        let chip_id = if data.len() >= 14 {
            u16::from_le_bytes([data[12], data[13]])
        } else {
            0
        };
        chip_name = match chip_id {
            0x0000 => crate::i18n::t("ESP32 (classic, dual-core Wi-Fi + Bluetooth)"),
            0x0002 => crate::i18n::t("ESP32-S2 (single-core, USB, no Bluetooth)"),
            0x0009 => crate::i18n::t("ESP32-S3 (dual-core, USB, AI instruction acceleration)"),
            0x000D => crate::i18n::t("ESP32-C3 (RISC-V single-core, USB, low cost)"),
            0x0012 => crate::i18n::t("ESP32-H2 (IEEE 802.15.4, no Wi-Fi)"),
            0x002D => crate::i18n::t("ESP32-C2 (compact low cost)"),
            _ => crate::i18n::t("ESP32 series"),
        };
        // 段解析（ESP8266 头 8 字节，ESP32 头 24 字节）
        let mut offset = if esp8266 { 8 } else { 24 };
        for _ in 0..seg_count.min(16) {
            if offset + 8 > data.len() {
                break;
            }
            let load = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let len = u32::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]) as usize;
            seg_infos.push((load, len, offset + 8));
            offset += 8 + len;
        }
    }

    // 应用描述符 / 分区表 / 框架 / 字符串
    let descs = find_app_descs(data);
    let parts = parse_partitions(data, base_addr);
    let frameworks = detect_frameworks(data);
    let cs = classify_strings(data);

    // ===== 一、一句话总结 =====
    o.push_str(crate::i18n::t("## What is this?\n\n"));
    if is_esp_image {
        let desc_str = if let Some(d) = descs.first() {
            crate::i18n::tf(
                "report.summary.desc",
                &[("{}", "{}", &format!("{}", if d.project_name.is_empty() { "?" } else { &d.project_name })),
                  ("{}", "{}", &format!("{}", if d.version.is_empty() { "?" } else { &d.version })),
                  ("{}", "{}", &format!("{}", if d.date.is_empty() { "?" } else { &d.date })),
                  ("{}", "{}", &format!("{}", if d.time.is_empty() { "" } else { &d.time }))],
            )
        } else {
            String::new()
        };
        let fw_str = frameworks
            .iter()
            .map(|(n, _)| n.to_string())
            .collect::<Vec<_>>()
            .join(" + ");
        let fw_note = if fw_str.is_empty() {
            String::new()
        } else {
            crate::i18n::tf("report.summary.framework",
                &[("{}", "{}", &format!("{}", fw_str))])
        };
        let part_note = if parts.is_empty() {
            String::new()
        } else {
            crate::i18n::tf("report.summary.partitions",
                &[("{}", "{}", &format!("{}", parts.len()))])
        };
        o.push_str(&crate::i18n::tf(
            "report.summary.esp32",
            &[("{}", "{}", &format!("{}", short_label(chip_name))),
              ("{}", "{}", &format!("{}", desc_str)),
              ("{}", "{}", &format!("{}", fw_note)),
              ("{}", "{}", &format!("{}", part_note)),
              ("{:08X}", "{:08X}", &format!("{:08X}", entry)),
              ("{}", "{}", &format!("{}", seg_count))],
        ));
    } else {
        o.push_str(&crate::i18n::tf(
            "report.summary.generic",
            &[("{}", "{}", &format!("{}", chip_label)),
              ("{}", "{}", &format!("{}", crate::i18n::t("no ESP32 firmware header found")))],
        ));
    }

    // ===== 二、应用信息（固件自述） =====
    if !descs.is_empty() {
        o.push_str(crate::i18n::t("## App info (built-in)\n\n"));
        for (i, d) in descs.iter().enumerate() {
            if descs.len() > 1 {
                o.push_str(&crate::i18n::tf("report.app_multi", &[("{}", "{}", &format!("{}", i + 1))]));
            }
            o.push_str(crate::i18n::t("report.table.app_info.header"));
            if !d.project_name.is_empty() {
                o.push_str(&crate::i18n::tf("report.table.app_info.project", &[("{}", "{}", &format!("{}", d.project_name))]));
            }
            if !d.version.is_empty() {
                o.push_str(&crate::i18n::tf("report.table.app_info.version", &[("{}", "{}", &format!("{}", d.version))]));
            }
            if !d.date.is_empty() || !d.time.is_empty() {
                o.push_str(&crate::i18n::tf("report.table.app_info.build_time",
                    &[("{}", "{}", &format!("{}", d.date)), ("{}", "{}", &format!("{}", d.time))]));
            }
            if !d.idf_ver.is_empty() {
                o.push_str(&crate::i18n::tf("report.table.app_info.idf_version",
                    &[("{}", "{}", &format!("{}", d.idf_ver))]));
            }
            o.push('\n');
        }
    }

    // ===== 三、分区表 =====
    if !parts.is_empty() {
        o.push_str(crate::i18n::t("## Flash partition layout\n\n"));
        o.push_str(crate::i18n::t("report.table.partitions.header"));
        for p in &parts {
            o.push_str(&format!(
                "| {} | {} | 0x{:08X} | {} |\n",
                p.label,
                p.type_name,
                p.offset,
                fmt_size(p.size as u64),
            ));
        }
        o.push('\n');
    }

    // ===== 四、段布局 =====
    if !seg_infos.is_empty() {
        o.push_str(crate::i18n::t("## Segments\n\n"));
        o.push_str(crate::i18n::t("report.table.segments.header"));
        for (i, (load, len, _)) in seg_infos.iter().enumerate() {
            let desc = if is_iram_addr(*load) {
                crate::i18n::t("Program code — copied to RAM and executed")
            } else if (0x3C000000..0x3D000000).contains(load) {
                crate::i18n::t("Read-only data — strings/constants, stays in Flash and read on demand")
            } else {
                crate::i18n::t("Initial values of globals — copied to RAM at boot")
            };
            o.push_str(&format!("| {} | 0x{:08X} | {} | {} |\n", i, load, fmt_size(*len as u64), desc));
        }
        o.push('\n');
    }

    // ===== 五、芯片与 Flash 配置白话解读 =====
    if is_esp_image {
        o.push_str(crate::i18n::t("## Chip & Flash config\n\n"));
        o.push_str(crate::i18n::t("report.table.chip_config.header"));
        o.push_str(&crate::i18n::tf("report.table.chip_config.chip",
            &[("{}", "{}", &format!("{}", short_label(chip_name))),
              ("{}", "{}", &format!("{}", chip_name))]));
        o.push_str(&crate::i18n::tf("report.table.chip_config.flash_mode",
            &[("{}", "{}", &format!("{}", short_label(spi_mode_str)))]));
        o.push_str(&crate::i18n::tf("report.table.chip_config.flash_clock",
            &[("{}", "{}", &format!("{}", short_label(spi_speed_str)))]));
        o.push_str(&crate::i18n::tf("report.table.chip_config.flash_size",
            &[("{}", "{}", &format!("{}", spi_size_str))]));
        o.push_str(&crate::i18n::tf("report.table.chip_config.entry",
            &[("{:08X}", "{:08X}", &format!("{:08X}", entry))]));
        o.push('\n');
    }

    // ===== 六、框架/组件 =====
    if !frameworks.is_empty() {
        o.push_str(crate::i18n::t("## Frameworks\n\n"));
        for (name, desc) in &frameworks {
            o.push_str(&crate::i18n::tf("report.framework_item",
                &[("{}", "{}", &format!("{}", name)), ("{}", "{}", &format!("{}", desc))]));
        }
        o.push('\n');
    }

    // ===== 七、字符串分类 =====
    o.push_str(crate::i18n::t("## Embedded strings (classified)\n\n"));
    let push_list = |o: &mut String, title: &str, items: &[(usize, String)], limit: usize, _is_zh: bool| {
        if items.is_empty() {
            return;
        }
        o.push_str(&format!("### {}\n\n", title));
        for (off, s) in items.iter().take(limit) {
            let display = if s.len() > 80 { format!("{}…", &s[..78]) } else { s.clone() };
            o.push_str(&crate::i18n::tf("report.string_item",
                &[("{}", "{}", &format!("{}", display)),
                  ("{:08X}", "{:08X}", &format!("{:08X}", base_addr + *off as u32))]));
        }
        if items.len() > limit {
            o.push_str(&format!(
                "- …{} {}\n",
                items.len() - limit,
                crate::i18n::t("more"),
            ));
        }
        o.push('\n');
    };
    push_list(&mut o, crate::i18n::t("Version info"), &cs.versions, 12, _is_zh);
    push_list(&mut o, crate::i18n::t("Source paths"), &cs.paths, 10, _is_zh);
    push_list(&mut o, crate::i18n::t("URLs"), &cs.urls, 8, _is_zh);
    push_list(&mut o, crate::i18n::t("Network"), &cs.network, 12, _is_zh);
    push_list(&mut o, crate::i18n::t("AT commands"), &cs.at_cmds, 10, _is_zh);
    push_list(&mut o, crate::i18n::t("Errors & debug"), &cs.errors, 12, _is_zh);
    push_list(&mut o, crate::i18n::t("Other"), &cs.other, 15, _is_zh);

    // ===== 八、数据统计与健康检查 =====
    let mut freq = [0u32; 256];
    for &b in data {
        freq[b as usize] += 1;
    }
    let size = data.len();
    let entropy: f64 = freq
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / size as f64;
            -p * p.log2()
        })
        .sum();
    let zero_count = freq[0] as usize;
    let last_valid = data.iter().rposition(|&b| b != 0xFF).unwrap_or(0);
    let used = last_valid + 1;

    o.push_str(crate::i18n::t("## Health check\n\n"));
    if is_esp_image {
        o.push_str(crate::i18n::t("report.health.esp_header"));
    }
    if !parts.is_empty() {
        o.push_str(&crate::i18n::tf("report.health.partitions_ok",
            &[("{}", "{}", &format!("{}", parts.len()))]));
    }
    if !descs.is_empty() {
        o.push_str(crate::i18n::t("report.health.app_desc_ok"));
    }
    if entropy > 7.5 {
        o.push_str(&crate::i18n::tf("report.health.entropy_high",
            &[("{:.2}", "{:.2}", &format!("{:.2}", entropy))]));
    } else {
        o.push_str(&crate::i18n::tf("report.health.entropy_normal",
            &[("{:.2}", "{:.2}", &format!("{:.2}", entropy))]));
    }
    if used < size {
        o.push_str(&crate::i18n::tf(
            "report.health.used_ratio",
            &[("{}", "{}", &format!("{}", fmt_size(used as u64))),
              ("{}", "{}", &format!("{}", fmt_size(size as u64))),
              ("{}", "{}", &format!("{}", used * 100 / size))],
        ));
    }
    if zero_count > size / 2 {
        o.push_str(&crate::i18n::tf("report.health.zero_ratio",
            &[("{}", "{}", &format!("{}", zero_count * 100 / size))]));
    }
    o.push('\n');
    o.push_str(crate::i18n::t("> Auto-generated readable report; use 'Export Disasm' for full instruction-level analysis.\n"));
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一份合成 ESP32-S3 固件镜像（镜像头 + 段 + app_desc + 分区表 + 字符串），
    /// 离线验证报告解析逻辑（无需连接真实设备）。
    #[test]
    fn test_readable_report_esp32_s3() {
        let mut data = vec![0xFFu8; 160 * 1024]; // 160KB 模拟 Flash 备份
        // ===== 镜像头（0x0，bootloader）=====
        data[0] = 0xE9; // magic
        data[1] = 2; // 段数
        data[2] = 2; // DIO
        data[3] = 0x02; // speed=0(40MHz) size=2(4MB)
        data[4..8].copy_from_slice(&0x4037_8000u32.to_le_bytes()); // entry
        data[12..14].copy_from_slice(&0x0009u16.to_le_bytes()); // chip_id = S3
        // 段0：DROM（只读数据），头 @24，数据 64B
        data[24..28].copy_from_slice(&0x3C00_0000u32.to_le_bytes());
        data[28..32].copy_from_slice(&64u32.to_le_bytes());
        // 段1：IRAM（代码），头 @24+8+64=96（段0 数据之后）
        data[96..100].copy_from_slice(&0x4037_8000u32.to_le_bytes());
        data[100..104].copy_from_slice(&32u32.to_le_bytes());

        // ===== 应用描述符（模拟 app @0x10000 + 0x20）=====
        let desc_off = 0x10000 + 0x20;
        data[desc_off..desc_off + 4].copy_from_slice(&APP_DESC_MAGIC.to_le_bytes());
        data[desc_off + 16..desc_off + 16 + 5].copy_from_slice(b"1.2.0"); // version
        data[desc_off + 48..desc_off + 48 + 10].copy_from_slice(b"my_project"); // project_name
        data[desc_off + 80..desc_off + 80 + 5].copy_from_slice(b"15:32"); // time
        data[desc_off + 96..desc_off + 96 + 11].copy_from_slice(b"Mar 27 2024"); // date
        data[desc_off + 112..desc_off + 112 + 6].copy_from_slice(b"v5.1.2"); // idf_ver

        // ===== 分区表（0x8000）=====
        let pt = 0x8000;
        let entries: [(&str, u8, u8, u32, u32); 3] = [
            ("nvs", 0x01, 0x02, 0x9000, 24 * 1024),
            ("phy_init", 0x01, 0x01, 0xF000, 4 * 1024),
            ("factory", 0x00, 0x00, 0x10000, 1024 * 1024),
        ];
        for (i, (label, t, st, poff, psize)) in entries.iter().enumerate() {
            let e = pt + i * 32;
            data[e..e + 2].copy_from_slice(&0xAA50u16.to_le_bytes());
            data[e + 2] = *t;
            data[e + 3] = *st;
            data[e + 4..e + 8].copy_from_slice(&poff.to_le_bytes());
            data[e + 8..e + 12].copy_from_slice(&psize.to_le_bytes());
            data[e + 12..e + 12 + label.len()].copy_from_slice(label.as_bytes());
        }

        // ===== 字符串（分类验证）=====
        let strings_off = 0x20000;
        let s = b"esp-idf v5.1.2\0FreeRTOS\0https://example.com\0/srv/build/main.c\0AT+GMR\0wifi_connect\0assert failed\0";
        data[strings_off..strings_off + s.len()].copy_from_slice(s);

        let report = build_readable_report(&data, 0, "ESP32", true, true);
        // 一句话总结 + 芯片识别
        assert!(report.contains("ESP32-S3"), "应识别芯片型号: {}", &report[..200.min(report.len())]);
        // 应用描述符
        assert!(report.contains("my_project"), "应提取项目名");
        assert!(report.contains("1.2.0"), "应提取版本");
        assert!(report.contains("Mar 27 2024"), "应提取编译日期");
        assert!(report.contains("v5.1.2"), "应提取 IDF 版本");
        // 分区表
        assert!(report.contains("nvs"), "应解析分区 nvs");
        assert!(report.contains("factory"), "应解析分区 factory");
        assert!(report.contains(crate::i18n::t("Data · config store (nvs)")), "分区应有通俗说明");
        // 段布局
        assert!(report.contains(crate::i18n::t("Read-only data — strings/constants, stays in Flash and read on demand")), "段布局应有通俗说明");
        assert!(report.contains(crate::i18n::t("Program code — copied to RAM and executed")), "IRAM 段应标注为代码");
        // Flash 配置
        assert!(report.contains("40MHz"), "应解读 Flash 时钟");
        assert!(report.contains("4MB"), "应解读 Flash 容量");
        // 框架
        assert!(report.contains("ESP-IDF"), "应识别 ESP-IDF 框架");
        assert!(report.contains("FreeRTOS"), "应识别 FreeRTOS（ESP-IDF 固件内置）");
        // 字符串分类
        assert!(report.contains("https://example.com"), "URL 应归入网络地址");
        assert!(report.contains("AT+GMR"), "AT 指令应被识别");
        assert!(report.contains("/srv/build/main.c"), "路径应被提取");
        // 健康检查
        assert!(report.contains(&crate::i18n::tf("report.health.partitions_ok", &[("{}", "{}", &format!("{}", 3))])), "健康检查应确认分区表");
    }

    /// 非 ESP 镜像数据（无 0xE9 头）应优雅回退，不 panic。
    #[test]
    fn test_readable_report_generic_data() {
        let mut data = vec![0u8; 4096];
        let s = b"STM32 HAL driver version 1.8.0\0/error/socket timeout\0";
        data[..s.len()].copy_from_slice(s);
        let report = build_readable_report(&data, 0x0800_0000, "STM32", true, false);
        assert!(report.contains(crate::i18n::t("no ESP32 firmware header found")), "应提示非 ESP 固件");
        assert!(report.contains("version"), "字符串分类应工作");
    }
}
