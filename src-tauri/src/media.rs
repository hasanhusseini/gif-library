use crate::database::Database;
use rusqlite::{params, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tauri::State;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRecord {
    pub id: i64,
    pub title: String,
    pub source_kind: String,
    pub remote_url: Option<String>,
    pub external_url: Option<String>,
    pub storage_filename: Option<String>,
    pub original_filename: Option<String>,
    pub media_type: String,
    pub file_hash: Option<String>,
    pub notes: String,
    pub created_at: String,
    pub updated_at: String,
    pub hosted_url: Option<String>,
    pub hosted_object_key: Option<String>,
    pub hosted_at: Option<String>,
    pub use_count: i64,
    pub last_used_at: Option<String>,
    pub has_manual_preview: bool,
    pub folder_names: Vec<String>,
    pub folder_ids: Vec<i64>,
    pub tag_names: Vec<String>,
    pub alias_names: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedMediaSnapshot {
    record: MediaRecord,
    thumbnail_filename: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaFolderState {
    pub media_id: i64,
    pub folder_ids: Vec<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicatePurgePreview {
    duplicate_groups: usize,
    membership_removals: usize,
    uncategorized_removals: usize,
    scopes_scanned: usize,
    normal_folder_removals: usize,
    title_only_groups_skipped: usize,
    group_reasons: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicatePurgeResult {
    duplicate_groups: usize,
    membership_removals: usize,
    uncategorized_removals: usize,
    scopes_scanned: usize,
    normal_folder_removals: usize,
    title_only_groups_skipped: usize,
    group_reasons: Vec<String>,
    previous_states: Vec<MediaFolderState>,
    deleted_snapshots: Vec<DeletedMediaSnapshot>,
}

#[derive(Debug)]
struct DuplicatePlan {
    duplicate_groups: usize,
    scopes_scanned: usize,
    title_only_groups_skipped: usize,
    group_reasons: Vec<String>,
    removals: Vec<DuplicateRemoval>,
}

#[derive(Debug)]
struct DuplicateRemoval {
    scope_id: i64,
    media_id: i64,
    remove_record: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInput {
    pub title: String,
    pub source_kind: String,
    pub remote_url: Option<String>,
    #[serde(default)]
    pub external_url: Option<String>,
    pub storage_filename: Option<String>,
    pub original_filename: Option<String>,
    pub media_type: String,
    pub file_hash: Option<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub folder_names: Vec<String>,
    #[serde(default)]
    pub folder_ids: Vec<i64>,
    #[serde(default)]
    pub tag_names: Vec<String>,
    #[serde(default)]
    pub alias_names: Vec<String>,
}

fn media_from_row(row: &Row<'_>) -> rusqlite::Result<MediaRecord> {
    Ok(MediaRecord {
        id: row.get(0)?,
        title: row.get(1)?,
        source_kind: row.get(2)?,
        remote_url: row.get(3)?,
        external_url: row.get(4)?,
        storage_filename: row.get(5)?,
        original_filename: row.get(6)?,
        media_type: row.get(7)?,
        file_hash: row.get(8)?,
        notes: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        hosted_url: row.get(12)?,
        hosted_object_key: row.get(13)?,
        hosted_at: row.get(14)?,
        use_count: row.get(15)?,
        last_used_at: row.get(16)?,
        has_manual_preview: row
            .get::<_, Option<String>>(17)?
            .is_some_and(|name| is_manual_preview_name(&name)),
        folder_names: Vec::new(),
        folder_ids: Vec::new(),
        tag_names: Vec::new(),
        alias_names: Vec::new(),
    })
}

const SELECT_MEDIA: &str = "SELECT id, title, source_kind, remote_url, external_url, storage_filename, original_filename, media_type, file_hash, notes, created_at, updated_at, hosted_url, hosted_object_key, hosted_at, use_count, last_used_at, thumbnail_filename FROM media";

pub(crate) fn normalize(input: MediaInput) -> Result<MediaInput, String> {
    let title = input.title.trim().to_owned();
    if title.is_empty() {
        return Err("title cannot be empty".into());
    }

    let media_type = input.media_type.trim().to_ascii_lowercase();
    if !matches!(media_type.as_str(), "gif" | "png" | "jpg" | "jpeg" | "webp") {
        return Err("unsupported media type; expected GIF, PNG, JPG/JPEG, or WEBP".into());
    }

    let remote_url = trim_optional(input.remote_url);
    let external_url = trim_optional(input.external_url);
    if let Some(url) = &external_url {
        validate_http_url(url, "externalUrl")?;
    }
    if let Some(url) = &remote_url {
        validate_http_url(url, "remoteUrl")?;
    }
    let storage_filename = trim_optional(input.storage_filename);
    match input.source_kind.as_str() {
        "remote_url" if remote_url.is_some() && storage_filename.is_none() => {}
        "local_file" if remote_url.is_none() && storage_filename.is_some() => {}
        "remote_url" => {
            return Err("remote URL media requires remoteUrl and no storageFilename".into())
        }
        "local_file" => return Err("local media requires storageFilename and no remoteUrl".into()),
        _ => return Err("sourceKind must be remote_url or local_file".into()),
    }

    Ok(MediaInput {
        title,
        media_type,
        remote_url,
        external_url,
        storage_filename,
        original_filename: trim_optional(input.original_filename),
        file_hash: trim_optional(input.file_hash),
        notes: input.notes.trim().to_owned(),
        folder_names: normalize_labels(input.folder_names),
        folder_ids: normalize_ids(input.folder_ids),
        tag_names: normalize_labels(input.tag_names),
        alias_names: normalize_labels(input.alias_names),
        ..input
    })
}

fn validate_http_url(value: &str, field: &str) -> Result<(), String> {
    if value.starts_with("http://") || value.starts_with("https://") {
        Ok(())
    } else {
        Err(format!("{field} must use http:// or https://"))
    }
}

fn normalize_ids(mut values: Vec<i64>) -> Vec<i64> {
    values.retain(|value| *value > 0);
    values.sort_unstable();
    values.dedup();
    values
}

fn normalize_labels(values: Vec<String>) -> Vec<String> {
    let mut values: Vec<_> = values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect();
    values.sort_by_key(|value| value.to_ascii_lowercase());
    values.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    values
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(crate) fn is_manual_preview_name(name: &str) -> bool {
    std::path::Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        == Some(name)
        && !name.ends_with(".thumb.webp")
}

#[tauri::command]
pub fn create_media(
    database: State<'_, Database>,
    input: MediaInput,
) -> Result<MediaRecord, String> {
    let input = normalize(input)?;
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("failed to start media transaction: {error}"))?;
    let record = create_in_transaction(&transaction, input, None)?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit media transaction: {error}"))?;
    Ok(record)
}

pub(crate) fn create_in_transaction(
    transaction: &Transaction<'_>,
    input: MediaInput,
    thumbnail_filename: Option<&str>,
) -> Result<MediaRecord, String> {
    transaction.execute(
        "INSERT INTO media (title, source_kind, remote_url, external_url, storage_filename, original_filename, media_type, file_hash, notes, thumbnail_filename) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![input.title, input.source_kind, input.remote_url, input.external_url, input.storage_filename, input.original_filename, input.media_type, input.file_hash, input.notes, thumbnail_filename],
    ).map_err(|error| format!("failed to create media record: {error}"))?;
    let id = transaction.last_insert_rowid();
    sync_folders(transaction, id, &input.folder_ids, &input.folder_names)?;
    sync_labels(
        transaction,
        id,
        "tags",
        "media_tags",
        "tag_id",
        &input.tag_names,
    )?;
    sync_aliases(transaction, id, &input.alias_names)?;
    find_by_id(transaction, id)?.ok_or_else(|| "created media record was not found".into())
}

#[tauri::command]
pub fn get_media(database: State<'_, Database>, id: i64) -> Result<Option<MediaRecord>, String> {
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    find_by_id(&connection, id)
}

#[tauri::command]
pub fn list_media(database: State<'_, Database>) -> Result<Vec<MediaRecord>, String> {
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    list_with_connection(&connection)
}

pub(crate) fn list_with_connection(
    connection: &rusqlite::Connection,
) -> Result<Vec<MediaRecord>, String> {
    let mut statement = connection
        .prepare(&format!("{SELECT_MEDIA} ORDER BY created_at DESC, id DESC"))
        .map_err(|error| format!("failed to prepare media query: {error}"))?;
    let mut records = statement
        .query_map([], media_from_row)
        .map_err(|error| format!("failed to list media records: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read media record: {error}"))?;
    for record in &mut records {
        hydrate_labels(connection, record)?;
    }
    Ok(records)
}

#[tauri::command]
pub fn update_media(
    database: State<'_, Database>,
    id: i64,
    input: MediaInput,
) -> Result<Option<MediaRecord>, String> {
    let input = normalize(input)?;
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("failed to start media transaction: {error}"))?;
    let changed = transaction.execute(
        "UPDATE media SET title = ?1, source_kind = ?2, remote_url = ?3, external_url = ?4, storage_filename = ?5, original_filename = ?6, media_type = ?7, file_hash = ?8, notes = ?9, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?10",
        params![input.title, input.source_kind, input.remote_url, input.external_url, input.storage_filename, input.original_filename, input.media_type, input.file_hash, input.notes, id],
    ).map_err(|error| format!("failed to update media record: {error}"))?;
    if changed == 0 {
        Ok(None)
    } else {
        sync_folders(&transaction, id, &input.folder_ids, &input.folder_names)?;
        sync_labels(
            &transaction,
            id,
            "tags",
            "media_tags",
            "tag_id",
            &input.tag_names,
        )?;
        sync_aliases(&transaction, id, &input.alias_names)?;
        let record = find_by_id(&transaction, id)?;
        transaction
            .commit()
            .map_err(|error| format!("failed to commit media transaction: {error}"))?;
        Ok(record)
    }
}

#[tauri::command]
pub fn delete_media_for_undo(
    database: State<'_, Database>,
    id: i64,
) -> Result<DeletedMediaSnapshot, String> {
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("failed to start remove transaction: {error}"))?;
    let record = find_by_id(&transaction, id)?.ok_or("media record not found")?;
    let thumbnail_filename = transaction
        .query_row(
            "SELECT thumbnail_filename FROM media WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to snapshot media preview: {error}"))?;
    transaction
        .execute("DELETE FROM media WHERE id = ?1", [id])
        .map_err(|error| format!("failed to remove media record: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit media removal: {error}"))?;
    Ok(DeletedMediaSnapshot {
        record,
        thumbnail_filename,
    })
}

#[tauri::command]
pub fn restore_deleted_media(
    database: State<'_, Database>,
    snapshot: DeletedMediaSnapshot,
) -> Result<(), String> {
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("failed to start undo transaction: {error}"))?;
    let record = snapshot.record;
    transaction.execute(
        "INSERT INTO media (id, title, source_kind, remote_url, external_url, storage_filename, original_filename, media_type, file_hash, notes, created_at, updated_at, thumbnail_filename, hosted_url, hosted_object_key, hosted_at, use_count, last_used_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![record.id, record.title, record.source_kind, record.remote_url, record.external_url, record.storage_filename, record.original_filename, record.media_type, record.file_hash, record.notes, record.created_at, record.updated_at, snapshot.thumbnail_filename, record.hosted_url, record.hosted_object_key, record.hosted_at, record.use_count, record.last_used_at],
    ).map_err(|error| format!("failed to restore media record: {error}"))?;
    sync_folders(&transaction, record.id, &record.folder_ids, &[])?;
    sync_labels(
        &transaction,
        record.id,
        "tags",
        "media_tags",
        "tag_id",
        &record.tag_names,
    )?;
    sync_aliases(&transaction, record.id, &record.alias_names)?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit media restore: {error}"))
}

#[tauri::command]
pub fn record_media_used(database: State<'_, Database>, id: i64) -> Result<(), String> {
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let changed = connection
        .execute(
            "UPDATE media SET use_count = use_count + 1, last_used_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
            [id],
        )
        .map_err(|error| format!("failed to update media usage: {error}"))?;
    if changed == 0 {
        return Err("media record not found".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderMembershipChange {
    folder_id: i64,
    assigned: bool,
}

#[tauri::command]
pub fn apply_media_folder_changes(
    database: State<'_, Database>,
    media_ids: Vec<i64>,
    folder_changes: Vec<FolderMembershipChange>,
) -> Result<(), String> {
    apply_media_folder_changes_inner(&database, media_ids, folder_changes)
}

#[tauri::command]
pub fn restore_media_folder_memberships(
    database: State<'_, Database>,
    states: Vec<MediaFolderState>,
) -> Result<(), String> {
    if states.is_empty() {
        return Err("No folder organization is available to undo.".into());
    }
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("failed to start folder undo transaction: {error}"))?;
    for state in states {
        sync_folders(
            &transaction,
            state.media_id,
            &normalize_ids(state.folder_ids),
            &[],
        )?;
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit folder undo: {error}"))
}

#[tauri::command]
pub fn preview_duplicate_purge(
    database: State<'_, Database>,
) -> Result<DuplicatePurgePreview, String> {
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let plan = duplicate_membership_removals(&connection)?;
    Ok(DuplicatePurgePreview {
        duplicate_groups: plan.duplicate_groups,
        membership_removals: plan.removals.len(),
        uncategorized_removals: plan
            .removals
            .iter()
            .filter(|removal| removal.scope_id == 0)
            .count(),
        normal_folder_removals: plan
            .removals
            .iter()
            .filter(|removal| removal.scope_id != 0)
            .count(),
        scopes_scanned: plan.scopes_scanned,
        title_only_groups_skipped: plan.title_only_groups_skipped,
        group_reasons: plan.group_reasons,
    })
}

#[tauri::command]
pub fn purge_duplicate_folder_items(
    database: State<'_, Database>,
) -> Result<DuplicatePurgeResult, String> {
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let plan = duplicate_membership_removals(&connection)?;
    let affected_ids = plan
        .removals
        .iter()
        .filter(|removal| removal.scope_id != 0 && !removal.remove_record)
        .map(|removal| removal.media_id)
        .collect::<HashSet<_>>();
    let mut previous_states = Vec::new();
    for media_id in affected_ids {
        previous_states.push(MediaFolderState {
            media_id,
            folder_ids: load_folders(&connection, media_id)?
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
        });
    }
    let mut deleted_snapshots = Vec::new();
    for removal in plan.removals.iter().filter(|removal| removal.remove_record) {
        let record = find_by_id(&connection, removal.media_id)?.ok_or("media record not found")?;
        let thumbnail_filename = connection
            .query_row(
                "SELECT thumbnail_filename FROM media WHERE id = ?1",
                [removal.media_id],
                |row| row.get(0),
            )
            .map_err(|error| format!("failed to snapshot duplicate media preview: {error}"))?;
        deleted_snapshots.push(DeletedMediaSnapshot {
            record,
            thumbnail_filename,
        });
    }
    let transaction = connection
        .transaction()
        .map_err(|error| format!("failed to start duplicate purge: {error}"))?;
    for removal in &plan.removals {
        if removal.remove_record {
            transaction
                .execute("DELETE FROM media WHERE id = ?1", [removal.media_id])
                .map_err(|error| format!("failed to remove duplicate library record: {error}"))?;
        } else {
            transaction
                .execute(
                    "DELETE FROM media_folders WHERE folder_id = ?1 AND media_id = ?2",
                    params![removal.scope_id, removal.media_id],
                )
                .map_err(|error| {
                    format!("failed to remove duplicate folder membership: {error}")
                })?;
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit duplicate purge: {error}"))?;
    Ok(DuplicatePurgeResult {
        duplicate_groups: plan.duplicate_groups,
        membership_removals: plan.removals.len(),
        uncategorized_removals: plan
            .removals
            .iter()
            .filter(|removal| removal.scope_id == 0)
            .count(),
        normal_folder_removals: plan
            .removals
            .iter()
            .filter(|removal| removal.scope_id != 0)
            .count(),
        scopes_scanned: plan.scopes_scanned,
        title_only_groups_skipped: plan.title_only_groups_skipped,
        group_reasons: plan.group_reasons,
        previous_states,
        deleted_snapshots,
    })
}

fn duplicate_membership_removals(
    connection: &rusqlite::Connection,
) -> Result<DuplicatePlan, String> {
    let records = list_with_connection(connection)?;
    let mut scopes: HashMap<i64, Vec<&MediaRecord>> = HashMap::new();
    for record in &records {
        if record.folder_ids.is_empty() {
            scopes.entry(0).or_default().push(record);
        } else {
            for folder_id in &record.folder_ids {
                scopes.entry(*folder_id).or_default().push(record);
            }
        }
    }
    let scopes_scanned = scopes.len();
    let mut removals = Vec::new();
    let mut duplicate_groups = 0;
    let mut title_only_groups_skipped = 0;
    let mut group_reasons = Vec::new();
    for (scope_id, scoped_records) in scopes {
        let mut claimed = HashSet::new();
        collect_duplicate_groups(
            scope_id,
            &scoped_records,
            strong_identity,
            &mut claimed,
            &mut duplicate_groups,
            &mut removals,
            &mut group_reasons,
        );
        collect_medium_duplicate_groups(
            scope_id,
            &scoped_records,
            &mut claimed,
            &mut duplicate_groups,
            &mut removals,
            &mut group_reasons,
        );
        let mut title_groups: HashMap<String, usize> = HashMap::new();
        for record in scoped_records
            .iter()
            .filter(|record| !claimed.contains(&record.id))
        {
            *title_groups
                .entry(normalize_match_text(&record.title))
                .or_default() += 1;
        }
        title_only_groups_skipped += title_groups.values().filter(|count| **count > 1).count();
    }
    Ok(DuplicatePlan {
        duplicate_groups,
        scopes_scanned,
        title_only_groups_skipped,
        group_reasons,
        removals,
    })
}

fn collect_duplicate_groups(
    scope_id: i64,
    scoped_records: &[&MediaRecord],
    identity: fn(&MediaRecord) -> Option<(String, String)>,
    claimed: &mut HashSet<i64>,
    duplicate_groups: &mut usize,
    removals: &mut Vec<DuplicateRemoval>,
    group_reasons: &mut Vec<String>,
) {
    let mut groups: HashMap<String, (String, Vec<&MediaRecord>)> = HashMap::new();
    for record in scoped_records
        .iter()
        .filter(|record| !claimed.contains(&record.id))
    {
        if let Some((key, reason)) = identity(record) {
            groups
                .entry(key)
                .or_insert((reason, Vec::new()))
                .1
                .push(record);
        }
    }
    for (_, (reason, mut duplicates)) in groups {
        if duplicates.len() < 2 {
            continue;
        }
        *duplicate_groups += 1;
        duplicates.sort_by(best_record_order);
        let kept_id = duplicates[0].id;
        for record in &duplicates {
            claimed.insert(record.id);
        }
        let scope_label = if scope_id == 0 {
            "Uncategorized".to_string()
        } else {
            format!("folder {scope_id}")
        };
        group_reasons.push(format!(
            "{scope_label}: {reason}; kept #{kept_id}; remove {}",
            duplicates
                .iter()
                .skip(1)
                .map(|record| format!("#{}", record.id))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        removals.extend(
            duplicates
                .into_iter()
                .skip(1)
                .map(|record| DuplicateRemoval {
                    scope_id,
                    media_id: record.id,
                    remove_record: scope_id == 0 || record.folder_ids.len() <= 1,
                }),
        );
    }
}

fn collect_medium_duplicate_groups(
    scope_id: i64,
    scoped_records: &[&MediaRecord],
    claimed: &mut HashSet<i64>,
    duplicate_groups: &mut usize,
    removals: &mut Vec<DuplicateRemoval>,
    group_reasons: &mut Vec<String>,
) {
    let mut groups: HashMap<String, Vec<&MediaRecord>> = HashMap::new();
    for record in scoped_records
        .iter()
        .filter(|record| !claimed.contains(&record.id))
    {
        let title = normalize_match_text(&record.title);
        if title.is_empty() {
            continue;
        }
        for label in duplicate_labels(record) {
            groups
                .entry(format!(
                    "medium:{title}:{}:{}:{label}",
                    record.source_kind, record.media_type
                ))
                .or_default()
                .push(record);
        }
    }
    for (_, mut duplicates) in groups {
        duplicates.retain(|record| !claimed.contains(&record.id));
        let mut seen = HashSet::new();
        duplicates.retain(|record| seen.insert(record.id));
        if duplicates.len() < 2 {
            continue;
        }
        *duplicate_groups += 1;
        duplicates.sort_by(best_record_order);
        let kept_id = duplicates[0].id;
        for record in &duplicates {
            claimed.insert(record.id);
        }
        let scope_label = if scope_id == 0 {
            "Uncategorized".to_string()
        } else {
            format!("folder {scope_id}")
        };
        group_reasons.push(format!(
            "{scope_label}: title + overlapping aliases/tags; kept #{kept_id}; remove {}",
            duplicates
                .iter()
                .skip(1)
                .map(|record| format!("#{}", record.id))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        removals.extend(
            duplicates
                .into_iter()
                .skip(1)
                .map(|record| DuplicateRemoval {
                    scope_id,
                    media_id: record.id,
                    remove_record: scope_id == 0 || record.folder_ids.len() <= 1,
                }),
        );
    }
}

fn best_record_order(a: &&MediaRecord, b: &&MediaRecord) -> std::cmp::Ordering {
    b.use_count
        .cmp(&a.use_count)
        .then_with(|| b.last_used_at.cmp(&a.last_used_at))
        .then_with(|| a.created_at.cmp(&b.created_at))
        .then_with(|| a.id.cmp(&b.id))
}

fn strong_identity(record: &MediaRecord) -> Option<(String, String)> {
    non_empty(&record.file_hash)
        .map(|value| (format!("hash:{value}"), "file hash".to_string()))
        .or_else(|| {
            normalized_link(record).map(|value| (format!("url:{value}"), "URL".to_string()))
        })
        .or_else(|| {
            non_empty(&record.storage_filename).map(|value| {
                (
                    format!("file:{}", normalize_match_text(value)),
                    "local path".to_string(),
                )
            })
        })
}

fn duplicate_labels(record: &MediaRecord) -> Vec<String> {
    let mut labels = record
        .alias_names
        .iter()
        .chain(record.tag_names.iter())
        .map(|value| normalize_match_text(value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    labels.sort();
    labels.dedup();
    labels
}

fn normalized_link(record: &MediaRecord) -> Option<String> {
    non_empty(&record.external_url)
        .or_else(|| non_empty(&record.remote_url))
        .or_else(|| non_empty(&record.hosted_url))
        .map(normalize_url)
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn normalize_url(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn normalize_match_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn apply_media_folder_changes_inner(
    database: &Database,
    mut media_ids: Vec<i64>,
    mut folder_changes: Vec<FolderMembershipChange>,
) -> Result<(), String> {
    media_ids.sort_unstable();
    media_ids.dedup();
    folder_changes.sort_by_key(|change| change.folder_id);
    folder_changes.dedup_by_key(|change| change.folder_id);
    if media_ids.is_empty() {
        return Err("Select at least one media item.".into());
    }
    if folder_changes.is_empty() || folder_changes.iter().any(|change| change.folder_id <= 0) {
        return Err("Change at least one normal folder membership.".into());
    }
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("failed to start bulk folder transaction: {error}"))?;
    for change in &folder_changes {
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
                [change.folder_id],
                |row| row.get(0),
            )
            .map_err(|error| format!("failed to validate destination folder: {error}"))?;
        if !exists {
            return Err("A selected destination folder no longer exists.".into());
        }
    }
    for media_id in &media_ids {
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM media WHERE id = ?1)",
                [media_id],
                |row| row.get(0),
            )
            .map_err(|error| format!("failed to validate selected media: {error}"))?;
        if !exists {
            return Err("A selected media item no longer exists.".into());
        }
        for change in &folder_changes {
            if change.assigned {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO media_folders (media_id, folder_id) VALUES (?1, ?2)",
                        params![media_id, change.folder_id],
                    )
                    .map_err(|error| format!("failed to add media to folder: {error}"))?;
            } else {
                transaction
                    .execute(
                        "DELETE FROM media_folders WHERE media_id = ?1 AND folder_id = ?2",
                        params![media_id, change.folder_id],
                    )
                    .map_err(|error| format!("failed to remove media from folder: {error}"))?;
            }
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit bulk folder changes: {error}"))?;
    Ok(())
}

fn find_by_id(connection: &rusqlite::Connection, id: i64) -> Result<Option<MediaRecord>, String> {
    let mut record = connection
        .query_row(
            &format!("{SELECT_MEDIA} WHERE id = ?1"),
            [id],
            media_from_row,
        )
        .optional()
        .map_err(|error| format!("failed to read media record: {error}"))?;
    if let Some(record) = &mut record {
        hydrate_labels(connection, record)?;
    }
    Ok(record)
}

fn sync_labels(
    connection: &rusqlite::Connection,
    media_id: i64,
    table: &str,
    join_table: &str,
    id_column: &str,
    names: &[String],
) -> Result<(), String> {
    connection
        .execute(
            &format!("DELETE FROM {join_table} WHERE media_id = ?1"),
            [media_id],
        )
        .map_err(|error| format!("failed to update labels: {error}"))?;
    for name in names {
        connection
            .execute(
                &format!("INSERT OR IGNORE INTO {table} (name) VALUES (?1)"),
                [name],
            )
            .map_err(|error| format!("failed to create label: {error}"))?;
        connection.execute(
            &format!("INSERT INTO {join_table} (media_id, {id_column}) SELECT ?1, id FROM {table} WHERE name = ?2 COLLATE NOCASE"),
            params![media_id, name],
        ).map_err(|error| format!("failed to attach label: {error}"))?;
    }
    Ok(())
}

fn hydrate_labels(
    connection: &rusqlite::Connection,
    record: &mut MediaRecord,
) -> Result<(), String> {
    let folders = load_folders(connection, record.id)?;
    record.folder_ids = folders.iter().map(|(id, _)| *id).collect();
    record.folder_names = folders.into_iter().map(|(_, path)| path).collect();
    record.tag_names = load_labels(connection, record.id, "tags", "media_tags", "tag_id")?;
    record.alias_names = load_aliases(connection, record.id)?;
    Ok(())
}

fn sync_folders(
    connection: &rusqlite::Connection,
    media_id: i64,
    folder_ids: &[i64],
    legacy_names: &[String],
) -> Result<(), String> {
    connection
        .execute("DELETE FROM media_folders WHERE media_id = ?1", [media_id])
        .map_err(|error| format!("failed to update folders: {error}"))?;
    for folder_id in folder_ids {
        let changed = connection.execute(
            "INSERT INTO media_folders (media_id, folder_id) SELECT ?1, id FROM folders WHERE id = ?2",
            params![media_id, folder_id],
        ).map_err(|error| format!("failed to attach folder: {error}"))?;
        if changed == 0 {
            return Err(format!("folder {folder_id} not found"));
        }
    }
    for name in legacy_names {
        connection
            .execute(
                "INSERT OR IGNORE INTO folders (name, parent_id) VALUES (?1, NULL)",
                [name],
            )
            .map_err(|error| format!("failed to create folder: {error}"))?;
        connection.execute(
            "INSERT OR IGNORE INTO media_folders (media_id, folder_id) SELECT ?1, id FROM folders WHERE name = ?2 COLLATE NOCASE AND parent_id IS NULL",
            params![media_id, name],
        ).map_err(|error| format!("failed to attach folder: {error}"))?;
    }
    Ok(())
}

fn sync_aliases(
    connection: &rusqlite::Connection,
    media_id: i64,
    names: &[String],
) -> Result<(), String> {
    connection
        .execute("DELETE FROM aliases WHERE media_id = ?1", [media_id])
        .map_err(|error| format!("failed to update aliases: {error}"))?;
    for name in names {
        connection
            .execute(
                "INSERT INTO aliases (media_id, name) VALUES (?1, ?2)",
                params![media_id, name],
            )
            .map_err(|error| format!("failed to attach alias: {error}"))?;
    }
    Ok(())
}

fn load_folders(
    connection: &rusqlite::Connection,
    media_id: i64,
) -> Result<Vec<(i64, String)>, String> {
    let mut statement = connection
        .prepare(
            "WITH RECURSIVE tree(id, name, parent_id, path) AS (
            SELECT id, name, parent_id, name FROM folders WHERE parent_id IS NULL
            UNION ALL SELECT child.id, child.name, child.parent_id, tree.path || ' / ' || child.name
            FROM folders child JOIN tree ON child.parent_id = tree.id
        ) SELECT tree.id, tree.path FROM tree JOIN media_folders link ON link.folder_id = tree.id
          WHERE link.media_id = ?1 ORDER BY tree.path COLLATE NOCASE",
        )
        .map_err(|error| format!("failed to prepare folders: {error}"))?;
    let values = statement
        .query_map([media_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|error| format!("failed to read folders: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read folder: {error}"))?;
    Ok(values)
}

fn load_aliases(connection: &rusqlite::Connection, media_id: i64) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare("SELECT name FROM aliases WHERE media_id = ?1 ORDER BY name COLLATE NOCASE")
        .map_err(|error| format!("failed to prepare aliases: {error}"))?;
    let values = statement
        .query_map([media_id], |row| row.get(0))
        .map_err(|error| format!("failed to read aliases: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read alias: {error}"))?;
    Ok(values)
}

fn load_labels(
    connection: &rusqlite::Connection,
    media_id: i64,
    table: &str,
    join_table: &str,
    id_column: &str,
) -> Result<Vec<String>, String> {
    let sql = format!("SELECT label.name FROM {table} label JOIN {join_table} link ON link.{id_column} = label.id WHERE link.media_id = ?1 ORDER BY label.name COLLATE NOCASE");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("failed to prepare labels: {error}"))?;
    let labels = statement
        .query_map([media_id], |row| row.get(0))
        .map_err(|error| format!("failed to read labels: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read label: {error}"))?;
    Ok(labels)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_media_folder_changes_inner, create_in_transaction, duplicate_membership_removals,
        normalize, FolderMembershipChange, MediaInput,
    };
    use crate::database::{run_migrations, Database};
    use rusqlite::{params, Connection};
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn remote_input() -> MediaInput {
        MediaInput {
            title: "Test".into(),
            source_kind: "remote_url".into(),
            remote_url: Some("https://example.com/test.gif".into()),
            external_url: None,
            storage_filename: None,
            original_filename: None,
            media_type: "gif".into(),
            file_hash: None,
            notes: String::new(),
            folder_names: Vec::new(),
            folder_ids: Vec::new(),
            tag_names: Vec::new(),
            alias_names: Vec::new(),
        }
    }

    fn insert_remote(connection: &Connection, title: &str, url: &str, use_count: i64) -> i64 {
        connection.execute(
            "INSERT INTO media (title, source_kind, remote_url, external_url, media_type, use_count) VALUES (?1, 'remote_url', ?2, ?2, 'gif', ?3)",
            params![title, url, use_count],
        ).unwrap();
        connection.last_insert_rowid()
    }

    fn attach_folder(connection: &Connection, media_id: i64, folder_id: i64) {
        connection
            .execute(
                "INSERT INTO media_folders (media_id, folder_id) VALUES (?1, ?2)",
                params![media_id, folder_id],
            )
            .unwrap();
    }

    fn attach_alias(connection: &Connection, media_id: i64, name: &str) {
        connection
            .execute(
                "INSERT INTO aliases (media_id, name) VALUES (?1, ?2)",
                params![media_id, name],
            )
            .unwrap();
    }

    #[test]
    fn rejects_video_formats() {
        let mut input = remote_input();
        input.media_type = "mp4".into();
        let result = normalize(input);
        assert!(result.is_err());
    }

    #[test]
    fn accepts_local_file_with_external_link() {
        let input = MediaInput {
            title: "Combined".into(),
            source_kind: "local_file".into(),
            remote_url: None,
            external_url: Some("https://example.com/reaction.gif".into()),
            storage_filename: Some("managed.gif".into()),
            original_filename: Some("reaction.gif".into()),
            media_type: "gif".into(),
            file_hash: Some("hash".into()),
            notes: String::new(),
            folder_names: Vec::new(),
            folder_ids: Vec::new(),
            tag_names: Vec::new(),
            alias_names: Vec::new(),
        };
        let normalized = normalize(input).unwrap();
        assert_eq!(normalized.source_kind, "local_file");
        assert_eq!(normalized.storage_filename.as_deref(), Some("managed.gif"));
        assert_eq!(
            normalized.external_url.as_deref(),
            Some("https://example.com/reaction.gif")
        );
    }

    #[test]
    fn failed_relationship_write_rolls_back_media_row() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&mut connection).unwrap();
        let transaction = connection.transaction().unwrap();
        let mut input = remote_input();
        input.folder_ids = vec![999];
        assert!(create_in_transaction(&transaction, input, None).is_err());
        drop(transaction);
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM media", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn media_can_commit_without_thumbnail() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&mut connection).unwrap();
        let transaction = connection.transaction().unwrap();
        let record = create_in_transaction(&transaction, remote_input(), None).unwrap();
        transaction.commit().unwrap();
        let thumbnail: Option<String> = connection
            .query_row(
                "SELECT thumbnail_filename FROM media WHERE id = ?1",
                [record.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(thumbnail, None);
    }

    #[test]
    fn bulk_folder_assignment_moves_uncategorized_media() {
        let token = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("gif-library-bulk-folders-{token}"));
        let database = Database::initialize(&root).unwrap();
        {
            let connection = database.connection.lock().unwrap();
            connection
                .execute("INSERT INTO folders (name) VALUES ('A'), ('B')", [])
                .unwrap();
            connection.execute("INSERT INTO media (title, source_kind, remote_url, external_url, media_type) VALUES ('One', 'remote_url', 'https://example.com/1.gif', 'https://example.com/1.gif', 'gif'), ('Two', 'remote_url', 'https://example.com/2.gif', 'https://example.com/2.gif', 'gif')", []).unwrap();
        }
        apply_media_folder_changes_inner(
            &database,
            vec![1, 2],
            vec![
                FolderMembershipChange {
                    folder_id: 1,
                    assigned: true,
                },
                FolderMembershipChange {
                    folder_id: 2,
                    assigned: true,
                },
            ],
        )
        .unwrap();
        let connection = database.connection.lock().unwrap();
        let memberships: i64 = connection
            .query_row("SELECT COUNT(*) FROM media_folders", [], |row| row.get(0))
            .unwrap();
        let uncategorized: i64 = connection.query_row("SELECT COUNT(*) FROM media WHERE NOT EXISTS (SELECT 1 FROM media_folders WHERE media_id = media.id)", [], |row| row.get(0)).unwrap();
        assert_eq!(memberships, 4);
        assert_eq!(uncategorized, 0);
        drop(connection);
        apply_media_folder_changes_inner(
            &database,
            vec![1],
            vec![
                FolderMembershipChange {
                    folder_id: 1,
                    assigned: false,
                },
                FolderMembershipChange {
                    folder_id: 2,
                    assigned: false,
                },
            ],
        )
        .unwrap();
        let connection = database.connection.lock().unwrap();
        let uncategorized: i64 = connection.query_row("SELECT COUNT(*) FROM media WHERE NOT EXISTS (SELECT 1 FROM media_folders WHERE media_id = media.id)", [], |row| row.get(0)).unwrap();
        assert_eq!(uncategorized, 1);
        drop(connection);
        drop(database);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicate_cleanup_detects_same_normal_folder_and_keeps_most_used() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&mut connection).unwrap();
        connection
            .execute("INSERT INTO folders (name) VALUES ('A'), ('B')", [])
            .unwrap();
        connection.execute("INSERT INTO media (title, source_kind, storage_filename, media_type, file_hash, use_count) VALUES ('Low', 'local_file', 'low.gif', 'gif', 'same-hash', 1), ('High', 'local_file', 'high.gif', 'gif', 'same-hash', 5)", []).unwrap();
        connection
            .execute(
                "INSERT INTO media_folders (media_id, folder_id) VALUES (1, 1), (1, 2), (2, 1)",
                [],
            )
            .unwrap();

        let plan = duplicate_membership_removals(&connection).unwrap();
        assert_eq!(plan.duplicate_groups, 1);
        assert_eq!(plan.removals.len(), 1);
        assert_eq!(plan.removals[0].scope_id, 1);
        assert_eq!(plan.removals[0].media_id, 1);
        assert!(!plan.removals[0].remove_record);
    }

    #[test]
    fn duplicate_cleanup_does_not_cross_normal_folders() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&mut connection).unwrap();
        connection
            .execute("INSERT INTO folders (name) VALUES ('A'), ('B')", [])
            .unwrap();
        let one = insert_remote(&connection, "Same", "https://example.com/same.gif", 1);
        let two = insert_remote(&connection, "Same", "https://example.com/same.gif", 2);
        attach_folder(&connection, one, 1);
        attach_folder(&connection, two, 2);

        let plan = duplicate_membership_removals(&connection).unwrap();
        assert_eq!(plan.duplicate_groups, 0);
        assert!(plan.removals.is_empty());
    }

    #[test]
    fn duplicate_cleanup_detects_uncategorized_scope() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&mut connection).unwrap();
        let one = insert_remote(&connection, "Same", "https://example.com/same.gif", 1);
        let two = insert_remote(&connection, "Same", "https://example.com/same.gif", 2);

        let plan = duplicate_membership_removals(&connection).unwrap();
        assert_eq!(plan.duplicate_groups, 1);
        assert_eq!(plan.removals.len(), 1);
        assert_eq!(plan.removals[0].scope_id, 0);
        assert_eq!(plan.removals[0].media_id, one);
        assert!(plan.removals[0].remove_record);
        assert_ne!(plan.removals[0].media_id, two);
    }

    #[test]
    fn duplicate_cleanup_does_not_cross_uncategorized_and_folder() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&mut connection).unwrap();
        connection
            .execute("INSERT INTO folders (name) VALUES ('A')", [])
            .unwrap();
        let uncategorized = insert_remote(&connection, "Same", "https://example.com/same.gif", 1);
        let foldered = insert_remote(&connection, "Same", "https://example.com/same.gif", 2);
        attach_folder(&connection, foldered, 1);

        let plan = duplicate_membership_removals(&connection).unwrap();
        assert_eq!(plan.duplicate_groups, 0);
        assert!(plan.removals.is_empty());
        assert_ne!(uncategorized, foldered);
    }

    #[test]
    fn duplicate_cleanup_does_not_cross_parent_child_folders() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&mut connection).unwrap();
        connection
            .execute(
                "INSERT INTO folders (id, name, parent_id) VALUES (1, 'Parent', NULL), (2, 'Child', 1)",
                [],
            )
            .unwrap();
        let parent = insert_remote(&connection, "Same", "https://example.com/same.gif", 1);
        let child = insert_remote(&connection, "Same", "https://example.com/same.gif", 2);
        attach_folder(&connection, parent, 1);
        attach_folder(&connection, child, 2);

        let plan = duplicate_membership_removals(&connection).unwrap();
        assert_eq!(plan.duplicate_groups, 0);
        assert!(plan.removals.is_empty());
    }

    #[test]
    fn duplicate_cleanup_uses_medium_metadata_but_skips_title_only() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&mut connection).unwrap();
        connection
            .execute("INSERT INTO folders (name) VALUES ('A')", [])
            .unwrap();
        let low = insert_remote(
            &connection,
            "Shared Title",
            "https://example.com/one.gif",
            1,
        );
        let high = insert_remote(
            &connection,
            "shared   title",
            "https://example.com/two.gif",
            2,
        );
        let title_only = insert_remote(
            &connection,
            "Shared Title",
            "https://example.com/three.gif",
            0,
        );
        attach_folder(&connection, low, 1);
        attach_folder(&connection, high, 1);
        attach_folder(&connection, title_only, 1);
        attach_alias(&connection, low, "sparxie");
        attach_alias(&connection, high, "Sparxie");

        let plan = duplicate_membership_removals(&connection).unwrap();
        assert_eq!(plan.duplicate_groups, 1);
        assert_eq!(plan.title_only_groups_skipped, 0);
        assert_eq!(plan.removals.len(), 1);
        assert_eq!(plan.removals[0].media_id, low);
    }

    #[test]
    fn duplicate_cleanup_skips_weak_title_only_groups() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&mut connection).unwrap();
        connection
            .execute("INSERT INTO folders (name) VALUES ('A')", [])
            .unwrap();
        let one = insert_remote(&connection, "Same Title", "https://example.com/one.gif", 1);
        let two = insert_remote(&connection, "same title", "https://example.com/two.gif", 2);
        attach_folder(&connection, one, 1);
        attach_folder(&connection, two, 1);

        let plan = duplicate_membership_removals(&connection).unwrap();
        assert_eq!(plan.duplicate_groups, 0);
        assert_eq!(plan.title_only_groups_skipped, 1);
        assert!(plan.removals.is_empty());
    }
}
