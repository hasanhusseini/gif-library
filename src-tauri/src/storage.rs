use crate::{
    database::Database,
    media::{create_in_transaction, is_manual_preview_name, normalize, MediaInput, MediaRecord},
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::State;

const MAX_IMPORT_BYTES: usize = 50 * 1024 * 1024;
const MAX_PREVIEW_BYTES: usize = 10 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalImportInput {
    title: String,
    original_filename: String,
    media_type: String,
    #[serde(default)]
    external_url: Option<String>,
    bytes: Vec<u8>,
    thumbnail_bytes: Vec<u8>,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    folder_names: Vec<String>,
    #[serde(default)]
    folder_ids: Vec<i64>,
    #[serde(default)]
    tag_names: Vec<String>,
    #[serde(default)]
    alias_names: Vec<String>,
    #[serde(default)]
    import_anyway: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewData {
    bytes: Vec<u8>,
    mime_type: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualPreviewInput {
    media_type: String,
    bytes: Vec<u8>,
}

#[tauri::command]
pub fn import_local_media(
    database: State<'_, Database>,
    input: LocalImportInput,
) -> Result<MediaRecord, String> {
    if input.bytes.is_empty() || input.bytes.len() > MAX_IMPORT_BYTES {
        return Err("file must be between 1 byte and 50 MB".into());
    }
    let media_type = input.media_type.trim().to_ascii_lowercase();
    validate_signature(&media_type, &input.bytes)?;
    let file_hash = format!("{:x}", Sha256::digest(&input.bytes));
    if !input.import_anyway {
        let connection = database
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let duplicate: Option<String> = connection
            .query_row(
                "SELECT title FROM media WHERE file_hash = ?1 LIMIT 1",
                [&file_hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("failed to check for duplicate: {error}"))?;
        if let Some(title) = duplicate {
            return Err(format!("DUPLICATE_FILE:{title}"));
        }
    }
    let extension = if media_type == "jpeg" {
        "jpg"
    } else {
        &media_type
    };
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock error: {error}"))?
        .as_nanos();
    let storage_filename = format!("{token}.{extension}");
    let thumbnail_filename = format!("{token}.thumb.webp");
    let media_path = database.media_dir.join(&storage_filename);
    let thumbnail_path = database.media_dir.join(&thumbnail_filename);

    let normalized = normalize(MediaInput {
        title: input.title,
        source_kind: "local_file".into(),
        remote_url: None,
        external_url: input.external_url,
        storage_filename: Some(storage_filename),
        original_filename: Some(input.original_filename),
        media_type,
        file_hash: Some(file_hash),
        notes: input.notes,
        folder_names: input.folder_names,
        folder_ids: input.folder_ids,
        tag_names: input.tag_names,
        alias_names: input.alias_names,
    })?;

    fs::write(&media_path, &input.bytes)
        .map_err(|error| format!("failed to copy media into managed storage: {error}"))?;
    let saved_thumbnail = if input.thumbnail_bytes.is_empty() {
        None
    } else {
        match fs::write(&thumbnail_path, &input.thumbnail_bytes) {
            Ok(()) => Some(thumbnail_filename),
            Err(_) => {
                let _ = fs::remove_file(&thumbnail_path);
                None
            }
        }
    };

    let result = (|| {
        let mut connection = database
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("failed to start media transaction: {error}"))?;
        let record = create_in_transaction(&transaction, normalized, saved_thumbnail.as_deref())?;
        transaction
            .commit()
            .map_err(|error| format!("failed to commit media transaction: {error}"))?;
        Ok(record)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&media_path);
        if saved_thumbnail.is_some() {
            let _ = fs::remove_file(&thumbnail_path);
        }
    }
    result
}

#[tauri::command]
pub fn read_media_preview(database: State<'_, Database>, id: i64) -> Result<PreviewData, String> {
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let preview: Option<(String, String, Option<String>)> = connection.query_row(
        "SELECT COALESCE(thumbnail_filename, storage_filename), media_type, thumbnail_filename FROM media WHERE id = ?1 AND (thumbnail_filename IS NOT NULL OR source_kind = 'local_file')",
        [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional().map_err(|error| format!("failed to find preview: {error}"))?;
    let (filename, media_type, thumbnail_filename) =
        preview.ok_or_else(|| "local preview not found".to_string())?;
    let bytes = fs::read(safe_managed_path(&database.media_dir, &filename)?)
        .map_err(|error| format!("failed to read preview: {error}"))?;
    let mime_type = if let Some(thumbnail_filename) = thumbnail_filename {
        mime_for_filename(&thumbnail_filename)
    } else {
        mime_for_media_type(&media_type)
    };
    Ok(PreviewData { bytes, mime_type })
}

#[tauri::command]
pub fn upload_manual_preview(
    database: State<'_, Database>,
    id: i64,
    input: ManualPreviewInput,
) -> Result<(), String> {
    if input.bytes.is_empty() || input.bytes.len() > MAX_PREVIEW_BYTES {
        return Err("preview file must be between 1 byte and 10 MB".into());
    }
    let media_type = input.media_type.trim().to_ascii_lowercase();
    validate_signature(&media_type, &input.bytes)?;
    let extension = if media_type == "jpeg" {
        "jpg"
    } else {
        &media_type
    };
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock error: {error}"))?
        .as_nanos();
    let preview_filename = format!("{token}.manual-preview.{extension}");
    let preview_path = database.media_dir.join(&preview_filename);
    fs::write(&preview_path, &input.bytes)
        .map_err(|error| format!("failed to store manual preview: {error}"))?;

    let previous = (|| {
        let connection = database
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let previous: Option<String> = connection
            .query_row(
                "SELECT thumbnail_filename FROM media WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("failed to find existing preview: {error}"))?
            .flatten();
        let changed = connection
            .execute(
                "UPDATE media SET thumbnail_filename = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?2",
                rusqlite::params![preview_filename, id],
            )
            .map_err(|error| format!("failed to attach manual preview: {error}"))?;
        if changed == 0 {
            return Err("media record not found".into());
        }
        Ok(previous)
    })();
    match previous {
        Ok(previous) => {
            if let Some(previous) = previous {
                if let Ok(path) = safe_managed_path(&database.media_dir, &previous) {
                    let _ = fs::remove_file(path);
                }
            }
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&preview_path);
            Err(error)
        }
    }
}

#[tauri::command]
pub fn clear_manual_preview(database: State<'_, Database>, id: i64) -> Result<(), String> {
    let previous = {
        let connection = database
            .connection
            .lock()
            .map_err(|_| "database lock poisoned")?;
        let previous: Option<String> = connection
            .query_row(
                "SELECT thumbnail_filename FROM media WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("failed to find manual preview: {error}"))?
            .flatten();
        let Some(previous) = previous else {
            return Err("This media item does not have an uploaded preview.".into());
        };
        if !is_manual_preview_name(&previous) {
            return Err("Only uploaded manual previews can be cleared.".into());
        }
        connection
            .execute(
                "UPDATE media SET thumbnail_filename = NULL, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
                [id],
            )
            .map_err(|error| format!("failed to clear manual preview: {error}"))?;
        previous
    };
    let _ = fs::remove_file(safe_managed_path(&database.media_dir, &previous)?);
    Ok(())
}

fn mime_for_filename(filename: &str) -> String {
    if filename.ends_with(".thumb.webp") {
        "image/webp".into()
    } else {
        let extension = Path::new(filename)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        mime_for_media_type(&extension)
    }
}

fn mime_for_media_type(media_type: &str) -> String {
    match media_type {
        "gif" => "image/gif",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
    .into()
}

#[tauri::command]
pub fn list_available_local_media_ids(database: State<'_, Database>) -> Result<Vec<i64>, String> {
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let mut statement = connection
        .prepare("SELECT id, storage_filename FROM media WHERE source_kind = 'local_file' AND storage_filename IS NOT NULL")
        .map_err(|error| format!("failed to prepare local media query: {error}"))?;
    let records = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("failed to query local media: {error}"))?;
    let mut available = Vec::new();
    for record in records {
        let (id, filename) =
            record.map_err(|error| format!("failed to read local media: {error}"))?;
        if safe_managed_path(&database.media_dir, &filename)?.is_file() {
            available.push(id);
        }
    }
    Ok(available)
}

#[tauri::command]
pub fn reveal_local_media(database: State<'_, Database>, id: i64) -> Result<(), String> {
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let filename: Option<String> = connection
        .query_row(
            "SELECT storage_filename FROM media WHERE id = ?1 AND source_kind = 'local_file'",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("failed to find local media: {error}"))?;
    let path = safe_managed_path(
        &database.media_dir,
        &filename.ok_or_else(|| "local media not found".to_string())?,
    )?;
    Command::new("explorer.exe")
        .arg("/select,")
        .arg(path)
        .spawn()
        .map_err(|error| format!("failed to reveal local media: {error}"))?;
    Ok(())
}

#[tauri::command]
pub fn copy_local_media_file(database: State<'_, Database>, id: i64) -> Result<(), String> {
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let filename: Option<String> = connection
        .query_row(
            "SELECT storage_filename FROM media WHERE id = ?1 AND source_kind = 'local_file'",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("failed to find local media: {error}"))?;
    let path = safe_managed_path(
        &database.media_dir,
        &filename.ok_or_else(|| "local media not found".to_string())?,
    )?;
    if !path.is_file() {
        return Err("managed media file is missing".into());
    }
    copy_file_to_clipboard(&path)
}

#[cfg(windows)]
fn copy_file_to_clipboard(path: &Path) -> Result<(), String> {
    use std::{ffi::OsStr, mem::size_of, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::{
        DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
        Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
    };

    #[repr(C)]
    struct DropFiles {
        p_files: u32,
        x: i32,
        y: i32,
        non_client: i32,
        wide: i32,
    }

    const CF_HDROP: u32 = 15;
    let mut wide: Vec<u16> = OsStr::new(path.as_os_str()).encode_wide().collect();
    wide.extend([0, 0]);
    let allocation_size = size_of::<DropFiles>() + wide.len() * size_of::<u16>();

    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 {
            return Err(format!(
                "could not open the clipboard: {}",
                std::io::Error::last_os_error()
            ));
        }
        struct ClipboardGuard;
        impl Drop for ClipboardGuard {
            fn drop(&mut self) {
                unsafe {
                    CloseClipboard();
                }
            }
        }
        let _guard = ClipboardGuard;
        if EmptyClipboard() == 0 {
            return Err(format!(
                "could not clear the clipboard: {}",
                std::io::Error::last_os_error()
            ));
        }
        let memory = GlobalAlloc(GMEM_MOVEABLE, allocation_size);
        if memory.is_null() {
            return Err("could not allocate clipboard memory".into());
        }
        let target = GlobalLock(memory);
        if target.is_null() {
            GlobalFree(memory);
            return Err("could not lock clipboard memory".into());
        }
        ptr::write(
            target.cast::<DropFiles>(),
            DropFiles {
                p_files: size_of::<DropFiles>() as u32,
                x: 0,
                y: 0,
                non_client: 0,
                wide: 1,
            },
        );
        ptr::copy_nonoverlapping(
            wide.as_ptr().cast::<u8>(),
            target.cast::<u8>().add(size_of::<DropFiles>()),
            wide.len() * size_of::<u16>(),
        );
        GlobalUnlock(memory);
        if SetClipboardData(CF_HDROP, memory).is_null() {
            GlobalFree(memory);
            return Err(format!(
                "could not copy the file: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn copy_file_to_clipboard(_path: &Path) -> Result<(), String> {
    Err("copying files is supported on Windows only".into())
}

fn safe_managed_path(media_dir: &Path, filename: &str) -> Result<std::path::PathBuf, String> {
    if Path::new(filename)
        .file_name()
        .and_then(|value| value.to_str())
        != Some(filename)
    {
        return Err("invalid managed filename".into());
    }
    Ok(media_dir.join(filename))
}

pub(crate) fn validate_signature(media_type: &str, bytes: &[u8]) -> Result<(), String> {
    let valid = match media_type {
        "gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "png" => bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
        "jpg" | "jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err("file contents do not match a supported image format".into())
    }
}

#[cfg(test)]
mod tests {
    use super::validate_signature;
    #[test]
    fn validates_supported_signatures_and_rejects_video() {
        assert!(validate_signature("gif", b"GIF89a...").is_ok());
        assert!(validate_signature("mp4", b"....ftyp").is_err());
    }
}
