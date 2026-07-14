use crate::{database::Database, transfer::export_data};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::{DialogExt, FilePath};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSettingsView {
    directory: Option<String>,
    folder_name: Option<String>,
    exists: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportWriteResult {
    folder_name: String,
    filename: String,
}

#[tauri::command]
pub fn get_export_settings(database: State<'_, Database>) -> Result<ExportSettingsView, String> {
    read_view(&database)
}

#[tauri::command]
pub fn choose_export_directory(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<ExportSettingsView, String> {
    let Some(selected) = app.dialog().file().blocking_pick_folder() else {
        return read_view(&database);
    };
    let path = match selected {
        FilePath::Path(path) => path,
        FilePath::Url(_) => {
            return Err("The selected export location is not a local folder.".into())
        }
    };
    if !path.is_dir() {
        return Err("The selected export location is not an accessible folder.".into());
    }
    save_directory(&database, Some(path.to_string_lossy().as_ref()))?;
    read_view(&database)
}

#[tauri::command]
pub fn clear_export_directory(database: State<'_, Database>) -> Result<ExportSettingsView, String> {
    save_directory(&database, None)?;
    read_view(&database)
}

#[tauri::command]
pub fn export_to_configured_directory(
    database: State<'_, Database>,
    kind: String,
    folder_id: Option<i64>,
    folder_name: Option<String>,
    date: String,
) -> Result<ExportWriteResult, String> {
    if !matches!(kind.as_str(), "library" | "aliases") {
        return Err("invalid export kind".into());
    }
    if !valid_date(&date) {
        return Err("invalid export date".into());
    }
    let directory = load_directory(&database)?
        .ok_or_else(|| "No export directory is configured.".to_string())?;
    let directory = PathBuf::from(directory);
    if !directory.is_dir() {
        return Err("The configured export directory no longer exists. Choose a new export location in Settings.".into());
    }
    let payload = export_data(&database, &kind, folder_id)?;
    let base = export_filename_base(&kind, folder_name.as_deref(), &date);
    let (path, filename) = available_export_path(&directory, &base)?;
    fs::write(&path, payload).map_err(|_| {
        "The export file could not be written to the configured folder.".to_string()
    })?;
    Ok(ExportWriteResult {
        folder_name: display_folder_name(&directory),
        filename,
    })
}

fn load_directory(database: &Database) -> Result<Option<String>, String> {
    database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?
        .query_row(
            "SELECT export_directory FROM app_settings WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to read export settings: {error}"))
}

fn save_directory(database: &Database, value: Option<&str>) -> Result<(), String> {
    database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?
        .execute(
            "UPDATE app_settings SET export_directory = ?1 WHERE id = 1",
            [value],
        )
        .map(|_| ())
        .map_err(|error| format!("failed to save export settings: {error}"))
}

fn read_view(database: &Database) -> Result<ExportSettingsView, String> {
    let directory = load_directory(database)?;
    let path = directory.as_deref().map(Path::new);
    Ok(ExportSettingsView {
        folder_name: path.map(display_folder_name),
        exists: path.is_none_or(Path::is_dir),
        directory,
    })
}

fn display_folder_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| path.to_str().unwrap_or("Export folder"))
        .to_string()
}

fn export_filename_base(kind: &str, folder_name: Option<&str>, date: &str) -> String {
    let prefix = if kind == "aliases" {
        "gif-alias-export"
    } else {
        "gif-library-export"
    };
    match folder_name
        .map(sanitize_filename)
        .filter(|value| !value.is_empty())
    {
        Some(folder) => format!("{prefix}-{folder}-{date}"),
        None => format!("{prefix}-{date}"),
    }
}

fn sanitize_filename(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            if matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            ) || character.is_control()
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    cleaned.trim().trim_matches('.').trim().to_string()
}

fn available_export_path(directory: &Path, base: &str) -> Result<(PathBuf, String), String> {
    for suffix in 0..10_000 {
        let filename = if suffix == 0 {
            format!("{base}.json")
        } else {
            format!("{base}({suffix}).json")
        };
        let path = directory.join(&filename);
        if !path.exists() {
            return Ok((path, filename));
        }
    }
    Err("Could not choose an unused export filename.".into())
}

fn valid_date(value: &str) -> bool {
    value.len() == 10
        && value.chars().enumerate().all(|(index, character)| {
            if matches!(index, 4 | 7) {
                character == '-'
            } else {
                character.is_ascii_digit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::{export_filename_base, sanitize_filename};

    #[test]
    fn creates_scoped_and_alias_export_names() {
        assert_eq!(
            export_filename_base("library", None, "2026-07-12"),
            "gif-library-export-2026-07-12"
        );
        assert_eq!(
            export_filename_base("library", Some("JJK"), "2026-07-12"),
            "gif-library-export-JJK-2026-07-12"
        );
        assert_eq!(
            export_filename_base("aliases", None, "2026-07-12"),
            "gif-alias-export-2026-07-12"
        );
        assert_eq!(sanitize_filename("Bad:/Name*"), "Bad__Name_");
    }
}
