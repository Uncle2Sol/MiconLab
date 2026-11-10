// 3D 力分布热力图子面板
// 功能：阵列配置 / 数据输入(手动/串口/随机) / 2D 热力图 / 3D 曲面图
//       统计(总和/最大/最小/均值/压力中心/接触面积) / CSV 记录
//
// 状态通过 state_manager 在 CentralPanel 外部加载/保存，闭包内传入 &mut HeatmapState。
// 串口采集需调用 state.send_and_read()，通过标志位在 UI 渲染外执行。

use crate::state::AppState;
use crate::theme;
use egui::{Color32, Pos2, RichText, ScrollArea, Stroke};
use miconlab_core::checksum::crc16_modbus;
use miconlab_core::hex::parse_hex;
use miconlab_core::product::{parse_data_bytes, DataFormat};

/// 视图模式
#[derive(Clone, Copy, PartialEq)]
enum ViewMode {
    Heatmap2D,
    Surface3D,
}

/// 热力图统计信息
struct HeatmapStats {
    total: f64,
    max: f64,
    min: f64,
    avg: f64,
    cop_x: f64,
    cop_y: f64,
    contact_count: usize,
    contact_ratio: f64,
}

/// 热力图状态
#[derive(Clone)]
pub struct HeatmapState {
    // 阵列配置
    rows: usize,
    cols: usize,
    data_format: DataFormat,
    // 数据
    values: Vec<f64>,
    // 视图
    view_mode: ViewMode,
    // 串口采集
    serial_cmd: String,
    wait_ms: u64,
    header_hex: String,
    // CSV 记录
    csv_path: String,
    recording: bool,
    csv_data: Vec<String>,
    record_count: u64,
    /// 上次连接会话 ID，用于检测连接变化并清除设备派生数据
    last_session_id: u64,
}

impl Default for HeatmapState {
    fn default() -> Self {
        Self {
            rows: 8,
            cols: 8,
            data_format: DataFormat::U16BE,
            values: vec![0.0; 64],
            view_mode: ViewMode::Heatmap2D,
            serial_cmd: "01 03 00 00 00 40".to_string(),
            wait_ms: 100,
            header_hex: String::new(),
            csv_path: "heatmap_log.csv".to_string(),
            recording: false,
            csv_data: Vec::new(),
            record_count: 0,
            last_session_id: 0,
        }
    }
}

/// 渲染力分布热力图子面板
pub fn show(_ctx: &egui::Context, state: &mut AppState, ui: &mut egui::Ui, s: &mut HeatmapState) {
    let zh = state.language == crate::i18n::Language::Zh;
    let connected = state.is_connected();

    // 连接会话变化时清除设备派生数据（热力图值、CSV 记录），避免跨设备混合
    if s.last_session_id != state.connection_session_id {
        s.values.fill(0.0);
        s.csv_data.clear();
        s.record_count = 0;
        s.recording = false;
        s.last_session_id = state.connection_session_id;
    }

    // 标题
    ui.heading(
        RichText::new(crate::i18n::t("Force Heatmap"))
            .color(theme::TEXT_PRIMARY)
            .strong(),
    );
    // 副标题：解释面板用途
    ui.label(
        RichText::new(crate::i18n::t("Visualize 2D data distribution with color mapping, suitable for temperature/stress fields"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    // 串口采集标志（需在 UI 借用外执行）
    let mut serial_requested = false;

    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // 阵列配置
            render_config_section(ui, s, zh);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            // 数据输入
            serial_requested = render_data_input_section(ui, s, zh, connected);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            // 可视化
            render_visualization(ui, s, zh);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            // 数据编辑网格
            render_editable_grid(ui, s, zh);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            // 统计
            render_statistics(ui, s, zh);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            // AI 压力模式识别
            render_ai_pressure(ui, s, zh);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            // CSV 记录
            render_recording_section(ui, s, zh);
        });

    // 在 UI 借用外执行串口采集
    if serial_requested {
        do_serial_acquire(state, s, zh);
    }

    // 错误提示
    if let Some(err) = &state.last_error {
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("\u{26A0} {}", err))
                .color(theme::ERROR)
                .small(),
        );
    }
}

// ===== 辅助函数 =====

