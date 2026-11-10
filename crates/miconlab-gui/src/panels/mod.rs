// 面板注册与枚举
// 定义所有可用面板类型，提供名称和图标的辅助方法

use crate::i18n::L10n;

/// 面板类型枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKind {
    /// 设备总入口（开发版：三台设备工作区选择；公开版：商业版占位入口「设备上位机」）
    DeviceHub,
    Connect,
    Terminal,
    Modbus,
    Can,
    I2cSpi,
    Flasher,
    FirmwareEditor,
    Scope,
    Tools,
    Logger,
    Transfer,
    Bridge,
    SensorDev,
    #[cfg(feature = "device-workspaces")]
    Insole,
    #[cfg(feature = "device-workspaces")]
    InsoleJq,
    #[cfg(feature = "device-workspaces")]
    Cushion,
    AiAssistant,
}

impl PanelKind {
    /// 迭代所有已实现的面板
    #[cfg(feature = "device-workspaces")]
    pub fn all() -> &'static [PanelKind] {
        &[
            PanelKind::DeviceHub,
            PanelKind::Connect,
            PanelKind::Terminal,
            PanelKind::Modbus,
            PanelKind::Can,
            PanelKind::I2cSpi,
            PanelKind::Flasher,
            PanelKind::FirmwareEditor,
            PanelKind::Scope,
            PanelKind::Tools,
            PanelKind::Logger,
            PanelKind::Transfer,
            PanelKind::Bridge,
            PanelKind::SensorDev,
            PanelKind::Insole,
            PanelKind::InsoleJq,
            PanelKind::Cushion,
            PanelKind::AiAssistant,
        ]
    }

    /// 迭代所有已实现的面板（公开版：不含三个设备工作区）
    #[cfg(not(feature = "device-workspaces"))]
    pub fn all() -> &'static [PanelKind] {
        &[
            PanelKind::DeviceHub,
            PanelKind::Connect,
            PanelKind::Terminal,
            PanelKind::Modbus,
            PanelKind::Can,
            PanelKind::I2cSpi,
            PanelKind::Flasher,
            PanelKind::FirmwareEditor,
            PanelKind::Scope,
            PanelKind::Tools,
            PanelKind::Logger,
            PanelKind::Transfer,
            PanelKind::Bridge,
            PanelKind::SensorDev,
            PanelKind::AiAssistant,
        ]
    }

    /// 获取面板的 i18n key
    pub fn i18n_key(self) -> &'static str {
        match self {
            #[cfg(feature = "device-workspaces")]
            PanelKind::DeviceHub => "panel.device_hub",
            #[cfg(not(feature = "device-workspaces"))]
            PanelKind::DeviceHub => "panel.device_host",
            PanelKind::Connect => "panel.connect",
            PanelKind::Terminal => "panel.terminal",
            PanelKind::Modbus => "panel.modbus",
            PanelKind::Can => "panel.can",
            PanelKind::I2cSpi => "panel.i2c_spi",
            PanelKind::Flasher => "panel.flasher",
            PanelKind::FirmwareEditor => "panel.firmware_editor",
            PanelKind::Scope => "panel.scope",
            PanelKind::Tools => "panel.tools",
            PanelKind::Logger => "panel.logger",
            PanelKind::Transfer => "panel.transfer",
            PanelKind::Bridge => "panel.bridge",
            PanelKind::SensorDev => "panel.sensor_dev",
            #[cfg(feature = "device-workspaces")]
            PanelKind::Insole => "panel.insole",
            #[cfg(feature = "device-workspaces")]
            PanelKind::InsoleJq => "panel.insole_jq",
            #[cfg(feature = "device-workspaces")]
            PanelKind::Cushion => "panel.cushion",
            PanelKind::AiAssistant => "panel.ai_assistant",
        }
    }

    /// 获取面板的本地化名称
    pub fn label(self, l10n: &L10n) -> &'static str {
        l10n.t(self.i18n_key())
    }

    /// 获取面板的图标符号（Unicode 字符）
    pub fn icon(self) -> &'static str {
        match self {
            PanelKind::DeviceHub => "\u{1F5C2}",   // 🗂 设备总入口
            PanelKind::Connect => "\u{2699}",   // ⚙ 齿轮
            PanelKind::Terminal => "\u{2261}",   // ≡ 终端符号
            PanelKind::Modbus => "\u{29C9}",     // ⧉ 模块
            PanelKind::Can => "\u{1D6BA}",       // 𝚺 总线
            PanelKind::I2cSpi => "\u{21C4}",     // ⇄ 双向
            PanelKind::Flasher => "\u{26A1}",    // ⚡ 闪电
            PanelKind::FirmwareEditor => "\u{270F}", // ✏ 铅笔
            PanelKind::Scope => "\u{223C}",      // ∼ 波形
            PanelKind::Tools => "\u{1F527}",     // 🔧 工具
            PanelKind::Logger => "\u{1F4DD}",    // 📝 记录
            PanelKind::Transfer => "\u{2194}",   // ↔ 传输
            PanelKind::Bridge => "\u{1F310}",    // 🌐 网络
            PanelKind::SensorDev => "\u{1F9EE}", // 🧮 传感器开发（算盘符号）
            #[cfg(feature = "device-workspaces")]
            PanelKind::Insole => "\u{1F462}",    // 👢 设备 A 上位机（专用）
            #[cfg(feature = "device-workspaces")]
            PanelKind::InsoleJq => "\u{1F9F6}",   // 🦶 设备 B 上位机（独立面板，与设备 A 隔离）
            #[cfg(feature = "device-workspaces")]
            PanelKind::Cushion => "\u{1F9D8}",    // 🧘 设备 C 工作区（16×16）
            PanelKind::AiAssistant => "\u{1F916}", // 🤖 机器人
        }
    }
}

