mod app_lighting;
pub mod device;
pub mod hid;
mod integration;
pub mod lighting;
pub mod protocol;
mod runtime;
pub mod status;
pub mod storage;
use tauri::Manager;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

/// 托盘与重复启动共用唤回逻辑，避免窗口隐藏或最小化后无法再次打开。
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(error) = window.show()
            .and_then(|_| window.unminimize())
            .and_then(|_| window.set_focus())
        {
            eprintln!("AirPulse 显示窗口失败: {error}");
        }
    }
}

/// 先创建可唤回和退出的托盘入口，再允许主窗口隐藏，避免后台运行却无法操作。
fn setup_tray(app: &tauri::App) -> anyhow::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 AirPulse", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let icon = app.default_window_icon()
        .ok_or_else(|| anyhow::anyhow!("缺少 AirPulse 托盘图标"))?;

    TrayIconBuilder::with_id("airpulse")
        .icon(icon.clone())
        .tooltip("AirPulse · 状态灯联动")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            // 使用正常退出事件，让原有清理流程恢复被接管的灯效。
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(event, TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }) {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// 用户确认退出后触发正常退出事件，复用恢复灯效的清理流程。
#[tauri::command]
fn exit_application(app: tauri::AppHandle) {
    app.exit(0);
}

/// 装配透明材质、托盘与共用 HID 服务，关闭行为由用户选择，正常退出时恢复原灯效。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(app_lighting::LightingService::default())
        .invoke_handler(tauri::generate_handler![
            app_lighting::read_lighting,
            app_lighting::set_lighting,
            app_lighting::restore_lighting,
            runtime::get_status,
            runtime::start_automatic,
            runtime::stop_automatic,
            integration::get_codex_integration,
            integration::get_codex_event,
            integration::install_codex_integration,
            exit_application,
        ])
        .plugin(tauri_plugin_single_instance::init(|app, _, _| show_main_window(app)))
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    // 等待前端关闭弹窗的选择，页面尚未就绪时也不直接退出。
                    api.prevent_close();
                }
            }
        })
        .setup(|app| {
            setup_tray(app)?;
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            let window = app.get_webview_window("main").expect("主窗口应已创建");
            #[cfg(target_os = "windows")]
            let _ = window.set_effects(
                tauri::window::EffectsBuilder::new()
                    .effect(tauri::window::Effect::Acrylic)
                    .color(tauri::window::Color(235, 239, 243, 32))
                    .build(),
            );
            #[cfg(target_os = "macos")]
            let _ = window.set_effects(
                tauri::window::EffectsBuilder::new()
                    .effect(tauri::window::Effect::Sidebar)
                    .build(),
            );
            runtime::spawn(app.state::<app_lighting::LightingService>().inner().clone())?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
                if let Err(error) = runtime::shutdown(app.state::<app_lighting::LightingService>().inner()) {
                    eprintln!("AirPulse 退出恢复失败: {error:#}");
                }
            }
        });
}
