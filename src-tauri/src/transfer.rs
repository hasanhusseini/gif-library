use crate::{
    database::Database,
    media::{create_in_transaction, list_with_connection, normalize, MediaInput},
    storage::validate_signature,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rusqlite::{params, OptionalExtension};
use serde::{
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{BufReader, Read},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::{DialogExt, FilePath};

const FORMAT_VERSION: u8 = 1;
const MAX_IMPORT_BYTES: usize = 50 * 1024 * 1024;
const MAX_TRANSFER_PAYLOAD_BYTES: u64 = 10 * 1024 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferFile {
    version: u8,
    kind: String,
    #[serde(default)]
    items: Vec<TransferItem>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferItem {
    title: String,
    source_kind: String,
    remote_url: Option<String>,
    #[serde(default)]
    external_url: Option<String>,
    #[serde(default)]
    hosted_url: Option<String>,
    original_filename: Option<String>,
    media_type: String,
    file_hash: Option<String>,
    notes: String,
    folder_paths: Vec<String>,
    tag_names: Vec<String>,
    alias_names: Vec<String>,
    #[serde(default)]
    media_base64: Option<String>,
    #[serde(default)]
    thumbnail_base64: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    kind: String,
    item_count: usize,
    conflict_count: usize,
    local_file_count: usize,
    remote_url_count: usize,
    conflicts: Vec<String>,
    alias_match_count: usize,
    alias_unmatched_count: usize,
    alias_unmatched: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    imported: usize,
    skipped: usize,
    imported_files: usize,
    imported_links: usize,
    skipped_duplicates: usize,
    skipped_unsupported: usize,
    unmatched_aliases: Vec<String>,
    alias_matched_records: usize,
    aliases_added: usize,
    aliases_skipped: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFileSelection {
    path: String,
    file_name: String,
    size_bytes: u64,
}

#[tauri::command]
pub fn export_library(
    database: State<'_, Database>,
    folder_id: Option<i64>,
) -> Result<String, String> {
    export_data(&database, "library", folder_id)
}

#[tauri::command]
pub fn export_aliases(
    database: State<'_, Database>,
    folder_id: Option<i64>,
) -> Result<String, String> {
    export_data(&database, "aliases", folder_id)
}

pub(crate) fn export_data(
    database: &Database,
    kind: &str,
    folder_id: Option<i64>,
) -> Result<String, String> {
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let records = list_with_connection(&connection)?;
    let scoped_ids = if let Some(folder_id) = folder_id {
        if folder_id == -1 {
            let mut statement = connection
                .prepare("SELECT id FROM media WHERE NOT EXISTS (SELECT 1 FROM media_folders WHERE media_id = media.id)")
                .map_err(|error| format!("failed to prepare uncategorized export: {error}"))?;
            let ids = statement
                .query_map([], |row| row.get(0))
                .map_err(|error| format!("failed to query uncategorized export: {error}"))?
                .collect::<Result<HashSet<i64>, _>>()
                .map_err(|error| format!("failed to read uncategorized export: {error}"))?;
            Some(ids)
        } else {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
                    [folder_id],
                    |row| row.get(0),
                )
                .map_err(|error| format!("failed to validate export folder: {error}"))?;
            if !exists {
                return Err("export folder not found".into());
            }
            let mut statement = connection
            .prepare(
                "WITH RECURSIVE scope(id) AS (
                    SELECT ?1
                    UNION ALL SELECT child.id FROM folders child JOIN scope ON child.parent_id = scope.id
                )
                SELECT DISTINCT media_id FROM media_folders WHERE folder_id IN scope",
            )
            .map_err(|error| format!("failed to prepare scoped export: {error}"))?;
            let ids = statement
                .query_map([folder_id], |row| row.get(0))
                .map_err(|error| format!("failed to query scoped export: {error}"))?
                .collect::<Result<HashSet<i64>, _>>()
                .map_err(|error| format!("failed to read scoped export: {error}"))?;
            Some(ids)
        }
    } else {
        None
    };
    let mut items = Vec::with_capacity(records.len());
    for record in records {
        if scoped_ids
            .as_ref()
            .is_some_and(|ids| !ids.contains(&record.id))
        {
            continue;
        }
        if kind == "aliases" && record.alias_names.is_empty() {
            continue;
        }
        let (media_base64, thumbnail_base64) =
            if kind == "library" && record.source_kind == "local_file" {
                let filenames: (String, Option<String>) = connection
                    .query_row(
                        "SELECT storage_filename, thumbnail_filename FROM media WHERE id = ?1",
                        [record.id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(|error| format!("failed to read managed filenames: {error}"))?;
                let media = fs::read(database.media_dir.join(&filenames.0))
                    .map_err(|error| format!("failed to read local media for export: {error}"))?;
                let thumbnail = filenames
                    .1
                    .map(|name| fs::read(database.media_dir.join(name)))
                    .transpose()
                    .map_err(|error| format!("failed to read thumbnail for export: {error}"))?;
                (
                    Some(BASE64.encode(media)),
                    thumbnail.map(|bytes| BASE64.encode(bytes)),
                )
            } else {
                (None, None)
            };
        items.push(TransferItem {
            title: record.title,
            source_kind: record.source_kind,
            remote_url: record.remote_url,
            external_url: record.external_url,
            hosted_url: record.hosted_url,
            original_filename: record.original_filename,
            media_type: record.media_type,
            file_hash: record.file_hash,
            notes: if kind == "library" {
                record.notes
            } else {
                String::new()
            },
            folder_paths: if kind == "library" {
                record.folder_names
            } else {
                Vec::new()
            },
            tag_names: if kind == "library" {
                record.tag_names
            } else {
                Vec::new()
            },
            alias_names: record.alias_names,
            media_base64,
            thumbnail_base64,
        });
    }
    serde_json::to_string_pretty(&TransferFile {
        version: FORMAT_VERSION,
        kind: kind.into(),
        items,
    })
    .map_err(|error| format!("failed to serialize export: {error}"))
}

#[tauri::command]
pub fn choose_import_file(app: AppHandle) -> Result<Option<ImportFileSelection>, String> {
    let Some(selected) = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = match selected {
        FilePath::Path(path) => path,
        FilePath::Url(_) => return Err("The selected import file is not a local file.".into()),
    };
    import_file_selection(path).map(Some)
}

#[tauri::command]
pub fn preview_import_file(
    database: State<'_, Database>,
    path: String,
) -> Result<ImportPreview, String> {
    let path = validate_import_file_path(&path)?;
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let mut conflicts = Vec::new();
    let mut conflict_count = 0;
    let mut item_count = 0;
    let mut local = 0;
    let mut remote = 0;
    let mut alias_match_count = 0;
    let mut alias_unmatched_count = 0;
    let mut alias_unmatched = Vec::new();
    let kind = stream_transfer_file(&path, |kind, item| {
        item_count += 1;
        if item.source_kind == "local_file" {
            local += 1;
        } else {
            remote += 1;
        }
        let matched = find_conflict(&connection, &item)?.is_some();
        if matched {
            conflict_count += 1;
            if conflicts.len() < 100 {
                conflicts.push(item.title.clone());
            }
        }
        if kind == "aliases" {
            if matched {
                alias_match_count += 1;
            } else {
                alias_unmatched_count += 1;
                if alias_unmatched.len() < 100 {
                    alias_unmatched.push(item.title);
                }
            }
        }
        Ok(())
    })?;
    Ok(ImportPreview {
        kind,
        item_count,
        conflict_count,
        local_file_count: local,
        remote_url_count: remote,
        conflicts,
        alias_match_count,
        alias_unmatched_count,
        alias_unmatched,
    })
}

#[tauri::command]
pub fn apply_import_file(
    database: State<'_, Database>,
    path: String,
    conflict_strategy: String,
    destination_folder_id: Option<i64>,
) -> Result<ImportResult, String> {
    if !matches!(conflict_strategy.as_str(), "skip" | "import_anyway") {
        return Err("invalid conflict strategy".into());
    }
    let path = validate_import_file_path(&path)?;
    apply_transfer_file(&database, &path, &conflict_strategy, destination_folder_id)
}
#[tauri::command]
pub fn preview_import(
    database: State<'_, Database>,
    payload: String,
) -> Result<ImportPreview, String> {
    let transfer = parse_transfer(&payload)?;
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let mut conflicts = Vec::new();
    let mut local = 0;
    let mut remote = 0;
    let mut alias_match_count = 0;
    let mut alias_unmatched = Vec::new();
    for item in &transfer.items {
        if item.source_kind == "local_file" {
            local += 1;
        } else {
            remote += 1;
        }
        let matched = find_conflict(&connection, item)?.is_some();
        if matched {
            conflicts.push(item.title.clone());
        }
        if transfer.kind == "aliases" {
            if matched {
                alias_match_count += 1;
            } else {
                alias_unmatched.push(item.title.clone());
            }
        }
    }
    Ok(ImportPreview {
        kind: transfer.kind,
        item_count: transfer.items.len(),
        conflict_count: conflicts.len(),
        local_file_count: local,
        remote_url_count: remote,
        conflicts,
        alias_match_count,
        alias_unmatched_count: alias_unmatched.len(),
        alias_unmatched,
    })
}

#[tauri::command]
pub fn apply_import(
    database: State<'_, Database>,
    payload: String,
    conflict_strategy: String,
    destination_folder_id: Option<i64>,
) -> Result<ImportResult, String> {
    if !matches!(conflict_strategy.as_str(), "skip" | "import_anyway") {
        return Err("invalid conflict strategy".into());
    }
    let transfer = parse_transfer(&payload)?;
    apply_transfer(
        &database,
        transfer,
        &conflict_strategy,
        destination_folder_id,
    )
}

fn apply_transfer(
    database: &Database,
    transfer: TransferFile,
    conflict_strategy: &str,
    destination_folder_id: Option<i64>,
) -> Result<ImportResult, String> {
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("failed to start import transaction: {error}"))?;
    if let Some(folder_id) = destination_folder_id {
        if transfer.kind == "aliases" && folder_id == -1 {
            // Uncategorized is a derived scope, not a normal folder row.
        } else {
            let exists: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
                    [folder_id],
                    |row| row.get(0),
                )
                .map_err(|error| format!("failed to validate import destination: {error}"))?;
            if !exists {
                return Err("The selected import destination no longer exists.".into());
            }
        }
    }
    let mut imported = 0;
    let mut skipped = 0;
    let mut imported_files = 0;
    let mut imported_links = 0;
    let mut skipped_duplicates = 0;
    let mut written_paths = Vec::new();
    let mut unmatched_aliases = Vec::new();
    let mut alias_matched_records = 0;
    let mut aliases_added = 0;
    let mut aliases_skipped = 0;
    for item in transfer.items {
        let mut conflict_id = find_conflict(&transaction, &item)?;
        if transfer.kind == "aliases" {
            if let Some(media_id) = conflict_id {
                if !media_is_in_scope(&transaction, media_id, destination_folder_id)? {
                    conflict_id = None;
                }
            }
            let Some(media_id) = conflict_id else {
                skipped += 1;
                aliases_skipped += normalize_names(item.alias_names).len();
                unmatched_aliases.push(item.title);
                continue;
            };
            alias_matched_records += 1;
            for alias in normalize_names(item.alias_names) {
                let changed = transaction
                    .execute(
                        "INSERT OR IGNORE INTO aliases (media_id, name) VALUES (?1, ?2)",
                        params![media_id, alias],
                    )
                    .map_err(|error| format!("failed to import alias: {error}"))?;
                if changed > 0 {
                    aliases_added += 1;
                } else {
                    aliases_skipped += 1;
                }
            }
            imported += 1;
            continue;
        }
        if conflict_id.is_some() && conflict_strategy == "skip" {
            skipped += 1;
            skipped_duplicates += 1;
            continue;
        }
        let is_file = item.source_kind == "local_file";
        match import_library_item(database, &transaction, item, destination_folder_id) {
            Ok(paths) => written_paths.extend(paths),
            Err(error) => {
                cleanup_files(&written_paths);
                return Err(error);
            }
        }
        imported += 1;
        if is_file {
            imported_files += 1;
        } else {
            imported_links += 1;
        }
    }
    if let Err(error) = transaction.commit() {
        cleanup_files(&written_paths);
        return Err(format!("failed to commit import transaction: {error}"));
    }
    Ok(ImportResult {
        imported,
        skipped,
        imported_files,
        imported_links,
        skipped_duplicates,
        skipped_unsupported: 0,
        unmatched_aliases,
        alias_matched_records,
        aliases_added,
        aliases_skipped,
    })
}

fn apply_transfer_file(
    database: &Database,
    path: &Path,
    conflict_strategy: &str,
    destination_folder_id: Option<i64>,
) -> Result<ImportResult, String> {
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("failed to start import transaction: {error}"))?;
    let mut imported = 0;
    let mut skipped = 0;
    let mut imported_files = 0;
    let mut imported_links = 0;
    let mut skipped_duplicates = 0;
    let mut written_paths = Vec::new();
    let mut unmatched_aliases = Vec::new();
    let mut alias_matched_records = 0;
    let mut aliases_added = 0;
    let mut aliases_skipped = 0;
    let mut destination_checked = false;
    let result = stream_transfer_file(path, |kind, item| {
        if !destination_checked {
            validate_import_destination(&transaction, kind, destination_folder_id)?;
            destination_checked = true;
        }
        let mut conflict_id = find_conflict(&transaction, &item)?;
        if kind == "aliases" {
            if let Some(media_id) = conflict_id {
                if !media_is_in_scope(&transaction, media_id, destination_folder_id)? {
                    conflict_id = None;
                }
            }
            let Some(media_id) = conflict_id else {
                skipped += 1;
                aliases_skipped += normalize_names(item.alias_names).len();
                unmatched_aliases.push(item.title);
                return Ok(());
            };
            alias_matched_records += 1;
            for alias in normalize_names(item.alias_names) {
                let changed = transaction
                    .execute(
                        "INSERT OR IGNORE INTO aliases (media_id, name) VALUES (?1, ?2)",
                        params![media_id, alias],
                    )
                    .map_err(|error| format!("failed to import alias: {error}"))?;
                if changed > 0 {
                    aliases_added += 1;
                } else {
                    aliases_skipped += 1;
                }
            }
            imported += 1;
            return Ok(());
        }
        if conflict_id.is_some() && conflict_strategy == "skip" {
            skipped += 1;
            skipped_duplicates += 1;
            return Ok(());
        }
        let is_file = item.source_kind == "local_file";
        match import_library_item(database, &transaction, item, destination_folder_id) {
            Ok(paths) => written_paths.extend(paths),
            Err(error) => {
                cleanup_files(&written_paths);
                return Err(error);
            }
        }
        imported += 1;
        if is_file {
            imported_files += 1;
        } else {
            imported_links += 1;
        }
        Ok(())
    });
    if let Err(error) = result {
        cleanup_files(&written_paths);
        return Err(error);
    }
    if !destination_checked {
        validate_import_destination(&transaction, "library", destination_folder_id)?;
    }
    if let Err(error) = transaction.commit() {
        cleanup_files(&written_paths);
        return Err(format!("failed to commit import transaction: {error}"));
    }
    Ok(ImportResult {
        imported,
        skipped,
        imported_files,
        imported_links,
        skipped_duplicates,
        skipped_unsupported: 0,
        unmatched_aliases,
        alias_matched_records,
        aliases_added,
        aliases_skipped,
    })
}

fn validate_import_destination(
    connection: &rusqlite::Connection,
    kind: &str,
    folder_id: Option<i64>,
) -> Result<(), String> {
    if let Some(folder_id) = folder_id {
        if kind == "aliases" && folder_id == -1 {
            return Ok(());
        }
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
                [folder_id],
                |row| row.get(0),
            )
            .map_err(|error| format!("failed to validate import destination: {error}"))?;
        if !exists {
            return Err("The selected import destination no longer exists.".into());
        }
    }
    Ok(())
}
fn media_is_in_scope(
    connection: &rusqlite::Connection,
    media_id: i64,
    folder_id: Option<i64>,
) -> Result<bool, String> {
    let Some(folder_id) = folder_id else {
        return Ok(true);
    };
    let scoped = if folder_id == -1 {
        connection.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM media_folders WHERE media_id = ?1)",
            [media_id],
            |row| row.get(0),
        )
    } else {
        connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM media_folders WHERE media_id = ?1 AND folder_id = ?2)",
            params![media_id, folder_id],
            |row| row.get(0),
        )
    };
    scoped.map_err(|error| format!("failed to check alias import scope: {error}"))
}