/// 数值映射为颜色（蓝→青→绿→黄→红）
fn value_to_color(v: f64, vmin: f64, vmax: f64) -> Color32 {
    let t = ((v - vmin) / (vmax - vmin).max(1e-9)).clamp(0.0, 1.0);
    let r = (if t < 0.5 { 0.0 } else { (t - 0.5) * 2.0 } * 255.0) as u8;
    let g = (if t < 0.5 { t * 2.0 } else { 1.0 - (t - 0.5) * 2.0 } * 255.0) as u8;
    let b = (if t < 0.5 { 1.0 - t * 2.0 } else { 0.0 } * 255.0) as u8;
    Color32::from_rgb(r, g, b)
}

/// 数据格式对应的字节数
fn data_format_size(format: &DataFormat) -> usize {
    match format {
        DataFormat::U8 => 1,
        DataFormat::U16BE | DataFormat::U16LE => 2,
        DataFormat::U32BE | DataFormat::U32LE | DataFormat::F32BE | DataFormat::F32LE => 4,
    }
}

/// 调整 values 大小以匹配 rows*cols
fn resize_values(s: &mut HeatmapState) {
    let n = s.rows * s.cols;
    s.values.resize(n, 0.0);
}

/// 计算统计信息
fn compute_stats(s: &HeatmapState) -> HeatmapStats {
    if s.values.is_empty() {
        return HeatmapStats {
            total: 0.0,
            max: 0.0,
            min: 0.0,
            avg: 0.0,
            cop_x: 0.0,
            cop_y: 0.0,
            contact_count: 0,
            contact_ratio: 0.0,
        };
    }

    let total: f64 = s.values.iter().sum();
    let max = s.values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let min = s.values.iter().cloned().fold(f64::INFINITY, f64::min);
    let avg = total / s.values.len() as f64;

    // 接触判定阈值：最大值的 10%
    let threshold = max * 0.1;
    let contact_count = s.values.iter().filter(|&&v| v > threshold).count();
    let contact_ratio = contact_count as f64 / s.values.len() as f64;

    // 压力中心 (Center of Pressure)
    let mut cop_x = 0.0;
    let mut cop_y = 0.0;
    let mut weight_sum = 0.0;
    for r in 0..s.rows {
        for c in 0..s.cols {
            let idx = r * s.cols + c;
            if idx < s.values.len() {
                let v = s.values[idx].max(0.0);
                cop_x += c as f64 * v;
                cop_y += r as f64 * v;
                weight_sum += v;
            }
        }
    }
    if weight_sum > 1e-9 {
        cop_x /= weight_sum;
        cop_y /= weight_sum;
    }

    HeatmapStats {
        total,
        max,
        min,
        avg,
        cop_x,
        cop_y,
        contact_count,
        contact_ratio,
    }
}

/// 随机生成数据（LCG + 高斯峰）
fn do_random_generate(s: &mut HeatmapState) {
    let n = s.rows * s.cols;
    s.values.resize(n, 0.0);

    // LCG 伪随机数生成器
    let mut seed = chrono::Local::now().timestamp_millis() as u64;
    let lcg = |seed: &mut u64| -> f64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((*seed >> 33) as f64) / (1u64 << 31) as f64
    };

    // 基础噪声
    for v in s.values.iter_mut() {
        *v = lcg(&mut seed) * 20.0;
    }

    // 高斯峰
    let cx = s.cols as f64 / 2.0 + (lcg(&mut seed) - 0.5) * s.cols as f64 * 0.6;
    let cy = s.rows as f64 / 2.0 + (lcg(&mut seed) - 0.5) * s.rows as f64 * 0.6;
    let sigma = (s.rows.min(s.cols) as f64 / 4.0).max(1.0);
    let peak = 80.0 + lcg(&mut seed) * 20.0;

    for r in 0..s.rows {
        for c in 0..s.cols {
            let dx = c as f64 - cx;
            let dy = r as f64 - cy;
            let g = (-((dx * dx + dy * dy) / (2.0 * sigma * sigma))).exp();
            let idx = r * s.cols + c;
            if idx < s.values.len() {
                s.values[idx] += g * peak;
            }
        }
    }
}

