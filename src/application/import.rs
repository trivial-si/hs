//! Transactional Hamster import workflow; SQLite formats live in storage.
pub use crate::domain::import::ImportSummary;
use crate::{Error, Result, Store, storage::hamster::read_snapshot};
use rusqlite::TransactionBehavior;
use std::path::Path;

pub fn import_hamster(
    store: &mut Store,
    source_path: &Path,
    source_name: &str,
    day_start: Option<u32>,
    dry_run: bool,
) -> Result<ImportSummary> {
    if source_name.trim().is_empty() {
        return Err(Error::Input(
            "the Hamster source name cannot be empty".into(),
        ));
    }
    if day_start.is_some_and(|m| m >= 24 * 60) {
        return Err(Error::Input(
            "day start must be 0 through 1439 minutes after midnight".into(),
        ));
    }
    let snapshot = read_snapshot(source_path)?;
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let summary = snapshot.import_into(&tx, source_name)?;
    if let Some(minutes) = day_start {
        crate::storage::mutations::set_day_start(&tx, minutes)?;
    }
    if dry_run {
        tx.rollback()?;
    } else {
        tx.commit()?;
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::hamster::legacy_time;
    use rusqlite::Connection;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::{TempDir, tempdir};

    fn fixture() -> (TempDir, PathBuf, Store) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hamster.db");
        let src = Connection::open(&path).unwrap();
        src.execute_batch("
            CREATE TABLE version(version INTEGER);
            INSERT INTO version VALUES (9);
            CREATE TABLE categories(id INTEGER PRIMARY KEY, name TEXT);
            CREATE TABLE activities(id INTEGER PRIMARY KEY, name TEXT, deleted INTEGER, category_id INTEGER);
            CREATE TABLE tags(id INTEGER PRIMARY KEY, name TEXT, autocomplete BOOL);
            CREATE TABLE facts(id INTEGER PRIMARY KEY, activity_id INTEGER, start_time TIMESTAMP, end_time TIMESTAMP, description TEXT);
            CREATE TABLE fact_tags(fact_id INTEGER, tag_id INTEGER);
            INSERT INTO categories VALUES (10,'Work'),(11,'Unused');
            INSERT INTO activities VALUES (20,'Design',NULL,10),(21,'Break',NULL,-1),(22,'Archived',1,10);
            INSERT INTO tags VALUES (30,'Čaj','false'),(31,'design',1),(32,'unused','true');
            INSERT INTO facts VALUES (100,20,'2025-01-02 09:00:10.123456','2025-01-02 11:00:00','résumé');
            INSERT INTO facts VALUES (101,21,'2025-01-02 10:00:00',NULL,NULL);
            INSERT INTO fact_tags VALUES (100,30),(100,31);
        ").unwrap();
        drop(src);
        let store = Store::open(&dir.path().join("hs.db")).unwrap();
        (dir, path, store)
    }

    #[test]
    fn import_keeps_links_timestamps_archived_vocabulary_and_source_bytes() {
        let (_dir, path, mut store) = fixture();
        let before = fs::read(&path).unwrap();
        let expected = ImportSummary {
            imported: 2,
            skipped: 0,
            unresolved: 1,
            categories: 2,
            activities: 3,
            tags: 3,
        };
        assert_eq!(
            import_hamster(&mut store, &path, "home", Some(330), true).unwrap(),
            expected
        );
        assert!(store.entries().unwrap().is_empty());
        assert_eq!(store.day_start().unwrap(), 0);
        let report = import_hamster(&mut store, &path, "home", Some(330), false).unwrap();
        assert_eq!(report, expected);
        assert_eq!(store.day_start().unwrap(), 330);
        let entries = store.entries().unwrap();
        assert_eq!(
            entries[0].start,
            legacy_time(100, "2025-01-02 09:00:10.123456").unwrap()
        );
        assert_eq!(entries[0].category.as_deref(), Some("Work"));
        assert_eq!(entries[0].tags, ["design", "Čaj"]);
        assert_eq!(entries[0].description, "résumé");
        assert_eq!(entries[1].category, None);
        assert_eq!(entries[1].end, None);
        assert!(!entries[1].running);
        assert!(store.current().unwrap().is_none());
        assert_eq!(store.names("categories").unwrap(), ["Unused", "Work"]);
        assert_eq!(store.names("tags").unwrap(), ["design", "unused"]);
        assert_eq!(store.activities().unwrap().len(), 2);
        let raw: String = store
            .conn
            .query_row(
                "SELECT source_start FROM import_entries WHERE legacy_id=100",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(raw, "2025-01-02 09:00:10.123456");
        assert_eq!(before, fs::read(&path).unwrap());

        // Re-importing the same alias after a move also respects local deletion.
        store
            .conn
            .execute("DELETE FROM entries WHERE id=?", [entries[0].id])
            .unwrap();
        let moved = path.with_file_name("history.db");
        fs::rename(path, &moved).unwrap();
        let report = import_hamster(&mut store, &moved, "home", None, false).unwrap();
        assert_eq!((report.imported, report.skipped), (0, 2));
        assert_eq!(store.entries().unwrap().len(), 1);
    }

    #[test]
    fn import_preserves_unicode_activity_and_distinct_multi_word_tag_names() {
        let (_dir, path, mut store) = fixture();
        let src = Connection::open(&path).unwrap();
        src.execute_batch(
            "
            UPDATE activities SET name='Revisão técnica' WHERE id=20;
            UPDATE facts SET description='Relatório de São Paulo' WHERE id=100;
            INSERT INTO tags VALUES (33,'deep work',1),(34,'Deep Work',0),(35,' deep work ',1);
            INSERT INTO fact_tags VALUES (100,33),(100,34),(100,35);
            ",
        )
        .unwrap();
        drop(src);

        let report = import_hamster(&mut store, &path, "home", None, false).unwrap();
        assert_eq!(report.tags, 6);
        let entries = store.entries().unwrap();
        let entry = &entries[0];
        assert_eq!(entry.activity, "Revisão técnica");
        assert_eq!(entry.description, "Relatório de São Paulo");
        assert_eq!(entry.tags.len(), 5);
        for name in ["deep work", "Deep Work", " deep work "] {
            assert!(entry.tags.iter().any(|tag| tag == name), "{name:?}");
        }
    }

    #[test]
    fn import_rejects_blank_category_before_saving_entries_or_settings() {
        let (_dir, path, mut store) = fixture();
        let src = Connection::open(&path).unwrap();
        src.execute("INSERT INTO categories VALUES (12,'')", [])
            .unwrap();
        drop(src);

        let err = import_hamster(&mut store, &path, "home", Some(330), false).unwrap_err();
        assert!(matches!(err, Error::Input(ref message) if message.contains("category 12")));
        assert!(store.entries().unwrap().is_empty());
        assert!(store.names("categories").unwrap().is_empty());
        assert_eq!(store.day_start().unwrap(), 0);
    }

    #[test]
    fn source_conflict_rolls_back_even_earlier_new_rows() {
        let (_dir, path, mut store) = fixture();
        import_hamster(&mut store, &path, "home", None, false).unwrap();
        let before = store.entries().unwrap();
        let src = Connection::open(&path).unwrap();
        src.execute_batch(
            "
            INSERT INTO categories VALUES(12,'New');
            INSERT INTO facts VALUES(1,21,'2025-01-01 09:00:00','2025-01-01 10:00:00',NULL);
            UPDATE facts SET description='changed' WHERE id=100;
        ",
        )
        .unwrap();
        let err = import_hamster(&mut store, &path, "home", Some(500), false).unwrap_err();
        assert!(matches!(err, Error::Conflict(ref msg) if msg.contains("100")));
        assert_eq!(before, store.entries().unwrap());
        assert_eq!(store.day_start().unwrap(), 0);
        assert!(
            !store
                .names("categories")
                .unwrap()
                .contains(&"New".to_owned())
        );
    }

    #[test]
    fn separate_source_names_can_reuse_ids_without_conflicting_or_duplicating() {
        let (dir, first, mut store) = fixture();
        let other = dir.path().join(":memory:");
        fs::copy(&first, &other).unwrap();
        let source = Connection::open(&other).unwrap();
        source
            .execute(
                "UPDATE facts SET description='Other history' WHERE id=100",
                [],
            )
            .unwrap();
        drop(source);
        assert_eq!(
            import_hamster(&mut store, &first, "home", None, false)
                .unwrap()
                .imported,
            2
        );
        assert_eq!(
            import_hamster(&mut store, &other, "work", None, false)
                .unwrap()
                .imported,
            2
        );
        assert_eq!(
            import_hamster(&mut store, &other, "work", None, false)
                .unwrap()
                .skipped,
            2
        );
        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 4);
        assert!(entries.iter().any(|e| e.description == "Other history"));
        assert!(entries.iter().any(|e| e.description == "résumé"));
    }

    #[test]
    fn ambiguous_source_ids_abort_without_merging_unrelated_rows() {
        for table in ["activities", "categories", "tags", "facts"] {
            let (_dir, path, mut store) = fixture();
            let source = Connection::open(&path).unwrap();
            source
                .execute_batch(&format!(
                    "ALTER TABLE {table} RENAME TO original;
                 CREATE TABLE {table} AS SELECT * FROM original;
                 INSERT INTO {table} SELECT * FROM original LIMIT 1;"
                ))
                .unwrap();
            let err = import_hamster(&mut store, &path, "home", Some(330), false).unwrap_err();
            assert!(err.to_string().contains("duplicate ID"), "{table}: {err}");
            assert!(store.entries().unwrap().is_empty());
            assert_eq!(store.day_start().unwrap(), 0);
        }
    }

    #[test]
    fn malformed_date_and_unsupported_schema_leave_destination_empty() {
        let (_dir, path, mut store) = fixture();
        let source = Connection::open(&path).unwrap();
        source
            .execute("UPDATE facts SET start_time='not a date' WHERE id=101", [])
            .unwrap();
        let err = import_hamster(&mut store, &path, "home", None, false).unwrap_err();
        assert!(err.to_string().contains("101"));
        assert!(store.entries().unwrap().is_empty());
        assert!(store.names("categories").unwrap().is_empty());
        source.execute("UPDATE version SET version=8", []).unwrap();
        let err = import_hamster(&mut store, &path, "home", None, false).unwrap_err();
        assert!(err.to_string().contains("version 9"));
        assert!(store.entries().unwrap().is_empty());
        assert!(import_hamster(&mut store, &path, "home", Some(1440), false).is_err());
    }
}
