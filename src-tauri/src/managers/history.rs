use anyhow::{anyhow, Result};
use chrono::{DateTime, Local, Utc};
use log::{debug, error, info};
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::fs;
use std::path::PathBuf;
use tauri::AppHandle;
use tauri_specta::Event;

/// Database migrations for transcription history.
/// Each migration is applied in order. The library tracks which migrations
/// have been applied using SQLite's user_version pragma.
///
/// Note: For users upgrading from tauri-plugin-sql, migrate_from_tauri_plugin_sql()
/// converts the old _sqlx_migrations table tracking to the user_version pragma,
/// ensuring migrations don't re-run on existing databases.
static MIGRATIONS: &[M] = &[
    M::up(
        "CREATE TABLE IF NOT EXISTS transcription_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_name TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            saved BOOLEAN NOT NULL DEFAULT 0,
            title TEXT NOT NULL,
            transcription_text TEXT NOT NULL
        );",
    ),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_processed_text TEXT;"),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_process_prompt TEXT;"),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_process_requested BOOLEAN NOT NULL DEFAULT 0;"),
    M::up("ALTER TABLE transcription_history ADD COLUMN source TEXT NOT NULL DEFAULT 'dictation';"),
    M::up("ALTER TABLE transcription_history ADD COLUMN ai_notes TEXT;"),
    M::up("ALTER TABLE transcription_history ADD COLUMN user_notes TEXT;"),
    M::up(
        "CREATE TABLE IF NOT EXISTS todos (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            done BOOLEAN NOT NULL DEFAULT 0,
            source_entry_id INTEGER
        );",
    ),
    // Appended AFTER the todos migration (already applied on user DBs —
    // list order is the migration identity). Backfill marks every pre-existing meeting as organized: their action
    // items were either extracted under the old build or predate the feature,
    // and re-generating notes must never re-create the same events/todos.
    M::up(
        "ALTER TABLE transcription_history ADD COLUMN action_items_organized BOOLEAN NOT NULL DEFAULT 0;
         UPDATE transcription_history SET action_items_organized = 1 WHERE source = 'meeting';",
    ),
    M::up(
        "CREATE TABLE IF NOT EXISTS ask_sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            query TEXT NOT NULL,
            answer TEXT,
            created_at INTEGER NOT NULL,
            provider_id TEXT
        );",
    ),
    // Notes the user jotted live during the meeting; kept verbatim as an
    // anchor for note generation. Internal (not on HistoryEntry).
    M::up("ALTER TABLE transcription_history ADD COLUMN live_notes TEXT;"),
    // Google Doc id this entry was synced to via Composio (None = never synced).
    M::up("ALTER TABLE transcription_history ADD COLUMN gdoc_id TEXT;"),
    // One-time flag for the "I'll email you that" Gmail-draft pass. Backfill
    // mirrors the action-items migration: meetings that predate the feature
    // must never suddenly draft emails on a regenerate.
    M::up(
        "ALTER TABLE transcription_history ADD COLUMN email_drafts_organized BOOLEAN NOT NULL DEFAULT 0;
         UPDATE transcription_history SET email_drafts_organized = 1 WHERE source = 'meeting';",
    ),
];

/// Where a history entry came from. Stored as TEXT so future sources
/// (e.g. imported audio files) don't need schema changes.
pub const SOURCE_DICTATION: &str = "dictation";
pub const SOURCE_MEETING: &str = "meeting";

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct PaginatedHistory {
    pub entries: Vec<HistoryEntry>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(tag = "action")]
pub enum HistoryUpdatePayload {
    #[serde(rename = "added")]
    Added { entry: HistoryEntry },
    #[serde(rename = "updated")]
    Updated { entry: HistoryEntry },
    #[serde(rename = "deleted")]
    Deleted { id: i64 },
    #[serde(rename = "toggled")]
    Toggled { id: i64 },
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct HistoryEntry {
    pub id: i64,
    pub file_name: String,
    pub timestamp: i64,
    pub saved: bool,
    pub title: String,
    pub transcription_text: String,
    pub post_processed_text: Option<String>,
    pub post_process_prompt: Option<String>,
    pub post_process_requested: bool,
    pub source: String,
    /// AI-generated meeting notes (None until generated).
    pub ai_notes: Option<String>,
    /// The user's own notes for this entry.
    pub user_notes: Option<String>,
    /// Google Doc this entry was synced to (None = never synced).
    pub gdoc_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct Todo {
    pub id: i64,
    pub title: String,
    pub created_at: i64,
    pub done: bool,
    /// Meeting this todo was extracted from (None for manually added ones).
    pub source_entry_id: Option<i64>,
}

/// A saved "ask your notes" session: one query, its (eventually) generated
/// answer, listed in the shell sidebar like a chat thread.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct AskSession {
    pub id: i64,
    pub query: String,
    pub answer: Option<String>,
    pub created_at: i64,
    pub provider_id: Option<String>,
}

/// One meeting matching a search query, with a snippet around the match.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct SearchHit {
    pub entry_id: i64,
    pub title: String,
    pub timestamp: i64,
    pub snippet: String,
}

pub struct HistoryManager {
    app_handle: AppHandle,
    recordings_dir: PathBuf,
    db_path: PathBuf,
}

