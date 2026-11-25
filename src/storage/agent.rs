//! Durable optimistic-concurrency tokens and idempotent operation receipts.
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Receipt {
    pub(crate) request_id: String,
    pub(crate) fingerprint: String,
    pub(crate) response_json: String,
    pub(crate) actor: Option<String>,
    pub(crate) session: Option<String>,
    pub(crate) operation: String,
    pub(crate) created_utc: i64,
}

pub(crate) fn database_identity(conn: &Connection) -> Result<String> {
    Ok(conn.query_row(
        "SELECT database_id FROM agent_control WHERE singleton=1",
        [],
        |row| row.get(0),
    )?)
}

pub(crate) fn timer_revision(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT timer_revision FROM agent_control WHERE singleton=1",
        [],
        |row| row.get(0),
    )?)
}

pub(crate) fn history_revision(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT history_revision FROM agent_control WHERE singleton=1",
        [],
        |row| row.get(0),
    )?)
}

pub(crate) fn task_revision(conn: &Connection, id: i64) -> Result<i64> {
    conn.query_row("SELECT revision FROM tasks WHERE id=?", [id], |row| {
        row.get(0)
    })
    .optional()?
    .ok_or_else(|| Error::Input(format!("Task {id} does not exist.")))
}

pub(crate) fn entry_revision(conn: &Connection, id: i64) -> Result<i64> {
    conn.query_row("SELECT revision FROM entries WHERE id=?", [id], |row| {
        row.get(0)
    })
    .optional()?
    .ok_or_else(|| Error::Input(format!("Entry {id} does not exist.")))
}

const RECEIPT_SELECT: &str = "SELECT request_id,fingerprint,response_json,actor,session,operation,created_utc FROM operation_receipts";

fn receipt(row: &Row<'_>) -> rusqlite::Result<Receipt> {
    Ok(Receipt {
        request_id: row.get(0)?,
        fingerprint: row.get(1)?,
        response_json: row.get(2)?,
        actor: row.get(3)?,
        session: row.get(4)?,
        operation: row.get(5)?,
        created_utc: row.get(6)?,
    })
}

pub(crate) fn lookup_receipt(conn: &Connection, request_id: &str) -> Result<Option<Receipt>> {
    Ok(conn
        .query_row(
            &format!("{RECEIPT_SELECT} WHERE request_id=?"),
            [request_id],
            receipt,
        )
        .optional()?)
}

pub(crate) fn insert_receipt(tx: &Transaction<'_>, value: &Receipt) -> Result<()> {
    tx.execute("INSERT INTO operation_receipts(request_id,fingerprint,response_json,actor,session,operation,created_utc)
                VALUES(?,?,?,?,?,?,?)", params![value.request_id,value.fingerprint,value.response_json,
                    value.actor,value.session,value.operation,value.created_utc])?;
    Ok(())
}

/// A list page carries only durable audit metadata. Read a full response by
/// its request ID when the caller needs to recover the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceiptSummary {
    pub(crate) request_id: String,
    pub(crate) operation: String,
    pub(crate) actor: Option<String>,
    pub(crate) session: Option<String>,
    pub(crate) created_utc: i64,
}

