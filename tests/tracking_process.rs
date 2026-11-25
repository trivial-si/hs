use std::path::Path;
use std::process::{Command, Output, Stdio};

use hs::Store;

fn hs(db: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hs"));
    command.env("TZ", "Etc/UTC").arg("--db").arg(db);
    command
}

fn expect_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn independent_processes_can_create_and_write_the_same_database() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("new directory").join("tracker.sqlite");

    // Let several instances race both schema creation and non-overlapping writes.
    let writers: Vec<_> = (1..=8)
        .map(|day| {
            hs(&db)
                .arg("add")
                .arg(format!(
                    "2020-01-{day:02} 09:00-10:00 Task {day}@Work, #test"
                ))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for writer in writers {
        expect_success(&writer.wait_with_output().unwrap());
    }

    let store = Store::open(&db).unwrap();
    let entries = store.entries().unwrap();
    assert_eq!(entries.len(), 8);
    assert!(store.current().unwrap().is_none());
    for (index, record) in entries.iter().enumerate() {
        assert_eq!(record.activity, format!("Task {}", index + 1));
        assert_eq!(record.tags, ["test"]);
        assert_eq!((record.end.unwrap() - record.start).num_minutes(), 60);
    }
}

#[test]
fn failed_edit_restores_the_original_timer_and_vocabulary() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tracker.sqlite");
    expect_success(
        &hs(&db)
            .args(["start", "2020-11-02 09:00 First@Work, #one"])
            .output()
            .unwrap(),
    );
    let store = Store::open(&db).unwrap();
    let before = store.current().unwrap().unwrap();
    drop(store);

    // Fail after the entry and its category have changed but before the new tag
    // can be attached. A failed edit must restore the whole prior snapshot.
    let sqlite = rusqlite::Connection::open(&db).unwrap();
    sqlite
        .execute_batch(
            "CREATE TRIGGER fail_edit BEFORE INSERT ON entry_tags
        BEGIN SELECT RAISE(ABORT, 'simulated storage failure'); END;",
        )
        .unwrap();
    drop(sqlite);
    let output = hs(&db)
        .args([
            "edit",
            &before.id.to_string(),
            "--end",
            "2020-11-02 10:00",
            "--category",
            "Other",
            "--tags",
            "two",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("simulated storage failure"));
    let reopened = Store::open(&db).unwrap();
    assert_eq!(reopened.current().unwrap(), Some(before.clone()));
    assert_eq!(reopened.entries().unwrap(), [before]);
    assert_eq!(reopened.names("categories").unwrap(), ["Work"]);
    assert_eq!(reopened.names("tags").unwrap(), ["one"]);
}

#[test]
fn concurrent_retries_start_one_timer_and_a_restart_can_stop_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tracker.sqlite");
    let writers: Vec<_> = (0..8)
        .map(|_| {
            hs(&db)
                .args(["start", "2020-01-02 09:00 Build@Work, #code #testing"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for writer in writers {
        expect_success(&writer.wait_with_output().unwrap());
    }
    let status = hs(&db).arg("status").output().unwrap();
    expect_success(&status);
    assert!(String::from_utf8_lossy(&status.stdout).contains("Build@Work"));

    // A user retry with reordered tags and a later start must retain the timer.
    expect_success(
        &hs(&db)
            .args(["start", "2020-01-02 10:00 Build@Work, #testing #code"])
            .output()
            .unwrap(),
    );
    let snapshot = Store::open(&db).unwrap();
    let running = snapshot.current().unwrap().unwrap();
    assert_eq!(snapshot.entries().unwrap(), std::slice::from_ref(&running));
    assert_eq!(running.start.to_string(), "2020-01-02 09:00:00");
    drop(snapshot);
    expect_success(
        &hs(&db)
            .args(["stop", "--at", "2020-01-02 10:20"])
            .output()
            .unwrap(),
    );
    let snapshot = Store::open(&db).unwrap();
    assert!(snapshot.current().unwrap().is_none());
    let finished = snapshot.entry(running.id).unwrap();
    assert_eq!((finished.end.unwrap() - finished.start).num_minutes(), 80);
}

#[test]
fn backup_opens_independently_and_cannot_be_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tracker.sqlite");
    let backup = dir.path().join("snapshots").join("tracker.sqlite");
    expect_success(
        &hs(&db)
            .args(["start", "2020-11-02 09:00 First@Work, #one"])
            .output()
            .unwrap(),
    );
    expect_success(&hs(&db).arg("backup").arg(&backup).output().unwrap());
    let original = Store::open(&db).unwrap().current().unwrap().unwrap();
    expect_success(&hs(&db).arg("stop").output().unwrap());
    let attempt = hs(&db).arg("backup").arg(&backup).output().unwrap();
    assert!(!attempt.status.success());
    let snapshot = Store::open(&backup).unwrap();
    assert_eq!(snapshot.current().unwrap(), Some(original.clone()));
    assert_eq!(snapshot.entries().unwrap(), [original]);
    assert!(Store::open(&db).unwrap().current().unwrap().is_none());
}

#[test]
fn a_future_interval_leaves_the_running_timer_intact() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tracker.sqlite");
    expect_success(&hs(&db).args(["start", "Active"]).output().unwrap());
    let before = Store::open(&db).unwrap().current().unwrap().unwrap();
    let future = chrono::Utc::now().date_naive() + chrono::Duration::days(2);
    let output = hs(&db)
        .arg("add")
        .arg(format!("{future} 12:00-13:00 Planned"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let reopened = Store::open(&db).unwrap();
    assert_eq!(reopened.current().unwrap(), Some(before.clone()));
    assert_eq!(reopened.entries().unwrap(), [before]);
}

#[test]
fn end_now_on_a_historical_entry_uses_the_invocation_clock() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tracker.sqlite");
    expect_success(
        &hs(&db)
            .args(["start", "2020-03-01 09:00 Active"])
            .output()
            .unwrap(),
    );
    let before = Store::open(&db).unwrap().current().unwrap().unwrap();
    let earliest = hs::model::minute(chrono::Utc::now().naive_utc());
    expect_success(
        &hs(&db)
            .args(["edit", &before.id.to_string(), "--end", "now"])
            .output()
            .unwrap(),
    );
    let latest = hs::model::minute(chrono::Utc::now().naive_utc());
    let reopened = Store::open(&db).unwrap();
    let end = reopened.entry(before.id).unwrap().end.unwrap();
    assert!((earliest..=latest).contains(&end), "ended at {end}");
    assert!(reopened.current().unwrap().is_none());
}

#[test]
fn backdated_open_entry_cannot_hide_a_completed_interval() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tracker.sqlite");
    expect_success(
        &hs(&db)
            .args(["add", "2020-03-01 10:00-11:00 Planning"])
            .output()
            .unwrap(),
    );
    let before = Store::open(&db).unwrap().entries().unwrap();
    let output = hs(&db)
        .args(["start", "2020-03-01 09:00 Other"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let reopened = Store::open(&db).unwrap();
    assert_eq!(reopened.entries().unwrap(), before);
    assert!(reopened.current().unwrap().is_none());
}

#[test]
fn retry_and_stop_in_the_same_minute_preserve_one_zero_minute_record() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tracker.sqlite");
    for _ in 0..2 {
        expect_success(
            &hs(&db)
                .args(["start", "2020-03-01 09:00 Short"])
                .output()
                .unwrap(),
        );
    }
    expect_success(
        &hs(&db)
            .args(["stop", "--at", "2020-03-01 09:00"])
            .output()
            .unwrap(),
    );
    let entries = Store::open(&db).unwrap().entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].running);
    assert_eq!(entries[0].end, Some(entries[0].start));
}

#[test]
fn terminating_a_writer_mid_switch_recovers_the_original_timer() {
    use std::{
        thread,
        time::{Duration, Instant},
    };

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tracker.sqlite");
    expect_success(
        &hs(&db)
            .args(["start", "2020-03-01 09:00 First"])
            .output()
            .unwrap(),
    );
    let before = Store::open(&db).unwrap().current().unwrap().unwrap();
    let sqlite = rusqlite::Connection::open(&db).unwrap();
    // Keep INSERT executing until the test kills this process. The previous
    // entry was already updated in the very same transaction.
    sqlite
        .execute_batch(
            "PRAGMA journal_mode=DELETE;
        CREATE TRIGGER hold_switch BEFORE INSERT ON entries BEGIN
        SELECT sum(i) FROM (WITH RECURSIVE counter(i) AS
        (VALUES(0) UNION ALL SELECT i+1 FROM counter) SELECT i FROM counter);
        END;",
        )
        .unwrap();
    drop(sqlite);
    let mut child = hs(&db)
        .args(["start", "2020-03-01 10:00 Second"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let journal = dir.path().join("tracker.sqlite-journal");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !journal.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let writing = journal.exists();
    let _ = child.kill();
    let _ = child.wait().unwrap();
    assert!(writing, "writer never entered the modifying transaction");
    let reopened = Store::open(&db).unwrap();
    assert_eq!(reopened.current().unwrap(), Some(before.clone()));
    assert_eq!(reopened.entries().unwrap(), [before]);
}
