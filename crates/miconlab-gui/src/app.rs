// AppRoot - eframe::App 实现
// 主窗口布局：顶部菜单栏 + 左侧导航栏 + 中央主区域 + 底部状态栏

#[cfg(feature = "device-workspaces")]
use crate::devices::{is_device_panel, DeviceId};
use crate::i18n::{Language, L10n};
use crate::panels::{PanelKind, render_panel};
use crate::state::AppState;
use crate::theme;
use egui::{Color32, RichText, Vec2};
use std::time::Duration;

/// 主应用
pub struct AppRoot {
    pub state: AppState,
}

impl AppRoot {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        // 应用工业风暗色主题
        theme::apply_theme(&cc.egui_ctx);
        // 注册内置 CJK 字体（Windows/macOS/Linux 均为同一份嵌入字节）
        crate::i18n::install_fonts(&cc.egui_ctx);

        // 初始化状态并刷新端口列表
        let mut state = AppState::new();
        state.refresh_ports();
        state.status_message = "Ready".to_string();
        // 启动 WebSocket 桥，让坐垫 HTML 可视化软件能连接
        state.start_web_bridge();

        // 注册图片加载器（导航栏品牌图标用）
        egui_extras::install_image_loaders(&cc.egui_ctx);

        Self { state }
    }

    /// 当前语言的 L10n
    fn l10n(&self) -> L10n {
        L10n::new(self.state.language)
    }
}

impl eframe::App for AppRoot {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 每帧非阻塞读取串口数据（烧录器读取时跳过，避免抢占响应数据）
        if !self.state.flasher_read_busy {
            self.state.poll_rx();
        }

        // 处理来自 HTML 客户端的 WebSocket 命令（坐垫可视化软件等）
        let web_commands = self.state.poll_web_commands();
        for cmd in web_commands {
            self.state.apply_web_command(cmd);
        }

        // 检查延迟端口刷新（USB 设备插入后系统注册有延迟，首次未发现时 500ms 后自动重试）
        self.state.check_pending_refresh();

        // 请求定时重绘，确保串口数据能持续刷新（~50fps）
        ctx.request_repaint_after(Duration::from_millis(20));

        // 同步窗口标题（随语言切换）：品牌名 + 统一副标题
        let title = format!(
            "{} · {}",
            crate::i18n::t("app.title"),
            crate::i18n::t("app.subtitle")
        );
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        let l10n = self.l10n();

        // 首次运行 Welcome 屏幕（模态窗口）
        if self.state.first_run {
            let mut keep_open = true;
            let mut start_clicked = false;
            egui::Window::new(crate::i18n::t("Welcome"))
                .open(&mut keep_open)
                .resizable(false)
                .collapsible(false)
                .min_width(720.0)
                .max_width(820.0)
                .default_pos([(1280.0 - 760.0) / 2.0, 80.0])
                .show(ctx, |ui| {
                    start_clicked = render_welcome_screen(ui);
                });
            if !keep_open || start_clicked {
                self.state.first_run = false;
            }
        }

        // About 面板：品牌名 + 统一副标题 + 版本
        if self.state.show_about {
            let mut open = true;
            egui::Window::new(crate::i18n::t("About"))
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(crate::i18n::t("app.title"))
                                .color(theme::ACCENT)
                                .strong()
                                .size(20.0),
                        );
                        ui.label(
                            RichText::new(crate::i18n::t("app.subtitle"))
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(4.0);
                        ui.label(format!("v{}", env!("CARGO_PKG_VERSION")));
                        ui.add_space(6.0);
                    });
                });
            if !open {
                self.state.show_about = false;
            }
        }

        // 顶部菜单栏
        egui::TopBottomPanel::top("menu_bar")
            .exact_height(24.0)
            .frame(
                egui::Frame::group(&ctx.style())
                    .fill(theme::BG_ELEVATED)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .inner_margin(egui::Margin::same(2.0)),
            )
            .show(ctx, |ui| {
                render_menu_bar(ui, &mut self.state, &l10n);
            });

        // 底部状态栏
        egui::TopBottomPanel::bottom("status_bar")
            .exact_height(28.0)
            .frame(
                egui::Frame::group(&ctx.style())
                    .fill(theme::BG_ELEVATED)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .inner_margin(egui::Margin::same(4.0)),
            )
            .show(ctx, |ui| {
                render_status_bar(ui, &mut self.state, &l10n);
            });

        // 左侧导航栏
        egui::SidePanel::left("nav_panel")
            .resizable(false)
            .exact_width(160.0)
            .frame(
                egui::Frame::group(&ctx.style())
                    .fill(theme::BG_SURFACE)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .inner_margin(egui::Margin::same(6.0)),
            )
            .show(ctx, |ui| {
                render_nav_panel(ui, &mut self.state, &l10n);
            });

        // 中央主区域 - 渲染当前激活的面板
        // 检查是否有面板跳转请求（如 terminal 异常告警条点击"查看详情"跳转到 AI 助手）
        let navigate_to_ai = ctx.data_mut(|d| {
            let flag = d.get_temp_mut_or_default::<bool>(egui::Id::new("ai_navigate_to_assistant"));
            let v = *flag;
            *flag = false;
            v
        });
        if navigate_to_ai {
            self.state.active_panel = PanelKind::AiAssistant;
        }
        let active_panel = self.state.active_panel;
        render_panel(active_panel, ctx, &mut self.state);
    }
}