pub(crate) fn recent_receipts(conn: &Connection, limit: u32) -> Result<Vec<ReceiptSummary>> {
    if !(1..=100).contains(&limit) {
        return Err(Error::Input(
            "Receipt limit must be between 1 and 100.".into(),
        ));
    }
    // Transaction commit order is stable even if the system clock goes back.
    let mut statement = conn.prepare(
        "SELECT request_id,operation,actor,session,created_utc FROM operation_receipts ORDER BY rowid DESC LIMIT ?",
    )?;
    Ok(statement
        .query_map([limit], |row| {
            Ok(ReceiptSummary {
                request_id: row.get(0)?,
                operation: row.get(1)?,
                actor: row.get(2)?,
                session: row.get(3)?,
                created_utc: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PreviewEntryChange {
    pub(crate) id: i64,
    pub(crate) existed: bool,
    pub(crate) exists: bool,
}

/// Called on a private preview copy inside the outer transaction. A baseline
/// needs only stable IDs and revisions, including rows the action will delete.
pub(crate) fn begin_preview(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TEMP TABLE agent_preview_entries(id INTEGER PRIMARY KEY, revision INTEGER NOT NULL);
         INSERT INTO temp.agent_preview_entries SELECT id, revision FROM main.entries;
         SAVEPOINT agent_preview;"
    )?;
    Ok(())
}

/// The extra row lets the caller reject an oversized preview before loading
/// any entry bodies. Updates include tag and vocabulary changes via revision.
pub(crate) fn preview_entry_changes(
    tx: &Transaction<'_>,
    limit: u32,
) -> Result<Vec<PreviewEntryChange>> {
    if !(1..=100).contains(&limit) {
        return Err(Error::Input(
            "Preview entry limit must be between 1 and 100.".into(),
        ));
    }
    let mut statement = tx.prepare(
        "SELECT b.id AS id, 1, e.id IS NOT NULL
           FROM temp.agent_preview_entries b LEFT JOIN main.entries e ON e.id=b.id
          WHERE e.id IS NULL OR e.revision != b.revision
          UNION ALL
         SELECT e.id AS id, 0, 1 FROM main.entries e
          WHERE NOT EXISTS (SELECT 1 FROM temp.agent_preview_entries b WHERE b.id=e.id)
          ORDER BY id LIMIT ?",
    )?;
    Ok(statement
        .query_map([limit + 1], |row| {
            Ok(PreviewEntryChange {
                id: row.get(0)?,
                existed: row.get(1)?,
                exists: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn rewind_preview(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("ROLLBACK TO agent_preview; RELEASE agent_preview;")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Store, parse::parse_live_entry, pomodoro, time::Moment};
    use std::{fs, path::Path};

    fn v4(path: &Path) {
        let c = Connection::open(path).unwrap();
        c.pragma_update(None, "foreign_keys", false).unwrap();
        for script in [
            include_str!("../../migrations/001.sql"),
            include_str!("../../migrations/002.sql"),
            include_str!("../../migrations/003.sql"),
            include_str!("../../migrations/004.sql"),
        ] {
            c.execute_batch(script).unwrap();
        }
        c.execute_batch("
            CREATE INDEX entries_start ON entries(start_time);
            CREATE INDEX entries_utc ON entries(start_utc) WHERE start_utc IS NOT NULL;
            INSERT INTO activities(id,name,category_id) VALUES(3,'Design',1);
            INSERT INTO tasks(id,title,activity_id,quadrant,created_utc) VALUES(7,'Ship',3,'do',100);
            INSERT INTO entries(id,activity_id,start_time,end_time,task_id,quadrant_snapshot)
                VALUES(10,3,'2026-03-01 09:00:00','2026-03-01 10:00:00',7,'do'),
                      (19,3,'2026-03-02 09:00:00',NULL,7,'do');
            INSERT INTO tags(id,name) VALUES(2,'blue'), (9,'green');
            INSERT INTO task_tags VALUES(7,2);
            INSERT INTO entry_tags VALUES(10,2);
            UPDATE current_entry SET entry_id=19;
        ").unwrap();
    }

    fn sample(id: &str, time: i64) -> Receipt {
        Receipt {
            request_id: id.into(),
            fingerprint: "hash".into(),
            response_json: r#"{"id":7}"#.into(),
            actor: Some("test".into()),
            session: None,
            operation: "task.create".into(),
            created_utc: time,
        }
    }

    #[test]
    fn read_only_never_initializes_or_upgrades_and_v5_preserves_current_links() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing/hs.sqlite");
        assert!(Store::open_read_only(&missing).is_err());
        assert!(Store::open_current(&missing).is_err());
        assert!(!missing.parent().unwrap().exists());
        let path = dir.path().join("v4.sqlite");
        v4(&path);
        let before = fs::read(&path).unwrap();
        assert!(
            matches!(Store::open_read_only(&path), Err(Error::Input(message)) if message.contains("schema 4"))
        );
        assert!(Store::open_current(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.current().unwrap().unwrap().id, 19);
        let row = store.entry(10).unwrap();
        assert_eq!(row.tags, ["blue"]);
        assert_eq!(row.task.unwrap().id, 7);
        let identity = database_identity(&store.conn).unwrap();
        assert_eq!(identity.len(), 32);
        assert!(identity.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(timer_revision(&store.conn).unwrap(), 0);
        assert_eq!(entry_revision(&store.conn, 10).unwrap(), 1);
        assert_eq!(task_revision(&store.conn, 7).unwrap(), 1);
        drop(store);
        let bytes = fs::read(&path).unwrap();
        let mut reader = Store::open_read_only(&path).unwrap();
        assert_eq!(database_identity(&reader.conn).unwrap(), identity);
        assert!(reader.set_day_start(330).is_err());
        assert_eq!(reader.day_start().unwrap(), 0);
        drop(reader);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        let independent = Store::open(&dir.path().join("independent.sqlite")).unwrap();
        assert_ne!(database_identity(&independent.conn).unwrap(), identity);
    }

    #[test]
    fn entity_revisions_track_fields_tags_and_current_ownership_without_cross_talk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("revisions.sqlite");
        v4(&path);
        let store = Store::open(&path).unwrap();
        let c = &store.conn;
        assert_eq!(history_revision(c).unwrap(), 0);
        c.execute("UPDATE tasks SET title=title WHERE id=7", [])
            .unwrap();
        assert_eq!(task_revision(c, 7).unwrap(), 1);
        c.execute("UPDATE tasks SET quadrant='plan' WHERE id=7", [])
            .unwrap();
        let task_edit = task_revision(c, 7).unwrap();
        assert!(task_edit > 1);
        c.execute("INSERT INTO task_tags VALUES(7,9)", []).unwrap();
        assert!(task_revision(c, 7).unwrap() > task_edit);
        assert_eq!(entry_revision(c, 10).unwrap(), 1);
        assert_eq!(timer_revision(c).unwrap(), 0);
        c.execute("DELETE FROM entry_tags WHERE entry_id=10", [])
            .unwrap();
        let old_history = entry_revision(c, 10).unwrap();
        assert!(old_history > 1);
        let history_after_tag = history_revision(c).unwrap();
        assert!(history_after_tag > 0);
        c.execute("UPDATE entries SET description='revised' WHERE id=10", [])
            .unwrap();
        assert!(entry_revision(c, 10).unwrap() > old_history);
        assert!(history_revision(c).unwrap() > history_after_tag);
        assert_eq!(timer_revision(c).unwrap(), 0);

        c.execute("INSERT INTO entry_tags VALUES(19,2)", [])
            .unwrap();
        let control = timer_revision(c).unwrap();
        assert!(control > 0);
        c.execute(
            "UPDATE entries SET start_time='2026-03-02 08:55:00' WHERE id=19",
            [],
        )
        .unwrap();
        let edited = timer_revision(c).unwrap();
        assert!(edited > control);
        c.execute("DELETE FROM entries WHERE id=19", []).unwrap();
        assert!(timer_revision(c).unwrap() > edited);
        assert!(store.current().unwrap().is_none());
        assert!(entry_revision(c, 19).is_err());
        let before_rename = (entry_revision(c, 10).unwrap(), task_revision(c, 7).unwrap());
        c.execute("UPDATE activities SET name='Planning' WHERE id=3", [])
            .unwrap();
        assert!(entry_revision(c, 10).unwrap() > before_rename.0);
        assert!(task_revision(c, 7).unwrap() > before_rename.1);
    }

    #[test]
    fn observation_heartbeat_preserves_token_while_pause_and_resume_invalidate_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("pomo.sqlite")).unwrap();
        let now = Moment::at("2026-03-02T10:00:00Z".parse().unwrap(), chrono_tz::UTC);
        let observed = Moment::at(now.utc + chrono::Duration::seconds(6), now.zone);
        let entry = parse_live_entry("Paint", now.local().unwrap(), 0).unwrap();
        pomodoro::start(&mut store, entry, Default::default(), &now).unwrap();
        let start = timer_revision(&store.conn).unwrap();
        assert!(start > 0);
        pomodoro::status(&mut store, &observed).unwrap();
        assert_eq!(timer_revision(&store.conn).unwrap(), start);
        pomodoro::pause(&mut store, &observed).unwrap();
        let paused = timer_revision(&store.conn).unwrap();
        assert!(paused > start);
        pomodoro::resume(&mut store, &observed).unwrap();
        assert!(timer_revision(&store.conn).unwrap() > paused);
    }

    #[test]
    fn receipts_and_tokens_commit_or_rollback_with_the_mutation_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipts.sqlite");
        v4(&path);
        let mut store = Store::open(&path).unwrap();
        let first = sample("request-1", 500);
        let second = sample("request-2", 100);
        let result: Result<()> = store.write(|tx| {
            tx.execute("UPDATE tasks SET title='discard' WHERE id=7", [])?;
            insert_receipt(tx, &first)?;
            Err(Error::Input("abort".into()))
        });
        assert!(result.is_err());
        assert_eq!(task_revision(&store.conn, 7).unwrap(), 1);
        assert!(
            lookup_receipt(&store.conn, &first.request_id)
                .unwrap()
                .is_none()
        );
        store.write(|tx| insert_receipt(tx, &first)).unwrap();
        assert!(
            store
                .write(|tx| {
                    tx.execute("UPDATE tasks SET title='discard duplicate' WHERE id=7", [])?;
                    insert_receipt(tx, &first)
                })
                .is_err()
        );
        assert_eq!(task_revision(&store.conn, 7).unwrap(), 1);
        store.write(|tx| insert_receipt(tx, &second)).unwrap();
        drop(store);
        let reader = Store::open_read_only(&path).unwrap();
        assert_eq!(
            lookup_receipt(&reader.conn, &first.request_id)
                .unwrap()
                .unwrap(),
            first
        );
        let page = recent_receipts(&reader.conn, 1).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].request_id, second.request_id);
        assert_eq!(page[0].created_utc, second.created_utc);
        let ids: Vec<_> = recent_receipts(&reader.conn, 2)
            .unwrap()
            .into_iter()
            .map(|summary| summary.request_id)
            .collect();
        assert_eq!(ids, [second.request_id, first.request_id]);
        assert!(recent_receipts(&reader.conn, 101).is_err());
        assert!(recent_receipts(&reader.conn, 0).is_err());
    }

    #[test]
    fn preview_diff_identifies_insert_edit_delete_and_rewinds_including_tags_and_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entry-preview.db");
        v4(&path);
        let source = Store::open(&path).unwrap();
        let mut copy = source.preview_copy().unwrap();
        let tx = copy.conn.transaction().unwrap();
        begin_preview(&tx).unwrap();
        assert!(preview_entry_changes(&tx, 100).unwrap().is_empty());
        tx.execute("DELETE FROM entry_tags WHERE entry_id=10", [])
            .unwrap();
        tx.execute("DELETE FROM entries WHERE id=19", []).unwrap();
        tx.execute(
            "INSERT INTO entries(activity_id,start_time) VALUES(3,'2026-03-05 13:15:00')",
            [],
        )
        .unwrap();
        let inserted = tx.last_insert_rowid();
        let changes = preview_entry_changes(&tx, 100).unwrap();
        assert_eq!(
            changes,
            [
                PreviewEntryChange {
                    id: 10,
                    existed: true,
                    exists: true
                },
                PreviewEntryChange {
                    id: 19,
                    existed: true,
                    exists: false
                },
                PreviewEntryChange {
                    id: inserted,
                    existed: false,
                    exists: true
                },
            ]
        );
        assert_eq!(preview_entry_changes(&tx, 1).unwrap(), &changes[..2]);
        assert!(crate::store::find_entry(&tx, 10).unwrap().tags.is_empty());
        rewind_preview(&tx).unwrap();
        assert!(preview_entry_changes(&tx, 100).unwrap().is_empty());
        assert_eq!(crate::store::find_entry(&tx, 10).unwrap().tags, ["blue"]);
        assert_eq!(crate::store::current_id(&tx).unwrap(), Some(19));
        assert!(crate::store::find_entry(&tx, inserted).is_err());
        let next: i64 = tx
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name='entries'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(next, 19);
        tx.rollback().unwrap();
        assert_eq!(source.current().unwrap().unwrap().id, 19);
        assert!(source.entry(inserted).is_err());
    }

    #[test]
    fn oversized_preview_returns_only_one_overflow_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversized-preview.db");
        let store = Store::open(&path).unwrap();
        let mut copy = store.preview_copy().unwrap();
        let tx = copy.conn.transaction().unwrap();
        begin_preview(&tx).unwrap();
        tx.execute(
            "INSERT INTO activities(id,name,category_id) VALUES(1,'Design',1)",
            [],
        )
        .unwrap();
        tx.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<150)
                    INSERT INTO entries(activity_id,start_time) SELECT 1,'2026-03-05 13:15:00' FROM n", []).unwrap();
        assert_eq!(preview_entry_changes(&tx, 100).unwrap().len(), 101);
        assert!(preview_entry_changes(&tx, 101).is_err());
        rewind_preview(&tx).unwrap();
        assert!(preview_entry_changes(&tx, 100).unwrap().is_empty());
        tx.rollback().unwrap();
        assert!(store.entries().unwrap().is_empty());
    }

    #[test]
    fn preview_reads_committed_wal_and_its_mutations_never_reach_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.sqlite");
        v4(&path);
        let writer = Store::open(&path).unwrap();
        writer
            .conn
            .pragma_update(None, "journal_mode", "wal")
            .unwrap();
        writer
            .conn
            .pragma_update(None, "wal_autocheckpoint", 0)
            .unwrap();
        writer
            .conn
            .execute("UPDATE tasks SET title='committed in WAL' WHERE id=7", [])
            .unwrap();
        let reader = Store::open_read_only(&path).unwrap();
        let mut preview = reader.preview_copy().unwrap();
        assert_eq!(task_revision(&preview.conn, 7).unwrap(), 2);
        assert_eq!(
            crate::tasks::get(&preview, 7).unwrap().title,
            "committed in WAL"
        );
        preview
            .write(|tx| {
                tx.execute("UPDATE tasks SET title='preview only' WHERE id=7", [])?;
                insert_receipt(tx, &sample("preview-id", 1))
            })
            .unwrap();
        assert_eq!(task_revision(&preview.conn, 7).unwrap(), 3);
        assert_eq!(task_revision(&reader.conn, 7).unwrap(), 2);
        assert!(
            lookup_receipt(&writer.conn, "preview-id")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            database_identity(&preview.conn).unwrap(),
            database_identity(&reader.conn).unwrap()
        );
    }
}