/// 执行串口采集（在 UI 借用外调用）
fn do_serial_acquire(state: &mut AppState, s: &mut HeatmapState, _zh: bool) {
    // 1. 读取采集参数
    let cmd_bytes = parse_hex(&s.serial_cmd);
    let wait_ms = s.wait_ms;
    let header = parse_hex(&s.header_hex).unwrap_or_default();
    let data_format = s.data_format.clone();
    let rows = s.rows;
    let cols = s.cols;

    let mut bytes = match cmd_bytes {
        Ok(b) if !b.is_empty() => b,
        Ok(_) => {
            state.last_error =
                Some(crate::i18n::t("Empty command").to_string());
            return;
        }
        Err(e) => {
            state.last_error = Some(format!(
                "{}: {}",
                crate::i18n::t("Parse error"),
                e
            ));
            return;
        }
    };

    // 2. 若命令是 6 字节的 Modbus 读请求（缺 CRC），自动追加 CRC16
    //    Modbus RTU 读请求格式: [slave_id][func_code][start_addr(2)][quantity(2)][CRC16 LE]
    let is_modbus_read = is_modbus_read_request(&bytes);
    if is_modbus_read {
        let crc = crc16_modbus(&bytes);
        bytes.extend_from_slice(&crc.to_le_bytes());
    }

    // 3. 发送并读取响应
    match state.send_and_read(&bytes, wait_ms) {
        Ok(resp) => {
            // 4. 更新状态
            // 判断响应是否为 Modbus RTU 响应帧
            // 响应格式: [slave_id][func_code][byte_count][data...][CRC16 LE]
            let data: Vec<u8> = if is_modbus_read && is_modbus_response(&resp, &bytes) {
                // 解析 Modbus RTU 响应，提取 data 区（跳过 slave_id/func_code/byte_count，去除 CRC16）
                extract_modbus_response_data(&resp)
            } else {
                // 非Modbus 响应：剥离帧头后直接作为数据区（适用于 SerialFrame 等协议）
                let data_start = if !header.is_empty()
                    && resp.len() >= header.len()
                    && resp[..header.len()] == header[..]
                {
                    header.len()
                } else {
                    0
                };
                resp[data_start..].to_vec()
            };

            // 按数据格式逐通道解析
            let cell_size = data_format_size(&data_format);
            let n = rows * cols;
            s.values.resize(n, 0.0);

            for i in 0..n {
                let off = i * cell_size;
                s.values[i] = if off + cell_size <= data.len() {
                    parse_data_bytes(&data[off..off + cell_size], &data_format)
                        .unwrap_or(0.0)
                } else {
                    0.0
                };
            }
            state.last_error = None;
        }
        Err(e) => {
            state.last_error = Some(e);
        }
    }
}

/// 判断字节序列是否为缺 CRC 的 Modbus 读请求（6 字节，func_code 为 0x01-0x04）
fn is_modbus_read_request(bytes: &[u8]) -> bool {
    if bytes.len() != 6 {
        return false;
    }
    // 读寄存器/线圈的功能码: 0x01 ReadCoils, 0x02 ReadDiscreteInputs,
    // 0x03 ReadHoldingRegisters, 0x04 ReadInputRegisters
    matches!(bytes.get(1), Some(0x01..=0x04))
}

/// 判断响应是否为 Modbus RTU 响应帧
/// 通过比对响应的 slave_id 和 func_code 与请求是否一致，并校验帧长度
fn is_modbus_response(resp: &[u8], req: &[u8]) -> bool {
    // 最小帧长度: slave(1) + func(1) + count(1) + CRC(2) = 5
    if resp.len() < 5 || req.len() < 2 {
        return false;
    }
    // slave_id 应与请求一致
    if resp[0] != req[0] {
        return false;
    }
    let req_func = req[1];
    let resp_func = resp[1];
    // 正常响应: func_code 相同; 异常响应: func_code | 0x80
    if resp_func != req_func && resp_func != (req_func | 0x80) {
        return false;
    }
    // 异常响应: [slave_id][func_code|0x80][exception_code][CRC16] = 5 字节
    if resp_func & 0x80 != 0 {
        return resp.len() == 5;
    }
    // 正常读响应: [slave_id][func_code][byte_count][data...][CRC16]
    let byte_count = resp[2] as usize;
    // 期望长度: slave(1) + func(1) + count(1) + data(byte_count) + CRC(2)
    resp.len() == 3 + byte_count + 2
}

/// 从 Modbus RTU 响应帧中提取数据区
/// 响应格式: [slave_id][func_code][byte_count][data...][CRC16 LE]
/// 返回 data 部分（跳过 slave_id/func_code/byte_count，去除尾部 CRC16）
fn extract_modbus_response_data(resp: &[u8]) -> Vec<u8> {
    if resp.len() < 5 {
        return Vec::new();
    }
    // 异常响应无数据区
    if resp[1] & 0x80 != 0 {
        return Vec::new();
    }
    let byte_count = resp[2] as usize;
    let data_end = 3 + byte_count;
    if data_end + 2 > resp.len() {
        // 长度不足，返回可用数据
        return resp[3..].to_vec();
    }
    resp[3..data_end].to_vec()
}

