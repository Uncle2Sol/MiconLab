// AI 感知辅助模块
//
// 提供可复用的 AI 提示条 UI 组件和 sense() 感知函数，
// 让所有面板能以最低成本（几行代码）接入 AI 智能感知能力。
//
// 使用方式：
//   1. 调用 sense_xxx() 函数获取 Option<AiHint>
//   2. 如果有提示，调用 render_ai_hint_bar(ui, &hint) 渲染
//
// 示例：
//   if let Some(hint) = ai_sense::sense_device(vid, pid, zh) {
//       ai_sense::render_ai_hint_bar(ui, &hint);
//   }

use crate::theme;
use egui::{Color32, RichText};
use miconlab_ai::{
    DiagnosticContext, DiagnosticEngine, FirmwareAnalysis, KnowledgeBase, ProtocolClass,
    ProtocolClassifier, StreamFeatures,
};

/// AI 提示条内容
#[derive(Clone)]
pub struct AiHint {
    /// 图标 emoji
    pub icon: &'static str,
    /// 边框/图标颜色
    pub color: Color32,
    /// 主文本（单行摘要）
    pub text: String,
    /// 可选详情（可多行，渲染为小字）
    pub detail: Option<String>,
}

impl AiHint {
    pub fn new(icon: &'static str, color: Color32, text: impl Into<String>) -> Self {
        Self {
            icon,
            color,
            text: text.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// 渲染 AI 提示条（紧凑型，一行图标 + 文本 + 可展开详情）
pub fn render_ai_hint_bar(ui: &mut egui::Ui, hint: &AiHint) {
    egui::Frame::group(&ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, hint.color))
        .inner_margin(egui::Margin::same(5.0))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(hint.icon).size(13.0));
                ui.label(
                    RichText::new(format!("AI: {}", hint.text))
                        .color(hint.color)
                        .strong()
                        .small(),
                );
            });
            if let Some(detail) = &hint.detail {
                ui.add_space(2.0);
                // 可展开的详情区域
                let detail_id = ui.id().with("ai_detail");
                egui::CollapsingHeader::new(crate::i18n::t("📋 View detailed steps"))
                    .id_salt(detail_id)
                    .default_open(false)
                    .show(ui, |ui| {
                        for line in detail.lines() {
                            ui.label(
                                RichText::new(line)
                                    .color(theme::TEXT_SECONDARY)
                                    .small()
                                    .family(egui::FontFamily::Monospace),
                            );
                        }
                    });
            }
        });
}

/// 渲染 AI 协议识别徽章（紧凑型，用于终端/Modbus 等面板顶部）
pub fn render_protocol_badge(ui: &mut egui::Ui, class: ProtocolClass, confidence: f32) {
    let label = crate::i18n::t_owned(class.label_en());
    let conf_color = if confidence >= 0.7 {
        theme::SUCCESS
    } else if confidence >= 0.4 {
        theme::WARNING
    } else {
        theme::TEXT_DISABLED
    };
    egui::Frame::group(&ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, conf_color))
        .inner_margin(egui::Margin::same(4.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("🤖").size(11.0));
                ui.label(RichText::new(label).color(conf_color).strong().small());
                ui.label(
                    RichText::new(format!("{:.0}%", confidence * 100.0))
                        .color(theme::TEXT_DISABLED)
                        .small(),
                );
            });
        });
}

// ==================== Sense 感知函数 ====================

/// 设备识别感知：基于 VID/PID 查询知识库
pub fn sense_device(vid: u16, pid: u16, _zh: bool) -> Option<AiHint> {
    let kb = KnowledgeBase::with_builtin_devices();
    kb.lookup(vid, pid).map(|dev| {
        let text = { crate::i18n::tf("Detected {} ({:04X}:{:04X})", &[("{}", "{}", &format!("{}", dev.chip_name)), ("{:04X}", "{:04X}", &format!("{:04X}", vid)), ("{:04X}", "{:04X}", &format!("{:04X}", pid))]) };
        let detail = { crate::i18n::tf("{} | Protocols: {}", &[("{}", "{}", &crate::i18n::t_owned(&dev.description)), ("{}", "{}", &format!("{}", dev.protocol_hints.join(", ")))]) };
        AiHint::new("🔌", theme::CYAN, text).with_detail(detail)
    })
}

