use crate::database::Database;
use rusqlite::OptionalExtension;
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderRecord {
    id: i64,
    name: String,
    parent_id: Option<i64>,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderDeleteImpact {
    child_folder_count: i64,
    media_count: i64,
}

#[tauri::command]
pub fn list_folders(database: State<'_, Database>) -> Result<Vec<FolderRecord>, String> {
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let mut statement = connection
        .prepare(
            "WITH RECURSIVE tree(id, name, parent_id, path) AS (
            SELECT id, name, parent_id, name FROM folders WHERE parent_id IS NULL
            UNION ALL
            SELECT child.id, child.name, child.parent_id, tree.path || ' / ' || child.name
            FROM folders child JOIN tree ON child.parent_id = tree.id
        ) SELECT id, name, parent_id, path FROM tree ORDER BY path COLLATE NOCASE",
        )
        .map_err(|error| format!("failed to prepare folders: {error}"))?;
    let folders = statement
        .query_map([], |row| {
            Ok(FolderRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                parent_id: row.get(2)?,
                path: row.get(3)?,
            })
        })
        .map_err(|error| format!("failed to list folders: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read folder: {error}"))?;
    Ok(folders)
}

#[tauri::command]
pub fn create_folder(
    database: State<'_, Database>,
    name: String,
    parent_id: Option<i64>,
) -> Result<FolderRecord, String> {
    let base = name.trim();
    if base.is_empty() {
        return Err("folder name cannot be empty".into());
    }
    if base.len() > 80 {
        return Err("folder name cannot exceed 80 characters".into());
    }
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    if let Some(parent_id) = parent_id {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
                [parent_id],
                |row| row.get(0),
            )
            .map_err(|error| format!("failed to check parent folder: {error}"))?;
        if !exists {
            return Err("parent folder not found".into());
        }
    }
    let mut candidate = base.to_owned();
    let mut suffix = 1;
    loop {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM folders WHERE name = ?1 COLLATE NOCASE AND parent_id IS ?2)",
            rusqlite::params![candidate, parent_id], |row| row.get(0),
        ).map_err(|error| format!("failed to check folder name: {error}"))?;
        if !exists {
            break;
        }
        candidate = format!("{base}({suffix})");
        suffix += 1;
    }
    connection
        .execute(
            "INSERT INTO folders (name, parent_id) VALUES (?1, ?2)",
            rusqlite::params![candidate, parent_id],
        )
        .map_err(|error| format!("failed to create folder: {error}"))?;
    let id = connection.last_insert_rowid();
    let path: String = connection.query_row(
        "WITH RECURSIVE ancestors(id, name, parent_id, path) AS (
            SELECT id, name, parent_id, name FROM folders WHERE id = ?1
            UNION ALL SELECT parent.id, parent.name, parent.parent_id, parent.name || ' / ' || ancestors.path
            FROM folders parent JOIN ancestors ON ancestors.parent_id = parent.id
        ) SELECT path FROM ancestors WHERE parent_id IS NULL", [id], |row| row.get(0),
    ).optional().map_err(|error| format!("failed to read folder path: {error}"))?.ok_or_else(|| "folder path not found".to_string())?;
    Ok(FolderRecord {
        id,
        name: candidate,
        parent_id,
        path,
    })
}

#[tauri::command]
pub fn rename_folder(
    database: State<'_, Database>,
    id: i64,
    name: String,
) -> Result<FolderRecord, String> {
    let base = validate_name(&name)?;
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("failed to start folder transaction: {error}"))?;
    let parent_id: Option<i64> = transaction
        .query_row("SELECT parent_id FROM folders WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|error| format!("failed to find folder: {error}"))?
        .ok_or_else(|| "folder not found".to_string())?;
    let candidate = available_name(&transaction, &base, parent_id, Some(id))?;
    transaction
        .execute(
            "UPDATE folders SET name = ?1 WHERE id = ?2",
            rusqlite::params![candidate, id],
        )
        .map_err(|error| format!("failed to rename folder: {error}"))?;
    let record = read_folder(&transaction, id)?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit folder rename: {error}"))?;
    Ok(record)
}

