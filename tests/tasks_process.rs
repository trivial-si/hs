use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn invoke(db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hs"))
        .env("TZ", "UTC")
        .env("COLUMNS", "80")
        .arg("--db")
        .arg(db)
        .args(args)
        .output()
        .unwrap()
}

fn run(db: &Path, args: &[&str]) -> String {
    let output = invoke(db, args);
    assert!(
        output.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn rows(db: &Path) -> Vec<Value> {
    let text = run(db, &["export", "--all", "--format", "json"]);
    let document: Value = serde_json::from_str(&text).unwrap();
    document["entries"].as_array().unwrap().clone()
}

fn matrix_quadrant(db: &Path, id: i64) -> String {
    let matrix: Value = serde_json::from_str(&run(db, &["--json", "matrix"])).unwrap();
    matrix["quadrants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|quadrant| {
            quadrant["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|task| task["id"] == id)
        })
        .unwrap()["key"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn matrix_keeps_due_dates_independent_and_suggests_only_do_then_plan() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tasks.db");

    // A numeric task title must survive even when it resembles a quick-entry clock.
    run(&db, &["task", "add", "0930", "--due", "2000-01-02"]);
    let matrix = run(&db, &["matrix"]);
    assert!(matrix.contains("INBOX"));
    assert!(matrix.contains("#1"));
    assert!(matrix.contains("0930"));
    assert!(matrix.contains("2000-01-02 overdue"));
    assert_eq!(matrix_quadrant(&db, 1), "inbox");
    let task = hs::tasks::get(&hs::Store::open(&db).unwrap(), 1).unwrap();
    assert_eq!(task.activity, "0930");
    assert!(run(&db, &["task", "next"]).contains("No open Do or Plan tasks"));
    run(&db, &["task", "set", "1", "--quadrant", "plan"]);
    run(&db, &["task", "add", "Call back", "--quadrant", "do"]);
    assert!(run(&db, &["task", "next"]).contains("2\tDo\topen"));
    let matrix = run(&db, &["matrix"]);
    assert!(matrix.contains("PLAN"));
    assert!(matrix.contains("#1"));
    assert!(matrix.contains("2000-01-02 overdue"));
    assert_eq!(matrix_quadrant(&db, 1), "plan");
    run(&db, &["task", "archive", "2"]);
    assert!(run(&db, &["task", "next"]).contains("1\tPlan\topen"));
    assert!(!run(&db, &["matrix"]).contains("Call back"));
    assert!(run(&db, &["task", "list", "--all"]).contains("2\tDo\tarchived"));
    run(&db, &["task", "reopen", "2"]);
    assert!(run(&db, &["task", "list"]).contains("2\tDo\topen"));
    assert!(rows(&db).is_empty());
}

#[test]
fn task_edits_preserve_live_focus_and_later_segments_take_the_new_priority() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("focus.db");
    run(
        &db,
        &[
            "task",
            "add",
            "Send proposal",
            "--activity",
            "Writing@Work, #client",
            "--quadrant",
            "plan",
        ],
    );
    run(&db, &["task", "add", "Unrelated task"]);
    run(&db, &["pomo", "start", "--task", "1", "--focus", "1h"]);

    let initial = rows(&db);
    assert_eq!(initial.len(), 1);
    let focus_id = initial[0]["id"].clone();
    assert_eq!(initial[0]["state"], "running");
    assert_eq!(initial[0]["task_id"], 1);
    assert_eq!(initial[0]["quadrant"], "plan");
    assert_eq!(initial[0]["category"], "Work");
    assert_eq!(initial[0]["tags"], serde_json::json!(["client"]));

    run(
        &db,
        &[
            "task",
            "set",
            "1",
            "--quadrant",
            "do",
            "--title",
            "Submit proposal",
            "--notes",
            "Reviewed",
        ],
    );
    run(&db, &["task", "done", "2"]);
    let current = rows(&db);
    assert_eq!(current.len(), 1);
    assert_eq!(current[0]["id"], focus_id);
    assert_eq!(current[0]["state"], "running");
    assert_eq!(current[0]["quadrant"], "plan");
    assert!(
        current[0]["description"]
            .as_str()
            .unwrap()
            .contains("Send proposal")
    );
    assert!(run(&db, &["pomo", "status"]).contains("focus, running"));
    let matrix = run(&db, &["matrix"]);
    assert!(matrix.contains("DO"));
    assert!(matrix.contains("#1"));
    assert!(matrix.contains("Submit proposal"));
    assert_eq!(matrix_quadrant(&db, 1), "do");

    run(&db, &["pomo", "pause"]);
    let suggestion = run(&db, &["task", "next"]);
    assert!(suggestion.contains("Use `hs pomo resume`"));
    assert!(!suggestion.contains("hs pomo start --task"));
    run(&db, &["pomo", "resume"]);
    run(&db, &["task", "done", "1"]);
    let records = rows(&db);
    assert_eq!(records.len(), 2);
    let old = records.iter().find(|row| row["id"] == focus_id).unwrap();
    let later = records.iter().find(|row| row["id"] != focus_id).unwrap();
    assert_eq!(old["quadrant"], "plan");
    assert_eq!(later["quadrant"], "do");
    assert_eq!(later["task_id"], 1);
    assert!(
        later["description"]
            .as_str()
            .unwrap()
            .contains("Submit proposal")
    );
    assert!(records.iter().all(|row| row["state"] == "completed"));
    assert!(run(&db, &["pomo", "status"]).contains("stopped"));
    assert!(!invoke(&db, &["start", "--task", "1"]).status.success());
    let report = run(&db, &["report", "--all", "--group-by", "quadrant"]);
    assert!(report.lines().any(|line| line.ends_with("\tPlan")));
    assert!(report.lines().any(|line| line.ends_with("\tDo")));
}

#[test]
fn task_and_free_text_are_exclusive_and_plain_tracking_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("manual.db");
    run(&db, &["task", "add", "Task"]);
    run(&db, &["start", "Independent@Home, #plain"]);
    for args in [
        &["start", "Conflicting", "--task", "1"][..],
        &["pomo", "start", "Conflicting", "--task", "1"],
        &["start"],
        &["pomo", "start"],
    ] {
        assert_eq!(invoke(&db, args).status.code(), Some(2), "{args:?}");
    }
    run(&db, &["task", "done", "1"]);
    let plain = rows(&db);
    assert_eq!(plain.len(), 1);
    assert_eq!(plain[0]["activity"], "Independent");
    assert_eq!(plain[0]["state"], "running");
    assert!(plain[0]["task_id"].is_null());
    assert!(plain[0]["quadrant"].is_null());

    run(&db, &["task", "reopen", "1"]);
    run(&db, &["start", "--task", "1"]);
    run(&db, &["task", "done", "1"]);
    let entries = rows(&db);
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|row| row["state"] == "completed"));
    assert!(
        entries
            .iter()
            .any(|row| row["task_id"] == 1 && row["quadrant"].is_null())
    );
    for shell in ["bash", "zsh", "fish"] {
        let script = run(&db, &["completions", shell]);
        for name in ["matrix", "reopen", "quadrant"] {
            assert!(script.contains(name), "{shell} completion lacks {name}");
        }
    }
}

