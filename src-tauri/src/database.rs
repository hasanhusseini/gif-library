use rusqlite::Connection;
use std::{fs, path::Path, sync::Mutex};

const MIGRATION_1: &str = r#"
CREATE TABLE media (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    title            TEXT NOT NULL,
    source_kind      TEXT NOT NULL CHECK (source_kind IN ('remote_url', 'local_file')),
    remote_url       TEXT,
    storage_filename TEXT,
    original_filename TEXT,
    media_type       TEXT NOT NULL CHECK (media_type IN ('gif', 'png', 'jpg', 'jpeg', 'webp')),
    file_hash        TEXT,
    notes            TEXT NOT NULL DEFAULT '',
    created_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (
        (source_kind = 'remote_url' AND remote_url IS NOT NULL AND storage_filename IS NULL)
        OR
        (source_kind = 'local_file' AND remote_url IS NULL AND storage_filename IS NOT NULL)
    )
);

CREATE INDEX media_created_at_idx ON media(created_at DESC);
CREATE INDEX media_file_hash_idx ON media(file_hash) WHERE file_hash IS NOT NULL;
"#;

const MIGRATION_2: &str = r#"
ALTER TABLE media ADD COLUMN thumbnail_filename TEXT;
CREATE TABLE folders (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL COLLATE NOCASE UNIQUE);
CREATE TABLE tags (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL COLLATE NOCASE UNIQUE);
CREATE TABLE media_folders (
    media_id INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    PRIMARY KEY (media_id, folder_id)
);
CREATE TABLE media_tags (
    media_id INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (media_id, tag_id)
);
"#;

const MIGRATION_3: &str = r#"
CREATE TABLE folders_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL COLLATE NOCASE,
    parent_id INTEGER REFERENCES folders_new(id) ON DELETE CASCADE
);
INSERT INTO folders_new (id, name, parent_id) SELECT id, name, NULL FROM folders;

CREATE TABLE media_folders_new (
    media_id INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    folder_id INTEGER NOT NULL REFERENCES folders_new(id) ON DELETE CASCADE,
    PRIMARY KEY (media_id, folder_id)
);
INSERT INTO media_folders_new SELECT media_id, folder_id FROM media_folders;
DROP TABLE media_folders;
DROP TABLE folders;
ALTER TABLE folders_new RENAME TO folders;
ALTER TABLE media_folders_new RENAME TO media_folders;
CREATE UNIQUE INDEX folders_sibling_name_idx ON folders(IFNULL(parent_id, -1), name COLLATE NOCASE);

CREATE TABLE aliases (
    media_id INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    name TEXT NOT NULL COLLATE NOCASE,
    PRIMARY KEY (media_id, name)
);
CREATE INDEX aliases_name_idx ON aliases(name COLLATE NOCASE);
"#;

const MIGRATION_4: &str = r#"
ALTER TABLE media ADD COLUMN external_url TEXT;
"#;

const MIGRATION_5: &str = r#"
ALTER TABLE media ADD COLUMN hosted_url TEXT;
ALTER TABLE media ADD COLUMN hosted_object_key TEXT;
ALTER TABLE media ADD COLUMN hosted_at TEXT;
"#;

const MIGRATION_6: &str = r#"
ALTER TABLE media ADD COLUMN use_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE media ADD COLUMN last_used_at TEXT;
CREATE INDEX media_last_used_at_idx ON media(last_used_at DESC);
"#;

const MIGRATION_7: &str = r#"
CREATE TABLE app_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    show_focus_keybind TEXT,
    toggle_visibility_keybind TEXT
);
INSERT INTO app_settings (id, show_focus_keybind, toggle_visibility_keybind) VALUES (1, NULL, NULL);
"#;

const MIGRATION_8: &str = r#"
ALTER TABLE app_settings ADD COLUMN export_directory TEXT;
"#;

pub struct Database {
    pub connection: Mutex<Connection>,
    pub media_dir: std::path::PathBuf,
}

impl Database {
    pub fn initialize(app_data_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(app_data_dir)
            .map_err(|error| format!("failed to create app data directory: {error}"))?;
        let media_dir = app_data_dir.join("media");
        fs::create_dir_all(&media_dir)
            .map_err(|error| format!("failed to create managed media directory: {error}"))?;

        let mut connection = Connection::open(app_data_dir.join("library.sqlite3"))
            .map_err(|error| format!("failed to open database: {error}"))?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|error| format!("failed to enable foreign keys: {error}"))?;

        run_migrations(&mut connection)?;

        Ok(Self {
            connection: Mutex::new(connection),
            media_dir,
        })
    }
}