#[tauri::command]
pub fn folder_delete_impact(
    database: State<'_, Database>,
    id: i64,
) -> Result<FolderDeleteImpact, String> {
    let connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
            [id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to find folder: {error}"))?;
    if !exists {
        return Err("folder not found".into());
    }
    let child_folder_count = connection.query_row(
        "WITH RECURSIVE descendants(id) AS (SELECT id FROM folders WHERE parent_id = ?1 UNION ALL SELECT child.id FROM folders child JOIN descendants ON child.parent_id = descendants.id) SELECT COUNT(*) FROM descendants",
        [id], |row| row.get(0),
    ).map_err(|error| format!("failed to count child folders: {error}"))?;
    let media_count = connection.query_row(
        "WITH RECURSIVE affected(id) AS (SELECT ?1 UNION ALL SELECT child.id FROM folders child JOIN affected ON child.parent_id = affected.id) SELECT COUNT(DISTINCT media_id) FROM media_folders WHERE folder_id IN affected",
        [id], |row| row.get(0),
    ).map_err(|error| format!("failed to count folder media: {error}"))?;
    Ok(FolderDeleteImpact {
        child_folder_count,
        media_count,
    })
}

#[tauri::command]
pub fn delete_folder(database: State<'_, Database>, id: i64, mode: String) -> Result<(), String> {
    if !matches!(mode.as_str(), "delete_subtree" | "reparent") {
        return Err("invalid folder deletion mode".into());
    }
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("failed to start folder transaction: {error}"))?;
    let parent_id: Option<i64> = transaction
        .query_row("SELECT parent_id FROM folders WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|error| format!("failed to find folder: {error}"))?
        .ok_or_else(|| "folder not found".to_string())?;
    if mode == "reparent" {
        if let Some(parent_id) = parent_id {
            transaction.execute(
                "INSERT OR IGNORE INTO media_folders (media_id, folder_id) SELECT media_id, ?1 FROM media_folders WHERE folder_id = ?2",
                rusqlite::params![parent_id, id],
            ).map_err(|error| format!("failed to move folder media: {error}"))?;
        }
        let children: Vec<(i64, String)> = {
            let mut statement = transaction
                .prepare("SELECT id, name FROM folders WHERE parent_id = ?1 ORDER BY id")
                .map_err(|error| format!("failed to read child folders: {error}"))?;
            let children = statement
                .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|error| format!("failed to read child folders: {error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("failed to read child folder: {error}"))?;
            children
        };
        for (child_id, child_name) in children {
            let candidate = available_name(&transaction, &child_name, parent_id, Some(child_id))?;
            transaction
                .execute(
                    "UPDATE folders SET name = ?1, parent_id = ?2 WHERE id = ?3",
                    rusqlite::params![candidate, parent_id, child_id],
                )
                .map_err(|error| format!("failed to move child folder: {error}"))?;
        }
    }
    transaction
        .execute("DELETE FROM folders WHERE id = ?1", [id])
        .map_err(|error| format!("failed to delete folder: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit folder deletion: {error}"))?;
    Ok(())
}

fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("folder name cannot be empty".into());
    }
    if name.len() > 80 {
        return Err("folder name cannot exceed 80 characters".into());
    }
    Ok(name.to_owned())
}

fn available_name(
    connection: &rusqlite::Connection,
    base: &str,
    parent_id: Option<i64>,
    excluded_id: Option<i64>,
) -> Result<String, String> {
    let mut candidate = base.to_owned();
    let mut suffix = 1;
    loop {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM folders WHERE name = ?1 COLLATE NOCASE AND parent_id IS ?2 AND id != COALESCE(?3, -1))",
            rusqlite::params![candidate, parent_id, excluded_id], |row| row.get(0),
        ).map_err(|error| format!("failed to check folder name: {error}"))?;
        if !exists {
            return Ok(candidate);
        }
        candidate = format!("{base}({suffix})");
        suffix += 1;
    }
}

fn read_folder(connection: &rusqlite::Connection, id: i64) -> Result<FolderRecord, String> {
    connection.query_row(
        "WITH RECURSIVE ancestors(id, name, parent_id, path) AS (SELECT id, name, parent_id, name FROM folders WHERE id = ?1 UNION ALL SELECT parent.id, parent.name, parent.parent_id, parent.name || ' / ' || ancestors.path FROM folders parent JOIN ancestors ON ancestors.parent_id = parent.id) SELECT ?1, (SELECT name FROM folders WHERE id = ?1), (SELECT parent_id FROM folders WHERE id = ?1), path FROM ancestors WHERE parent_id IS NULL",
        [id], |row| Ok(FolderRecord { id: row.get(0)?, name: row.get(1)?, parent_id: row.get(2)?, path: row.get(3)? }),
    ).map_err(|error| format!("failed to read folder: {error}"))
}