/// 记录一行 CSV 数据
fn record_csv(s: &mut HeatmapState) {
    if !s.recording {
        return;
    }
    let timestamp = chrono::Local::now().to_rfc3339();
    let mut row = timestamp;
    for v in &s.values {
        row.push_str(&format!(",{:.4}", v));
    }
    s.csv_data.push(row);
    s.record_count += 1;
}

/// 保存 CSV 到文件
fn save_csv(s: &HeatmapState, _zh: bool) -> Result<(), String> {
    if s.csv_data.is_empty() {
        return Err({ crate::i18n::t("No data") }.to_string());
    }
    let mut content = String::new();
    // 表头
    content.push_str("timestamp");
    for i in 0..(s.rows * s.cols) {
        content.push_str(&format!(",cell_{}", i));
    }
    content.push('\n');
    // 数据行
    for row in &s.csv_data {
        content.push_str(row);
        content.push('\n');
    }
    std::fs::write(&s.csv_path, content)
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ===== 渲染函数 =====

/// 渲染阵列配置区
fn render_config_section(ui: &mut egui::Ui, s: &mut HeatmapState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Array Config"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    egui::Grid::new("hm_config_grid")
        .num_columns(4)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("Rows"))
                    .color(theme::TEXT_SECONDARY),
            );
            if ui
                .add(egui::DragValue::new(&mut s.rows).range(1..=32).speed(0.1))
                .on_hover_text(crate::i18n::t("Array rows (1-32), determines vertical resolution"))
                .changed()
            {
                resize_values(s);
            }
            ui.label(
                RichText::new(crate::i18n::t("Cols"))
                    .color(theme::TEXT_SECONDARY),
            );
            if ui
                .add(egui::DragValue::new(&mut s.cols).range(1..=32).speed(0.1))
                .on_hover_text(crate::i18n::t("Array columns (1-32), determines horizontal resolution"))
                .changed()
            {
                resize_values(s);
            }
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Format"))
                    .color(theme::TEXT_SECONDARY),
            );
            egui::ComboBox::from_id_salt("hm_data_format")
                .selected_text(format!("{}", s.data_format))
                .width(100.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.data_format, DataFormat::U8, "U8");
                    ui.selectable_value(&mut s.data_format, DataFormat::U16BE, "U16BE");
                    ui.selectable_value(&mut s.data_format, DataFormat::U16LE, "U16LE");
                    ui.selectable_value(&mut s.data_format, DataFormat::U32BE, "U32BE");
                    ui.selectable_value(&mut s.data_format, DataFormat::U32LE, "U32LE");
                    ui.selectable_value(&mut s.data_format, DataFormat::F32BE, "F32BE");
                    ui.selectable_value(&mut s.data_format, DataFormat::F32LE, "F32LE");
                })
                .response
                .on_hover_text(crate::i18n::t("Data format: byte width and parsing per cell (U8/U16/U32/F32, big/little endian)"));
            ui.label(
                RichText::new(format!("{}x{} = {}", s.rows, s.cols, s.rows * s.cols))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.end_row();
        });
}

