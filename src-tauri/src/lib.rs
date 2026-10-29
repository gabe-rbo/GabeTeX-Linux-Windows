pub mod commands;
pub mod core;

/// Every command is registered here. To add one: write it in `commands.rs`, add it to this list,
/// and add the typed wrapper in `src/ipc.ts`.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::open_project,
            commands::list_files,
            commands::read_file,
            commands::write_file,
            commands::create_project,
            commands::project_index,
            commands::completions,
            commands::detect_tex,
            commands::compile,
            commands::synctex_forward,
            commands::synctex_inverse,
            commands::install_packages,
            commands::watch_project,
            commands::unwatch_project,
            commands::reveal_in_file_manager,
            commands::load_settings,
            commands::save_settings,
            commands::recent_projects,
            commands::add_recent_project,
            commands::set_project_label,
        ])
        .run(tauri::generate_context!())
        .expect("error while running GabeTeX");
}
