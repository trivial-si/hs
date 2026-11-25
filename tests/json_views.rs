use std::{
    path::Path,
    process::{Command, Output},
};

use serde_json::{Value, json};

fn run(path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hs"))
        .env("TZ", "UTC")
        .arg("--db")
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}

fn success(path: &Path, args: &[&str]) -> String {
    let output = run(path, args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn view(path: &Path, args: &[&str], kind: &str) -> Value {
    let mut args_with_format = vec!["--json"];
    args_with_format.extend_from_slice(args);
    let response: Value = serde_json::from_str(&success(path, &args_with_format)).unwrap();
    assert_eq!(response["schema_version"], 1);
    assert_eq!(response["kind"], kind);
    response
}

#[test]
fn task_and_timer_views_publish_one_consistent_machine_readable_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hs.db");
    assert!(success(&path, &["status"]).contains("No activity is running."));
    assert!(view(&path, &["status"], "status")["current"].is_null());
    assert!(view(&path, &["pomo", "status"], "pomo.status")["session"].is_null());
    assert_eq!(
        view(&path, &["task", "next"], "task.next")["action"],
        "triage"
    );
    let title = "Review \"résumé\"";
    success(
        &path,
        &[
            "task",
            "add",
            title,
            "--activity",
            "Writing@Work, check figures, #finance",
            "--quadrant",
            "plan",
            "--due",
            "2000-01-01",
        ],
    );
    let list = view(&path, &["task", "list"], "task.list");
    let task = &list["tasks"][0];
    assert_eq!(task["title"], title);
    assert_eq!(task["quadrant"], "plan");
    assert_eq!(task["recorded_seconds"], 0);
    assert_eq!(task["overdue"], true);
    assert_eq!(task["tags"], json!(["finance"]));
    assert_eq!(
        &view(&path, &["task", "show", "1"], "task.show")["task"],
        task
    );
    let matrix = view(&path, &["matrix"], "matrix");
    let quadrants = matrix["quadrants"].as_array().unwrap();
    assert_eq!(
        quadrants.iter().map(|q| &q["key"]).collect::<Vec<_>>(),
        ["do", "plan", "delegate", "drop", "inbox"]
    );
    assert_eq!(&quadrants[1]["tasks"][0], task);
    assert_eq!(
        view(&path, &["task", "next"], "task.next")["action"],
        "start"
    );

    success(&path, &["pomo", "start", "--task", "1", "--focus", "10m"]);
    let status = view(&path, &["status"], "status");
    let session = &status["pomodoro"];
    assert_eq!(session["state"], "running");
    assert_eq!(session["phase"], "focus");
    assert_eq!(session["entry_id"], status["current"]["id"]);
    assert_eq!(status["current"]["task_id"], 1);
    assert_eq!(status["current"]["quadrant"], "plan");
    let pomo = view(&path, &["pomo", "status"], "pomo.status");
    assert_eq!(pomo["session"]["deadline_utc"], session["deadline_utc"]);
    assert!(
        pomo["session"]["remaining_seconds"]
            .as_i64()
            .is_some_and(|s| (1..=600).contains(&s))
    );
    assert_eq!(
        view(&path, &["task", "show", "1"], "task.show")["task"]["timer"]["kind"],
        "pomodoro"
    );
    assert_eq!(
        view(&path, &["task", "next"], "task.next")["action"],
        "pomo_watch"
    );
    success(&path, &["pomo", "pause"]);
    assert_eq!(
        view(&path, &["task", "next"], "task.next")["action"],
        "pomo_resume"
    );
    success(&path, &["task", "done", "1"]);
    assert_eq!(
        view(&path, &["task", "list"], "task.list")["tasks"],
        json!([])
    );
    assert_eq!(
        view(&path, &["task", "list", "--all"], "task.list")["tasks"][0]["status"],
        "done"
    );
    let stats = view(&path, &["pomo", "stats"], "pomo.stats");
    assert_eq!(stats["interrupted_focus"], 1);
    assert_eq!(stats["completed_focus"], 0);
    assert!(stats["focus_seconds"].is_i64());
    assert_eq!(stats["break_seconds"], 0);
}

#[test]
fn reports_publish_clipped_totals_and_keep_existing_output_modes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hs.db");
    success(&path, &["add", "2025-01-03 23:50-00:15 Review@Work"]);
    let report = view(
        &path,
        &[
            "report",
            "--from",
            "2025-01-04",
            "--to",
            "2025-01-04",
            "--group-by",
            "day",
        ],
        "report",
    );
    assert_eq!(report["total_seconds"], 900);
    assert_eq!(report["total_minutes"], 15);
    assert_eq!(
        report["groups"],
        json!([{"name":"2025-01-04","seconds":900}])
    );
    assert_eq!(report["unresolved_ids"], json!([]));
    assert_eq!(report["range"]["start"], "2025-01-04 00:00:00");
    assert!(success(&path, &["report", "--all"]).contains("Total\t0h 25m"));
    let export: Value =
        serde_json::from_str(&success(&path, &["export", "--all", "--format", "json"])).unwrap();
    assert_eq!(export["entries"][0]["seconds"], 1500);
    assert_eq!(export["schema_version"], 1);
    assert!(export.get("kind").is_none());
}

#[test]
fn format_errors_are_structured_and_precede_mutations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("never-created.db");
    for (args, code) in [
        (
            vec!["task", "add", "Should not exist", "--json"],
            "invalid_input",
        ),
        (vec!["--json", "pomo", "start", "Work"], "invalid_input"),
        (
            vec!["--json", "task", "show", "not-an-id"],
            "invalid_arguments",
        ),
    ] {
        let output = run(&path, &args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let err: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(err["schema_version"], 1);
        assert_eq!(err["kind"], "error");
        assert_eq!(err["error"]["kind"], "input");
        assert_eq!(err["error"]["code"], code);
        assert!(!err["error"]["message"].as_str().unwrap().is_empty());
        assert!(!path.exists());
    }
    let missing = run(&path, &["task", "show", "99", "--json"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stdout.is_empty());
    let error: Value = serde_json::from_slice(&missing.stderr).unwrap();
    assert!(error["error"]["message"].as_str().unwrap().contains("99"));
}

#[test]
fn json_respects_literal_positional_values_and_help_and_reports_clock_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hs.db");
    for flag in ["--help", "--version"] {
        let out = success(&path, &["--json", flag]);
        assert!(out.contains("hs"));
        assert!(!path.exists());
    }
    let invalid_zone = Command::new(env!("CARGO_BIN_EXE_hs"))
        .env("TZ", "Wrong/Timezone")
        .arg("--db")
        .arg(&path)
        .args(["status", "--json"])
        .output()
        .unwrap();
    assert_eq!(invalid_zone.status.code(), Some(2));
    assert!(invalid_zone.stdout.is_empty());
    let error: Value = serde_json::from_slice(&invalid_zone.stderr).unwrap();
    assert_eq!(error["error"]["code"], "invalid_input");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Wrong/Timezone")
    );
    assert!(!path.exists());

    let ordinary = success(&path, &["start", "--", "--json"]);
    assert!(ordinary.contains("Started entry"));
    assert_eq!(
        view(&path, &["status"], "status")["current"]["activity"],
        "--json"
    );
}