impl HistoryManager {
    pub fn new(app_handle: &AppHandle) -> Result<Self> {
        // Create recordings directory in app data dir
        let app_data_dir = crate::portable::app_data_dir(app_handle)?;
        let recordings_dir = app_data_dir.join("recordings");
        let db_path = app_data_dir.join("history.db");

        // Ensure recordings directory exists
        if !recordings_dir.exists() {
            fs::create_dir_all(&recordings_dir)?;
            debug!("Created recordings directory: {:?}", recordings_dir);
        }

        let manager = Self {
            app_handle: app_handle.clone(),
            recordings_dir,
            db_path,
        };

        // Initialize database and run migrations synchronously
        manager.init_database()?;

        Ok(manager)
    }

    fn init_database(&self) -> Result<()> {
        info!("Initializing database at {:?}", self.db_path);

        let mut conn = Connection::open(&self.db_path)?;

        // Handle migration from tauri-plugin-sql to rusqlite_migration
        // tauri-plugin-sql used _sqlx_migrations table, rusqlite_migration uses user_version pragma
        self.migrate_from_tauri_plugin_sql(&conn)?;

        // Create migrations object and run to latest version
        let migrations = Migrations::new(MIGRATIONS.to_vec());

        // Validate migrations in debug builds
        #[cfg(debug_assertions)]
        migrations.validate().expect("Invalid migrations");

        // Get current version before migration
        let version_before: i32 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        debug!("Database version before migration: {}", version_before);

        // Apply any pending migrations
        migrations.to_latest(&mut conn)?;

        // Get version after migration
        let version_after: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if version_after > version_before {
            info!(
                "Database migrated from version {} to {}",
                version_before, version_after
            );
        } else {
            debug!("Database already at latest version {}", version_after);
        }

        Ok(())
    }

    /// Migrate from tauri-plugin-sql's migration tracking to rusqlite_migration's.
    /// tauri-plugin-sql used a _sqlx_migrations table, while rusqlite_migration uses
    /// SQLite's user_version pragma. This function checks if the old system was in use
    /// and sets the user_version accordingly so migrations don't re-run.
    fn migrate_from_tauri_plugin_sql(&self, conn: &Connection) -> Result<()> {
        // Check if the old _sqlx_migrations table exists
        let has_sqlx_migrations: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !has_sqlx_migrations {
            return Ok(());
        }

        // Check current user_version
        let current_version: i32 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if current_version > 0 {
            // Already migrated to rusqlite_migration system
            return Ok(());
        }

        // Get the highest version from the old migrations table
        let old_version: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if old_version > 0 {
            info!(
                "Migrating from tauri-plugin-sql (version {}) to rusqlite_migration",
                old_version
            );

            // Set user_version to match the old migration state
            conn.pragma_update(None, "user_version", old_version)?;

            // Optionally drop the old migrations table (keeping it doesn't hurt)
            // conn.execute("DROP TABLE IF EXISTS _sqlx_migrations", [])?;

            info!(
                "Migration tracking converted: user_version set to {}",
                old_version
            );
        }

        Ok(())
    }