pub(crate) fn run_migrations(connection: &mut Connection) -> Result<(), String> {
    let current_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| format!("failed to read schema version: {error}"))?;

    if current_version < 1 {
        let transaction = connection
            .transaction()
            .map_err(|error| format!("failed to start migration: {error}"))?;
        transaction
            .execute_batch(MIGRATION_1)
            .map_err(|error| format!("migration 1 failed: {error}"))?;
        transaction
            .pragma_update(None, "user_version", 1)
            .map_err(|error| format!("failed to record schema version: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("failed to commit migration: {error}"))?;
    }

    let current_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| format!("failed to read schema version: {error}"))?;
    if current_version < 2 {
        let transaction = connection
            .transaction()
            .map_err(|error| format!("failed to start migration: {error}"))?;
        transaction
            .execute_batch(MIGRATION_2)
            .map_err(|error| format!("migration 2 failed: {error}"))?;
        transaction
            .pragma_update(None, "user_version", 2)
            .map_err(|error| format!("failed to record schema version: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("failed to commit migration: {error}"))?;
    }

    let current_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| format!("failed to read schema version: {error}"))?;
    if current_version < 3 {
        let transaction = connection
            .transaction()
            .map_err(|error| format!("failed to start migration: {error}"))?;
        transaction
            .execute_batch(MIGRATION_3)
            .map_err(|error| format!("migration 3 failed: {error}"))?;
        transaction
            .pragma_update(None, "user_version", 3)
            .map_err(|error| format!("failed to record schema version: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("failed to commit migration: {error}"))?;
    }

    let current_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| format!("failed to read schema version: {error}"))?;
    if current_version < 4 {
        let transaction = connection
            .transaction()
            .map_err(|error| format!("failed to start migration: {error}"))?;
        transaction
            .execute_batch(MIGRATION_4)
            .map_err(|error| format!("migration 4 failed: {error}"))?;
        transaction
            .pragma_update(None, "user_version", 4)
            .map_err(|error| format!("failed to record schema version: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("failed to commit migration: {error}"))?;
    }

    let current_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| format!("failed to read schema version: {error}"))?;
    if current_version < 5 {
        let transaction = connection
            .transaction()
            .map_err(|error| format!("failed to start migration: {error}"))?;
        transaction
            .execute_batch(MIGRATION_5)
            .map_err(|error| format!("migration 5 failed: {error}"))?;
        transaction
            .pragma_update(None, "user_version", 5)
            .map_err(|error| format!("failed to record schema version: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("failed to commit migration: {error}"))?;
    }

    let current_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| format!("failed to read schema version: {error}"))?;
    if current_version < 6 {
        let transaction = connection
            .transaction()
            .map_err(|error| format!("failed to start migration: {error}"))?;
        transaction
            .execute_batch(MIGRATION_6)
            .map_err(|error| format!("migration 6 failed: {error}"))?;
        transaction
            .pragma_update(None, "user_version", 6)
            .map_err(|error| format!("failed to record schema version: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("failed to commit migration: {error}"))?;
    }

    let current_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| format!("failed to read schema version: {error}"))?;
    if current_version < 7 {
        let transaction = connection
            .transaction()
            .map_err(|error| format!("failed to start migration: {error}"))?;
        transaction
            .execute_batch(MIGRATION_7)
            .map_err(|error| format!("migration 7 failed: {error}"))?;
        transaction
            .pragma_update(None, "user_version", 7)
            .map_err(|error| format!("failed to record schema version: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("failed to commit migration: {error}"))?;
    }

    let current_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| format!("failed to read schema version: {error}"))?;
    if current_version < 8 {
        let transaction = connection
            .transaction()
            .map_err(|error| format!("failed to start migration: {error}"))?;
        transaction
            .execute_batch(MIGRATION_8)
            .map_err(|error| format!("migration 8 failed: {error}"))?;
        transaction
            .pragma_update(None, "user_version", 8)
            .map_err(|error| format!("failed to record schema version: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("failed to commit migration: {error}"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{run_migrations, MIGRATION_1, MIGRATION_2};
    use rusqlite::Connection;

    #[test]
    fn migration_creates_versioned_media_table() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_migrations(&mut connection).unwrap();
        run_migrations(&mut connection).unwrap();

        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'media'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(version, 8);
        let settings: (Option<String>, Option<String>) = connection
            .query_row(
                "SELECT show_focus_keybind, toggle_visibility_keybind FROM app_settings WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(settings, (None, None));
        assert_eq!(table_count, 1);
    }

    #[test]
    fn migration_three_preserves_existing_folder_membership() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(MIGRATION_1).unwrap();
        connection.execute_batch(MIGRATION_2).unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
        connection
            .execute("INSERT INTO folders (name) VALUES ('Memes')", [])
            .unwrap();
        connection.execute(
            "INSERT INTO media (title, source_kind, remote_url, media_type) VALUES ('Test', 'remote_url', 'https://example.com/a.gif', 'gif')",
            [],
        ).unwrap();
        connection
            .execute("INSERT INTO media_folders VALUES (1, 1)", [])
            .unwrap();

        run_migrations(&mut connection).unwrap();
        let membership: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM media_folders WHERE media_id = 1 AND folder_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(membership, 1);
    }
}