fn parse_transfer(payload: &str) -> Result<TransferFile, String> {
    if payload.len() as u64 > MAX_TRANSFER_PAYLOAD_BYTES {
        return Err("import file is too large; maximum import size is 10 GB".into());
    }
    let transfer: TransferFile =
        serde_json::from_str(payload).map_err(|error| format!("invalid import file: {error}"))?;
    if transfer.version != FORMAT_VERSION {
        return Err(format!("unsupported import version: {}", transfer.version));
    }
    if !matches!(transfer.kind.as_str(), "library" | "aliases") {
        return Err("unsupported import type".into());
    }
    Ok(transfer)
}

fn import_file_selection(path: PathBuf) -> Result<ImportFileSelection, String> {
    let path = validate_import_file_path(path.to_string_lossy().as_ref())?;
    let metadata = path
        .metadata()
        .map_err(|error| format!("failed to read import file metadata: {error}"))?;
    Ok(ImportFileSelection {
        file_name: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("Import file")
            .to_string(),
        path: path.to_string_lossy().to_string(),
        size_bytes: metadata.len(),
    })
}

fn validate_import_file_path(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    let metadata = path
        .metadata()
        .map_err(|error| format!("failed to read import file: {error}"))?;
    if !metadata.is_file() {
        return Err("The selected import path is not a file.".into());
    }
    if metadata.len() > MAX_TRANSFER_PAYLOAD_BYTES {
        return Err("import file is too large; maximum import size is 10 GB".into());
    }
    Ok(path)
}

