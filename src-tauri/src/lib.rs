#[cfg(windows)]
mod credential_cleanup;
mod database;
mod export_settings;
mod folders;
mod keybinds;
mod maintenance;
mod media;
mod storage;
mod transfer;

use database::Database;
use export_settings::{
    choose_export_directory, clear_export_directory, export_to_configured_directory,
    get_export_settings,
};
use folders::{create_folder, delete_folder, folder_delete_impact, list_folders, rename_folder};
use keybinds::{
    get_keybind_settings, set_show_focus_keybind, set_toggle_visibility_keybind, KeybindManager,
};
use maintenance::{open_uninstall, purge_static_image_thumbnails, wipe_library};
use media::{
    apply_media_folder_changes, create_media, delete_media_for_undo, get_media, list_media,
    preview_duplicate_purge, purge_duplicate_folder_items, record_media_used,
    restore_deleted_media, restore_media_folder_memberships, update_media,
};
use storage::{
    clear_manual_preview, copy_local_media_file, import_local_media,
    list_available_local_media_ids, read_media_preview, reveal_local_media, upload_manual_preview,
};
use tauri::Manager;
use transfer::{apply_import, export_aliases, export_library, preview_import};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(KeybindManager::default())
        .plugin(keybinds::plugin())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            #[cfg(windows)]
            credential_cleanup::remove_obsolete_hosting_credential();
            let app_data_dir = app.path().app_data_dir()?;
            let database =
                Database::initialize(&app_data_dir).map_err(Box::<dyn std::error::Error>::from)?;
            app.manage(database);
            keybinds::initialize(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            create_media,
            get_media,
            list_media,
            update_media,
            delete_media_for_undo,
            restore_deleted_media,
            record_media_used,
            apply_media_folder_changes,
            restore_media_folder_memberships,
            preview_duplicate_purge,
            purge_duplicate_folder_items,
            import_local_media,
            upload_manual_preview,
            clear_manual_preview,
            read_media_preview,
            list_available_local_media_ids,
            reveal_local_media,
            copy_local_media_file,
            create_folder,
            list_folders,
            rename_folder,
            folder_delete_impact,
            delete_folder,
            export_aliases,
            export_library,
            preview_import,
            apply_import,
            get_keybind_settings,
            set_show_focus_keybind,
            set_toggle_visibility_keybind,
            get_export_settings,
            choose_export_directory,
            clear_export_directory,
            export_to_configured_directory,
            wipe_library,
            purge_static_image_thumbnails,
            open_uninstall
        ])
        .run(tauri::generate_context!())
        .expect("error while running the GIF Library application");
}
