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

/// 装配原生透明材质、状态同步与共用 HID 服务，正常退出时恢复原灯效。
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
        ])
        .plugin(tauri_plugin_single_instance::init(|_, _, _| {}))
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .setup(|app| {
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
