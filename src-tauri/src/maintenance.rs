use crate::{
    database::Database,
    keybinds::{clear_runtime, KeybindManager},
};
use serde::Serialize;
use std::{fs, process::Command};
use tauri::{AppHandle, State};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WipeResult {
    cleanup_failures: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeResult {
    purged: usize,
    cleanup_failures: usize,
}

#[tauri::command]
pub fn purge_static_image_thumbnails(database: State<'_, Database>) -> Result<PurgeResult, String> {
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let candidates = {
        let mut statement = connection
            .prepare("SELECT id, media_type, storage_filename, thumbnail_filename FROM media WHERE source_kind = 'local_file' AND thumbnail_filename IS NOT NULL AND media_type IN ('png', 'jpg', 'jpeg')")
            .map_err(|error| format!("failed to prepare static thumbnail cleanup: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|error| format!("failed to find static thumbnails: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("failed to read static thumbnails: {error}"))?;
        rows
    };
    let mut verified = Vec::new();
    for (id, media_type, storage_filename, thumbnail_filename) in candidates {
        if !is_generated_thumbnail_name(&thumbnail_filename) {
            continue;
        }
        if media_type == "png" {
            let original = database.media_dir.join(&storage_filename);
            let Ok(bytes) = fs::read(original) else {
                continue;
            };
            if png_has_actl(&bytes) {
                continue;
            }
        }
        verified.push((id, thumbnail_filename));
    }
    if verified.is_empty() {
        return Ok(PurgeResult {
            purged: 0,
            cleanup_failures: 0,
        });
    }
    let transaction = connection
        .transaction()
        .map_err(|error| format!("failed to start thumbnail cleanup: {error}"))?;
    for (id, _) in &verified {
        transaction
            .execute(
                "UPDATE media SET thumbnail_filename = NULL WHERE id = ?1",
                [id],
            )
            .map_err(|error| format!("failed to detach static thumbnail: {error}"))?;
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit thumbnail cleanup: {error}"))?;

    let mut cleanup_failures = 0;
    for (_, thumbnail_filename) in &verified {
        if fs::remove_file(database.media_dir.join(thumbnail_filename)).is_err() {
            cleanup_failures += 1;
        }
    }
    Ok(PurgeResult {
        purged: verified.len(),
        cleanup_failures,
    })
}

fn is_generated_thumbnail_name(name: &str) -> bool {
    std::path::Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|filename| filename == name && filename.ends_with(".thumb.webp"))
}

fn png_has_actl(bytes: &[u8]) -> bool {
    bytes.windows(4).any(|chunk| chunk == b"acTL")
}

#[tauri::command]
pub fn wipe_library(
    app: AppHandle,
    database: State<'_, Database>,
    keybinds: State<'_, KeybindManager>,
    confirmation: String,
) -> Result<WipeResult, String> {
    if confirmation != "Reset" {
        return Err("Type Reset exactly to permanently wipe the library.".into());
    }
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("failed to start wipe transaction: {error}"))?;
    transaction
        .execute_batch(
            "DELETE FROM media;
             DELETE FROM folders;
             DELETE FROM tags;
             DELETE FROM sqlite_sequence WHERE name IN ('media', 'folders', 'tags');
             UPDATE app_settings SET show_focus_keybind = NULL, toggle_visibility_keybind = NULL, export_directory = NULL WHERE id = 1;",
        )
        .map_err(|error| format!("failed to clear library data: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit library wipe: {error}"))?;

    let mut cleanup_failures = Vec::new();
    if app.global_shortcut().unregister_all().is_err() {
        cleanup_failures.push("configured runtime keybinds could not be unregistered".into());
    }
    if clear_runtime(&keybinds).is_err() {
        cleanup_failures.push("runtime keybind state could not be reset".into());
    }
    match fs::read_dir(&database.media_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_generated_preview = path.is_file()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.ends_with(".thumb.webp"));
                if !is_generated_preview {
                    continue;
                }
                let result = fs::remove_file(&path);
                if result.is_err() {
                    cleanup_failures.push(format!(
                        "could not delete generated preview {}",
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("(unknown)")
                    ));
                }
            }
        }
        Err(_) => cleanup_failures.push("managed media directory could not be read".into()),
    }
    Ok(WipeResult { cleanup_failures })
}

#[tauri::command]
pub fn open_uninstall() -> Result<String, String> {
    let executable = std::env::current_exe()
        .map_err(|_| "Could not determine whether this is an installed build.".to_string())?;
    let development = executable.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("target")
    });
    if development {
        return Ok("Uninstaller is only available in installed builds. You can remove the development build manually.".into());
    }
    #[cfg(windows)]
    Command::new("explorer.exe")
        .arg("ms-settings:appsfeatures")
        .spawn()
        .map_err(|_| "Could not open Windows Installed Apps settings.".to_string())?;
    Ok("Opened Windows Installed Apps. Select this app there to uninstall it.".into())
}

#[cfg(test)]
mod tests {
    use super::{is_generated_thumbnail_name, png_has_actl};

    #[test]
    fn verifies_generated_thumbnail_names_without_paths() {
        assert!(is_generated_thumbnail_name("123.thumb.webp"));
        assert!(!is_generated_thumbnail_name("123.webp"));
        assert!(!is_generated_thumbnail_name("nested/123.thumb.webp"));
    }

    #[test]
    fn detects_standard_apng_control_chunk() {
        assert!(png_has_actl(b"PNG-data-acTL-more"));
        assert!(!png_has_actl(b"PNG-data-IHDR-more"));
    }
}