/// 固件分析摘要：按当前界面语言组装（架构名、地址等技术标识原样嵌入）。
///
/// `summary_zh` / `summary_en` 是分析阶段拼好的定长字符串，无法作为 key 查表，
/// 因此这里用 `firmware.summary` 模板 + 运行时参数重建。
pub fn firmware_summary(a: &FirmwareAnalysis) -> String {
    let arch = crate::i18n::t(a.arch.label_en());
    let size = if a.size >= 1024 {
        format!("{}KB", a.size / 1024)
    } else {
        format!("{}B", a.size)
    };
    let entry = a
        .entry_addr
        .map(|e| format!("0x{e:08X}"))
        .unwrap_or_else(|| "—".to_string());
    let sp = a
        .sp_init
        .map(|s| format!("0x{s:08X}"))
        .unwrap_or_else(|| "—".to_string());
    let strings = a.strings.len().to_string();
    let conf = format!("{:.0}", a.confidence * 100.0);
    crate::i18n::tf(
        "firmware.summary",
        &[
            ("{arch}", "{arch}", arch),
            ("{size}", "{size}", &size),
            ("{entry}", "{entry}", &entry),
            ("{sp}", "{sp}", &sp),
            ("{strings}", "{strings}", &strings),
            ("{conf}", "{conf}", &conf),
        ],
    )
}

/// 固件智能分析感知
#[allow(dead_code)] // 预留给固件面板接入，当前尚无调用点
pub fn sense_firmware(data: &[u8], _zh: bool) -> Option<AiHint> {
    if data.len() < 8 {
        return None;
    }
    let analysis = FirmwareAnalysis::analyze(data);
    if analysis.confidence < 0.3 {
        return None;
    }
    Some(AiHint::new("📦", theme::ACCENT, firmware_summary(&analysis)))
}

/// 协议识别感知：从接收数据中识别协议类型
pub fn sense_protocol(rx_data: &[u8]) -> Option<(ProtocolClass, f32, AiHint)> {
    if rx_data.len() < 16 {
        return None;
    }
    let classifier = ProtocolClassifier::with_builtin_templates();
    let features = StreamFeatures::extract(rx_data, None);
    let (class, conf) = classifier.predict(&features);
    if class == ProtocolClass::Unknown || conf < 0.3 {
        return None;
    }
    let label = crate::i18n::t_owned(class.label_en());
    let desc = crate::i18n::t_owned(class.description_en());
    let hint = AiHint::new("📡", theme::CYAN, format!(
        "{} {} ({:.0}%)",
        crate::i18n::t("Protocol detected"),
        label, conf * 100.0
    )).with_detail(desc.to_string());
    Some((class, conf, hint))
}

/// 错误诊断感知：基于错误消息推断根因
pub fn sense_error(error_msg: &str, is_connected: bool, baud: u32, zh: bool) -> Option<AiHint> {
    sense_error_full(error_msg, is_connected, baud, &[], None, None, zh)
}

