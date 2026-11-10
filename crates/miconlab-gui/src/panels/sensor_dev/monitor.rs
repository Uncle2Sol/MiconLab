// 实时采集监控子面板
//
// 功能：
// 1. 采集控制：开始/停止/清空
// 2. 多通道实时波形图：用 egui::Painter 绘制滚动波形
//    - X 轴 = 时间（秒），Y 轴 = 电压/物理量
//    - 多通道叠加显示，每通道不同颜色
//    - 自动滚动，显示最近 N 秒数据
// 3. 数值表格：当前值/最大/最小/平均/标准差
// 4. 自动记录：切换开关，开启后数据写入 CSV
//
// 数据来源：从 state.rx_buffer 获取最新串口数据，按通道数解析为通道值，存入 SampleBuffer。
// 状态通过 state_manager 在 CentralPanel 外部加载/保存，闭包内传入 &mut MonitorState。

use crate::state::AppState;
use crate::theme;
use egui::{Color32, Pos2, RichText, Shape, Stroke};
use miconlab_core::acquisition::{AcquisitionConfig, SampleBuffer};
use miconlab_core::product::{ChecksumType, DataFormat, ProtocolConfig, ProtocolType};
use std::path::PathBuf;
use std::time::Instant;

/// 监控面板状态
#[derive(Clone)]
pub struct MonitorState {
    /// 是否正在采集
    sampling: bool,
    /// 采样缓冲区（每通道一个 Vec<f64>）
    buffer: SampleBuffer,
    /// 当前使用的采集配置（从向导同步）
    config: AcquisitionConfig,
    /// 当前协议配置（从向导同步，用于按协议解析串口数据）
    protocol_config: ProtocolConfig,
    /// 波形显示时间窗口（秒）
    time_window: f64,
    /// 是否自动记录到 CSV
    auto_record: bool,
    /// CSV 记录文件路径
    record_path: String,
    /// 上次已处理的 bytes_rx 偏移（用于增量读取）
    last_bytes_rx: u64,
    /// 采集开始时间（用于计算时间戳）
    start_time: Option<Instant>,
    /// Y 轴自动量程：当前显示范围
    y_min: f64,
    y_max: f64,
    /// 未解析的原始字节缓冲（用于跨帧拼接）
    raw_tail: Vec<u8>,
    /// 上次连接会话 ID，用于检测连接变化并清除设备派生数据
    last_session_id: u64,
}

impl Default for MonitorState {
    fn default() -> Self {
        Self::new()
    }
}

impl MonitorState {
    pub fn new() -> Self {
        // 使用默认 WizardState 初始化，首帧 sync_config 会同步实际配置
        let default_ws = super::wizard::WizardState::default();
        let config = super::wizard::current_config(&default_ws);
        let protocol_config = super::wizard::current_protocol_config(&default_ws);
        let channel_count = config.channels.len().max(1);
        Self {
            sampling: false,
            buffer: SampleBuffer::new(channel_count, 10000),
            config,
            protocol_config,
            time_window: 5.0,
            auto_record: false,
            record_path: String::new(),
            last_bytes_rx: 0,
            start_time: None,
            y_min: 0.0,
            y_max: 3.3,
            raw_tail: Vec::new(),
            last_session_id: 0,
        }
    }

    /// 当通道数变化时重建缓冲区
    fn sync_config(&mut self, ws: &super::wizard::WizardState) {
        let new_cfg = super::wizard::current_config(ws);
        let new_proto = super::wizard::current_protocol_config(ws);
        let new_ch = new_cfg.channels.len().max(1);
        let old_ch = self.config.channels.len().max(1);
        if new_ch != old_ch {
            self.buffer = SampleBuffer::new(new_ch, 10000);
            self.raw_tail.clear();
        }
        self.config = new_cfg;
        self.protocol_config = new_proto;
    }
}