    fn get_connection(&self) -> Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    fn map_history_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
        Ok(HistoryEntry {
            id: row.get("id")?,
            file_name: row.get("file_name")?,
            timestamp: row.get("timestamp")?,
            saved: row.get("saved")?,
            title: row.get("title")?,
            transcription_text: row.get("transcription_text")?,
            post_processed_text: row.get("post_processed_text")?,
            post_process_prompt: row.get("post_process_prompt")?,
            post_process_requested: row.get("post_process_requested")?,
            source: row.get("source")?,
            ai_notes: row.get("ai_notes")?,
            user_notes: row.get("user_notes")?,
            gdoc_id: row.get("gdoc_id")?,
        })
    }

    pub fn recordings_dir(&self) -> &std::path::Path {
        &self.recordings_dir
    }

    /// Save a new dictation history entry to the database.
    /// The WAV file should already have been written to the recordings directory.
    pub fn save_entry(
        &self,
        file_name: String,
        transcription_text: String,
        post_process_requested: bool,
        post_processed_text: Option<String>,
        post_process_prompt: Option<String>,
    ) -> Result<HistoryEntry> {
        self.save_entry_with_source(
            file_name,
            transcription_text,
            post_process_requested,
            post_processed_text,
            post_process_prompt,
            SOURCE_DICTATION,
            false,
        )
    }

    /// Save a meeting-session entry. Marked `saved` from the start so retention
    /// cleanup never deletes a long recording to make room for quick dictations.
    /// `live_notes` are the user's own jottings from the live meeting view.
    pub fn save_meeting_entry(
        &self,
        file_name: String,
        transcription_text: String,
        live_notes: Option<String>,
    ) -> Result<HistoryEntry> {
        let entry = self.save_entry_with_source(
            file_name,
            transcription_text,
            false,
            None,
            None,
            SOURCE_MEETING,
            true,
        )?;
        if let Some(notes) = live_notes.filter(|notes| !notes.trim().is_empty()) {
            let conn = self.get_connection()?;
            conn.execute(
                "UPDATE transcription_history SET live_notes = ?1 WHERE id = ?2",
                params![notes, entry.id],
            )?;
        }
        Ok(entry)
    }

    /// The user's live-meeting jottings for an entry, if any.
    pub fn get_entry_live_notes(&self, id: i64) -> Result<Option<String>> {
        let conn = self.get_connection()?;
        let notes: Option<String> = conn.query_row(
            "SELECT live_notes FROM transcription_history WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(notes.filter(|value| !value.trim().is_empty()))
    }

    #[allow(clippy::too_many_arguments)]
    fn save_entry_with_source(
        &self,
        file_name: String,
        transcription_text: String,
        post_process_requested: bool,
        post_processed_text: Option<String>,
        post_process_prompt: Option<String>,
        source: &str,
        saved: bool,
    ) -> Result<HistoryEntry> {
        let timestamp = Utc::now().timestamp();
        let title = self.format_timestamp_title(timestamp);

        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO transcription_history (
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested,
                source
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                &file_name,
                timestamp,
                saved,
                &title,
                &transcription_text,
                &post_processed_text,
                &post_process_prompt,
                post_process_requested,
                source,
            ],
        )?;

        let entry = HistoryEntry {
            id: conn.last_insert_rowid(),
            file_name,
            timestamp,
            saved,
            title,
            transcription_text,
            post_processed_text,
            post_process_prompt,
            post_process_requested,
            source: source.to_string(),
            ai_notes: None,
            user_notes: None,
            gdoc_id: None,
        };

        debug!("Saved history entry with id {}", entry.id);

        self.cleanup_old_entries()?;

        // Emit typed event for real-time frontend updates
        if let Err(e) = (HistoryUpdatePayload::Added {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(entry)
    }

    /// Update an existing history entry with new transcription results (used by retry).
    pub fn update_transcription(
        &self,
        id: i64,
        transcription_text: String,
        post_processed_text: Option<String>,
        post_process_prompt: Option<String>,
    ) -> Result<HistoryEntry> {
        let conn = self.get_connection()?;
        let updated = conn.execute(
            "UPDATE transcription_history
             SET transcription_text = ?1,
                 post_processed_text = ?2,
                 post_process_prompt = ?3
             WHERE id = ?4",
            params![
                transcription_text,
                post_processed_text,
                post_process_prompt,
                id
            ],
        )?;

        if updated == 0 {
            return Err(anyhow!("History entry {} not found", id));
        }

        let entry = conn
            .query_row(
                "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, source, ai_notes, user_notes, gdoc_id
                 FROM transcription_history WHERE id = ?1",
                params![id],
                Self::map_history_entry,
            )?;

        debug!("Updated transcription for history entry {}", id);

        if let Err(e) = (HistoryUpdatePayload::Updated {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(entry)
    }

    /// Store (or clear) the AI-generated notes for an entry and notify the UI.
    pub fn set_ai_notes(&self, id: i64, notes: Option<String>) -> Result<HistoryEntry> {
        let conn = self.get_connection()?;
        let updated = conn.execute(
            "UPDATE transcription_history SET ai_notes = ?1 WHERE id = ?2",
            params![notes, id],
        )?;
        if updated == 0 {
            return Err(anyhow!("History entry {} not found", id));
        }
        drop(conn);
        self.fetch_and_emit_updated(id)
    }

    /// Store the user's own notes for an entry. Deliberately does NOT emit an
    /// update event: the only writer is the notes editor itself, and an echo
    /// event would clobber text the user is still typing.
    pub fn set_user_notes(&self, id: i64, notes: String) -> Result<()> {
        let conn = self.get_connection()?;
        let updated = conn.execute(
            "UPDATE transcription_history SET user_notes = ?1 WHERE id = ?2",
            params![notes, id],
        )?;
        if updated == 0 {
            return Err(anyhow!("History entry {} not found", id));
        }
        Ok(())
    }

    /// Rename an entry and notify the UI.
    pub fn set_title(&self, id: i64, title: String) -> Result<HistoryEntry> {
        let conn = self.get_connection()?;
        let updated = conn.execute(
            "UPDATE transcription_history SET title = ?1 WHERE id = ?2",
            params![title, id],
        )?;
        if updated == 0 {
            return Err(anyhow!("History entry {} not found", id));
        }
        drop(conn);
        self.fetch_and_emit_updated(id)
    }

    /// Remember which Google Doc an entry was synced to and notify the UI.
    pub fn set_gdoc_id(&self, id: i64, gdoc_id: String) -> Result<HistoryEntry> {
        let conn = self.get_connection()?;
        let updated = conn.execute(
            "UPDATE transcription_history SET gdoc_id = ?1 WHERE id = ?2",
            params![gdoc_id, id],
        )?;
        if updated == 0 {
            return Err(anyhow!("History entry {} not found", id));
        }
        drop(conn);
        self.fetch_and_emit_updated(id)
    }

    /// Replace "Speaker N" labels with user-assigned names across the
    /// transcript, the generated notes, and the user's edited note body
    /// (the note view shows user_notes when present, so it must be
    /// rewritten too). One UPDATE, one Updated event.
    pub fn rename_speakers(
        &self,
        id: i64,
        names: &std::collections::HashMap<i32, String>,
    ) -> Result<HistoryEntry> {
        let entry = self
            .get_entry_by_id_sync(id)?
            .ok_or_else(|| anyhow!("History entry {} not found", id))?;
        let rename = |text: &str| crate::diarization::apply_speaker_names(text, names);
        let conn = self.get_connection()?;
        conn.execute(
            "UPDATE transcription_history
             SET transcription_text = ?1, ai_notes = ?2, user_notes = ?3
             WHERE id = ?4",
            params![
                rename(&entry.transcription_text),
                entry.ai_notes.as_deref().map(rename),
                entry.user_notes.as_deref().map(rename),
                id
            ],
        )?;
        drop(conn);
        self.fetch_and_emit_updated(id)
    }

    fn fetch_and_emit_updated(&self, id: i64) -> Result<HistoryEntry> {
        let entry = self
            .get_entry_by_id_sync(id)?
            .ok_or_else(|| anyhow!("History entry {} not found", id))?;
        if let Err(e) = (HistoryUpdatePayload::Updated {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }
        Ok(entry)
    }

    fn map_todo(row: &rusqlite::Row<'_>) -> rusqlite::Result<Todo> {
        Ok(Todo {
            id: row.get("id")?,
            title: row.get("title")?,
            created_at: row.get("created_at")?,
            done: row.get("done")?,
            source_entry_id: row.get("source_entry_id")?,
        })
    }

    fn emit_todos_updated(&self) {
        use tauri::Emitter;
        let _ = self.app_handle.emit("todos-updated", ());
    }

    pub fn add_todo(&self, title: &str, source_entry_id: Option<i64>) -> Result<Todo> {
        let title = title.trim();
        if title.is_empty() {
            return Err(anyhow!("todo title is empty"));
        }
        let created_at = Utc::now().timestamp();
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO todos (title, created_at, done, source_entry_id) VALUES (?1, ?2, 0, ?3)",
            params![title, created_at, source_entry_id],
        )?;
        let todo = Todo {
            id: conn.last_insert_rowid(),
            title: title.to_string(),
            created_at,
            done: false,
            source_entry_id,
        };
        self.emit_todos_updated();
        Ok(todo)
    }

    /// All todos, open ones first, newest first within each group.
    pub fn get_todos(&self) -> Result<Vec<Todo>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, created_at, done, source_entry_id FROM todos
             ORDER BY done ASC, id DESC",
        )?;
        let todos = stmt
            .query_map([], Self::map_todo)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(todos)
    }

    pub fn get_todo_by_id(&self, id: i64) -> Result<Option<Todo>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, created_at, done, source_entry_id FROM todos WHERE id = ?1",
        )?;
        Ok(stmt.query_row([id], Self::map_todo).optional()?)
    }

    pub fn set_todo_done(&self, id: i64, done: bool) -> Result<()> {
        let conn = self.get_connection()?;
        let updated = conn.execute(
            "UPDATE todos SET done = ?1 WHERE id = ?2",
            params![done, id],
        )?;
        if updated == 0 {
            return Err(anyhow!("todo {} not found", id));
        }
        self.emit_todos_updated();
        Ok(())
    }

    pub fn delete_todo(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM todos WHERE id = ?1", params![id])?;
        self.emit_todos_updated();
        Ok(())
    }

    /// Atomically claim the one-time action-item extraction for an entry.
    /// Returns true exactly once per entry — a concurrent second generation
    /// (or a later regenerate) gets false and must skip organizing, so the
    /// same events and todos can never be created twice. Kept internal (not
    /// on `HistoryEntry`).
    pub fn try_claim_action_items(&self, id: i64) -> Result<bool> {
        let conn = self.get_connection()?;
        let updated = conn.execute(
            "UPDATE transcription_history SET action_items_organized = 1
             WHERE id = ?1 AND action_items_organized = 0",
            params![id],
        )?;
        Ok(updated == 1)
    }

    /// Same one-shot claim as action items, for the Gmail-draft pass:
    /// commitments spotted in a meeting must draft each email exactly once.
    pub fn try_claim_email_drafts(&self, id: i64) -> Result<bool> {
        let conn = self.get_connection()?;
        let updated = conn.execute(
            "UPDATE transcription_history SET email_drafts_organized = 1
             WHERE id = ?1 AND email_drafts_organized = 0",
            params![id],
        )?;
        Ok(updated == 1)
    }

    fn map_ask_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<AskSession> {
        Ok(AskSession {
            id: row.get("id")?,
            query: row.get("query")?,
            answer: row.get("answer")?,
            created_at: row.get("created_at")?,
            provider_id: row.get("provider_id")?,
        })
    }

    fn emit_ask_sessions_updated(&self) {
        use tauri::Emitter;
        let _ = self.app_handle.emit("ask-sessions-updated", ());
    }

    pub fn create_ask_session(&self, query: &str) -> Result<AskSession> {
        let query = query.trim();
        if query.is_empty() {
            return Err(anyhow!("query is empty"));
        }
        let created_at = Utc::now().timestamp();
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO ask_sessions (query, created_at) VALUES (?1, ?2)",
            params![query, created_at],
        )?;
        let session = AskSession {
            id: conn.last_insert_rowid(),
            query: query.to_string(),
            answer: None,
            created_at,
            provider_id: None,
        };
        self.emit_ask_sessions_updated();
        Ok(session)
    }

    pub fn list_ask_sessions(&self, limit: usize) -> Result<Vec<AskSession>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, query, answer, created_at, provider_id FROM ask_sessions
             ORDER BY id DESC LIMIT ?1",
        )?;
        let sessions = stmt
            .query_map([limit.min(100) as i64], Self::map_ask_session)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(sessions)
    }

    /// The newest meetings that have notes (user-edited preferred, else AI),
    /// for the task key's meeting-notes context: (title, local datetime, body).
    pub fn recent_meeting_notes(&self, limit: usize) -> Result<Vec<(String, String, String)>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT title, timestamp, ai_notes, user_notes FROM transcription_history
             WHERE source = 'meeting' ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit.min(10) as i64], |row| {
            let title: String = row.get(0)?;
            let timestamp: i64 = row.get(1)?;
            let ai_notes: Option<String> = row.get(2)?;
            let user_notes: Option<String> = row.get(3)?;
            Ok((title, timestamp, ai_notes, user_notes))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (title, timestamp, ai_notes, user_notes) = row?;
            let body = match user_notes {
                Some(text) if !text.trim().is_empty() => text,
                _ => ai_notes.unwrap_or_default(),
            };
            if body.trim().is_empty() {
                continue;
            }
            let date = chrono::DateTime::from_timestamp(timestamp, 0)
                .map(|utc| {
                    utc.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_else(|| timestamp.to_string());
            out.push((title, date, body));
        }
        Ok(out)
    }

    pub fn get_ask_session(&self, id: i64) -> Result<Option<AskSession>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, query, answer, created_at, provider_id FROM ask_sessions WHERE id = ?1",
        )?;
        Ok(stmt.query_row([id], Self::map_ask_session).optional()?)
    }

    pub fn set_ask_answer(
        &self,
        id: i64,
        answer: Option<&str>,
        provider_id: Option<&str>,
    ) -> Result<()> {
        let conn = self.get_connection()?;
        let updated = conn.execute(
            "UPDATE ask_sessions SET answer = ?1, provider_id = ?2 WHERE id = ?3",
            params![answer, provider_id, id],
        )?;
        if updated == 0 {
            return Err(anyhow!("ask session {} not found", id));
        }
        self.emit_ask_sessions_updated();
        Ok(())
    }

    pub fn delete_ask_session(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM ask_sessions WHERE id = ?1", params![id])?;
        self.emit_ask_sessions_updated();
        Ok(())
    }

    /// Keyword search over meeting titles, notes and transcripts: the query
    /// is reduced to its meaningful words (a whole spoken question like "can
    /// you tell me what I need to do with ucg videos" must still find the
    /// meeting that mentions "UCG videos"), candidates matching ANY keyword
    /// are scored by how many they contain (whole-phrase matches rank first),
    /// and each hit carries a snippet around its best match.
    pub fn search_meeting_notes(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut keywords = query_keywords(query);
        if keywords.is_empty() {
            keywords.push(query.to_lowercase());
        }

        let escape = |word: &str| {
            format!(
                "%{}%",
                word.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        };
        let patterns: Vec<String> = keywords.iter().map(|word| escape(word)).collect();
        let clauses: Vec<String> = (1..=patterns.len())
            .map(|index| {
                format!(
                    "(title LIKE ?{index} ESCAPE '\\' OR ai_notes LIKE ?{index} ESCAPE '\\' \
                     OR user_notes LIKE ?{index} ESCAPE '\\' OR transcription_text LIKE ?{index} ESCAPE '\\')"
                )
            })
            .collect();
        let sql = format!(
            "SELECT id, title, timestamp, transcription_text, ai_notes, user_notes
             FROM transcription_history
             WHERE source = 'meeting' AND ({})
             ORDER BY id DESC LIMIT 40",
            clauses.join(" OR ")
        );

        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(patterns.iter()), |row| {
            Ok((
                row.get::<_, i64>("id")?,
                row.get::<_, String>("title")?,
                row.get::<_, i64>("timestamp")?,
                row.get::<_, String>("transcription_text")?,
                row.get::<_, Option<String>>("ai_notes")?,
                row.get::<_, Option<String>>("user_notes")?,
            ))
        })?;

        let phrase = query.to_lowercase();
        let mut scored = Vec::new();
        for row in rows {
            let (id, title, timestamp, transcript, ai_notes, user_notes) = row?;
            let haystack = format!(
                "{}\n{}\n{}\n{}",
                title,
                user_notes.as_deref().unwrap_or(""),
                ai_notes.as_deref().unwrap_or(""),
                transcript
            )
            .to_lowercase();
            let score = match_score(&haystack, &keywords, &phrase);
            if score == 0 {
                continue;
            }
            // Snippet around the whole phrase when present, else the first
            // keyword that appears.
            let fields = [
                user_notes.as_deref().unwrap_or(""),
                ai_notes.as_deref().unwrap_or(""),
                transcript.as_str(),
            ];
            let needle = if haystack.contains(&phrase) {
                phrase.clone()
            } else {
                keywords
                    .iter()
                    .find(|word| haystack.contains(word.as_str()))
                    .cloned()
                    .unwrap_or_else(|| keywords[0].clone())
            };
            let snippet = fields
                .iter()
                .find_map(|field| snippet_around(field, &needle))
                .unwrap_or_default();
            scored.push((
                score,
                SearchHit {
                    entry_id: id,
                    title,
                    timestamp,
                    snippet,
                },
            ));
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.entry_id.cmp(&a.1.entry_id)));
        Ok(scored
            .into_iter()
            .map(|(_, hit)| hit)
            .take(limit.min(50))
            .collect())
    }

    pub fn cleanup_old_entries(&self) -> Result<()> {
        let retention_period = crate::settings::get_recording_retention_period(&self.app_handle);

        match retention_period {
            crate::settings::RecordingRetentionPeriod::Never => {
                // Don't delete anything
                Ok(())
            }
            crate::settings::RecordingRetentionPeriod::PreserveLimit => {
                // Use the old count-based logic with history_limit
                let limit = crate::settings::get_history_limit(&self.app_handle);
                self.cleanup_by_count(limit)
            }
            _ => {
                // Use time-based logic
                self.cleanup_by_time(retention_period)
            }
        }
    }

    fn delete_entries_and_files(&self, entries: &[(i64, String)]) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }

        let conn = self.get_connection()?;
        let mut deleted_count = 0;

        for (id, file_name) in entries {
            // Delete database entry
            conn.execute(
                "DELETE FROM transcription_history WHERE id = ?1",
                params![id],
            )?;

            // Delete WAV file
            let file_path = self.recordings_dir.join(file_name);
            if file_path.exists() {
                if let Err(e) = fs::remove_file(&file_path) {
                    error!("Failed to delete WAV file {}: {}", file_name, e);
                } else {
                    debug!("Deleted old WAV file: {}", file_name);
                    deleted_count += 1;
                }
            }
        }

        Ok(deleted_count)
    }

    fn cleanup_by_count(&self, limit: usize) -> Result<()> {
        let conn = self.get_connection()?;

        // Get all entries that are not saved, ordered by timestamp desc
        let mut stmt = conn.prepare(
            "SELECT id, file_name FROM transcription_history WHERE saved = 0 ORDER BY timestamp DESC"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>("id")?, row.get::<_, String>("file_name")?))
        })?;

        let mut entries: Vec<(i64, String)> = Vec::new();
        for row in rows {
            entries.push(row?);
        }

        if entries.len() > limit {
            let entries_to_delete = &entries[limit..];
            let deleted_count = self.delete_entries_and_files(entries_to_delete)?;

            if deleted_count > 0 {
                debug!("Cleaned up {} old history entries by count", deleted_count);
            }
        }

        Ok(())
    }

    fn cleanup_by_time(
        &self,
        retention_period: crate::settings::RecordingRetentionPeriod,
    ) -> Result<()> {
        let conn = self.get_connection()?;

        // Calculate cutoff timestamp (current time minus retention period)
        let now = Utc::now().timestamp();
        let cutoff_timestamp = match retention_period {
            crate::settings::RecordingRetentionPeriod::Days3 => now - (3 * 24 * 60 * 60), // 3 days in seconds
            crate::settings::RecordingRetentionPeriod::Weeks2 => now - (2 * 7 * 24 * 60 * 60), // 2 weeks in seconds
            crate::settings::RecordingRetentionPeriod::Months3 => now - (3 * 30 * 24 * 60 * 60), // 3 months in seconds (approximate)
            _ => unreachable!("Should not reach here"),
        };

        // Get all unsaved entries older than the cutoff timestamp
        let mut stmt = conn.prepare(
            "SELECT id, file_name FROM transcription_history WHERE saved = 0 AND timestamp < ?1",
        )?;

        let rows = stmt.query_map(params![cutoff_timestamp], |row| {
            Ok((row.get::<_, i64>("id")?, row.get::<_, String>("file_name")?))
        })?;

        let mut entries_to_delete: Vec<(i64, String)> = Vec::new();
        for row in rows {
            entries_to_delete.push(row?);
        }

        let deleted_count = self.delete_entries_and_files(&entries_to_delete)?;

        if deleted_count > 0 {
            debug!(
                "Cleaned up {} old history entries based on retention period",
                deleted_count
            );
        }

        Ok(())
    }

    pub async fn get_history_entries(
        &self,
        cursor: Option<i64>,
        limit: Option<usize>,
    ) -> Result<PaginatedHistory> {
        let conn = self.get_connection()?;
        let limit = limit.map(|l| l.min(100));

        let mut entries: Vec<HistoryEntry> = match (cursor, limit) {
            (Some(cursor_id), Some(lim)) => {
                let fetch_count = (lim + 1) as i64;
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, source, ai_notes, user_notes, gdoc_id
                     FROM transcription_history
                     WHERE id < ?1
                     ORDER BY id DESC
                     LIMIT ?2",
                )?;
                let result = stmt
                    .query_map(params![cursor_id, fetch_count], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
            (None, Some(lim)) => {
                let fetch_count = (lim + 1) as i64;
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, source, ai_notes, user_notes, gdoc_id
                     FROM transcription_history
                     ORDER BY id DESC
                     LIMIT ?1",
                )?;
                let result = stmt
                    .query_map(params![fetch_count], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
            (_, None) => {
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, source, ai_notes, user_notes, gdoc_id
                     FROM transcription_history
                     ORDER BY id DESC",
                )?;
                let result = stmt
                    .query_map([], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
        };

        let has_more = limit.is_some_and(|lim| entries.len() > lim);
        if has_more {
            entries.pop();
        }

        Ok(PaginatedHistory { entries, has_more })
    }

    /// Keyset-paginated meeting entries (source = 'meeting'), newest first.
    /// Mirrors [`Self::get_history_entries`] with a source filter.
    pub async fn get_meeting_entries(
        &self,
        cursor: Option<i64>,
        limit: Option<usize>,
    ) -> Result<PaginatedHistory> {
        let conn = self.get_connection()?;
        let limit = limit.map(|l| l.min(100));

        let mut entries: Vec<HistoryEntry> = match (cursor, limit) {
            (Some(cursor_id), Some(lim)) => {
                let fetch_count = (lim + 1) as i64;
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, source, ai_notes, user_notes, gdoc_id
                     FROM transcription_history
                     WHERE source = 'meeting' AND id < ?1
                     ORDER BY id DESC
                     LIMIT ?2",
                )?;
                let result = stmt
                    .query_map(params![cursor_id, fetch_count], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
            (None, Some(lim)) => {
                let fetch_count = (lim + 1) as i64;
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, source, ai_notes, user_notes, gdoc_id
                     FROM transcription_history
                     WHERE source = 'meeting'
                     ORDER BY id DESC
                     LIMIT ?1",
                )?;
                let result = stmt
                    .query_map(params![fetch_count], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
            (_, None) => {
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, source, ai_notes, user_notes, gdoc_id
                     FROM transcription_history
                     WHERE source = 'meeting'
                     ORDER BY id DESC",
                )?;
                let result = stmt
                    .query_map([], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
        };

        let has_more = limit.is_some_and(|lim| entries.len() > lim);
        if has_more {
            entries.pop();
        }

        Ok(PaginatedHistory { entries, has_more })
    }

    #[cfg(test)]
    fn get_latest_entry_with_conn(conn: &Connection) -> Result<Option<HistoryEntry>> {
        let mut stmt = conn.prepare(
            "SELECT
                id,
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested,
                source,
                ai_notes,
                user_notes,
                gdoc_id
             FROM transcription_history
             ORDER BY timestamp DESC
             LIMIT 1",
        )?;

        let entry = stmt.query_row([], Self::map_history_entry).optional()?;
        Ok(entry)
    }

    /// Get the latest entry with non-empty transcription text.
    pub fn get_latest_completed_entry(&self) -> Result<Option<HistoryEntry>> {
        let conn = self.get_connection()?;
        Self::get_latest_completed_entry_with_conn(&conn)
    }

    fn get_latest_completed_entry_with_conn(conn: &Connection) -> Result<Option<HistoryEntry>> {
        let mut stmt = conn.prepare(
            "SELECT
                id,
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested,
                source,
                ai_notes,
                user_notes,
                gdoc_id
             FROM transcription_history
             WHERE transcription_text != ''
             ORDER BY timestamp DESC
             LIMIT 1",
        )?;

        let entry = stmt.query_row([], Self::map_history_entry).optional()?;
        Ok(entry)
    }

    pub async fn toggle_saved_status(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;

        // Get current saved status
        let current_saved: bool = conn.query_row(
            "SELECT saved FROM transcription_history WHERE id = ?1",
            params![id],
            |row| row.get("saved"),
        )?;

        let new_saved = !current_saved;

        conn.execute(
            "UPDATE transcription_history SET saved = ?1 WHERE id = ?2",
            params![new_saved, id],
        )?;

        debug!("Toggled saved status for entry {}: {}", id, new_saved);

        // Emit history updated event
        if let Err(e) = (HistoryUpdatePayload::Toggled { id }).emit(&self.app_handle) {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(())
    }

    pub fn get_audio_file_path(&self, file_name: &str) -> PathBuf {
        self.recordings_dir.join(file_name)
    }

    pub async fn get_entry_by_id(&self, id: i64) -> Result<Option<HistoryEntry>> {
        self.get_entry_by_id_sync(id)
    }

    pub fn get_entry_by_id_sync(&self, id: i64) -> Result<Option<HistoryEntry>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT
                id,
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested,
                source,
                ai_notes,
                user_notes,
                gdoc_id
             FROM transcription_history
             WHERE id = ?1",
        )?;

        let entry = stmt.query_row([id], Self::map_history_entry).optional()?;

        Ok(entry)
    }

    pub async fn delete_entry(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;

        // Get the entry to find the file name
        if let Some(entry) = self.get_entry_by_id(id).await? {
            // Delete the audio file first
            let file_path = self.get_audio_file_path(&entry.file_name);
            if file_path.exists() {
                if let Err(e) = fs::remove_file(&file_path) {
                    error!("Failed to delete audio file {}: {}", entry.file_name, e);
                    // Continue with database deletion even if file deletion fails
                }
            }
        }

        // Delete from database
        conn.execute(
            "DELETE FROM transcription_history WHERE id = ?1",
            params![id],
        )?;

        debug!("Deleted history entry with id: {}", id);

        // Emit history updated event
        if let Err(e) = (HistoryUpdatePayload::Deleted { id }).emit(&self.app_handle) {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(())
    }

    fn format_timestamp_title(&self, timestamp: i64) -> String {
        if let Some(utc_datetime) = DateTime::from_timestamp(timestamp, 0) {
            // Convert UTC to local timezone
            let local_datetime = utc_datetime.with_timezone(&Local);
            local_datetime.format("%B %e, %Y - %l:%M%p").to_string()
        } else {
            format!("Recording {}", timestamp)
        }
    }
}

/// The meaningful words of a query: lowercased, split on non-alphanumerics,
/// filler words and one-character tokens dropped, capped at 8.
fn query_keywords(query: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the",
        "a",
        "an",
        "and",
        "or",
        "to",
        "of",
        "in",
        "on",
        "for",
        "with",
        "about",
        "was",
        "were",
        "is",
        "are",
        "am",
        "be",
        "been",
        "did",
        "do",
        "does",
        "what",
        "when",
        "where",
        "who",
        "why",
        "how",
        "can",
        "could",
        "will",
        "would",
        "should",
        "you",
        "your",
        "me",
        "my",
        "we",
        "our",
        "us",
        "tell",
        "need",
        "that",
        "this",
        "these",
        "those",
        "it",
        "its",
        "at",
        "from",
        "have",
        "has",
        "had",
        "say",
        "said",
        "says",
        "talk",
        "talked",
        "spoke",
        "speak",
        "discussed",
        "discuss",
        "meeting",
        "meetings",
        "note",
        "notes",
        "please",
        "any",
        "anything",
        "there",
        "recently",
        "last",
        "again",
    ];
    let mut seen = std::collections::HashSet::new();
    query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.len() >= 2 && !STOPWORDS.contains(word))
        .filter(|word| seen.insert(word.to_string()))
        .map(str::to_string)
        .take(8)
        .collect()
}

/// Relevance of a lowercased haystack: keyword hits count once each, a
/// whole-phrase match dominates everything.
fn match_score(haystack_lower: &str, keywords: &[String], phrase_lower: &str) -> u32 {
    let mut score = keywords
        .iter()
        .filter(|word| haystack_lower.contains(word.as_str()))
        .count() as u32;
    if !phrase_lower.is_empty() && haystack_lower.contains(phrase_lower) {
        score += 100;
    }
    score
}

/// ~160 chars of context around the first case-insensitive match of
/// `needle` in `haystack`, ellipsized on clipped ends. None when absent.
fn snippet_around(haystack: &str, needle: &str) -> Option<String> {
    if haystack.is_empty() || needle.is_empty() {
        return None;
    }
    let pos = haystack.to_lowercase().find(&needle.to_lowercase())?;
    // Case-folding can shift byte offsets for exotic characters; clamp the
    // position onto a char boundary of the original text.
    let mut start = pos.saturating_sub(60).min(haystack.len());
    while start > 0 && !haystack.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (pos + needle.len() + 100).min(haystack.len());
    while end < haystack.len() && !haystack.is_char_boundary(end) {
        end += 1;
    }
    let core: String = haystack[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < haystack.len() { "…" } else { "" };
    Some(format!("{prefix}{core}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE transcription_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_name TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                saved BOOLEAN NOT NULL DEFAULT 0,
                title TEXT NOT NULL,
                transcription_text TEXT NOT NULL,
                post_processed_text TEXT,
                post_process_prompt TEXT,
                post_process_requested BOOLEAN NOT NULL DEFAULT 0,
                source TEXT NOT NULL DEFAULT 'dictation',
                ai_notes TEXT,
                user_notes TEXT,
                gdoc_id TEXT
            );",
        )
        .expect("create transcription_history table");
        conn
    }

    fn insert_entry(conn: &Connection, timestamp: i64, text: &str, post_processed: Option<&str>) {
        conn.execute(
            "INSERT INTO transcription_history (
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                format!("handy-{}.wav", timestamp),
                timestamp,
                false,
                format!("Recording {}", timestamp),
                text,
                post_processed,
                Option::<String>::None,
                false,
            ],
        )
        .expect("insert history entry");
    }

    #[test]
    fn spoken_questions_reduce_to_meaningful_keywords() {
        let words = super::query_keywords("can you tell me what i need to do with ucg videos");
        assert_eq!(words, vec!["ucg".to_string(), "videos".to_string()]);
        // Non-English queries keep their words.
        assert!(!super::query_keywords("Umsatz im Oktober").is_empty());
    }

    #[test]
    fn scoring_prefers_more_keywords_and_phrase_matches() {
        let kws = vec!["ucg".to_string(), "videos".to_string()];
        assert_eq!(
            super::match_score("we must finish the ucg videos", &kws, "zzz"),
            2
        );
        assert_eq!(super::match_score("about the videos", &kws, "zzz"), 1);
        assert_eq!(super::match_score("nothing relevant", &kws, "zzz"), 0);
        assert!(super::match_score("finish ucg videos", &kws, "ucg videos") > 100);
    }

    #[test]
    fn snippet_finds_case_insensitive_matches_with_context() {
        let text = "We talked for a while and then agreed the Launch moves to October because of the review.";
        let snip = super::snippet_around(text, "launch").expect("match");
        assert!(snip.contains("Launch moves to October"));
        assert!(super::snippet_around(text, "missingword").is_none());
    }

    #[test]
    fn get_latest_entry_returns_none_when_empty() {
        let conn = setup_conn();
        let entry = HistoryManager::get_latest_entry_with_conn(&conn).expect("fetch latest entry");
        assert!(entry.is_none());
    }

    #[test]
    fn get_latest_entry_returns_newest_entry() {
        let conn = setup_conn();
        insert_entry(&conn, 100, "first", None);
        insert_entry(&conn, 200, "second", Some("processed"));

        let entry = HistoryManager::get_latest_entry_with_conn(&conn)
            .expect("fetch latest entry")
            .expect("entry exists");

        assert_eq!(entry.timestamp, 200);
        assert_eq!(entry.transcription_text, "second");
        assert_eq!(entry.post_processed_text.as_deref(), Some("processed"));
    }

    #[test]
    fn get_latest_completed_entry_skips_empty_entries() {
        let conn = setup_conn();
        insert_entry(&conn, 100, "completed", None);
        insert_entry(&conn, 200, "", None);

        let entry = HistoryManager::get_latest_completed_entry_with_conn(&conn)
            .expect("fetch latest completed entry")
            .expect("completed entry exists");

        assert_eq!(entry.timestamp, 100);
        assert_eq!(entry.transcription_text, "completed");
    }
}
