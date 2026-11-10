//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;
// =====================================================================
// PIC ICSP 单步操作实现（仅命令构造展示，需 PicKit 等硬件）
// =====================================================================

pub(crate) fn do_pic_load_config(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let pic = PicIcspFlasher::new();
    // 加载配置寄存器（地址 0x2000）
    let cmd = pic.load_config(0x3FFF);
    log_push(fs, format!(">> PIC Load Config: {}", format_hex_spaced(&cmd)));
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    log_push(fs, "<< Command sent (need PicKit hardware to actually execute)");
    state.last_error = Some("PIC ICSP requires PicKit/ICD hardware".to_string());
}

pub(crate) fn do_pic_bulk_erase(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let pic = PicIcspFlasher::new();
    let cmd = pic.bulk_erase_pm();
    log_push(fs, format!(">> PIC Bulk Erase PM: {}", format_hex_spaced(&cmd)));
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    log_push(fs, "<< Command sent (need PicKit hardware)");
    state.last_error = Some("PIC ICSP requires PicKit/ICD hardware".to_string());
}

pub(crate) fn do_pic_read_page(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    fs.read_chip = ChipType::Pic;
    let pic = PicIcspFlasher::new();
    let cmd = pic.read_data_pm();
    log_push(fs, format!(">> PIC Read Data PM: {}", format_hex_spaced(&cmd)));
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            match PicIcspFlasher::parse_14bit_response(&resp) {
                Ok(data) => {
                    fs.last_read_data.extend_from_slice(&data.to_le_bytes());
                    log_push(fs, format!("<< Read 14-bit: 0x{:04X}", data));
                }
                Err(e) => {
                    log_push(fs, format!("<< Parse error: {}", e));
                }
            }
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

pub(crate) fn do_pic_write_page(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let pic = PicIcspFlasher::new();
    let data = 0x3FFFu16; // 占位：擦除态
    let cmd = pic.load_data_pm(data);
    log_push(fs, format!(">> PIC Load Data PM (0x{:04X}): {}", data, format_hex_spaced(&cmd)));
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    let begin = pic.begin_prog();
    let _ = state.send_raw(&begin);
    let _ = state.read_response(500);
    let end = pic.end_prog();
    let _ = state.send_raw(&end);
    log_push(fs, "<< Command sent (need PicKit hardware)");
    state.last_error = Some("PIC ICSP requires PicKit/ICD hardware".to_string());
}

// =====================================================================

