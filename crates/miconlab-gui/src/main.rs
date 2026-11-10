// miconlab GUI 应用入口
//
// release 构建在 Windows 上使用 GUI 子系统，双击启动不再弹出控制台黑框；
// debug 构建保留控制台，便于查看日志。其余平台不受影响（cfg 不成立）。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
#[cfg(feature = "device-workspaces")]
mod device_stream;
#[cfg(feature = "device-workspaces")]
mod devices;
mod i18n;
#[cfg(target_os = "macos")]
mod macos_icon;
mod panels;
mod settings;
mod state;
mod state_manager;
mod theme;

fn main() -> eframe::Result<()> {
    // macOS：显式设置运行时 Dock 图标（winit 不处理，否则运行中显示通用占位图标）
    #[cfg(target_os = "macos")]
    macos_icon::set();

    // 启动时先按系统语言初始化（AppState::new 会再用配置里的手动选择覆盖）
    i18n::set_language(i18n::system_language());

    // 应用名称：品牌名 + 统一副标题（跟随当前语言）
    let app_title = format!(
        "{} · {}",
        i18n::t("app.title"),
        i18n::t("app.subtitle")
    );
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 600.0])
            .with_maximized(true)
            .with_title(app_title.clone()),
        ..Default::default()
    };
    eframe::run_native(
        &app_title,
        options,
        Box::new(|cc| Ok(Box::new(app::AppRoot::new(cc)))),
    )
}
