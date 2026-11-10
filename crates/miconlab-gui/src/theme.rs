// 工业风暗色主题系统
// 定义所有颜色常量，并在应用启动时应用到 egui 的 Visuals

use egui::{Color32, Rounding, Stroke, Vec2};

// 背景层级（从深到浅）
pub const BG_DEEP: Color32 = Color32::from_rgb(0x0A, 0x0E, 0x14); // 最深背景
pub const BG_SURFACE: Color32 = Color32::from_rgb(0x13, 0x18, 0x20); // 面板背景
pub const BG_ELEVATED: Color32 = Color32::from_rgb(0x1C, 0x23, 0x30); // 卡片/输入框
pub const BG_HOVER: Color32 = Color32::from_rgb(0x24, 0x2D, 0x3D); // 悬停

// 边框
pub const BORDER: Color32 = Color32::from_rgb(0x2A, 0x34, 0x41);
pub const BORDER_ACTIVE: Color32 = Color32::from_rgb(0x3A, 0x4A, 0x5E);

// 强调色 - 工业琥珀色
pub const ACCENT: Color32 = Color32::from_rgb(0xFF, 0xB0, 0x00); // 主强调（琥珀）
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0xB0, 0x7A, 0x00); // 暗琥珀
pub const CYAN: Color32 = Color32::from_rgb(0x00, 0xB4, 0xD8); // 数据/科技青

// 文字
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xE0, 0xE6, 0xED);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x7A, 0x86, 0x99);
pub const TEXT_DISABLED: Color32 = Color32::from_rgb(0x4A, 0x54, 0x60);

// 状态色
pub const SUCCESS: Color32 = Color32::from_rgb(0x3F, 0xB9, 0x50);
pub const WARNING: Color32 = Color32::from_rgb(0xD2, 0x99, 0x22);
pub const ERROR: Color32 = Color32::from_rgb(0xF8, 0x51, 0x49);

// TX/RX 数据色
pub const TX_COLOR: Color32 = Color32::from_rgb(0x4F, 0xC3, 0xF7); // 发送-蓝
pub const RX_COLOR: Color32 = Color32::from_rgb(0x81, 0xC7, 0x84); // 接收-绿

/// 应用工业风暗色主题到 egui 上下文
pub fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    // 背景层级
    visuals.panel_fill = BG_SURFACE;
    visuals.window_fill = BG_ELEVATED;
    visuals.extreme_bg_color = BG_DEEP;
    visuals.faint_bg_color = BG_ELEVATED;

    // 默认圆角 - 工业风偏方正
    visuals.window_rounding = Rounding::same(3.0);
    visuals.menu_rounding = Rounding::same(3.0);
    visuals.widgets.noninteractive.rounding = Rounding::same(2.0);
    visuals.widgets.inactive.rounding = Rounding::same(2.0);
    visuals.widgets.hovered.rounding = Rounding::same(2.0);
    visuals.widgets.active.rounding = Rounding::same(2.0);
    visuals.widgets.open.rounding = Rounding::same(2.0);
    visuals.widgets.hovered.expansion = 0.0;
    visuals.widgets.active.expansion = 0.0;

    // 非交互组件（标签、分隔线等）
    visuals.widgets.noninteractive.bg_fill = BG_SURFACE;
    visuals.widgets.noninteractive.weak_bg_fill = BG_SURFACE;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_SECONDARY);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);

    // 非激活状态（普通按钮、输入框）
    visuals.widgets.inactive.bg_fill = BG_ELEVATED;
    visuals.widgets.inactive.weak_bg_fill = BG_ELEVATED;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);

    // 悬停状态
    visuals.widgets.hovered.bg_fill = BG_HOVER;
    visuals.widgets.hovered.weak_bg_fill = BG_HOVER;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, BORDER_ACTIVE);

    // 激活/按下状态
    visuals.widgets.active.bg_fill = ACCENT_DIM;
    visuals.widgets.active.weak_bg_fill = ACCENT_DIM;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);

    // 打开状态（下拉框等）
    visuals.widgets.open.bg_fill = BG_ELEVATED;
    visuals.widgets.open.weak_bg_fill = BG_ELEVATED;
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, BORDER_ACTIVE);

    // 选择高亮色
    visuals.selection.bg_fill = ACCENT_DIM;
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);

    // 超链接
    visuals.hyperlink_color = CYAN;

    // 窗口边框
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER);

    // 间距偏紧凑，提高信息密度
    visuals.clip_rect_margin = 4.0;

    ctx.set_visuals(visuals);

    // 中文字体由 i18n::install_fonts 统一注册内置 Noto Sans SC 子集，
    // 这里不再读取平台系统字体（避免依赖系统里的专有字体文件）。

    // 字体大小与文本样式映射
    use egui::TextStyle::*;
    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (Heading, egui::FontId::new(18.0, egui::FontFamily::Proportional)),
        (Name("Heading2".into()), egui::FontId::new(15.0, egui::FontFamily::Proportional)),
        (Body, egui::FontId::new(13.0, egui::FontFamily::Proportional)),
        (Monospace, egui::FontId::new(12.5, egui::FontFamily::Monospace)),
        (Button, egui::FontId::new(13.0, egui::FontFamily::Proportional)),
        (Small, egui::FontId::new(11.0, egui::FontFamily::Proportional)),
        (Name("Terminal".into()), egui::FontId::new(12.5, egui::FontFamily::Monospace)),
    ]
    .into();

    // 设置自定义间距
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(6.0, 4.0);
    style.spacing.button_padding = Vec2::new(8.0, 3.0);
    style.spacing.window_margin = egui::Margin::same(8.0);
    style.spacing.menu_margin = egui::Margin::same(6.0);
    ctx.set_style(style);
}

