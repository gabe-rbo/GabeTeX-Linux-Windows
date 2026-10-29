pub mod commands;
pub mod core;

/// Every command is registered here. To add one: write it in `commands.rs`, add it to this list,
/// and add the typed wrapper in `src/ipc.ts`.
pub fn run() {
    let app = tauri::Builder::default()
        .manage(commands::AppState::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::open_project,
            commands::list_files,
            commands::read_file,
            commands::write_file,
            commands::create_project,
            commands::project_index,
            commands::completions,
            commands::complete,
            commands::detect_tex,
            commands::compile,
            commands::cancel_compile,
            commands::synctex_forward,
            commands::synctex_inverse,
            commands::install_packages,
            commands::watch_project,
            commands::unwatch_project,
            commands::reveal_in_file_manager,
            commands::open_url,
            commands::load_settings,
            commands::save_settings,
            commands::recent_projects,
            commands::add_recent_project,
            commands::set_project_label,
            commands::read_pdf,
            commands::create_file,
            commands::create_folder,
            commands::rename_path,
            commands::duplicate_path,
            commands::move_to_trash,
            commands::open_in_default_app,
            commands::clean_auxiliary_files,
            commands::remove_recent_project,
        ])
        .build(tauri::generate_context!())
        .expect("error while building GabeTeX");
    app.run(|_, event| {
        if let tauri::RunEvent::Exit = event {
            core::watcher::unwatch_all();
            core::compiler::terminate_all_running_processes();
        }
    });
}
