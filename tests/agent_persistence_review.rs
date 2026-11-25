use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn request(path: &Path, mut payload: Value, zone: &str) -> Value {
    payload["schema_version"] = json!(1);
    let mut child = Command::new(env!("CARGO_BIN_EXE_hs"))
        .arg("--db")
        .arg(path)
        .arg("api")
        .env("TZ", zone)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    let body: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        result.status.success(),
        body["ok"].as_bool().unwrap(),
        "{body}"
    );
    body
}

#[test]
fn committed_replay_and_request_id_conflicts_do_not_require_the_writer_lock_or_clock() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("live.db");
    let initialize = json!({"operation":"initialize","request_id":"init"});
    let first = request(&db, initialize.clone(), "UTC");
    assert_eq!(first["ok"], true);
    let capture = json!({"operation":"task.add","request_id":"capture","input":{"title":"A note"},
        "preconditions":{"database_id":first["data"]["context"]["database_id"]}});
    let saved = request(&db, capture.clone(), "UTC");
    assert_eq!(saved["ok"], true);

    let writer = Connection::open(&db).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert_eq!(request(&db, initialize, "Invalid/Zone"), first);
    assert_eq!(request(&db, capture.clone(), "Invalid/Zone"), saved);
    let mut preview = capture.clone();
    preview["preview"] = json!(true);
    assert_eq!(
        request(&db, preview, "Invalid/Zone")["error"]["code"],
        "already_committed"
    );
    let mut conflict = capture;
    conflict["input"]["title"] = json!("Another note");
    assert_eq!(
        request(&db, conflict, "Invalid/Zone")["error"]["code"],
        "request_id_conflict"
    );
    writer.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn failed_initialization_does_not_create_paths_or_migrate_an_existing_v4_database() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("unused/store.sqlite");
    let initialize = json!({"operation":"initialize","request_id":"init"});
    assert_eq!(
        request(&missing, initialize.clone(), "Invalid/Zone")["error"]["code"],
        "invalid_input"
    );
    assert!(!missing.parent().unwrap().exists());

    let old = dir.path().join("existing.sqlite");
    let conn = Connection::open(&old).unwrap();
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    for sql in [
        include_str!("../migrations/001.sql"),
        include_str!("../migrations/002.sql"),
        include_str!("../migrations/003.sql"),
        include_str!("../migrations/004.sql"),
    ] {
        conn.execute_batch(sql).unwrap();
    }
    drop(conn);
    let original = fs::read(&old).unwrap();
    assert_eq!(
        request(&old, initialize.clone(), "Invalid/Zone")["error"]["code"],
        "invalid_input"
    );
    assert_eq!(fs::read(&old).unwrap(), original);
    assert_eq!(request(&old, initialize, "UTC")["ok"], true);
}

#[test]
fn filtered_entry_cursor_excludes_other_tasks_and_unlinked_history() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("filtered.sqlite");
    let init = request(
        &db,
        json!({"operation":"initialize","request_id":"init"}),
        "UTC",
    );
    assert_eq!(init["ok"], true);
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("
        INSERT INTO activities(id,name,category_id) VALUES(3,'Design',1);
        INSERT INTO tasks(id,title,activity_id,status,created_utc) VALUES (10,'A',3,'open',0),(20,'B',3,'open',0);
        WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<200)
        INSERT INTO entries(id,activity_id,start_time,end_time,task_id,quadrant_snapshot)
        SELECT i,3,'2025-01-01 10:00','2025-01-01 11:00',
               CASE WHEN i IN (22,98,173) THEN 10 WHEN i%2=0 THEN 20 ELSE NULL END,
               CASE WHEN i IN (22,98,173) THEN 'plan' ELSE NULL END FROM n;
    ").unwrap();
    let mut page = json!({"operation":"query","input":{"entity":"entries","task_id":10,
        "quadrant":"plan","search":"Design","limit":2}});
    let first = request(&db, page.clone(), "UTC");
    assert_eq!(first["data"]["items"][0]["id"], 22);
    assert_eq!(first["data"]["items"][1]["id"], 98);
    assert_eq!(first["data"]["next_after_id"], 98);
    page["input"]["after_id"] = first["data"]["next_after_id"].clone();
    let second = request(&db, page, "UTC");
    assert_eq!(second["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["data"]["items"][0]["id"], 173);
    assert!(second["data"]["next_after_id"].is_null());
}