/// 渲染数据输入区，返回是否请求串口采集
fn render_data_input_section(
    ui: &mut egui::Ui,
    s: &mut HeatmapState,
    _zh: bool,
    connected: bool,
) -> bool {
    ui.label(
        RichText::new(crate::i18n::t("Data Input"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    let mut serial_clicked = false;

    // 串口采集
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Cmd(hex)"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut s.serial_cmd)
                .desired_width(160.0)
                .font(egui::FontId::monospace(12.0)),
        )
        .on_hover_text(crate::i18n::t("HEX Modbus read command (e.g. 01 03 00 00 00 40 44 0D)"));
        ui.label(
            RichText::new(crate::i18n::t("Header"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut s.header_hex)
                .desired_width(60.0)
                .font(egui::FontId::monospace(12.0)),
        )
        .on_hover_text(crate::i18n::t("HEX frame header used to filter response data (e.g. match the first N bytes of the response)"));
        ui.label(
            RichText::new(crate::i18n::t("Wait"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::DragValue::new(&mut s.wait_ms)
                .range(10..=5000)
                .speed(0.5),
        )
        .on_hover_text(crate::i18n::t("Time to wait for a response after sending (ms)"));
        // 串口采集按钮（未连接时禁用）
        ui.add_enabled_ui(connected, |ui| {
            let btn = egui::Button::new(
                RichText::new(crate::i18n::t("Acquire"))
                    .color(Color32::BLACK)
                    .strong(),
            )
            .fill(theme::ACCENT)
            .stroke(egui::Stroke::NONE);
            if ui.add(btn)
                .on_hover_text(crate::i18n::t("Send command and parse the response to fill the heatmap"))
                .clicked()
            {
                serial_clicked = true;
            }
        });
    });

    // 随机生成 / 清空
    ui.horizontal(|ui| {
        if ui
            .button(RichText::new(crate::i18n::t("Random")).strong())
            .on_hover_text(crate::i18n::t("Generate simulated data with Gaussian peaks for preview"))
            .clicked()
        {
            do_random_generate(s);
        }
        if ui
            .button(RichText::new(crate::i18n::t("Zero")).strong())
            .on_hover_text(crate::i18n::t("Set all cell values to zero"))
            .clicked()
        {
            s.values.fill(0.0);
        }
        // 记录一行
        if s.recording {
            if ui
                .button(RichText::new(crate::i18n::t("Record")).strong())
                .on_hover_text(crate::i18n::t("Record the current heatmap frame to CSV"))
                .clicked()
            {
                record_csv(s);
            }
        }
    });

    serial_clicked
}

/// 渲染可视化区
fn render_visualization(ui: &mut egui::Ui, s: &mut HeatmapState, _zh: bool) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Visualization"))
                .color(theme::CYAN)
                .strong(),
        );
        ui.add_space(8.0);
        ui.selectable_value(
            &mut s.view_mode,
            ViewMode::Heatmap2D,
            crate::i18n::t("2D Heatmap"),
        )
        .on_hover_text(crate::i18n::t("Flat heatmap mode (color mapping)"));
        ui.selectable_value(
            &mut s.view_mode,
            ViewMode::Surface3D,
            crate::i18n::t("3D Surface"),
        )
        .on_hover_text(crate::i18n::t("3D surface mode (height + color mapping)"));
    });
    ui.add_space(4.0);

    match s.view_mode {
        ViewMode::Heatmap2D => render_heatmap_2d(ui, s),
        ViewMode::Surface3D => render_surface_3d(ui, s),
    }
}

/// 渲染 2D 热力图（painter 单元格）
fn render_heatmap_2d(ui: &mut egui::Ui, s: &HeatmapState) {
    if s.values.is_empty() || s.rows == 0 || s.cols == 0 {
        ui.label(
            RichText::new("No data")
                .color(theme::TEXT_DISABLED)
                .small(),
        );
        return;
    }

    let avail = ui.available_size();
    let cell_w = (avail.x / s.cols as f32).min(50.0);
    let cell_h = (avail.y / s.rows as f32).min(40.0).max(20.0);
    let total_w = cell_w * s.cols as f32;
    let total_h = cell_h * s.rows as f32;

    let (resp, painter) = ui.allocate_painter(
        egui::Vec2::new(total_w, total_h),
        egui::Sense::hover(),
    );
    let rect = resp.rect;

    let vmin = s.values.iter().cloned().fold(f64::INFINITY, f64::min);
    let vmax = s.values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    for r in 0..s.rows {
        for c in 0..s.cols {
            let idx = r * s.cols + c;
            if idx >= s.values.len() {
                break;
            }
            let v = s.values[idx];
            let color = value_to_color(v, vmin, vmax);
            let x = rect.left() + c as f32 * cell_w;
            let y = rect.top() + r as f32 * cell_h;
            let cell_rect = egui::Rect::from_min_size(
                Pos2::new(x, y),
                egui::Vec2::new(cell_w - 1.0, cell_h - 1.0),
            );
            painter.rect_filled(cell_rect, 1.0, color);
            painter.rect_stroke(cell_rect, 1.0, Stroke::new(0.5_f32, theme::BORDER));

            // 数值标签（仅当单元格足够大时显示）
            if cell_w > 30.0 && cell_h > 20.0 {
                painter.text(
                    cell_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{:.0}", v),
                    egui::FontId::new(9.0, egui::FontFamily::Monospace),
                    Color32::BLACK,
                );
            }
        }
    }

    // 色标
    let bar_x = rect.right() + 8.0;
    if bar_x + 16.0 < ui.max_rect().right() {
        let bar_rect = egui::Rect::from_min_size(
            Pos2::new(bar_x, rect.top()),
            egui::Vec2::new(12.0, rect.height()),
        );
        for i in 0..bar_rect.height() as usize {
            let t = 1.0 - (i as f32 / bar_rect.height());
            let color = value_to_color(
                vmin + t as f64 * (vmax - vmin),
                vmin,
                vmax,
            );
            painter.line_segment(
                [
                    Pos2::new(bar_rect.left(), bar_rect.top() + i as f32),
                    Pos2::new(bar_rect.right(), bar_rect.top() + i as f32),
                ],
                Stroke::new(1.0_f32, color),
            );
        }
        painter.rect_stroke(bar_rect, 1.0, Stroke::new(0.5_f32, theme::BORDER));
        painter.text(
            Pos2::new(bar_rect.center().x, bar_rect.top() - 8.0),
            egui::Align2::CENTER_CENTER,
            format!("{:.0}", vmax),
            egui::FontId::new(9.0, egui::FontFamily::Monospace),
            theme::TEXT_SECONDARY,
        );
        painter.text(
            Pos2::new(bar_rect.center().x, bar_rect.bottom() + 8.0),
            egui::Align2::CENTER_CENTER,
            format!("{:.0}", vmin),
            egui::FontId::new(9.0, egui::FontFamily::Monospace),
            theme::TEXT_SECONDARY,
        );
    }
}

