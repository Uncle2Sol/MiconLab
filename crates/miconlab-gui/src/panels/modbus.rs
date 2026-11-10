// Modbus RTU 主站面板
// 提供从站 ID、功能码、地址、数量/值配置，构造请求帧并发送，
// 解析响应（HEX + 寄存器/线圈解码），检测并显示异常响应。

use crate::i18n::{Language, L10n};
use crate::state::AppState;
use crate::theme;
use egui::{Id, RichText};
use miconlab_core::protocol::modbus_common::{
    build_write_multiple_coils_frame, build_write_multiple_registers_frame,
    build_write_single_coil_frame, parse_read_bits_response, parse_read_registers_response,
};
use miconlab_core::protocol::{ModbusFrame, ModbusFunction, ModbusParser};
use std::time::{Duration, Instant};

/// 持久化的 Modbus 面板状态。
/// egui 0.29 的 temp storage 要求 `Clone + Send + Sync + Default`，故派生 Clone。
#[derive(Clone)]
struct ModbusState {
    slave_id: u8,
    function_idx: usize,
    address: u16,
    quantity: u16,
    /// 多寄存器/多线圈写入值（逗号分隔）
    multi_values: String,
    /// 响应显示文本
    response: String,
}

impl Default for ModbusState {
    fn default() -> Self {
        Self {
            slave_id: 1,
            function_idx: 2, // 默认 Read Holding Registers
            address: 0,
            quantity: 1,
            multi_values: String::from("0, 0, 0"),
            response: String::new(),
        }
    }
}

/// 下拉框中展示的 8 个标准功能码（顺序固定）
const FUNCTIONS: &[ModbusFunction] = &[
    ModbusFunction::ReadCoils,
    ModbusFunction::ReadDiscreteInputs,
    ModbusFunction::ReadHoldingRegisters,
    ModbusFunction::ReadInputRegisters,
    ModbusFunction::WriteSingleCoil,
    ModbusFunction::WriteSingleRegister,
    ModbusFunction::WriteMultipleCoils,
    ModbusFunction::WriteMultipleRegisters,
];

/// 功能码的本地化标签
fn function_label(f: ModbusFunction) -> String {
    let code = f.to_code();
    format!("{:02X} {}", code, crate::i18n::t_owned(f.name()))
}

/// 异常码可读描述
fn exception_label(code: u8) -> &'static str {
    match code {
        0x01 => crate::i18n::t("Illegal Function"),
        0x02 => crate::i18n::t("Illegal Data Address"),
        0x03 => crate::i18n::t("Illegal Data Value"),
        0x04 => crate::i18n::t("Slave Device Failure"),
        0x05 => crate::i18n::t("Acknowledge"),
        0x06 => crate::i18n::t("Slave Device Busy"),
        0x08 => crate::i18n::t("Memory Parity Error"),
        0x0A => crate::i18n::t("Gateway Path Unavailable"),
        0x0B => crate::i18n::t("Gateway Target No Response"),
        _ => crate::i18n::t("Unknown exception code"),
    }
}

/// 解析逗号分隔的 u16 列表（支持十进制与 0x 前缀十六进制）
fn parse_u16_list(s: &str) -> Vec<u16> {
    s.split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            if let Some(hex) = p.strip_prefix("0x").or_else(|| p.strip_prefix("0X")) {
                u16::from_str_radix(hex, 16).ok()
            } else {
                p.parse::<u16>().ok()
            }
        })
        .collect()
}

/// 解析逗号分隔的线圈值列表（1/0、true/false、on/off）
fn parse_bool_list(s: &str) -> Vec<bool> {
    s.split(',')
        .map(|p| p.trim().to_ascii_lowercase())
        .filter(|p| !p.is_empty())
        .filter_map(|p| match p.as_str() {
            "1" | "true" | "on" | "yes" => Some(true),
            "0" | "false" | "off" | "no" => Some(false),
            _ => None,
        })
        .collect()
}

