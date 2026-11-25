use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};

use crate::{
    Error, Result,
    domain::workday::Workday,
    model::{Entry, EntryClock, NewEntry, format_time, validate_civil_time},
    tasks::Quadrant,
    time::{local_at, normalized_clock},
};

pub(crate) use crate::storage::entries::{
    current_id, find_entry, load_entries, overlap_candidates, parse_datetime,
};

const APPLICATION_ID: i64 = 1752367105;
const SCHEMA_VERSION: u32 = 5;
const LOCK_TIMEOUT: Duration = Duration::from_secs(3);
const PREVIEW_MEMORY_LIMIT: u32 = 16 * 1024 * 1024;

/// SQLite interprets a leading `file:` and the exact name `:memory:` specially.
/// Resolve filesystem paths before handing them to SQLite so that it opens the
/// same file the caller selected and that hs created with private permissions.
pub(crate) fn literal_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(Error::Input("A database file path is required.".into()));
    }
    Ok(std::path::absolute(path)?)
}

fn open_existing(path: &Path) -> Result<Connection> {
    // All callers already reserved the file. SQLite must not create another
    // file if the original disappeared or was a dangling symlink.
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}

fn identity(conn: &Connection) -> Result<(i64, u32)> {
    let application_id = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok((application_id, version))
}

pub struct Store {
    pub(crate) conn: Connection,
}

impl Store {
    /// Bound lock waits for interactive callers that can retry between events.
    /// This changes only this connection; the normal CLI default is unchanged.
    pub fn set_lock_timeout(&self, timeout: Duration) -> Result<()> {
        self.conn.busy_timeout(timeout)?;
        Ok(())
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        crate::storage::agent_queries::register_search(&conn)?;
        Ok(Self { conn })
    }