/// 渲染 3D 曲面图（等距投影线框）
fn render_surface_3d(ui: &mut egui::Ui, s: &HeatmapState) {
    let avail = ui.available_size();
    let (resp, painter) = ui.allocate_painter(
        avail.min([f32::MAX, 320.0].into()),
        egui::Sense::hover(),
    );
    let rect = resp.rect;
    painter.rect_filled(rect, 0.0, theme::BG_DEEP);

    if s.values.is_empty() || s.rows == 0 || s.cols == 0 {
        return;
    }

    let vmin = s.values.iter().cloned().fold(f64::INFINITY, f64::min);
    let vmax = s.values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let vrange = (vmax - vmin).max(1e-9);

    // 等距投影缩放参数
    let max_dim = s.rows.max(s.cols) as f32;
    let cell_size = (rect.width() / (max_dim * 2.5)).min(22.0).max(8.0);
    let z_scale = cell_size * 1.5;

    // 中心偏移：将网格中心投影到画布中心
    let half_cols = s.cols as f32 * 0.5;
    let half_rows = s.rows as f32 * 0.5;
    let cx = rect.center().x;
    let cy = rect.center().y - z_scale * 0.3;

    // 投影函数: x' = (gx-gy), y' = (gx+gy)/2 - z
    let project = |gx: f32, gy: f32, gz: f32| -> Pos2 {
        let dx = gx - half_cols;
        let dy = gy - half_rows;
        let px = (dx - dy) * cell_size;
        let py = (dx + dy) * 0.5 * cell_size - gz * z_scale;
        Pos2::new(cx + px, cy + py)
    };

    // 归一化高度
    let norm_z = |r: usize, c: usize| -> f32 {
        let idx = r * s.cols + c;
        if idx < s.values.len() {
            ((s.values[idx] - vmin) / vrange) as f32
        } else {
            0.0
        }
    };

    // 绘制网格线（沿行方向：固定 r，变化 c）
    for r in 0..s.rows {
        for c in 0..s.cols.saturating_sub(1) {
            let z1 = norm_z(r, c);
            let z2 = norm_z(r, c + 1);
            let p1 = project(c as f32, r as f32, z1);
            let p2 = project((c + 1) as f32, r as f32, z2);
            painter.line_segment([p1, p2], Stroke::new(0.7_f32, theme::BORDER));
        }
    }
    // 沿列方向：固定 c，变化 r
    for c in 0..s.cols {
        for r in 0..s.rows.saturating_sub(1) {
            let z1 = norm_z(r, c);
            let z2 = norm_z(r + 1, c);
            let p1 = project(c as f32, r as f32, z1);
            let p2 = project(c as f32, (r + 1) as f32, z2);
            painter.line_segment([p1, p2], Stroke::new(0.7_f32, theme::BORDER));
        }
    }

    // 绘制顶点（彩色圆点）
    for r in 0..s.rows {
        for c in 0..s.cols {
            let idx = r * s.cols + c;
            if idx >= s.values.len() {
                break;
            }
            let v = s.values[idx];
            let z = norm_z(r, c);
            let pos = project(c as f32, r as f32, z);
            let color = value_to_color(v, vmin, vmax);
            painter.circle_filled(pos, 2.0, color);
        }
    }
}