fn stream_transfer_file<F>(path: &Path, mut on_item: F) -> Result<String, String>
where
    F: FnMut(&str, TransferItem) -> Result<(), String>,
{
    let file = File::open(path).map_err(|error| format!("failed to open import file: {error}"))?;
    stream_transfer_reader(BufReader::new(file), &mut on_item)
}

fn stream_transfer_reader<R, F>(reader: R, on_item: &mut F) -> Result<String, String>
where
    R: Read,
    F: FnMut(&str, TransferItem) -> Result<(), String>,
{
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    TransferStreamSeed { on_item }
        .deserialize(&mut deserializer)
        .map_err(|error| format!("invalid import file: {error}"))
}

struct TransferStreamSeed<'a, F> {
    on_item: &'a mut F,
}

impl<'de, 'a, F> DeserializeSeed<'de> for TransferStreamSeed<'a, F>
where
    F: FnMut(&str, TransferItem) -> Result<(), String>,
{
    type Value = String;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(TransferStreamVisitor {
            on_item: self.on_item,
        })
    }
}

struct TransferStreamVisitor<'a, F> {
    on_item: &'a mut F,
}

impl<'de, 'a, F> Visitor<'de> for TransferStreamVisitor<'a, F>
where
    F: FnMut(&str, TransferItem) -> Result<(), String>,
{
    type Value = String;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a GIF Library transfer object")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut version = None;
        let mut kind: Option<String> = None;
        let mut items_seen = false;
        let on_item = self.on_item;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "version" => version = Some(map.next_value::<u8>()?),
                "kind" => {
                    let value = map.next_value::<String>()?;
                    if !matches!(value.as_str(), "library" | "aliases") {
                        return Err(de::Error::custom("unsupported import type"));
                    }
                    kind = Some(value);
                }
                "items" => {
                    let Some(kind_value) = kind.as_deref() else {
                        return Err(de::Error::custom(
                            "import file must declare kind before items",
                        ));
                    };
                    validate_transfer_header(version, kind_value).map_err(de::Error::custom)?;
                    map.next_value_seed(TransferItemsSeed {
                        kind: kind_value,
                        on_item: &mut *on_item,
                    })?;
                    items_seen = true;
                }
                _ => {
                    let _: IgnoredAny = map.next_value()?;
                }
            }
        }
        let kind = kind.ok_or_else(|| de::Error::custom("import file is missing kind"))?;
        validate_transfer_header(version, &kind).map_err(de::Error::custom)?;
        if !items_seen {
            return Err(de::Error::custom("import file is missing items"));
        }
        Ok(kind)
    }
}