#[test]
fn plain_task_metadata_edits_preserve_other_fields_and_recorded_history() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metadata.db");
    run(
        &db,
        &[
            "task",
            "add",
            "Send proposal",
            "--category",
            " Work ",
            "--notes",
            "Reviewed",
            "--tags",
            "client,billable",
        ],
    );
    let get_task = || hs::tasks::get(&hs::Store::open(&db).unwrap(), 1).unwrap();
    assert_eq!(get_task().activity, "Send proposal");
    assert_eq!(get_task().category.as_deref(), Some("Work"));
    run(&db, &["pomo", "start", "--task", "1", "--focus", "1h"]);
    let initial = rows(&db);

    run(
        &db,
        &[
            "task",
            "set",
            "1",
            "--time-label",
            " Writing@home, #literal ",
        ],
    );
    let task = get_task();
    assert_eq!(task.activity, "Writing@home, #literal");
    assert_eq!(task.category.as_deref(), Some("Work"));
    assert_eq!(task.description, "Reviewed");
    assert_eq!(task.tags, ["billable", "client"]);
    // Editing the task must not relabel a running or historical time entry.
    let running = rows(&db);
    for field in ["id", "activity", "category", "description", "tags", "state"] {
        assert_eq!(running[0][field], initial[0][field], "{field}");
    }

    run(&db, &["task", "set", "1", "--category", ""]);
    let task = get_task();
    assert_eq!(task.category, None);
    assert_eq!(task.activity, "Writing@home, #literal");
    assert_eq!(task.description, "Reviewed");
    assert_eq!(task.tags, ["billable", "client"]);
    run(&db, &["pomo", "pause"]);
    run(&db, &["pomo", "resume"]);
    let entries = rows(&db);
    assert_eq!(entries.len(), 2);
    let later = entries
        .iter()
        .find(|entry| entry["id"] != initial[0]["id"])
        .unwrap();
    assert_eq!(later["activity"], "Writing@home, #literal");
    assert!(later["category"].is_null());
    assert_eq!(later["tags"], serde_json::json!(["billable", "client"]));
    run(&db, &["pomo", "stop"]);

    // Title edits keep the separate reporting label, including the original default.
    run(&db, &["task", "add", "Default label"]);
    run(&db, &["task", "set", "2", "--title", "New title"]);
    let task = hs::tasks::get(&hs::Store::open(&db).unwrap(), 2).unwrap();
    assert_eq!(task.activity, "Default label");
    run(&db, &["task", "set", "1", "--title", "Submit proposal"]);
    assert_eq!(get_task().activity, "Writing@home, #literal");
}

