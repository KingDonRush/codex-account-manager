mod adapters;
mod application;
mod commands;
mod domain;
mod error;
mod ports;
pub fn run_integration_cli() -> Result<(), String> {
    #[cfg(any(target_os = "linux", windows))]
    if adapters::browser_runtime::native_bridge::is_host_request() {
        return adapters::browser_runtime::native_bridge::run_host()
            .map_err(|error| adapters::logging::sanitize_error(&error.to_string()));
    }
    application::integration_cli::run()
        .map_err(|error| adapters::logging::sanitize_error(&error.to_string()))
}

use std::sync::Arc;

use application::manager::ManagerCore;
use domain::OperationEvent;
use parking_lot::Mutex;
use ports::EventSink;
use tauri::{AppHandle, Emitter, Manager, RunEvent};

#[derive(Default)]
struct PendingActivation(Mutex<Vec<Vec<String>>>);

struct TauriEventSink {
    app: AppHandle,
}

impl EventSink for TauriEventSink {
    fn show_authorization(&self) {
        if let Some(window) = self.app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }
    fn emit(&self, event: &OperationEvent) {
        let _ = self.app.emit("operation-progress", event);
    }

    fn emit_dashboard_changed(&self) {
        let _ = self.app.emit("dashboard-changed", ());
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();
    if application::local_interface::try_metadata_cli(&context.config().identifier) {
        return;
    }
    tauri::Builder::default()
        .manage(PendingActivation::default())
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if let Some(core) = app.try_state::<ManagerCore>() {
                core.mark_ui_visible();
                application::local_interface::activate(&core, &args);
            } else if application::local_interface::activation_target(&args).ok().flatten().is_some() {
                if let Some(pending) = app.try_state::<PendingActivation>() {
                    let mut queued = pending.0.lock();
                    if queued.len() < 16 { queued.push(args); }
                }
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            configure_tray(app)?;
            let data_dir = app.path().app_data_dir()?;
            let bundled_runtime_dir = app.path().resource_dir()?.join("runtime/chromium");
            let core = ManagerCore::with_bundled_runtime(data_dir, Some(bundled_runtime_dir))?;
            core.set_event_sink(Arc::new(TauriEventSink {
                app: app.handle().clone(),
            }));
            app.manage(core.clone());
            core.recover_executions()?;
            core.recover_application_server_metadata()?;
            #[cfg(any(unix, windows))]
            core.start_integration_service()?;
            application::local_interface::activate(&core, &std::env::args().collect::<Vec<_>>());
            for args in app.state::<PendingActivation>().0.lock().drain(..) {
                application::local_interface::activate(&core, &args);
            }
            Ok(())
        })
        .plugin(
            tauri_plugin_updater::Builder::new()
                .pubkey("dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEREMjZGQ0E4QkVBMjdFQkYKUldTL2ZxSytxUHdtM1FtQUYyKysrdU56a2kxUmdTcFdsVW9HREZXVm13L3ZmTWRjK25JU0t3K2gK")
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::retry_browser_cleanup,
            commands::recover_saved_accounts,
            commands::integrations::clear_application_history,
            commands::integrations::export_application_history,
            commands::integrations::recover_application_credential,
            commands::focus_codex,
            commands::select_codex_desktop,
            commands::confirm_codex_close,
            commands::deletion::list_deletion_requests,
            commands::deletion::request_account_deletion,
            commands::deletion::confirm_deletion,
            commands::deletion::deny_deletion,
            commands::integrations::list_executions,
            commands::integrations::list_application_servers,
            commands::integrations::close_application_server,
            commands::integrations::cancel_execution,
            commands::integrations::start_application_login,
            commands::authorization::list_authorization_requests,
            commands::authorization::approve_authorization,
            commands::authorization::deny_authorization,
            commands::authorization::revoke_authorization,
            commands::integrations::get_integrations,
            commands::integrations::save_account_group,
            commands::integrations::delete_account_group,
            commands::integrations::save_application,
            commands::integrations::save_execution_binding,
            commands::integrations::set_account_policy,
            commands::integrations::save_mail_preference,
            commands::list_profiles,
            commands::get_manager_settings,
            commands::save_manager_settings,
            commands::save_profile_options,
            commands::open_profile_browser,
            commands::check_profile,
            commands::get_dashboard_snapshot,
            commands::list_mail_providers,
            commands::save_mail_provider,
            commands::list_operations,
            commands::get_operation_stages,
            commands::get_runtime_status,
            commands::inspect_shared_runtime,
            commands::recover_original_chat,
            commands::open_original_chat,
            commands::create_profile,
            commands::rename_profile,
            commands::update_profile_mail_url,
            commands::archive_profile,
            commands::restore_profile,
            commands::delete_profile,
            commands::unlink_profile,
            commands::logout_profile,
            commands::start_replacement,
            commands::get_replacement_preview,
            commands::confirm_replacement,
            commands::open_mail,
            commands::open_profile_folder,
            commands::start_login,
            commands::cancel_operation,
            commands::export_diagnostics,
            commands::start_switch_account,
            commands::check_for_update,
            commands::install_update,
        ])
        .on_window_event(|window, event| {
            if matches!(event,tauri::WindowEvent::Focused(true)){
                if let Some(core)=window.app_handle().try_state::<ManagerCore>(){core.mark_ui_visible();}
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if window.app_handle().try_state::<ManagerCore>().is_some_and(|core|core.keep_authorized_background()){
                    let _=window.hide();return;
                }
                request_shutdown(window.app_handle(), 0);
            }
        })
        .build(context)
        .expect("erro ao construir o gerenciador de contas do Codex")
        .run(|app, event| {
            if let RunEvent::ExitRequested { api, code, .. } = event {
                if app.try_state::<ManagerCore>().is_some_and(|core| !core.is_exit_ready()) {
                    api.prevent_exit();
                    request_shutdown(app, code.unwrap_or(0));
                }
            }
        })
}

fn request_shutdown(app: &AppHandle, code: i32) {
    let Some(core) = app.try_state::<ManagerCore>() else {
        return;
    };
    if !core.claim_shutdown() {
        return;
    }
    let core = core.inner().clone();
    let app = app.clone();
    let _ = app.emit("shutdown-state", serde_json::json!({"state":"waiting","message":"Encerrando operações e preservando gravações em andamento…"}));
    tauri::async_runtime::spawn_blocking(move || match core.shutdown() {
        Ok(()) => app.exit(code),
        Err(error) => {
            let _ = app.emit("shutdown-state", serde_json::json!({"state":"blocked","message":adapters::logging::sanitize_error(&error.to_string())}));
        }
    });
}

fn configure_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;
    let show = MenuItem::with_id(app, "show-manager", "Abrir gerenciador", true, None::<&str>)?;
    let quit = MenuItem::with_id(
        app,
        "quit-manager",
        "Sair completamente",
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut tray = TrayIconBuilder::with_id("account-manager")
        .menu(&menu)
        .tooltip("Gerenciador de contas do Codex")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show-manager" => {
                if let Some(core) = app.try_state::<ManagerCore>() {
                    core.mark_ui_visible();
                }
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
            "quit-manager" => request_shutdown(app, 0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