struct TransferItemsSeed<'a, 'b, F> {
    kind: &'a str,
    on_item: &'b mut F,
}

impl<'de, 'a, 'b, F> DeserializeSeed<'de> for TransferItemsSeed<'a, 'b, F>
where
    F: FnMut(&str, TransferItem) -> Result<(), String>,
{
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(TransferItemsVisitor {
            kind: self.kind,
            on_item: self.on_item,
        })
    }
}

struct TransferItemsVisitor<'a, 'b, F> {
    kind: &'a str,
    on_item: &'b mut F,
}

impl<'de, 'a, 'b, F> Visitor<'de> for TransferItemsVisitor<'a, 'b, F>
where
    F: FnMut(&str, TransferItem) -> Result<(), String>,
{
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an array of transfer items")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while let Some(item) = sequence.next_element::<TransferItem>()? {
            (self.on_item)(self.kind, item).map_err(de::Error::custom)?;
        }
        Ok(())
    }
}

fn validate_transfer_header(version: Option<u8>, kind: &str) -> Result<(), String> {
    let Some(version) = version else {
        return Err("import file is missing version".into());
    };
    if version != FORMAT_VERSION {
        return Err(format!("unsupported import version: {version}"));
    }
    if !matches!(kind, "library" | "aliases") {
        return Err("unsupported import type".into());
    }
    Ok(())
}
fn find_conflict(
    connection: &rusqlite::Connection,
    item: &TransferItem,
) -> Result<Option<i64>, String> {
    let result = if let Some(hash) = &item.file_hash {
        connection
            .query_row(
                "SELECT id FROM media WHERE file_hash = ?1 LIMIT 1",
                [hash],
                |row| row.get(0),
            )
            .optional()
    } else if let Some(url) = &item.remote_url {
        connection
            .query_row(
                "SELECT id FROM media WHERE remote_url = ?1 LIMIT 1",
                [url.trim()],
                |row| row.get(0),
            )
            .optional()
    } else {
        Ok(None)
    };
    result.map_err(|error| format!("failed to check import conflict: {error}"))
}

