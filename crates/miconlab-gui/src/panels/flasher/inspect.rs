//! flasher 子模块：固件解读路由分发。
//!
//! 本文件仅负责按固件魔数/特征将解读请求路由到对应的芯片解读模块：
//!   - 0xE9 魔数  → inspect_esp32::inspect_esp32_firmware (Xtensa)
//!   - ARM 向量表 → inspect_arm::inspect_arm_firmware (Cortex-M Thumb)
//! 不包含任何具体的解读逻辑，便于扩展新架构（如 RISC-V / AVR）时只需新增
//! inspect_<arch>.rs 并在此处添加路由分支。

use super::*;

/// 固件解读入口：按魔数/特征路由到对应架构的解读函数
pub(crate) fn inspect_read_data(fs: &mut FlasherState, is_zh: bool) {
    let data: Vec<u8> = fs.last_read_data.clone();
    let base_addr = fs.address;
    let zh = is_zh;

    if data.len() < 8 {
        log_push(fs, crate::i18n::t("<< Data < 8 bytes, cannot verify vector table"));
        return;
    }

    let size = data.len();
    log_push(fs, format!("=== {} ({} bytes / {} KB) ===",
        crate::i18n::t("Firmware Analysis"),
        size, size / 1024));

    // 检测全 0xFF（已擦除）或全 0x00（空白）
    let all_ff = data.iter().all(|&b| b == 0xFF);
    let all_zero = data.iter().all(|&b| b == 0x00);
    if all_ff {
        log_push(fs, crate::i18n::t("⚠ Flash is all 0xFF — erased or unprogrammed"));
        return;
    }
    if all_zero {
        log_push(fs, crate::i18n::t("⚠ Flash is all 0x00 — wrong address?"));
        return;
    }

    // === ESP32/Xtensa 固件解读（0xE9 魔数）===
    // ESP32 固件以 0xE9 开头，无 ARM 向量表，走独立解读路径
    // 必须在 STM32 向量表校验前拦截，否则 ESP32 数据会被误判为"非有效固件"
    if data.len() >= 24 && data[0] == 0xE9 {
        inspect_esp32_firmware(fs, &data, base_addr, zh);
        return;
    }

    // === ARM/Cortex-M 固件解读（向量表特征）===
    // 委托给 inspect_arm 模块处理：向量表校验、SP/Reset 分析、Thumb 反汇编、
    // 字符串提取、HEX dump、深度函数分析（调用图）等
    inspect_arm_firmware(fs, &data, base_addr);
}