/// 字节序列格式化为空格分隔的大写 HEX 字符串
fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 解析响应 PDU 中的寄存器/线圈值，返回附加的可读解码文本
fn decode_response_values(
    frame: &ModbusFrame,
    request_func: ModbusFunction,
    quantity: u16,
) -> String {
    // ModbusFrame.data 不含功能码字节，重建 PDU = [功能码, 字节数, 数据...]
    let mut pdu = vec![frame.function.to_code()];
    pdu.extend_from_slice(&frame.data);

    match request_func {
        ModbusFunction::ReadHoldingRegisters | ModbusFunction::ReadInputRegisters => {
            if let Ok(regs) = parse_read_registers_response(&pdu) {
                let detail: Vec<String> = regs
                    .iter()
                    .enumerate()
                    .map(|(i, v)| format!("[{}]={:04X}({})", i, v, v))
                    .collect();
                return crate::i18n::tf(
                    "modbus.response.register_values",
                    &[("{values}", "{values}", &detail.join(", "))],
                );
            }
        }
        ModbusFunction::ReadCoils | ModbusFunction::ReadDiscreteInputs => {
            if let Ok(bits) = parse_read_bits_response(&pdu, quantity as usize) {
                let detail: Vec<String> = bits
                    .iter()
                    .enumerate()
                    .map(|(i, b)| format!("[{}]={}", i, if *b { 1 } else { 0 }))
                    .collect();
                return crate::i18n::tf(
                    "modbus.response.coil_values",
                    &[("{values}", "{values}", &detail.join(", "))],
                );
            }
        }
        _ => {}
    }
    String::new()
}

/// 根据已收到的响应前缀判断完整 Modbus RTU 帧的期望长度。
/// 返回 `None` 表示当前数据尚不足以判断（需继续读取）。
///
/// - 异常响应 `[slave][func|0x80][exc_code][CRC16]` = 5 字节
/// - 读响应 `[slave][func][byte_count][data...][CRC16]`：需第 3 字节确定长度
/// - 写响应（FC 5/6/15/16）固定 8 字节
fn modbus_expected_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 2 {
        return None;
    }
    let func = buf[1];
    if func & 0x80 != 0 {
        // 异常响应：[slave_id][func|0x80][exception_code][CRC16] = 5 字节
        return Some(5);
    }
    match func {
        1 | 2 | 3 | 4 => {
            // 读响应：[slave_id][func][byte_count][data...][CRC16]
            // 需要第 3 字节（byte_count）才能确定完整长度
            if buf.len() >= 3 {
                let byte_count = buf[2] as usize;
                Some(3 + byte_count + 2)
            } else {
                None
            }
        }
        5 | 6 | 15 | 16 => {
            // 写响应固定 8 字节：[slave_id][func][addr(2)][value/qty(2)][CRC16]
            Some(8)
        }
        _ => None,
    }
}

/// 循环读取串口直到收到完整 Modbus RTU 响应帧或总超时。
///
/// `read_response()` 仅做一次 `port.read()`，可能只返回部分帧；本函数根据
/// 功能码判断期望帧长，循环读取并拼接，直到收齐完整帧或超时。
fn read_modbus_response(state: &mut AppState, timeout_ms: u64) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    // 每次轮询的短超时；循环读取可拼接跨包到达的帧片段
    let poll_ms = timeout_ms.min(50).max(20);
    let mut buf: Vec<u8> = Vec::new();

    loop {
        // 尝试读取更多数据（无数据时 read_response 返回 Err，属正常）
        if let Ok(chunk) = state.read_response(poll_ms) {
            buf.extend_from_slice(&chunk);
        }

        // 判断当前缓冲是否已构成完整帧
        if let Some(expected) = modbus_expected_len(&buf) {
            if buf.len() >= expected {
                buf.truncate(expected);
                return Ok(buf);
            }
        }

        if Instant::now() >= deadline {
            break;
        }
    }

    if buf.is_empty() {
        Err(if state.is_connected() {
            "No response (timeout)".to_string()
        } else {
            "Not connected".to_string()
        })
    } else {
        // 超时但已收到部分数据：返回供调用方尝试解析（通常解析失败会显示 HEX）
        Ok(buf)
    }
}