fn import_library_item(
    database: &Database,
    transaction: &rusqlite::Transaction<'_>,
    item: TransferItem,
    destination_folder_id: Option<i64>,
) -> Result<Vec<std::path::PathBuf>, String> {
    let mut folder_ids = ensure_folder_paths(transaction, &item.folder_paths)?;
    if let Some(folder_id) = destination_folder_id {
        if !folder_ids.contains(&folder_id) {
            folder_ids.push(folder_id);
        }
    }
    let (storage_filename, hash, written_paths, thumbnail_filename) =
        if item.source_kind == "local_file" {
            let bytes = BASE64
                .decode(
                    item.media_base64
                        .as_deref()
                        .ok_or("local item is missing media data")?,
                )
                .map_err(|error| format!("invalid local media data: {error}"))?;
            if bytes.is_empty() || bytes.len() > MAX_IMPORT_BYTES {
                return Err("imported local file must be between 1 byte and 50 MB".into());
            }
            validate_signature(&item.media_type, &bytes)?;
            let hash = format!("{:x}", Sha256::digest(&bytes));
            let token = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| format!("system clock error: {error}"))?
                .as_nanos();
            let extension = if item.media_type == "jpeg" {
                "jpg"
            } else {
                item.media_type.as_str()
            };
            let name = format!("{token}.{extension}");
            let media_path = database.media_dir.join(&name);
            fs::write(&media_path, bytes)
                .map_err(|error| format!("failed to restore local media: {error}"))?;
            let mut written = vec![media_path];
            let thumbnail_name = if let Some(encoded) = &item.thumbnail_base64 {
                let bytes = match BASE64.decode(encoded) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        let _ = fs::remove_file(&written[0]);
                        return Err(format!("invalid thumbnail data: {error}"));
                    }
                };
                if bytes.len() > 5 * 1024 * 1024 {
                    let _ = fs::remove_file(&written[0]);
                    return Err("imported thumbnail exceeds 5 MB".into());
                }
                let name = format!("{token}.thumb.webp");
                let path = database.media_dir.join(&name);
                if let Err(error) = fs::write(&path, bytes) {
                    let _ = fs::remove_file(&written[0]);
                    return Err(format!("failed to restore thumbnail: {error}"));
                }
                written.push(path);
                Some(name)
            } else {
                None
            };
            (Some(name), Some(hash), written, thumbnail_name)
        } else {
            (None, None, Vec::new(), None)
        };
    let input = normalize(MediaInput {
        title: item.title,
        source_kind: item.source_kind,
        remote_url: item.remote_url,
        external_url: item.external_url.or(item.hosted_url),
        storage_filename,
        original_filename: item.original_filename,
        media_type: item.media_type,
        file_hash: hash.or(item.file_hash),
        notes: item.notes,
        folder_names: Vec::new(),
        folder_ids,
        tag_names: item.tag_names,
        alias_names: item.alias_names,
    });
    let input = match input {
        Ok(value) => value,
        Err(error) => {
            for path in written_paths {
                let _ = fs::remove_file(path);
            }
            return Err(error);
        }
    };
    if let Err(error) = create_in_transaction(transaction, input, thumbnail_filename.as_deref()) {
        cleanup_files(&written_paths);
        return Err(error);
    }
    Ok(written_paths)
}

