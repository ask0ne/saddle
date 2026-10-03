mod commands;
mod fleet;
mod platform;

use fleet::store::Fleet;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Fleet::default())
        .invoke_handler(tauri::generate_handler![
            commands::sessions,
            commands::mark_seen,
            commands::focus,
            commands::enter_saddle_mode,
            commands::enter_stable_mode,
            commands::quit,
            commands::app_version,
            commands::hooks_installed,
            commands::install_hooks,
            commands::keep_pill_transparent
        ])
        .setup(|app| {
            // Launches as a normal, Dock-visible app with the stable window.
            // Pill mode (accessory policy, no Dock icon) is entered on
            // demand — see commands::enter_saddle_mode for why it's an
            // activation-policy switch and not just a window toggle — driven
            // by the stable window's own post-splash check of the persisted pref.
            commands::start(app.handle());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
