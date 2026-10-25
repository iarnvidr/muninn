// Muninn: one app. The Rust core (caches, rules, yt-dlp worker, capture proxy,
// local server) runs in this process; the window is Muninn's own UI plus one
// native web view per live YouTube tab (see shell.rs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod deep_link;
mod e2e;
mod geometry;
#[cfg(target_os = "linux")]
mod linux_overlay;
mod shell;
mod trust;
#[cfg(target_os = "macos")]
mod trust_mac;
mod updater;

use muninn_core::{CoreConfig, Service};
use shell::Shell;
use std::sync::Arc;
use tauri::{Manager, WindowEvent};

fn bundled_bin(app: &tauri::App) -> Option<std::path::PathBuf> {
    // Contents/Resources/bin in the .app; MUNINN_BUNDLED_BIN overrides (dev, tests).
    if let Some(p) = std::env::var_os("MUNINN_BUNDLED_BIN") {
        return Some(p.into());
    }
    app.path()
        .resource_dir()
        .ok()
        .map(|d| d.join("bin"))
        .filter(|d| d.is_dir())
}

/// The macOS menu bar: the standard App/Edit/Window menus plus the browser shortcuts, which
/// work no matter which web view has the keyboard focus.
#[cfg(target_os = "macos")]
fn install_menu(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
    let item = |id: &str, text: &str, accel: &str| {
        MenuItemBuilder::with_id(id, text)
            .accelerator(accel)
            .build(app)
    };
    let app_menu = SubmenuBuilder::new(app, "Muninn")
        .about(None)
        .separator()
        .item(&item("settings", "Settings…", "CmdOrCtrl+,")?)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&item("new_tab", "New Tab", "CmdOrCtrl+T")?)
        .item(&item("close_tab", "Close Tab", "CmdOrCtrl+W")?)
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&item("reload", "Reload", "CmdOrCtrl+R")?)
        .item(&item("focus_address", "Focus Address Bar", "CmdOrCtrl+L")?)
        .build()?;
    let history = SubmenuBuilder::new(app, "History")
        .item(&item("back", "Back", "CmdOrCtrl+[")?)
        .item(&item("forward", "Forward", "CmdOrCtrl+]")?)
        .build()?;
    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .close_window()
        .build()?;
    let menu = MenuBuilder::new(app)
        .items(&[&app_menu, &file, &edit, &view, &history, &window])
        .build()?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        let Some(shell) = app.try_state::<Arc<Shell>>() else {
            return;
        };
        let shell = shell.inner().clone();
        let active = shell.tabs_state()["active"].as_u64();
        match (event.id().as_ref(), active) {
            ("settings", _) => open_settings(app),
            ("new_tab", _) => {
                shell.new_tab(None);
            }
            ("close_tab", Some(id)) => shell.close_tab(id),
            ("reload", Some(id)) => shell.reload(id),
            ("back", Some(id)) => shell.back(id),
            ("forward", Some(id)) => shell.forward(id),
            ("focus_address", _) => {
                if let Some(wv) = app.get_webview(shell::CHROME) {
                    // The key view must be the chrome page for the text field to take the keyboard.
                    let _ = wv.set_focus();
                    let _ = wv.eval("window.muninnFocusAddress && window.muninnFocusAddress()");
                }
            }
            _ => {}
        }
    });
    Ok(())
}