/// 错误诊断感知（完整版）：带上下文信息（接收数据/VID:PID/端口名）
pub fn sense_error_full(
    error_msg: &str,
    is_connected: bool,
    baud: u32,
    last_rx_data: &[u8],
    vid_pid: Option<(u16, u16)>,
    port_name: Option<&str>,
    _zh: bool, // 诊断引擎输出当前为中文模板，保留参数位待 i18n
) -> Option<AiHint> {
    if error_msg.is_empty() {
        return None;
    }
    let mut ctx = DiagnosticContext::new();
    ctx.error_message = Some(error_msg.to_string());
    ctx.is_connected = is_connected;
    ctx.baud_rate = baud;
    // 自动检测 Bootloader 相关错误
    let err_lower = error_msg.to_lowercase();
    if err_lower.contains("bootloader") || err_lower.contains("ack") || err_lower.contains("0x79") || err_lower.contains("0x7f") {
        ctx.bootloader_attempt = true;
    }
    // 提供上下文：最近接收数据（截取最近 256 字节）+ VID/PID + 端口名
    if !last_rx_data.is_empty() {
        let start = last_rx_data.len().saturating_sub(256);
        ctx.last_rx_data = last_rx_data[start..].to_vec();
    }
    ctx.vid_pid = vid_pid;
    ctx.port_name = port_name.map(|s| s.to_string());
    let engine = DiagnosticEngine::with_builtin_rules();
    let results = engine.diagnose(&ctx);
    // 找最高严重度的诊断
    let best = results.iter().max_by_key(|r| match r.severity {
        miconlab_ai::Severity::Critical => 4,
        miconlab_ai::Severity::Error => 3,
        miconlab_ai::Severity::Warning => 2,
        miconlab_ai::Severity::Info => 1,
    })?;
    if best.confidence < 0.3 {
        return None;
    }
    let suggestion = best
        .suggestions
        .first()
        .map(|s| crate::i18n::t_owned(s))
        .unwrap_or_default();
    let text = format!("{} → {}", crate::i18n::t_owned(&best.diagnosis), suggestion);
    // 如果建议超过 1 条，将所有建议合并为详情
    let detail = if best.suggestions.len() > 1 {
        Some(
            best.suggestions
                .iter()
                .map(|s| crate::i18n::t_owned(s))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    } else {
        None
    };
    let color = match best.severity {
        miconlab_ai::Severity::Critical | miconlab_ai::Severity::Error => theme::ERROR,
        miconlab_ai::Severity::Warning => theme::WARNING,
        miconlab_ai::Severity::Info => theme::CYAN,
    };
    let hint = AiHint::new("🔍", color, text);
    if let Some(d) = detail {
        Some(hint.with_detail(d))
    } else {
        Some(hint)
    }
}

/// 波特率不匹配感知：检测数据是否像乱码（高控制字符比例）
pub fn sense_baud_mismatch(rx_data: &[u8], current_baud: u32, _zh: bool) -> Option<AiHint> {
    if rx_data.len() < 32 {
        return None;
    }
    let features = StreamFeatures::extract(rx_data, None);
    // 乱码特征：高熵 + 高控制字符比例 + 低可打印比例
    let is_garbled = features.shannon_entropy > 6.5
        && features.control_ratio > 0.3
        && features.printable_ratio < 0.3;
    if !is_garbled {
        return None;
    }
    // 建议常见波特率
    let suggestions = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];
    let alt = suggestions.iter().find(|&&b| b != current_baud);
    let text = { crate::i18n::tf("Data looks garbled, {} baud may be mismatched", &[("{}", "{}", &format!("{}", current_baud))]) };
    let detail = alt.map(|b| {
        crate::i18n::tf("💡 Try {} baud", &[("{}", "{}", &format!("{}", b))])
    });
    let mut hint = AiHint::new("⚠", theme::WARNING, text);
    hint.detail = detail;
    Some(hint)
}

/// 从 rx_buffer 收集最近 N 条 RX 数据
pub fn collect_recent_rx(rx_buffer: &[crate::state::RxEntry], max_bytes: usize) -> Vec<u8> {
    // 逆序收集各段（新→旧），最后对「段列表」reverse 恢复时间顺序。
    // 注意不能对拼接后的 flat 字节数组 reverse，那会翻转每段内部的字节序。
    let mut segments: Vec<&[u8]> = Vec::new();
    let mut total = 0usize;
    for entry in rx_buffer.iter().rev() {
        if entry.direction != crate::state::Direction::Rx {
            continue;
        }
        if total + entry.data.len() > max_bytes {
            let remaining = max_bytes.saturating_sub(total);
            if remaining > 0 {
                // 最旧的一段只取其尾部 remaining 字节（时间最近的字节）
                segments.push(&entry.data[entry.data.len() - remaining..]);
            }
            break;
        }
        segments.push(&entry.data);
        total += entry.data.len();
    }
    segments.reverse();
    segments.concat()
}