#[test]
fn task_plain_options_are_literal_and_explicit_category_overrides_legacy_input() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("options.db");
    run(
        &db,
        &[
            "task",
            "add",
            "First",
            "--time-label",
            "  Writing@home, #literal  ",
            "--category",
            " Personal ",
        ],
    );
    let get_task = || hs::tasks::get(&hs::Store::open(&db).unwrap(), 1).unwrap();
    assert_eq!(get_task().activity, "Writing@home, #literal");
    assert_eq!(get_task().category.as_deref(), Some("Personal"));
    assert!(get_task().tags.is_empty());

    run(
        &db,
        &[
            "task",
            "add",
            "Second",
            "--activity",
            "Review@Embedded, notes, #tag",
            "--category",
            "Explicit",
        ],
    );
    let second = hs::tasks::get(&hs::Store::open(&db).unwrap(), 2).unwrap();
    assert_eq!(second.activity, "Review");
    assert_eq!(second.category.as_deref(), Some("Explicit"));
    assert_eq!(second.description, "notes");
    assert_eq!(second.tags, ["tag"]);

    run(
        &db,
        &[
            "task",
            "set",
            "1",
            "--category",
            "Explicit",
            "--activity",
            "Review@Embedded, notes, #tag",
        ],
    );
    assert_eq!(get_task().category.as_deref(), Some("Explicit"));
    run(
        &db,
        &[
            "task",
            "set",
            "1",
            "--category",
            " ",
            "--activity",
            "Review@Embedded",
        ],
    );
    assert_eq!(get_task().category, None);
    assert_eq!(get_task().tags, ["tag"]);
    run(
        &db,
        &[
            "task",
            "set",
            "1",
            "--time-label",
            "Writing",
            "--category",
            " Work ",
        ],
    );
    assert_eq!(get_task().category.as_deref(), Some("Work"));
    assert_eq!(get_task().description, "notes");
    assert_eq!(get_task().tags, ["tag"]);

    for args in [
        &[
            "task",
            "add",
            "Invalid",
            "--activity",
            "A",
            "--time-label",
            "B",
        ][..],
        &["task", "set", "1", "--activity", "A", "--time-label", "B"],
        &[
            "task",
            "set",
            "1",
            "--time-label",
            " ",
            "--category",
            "Unwanted",
        ],
        &["task", "add", "Invalid", "--time-label", " "],
    ] {
        let before = get_task();
        assert!(!invoke(&db, args).status.success(), "{args:?}");
        assert_eq!(get_task(), before);
        assert_eq!(
            hs::tasks::list(&hs::Store::open(&db).unwrap(), None)
                .unwrap()
                .len(),
            2
        );
    }
}