/// 渲染顶部菜单栏
fn render_menu_bar(ui: &mut egui::Ui, state: &mut AppState, l10n: &L10n) {
    egui::menu::bar(ui, |ui| {
        // File 菜单
        ui.menu_button(l10n.t("menu.file"), |ui| {
            if ui
                .button(crate::i18n::t("Exit"))
                .clicked()
            {
                std::process::exit(0);
            }
        });

        // View 菜单 - 面板切换
        ui.menu_button(l10n.t("menu.view"), |ui| {
            for &panel in PanelKind::all() {
                if ui
                    .button(format!("{}  {}", panel.icon(), panel.label(l10n)))
                    .clicked()
                {
                    state.active_panel = panel;
                    ui.close_menu();
                }
            }
        });

        // Language 菜单：跟随系统 + 资源文件里发现的全部语言（新增语种无需改代码）
        ui.menu_button(l10n.t("Language"), |ui| {
            let follows_system = state.language_follows_system();
            if ui
                .radio(follows_system, crate::i18n::t("Follow system"))
                .clicked()
            {
                state.set_language(None);
                ui.close_menu();
            }
            ui.separator();
            for info in Language::all() {
                let selected = !follows_system && state.language == info.language;
                if ui.radio(selected, info.native_name).clicked() {
                    state.set_language(Some(info.language.code()));
                    ui.close_menu();
                }
            }
        });

        // Help 菜单
        ui.menu_button(l10n.t("menu.help"), |ui| {
            if ui
                .button(crate::i18n::t("About"))
                .clicked()
            {
                state.show_about = true;
                ui.close_menu();
            }
        });

        // 右侧标题
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(l10n.t("app.title"))
                    .color(theme::ACCENT)
                    .strong(),
            );
        });
    });
}

/// 渲染左侧导航面板
fn render_nav_panel(ui: &mut egui::Ui, state: &mut AppState, l10n: &L10n) {
    ui.vertical(|ui| {
        // 导航栏品牌图标：与应用图标同源（scripts/icon_preview.png 生成）
        ui.vertical_centered(|ui| {
            ui.add(
                egui::Image::from_bytes(
                    "bytes://app-icon.png",
                    include_bytes!("../icon_embedded.png"),
                )
                .fit_to_exact_size(egui::Vec2::splat(26.0)),
            );
            ui.label(
                RichText::new("miconlab")
                    .color(theme::TEXT_PRIMARY)
                    .strong()
                    .size(12.5),
            );
        });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);

        ui.label(
            RichText::new(crate::i18n::t("Navigation"))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
        ui.add_space(8.0);

        // 设备工作区分区：开发版为总入口 + 三台设备；公开版只保留「设备上位机」占位入口
        #[cfg(feature = "device-workspaces")]
        {
            nav_section(
                ui,
                crate::i18n::t("Device workspaces"),
            );
            nav_item(ui, state, PanelKind::DeviceHub, "");
            for dev in DeviceId::ALL {
                let meta = dev.meta();
                let label = dev.name();
                nav_item(ui, state, meta.panel, label);
            }
        }
        #[cfg(not(feature = "device-workspaces"))]
        nav_item(ui, state, PanelKind::DeviceHub, "");

        // 通用工具分区：既有面板全部保留，只做分组
        nav_section(
            ui,
            crate::i18n::t("General tools"),
        );
        for &panel in PanelKind::all() {
            if panel == PanelKind::DeviceHub {
                continue;
            }
            #[cfg(feature = "device-workspaces")]
            if is_device_panel(panel) {
                continue;
            }
            let label = panel.label(l10n);
            nav_item(ui, state, panel, label);
        }

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);

        // 连接状态指示
        ui.horizontal(|ui| {
            let (dot, color) = if state.is_connected() {
                ("\u{25CF}", theme::SUCCESS)
            } else {
                ("\u{25CB}", theme::TEXT_DISABLED)
            };
            ui.label(RichText::new(dot).color(color).size(12.0));
            ui.label(
                RichText::new(if state.is_connected() {
                    l10n.t("connect.connected")
                } else {
                    l10n.t("connect.disconnected")
                })
                .color(color)
                .small(),
            );
        });

        // 当前端口
        if let Some(name) = state.selected_port_name() {
            ui.add_space(4.0);
            ui.label(
                RichText::new(name)
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .family(egui::FontFamily::Monospace),
            );
        }
    });
}