/// 渲染实时监控子面板
pub fn show(_ctx: &egui::Context, state: &mut AppState, ui: &mut egui::Ui, ws: &mut super::wizard::WizardState, s: &mut MonitorState) {
    let zh = state.language == crate::i18n::Language::Zh;
    let connected = state.is_connected();

    s.sync_config(ws);

    // 连接会话变化时清除设备派生数据（波形缓冲、原始尾包），避免跨设备残留
    if s.last_session_id != state.connection_session_id {
        s.buffer.clear();
        s.raw_tail.clear();
        s.last_bytes_rx = state.bytes_rx;
        s.start_time = None;
        s.last_session_id = state.connection_session_id;
    }

    // 副标题
    ui.label(
        RichText::new(crate::i18n::t("Monitor sensor data streams in real time, auto-parse and display value trends"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    // ===== 采集控制栏 =====
    ui.horizontal(|ui| {
        // 开始/停止按钮
        let (btn_text, btn_color) = if s.sampling {
            (
                { crate::i18n::t("Stop##panels/sensor_dev/monitor.rs:3776") }.to_string(),
                theme::ERROR,
            )
        } else {
            (
                { crate::i18n::t("Start##panels/sensor_dev/monitor.rs:3913") }.to_string(),
                theme::SUCCESS,
            )
        };
        let btn_enabled = connected || s.sampling;
        let btn = egui::Button::new(
            RichText::new(btn_text).color(Color32::BLACK).strong(),
        )
        .fill(btn_color)
        .min_size(egui::Vec2::new(120.0, 28.0))
        .stroke(egui::Stroke::NONE);
        if ui
            .add_enabled(btn_enabled, btn)
            .on_hover_text(crate::i18n::t("Start/stop real-time acquisition. Begins from the current RX byte count, parses serial data per wizard-configured channels/protocol, and plots the waveform"))
            .clicked() {
            if s.sampling {
                // 停止
                s.sampling = false;
                s.start_time = None;
                // 停止时若开启了自动记录，导出 CSV
                if s.auto_record && !s.record_path.is_empty() {
                    let path = PathBuf::from(&s.record_path);
                    if let Err(e) = s.buffer.export_csv(&path) {
                        state.last_error = Some(e);
                    }
                }
            } else {
                // 开始：以当前 bytes_rx 为基准，避免回填历史数据
                s.sampling = true;
                s.last_bytes_rx = state.bytes_rx;
                s.start_time = Some(Instant::now());
                s.buffer.clear();
                s.raw_tail.clear();
                super::wizard::mark_started(ws);
            }
        }

        ui.add_space(8.0);
        // 清空按钮
        if ui
            .button(RichText::new(crate::i18n::t("Clear")).strong())
            .on_hover_text(crate::i18n::t("Clear waveform buffer and stats (does not affect the CSV file)"))
            .clicked()
        {
            s.buffer.clear();
            s.raw_tail.clear();
        }

        ui.add_space(12.0);
        // 采集状态指示
        let (dot, dot_color) = if s.sampling {
            ("\u{25CF}", theme::SUCCESS)
        } else {
            ("\u{25CB}", theme::TEXT_DISABLED)
        };
        ui.label(
            RichText::new(format!(
                "{} {}",
                dot,
                if s.sampling {
                    crate::i18n::t("Capturing")
                } else {
                    crate::i18n::t("Stopped")
                }
            ))
            .color(dot_color)
            .small(),
        );

        ui.add_space(16.0);
        // 自动记录开关
        ui.checkbox(&mut s.auto_record, crate::i18n::t("Auto Record CSV"))
            .on_hover_text(crate::i18n::t("When enabled, samples are streamed to a CSV file (timestamp + channel values), auto-flushed on stop. Defaults to the system temp directory"));
        if s.auto_record && s.record_path.is_empty() {
            // 默认记录路径
            let dir = std::env::temp_dir();
            let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
            s.record_path = dir
                .join(format!("miconlab_monitor_{}.csv", ts))
                .to_string_lossy()
                .to_string();
        }
        if s.auto_record {
            ui.label(
                RichText::new(format!(
                    "{} {}",
                    crate::i18n::t("File:"),
                    s.record_path
                ))
                .color(theme::TEXT_SECONDARY)
                .small()
                .family(egui::FontFamily::Monospace),
            );
        }

        ui.add_space(8.0);
        // 时间窗口调节
        ui.label(
            RichText::new(crate::i18n::t("Window"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::DragValue::new(&mut s.time_window)
                .range(1.0..=60.0)
                .speed(0.1)
                .suffix("s"),
        )
        .on_hover_text(crate::i18n::t("Time window length on the waveform X-axis (1-60s). Larger windows show longer trends but denser waveforms"));
    });

    // 未连接提示
    if !connected && !s.sampling {
        ui.add_space(4.0);
        ui.label(
            RichText::new(crate::i18n::t("⚠ Not connected. Open Connection panel first."))
            .color(theme::WARNING)
            .small(),
        );
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    // ===== 喂入新数据 =====
    if s.sampling {
        feed_new_data(s, state);
    }

    // ===== 波形显示区 =====
    ui.label(
        RichText::new(crate::i18n::t("Real-time Waveform"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);
    render_waveform(ui, &s, zh);

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    // ===== 统计表格 =====
    render_stats_table(ui, &s, zh);

    // ===== AI 传感器智能分析 =====
    render_ai_analysis(ui, &s);
}

/// 从 state.rx_buffer 增量获取新数据，解析为通道值并推入 SampleBuffer
fn feed_new_data(s: &mut MonitorState, state: &AppState) {
    // 防御：如果 bytes_rx 回绕（如被外部重置），重置基准避免永久失效
    if state.bytes_rx < s.last_bytes_rx {
        s.last_bytes_rx = state.bytes_rx;
        s.raw_tail.clear();
        return;
    }
    if state.bytes_rx <= s.last_bytes_rx {
        return;
    }
    let new_count = (state.bytes_rx - s.last_bytes_rx) as usize;
    s.last_bytes_rx = state.bytes_rx;

    // 从 rx_buffer 末尾向前收集恰好 new_count 个字节（只取 RX 方向的数据）。
    // 段级收集后对「段列表」reverse；不能对拼接后的 flat 数组 reverse，
    // 那会翻转每段内部的字节序（U16LE 高低字节互换、波形数据错乱）。
    let mut segments: Vec<&[u8]> = Vec::new();
    let mut collected = 0usize;
    for entry in state.rx_buffer.iter().rev() {
        if collected >= new_count {
            break;
        }
        // 跳过发送方向的数据
        if entry.direction != crate::state::Direction::Rx {
            continue;
        }
        let needed = new_count - collected;
        if entry.data.len() <= needed {
            segments.push(&entry.data);
            collected += entry.data.len();
        } else {
            let start = entry.data.len() - needed;
            segments.push(&entry.data[start..]);
            collected += needed;
        }
    }
    segments.reverse();
    let new_bytes: Vec<u8> = segments.concat();

    if new_bytes.is_empty() {
        return;
    }

    // 将新字节追加到 raw_tail，然后按协议类型解析
    s.raw_tail.extend_from_slice(&new_bytes);

    let start = match s.start_time {
        Some(t) => t,
        None => {
            s.start_time = Some(Instant::now());
            s.start_time.unwrap()
        }
    };

    // 根据协议类型解析数据，返回 (timestamp, channel_values) 列表
    let samples = parse_protocol_data(s, start);
    for (ts, values) in samples {
        s.buffer.push(ts, &values);
    }

    // 更新 Y 轴自动量程
    update_y_range(s);
}

/// 根据协议类型解析 raw_tail 中的数据，返回 (timestamp, channel_values) 列表。
/// 各子解析函数直接操作 s.raw_tail（消费已解析字节）并返回采样点。
fn parse_protocol_data(s: &mut MonitorState, start: Instant) -> Vec<(f64, Vec<f64>)> {
    match s.protocol_config.protocol_type {
        ProtocolType::SerialFrame => parse_serial_frame(s, start),
        ProtocolType::ModbusRtu => parse_modbus_response(s, start),
        _ => parse_raw_data(s, start), // ADC/I2C/SPI 直接按数据格式解析
    }
}

/// 获取当前协议的数据格式（无帧格式定义时默认 U16LE）
fn get_data_format(s: &MonitorState) -> DataFormat {
    if let Some(ff) = &s.protocol_config.frame_format {
        ff.data_format.clone()
    } else {
        DataFormat::U16LE
    }
}

/// 返回数据格式每个样本占用的字节数
fn data_format_size(format: &DataFormat) -> usize {
    match format {
        DataFormat::U8 => 1,
        DataFormat::U16LE | DataFormat::U16BE => 2,
        DataFormat::U32LE | DataFormat::U32BE | DataFormat::F32LE | DataFormat::F32BE => 4,
    }
}

/// 按数据格式解析单个样本为 f64
fn parse_data_bytes(bytes: &[u8], format: &DataFormat) -> f64 {
    match format {
        DataFormat::U8 => bytes[0] as f64,
        DataFormat::U16LE => u16::from_le_bytes([bytes[0], bytes[1]]) as f64,
        DataFormat::U16BE => u16::from_be_bytes([bytes[0], bytes[1]]) as f64,
        DataFormat::U32LE => {
            u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64
        }
        DataFormat::U32BE => {
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64
        }
        DataFormat::F32LE => {
            f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64
        }
        DataFormat::F32BE => {
            f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64
        }
    }
}

/// 按数据格式直接解析（无帧同步），适用于 ADC/I2C/SPI 等裸数据流。
/// 按通道数 × 每通道字节数 切分样本组，应用校准后输出。
fn parse_raw_data(s: &mut MonitorState, start: Instant) -> Vec<(f64, Vec<f64>)> {
    let channel_count = s.config.channels.len().max(1);
    let data_format = get_data_format(s);
    let sample_size = data_format_size(&data_format);
    let bytes_per_sample = channel_count * sample_size;

    let mut results = Vec::new();
    let mut consumed = 0usize;
    while s.raw_tail.len() - consumed >= bytes_per_sample {
        let sample_bytes = &s.raw_tail[consumed..consumed + bytes_per_sample];
        let mut values: Vec<f64> = Vec::with_capacity(channel_count);
        for ch in 0..channel_count {
            let off = ch * sample_size;
            let raw = parse_data_bytes(&sample_bytes[off..off + sample_size], &data_format);
            // 应用采集配置的校准/增益/偏移
            let calibrated = s.config.apply_calibration(ch, raw);
            values.push(calibrated);
        }
        let ts = start.elapsed().as_secs_f64();
        results.push((ts, values));
        consumed += bytes_per_sample;
    }
    // 保留未消费的尾部字节
    if consumed > 0 {
        s.raw_tail.drain(0..consumed);
    }
    results
}

/// 解析串口自定义帧：使用帧头/帧尾同步，提取通道数据。
/// 帧结构：[header][data...][checksum][footer]
fn parse_serial_frame(s: &mut MonitorState, start: Instant) -> Vec<(f64, Vec<f64>)> {
    let ff = match &s.protocol_config.frame_format {
        Some(ff) => ff.clone(),
        None => return parse_raw_data(s, start), // 无帧格式定义，退化为直接解析
    };

    let mut results = Vec::new();
    let channel_count = s.config.channels.len().max(1);
    let data_size = data_format_size(&ff.data_format);
    let checksum_size = match ff.checksum_type {
        ChecksumType::None => 0,
        ChecksumType::Sum8 | ChecksumType::Xor => 1,
        ChecksumType::Crc16 => 2,
    };

    loop {
        // 1. 查找帧头
        let header_pos = if ff.header.is_empty() {
            0
        } else {
            match s
                .raw_tail
                .windows(ff.header.len())
                .position(|w| w == ff.header.as_slice())
            {
                Some(p) => p,
                None => {
                    // 保留尾部可能的不完整帧头字节，丢弃前面的垃圾数据
                    let keep = ff.header.len().saturating_sub(1);
                    if s.raw_tail.len() > keep {
                        s.raw_tail.drain(..s.raw_tail.len() - keep);
                    }
                    break;
                }
            }
        };
        if header_pos > 0 {
            s.raw_tail.drain(..header_pos);
        }

        // 2. 查找帧尾，确定整帧边界
        let after_header = ff.header.len();
        let frame_end = if !ff.footer.is_empty() {
            match s.raw_tail[after_header..]
                .windows(ff.footer.len())
                .position(|w| w == ff.footer.as_slice())
            {
                Some(p) => after_header + p + ff.footer.len(),
                None => break, // 帧尾未到达，等待更多数据
            }
        } else {
            // 无帧尾：按固定长度（帧头 + 通道数据 + 校验）截取
            let total = after_header + ff.channel_count * data_size + checksum_size;
            if s.raw_tail.len() < total {
                break;
            }
            total
        };

        // 3. 提取整帧并从 raw_tail 移除
        let frame: Vec<u8> = s.raw_tail.drain(..frame_end).collect();

        // 4. 提取通道数据区：[header][data_offset][data...][checksum][footer]
        let data_start = after_header + ff.data_offset;
        let data_end = frame.len().saturating_sub(ff.footer.len() + checksum_size);
        let mut values: Vec<f64> = Vec::with_capacity(channel_count);
        for i in 0..channel_count {
            let off = data_start + i * data_size;
            if off + data_size > data_end {
                break;
            }
            let raw = parse_data_bytes(&frame[off..off + data_size], &ff.data_format);
            let calibrated = s.config.apply_calibration(i, raw);
            values.push(calibrated);
        }

        if !values.is_empty() {
            let ts = start.elapsed().as_secs_f64();
            results.push((ts, values));
        }
    }
    results
}

/// 解析 Modbus RTU 响应帧，提取寄存器数据。
/// 响应帧格式：[slave_id][func_code][byte_count][data...][CRC16 LE]
fn parse_modbus_response(s: &mut MonitorState, start: Instant) -> Vec<(f64, Vec<f64>)> {
    let mut results = Vec::new();
    let channel_count = s.config.channels.len().max(1);
    let slave_id = s.protocol_config.slave_id.unwrap_or(1);

    loop {
        // 最小帧长度：slave(1) + func(1) + count(1) + CRC(2) = 5
        if s.raw_tail.len() < 5 {
            break;
        }

        // 1. 匹配从站 ID
        if s.raw_tail[0] != slave_id {
            // 丢弃一个字节，继续搜索
            s.raw_tail.remove(0);
            continue;
        }

        let func_code = s.raw_tail[1];
        // 异常响应：[slave_id][func_code | 0x80][exception_code][CRC16]，共 5 字节
        if func_code & 0x80 != 0 {
            if s.raw_tail.len() < 5 {
                break;
            }
            s.raw_tail.drain(..5);
            continue;
        }

        // 2. 计算整帧长度
        let byte_count = s.raw_tail[2] as usize;
        let frame_len = 3 + byte_count + 2; // slave + func + count + data + CRC16
        if s.raw_tail.len() < frame_len {
            break; // 帧不完整，等待更多数据
        }

        // 3. 验证 CRC16
        let data = &s.raw_tail[..frame_len];
        let crc_data = &data[..frame_len - 2];
        let crc_recv = u16::from_le_bytes([data[frame_len - 2], data[frame_len - 1]]);
        let crc_calc = miconlab_core::checksum::crc16_modbus(crc_data);
        if crc_recv != crc_calc {
            // CRC 错误，丢弃首字节继续搜索
            s.raw_tail.remove(0);
            continue;
        }

        // 4. 提取寄存器数据（每个寄存器 2 字节，大端）
        let reg_data = &s.raw_tail[3..3 + byte_count];
        let mut values: Vec<f64> = Vec::with_capacity(channel_count);
        for i in 0..channel_count {
            let off = i * 2;
            if off + 2 > byte_count {
                break;
            }
            let raw = u16::from_be_bytes([reg_data[off], reg_data[off + 1]]) as f64;
            let calibrated = s.config.apply_calibration(i, raw);
            values.push(calibrated);
        }

        s.raw_tail.drain(..frame_len);

        if !values.is_empty() {
            let ts = start.elapsed().as_secs_f64();
            results.push((ts, values));
        }
    }
    results
}

/// 根据当前缓冲区数据更新 Y 轴显示范围
fn update_y_range(s: &mut MonitorState) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for ch in 0..s.config.channels.len() {
        let data = s.buffer.channel_data(ch);
        for &v in data {
            if v < min {
                min = v;
            }
            if v > max {
                max = v;
            }
        }
    }
    if min.is_finite() && max.is_finite() {
        if (max - min).abs() < f64::EPSILON {
            // 单值情况，扩展一点范围
            s.y_min = min - 1.0;
            s.y_max = max + 1.0;
        } else {
            // 留 10% 边距
            let margin = (max - min) * 0.1;
            s.y_min = min - margin;
            s.y_max = max + margin;
        }
    }
}

/// 渲染实时波形图
fn render_waveform(ui: &mut egui::Ui, s: &MonitorState, _zh: bool) {
    let avail = ui.available_size();
    let height = 220.0_f32;
    let (rect, _response) =
        ui.allocate_exact_size(egui::Vec2::new(avail.x, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);

    // 背景填充
    painter.rect_filled(rect, 0.0, theme::BG_DEEP);

    // 边框
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, theme::BORDER));

    // 绘制网格（5x5）
    let grid_color = theme::BORDER;
    for i in 1..5 {
        let x = rect.left() + (rect.width() * i as f32 / 5.0);
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(0.5_f32, grid_color),
        );
        let y = rect.top() + (rect.height() * i as f32 / 5.0);
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            Stroke::new(0.5_f32, grid_color),
        );
    }

    // Y 轴标签
    let y_range = s.y_max - s.y_min;
    if y_range.abs() > f64::EPSILON {
        for i in 0..=5 {
            let y = rect.top() + (rect.height() * i as f32 / 5.0);
            let val = s.y_max - (y_range * i as f64 / 5.0);
            painter.text(
                Pos2::new(rect.left() + 2.0, y),
                egui::Align2::LEFT_CENTER,
                format!("{:.2}", val),
                egui::FontId::new(9.0, egui::FontFamily::Monospace),
                theme::TEXT_SECONDARY,
            );
        }
    }

    // X 轴标签（时间，秒）
    for i in 0..=5 {
        let x = rect.left() + (rect.width() * i as f32 / 5.0);
        let t = s.time_window * i as f64 / 5.0;
        painter.text(
            Pos2::new(x, rect.bottom() + 2.0),
            egui::Align2::CENTER_TOP,
            format!("{:.1}s", t),
            egui::FontId::new(9.0, egui::FontFamily::Monospace),
            theme::TEXT_SECONDARY,
        );
    }

    // 绘制每通道波形
    let timestamps = &s.buffer.timestamps;
    let t_now = if timestamps.is_empty() {
        0.0
    } else {
        *timestamps.last().unwrap_or(&0.0)
    };
    let t_start = (t_now - s.time_window).max(0.0);
    let t_span = (t_now - t_start).max(1e-6);

    let y_min = s.y_min;
    let y_max = s.y_max;
    let y_span = (y_max - y_min).max(1e-6);

    let x_for = |t: f64| -> f32 {
        let ratio = ((t - t_start) / t_span).clamp(0.0, 1.0) as f32;
        rect.left() + ratio * rect.width()
    };
    let y_for = |v: f64| -> f32 {
        let ratio = ((v - y_min) / y_span).clamp(0.0, 1.0) as f32;
        rect.bottom() - ratio * rect.height()
    };

    let mut any_drawn = false;
    for (ch_idx, ch_cfg) in s.config.channels.iter().enumerate() {
        let data = s.buffer.channel_data(ch_idx);
        if data.len() < 2 {
            continue;
        }

        // 找到时间窗口内的数据点
        let points: Vec<Pos2> = timestamps
            .iter()
            .zip(data.iter())
            .filter(|(t, _)| **t >= t_start)
            .map(|(t, v)| Pos2::new(x_for(*t), y_for(*v)))
            .collect();

        if points.len() < 2 {
            continue;
        }
        any_drawn = true;

        let color = Color32::from_rgb(ch_cfg.color[0], ch_cfg.color[1], ch_cfg.color[2]);
        painter.add(Shape::line(points, Stroke::new(1.5_f32, color)));

        // 末端标记
        if let Some(last) = timestamps
            .iter()
            .zip(data.iter())
            .filter(|(t, _)| **t >= t_start)
            .map(|(t, v)| Pos2::new(x_for(*t), y_for(*v)))
            .last()
        {
            painter.circle_filled(last, 2.5, color);
        }
    }

    if !any_drawn {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            crate::i18n::t("No data (click Start)"),
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
            theme::TEXT_DISABLED,
        );
    }

    // 通道图例
    let legend_y = rect.top() + 4.0;
    let legend_x = rect.right() - 4.0;
    painter.text(
        Pos2::new(legend_x, legend_y),
        egui::Align2::RIGHT_TOP,
        crate::i18n::t("Channels"),
        egui::FontId::new(9.0, egui::FontFamily::Proportional),
        theme::TEXT_SECONDARY,
    );
    let mut ly = legend_y + 14.0;
    for (ch_idx, ch_cfg) in s.config.channels.iter().enumerate() {
        let color = Color32::from_rgb(ch_cfg.color[0], ch_cfg.color[1], ch_cfg.color[2]);
        painter.line_segment(
            [
                Pos2::new(legend_x - 60.0, ly + 5.0),
                Pos2::new(legend_x - 48.0, ly + 5.0),
            ],
            Stroke::new(2.0_f32, color),
        );
        painter.text(
            Pos2::new(legend_x - 44.0, ly),
            egui::Align2::LEFT_TOP,
            &ch_cfg.name,
            egui::FontId::new(9.0, egui::FontFamily::Monospace),
            theme::TEXT_PRIMARY,
        );
        ly += 12.0;
        let _ = ch_idx;
    }
}

/// 渲染统计表格
fn render_stats_table(ui: &mut egui::Ui, s: &MonitorState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Channel Statistics"))
            .color(theme::CYAN)
            .strong(),
    )
    .on_hover_text(crate::i18n::t("Per-channel live statistics: current/min/max/avg/std. Channel count and unit come from the acquisition wizard config; colors match the waveform"));
    ui.add_space(4.0);

    let channel_count = s.config.channels.len();
    if channel_count == 0 {
        ui.label(
            RichText::new(crate::i18n::t("No channels"))
                .color(theme::TEXT_DISABLED)
                .small(),
        );
        return;
    }

    egui::Grid::new("monitor_stats_grid")
        .num_columns(7)
        .spacing([12.0, 3.0])
        .striped(true)
        .show(ui, |ui| {
            // 表头
            ui.label(RichText::new(crate::i18n::t("Channel")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(RichText::new(crate::i18n::t("Unit")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(RichText::new(crate::i18n::t("Current")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(RichText::new(crate::i18n::t("Min")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(RichText::new(crate::i18n::t("Max")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(RichText::new(crate::i18n::t("Avg")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(RichText::new(crate::i18n::t("Std")).color(theme::TEXT_SECONDARY).small().strong());
            ui.end_row();

            for (ch_idx, ch_cfg) in s.config.channels.iter().enumerate() {
                let stats = s.buffer.stats(ch_idx);
                let color = Color32::from_rgb(ch_cfg.color[0], ch_cfg.color[1], ch_cfg.color[2]);
                ui.label(
                    RichText::new(&ch_cfg.name)
                        .color(color)
                        .family(egui::FontFamily::Monospace),
                );
                ui.label(
                    RichText::new(&ch_cfg.unit)
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
                ui.label(RichText::new(format!("{:.4}", stats.current)).color(theme::TEXT_PRIMARY).family(egui::FontFamily::Monospace));
                ui.label(RichText::new(format!("{:.4}", stats.min)).color(theme::TEXT_PRIMARY).family(egui::FontFamily::Monospace));
                ui.label(RichText::new(format!("{:.4}", stats.max)).color(theme::TEXT_PRIMARY).family(egui::FontFamily::Monospace));
                ui.label(RichText::new(format!("{:.4}", stats.avg)).color(theme::TEXT_PRIMARY).family(egui::FontFamily::Monospace));
                ui.label(RichText::new(format!("{:.4}", stats.std)).color(theme::TEXT_PRIMARY).family(egui::FontFamily::Monospace));
                ui.end_row();
            }
        });

    // 样本数显示
    ui.add_space(4.0);
    ui.label(
        RichText::new(format!(
            "{}: {}  |  {}: {:.1} Hz",
            { crate::i18n::t("Samples") },
            s.buffer.len(),
            crate::i18n::t("Rate"),
            s.config.sample_rate,
        ))
        .color(theme::TEXT_SECONDARY)
        .small()
        .family(egui::FontFamily::Monospace),
    );
}

/// 渲染 AI 传感器智能分析
///
/// 从采样缓冲区提取多通道数据，调用 miconlab-ai 的 SensorAnalysis 进行：
/// - 传感器类型推断（压力/温度/加速度/陀螺仪/IMU/电压等）
/// - 数据质量评分（SNR + 稳定性 + 异常综合）
/// - 异常检测（突变/漂移/卡死/通道丢失/高噪声/周期性干扰）
/// - 推荐采样率
fn render_ai_analysis(ui: &mut egui::Ui, s: &MonitorState) {
    let channel_count = s.config.channels.len();
    if channel_count == 0 || s.buffer.len() < 30 {
        return;
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    // 收集通道数据
    let channel_refs: Vec<&[f64]> = (0..channel_count)
        .map(|ch| s.buffer.channel_data(ch))
        .collect();
    let analysis = miconlab_ai::SensorAnalysis::analyze(&channel_refs);

    // 标题
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("🤖").size(16.0));
        ui.label(
            egui::RichText::new(crate::i18n::t("AI Sensor Analysis"))
                .color(theme::CYAN)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (q_color, q_label) = if analysis.quality_score >= 0.8 {
                (theme::SUCCESS, crate::i18n::t("Excellent"))
            } else if analysis.quality_score >= 0.6 {
                (theme::WARNING, crate::i18n::t("Good"))
            } else if analysis.quality_score >= 0.4 {
                (theme::WARNING, crate::i18n::t("Fair"))
            } else {
                (theme::ERROR, crate::i18n::t("Poor"))
            };
            ui.label(
                egui::RichText::new(format!("{}: {} ({:.0}%)", crate::i18n::t("Quality"), q_label, analysis.quality_score * 100.0))
                    .color(q_color)
                    .strong()
                    .small(),
            );
        });
    });

    ui.add_space(4.0);

    // 分析摘要
    egui::Frame::group(&ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            egui::Grid::new("sensor_ai_grid")
                .num_columns(2)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    // 传感器类型
                    ui.label(egui::RichText::new(crate::i18n::t("Sensor Type")).color(theme::TEXT_SECONDARY).small());
                    ui.horizontal(|ui| {
                        let type_color = if analysis.type_confidence >= 0.7 { theme::SUCCESS } else if analysis.type_confidence >= 0.4 { theme::WARNING } else { theme::TEXT_DISABLED };
                        ui.label(egui::RichText::new(crate::i18n::t_owned(analysis.sensor_type.label_en())).color(type_color).strong().small());
                        ui.label(egui::RichText::new(format!("({:.0}%)", analysis.type_confidence * 100.0)).color(theme::TEXT_DISABLED).small());
                        ui.label(egui::RichText::new(crate::i18n::tf("| Recommended unit: {unit}", &[("{unit}", "{unit}", analysis.sensor_type.recommended_unit())])).color(theme::TEXT_SECONDARY).small());
                    });
                    ui.end_row();

                    // 信噪比
                    ui.label(egui::RichText::new(crate::i18n::t("SNR")).color(theme::TEXT_SECONDARY).small());
                    let snr_color = if analysis.snr_db >= 30.0 { theme::SUCCESS } else if analysis.snr_db >= 15.0 { theme::WARNING } else { theme::ERROR };
                    ui.label(egui::RichText::new(format!("{:.1} dB", analysis.snr_db)).color(snr_color).small().family(egui::FontFamily::Monospace));
                    ui.end_row();

                    // 稳定性
                    ui.label(egui::RichText::new(crate::i18n::t("Stability")).color(theme::TEXT_SECONDARY).small());
                    let stab_color = if analysis.stability >= 0.7 { theme::SUCCESS } else if analysis.stability >= 0.4 { theme::WARNING } else { theme::ERROR };
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{:.0}%", analysis.stability * 100.0)).color(stab_color).small().family(egui::FontFamily::Monospace));
                        // 稳定性进度条
                        let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(100.0, 6.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, theme::BG_SURFACE);
                        let mut fill = rect;
                        fill.set_width(rect.width() * analysis.stability.clamp(0.0, 1.0));
                        ui.painter().rect_filled(fill, 2.0, stab_color);
                    });
                    ui.end_row();

                    // 推荐采样率
                    ui.label(egui::RichText::new(crate::i18n::t("Recommended Rate")).color(theme::TEXT_SECONDARY).small());
                    let current_rate = s.config.sample_rate;
                    let recommended = analysis.recommended_sample_rate;
                    let rate_color = if (current_rate - recommended).abs() / recommended.max(1.0) < 0.2 {
                        theme::SUCCESS
                    } else {
                        theme::WARNING
                    };
                    ui.label(egui::RichText::new(crate::i18n::tf("{rate} Hz (current: {current} Hz)", &[("{rate}", "{rate}", &format!("{:.0}", recommended)), ("{current}", "{current}", &format!("{:.0}", current_rate))])).color(rate_color).small().family(egui::FontFamily::Monospace));
                    ui.end_row();
                });
        });

    // 异常列表
    if !analysis.anomalies.is_empty() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(crate::i18n::t("Detected Anomalies")).color(theme::ERROR).strong().small());
        ui.add_space(2.0);
        egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
            for (kind, desc) in &analysis.anomalies {
                let (icon, color) = match kind {
                    miconlab_ai::SensorAnomaly::Spike => ("⚡", theme::WARNING),
                    miconlab_ai::SensorAnomaly::Drift => ("📈", theme::WARNING),
                    miconlab_ai::SensorAnomaly::Stuck => ("🚫", theme::ERROR),
                    miconlab_ai::SensorAnomaly::Overflow => ("⚠", theme::ERROR),
                    miconlab_ai::SensorAnomaly::ChannelLoss => ("✗", theme::ERROR),
                    miconlab_ai::SensorAnomaly::HighNoise => ("🔊", theme::WARNING),
                    miconlab_ai::SensorAnomaly::PeriodicInterference => ("🔄", theme::WARNING),
                };
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(icon).size(12.0));
                    ui.label(egui::RichText::new(crate::i18n::t_owned(kind.label_zh())).color(color).strong().small());
                    ui.label(egui::RichText::new(desc).color(theme::TEXT_SECONDARY).small());
                });
            }
        });
    }

    // 通道级统计
    ui.add_space(6.0);
    ui.label(egui::RichText::new(crate::i18n::t("AI Per-Channel Analysis")).color(theme::TEXT_SECONDARY).small());
    ui.add_space(2.0);
    egui::Grid::new("sensor_ai_ch_grid")
        .num_columns(6)
        .spacing([10.0, 2.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label(egui::RichText::new(crate::i18n::t("Ch")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(egui::RichText::new(crate::i18n::t("Mean")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(egui::RichText::new(crate::i18n::t("Std")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(egui::RichText::new(crate::i18n::t("CV")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(egui::RichText::new(crate::i18n::t("Range")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(egui::RichText::new(crate::i18n::t("Status")).color(theme::TEXT_SECONDARY).small().strong());
            ui.end_row();
            for cs in &analysis.channel_stats {
                let ch_name = s.config.channels.get(cs.channel).map(|c| c.name.as_str()).unwrap_or("?");
                ui.label(egui::RichText::new(ch_name).color(theme::TEXT_PRIMARY).small().family(egui::FontFamily::Monospace));
                ui.label(egui::RichText::new(format!("{:.3}", cs.mean)).color(theme::TEXT_PRIMARY).small().family(egui::FontFamily::Monospace));
                ui.label(egui::RichText::new(format!("{:.3}", cs.std)).color(theme::TEXT_PRIMARY).small().family(egui::FontFamily::Monospace));
                ui.label(egui::RichText::new(format!("{:.3}", cs.cv)).color(theme::TEXT_PRIMARY).small().family(egui::FontFamily::Monospace));
                ui.label(egui::RichText::new(format!("{:.3}~{:.3}", cs.min, cs.max)).color(theme::TEXT_PRIMARY).small().family(egui::FontFamily::Monospace));
                let (status, color) = if !cs.active {
                    (crate::i18n::t("Inactive"), theme::TEXT_DISABLED)
                } else if cs.cv.abs() > 0.5 {
                    (crate::i18n::t("High noise"), theme::WARNING)
                } else {
                    (crate::i18n::t("Normal"), theme::SUCCESS)
                };
                ui.label(egui::RichText::new(status).color(color).small());
                ui.end_row();
            }
        });
}