/// 渲染可编辑数据网格
fn render_editable_grid(ui: &mut egui::Ui, s: &mut HeatmapState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Data Editor"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(2.0);

    // 大阵列时用 ScrollArea
    let need_scroll = s.rows > 12 || s.cols > 12;

    let mut grid_content = |ui: &mut egui::Ui| {
        egui::Grid::new("hm_edit_grid")
            .spacing([2.0, 2.0])
            .show(ui, |ui| {
                for r in 0..s.rows {
                    for c in 0..s.cols {
                        let idx = r * s.cols + c;
                        if idx < s.values.len() {
                            ui.add_sized(
                                egui::Vec2::new(48.0, 0.0),
                                egui::DragValue::new(&mut s.values[idx])
                                    .range(0.0..=10000.0)
                                    .speed(0.1)
                                    .suffix(""),
                            );
                        }
                    }
                    ui.end_row();
                }
            });
    };

    if need_scroll {
        ScrollArea::both()
            .max_height(200.0)
            .auto_shrink([false, true])
            .show(ui, grid_content);
    } else {
        grid_content(ui);
    }
}

/// 渲染统计信息
fn render_statistics(ui: &mut egui::Ui, s: &HeatmapState, zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Statistics"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(2.0);

    let stats = compute_stats(s);

    egui::Grid::new("hm_stats_grid")
        .num_columns(4)
        .spacing([12.0, 3.0])
        .striped(true)
        .show(ui, |ui| {
            stat_row(
                ui,
                zh,
                crate::i18n::t("Total##panels/sensor_dev/heatmap.rs:27047"),
                format!("{:.2}", stats.total),
            );
            stat_row(
                ui,
                zh,
                crate::i18n::t("Max"),
                format!("{:.2}", stats.max),
            );
            stat_row(
                ui,
                zh,
                crate::i18n::t("Min"),
                format!("{:.2}", stats.min),
            );
            stat_row(
                ui,
                zh,
                crate::i18n::t("Avg"),
                format!("{:.2}", stats.avg),
            );
            stat_row(
                ui,
                zh,
                crate::i18n::t("COP X"),
                format!("{:.2}", stats.cop_x),
            );
            stat_row(
                ui,
                zh,
                crate::i18n::t("COP Y"),
                format!("{:.2}", stats.cop_y),
            );
            stat_row(
                ui,
                zh,
                crate::i18n::t("Contact"),
                format!("{}", stats.contact_count),
            );
            stat_row(
                ui,
                zh,
                crate::i18n::t("Ratio"),
                format!("{:.1}%", stats.contact_ratio * 100.0),
            );
        });
}

/// 统计表格行
fn stat_row(ui: &mut egui::Ui, _zh: bool, label: &str, value: String) {
    ui.label(
        RichText::new(label)
            .color(theme::TEXT_SECONDARY)
            .small(),
    );
    ui.label(
        RichText::new(value)
            .color(theme::TEXT_PRIMARY)
            .small()
            .family(egui::FontFamily::Monospace),
    );
    ui.end_row();
}

