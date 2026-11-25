use std::{
    path::Path,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use chrono::Utc;
use rusqlite::Connection;

fn hs(path: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hs"));
    command.env("TZ", "Etc/UTC").arg("--db").arg(path);
    command
}

fn start_cmd(path: &Path, activity: &str, focus: &str) -> Command {
    let mut command = hs(path);
    command.args([
        "pomo",
        "start",
        activity,
        "--focus",
        focus,
        "--break",
        "1s",
        "--long-break",
        "2s",
        "--rounds",
        "2",
    ]);
    command
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "hs failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn run(path: &Path, args: &[&str]) -> String {
    success(hs(path).args(args).output().unwrap())
}

fn number(path: &Path, query: &str) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(query, [], |row| row.get(0))
        .unwrap()
}

fn deadline(path: &Path) -> i64 {
    number(
        path,
        "SELECT deadline_utc FROM pomodoro_sessions WHERE state='running'",
    )
}

// Poll the persisted UTC deadline, independent of how close the test began
// to the next whole second. Fail promptly if the test clock moves backwards.
fn wait_until(utc_second: i64) {
    let timeout = Instant::now() + Duration::from_secs(3);
    while Utc::now().timestamp() < utc_second {
        assert!(
            Instant::now() < timeout,
            "deadline did not arrive within three seconds"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn watch(path: &Path) -> String {
    let mut child = hs(path)
        .args(["pomo", "watch", "--no-bell"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let timeout = Instant::now() + Duration::from_secs(3);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= timeout {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "watch failed to exit at deadline: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        thread::sleep(Duration::from_millis(15));
    }
    let text = success(child.wait_with_output().unwrap());
    assert!(!text.contains('\u{7}'));
    text
}

fn concurrent(commands: impl IntoIterator<Item = Command>) {
    let children: Vec<_> = commands
        .into_iter()
        .map(|mut command| {
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        success(child.wait_with_output().unwrap());
    }
}

#[test]
fn ordinary_commands_recover_after_a_restart_at_the_recorded_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let cases: &[(&str, &[&str])] = &[
        ("report", &["report", "--all"]),
        ("status", &["status"]),
        ("export", &["export", "--all", "--format", "json"]),
    ];
    let mut records = Vec::new();
    for &(kind, args) in cases {
        let path = dir.path().join(kind);
        success(
            start_cmd(&path, "Design@Work, #client", "1s")
                .output()
                .unwrap(),
        );
        records.push((kind, args, deadline(&path), path));
    }
    // A caller which returns late must not charge the time spent asleep.
    wait_until(records.iter().map(|record| record.2).max().unwrap() + 1);
    for (kind, args, due, path) in records {
        let output = run(&path, args);
        assert_eq!(number(&path, "SELECT end_utc FROM entries"), due);
        assert_eq!(number(&path, "SELECT end_utc - start_utc FROM entries"), 1);
        assert_eq!(
            number(&path, "SELECT elapsed_seconds FROM pomodoro_phases"),
            1
        );
        assert_eq!(
            number(
                &path,
                "SELECT completed_focus FROM pomodoro_sessions WHERE state='ready' AND phase='short_break'"
            ),
            1
        );
        assert_eq!(
            number(
                &path,
                "SELECT count(*) FROM current_entry WHERE entry_id IS NOT NULL"
            ),
            0
        );
        if kind == "export" {
            let exported: serde_json::Value = serde_json::from_str(&output).unwrap();
            assert_eq!(exported["entries"][0]["state"], "completed");
        }
    }
}

#[test]
fn paused_time_is_not_charged_to_either_focus_segment() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paused.db");
    success(start_cmd(&path, "Study", "2s").output().unwrap());
    run(&path, &["pomo", "pause"]);
    let paused_at = number(&path, "SELECT end_utc FROM entries");
    let remaining = number(
        &path,
        "SELECT remaining_seconds FROM pomodoro_sessions WHERE state='paused'",
    );
    assert!((1..=2).contains(&remaining));
    wait_until(paused_at + 1);
    run(&path, &["pomo", "resume"]);
    let resumed_at = number(&path, "SELECT max(start_utc) FROM entries");
    assert!(resumed_at > paused_at);
    let due = deadline(&path);
    assert_eq!(due - resumed_at, remaining);
    wait_until(due);
    run(&path, &["pomo", "status"]);
    assert_eq!(number(&path, "SELECT count(*) FROM pomodoro_segments"), 2);
    assert_eq!(
        number(&path, "SELECT sum(end_utc-start_utc) FROM entries"),
        2
    );
    assert_eq!(
        number(&path, "SELECT elapsed_seconds FROM pomodoro_phases"),
        2
    );
    assert_eq!(
        number(&path, "SELECT max(end_utc)-min(start_utc) FROM entries"),
        2 + resumed_at - paused_at
    );
}

#[test]
fn concurrent_starts_and_completions_are_idempotent_and_conflicts_preserve_work() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race.db");
    concurrent((0..4).map(|_| start_cmd(&path, "Review", "2s")));
    let due = deadline(&path);
    assert_eq!(number(&path, "SELECT count(*) FROM pomodoro_sessions"), 1);
    let conflict = start_cmd(&path, "Another task", "2s").output().unwrap();
    assert!(!conflict.status.success());
    assert_eq!(deadline(&path), due);
    assert_eq!(
        number(
            &path,
            "SELECT count(*) FROM activities WHERE name='Another task'"
        ),
        0
    );
    assert_eq!(number(&path, "SELECT count(*) FROM pomodoro_segments"), 1);
    wait_until(due);
    concurrent((0..4).map(|_| {
        let mut command = hs(&path);
        command.args(["pomo", "status"]);
        command
    }));
    assert_eq!(
        number(&path, "SELECT completed_focus FROM pomodoro_sessions"),
        1
    );
    assert_eq!(number(&path, "SELECT count(*) FROM pomodoro_phases"), 1);
    assert_eq!(number(&path, "SELECT count(*) FROM entries"), 1);
    assert_eq!(number(&path, "SELECT end_utc FROM entries"), due);
}

#[test]
fn watch_exits_at_ready_and_breaks_never_create_work_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watch.db");
    success(start_cmd(&path, "Read", "1s").output().unwrap());
    assert!(watch(&path).contains("ready"));
    assert_eq!(number(&path, "SELECT count(*) FROM pomodoro_phases"), 1);
    run(&path, &["pomo", "next"]);
    assert!(watch(&path).contains("ready"));
    assert_eq!(number(&path, "SELECT count(*) FROM entries"), 1);
    run(&path, &["pomo", "next"]);
    assert!(watch(&path).contains("ready"));
    assert_eq!(
        number(
            &path,
            "SELECT remaining_seconds FROM pomodoro_sessions WHERE phase='long_break' AND state='ready'"
        ),
        2
    );
    assert_eq!(number(&path, "SELECT count(*) FROM entries"), 2);
    assert_eq!(
        number(
            &path,
            "SELECT elapsed_seconds FROM pomodoro_phases WHERE phase='short_break'"
        ),
        1
    );
}