fn cleanup_files(paths: &[std::path::PathBuf]) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

fn ensure_folder_paths(
    connection: &rusqlite::Connection,
    paths: &[String],
) -> Result<Vec<i64>, String> {
    let mut result = Vec::new();
    for path in paths {
        let mut parent: Option<i64> = None;
        for component in path
            .split(" / ")
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let existing: Option<i64> = connection
                .query_row(
                    "SELECT id FROM folders WHERE name = ?1 COLLATE NOCASE AND parent_id IS ?2",
                    params![component, parent],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| format!("failed to find import folder: {error}"))?;
            parent = Some(if let Some(id) = existing {
                id
            } else {
                connection
                    .execute(
                        "INSERT INTO folders (name, parent_id) VALUES (?1, ?2)",
                        params![component, parent],
                    )
                    .map_err(|error| format!("failed to create import folder: {error}"))?;
                connection.last_insert_rowid()
            });
        }
        if let Some(id) = parent {
            result.push(id);
        }
    }
    result.sort_unstable();
    result.dedup();
    Ok(result)
}

fn normalize_names(mut names: Vec<String>) -> Vec<String> {
    names = names
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect();
    names.sort_by_key(|value| value.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

#[cfg(test)]
mod tests {
    use super::{apply_transfer, export_data, TransferFile, TransferItem, FORMAT_VERSION};
    use crate::database::Database;
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn local_item(title: &str, bytes: &[u8]) -> TransferItem {
        TransferItem {
            title: title.into(),
            source_kind: "local_file".into(),
            remote_url: None,
            external_url: None,
            hosted_url: None,
            original_filename: Some(format!("{title}.gif")),
            media_type: "gif".into(),
            file_hash: None,
            notes: String::new(),
            folder_paths: Vec::new(),
            tag_names: Vec::new(),
            alias_names: Vec::new(),
            media_base64: Some(BASE64.encode(bytes)),
            thumbnail_base64: None,
        }
    }

    fn remote_item(title: &str) -> TransferItem {
        TransferItem {
            title: title.into(),
            source_kind: "remote_url".into(),
            remote_url: Some(format!("https://example.com/{title}.gif")),
            external_url: Some(format!("https://example.com/{title}.gif")),
            hosted_url: None,
            original_filename: None,
            media_type: "gif".into(),
            file_hash: None,
            notes: String::new(),
            folder_paths: Vec::new(),
            tag_names: Vec::new(),
            alias_names: Vec::new(),
            media_base64: None,
            thumbnail_base64: None,
        }
    }

    #[test]
    fn failed_restore_rolls_back_database_and_files() {
        let token = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("gif-library-rollback-{token}"));
        let database = Database::initialize(&root).unwrap();
        let transfer = TransferFile {
            version: FORMAT_VERSION,
            kind: "library".into(),
            items: vec![
                local_item("valid", b"GIF89a-valid"),
                local_item("invalid", b"not-an-image"),
            ],
        };

        assert!(apply_transfer(&database, transfer, "skip", None).is_err());
        let count: i64 = database
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM media", [], |row| row.get(0))
            .unwrap();
        let stored_files = fs::read_dir(root.join("media")).unwrap().count();
        assert_eq!(count, 0);
        assert_eq!(stored_files, 0);
        drop(database);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn folder_export_includes_descendants_and_excludes_other_media() {
        let token = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("gif-library-export-scope-{token}"));
        let database = Database::initialize(&root).unwrap();
        {
            let connection = database.connection.lock().unwrap();
            connection
                .execute("INSERT INTO folders (name) VALUES ('Parent')", [])
                .unwrap();
            connection
                .execute(
                    "INSERT INTO folders (name, parent_id) VALUES ('Child', 1)",
                    [],
                )
                .unwrap();
            connection.execute(
                "INSERT INTO media (title, source_kind, remote_url, external_url, media_type) VALUES ('Included', 'remote_url', 'https://example.com/in.gif', 'https://example.com/in.gif', 'gif')",
                [],
            ).unwrap();
            connection.execute(
                "INSERT INTO media (title, source_kind, remote_url, external_url, media_type) VALUES ('Excluded', 'remote_url', 'https://example.com/out.gif', 'https://example.com/out.gif', 'gif')",
                [],
            ).unwrap();
            connection
                .execute(
                    "INSERT INTO media_folders (media_id, folder_id) VALUES (1, 2)",
                    [],
                )
                .unwrap();
        }
        let payload = export_data(&database, "library", Some(1)).unwrap();
        let transfer: TransferFile = serde_json::from_str(&payload).unwrap();
        assert_eq!(transfer.items.len(), 1);
        assert_eq!(transfer.items[0].title, "Included");
        assert_eq!(transfer.items[0].folder_paths, vec!["Parent / Child"]);
        let uncategorized: TransferFile =
            serde_json::from_str(&export_data(&database, "library", Some(-1)).unwrap()).unwrap();
        assert_eq!(uncategorized.items.len(), 1);
        assert_eq!(uncategorized.items[0].title, "Excluded");
        drop(database);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn import_destination_adds_folder_membership_and_reports_links() {
        let token = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("gif-library-import-destination-{token}"));
        let database = Database::initialize(&root).unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute("INSERT INTO folders (name) VALUES ('Rust')", [])
            .unwrap();
        let transfer = TransferFile {
            version: FORMAT_VERSION,
            kind: "library".into(),
            items: vec![remote_item("ferris")],
        };
        let result = apply_transfer(&database, transfer, "skip", Some(1)).unwrap();
        assert_eq!(result.imported_files, 0);
        assert_eq!(result.imported_links, 1);
        let membership: i64 = database
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM media_folders WHERE media_id = 1 AND folder_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(membership, 1);
        drop(database);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn alias_import_respects_folder_scope_and_reports_effect() {
        let token = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("gif-library-alias-scope-{token}"));
        let database = Database::initialize(&root).unwrap();
        {
            let connection = database.connection.lock().unwrap();
            connection
                .execute("INSERT INTO folders (name) VALUES ('A'), ('B')", [])
                .unwrap();
            connection.execute("INSERT INTO media (title, source_kind, remote_url, external_url, media_type) VALUES ('Match', 'remote_url', 'https://example.com/match.gif', 'https://example.com/match.gif', 'gif')", []).unwrap();
            connection
                .execute(
                    "INSERT INTO media_folders (media_id, folder_id) VALUES (1, 1)",
                    [],
                )
                .unwrap();
        }
        let alias_transfer = || {
            let mut item = remote_item("match");
            item.alias_names = vec!["reaction".into()];
            TransferFile {
                version: FORMAT_VERSION,
                kind: "aliases".into(),
                items: vec![item],
            }
        };
        let outside = apply_transfer(&database, alias_transfer(), "skip", Some(2)).unwrap();
        assert_eq!(outside.alias_matched_records, 0);
        assert_eq!(outside.aliases_added, 0);
        assert_eq!(outside.unmatched_aliases, vec!["match"]);

        let inside = apply_transfer(&database, alias_transfer(), "skip", Some(1)).unwrap();
        assert_eq!(inside.alias_matched_records, 1);
        assert_eq!(inside.aliases_added, 1);
        assert_eq!(inside.aliases_skipped, 0);
        drop(database);
        fs::remove_dir_all(root).unwrap();
    }
}
