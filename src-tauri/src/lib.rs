#[cfg(not(windows))]
compile_error!("CapturaX only supports Windows (WASAPI, Media Foundation and WebView2).");

mod audio;
mod capture;
mod clock;
mod commands;
mod config;
mod error;
mod media;
mod stream;
mod video_modes;
mod window;

use commands::AppState;
use tauri::{Manager, RunEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                window::auto_grant_media_permissions(&window)?;
                app.manage(capture::ScreenCapture::new(capture::Bridge::new(window)?));
            }
            let config_dir = app.path().app_config_dir()?;
            let state = AppState::new(config::ConfigStore::load(config_dir));
            let initial = state.config.lock().unwrap().current.clone();
            app.manage(state);
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                handle.state::<AppState>().apply(&initial).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            window::set_window_animations,
            commands::clock_now,
            commands::get_config,
            commands::set_config,
            commands::audio_devices,
            commands::audio_set_preview_volume,
            commands::audio_apps,
            commands::audio_set_paired_input,
            commands::audio_set_output_paired_input,
            commands::system_languages,
            commands::video_modes,
            commands::capture_sources,
            commands::p2p_audio_start,
            commands::p2p_audio_stop,
            commands::capture_start,
            commands::capture_set_fps,
            commands::capture_stop,
            commands::recording_start,
            commands::recording_stop,
            commands::stream_start,
            commands::stream_prepare,
            commands::stream_unprepare,
            commands::stream_stop,
            commands::media_video_config,
            commands::media_video_chunk,
            commands::media_status,
            commands::stream_status,
            commands::firewall_fix,
            commands::save_screenshot,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            // Finaliza a gravação em andamento (o arquivo vira um MP4 comum) antes de sair.
            handle.state::<AppState>().shutdown();
            if let Some(capture) = handle.try_state::<capture::ScreenCapture>() {
                capture.stop();
            }
        }
    });
}