    pub fn open(path: &Path) -> Result<Self> {
        let uri_spelling = path.as_os_str().as_encoded_bytes().starts_with(b"file:");
        let path = literal_path(path)?;
        if let Some(parent) = path.parent() {
            create_directory(parent)?;
        }
        let created = create_private_file(&path)?;
        let metadata = fs::metadata(&path)?;
        if !metadata.is_file() {
            return Err(Error::Input(format!(
                "Database path is not a regular file: {}",
                path.display()
            )));
        }
        let possible_legacy_placeholder = uri_spelling && !created && metadata.len() == 0;
        let mut conn = open_existing(&path)?;
        conn.busy_timeout(LOCK_TIMEOUT)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        // An ordinary reader can use the current schema while another client
        // has an uncommitted write transaction. Only migration needs a writer.
        if identity(&conn)? == (APPLICATION_ID, SCHEMA_VERSION) {
            return Self::from_connection(conn);
        }
        // Connection-local. SQLite ignores changes to this pragma inside a
        // transaction; rebuilding entries with it enabled mutates dependent rows.
        conn.pragma_update(None, "foreign_keys", false)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Another process may have initialized or upgraded the file since our probe.
        let (application_id, version) = identity(&tx)?;
        if version == 0 && application_id == 0 {
            if possible_legacy_placeholder {
                return Err(Error::Input("This empty file: path may be a placeholder from an older hs. Open the previous database using its plain filesystem path, or prefix this name with ./ to initialize the literal file.".into()));
            }
            let objects: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*'",
                [],
                |row| row.get(0),
            )?;
            if objects != 0 {
                return Err(Error::Input("This is not an hs database. Use `hs import hamster PATH` to import Hamster history into a separate hs database.".into()));
            }
            tx.execute_batch(include_str!("../../migrations/001.sql"))?;
        } else if application_id != APPLICATION_ID {
            return Err(Error::Input("This is not an hs database. Use an independent hs database and the import command.".into()));
        } else if !(1..=SCHEMA_VERSION).contains(&version) {
            return Err(Error::Input(format!(
                "Unsupported hs database schema {version}."
            )));
        }
        if version < 2 {
            rebuild_entries(&tx, include_str!("../../migrations/002.sql"))?;
        }
        if version < 3 {
            rebuild_entries(&tx, include_str!("../../migrations/003.sql"))?;
            tx.execute("CREATE INDEX IF NOT EXISTS entries_utc ON entries(start_utc) WHERE start_utc IS NOT NULL", [])?;
        }
        if version < 4 {
            tx.execute_batch(include_str!("../../migrations/004.sql"))?;
        }
        if version < 5 {
            tx.execute_batch(include_str!("../../migrations/005.sql"))?;
            check_foreign_keys(&tx)?;
        }
        tx.commit()?;
        conn.pragma_update(None, "foreign_keys", true)?;
        Self::from_connection(conn)
    }

    /// Open an existing current-schema database without initializing a path,
    /// upgrading it, or acquiring the migration writer lock.
    pub fn open_read_only(path: &Path) -> Result<Self> {
        Self::open_current_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
    }

    /// An agent apply can modify an existing current database, but only
    /// initialize is allowed to create or upgrade one. Validate the very
    /// connection the operation will use, even if its path was replaced.
    pub(crate) fn open_current(path: &Path) -> Result<Self> {
        Self::open_current_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
    }

    fn open_current_with_flags(path: &Path, mode: OpenFlags) -> Result<Self> {
        let path = literal_path(path)?;
        if !fs::metadata(&path)?.is_file() {
            return Err(Error::Input(format!(
                "Database path is not a regular file: {}",
                path.display()
            )));
        }
        let conn = Connection::open_with_flags(path, mode | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(LOCK_TIMEOUT)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        if mode.contains(OpenFlags::SQLITE_OPEN_READ_ONLY) {
            conn.pragma_update(None, "query_only", true)?;
        }
        let (application_id, version) = identity(&conn)?;
        if application_id != APPLICATION_ID {
            return Err(Error::Input(
                "The selected file is not an hs database.".into(),
            ));
        }
        if (1..SCHEMA_VERSION).contains(&version) {
            return Err(Error::Input(format!(
                "This hs database uses schema {version}. Open it once with a writable hs command to upgrade to schema {SCHEMA_VERSION}, then retry."
            )));
        }
        if version != SCHEMA_VERSION {
            return Err(Error::Input(format!(
                "Unsupported hs database schema {version}."
            )));
        }
        Self::from_connection(conn)
    }

    /// Small previews run in RAM. For a larger source, SQLite uses a private
    /// temporary database which can spill pages to disk and is deleted on close.
    pub(crate) fn preview_copy(&self) -> Result<Self> {
        // Pin one read snapshot so the backing store is chosen for the same
        // committed pages that are copied, even as another WAL writer commits.
        let source = self.conn.unchecked_transaction()?;
        let pages: u32 = source.pragma_query_value(None, "page_count", |row| row.get(0))?;
        let page_size: u32 = source.pragma_query_value(None, "page_size", |row| row.get(0))?;
        let small = pages
            .checked_mul(page_size)
            .is_some_and(|size| size <= PREVIEW_MEMORY_LIMIT);
        let mut conn = if small {
            Connection::open_in_memory()?
        } else {
            // SQLite reserves the empty filename for an anonymous temporary DB.
            Connection::open("")?
        };
        conn.busy_timeout(LOCK_TIMEOUT)?;
        conn.pragma_update(None, "page_size", page_size)?;
        {
            let backup = rusqlite::backup::Backup::new(&source, &mut conn)?;
            copy_backup_pages(&backup, LOCK_TIMEOUT)?;
        }
        source.commit()?;
        conn.pragma_update(None, "foreign_keys", true)?;
        Self::from_connection(conn)
    }

    pub(crate) fn write<T>(
        &mut self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = operation(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn entries(&self) -> Result<Vec<Entry>> {
        let transaction = self.conn.unchecked_transaction()?;
        let entries = load_entries(&transaction)?;
        transaction.commit()?;
        Ok(entries)
    }

    pub fn entry(&self, id: i64) -> Result<Entry> {
        let tx = self.conn.unchecked_transaction()?;
        let entry = find_entry(&tx, id)?;
        tx.commit()?;
        Ok(entry)
    }

    pub fn current(&self) -> Result<Option<Entry>> {
        let tx = self.conn.unchecked_transaction()?;
        let entry = current_id(&tx)?.map(|id| find_entry(&tx, id)).transpose()?;
        tx.commit()?;
        Ok(entry)
    }

    pub fn day_start(&self) -> Result<u32> {
        day_start_from(&self.conn)
    }

    pub fn set_day_start(&mut self, minutes: u32) -> Result<()> {
        Workday::new(minutes)?;
        self.conn.execute(
            "UPDATE settings SET value=? WHERE key='day_start'",
            [minutes.to_string()],
        )?;
        Ok(())
    }

    pub fn backup(&self, path: &Path) -> Result<()> {
        let path = literal_path(path)?;
        if path.exists() {
            return Err(Error::Input(format!(
                "Backup destination already exists: {}",
                path.display()
            )));
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            create_directory(parent)?;
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let temporary = tempfile::NamedTempFile::new_in(parent)?;
        let mut destination = open_existing(temporary.path())?;
        {
            let backup = rusqlite::backup::Backup::new(&self.conn, &mut destination)?;
            copy_backup_pages(&backup, LOCK_TIMEOUT)?;
        }
        let check: String =
            destination.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if check != "ok" {
            return Err(Error::Conflict(format!(
                "Backup failed integrity check: {check}"
            )));
        }
        destination
            .close()
            .map_err(|(_, error)| Error::Database(error))?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(&path)
            .map_err(|error| Error::Io(error.error))?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    }

    pub fn activities(&self) -> Result<Vec<(String, Option<String>)>> {
        let mut statement = self.conn.prepare(
            "SELECT a.name, NULLIF(c.name, '') FROM activities a JOIN categories c ON c.id=a.category_id WHERE a.archived=0 ORDER BY a.name, c.name",
        )?;
        Ok(statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?)
    }

    pub fn names(&self, kind: &str) -> Result<Vec<String>> {
        let query = match kind {
            "categories" => "SELECT name FROM categories WHERE name != '' ORDER BY name",
            "tags" => "SELECT name FROM tags WHERE autocomplete=1 ORDER BY name",
            _ => return Err(Error::Input("Unknown name list".into())),
        };
        let mut statement = self.conn.prepare(query)?;
        Ok(statement
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?)
    }
}

pub(crate) fn day_start_from(conn: &Connection) -> Result<u32> {
    let value: String = conn.query_row(
        "SELECT value FROM settings WHERE key='day_start'",
        [],
        |row| row.get(0),
    )?;
    value
        .parse::<u32>()
        .ok()
        .filter(|minutes| Workday::new(*minutes).is_ok())
        .ok_or_else(|| Error::Input("Invalid day_start setting in database".into()))
}

/// SQLite can report BUSY indefinitely, or restart an incremental copy on
/// every source write. Bound stalled progress without limiting large healthy
/// backups whose remaining page count keeps decreasing.
fn copy_backup_pages(
    backup: &rusqlite::backup::Backup<'_, '_>,
    stall_limit: Duration,
) -> Result<()> {
    use rusqlite::backup::StepResult;
    let mut least_remaining = i32::MAX;
    let mut last_progress = Instant::now();
    loop {
        let step = backup.step(128)?;
        if step == StepResult::Done {
            return Ok(());
        }
        if step == StepResult::More {
            let remaining = backup.progress().remaining;
            if remaining < least_remaining {
                least_remaining = remaining;
                last_progress = Instant::now();
                continue;
            }
        }
        if last_progress.elapsed() >= stall_limit {
            return Err(Error::Conflict("Backup could not obtain a consistent snapshot while the database was busy. Retry when the writer finishes.".into()));
        }
        // A busy writer or an incremental copy restarted by a source change
        // may stall. Back off here; a healthy copy needs no pacing delay.
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn rebuild_entries(conn: &Connection, migration: &str) -> Result<()> {
    let mut statement =
        conn.prepare("SELECT name FROM pragma_table_xinfo('entries') ORDER BY cid")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if columns != ["id", "activity_id", "description", "start_time", "end_time"] {
        return Err(Error::Conflict(
            "Cannot upgrade a modified entries table; its columns differ from the hs schema."
                .into(),
        ));
    }
    drop(statement);

    // Restore user-defined triggers and indexes as well as the standard
    // entries_start index. Installing triggers after the copy avoids firing
    // user triggers for data which already existed before the migration.
    let mut statement = conn.prepare(
        "SELECT sql FROM sqlite_schema WHERE tbl_name='entries' AND type IN ('index', 'trigger')
         AND sql IS NOT NULL ORDER BY type, name",
    )?;
    let definitions: Vec<String> = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    drop(statement);

    // Keep references from external triggers and views intact during the short
    // interval between dropping entries and renaming its replacement.
    conn.pragma_update(None, "legacy_alter_table", true)?;
    conn.execute_batch(migration)?;
    conn.pragma_update(None, "legacy_alter_table", false)?;
    for definition in definitions {
        conn.execute(&definition, [])?;
    }
    conn.execute(
        "CREATE INDEX IF NOT EXISTS entries_start ON entries(start_time)",
        [],
    )?;

    Ok(())
}

fn check_foreign_keys(conn: &Connection) -> Result<()> {
    let violation: Option<(String, String)> = conn
        .query_row("PRAGMA foreign_key_check", [], |row| {
            Ok((row.get(0)?, row.get(2)?))
        })
        .optional()?;
    if let Some((table, parent)) = violation {
        return Err(Error::Conflict(format!(
            "Database migration rolled back: {table} contains a missing reference to {parent}."
        )));
    }
    Ok(())
}

pub(crate) fn create_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}

fn create_private_file(path: &Path) -> Result<bool> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn ensure_activity(
    conn: &Connection,
    name: &str,
    category: Option<&str>,
) -> Result<i64> {
    let category = category.unwrap_or("");
    conn.execute(
        "INSERT OR IGNORE INTO categories(name) VALUES (?)",
        [category],
    )?;
    let category_id: i64 = conn.query_row(
        "SELECT id FROM categories WHERE name=?",
        [category],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO activities(name, category_id) VALUES (?, ?)",
        params![name, category_id],
    )?;
    Ok(conn.query_row(
        "SELECT id FROM activities WHERE name=? AND category_id=?",
        params![name, category_id],
        |row| row.get(0),
    )?)
}

pub(crate) fn ensure_tag(conn: &Connection, name: &str) -> Result<i64> {
    conn.execute("INSERT OR IGNORE INTO tags(name) VALUES (?)", [name])?;
    Ok(conn.query_row("SELECT id FROM tags WHERE name=?", [name], |row| row.get(0))?)
}

pub(crate) fn insert_entry(conn: &Transaction<'_>, entry: &NewEntry) -> Result<i64> {
    let clock = checked_clock(entry)?;
    let activity_id = ensure_activity(conn, &entry.activity, entry.category.as_deref())?;
    conn.execute(
        "INSERT INTO entries(activity_id, description, start_time, end_time, start_utc, end_utc, timezone, task_id, quadrant_snapshot) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            activity_id,
            entry.description,
            format_time(entry.start),
            entry.end.map(format_time),
            clock.map(|c| c.start_utc.timestamp()),
            clock.and_then(|c| c.end_utc).map(|u| u.timestamp()),
            clock.map(|c| c.zone.name()),
            entry.task.map(|task| task.id),
            entry.task.and_then(|task| task.quadrant).map(Quadrant::key),
        ],
    )?;
    let id = conn.last_insert_rowid();
    write_tags(conn, id, &entry.tags)?;
    Ok(id)
}

pub(crate) fn update_entry(conn: &Transaction<'_>, id: i64, entry: &NewEntry) -> Result<()> {
    let clock = checked_clock(entry)?;
    let activity_id = ensure_activity(conn, &entry.activity, entry.category.as_deref())?;
    let changed = conn.execute(
        "UPDATE entries SET activity_id=?, description=?, start_time=?, end_time=?, start_utc=?, end_utc=?, timezone=?, task_id=?, quadrant_snapshot=? WHERE id=?",
        params![
            activity_id,
            entry.description,
            format_time(entry.start),
            entry.end.map(format_time),
            clock.map(|c| c.start_utc.timestamp()),
            clock.and_then(|c| c.end_utc).map(|u| u.timestamp()),
            clock.map(|c| c.zone.name()),
            entry.task.map(|task| task.id),
            entry.task.and_then(|task| task.quadrant).map(Quadrant::key),
            id
        ],
    )?;
    if changed == 0 {
        return Err(Error::Input(format!("Entry {id} does not exist")));
    }
    // Keep existing links so a no-op edit also preserves its revision.
    conn.execute(
        "DELETE FROM entry_tags WHERE entry_id=?1 AND tag_id NOT IN
         (SELECT t.id FROM json_each(?2) selected JOIN tags t ON t.name=selected.value)",
        params![id, serde_json::to_string(&entry.tags)?],
    )?;
    write_tags(conn, id, &entry.tags)
}

/// Close an entry within the caller's write transaction. An absolute end is
/// authoritative for clocked entries, including when the system has traveled
/// to another time zone since the timer began.
pub(crate) fn finish_entry(
    conn: &Transaction<'_>,
    id: i64,
    local_end: NaiveDateTime,
    end_utc: Option<DateTime<Utc>>,
) -> Result<()> {
    let mut entry = find_entry(conn, id)?.as_new();
    if let (Some(clock), Some(instant)) = (&mut entry.clock, end_utc) {
        entry.end = Some(local_at(instant, clock.zone)?);
        clock.end_utc = Some(instant);
    } else {
        entry.end = Some(local_end);
    }
    let clock = checked_clock(&entry)?;
    conn.execute(
        "UPDATE entries SET end_time=?1, end_utc=?2 WHERE id=?3",
        params![
            entry.end.map(format_time),
            clock.and_then(|c| c.end_utc).map(|u| u.timestamp()),
            id
        ],
    )?;
    conn.execute(
        "UPDATE current_entry SET entry_id=NULL WHERE entry_id=?",
        [id],
    )?;
    Ok(())
}

fn write_tags(conn: &Connection, id: i64, tags: &[String]) -> Result<()> {
    for name in tags {
        let tag_id = ensure_tag(conn, name)?;
        conn.execute(
            "INSERT INTO entry_tags(entry_id,tag_id) SELECT ?1,?2
             WHERE NOT EXISTS (SELECT 1 FROM entry_tags WHERE entry_id=?1 AND tag_id=?2)",
            params![id, tag_id],
        )?;
    }
    Ok(())
}

pub(crate) fn validate_entry(entry: &NewEntry) -> Result<()> {
    checked_clock(entry).map(|_| ())
}

fn checked_clock(entry: &NewEntry) -> Result<Option<EntryClock>> {
    validate_civil_time(entry.start)?;
    if let Some(end) = entry.end {
        validate_civil_time(end)?;
    }
    let clock = normalized_clock(entry)?;
    if entry.activity.trim().is_empty() {
        return Err(Error::Input("An activity name is required".into()));
    }
    if clock.is_none() && entry.end.is_some_and(|end| end < entry.start) {
        return Err(Error::Input(
            "End time must be on or after the start time".into(),
        ));
    }
    if entry.tags.iter().any(|tag| tag.trim().is_empty()) {
        return Err(Error::Input("Tags must have a name".into()));
    }
    Ok(clock)
}

#[cfg(test)]
pub(crate) fn write_fixture<T>(
    conn: &Connection,
    action: impl FnOnce(&Transaction<'_>) -> Result<T>,
) -> Result<T> {
    let tx = conn.unchecked_transaction()?;
    let value = action(&tx)?;
    tx.commit()?;
    Ok(value)
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use tempfile::{TempDir, tempdir};

    fn legacy_store() -> (TempDir, std::path::PathBuf) {
        let temp = tempdir().unwrap();
        let path = temp.path().join("v1.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(include_str!("../../migrations/001.sql"))
            .unwrap();
        conn.execute_batch(
            "
            INSERT INTO activities(id, name, category_id) VALUES (10,'Painting',1);
            INSERT INTO entries(id, activity_id, description, start_time, end_time) VALUES
                (12,10,'completed','2026-03-01 09:00:00','2026-03-01 10:00:00'),
                (42,10,'in progress','2026-03-02 09:00:00',NULL);
            INSERT INTO tags(id, name) VALUES (23,'Home');
            INSERT INTO entry_tags(entry_id,tag_id) VALUES (12,23),(42,23);
            UPDATE current_entry SET entry_id=42;
            INSERT INTO import_sources(id, name) VALUES (1,'legacy');
            INSERT INTO import_entries(source_id,legacy_id,entry_id,fingerprint,source_start)
                VALUES (1,703,12,'fingerprint','2026-03-01 09:00:00');
            CREATE TABLE audit(entry_id INTEGER);
            CREATE TRIGGER record_insert AFTER INSERT ON entries BEGIN
                INSERT INTO audit VALUES (new.id);
            END;
            CREATE INDEX custom_entry_description ON entries(description);
            CREATE VIEW custom_history AS SELECT id FROM entries;
        ",
        )
        .unwrap();
        (temp, path)
    }

    fn scalar(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn current_schema_can_be_opened_while_another_writer_is_preparing_changes() {
        let (_dir, path) = legacy_store();
        let writer = Store::open(&path).unwrap();
        writer
            .conn
            .execute_batch("BEGIN IMMEDIATE; UPDATE entries SET description='pending' WHERE id=12;")
            .unwrap();
        let reader = Store::open(&path).unwrap();
        assert_eq!(reader.entry(12).unwrap().description, "completed");
        writer.conn.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn new_entries_reject_years_that_cannot_sort_chronologically_in_storage() {
        let (_dir, path) = legacy_store();
        let store = Store::open(&path).unwrap();
        let mut entry = store.entry(12).unwrap().as_new();
        entry.activity = "Unstored".into();
        for start in ["-0001-01-01T10:00:00", "+10000-01-01T10:00:00"] {
            entry.start = start.parse().unwrap();
            entry.end = None;
            let error = write_fixture(&store.conn, |tx| insert_entry(tx, &entry)).unwrap_err();
            assert!(matches!(error, Error::Input(message) if message.contains("four-digit")));
        }
        entry.start = "9999-12-31T23:59:00".parse().unwrap();
        entry.end = Some("+10000-01-01T00:01:00".parse().unwrap());
        assert!(write_fixture(&store.conn, |tx| update_entry(tx, 12, &entry)).is_err());
        assert_eq!(store.entry(12).unwrap().activity, "Painting");
        assert!(
            !store
                .activities()
                .unwrap()
                .iter()
                .any(|(name, _)| name == "Unstored")
        );
    }

    #[test]
    fn large_preview_uses_an_anonymous_database_and_leaves_the_source_unchanged() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("large.sqlite");
        let store = Store::open(&db).unwrap();
        let small = store.preview_copy().unwrap();
        let mode: String = small
            .conn
            .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "memory");
        store
            .conn
            .execute("CREATE TABLE preview_fixture(payload BLOB)", [])
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO preview_fixture VALUES(zeroblob(?))",
                [PREVIEW_MEMORY_LIMIT + 1],
            )
            .unwrap();
        let reader = Store::open_read_only(&db).unwrap();
        let mut copy = reader.preview_copy().unwrap();
        // An in-memory database refuses DELETE mode; SQLite's anonymous
        // temporary database permits it and has no path to expose or clean up.
        let mode: String = copy
            .conn
            .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "delete");
        let filename: String = copy
            .conn
            .query_row("PRAGMA database_list", [], |row| row.get(2))
            .unwrap();
        assert!(filename.is_empty());
        copy.set_day_start(330).unwrap();
        assert_eq!(reader.day_start().unwrap(), 0);
        drop(copy);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn backup_under_a_prolonged_exclusive_lock_returns_without_publishing_a_partial_copy() {
        let (dir, path) = legacy_store();
        let store = Store::open(&path).unwrap();
        let locker = Connection::open(&path).unwrap();
        locker.execute_batch("BEGIN EXCLUSIVE").unwrap();
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let locked = std::thread::spawn(move || {
            // Let the test fail normally even if the backup starts retrying
            // forever again. The successful implementation exits first.
            let _ = wait.recv_timeout(Duration::from_secs(8));
            drop(locker);
        });
        let destination = dir.path().join("backup.db");
        let result = store.backup(&destination);
        let _ = release.send(());
        locked.join().unwrap();
        assert!(matches!(result, Err(Error::Conflict(ref msg)) if msg.contains("busy")));
        assert!(!destination.exists());
        store.backup(&destination).unwrap();
        assert_eq!(
            Store::open(&destination)
                .unwrap()
                .entry(12)
                .unwrap()
                .description,
            "completed"
        );
    }

    #[test]
    fn upgrading_preserves_ids_links_custom_schema_and_future_id_high_water_mark() {
        let (_temp, path) = legacy_store();
        let store = Store::open(&path).unwrap();
        assert_eq!(
            scalar(&store.conn, "PRAGMA user_version"),
            i64::from(SCHEMA_VERSION)
        );
        assert_eq!(scalar(&store.conn, "PRAGMA application_id"), 1752367105);
        assert_eq!(scalar(&store.conn, "PRAGMA foreign_keys"), 1);
        assert_eq!(store.current().unwrap().unwrap().id, 42);
        assert_eq!(store.entry(12).unwrap().tags, ["Home"]);
        assert_eq!(store.entry(42).unwrap().tags, ["Home"]);
        assert_eq!(
            scalar(
                &store.conn,
                "SELECT entry_id FROM import_entries WHERE legacy_id=703"
            ),
            12
        );
        assert_eq!(
            scalar(&store.conn, "SELECT count(*) FROM custom_history"),
            2
        );
        assert_eq!(
            scalar(
                &store.conn,
                "SELECT count(*) FROM sqlite_schema WHERE name='custom_entry_description'"
            ),
            1
        );
        assert_eq!(scalar(&store.conn, "SELECT count(*) FROM audit"), 0);

        let entry = store.entry(12).unwrap().as_new();
        store
            .conn
            .execute("DELETE FROM entries WHERE id=42", [])
            .unwrap();
        assert!(store.current().unwrap().is_none());
        assert_eq!(scalar(&store.conn, "SELECT count(*) FROM entry_tags"), 1);
        drop(store);
        let reopened = Store::open(&path).unwrap();
        let new_id = write_fixture(&reopened.conn, |tx| insert_entry(tx, &entry)).unwrap();
        assert!(new_id > 42);
        assert_eq!(scalar(&reopened.conn, "SELECT entry_id FROM audit"), new_id);
        assert!(reopened.entry(42).is_err());
        assert_eq!(
            scalar(
                &reopened.conn,
                "SELECT count(*) FROM pragma_foreign_key_check"
            ),
            0
        );
        reopened.conn.execute("DELETE FROM entries", []).unwrap();
        assert!(write_fixture(&reopened.conn, |tx| insert_entry(tx, &entry)).unwrap() > new_id);
    }

    #[test]
    fn v2_upgrade_retains_deleted_high_water_mark_and_civil_history() {
        for empty in [false, true] {
            let (_temp, path) = legacy_store();
            let mut v2 = Connection::open(&path).unwrap();
            v2.pragma_update(None, "foreign_keys", false).unwrap();
            let tx = v2.transaction().unwrap();
            rebuild_entries(&tx, include_str!("../../migrations/002.sql")).unwrap();
            tx.commit().unwrap();
            v2.pragma_update(None, "foreign_keys", true).unwrap();
            v2.execute("INSERT INTO entries(id,activity_id,start_time) VALUES(5000,10,'2026-03-03 09:00:00')", []).unwrap();
            v2.execute("DELETE FROM entries WHERE id=5000", []).unwrap();
            if empty {
                v2.execute("DELETE FROM entries", []).unwrap();
            }
            drop(v2);
            let store = Store::open(&path).unwrap();
            assert_eq!(
                scalar(&store.conn, "PRAGMA user_version"),
                i64::from(SCHEMA_VERSION)
            );
            assert_eq!(
                scalar(&store.conn, "SELECT count(*) FROM pragma_foreign_key_check"),
                0
            );
            if !empty {
                let current = store.current().unwrap().unwrap();
                assert_eq!(
                    (current.id, current.clock, current.tags),
                    (42, None, vec!["Home".into()])
                );
                assert_eq!(store.entry(12).unwrap().clock, None);
                assert_eq!(
                    scalar(
                        &store.conn,
                        "SELECT entry_id FROM import_entries WHERE legacy_id=703"
                    ),
                    12
                );
            }
            store
                .conn
                .execute(
                    "INSERT INTO entries(activity_id,start_time) VALUES(10,'2026-03-04 09:00:00')",
                    [],
                )
                .unwrap();
            assert!(store.conn.last_insert_rowid() > 5000);
        }
    }

    #[test]
    fn v3_upgrade_keeps_active_pomodoro_and_existing_work_unclassified() {
        let (_temp, path) = legacy_store();
        let mut v3 = Connection::open(&path).unwrap();
        v3.pragma_update(None, "foreign_keys", false).unwrap();
        let tx = v3.transaction().unwrap();
        rebuild_entries(&tx, include_str!("../../migrations/002.sql")).unwrap();
        rebuild_entries(&tx, include_str!("../../migrations/003.sql")).unwrap();
        tx.execute_batch("
            UPDATE entries SET start_utc=unixepoch(start_time),timezone='UTC' WHERE id=42;
            INSERT INTO pomodoro_sessions(id,activity,focus_seconds,break_seconds,long_break_seconds,rounds,
                phase,state,deadline_utc,remaining_seconds,current_entry_id,timezone,created_utc,last_observed_utc)
                VALUES(7,'Painting',1500,300,900,4,'focus','running',1772443500,1500,42,'UTC',1772442000,1772442000);
            INSERT INTO pomodoro_phases(id,session_id,phase,state,started_utc,target_seconds)
                VALUES(81,7,'focus','running',1772442000,1500);
            UPDATE pomodoro_sessions SET current_phase_id=81 WHERE id=7;
            INSERT INTO pomodoro_segments(phase_id,entry_id) VALUES(81,42);
        ").unwrap();
        tx.commit().unwrap();
        assert_eq!(scalar(&v3, "PRAGMA user_version"), 3);
        let seq = scalar(&v3, "SELECT seq FROM sqlite_sequence WHERE name='entries'");
        drop(v3);

        let store = Store::open(&path).unwrap();
        assert_eq!(
            scalar(&store.conn, "PRAGMA user_version"),
            i64::from(SCHEMA_VERSION)
        );
        let timer = store.current().unwrap().unwrap();
        assert_eq!(timer.id, 42);
        assert!(timer.clock.is_some());
        assert_eq!(timer.task, None);
        assert_eq!(store.entry(12).unwrap().task, None);
        assert_eq!(
            scalar(
                &store.conn,
                "SELECT current_phase_id FROM pomodoro_sessions WHERE id=7 AND task_id IS NULL"
            ),
            81
        );
        assert_eq!(
            scalar(
                &store.conn,
                "SELECT entry_id FROM pomodoro_segments WHERE phase_id=81"
            ),
            42
        );
        assert_eq!(scalar(&store.conn, "SELECT count(*) FROM tasks"), 0);
        assert_eq!(
            scalar(
                &store.conn,
                "SELECT seq FROM sqlite_sequence WHERE name='entries'"
            ),
            seq
        );
        assert_eq!(
            scalar(&store.conn, "SELECT count(*) FROM pragma_foreign_key_check"),
            0
        );
    }

    #[test]
    fn backward_civil_clock_finish_uses_elapsed_instants_and_preserves_zone_on_edit() {
        let (_temp, path) = legacy_store();
        let store = Store::open(&path).unwrap();
        let zone = chrono_tz::Europe::Zagreb;
        let start = "2026-10-25T00:55:00Z".parse::<DateTime<Utc>>().unwrap();
        let end = "2026-10-25T01:05:00Z".parse::<DateTime<Utc>>().unwrap();
        let mut entry = store.entry(12).unwrap().as_new();
        entry.start = local_at(start, zone).unwrap();
        entry.end = None;
        entry.clock = Some(EntryClock {
            start_utc: start,
            end_utc: None,
            zone,
        });
        let id = write_fixture(&store.conn, |tx| insert_entry(tx, &entry)).unwrap();
        store
            .conn
            .execute("UPDATE current_entry SET entry_id=?", [id])
            .unwrap();
        // The user has traveled; local_end is in Tokyo while the entry was
        // started in Zagreb. Its stored civil end must still read 02:05.
        write_fixture(&store.conn, |tx| {
            finish_entry(
                tx,
                id,
                local_at(end, chrono_tz::Asia::Tokyo).unwrap(),
                Some(end),
            )
        })
        .unwrap();
        let closed = store.entry(id).unwrap();
        assert!(closed.end.unwrap() < closed.start);
        assert!(store.current().unwrap().is_none());
        assert_eq!(closed.end, Some(local_at(end, zone).unwrap()));
        assert_eq!(closed.clock.unwrap().end_utc, Some(end));
        let mut edited = closed.as_new();
        edited.end = closed
            .end
            .unwrap()
            .checked_add_signed(chrono::Duration::minutes(1));
        write_fixture(&store.conn, |tx| update_entry(tx, id, &edited)).unwrap();
        assert_eq!(
            store.entry(id).unwrap().clock.unwrap().end_utc,
            Some(end + chrono::Duration::minutes(1))
        );
        assert!(
            overlap_candidates(
                &store.conn,
                closed.start,
                closed.end.unwrap(),
                Some(start),
                Some(end),
                false,
            )
            .unwrap()
            .iter()
            .any(|e| e.id == id)
        );
    }

    #[test]
    fn unrelated_databases_are_untouched_even_when_their_version_matches() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("unrelated.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute("CREATE TABLE sqlitecustom(value TEXT)", [])
            .unwrap();
        for (version, app_id) in [(0, 0), (1, 900), (2, 0)] {
            conn.pragma_update(None, "user_version", version).unwrap();
            conn.pragma_update(None, "application_id", app_id).unwrap();
            assert!(matches!(Store::open(&path), Err(Error::Input(_))));
            assert_eq!(scalar(&conn, "PRAGMA user_version"), version);
            assert_eq!(scalar(&conn, "PRAGMA application_id"), app_id);
            assert_eq!(scalar(&conn, "SELECT count(*) FROM sqlite_schema"), 1);
        }
    }

    #[test]
    fn migration_with_dangling_reference_rolls_back_the_whole_rebuild() {
        let (_temp, path) = legacy_store();
        let damaged = Connection::open(&path).unwrap();
        damaged.pragma_update(None, "foreign_keys", false).unwrap();
        damaged
            .execute(
                r#"CREATE TABLE pragma_foreign_key_check("table" TEXT, parent TEXT)"#,
                [],
            )
            .unwrap();
        damaged
            .execute("UPDATE current_entry SET entry_id=999", [])
            .unwrap();
        drop(damaged);

        let Some(err) = Store::open(&path).err() else {
            panic!("opened corrupt v1 store");
        };
        assert!(
            matches!(err, Error::Conflict(ref msg) if msg.contains("current_entry") && msg.contains("entries"))
        );
        let original = Connection::open(&path).unwrap();
        assert_eq!(scalar(&original, "PRAGMA user_version"), 1);
        assert_eq!(scalar(&original, "SELECT entry_id FROM current_entry"), 999);
        assert_eq!(scalar(&original, "SELECT count(*) FROM entries"), 2);
        assert_eq!(scalar(&original, "SELECT count(*) FROM entry_tags"), 2);
        assert_eq!(
            scalar(
                &original,
                "SELECT count(*) FROM sqlite_schema WHERE name='record_insert'"
            ),
            1
        );
        assert_eq!(
            scalar(
                &original,
                "SELECT count(*) FROM sqlite_schema WHERE name='entries_v2' OR name='sqlite_sequence'"
            ),
            0
        );
    }
}