pub mod ai_assistant;
pub mod ai_sense;
pub mod bridge;
pub mod can;
pub mod connect;
#[cfg(feature = "device-workspaces")]
pub mod cushion;
#[cfg(feature = "device-workspaces")]
pub mod device_hub;
pub mod firmware_editor;
pub mod flasher;
pub mod i2c_spi;
#[cfg(feature = "device-workspaces")]
pub mod insole;
#[cfg(feature = "device-workspaces")]
pub mod insole_jq;
pub mod logger;
pub mod modbus;
pub mod scope;
pub mod sensor_dev;
pub mod terminal;
pub mod tools;
pub mod transfer;

/// 渲染当前激活的面板
pub fn render_panel(
    panel: PanelKind,
    ctx: &egui::Context,
    state: &mut crate::state::AppState,
) {
    match panel {
        #[cfg(feature = "device-workspaces")]
        PanelKind::DeviceHub => device_hub::show(ctx, state),
        #[cfg(not(feature = "device-workspaces"))]
        PanelKind::DeviceHub => render_commercial_placeholder(ctx),
        PanelKind::Connect => connect::show(ctx, state),
        PanelKind::Terminal => terminal::show(ctx, state),
        PanelKind::Modbus => modbus::show(ctx, state),
        PanelKind::Can => can::show(ctx, state),
        PanelKind::I2cSpi => i2c_spi::show(ctx, state),
        PanelKind::Flasher => flasher::show(ctx, state),
        PanelKind::FirmwareEditor => firmware_editor::show(ctx, state),
        PanelKind::Scope => scope::show(ctx, state),
        PanelKind::Tools => tools::show(ctx, state),
        PanelKind::Logger => logger::show(ctx, state),
        PanelKind::Transfer => transfer::show(ctx, state),
        PanelKind::Bridge => bridge::show(ctx, state),
        PanelKind::SensorDev => sensor_dev::show(ctx, state),
        #[cfg(feature = "device-workspaces")]
        PanelKind::Insole => insole::show(ctx, state),
        #[cfg(feature = "device-workspaces")]
        PanelKind::InsoleJq => insole_jq::show(ctx, state),
        #[cfg(feature = "device-workspaces")]
        PanelKind::Cushion => cushion::show(ctx, state),
        PanelKind::AiAssistant => ai_assistant::show(ctx, state),
    }
}

/// 公开版「设备上位机」占位页
///
/// 只保留入口、标题与图标，内容区替换为商业版说明；不包含任何设备实现。
#[cfg(not(feature = "device-workspaces"))]
fn render_commercial_placeholder(ctx: &egui::Context) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(120.0);
            ui.label(
                egui::RichText::new(PanelKind::DeviceHub.icon())
                    .size(48.0)
                    .color(crate::theme::TEXT_DISABLED),
            );
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(crate::i18n::t("panel.device_host"))
                    .size(20.0)
                    .strong()
                    .color(crate::theme::TEXT_PRIMARY),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(crate::i18n::t(
                    "This feature is available in the Commercial Edition only.",
                ))
                .size(14.0)
                .color(crate::theme::TEXT_SECONDARY),
            );
        });
    });
}