/// 执行发送：构造帧、发送+读取、解析响应并写入 st.response
fn do_send(st: &mut ModbusState, state: &mut AppState) {
    let function = FUNCTIONS[st.function_idx.min(FUNCTIONS.len() - 1)].clone();
    let slave_id = st.slave_id;
    let address = st.address;
    let quantity = st.quantity;

    let frame_result: Result<ModbusFrame, String> = match function {
        ModbusFunction::ReadCoils
        | ModbusFunction::ReadDiscreteInputs
        | ModbusFunction::ReadHoldingRegisters
        | ModbusFunction::ReadInputRegisters => {
            Ok(ModbusParser::build_read_request(slave_id, function.clone(), address, quantity))
        }
        ModbusFunction::WriteSingleRegister => {
            // quantity 字段作为写入值
            Ok(ModbusParser::build_write_single(slave_id, address, quantity))
        }
        ModbusFunction::WriteSingleCoil => {
            // quantity != 0 视为 ON
            Ok(build_write_single_coil_frame(slave_id, address, quantity != 0))
        }
        ModbusFunction::WriteMultipleRegisters => {
            let values = parse_u16_list(&st.multi_values);
            if values.is_empty() {
                Err(crate::i18n::t("multi-register values parse failed or empty").to_string())
            } else {
                build_write_multiple_registers_frame(slave_id, address, &values)
                    .map_err(|e| e.to_string())
            }
        }
        ModbusFunction::WriteMultipleCoils => {
            let coils = parse_bool_list(&st.multi_values);
            if coils.is_empty() {
                Err(crate::i18n::t("multi-coil values parse failed or empty").to_string())
            } else {
                build_write_multiple_coils_frame(slave_id, address, &coils).map_err(|e| e.to_string())
            }
        }
        ModbusFunction::Other(_) => Err(
            crate::i18n::t("unsupported function code").to_string(),
        ),
    };

    let frame = match frame_result {
        Ok(f) => f,
        Err(e) => {
            st.response = format!(
                "{}: {}",
                crate::i18n::t("Error"),
                e
            );
            return;
        }
    };

    let bytes = frame.to_bytes();
    let req_hex = bytes_to_hex(&bytes);
    let req_prefix = crate::i18n::t("Request");

    let err_label = crate::i18n::t("Error");

    // 先发送请求帧
    if let Err(e) = state.send_raw(&bytes) {
        st.response = format!("{}: {}\n{}: {}", req_prefix, req_hex, err_label, e);
        return;
    }

    // 循环读取直到收到完整 Modbus RTU 响应帧或超时。
    // 单次 read_response() 只做一次 port.read()，可能只读到部分帧。
    match read_modbus_response(state, 300) {
        Ok(resp) if resp.is_empty() => {
            st.response = format!("{}: {}\n<{}>", req_prefix, req_hex,
                crate::i18n::t("no response"));
        }
        Ok(resp) => match ModbusFrame::parse(&resp) {
            Ok(parsed) => {
                let resp_prefix = crate::i18n::t("Response");
                if parsed.is_exception() {
                    let code = parsed.exception_code().unwrap_or(0);
                    let exc = crate::i18n::t("Exception");
                    st.response = format!(
                        "{}: {}\n{}: code={} ({})",
                        req_prefix, req_hex, exc, code, exception_label(code)
                    );
                } else {
                    let mut text = format!(
                        "{}: {}\n{}: {}",
                        req_prefix,
                        req_hex,
                        resp_prefix,
                        ModbusParser::format_frame(&parsed)
                    );
                    text.push_str(&decode_response_values(&parsed, function, quantity));
                    st.response = text;
                }
            }
            Err(e) => {
                let resp_hex = bytes_to_hex(&resp);
                let fail = crate::i18n::t("parse failed");
                st.response = format!(
                    "{}: {}\n{}: {}\nHEX: {}",
                    req_prefix, req_hex, fail, e, resp_hex
                );
            }
        },
        Err(e) => {
            let err = crate::i18n::t("Error");
            st.response = format!("{}: {}\n{}: {}", req_prefix, req_hex, err, e);
        }
    }
}