/// 渲染 AI 压力模式识别区
fn render_ai_pressure(ui: &mut egui::Ui, s: &HeatmapState, zh: bool) {
    // 标题
    ui.horizontal(|ui| {
        ui.label(RichText::new("\u{1F916}").size(16.0));
        ui.label(
            RichText::new(crate::i18n::t("AI Pressure Pattern"))
                .color(theme::CYAN)
                .strong(),
        );
    });
    ui.add_space(4.0);

    // 执行分析
    let analysis = miconlab_ai::PressureAnalysis::analyze(&s.values, s.rows, s.cols);

    // 模式颜色编码
    let pattern_color = match analysis.pattern {
        miconlab_ai::PressurePattern::NoContact => theme::TEXT_DISABLED,
        miconlab_ai::PressurePattern::SinglePoint => theme::CYAN,
        miconlab_ai::PressurePattern::MultiPoint => theme::WARNING,
        miconlab_ai::PressurePattern::SurfaceContact => theme::ACCENT,
        miconlab_ai::PressurePattern::Grasp => theme::SUCCESS,
        miconlab_ai::PressurePattern::Sliding => theme::WARNING,
        miconlab_ai::PressurePattern::Uniform => theme::SUCCESS,
    };

    // 结果卡片
    egui::Frame::group(&ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            egui::Grid::new("hm_ai_grid")
                .num_columns(4)
                .spacing([12.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    // 压力模式 + 置信度
                    stat_row(
                        ui,
                        zh,
                        crate::i18n::t("Pattern"),
                        format!(
                            "{} ({:.0}%)",
                            crate::i18n::t_owned(analysis.pattern.label_zh()),
                            analysis.confidence * 100.0
                        ),
                    );
                    // 压力中心 CoP
                    stat_row(
                        ui,
                        zh,
                        crate::i18n::t("CoP (x,y)"),
                        format!("({:.2}, {:.2})", analysis.cop.0, analysis.cop.1),
                    );
                    // 总压力
                    stat_row(
                        ui,
                        zh,
                        crate::i18n::t("Total Force"),
                        format!("{:.2}", analysis.total_force),
                    );
                    // 接触面积比例
                    stat_row(
                        ui,
                        zh,
                        crate::i18n::t("Contact Ratio"),
                        format!("{:.1}%", analysis.contact_ratio * 100.0),
                    );
                    // 有效通道数
                    stat_row(
                        ui,
                        zh,
                        crate::i18n::t("Active Ch"),
                        format!("{}", analysis.active_channels),
                    );
                });

            ui.add_space(2.0);
            // 模式彩色徽标 + 摘要
            ui.horizontal(|ui| {
                ui.label(RichText::new("\u{25CF}").color(pattern_color).size(14.0));
                ui.label(
                    RichText::new(crate::i18n::t_owned(analysis.pattern.label_zh()))
                        .color(pattern_color)
                        .strong()
                        .small(),
                );
                ui.label(
                    RichText::new(format!(
                        "({:.0}%)",
                        analysis.confidence * 100.0
                    ))
                    .color(theme::TEXT_DISABLED)
                    .small(),
                );
                ui.separator();
                ui.label(
                    RichText::new(&analysis.summary_zh)
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
            });
        });
}

/// 渲染 CSV 记录区
fn render_recording_section(ui: &mut egui::Ui, s: &mut HeatmapState, zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("CSV Recording"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(2.0);

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("File"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut s.csv_path)
                .desired_width(180.0)
                .font(egui::FontId::monospace(12.0)),
        )
        .on_hover_text(crate::i18n::t("CSV output file path"));

        // 开始/停止记录
        let (btn_text, btn_color) = if s.recording {
            (
                { crate::i18n::t("Stop") }.to_string(),
                theme::ERROR,
            )
        } else {
            (
                { crate::i18n::t("Start") }.to_string(),
                theme::SUCCESS,
            )
        };
        if ui
            .add(
                egui::Button::new(
                    RichText::new(btn_text).color(Color32::BLACK).strong(),
                )
                .fill(btn_color)
                .stroke(egui::Stroke::NONE),
            )
            .on_hover_text(crate::i18n::t("Start auto-record mode (append on every capture)"))
            .clicked()
        {
            s.recording = !s.recording;
        }

        // 记录一帧
        if s.recording {
            if ui
                .button(RichText::new(crate::i18n::t("Record")).strong())
                .clicked()
            {
                record_csv(s);
            }
        }

        // 保存
        if ui
            .button(RichText::new(crate::i18n::t("Save")).strong())
            .on_hover_text(crate::i18n::t("Save CSV file to disk"))
            .clicked()
        {
            match save_csv(s, zh) {
                Ok(()) => {
                    s.csv_data.clear();
                }
                Err(e) => {
                    // 错误无法直接设置到 state（在闭包内），
                    // 暂时用 eprintln 输出
                    eprintln!("[heatmap] CSV save error: {}", e);
                }
            }
        }

        // 清空缓存
        if ui
            .button(RichText::new(crate::i18n::t("Clear")).small())
            .on_hover_text(crate::i18n::t("Clear recorded CSV data (does not affect the file)"))
            .clicked()
        {
            s.csv_data.clear();
            s.record_count = 0;
        }
    });

    // 记录状态
    ui.horizontal(|ui| {
        let (dot, dot_color) = if s.recording {
            ("\u{25CF}", theme::ERROR)
        } else {
            ("\u{25CB}", theme::TEXT_DISABLED)
        };
        ui.label(
            RichText::new(format!(
                "{} {} {} {}",
                dot,
                if s.recording {
                    crate::i18n::t("Recording")
                } else {
                    crate::i18n::t("Stopped")
                },
                s.record_count,
                crate::i18n::t("rows")
            ))
            .color(dot_color)
            .small(),
        );
    });
}
