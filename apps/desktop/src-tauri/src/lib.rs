mod activity;
mod commands;
mod connection_id;
mod connections;
mod deadline;
mod desktop_shell;
mod error;
mod mcp_providers;
mod models;
mod operation;
mod platform;
mod process;
mod state;
mod tray;
mod tunnel_config;
mod webcodex;
mod workspace;

use state::AppState;
use tauri::Manager;
use tauri_plugin_autostart::MacosLauncher;

const TUNNEL_DEBUG_LOG_NAME: &str = "WebCodex-Tunnel-Debug.log";

fn reset_desktop_debug_log() {
    use std::io::Write as _;

    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Some(parent) = executable.parent() else {
        return;
    };
    let path = parent.join(TUNNEL_DEBUG_LOG_NAME);
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
    else {
        return;
    };
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let _ = writeln!(
        file,
        "[{timestamp_ms}] desktop_app phase=setup_begin exe={}",
        executable.to_string_lossy()
    );
}

fn append_desktop_debug_log(message: &str) {
    use std::io::Write as _;

    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Some(parent) = executable.parent() else {
        return;
    };
    let path = parent.join(TUNNEL_DEBUG_LOG_NAME);
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let _ = writeln!(file, "[{timestamp_ms}] {message}");
}

pub fn run() {
    let app = tauri::Builder::default()
        // Tauri recommends registering single-instance first so a secondary
        // process is rejected before any other plugin can initialize state.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            desktop_shell::handle_second_instance(app, &argv);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .setup(|app| {
            reset_desktop_debug_log();
            let data_dir = app.path().app_local_data_dir()?;
            let resource_dir = app.path().resource_dir()?;
            append_desktop_debug_log(&format!(
                "desktop_app phase=paths_resolved data_dir={} resource_dir={}",
                data_dir.to_string_lossy(),
                resource_dir.to_string_lossy()
            ));
            app.manage(AppState::new(data_dir, resource_dir)?);
            append_desktop_debug_log("desktop_app phase=state_ready");
            app.manage(desktop_shell::DesktopShellState::default());
            app.manage(tray::TrayPresentationCache::default());
            tray::setup(app.handle())?;

            let snapshot = app.state::<AppState>().get_state();
            tray::refresh_from_snapshot(app.handle(), &snapshot);
            if !desktop_shell::is_background_launch(std::env::args()) {
                desktop_shell::show_main_window(app.handle())?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_desktop_state,
            commands::workspace_query,
            commands::get_computer_permissions,
            commands::request_computer_permission,
            commands::get_runner_settings,
            commands::add_runner_plugin,
            commands::update_runner_settings,
            commands::restart_owned_runner,
            commands::open_powershell_install_guide,
            commands::get_launch_at_login,
            commands::set_launch_at_login,
            commands::refresh_runtime_status,
            commands::observe_chatgpt_activity,
            commands::resume_saved_runtime,
            commands::update_tunnel_proxy,
            commands::update_tunnel_config,
            commands::save_tunnel_profile,
            commands::save_mcp_provider,
            commands::remove_mcp_provider,
            commands::tunnel_profile_action,
            commands::inspect_project,
            commands::configure_local_setup,
            commands::activate_local_project,
            commands::configure_remote_setup,
            commands::start_quick_share,
            commands::stop_quick_share,
            commands::start_regular_tunnel,
            commands::stop_regular_tunnel,
            commands::stop_local_runtime,
            commands::cancel_desktop_operation,
            commands::get_bounded_activity,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build WebCodex Desktop");

    app.run(|app_handle, event| match event {
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } if label == desktop_shell::MAIN_WINDOW_LABEL => {
            let shell = app_handle.state::<desktop_shell::DesktopShellState>();
            if shell.close_disposition() == desktop_shell::CloseDisposition::HideWindow {
                api.prevent_close();
                let _ = desktop_shell::hide_main_window(app_handle);
            }
        }
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen {
            has_visible_windows,
            ..
        } => {
            if !has_visible_windows {
                let _ = desktop_shell::show_main_window(app_handle);
            }
        }
        tauri::RunEvent::ExitRequested { .. } => {
            app_handle
                .state::<desktop_shell::DesktopShellState>()
                .mark_exit_requested();
            let state = app_handle.state::<AppState>();
            tauri::async_runtime::block_on(state.shutdown());
        }
        tauri::RunEvent::Exit => {
            let state = app_handle.state::<AppState>();
            tauri::async_runtime::block_on(state.shutdown());
        }
        _ => {}
    });
}
