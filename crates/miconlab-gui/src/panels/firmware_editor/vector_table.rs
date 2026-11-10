// 向量表解析（ARM Cortex-M）

use super::memory::FirmwareMemory;

/// 向量表条目
pub struct VectorTableEntry {
    pub index: usize,
    pub name: &'static str,
    pub name_zh: &'static str,
    pub address: u32,
    pub value: u32,
    pub description: String,
}

/// 获取 ARM Cortex-M 向量表条目名称
pub fn arm_vector_name(index: usize) -> &'static str {
    match index {
        0 => "Initial SP",
        1 => "Reset Handler",
        2 => "NMI",
        3 => "HardFault",
        4 => "MemManage",
        5 => "BusFault",
        6 => "UsageFault",
        7 => "SecureFault",
        8 => "Reserved",
        9 => "Reserved",
        10 => "Reserved",
        11 => "SVC",
        12 => "DebugMon",
        13 => "Reserved",
        14 => "PendSV",
        15 => "SysTick",
        16 => "IRQ0 (WWDG)",
        17 => "IRQ1 (PVD)",
        18 => "IRQ2 (TAMPER)",
        19 => "IRQ3 (RTC)",
        20 => "IRQ4 (FLASH)",
        21 => "IRQ5 (RCC)",
        22 => "IRQ6 (EXTI0)",
        23 => "IRQ7 (EXTI1)",
        24 => "IRQ8 (EXTI2)",
        25 => "IRQ9 (EXTI3)",
        26 => "IRQ10 (EXTI4)",
        27 => "IRQ11 (DMA1_Ch1)",
        28 => "IRQ12 (DMA1_Ch2)",
        29 => "IRQ13 (DMA1_Ch3)",
        30 => "IRQ14 (DMA1_Ch4)",
        31 => "IRQ15 (DMA1_Ch5)",
        32 => "IRQ16 (DMA1_Ch6)",
        33 => "IRQ17 (DMA1_Ch7)",
        34 => "IRQ18 (ADC1_2)",
        35 => "IRQ19 (USB_HP_CAN_TX)",
        36 => "IRQ20 (USB_LP_CAN_RX0)",
        37 => "IRQ21 (CAN_RX1)",
        38 => "IRQ22 (CAN_SCE)",
        39 => "IRQ23 (EXTI9_5)",
        40 => "IRQ24 (TIM1_BRK)",
        41 => "IRQ25 (TIM1_UP)",
        42 => "IRQ26 (TIM1_TRG_COM)",
        43 => "IRQ27 (TIM1_CC)",
        44 => "IRQ28 (TIM2)",
        45 => "IRQ29 (TIM3)",
        46 => "IRQ30 (TIM4)",
        47 => "IRQ31 (I2C1_EV)",
        48 => "IRQ32 (I2C1_ER)",
        49 => "IRQ33 (I2C2_EV)",
        50 => "IRQ34 (I2C2_ER)",
        51 => "IRQ35 (SPI1)",
        52 => "IRQ36 (SPI2)",
        53 => "IRQ37 (USART1)",
        54 => "IRQ38 (USART2)",
        55 => "IRQ39 (USART3)",
        56 => "IRQ40 (EXTI15_10)",
        57 => "IRQ41 (RTCAlarm)",
        58 => "IRQ42 (USBWakeUp)",
        _ => "IRQ?",
    }
}

/// 获取 ARM Cortex-M 向量表中文描述
pub fn arm_vector_name_zh(index: usize) -> &'static str {
    match index {
        0 => crate::i18n::t("Initial Stack Pointer"),
        1 => crate::i18n::t("Reset Vector"),
        2 => crate::i18n::t("Non-Maskable Interrupt"),
        3 => crate::i18n::t("HardFault"),
        4 => crate::i18n::t("Memory Management Fault"),
        5 => crate::i18n::t("Bus Fault"),
        6 => crate::i18n::t("Usage Fault"),
        7 => crate::i18n::t("Secure Fault"),
        11 => crate::i18n::t("Supervisor Call"),
        12 => crate::i18n::t("Debug Monitor"),
        14 => "PendSV",
        15 => crate::i18n::t("SysTick system timer"),
        16 => crate::i18n::t("WWDG window watchdog"),
        22 => crate::i18n::t("EXTI0 external interrupt 0"),
        27 => crate::i18n::t("DMA1 channel 1"),
        37 => "USART1",
        38 => "USART2",
        39 => "USART3",
        44 => crate::i18n::t("TIM2 timer 2"),
        _ => "",
    }
}

/// 解析向量表（ARM Cortex-M）
pub fn parse_arm_vector_table(
    mem: &FirmwareMemory,
    base: u32,
    count: usize,
) -> Vec<VectorTableEntry> {
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let addr = base + (i * 4) as u32;
        if let Some(val) = mem.read_u32_le(addr) {
            let name = arm_vector_name(i);
            let name_zh = arm_vector_name_zh(i);
            let desc = if i == 0 {
                // SP 验证
                if val >= 0x2000_0000 && val < 0x2008_0000 {
                    let ram_kb = (val - 0x2000_0000) / 1024;
                    crate::i18n::tf(
                        "→ {addr} (RAM region, inferred RAM ≥ {kb}KB)",
                        &[
                            ("{addr}", "{addr}", &format!("0x{:08X}", val)),
                            ("{kb}", "{kb}", &ram_kb.to_string()),
                        ],
                    )
                } else {
                    crate::i18n::tf("→ {addr} (atypical RAM address)", &[("{addr}", "{addr}", &format!("0x{:08X}", val))])
                }
            } else if i == 1 {
                // Reset Handler
                let thumb = val & 1 != 0;
                let target = val & 0xFFFF_FFFE;
                if target >= 0x0800_0000 && target < 0x0810_0000 {
                    crate::i18n::tf(
                        "→ {addr} (Flash, Thumb={thumb})",
                        &[
                            ("{addr}", "{addr}", &format!("0x{:08X}", val)),
                            ("{thumb}", "{thumb}", &thumb.to_string()),
                        ],
                    )
                } else {
                    crate::i18n::tf("→ {addr} (not in Flash region)", &[("{addr}", "{addr}", &format!("0x{:08X}", val))])
                }
            } else {
                format!("→ 0x{:08X}", val)
            };
            entries.push(VectorTableEntry {
                index: i,
                name,
                name_zh,
                address: addr,
                value: val,
                description: desc,
            });
        }
    }
    entries
}