/// 渲染 Modbus 面板
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let l10n = L10n::new(state.language);
    let zh = state.language == Language::Zh;

    // 注意：不能在 CentralPanel::show 闭包内调用 ctx.data_mut()，
    // 因为 CentralPanel::show 持有 ctx 写锁，data_mut 重入会死锁。
    // 解决：先取出持久化状态（clone），闭包内使用，最后写回。
    let mut st: ModbusState =
        ctx.data_mut(|data| data.get_temp_mut_or_default::<ModbusState>(Id::new("modbus_state_v2")).clone());

    egui::CentralPanel::default()
        .frame(
            egui::Frame::central_panel(&ctx.style())
                .inner_margin(egui::Margin::same(16.0)),
        )
        .show(ctx, |ui| {
            ui.heading(
                RichText::new(l10n.t("panel.modbus"))
                    .color(theme::TEXT_PRIMARY)
                    .strong(),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(crate::i18n::t("Modbus RTU/TCP master debugging - read holding/input registers, read/write coils, etc."))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );

            // AI 感知：错误诊断（连接失败或操作异常时自动推断根因）
            if let Some(err) = &state.last_error {
                if let Some(hint) = crate::panels::ai_sense::sense_error(err, state.is_connected(), state.config.baud_rate, zh) {
                    crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);
                    ui.add_space(4.0);
                }
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // 未连接提示
            if !state.is_connected() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("\u{26A0}").color(theme::WARNING));
                    ui.label(
                        RichText::new(crate::i18n::t("Not connected. Open a port in the Connection panel first."))
                        .color(theme::WARNING),
                    );
                });
                ui.add_space(8.0);
            }

            // ---- 参数配置网格 ----
            egui::Grid::new("modbus_grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    // 从站 ID
                    ui.label(
                        RichText::new(crate::i18n::t("Slave ID"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::DragValue::new(&mut st.slave_id)
                            .range(1..=247u8)
                            .speed(0.1),
                    )
                    .on_hover_text(crate::i18n::t("Slave address (1-247), 0 is broadcast"));
                    ui.end_row();

                    // 功能码
                    ui.label(
                        RichText::new(crate::i18n::t("Function"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    let current = function_label(FUNCTIONS[st.function_idx].clone());
                    egui::ComboBox::from_id_salt("modbus_func_select")
                        .selected_text(current)
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for (i, f) in FUNCTIONS.iter().enumerate() {
                                ui.selectable_value(
                                    &mut st.function_idx,
                                    i,
                                    function_label(f.clone()),
                                );
                            }
                        })
                        .response
                        .on_hover_text(crate::i18n::t("Function codes: 0x01 Read Coils / 0x02 Read Discrete Inputs / 0x03 Read Holding Registers / 0x04 Read Input Registers / 0x05 Write Single Coil / 0x06 Write Single Register / 0x0F Write Multiple Coils / 0x10 Write Multiple Registers"));
                    ui.end_row();

                    // 起始地址
                    ui.label(
                        RichText::new(crate::i18n::t("Start Address"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::DragValue::new(&mut st.address)
                            .range(0..=65535u16)
                            .speed(0.1)
                            .hexadecimal(4, false, true),
                    )
                    .on_hover_text(crate::i18n::t("Register/coil start address (0x0000-0xFFFF), displayed in hex"));
                    ui.end_row();

                    // 数量/值
                    let q_label = match FUNCTIONS[st.function_idx] {
                        ModbusFunction::WriteSingleCoil => {
                            crate::i18n::t("Coil Value (0/1)")
                        }
                        ModbusFunction::WriteSingleRegister => {
                            crate::i18n::t("Write Value")
                        }
                        _ => crate::i18n::t("Quantity"),
                    };
                    ui.label(RichText::new(q_label).color(theme::TEXT_SECONDARY));
                    ui.add(
                        egui::DragValue::new(&mut st.quantity)
                            .range(0..=65535u16)
                            .speed(0.1),
                    )
                    .on_hover_text(crate::i18n::t("Read count or write value: count for read ops, 0=OFF/non-zero=ON for single coil, value for single register"));
                    ui.end_row();
                });

            // ---- 多寄存器/多线圈写入值 ----
            let needs_multi = matches!(
                FUNCTIONS[st.function_idx],
                ModbusFunction::WriteMultipleRegisters
                    | ModbusFunction::WriteMultipleCoils
            );
            if needs_multi {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(
                            match FUNCTIONS[st.function_idx] {
                                ModbusFunction::WriteMultipleRegisters =>
                                    crate::i18n::t("Registers (comma-separated, 0x ok):"),
                                _ => crate::i18n::t("Coils (comma-separated 1/0):"),
                            }
                        )
                        .color(theme::TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut st.multi_values)
                            .desired_width(320.0)
                            .code_editor(),
                    )
                    .on_hover_text(crate::i18n::t("Separate values with spaces or commas; HEX values need the 0x prefix (e.g. 0x01 0x02 or 1,2,3)"));
                });
            }

            ui.add_space(12.0);

            // ---- 发送请求按钮 ----
            ui.horizontal(|ui| {
                let connected = state.is_connected();
                let btn = egui::Button::new(
                    RichText::new(crate::i18n::t("Send Request"))
                        .color(egui::Color32::BLACK)
                        .strong(),
                )
                .fill(theme::ACCENT)
                .min_size(egui::Vec2::new(130.0, 30.0))
                .stroke(egui::Stroke::NONE);

                let clicked = ui.add_enabled(connected, btn)
                    .on_hover_text(crate::i18n::t("Build and send a complete Modbus frame (with CRC16) for the selected function code"))
                    .clicked();
                if clicked {
                    do_send(&mut st, state);
                }

                // 清空响应
                if ui
                    .button(RichText::new(crate::i18n::t("Clear")).color(theme::TEXT_PRIMARY))
                    .on_hover_text(crate::i18n::t("Clear log and response data"))
                    .clicked()
                {
                    st.response.clear();
                }
            });

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);

            // ---- 响应显示区 ----
            ui.label(
                RichText::new(crate::i18n::t("Response"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add_space(4.0);

            egui::Frame::group(ui.style())
                .fill(theme::BG_DEEP)
                .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                .inner_margin(egui::Margin::same(8.0))
                .show(ui, |ui| {
                    ui.set_min_height(160.0);
                    if st.response.is_empty() {
                        ui.label(
                            RichText::new(crate::i18n::t("Waiting for request..."))
                            .color(theme::TEXT_DISABLED)
                            .family(egui::FontFamily::Monospace),
                        );
                    } else {
                        egui::ScrollArea::vertical()
                            .max_height(240.0)
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(st.response.as_str())
                                        .color(theme::CYAN)
                                        .family(egui::FontFamily::Monospace),
                                );
                            });
                    }
                });

            // ---- 底部错误信息（来自全局 state.last_error）----
            if let Some(err) = &state.last_error {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("\u{26A0}").color(theme::WARNING));
                    ui.label(RichText::new(err).color(theme::ERROR));
                });
            }
        });

    // 写回持久化状态
    ctx.data_mut(|data| {
        *data.get_temp_mut_or_default::<ModbusState>(Id::new("modbus_state_v2")) = st;
    });
}
