use std::{path::Path, process::Command};

fn run(path: &Path, args: &[&str]) -> String {
    let result = Command::new(env!("CARGO_BIN_EXE_hs"))
        .env("TZ", "UTC")
        .arg("--db")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn task_activity_edits_do_not_discard_supplied_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hs.db");
    run(
        &path,
        &[
            "task",
            "add",
            "Proposal",
            "--activity",
            "Writing@Work, old notes, #old",
        ],
    );
    run(
        &path,
        &[
            "task",
            "set",
            "1",
            "--activity",
            "Review@Work, updated notes, #review",
        ],
    );
    let view = run(&path, &["task", "show", "1"]);
    assert!(view.contains("Notes: updated notes"));
    assert!(view.contains("Tags: review"));
    run(&path, &["task", "set", "1", "--activity", "Planning@Work"]);
    let view = run(&path, &["task", "show", "1"]);
    assert!(view.contains("Notes: updated notes"));
    assert!(view.contains("Tags: review"));
    run(
        &path,
        &[
            "task",
            "set",
            "1",
            "--activity",
            "Review@Work, embedded notes, #embedded",
            "--notes",
            "",
            "--tags",
            "explicit",
        ],
    );
    let task = hs::tasks::get(&hs::Store::open(&path).unwrap(), 1).unwrap();
    assert_eq!(task.description, "");
    assert_eq!(task.tags, ["explicit"]);
}

#[test]
fn surrounding_whitespace_does_not_change_relative_historical_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hs.db");
    run(&path, &["add", "2000-01-02 09:00-09:30 Old work"]);
    let before = chrono::Utc::now().naive_utc();
    run(&path, &["edit", "1", "--end", " NOW "]);
    let after = chrono::Utc::now().naive_utc();
    let end = hs::Store::open(&path)
        .unwrap()
        .entry(1)
        .unwrap()
        .end
        .unwrap();
    assert!(end >= hs::model::minute(before) && end <= hs::model::minute(after));
}

#[test]
fn read_commands_do_not_require_a_writer_lock_without_a_live_pomodoro() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hs.db");
    run(&path, &["task", "add", "Read", "--quadrant", "plan"]);
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    for args in [
        &["status"][..],
        &["list", "--all"],
        &["report", "--all"],
        &["export", "--all", "--format", "json"],
        &["task", "list"],
        &["task", "next"],
        &["matrix"],
    ] {
        run(&path, args);
    }
    writer.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn shipped_shell_scripts_match_live_command_schema_without_opening_a_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("must-not-exist.db");
    for (shell, name) in [("bash", "hs.bash"), ("zsh", "_hs"), ("fish", "hs.fish")] {
        let result = Command::new(env!("CARGO_BIN_EXE_hs"))
            .env("TZ", "Deliberately/Invalid")
            .arg("--db")
            .arg(&path)
            .args(["completions", shell])
            .output()
            .unwrap();
        assert!(result.status.success());
        let shipped = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("completions")
                .join(name),
        )
        .unwrap();
        assert_eq!(
            result.stdout, shipped,
            "Regenerate the {shell} completion with `hs completions {shell}`"
        );
    }
    assert!(!path.exists());
}