/// 渲染底部状态栏
fn render_status_bar(ui: &mut egui::Ui, state: &mut AppState, l10n: &L10n) {
    ui.horizontal(|ui| {
        // 连接状态指示灯
        let (dot, dot_color) = if state.is_connected() {
            ("\u{25CF}", theme::SUCCESS)
        } else {
            ("\u{25CB}", theme::TEXT_DISABLED)
        };
        ui.label(RichText::new(dot).color(dot_color).size(10.0));
        ui.label(
            RichText::new(if state.is_connected() {
                l10n.t("connect.connected")
            } else {
                l10n.t("connect.disconnected")
            })
            .color(dot_color)
            .small(),
        );

        ui.separator();

        // 端口名
        let port_name = state.selected_port_name().unwrap_or("—");
        ui.label(
            RichText::new(port_name)
                .color(theme::TEXT_SECONDARY)
                .small()
                .family(egui::FontFamily::Monospace),
        );

        ui.separator();

        // 波特率
        if state.is_connected() {
            ui.label(
                RichText::new(format!("{} baud", state.config.baud_rate))
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .family(egui::FontFamily::Monospace),
            );
            ui.separator();
        }

        // TX/RX 字节计数
        ui.label(
            RichText::new(format!("{}: {}", l10n.t("status.bytes_tx"), state.bytes_tx))
                .color(theme::TX_COLOR)
                .small()
                .family(egui::FontFamily::Monospace),
        );
        ui.label(
            RichText::new(format!("{}: {}", l10n.t("status.bytes_rx"), state.bytes_rx))
                .color(theme::RX_COLOR)
                .small()
                .family(egui::FontFamily::Monospace),
        );

        // 右侧状态消息
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // 串扰告警标签（检测到 TX 自发自收时显示）
            if let Some(report) = &state.crosstalk_report {
                if report.is_loopback || report.is_half_echo() {
                    let _zh = state.language == crate::i18n::Language::Zh;
                    let label_text = crate::i18n::t("⚠ Crosstalk");
                    let resp = ui.add(
                        egui::Label::new(
                            RichText::new(label_text)
                                .color(theme::ERROR)
                                .small()
                                .strong(),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if resp.clicked() {
                        state.active_panel = PanelKind::Connect;
                    }
                    resp.on_hover_text(crate::i18n::t("CH340 TX self-loopback crosstalk detected, click for details"));
                    ui.separator();
                }
            }
            if let Some(err) = &state.last_error {
                ui.label(
                    RichText::new(format!("\u{26A0} {}", err))
                        .color(theme::ERROR)
                        .small(),
                );
            } else {
                ui.label(
                    RichText::new(l10n.t("status.ready"))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
            }
        });
    });
}

/// 渲染首次运行 Welcome 屏幕
/// 返回 true 表示用户点击了「开始使用」按钮
fn render_welcome_screen(ui: &mut egui::Ui) -> bool {
    let mut start_clicked = false;

    // ===== 标题（居中） =====
    ui.vertical_centered(|ui| {
        ui.add_space(4.0);
        ui.label(
            RichText::new(crate::i18n::t("app.title"))
            .color(theme::ACCENT)
            .strong()
            .size(22.0),
        );
        ui.label(
            RichText::new(crate::i18n::t("app.subtitle"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(crate::i18n::t("All-in-one serial development tool - connection, protocol analysis, firmware flashing, sensor development"))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
        ui.add_space(2.0);
        ui.label(
            RichText::new(crate::i18n::t("Version 0.4.0"))
            .color(theme::TEXT_DISABLED)
            .small(),
        );
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    // ===== 主要功能列表（两列布局） =====
    ui.label(
        RichText::new(crate::i18n::t("Key Features"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    // 功能列表：(图标, 名称, 说明)
    let features: &[(&str, &str, &str)] = &[
        ("\u{2699}", crate::i18n::t("Connection"), crate::i18n::t("Configure serial parameters and connect")),
        ("\u{2261}", crate::i18n::t("Terminal"), crate::i18n::t("Send/receive serial data for debugging")),
        ("\u{29C9}", "Modbus", crate::i18n::t("RTU/TCP master debugging")),
        ("\u{1D6BA}", crate::i18n::t("CAN Bus"), crate::i18n::t("SLCAN command interaction")),
        ("\u{21C4}", "I2C/SPI", crate::i18n::t("USB adapter scan & read/write")),
        ("\u{26A1}", crate::i18n::t("Flasher"), crate::i18n::t("Supports STM32/ESP32/Arduino (11 chips)")),
        ("\u{223C}", crate::i18n::t("Oscilloscope"), crate::i18n::t("Serial digital waveform analysis")),
        ("\u{1F4DD}", crate::i18n::t("Data Logger"), crate::i18n::t("Timestamped logging to file")),
        ("\u{2194}", crate::i18n::t("File Transfer"), "XMODEM/YMODEM/ZMODEM"),
        ("\u{1F310}", crate::i18n::t("Bridge & Sim"), crate::i18n::t("Serial bridge & Modbus slave emulation")),
        ("\u{1F9EE}", crate::i18n::t("Sensor Dev"), crate::i18n::t("Protocol, products, calibration, monitor, heatmap")),
    ];

    // 用 Grid 实现两列布局
    egui::Grid::new("welcome_features_grid")
        .num_columns(2)
        .spacing([16.0, 6.0])
        .min_col_width(320.0)
        .show(ui, |ui| {
            // 按两列分发：奇数项左列，偶数项右列
            let chunks: Vec<&[(&str, &str, &str)]> = features.chunks(2).collect();
            for row in chunks {
                // 左列
                if let Some(feat) = row.first() {
                    render_feature_item(ui, feat.0, feat.1, feat.2);
                }
                // 右列（可能为空）
                if let Some(feat) = row.get(1) {
                    render_feature_item(ui, feat.0, feat.1, feat.2);
                } else {
                    ui.label(RichText::new(""));
                }
                ui.end_row();
            }
        });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    // ===== 底部「开始使用」按钮（居中） =====
    ui.vertical_centered(|ui| {
        if ui
            .add(
                egui::Button::new(
                    RichText::new(crate::i18n::t("Get Started →"))
                        .color(Color32::BLACK)
                        .strong(),
                )
                .fill(theme::ACCENT)
                .min_size(Vec2::new(200.0, 32.0)),
            )
            .clicked()
        {
            start_clicked = true;
        }
        ui.add_space(2.0);
        ui.label(
            RichText::new(crate::i18n::t("Will not show again after closing"))
            .color(theme::TEXT_DISABLED)
            .small(),
        );
    });

    start_clicked
}

/// 渲染单个功能条目（图标 + 名称 + 说明）
fn render_feature_item(ui: &mut egui::Ui, icon: &str, name: &str, desc: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).color(theme::ACCENT).size(16.0));
        ui.vertical(|ui| {
            ui.label(
                RichText::new(name)
                    .color(theme::TEXT_PRIMARY)
                    .strong()
                    .small(),
            );
            ui.label(
                RichText::new(desc)
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
        });
    });
}

/// 导航分区标题
fn nav_section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(
        RichText::new(title)
            .color(theme::TEXT_DISABLED)
            .size(11.0),
    );
    ui.add_space(2.0);
}

/// 导航按钮
///
/// `label_override` 为空时使用面板自身的本地化名称（设备区用设备名覆盖）。
/// 点击设备面板时走 `enter_device`：同时载入该设备自己的端口/波特率并切面板。
fn nav_item(ui: &mut egui::Ui, state: &mut AppState, panel: PanelKind, label_override: &str) {
    let label: String = if label_override.is_empty() {
        let l10n = L10n::new(state.language);
        panel.label(&l10n).to_string()
    } else {
        label_override.to_string()
    };

    let is_active = state.active_panel == panel;
    let (btn_color, text_color) = if is_active {
        (theme::ACCENT, Color32::BLACK)
    } else {
        (theme::BG_ELEVATED, theme::TEXT_PRIMARY)
    };

    let btn = egui::Button::new(
        RichText::new(format!("{}  {}", panel.icon(), label)).color(text_color),
    )
    .fill(btn_color)
    .min_size(Vec2::new(140.0, 32.0))
    .stroke(if is_active {
        egui::Stroke::new(1.0_f32, theme::ACCENT)
    } else {
        egui::Stroke::new(1.0_f32, theme::BORDER)
    });

    if ui.add(btn).clicked() && !is_active {
        #[cfg(feature = "device-workspaces")]
        {
            // 进入设备工作区：应用该设备自己的串口配置
            if let Some(dev) = DeviceId::from_panel(panel) {
                state.enter_device(dev);
                ui.add_space(2.0);
                return;
            }
        }
        // 通用面板/总入口：只切面板，不影响当前设备配置
        state.active_panel = panel;
    }
    ui.add_space(2.0);
}