pub(crate) fn open_settings(app: &tauri::AppHandle) {
    show_main(app);
    if let Some(view) = app.get_webview(shell::CHROME) {
        let _ = view.eval("window.muninnOpenSettings && window.muninnOpenSettings()");
        let _ = view.set_focus();
    }
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_window(shell::MAIN) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[derive(Default)]
struct PendingAppLinks(std::sync::Mutex<Vec<tauri::Url>>);

fn flush_app_links(app: &tauri::AppHandle) {
    let Some(shell) = app.try_state::<Arc<Shell>>() else {
        return;
    };
    let pending = app.state::<PendingAppLinks>();
    let targets = std::mem::take(&mut *pending.0.lock().unwrap_or_else(|e| e.into_inner()));
    if targets.is_empty() {
        return;
    }
    for target in targets {
        shell.open_from_link(&target);
    }
    show_main(app);
}

fn open_app_links(app: &tauri::AppHandle, links: impl IntoIterator<Item = impl AsRef<str>>) {
    let targets: Vec<_> = links
        .into_iter()
        .filter_map(|link| deep_link::youtube_target(link.as_ref()))
        .collect();
    if targets.is_empty() {
        return;
    }
    // Opened events can precede Ready/setup on macOS. Keep requests until the
    // shell exists, then let its retained handoff revision reach the chrome.
    app.state::<PendingAppLinks>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .extend(targets);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || flush_app_links(&handle));
}

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--cli") {
        if let Err(e) = muninn_core::cli::run(args.collect()) {
            eprintln!("muninn: {e}");
            std::process::exit(1);
        }
        return;
    }
    let app = tauri::Builder::default()
        .manage(PendingAppLinks::default())
        .manage(Arc::new(updater::UpdaterState::default()))
        .plugin(updater::plugin_builder())
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            open_app_links(app, args.iter().skip(1));
            show_main(app);
        }))
        .invoke_handler(tauri::generate_handler![
            commands::app_boot,
            commands::tabs_state,
            commands::tab_new,
            commands::tab_activate,
            commands::tab_close,
            commands::tab_go,
            commands::tab_back,
            commands::tab_forward,
            commands::tab_reload,
            commands::tab_open_video,
            commands::tab_hibernate,
            commands::layout,
            commands::native_visible,
            commands::open_external,
            commands::trust_status,
            commands::trust_install,
            commands::signin_popup,
            commands::youtube_session_status,
            commands::export_diagnostics,
            commands::channel_unsubscribe,
            commands::page_open_video,
            commands::page_state,
            commands::e2e_result,
            updater::updater_status,
            updater::updater_check,
            updater::updater_download,
            updater::updater_install,
            updater::updater_cancel,
        ])
        .setup(|app| {
            // ~/Library/Application Support/io.github.iarnvidr.muninn (follows the bundle id)
            let data_dir = app.path().app_data_dir()?;
            let service = Arc::new(Service::start(
                CoreConfig {
                    data_dir,
                    bundled_bin: bundled_bin(app),
                    youtube_cookie_reader: Some(shell::youtube_cookie_reader(app.handle().clone())),
                    ..CoreConfig::default()
                },
                0,
            )?);
            let window = tauri::window::WindowBuilder::new(app, shell::MAIN)
                .title("Muninn")
                .inner_size(1360.0, 860.0)
                .min_inner_size(900.0, 600.0)
                .build()?;
            let handle = window.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    // Keep running (downloads continue); Cmd+Q quits, the Dock icon reopens.
                    #[cfg(target_os = "macos")]
                    {
                        api.prevent_close();
                        let _ = handle.hide();
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        let _ = (api, &handle);
                    }
                }
            });
            let shell = Shell::new(app.handle().clone(), service);
            app.manage(shell.clone());
            shell::build_chrome(&shell)?;
            shell.restore_tabs();
            flush_app_links(app.handle());
            // Direct executable launches (macOS Launch Services uses RunEvent::Opened).
            open_app_links(app.handle(), std::env::args().skip(1));
            shell.start_hibernation_thread();
            trust::start_monitor(&shell);
            e2e::start(&shell);
            #[cfg(target_os = "macos")]
            install_menu(app.handle())?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Muninn");

    app.run(|handle, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { ref urls } = event {
            open_app_links(handle, urls.iter().map(|url| url.as_str()));
        }
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            show_main(handle);
        }
        if let tauri::RunEvent::Exit = event {
            if let Some(shell) = handle.try_state::<Arc<Shell>>() {
                shell.core.shutdown();
            }
        }
        let _ = (handle, event);
    });
}
