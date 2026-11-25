use std::{path::Path, process::Command};

use hs::{
    Store,
    application::{tasks, workflows},
    domain::{
        ids::TaskId,
        model::ActivityTemplate,
        tasks::{Quadrant, TaskInput},
        time::Moment,
    },
};
use serde_json::{Value, json};
use unicode_width::UnicodeWidthStr;

fn run(db: &Path, width: &str, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_hs"))
        .env("TZ", "UTC")
        .env("COLUMNS", width)
        .env("TERM", "dumb")
        .arg("--db")
        .arg(db)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn position(text: &str, marker: &str) -> (usize, usize) {
    let matches: Vec<_> = text
        .lines()
        .enumerate()
        .filter_map(|(row, line)| line.find(marker).map(|column| (row, column)))
        .collect();
    assert_eq!(matches.len(), 1, "expected one {marker:?}: {text}");
    matches[0]
}

fn bounded(text: &str, width: usize) {
    assert!(!text.contains(['\x1b', '\x07', '\t', '\r']));
    for line in text.lines() {
        assert!(
            line.width() <= width,
            "line exceeds {width} cells ({}): {line:?}",
            line.width()
        );
    }
}

#[test]
fn tasks_occupy_the_cartesian_quadrants_and_inbox_is_outside_the_axes() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("matrix.db");
    for (title, quadrant) in [
        ("Plan strategy", "plan"),
        ("Fix outage", "do"),
        ("Discard distraction", "drop"),
        ("Delegate booking", "delegate"),
        ("Unclassified thought", "inbox"),
    ] {
        run(&db, "80", &["task", "add", title, "--quadrant", quadrant]);
    }
    let text = run(&db, "80", &["matrix"]);
    bounded(&text, 80);
    let plan = position(&text, "#1");
    let urgent = position(&text, "#2");
    let discard = position(&text, "#3");
    let delegate = position(&text, "#4");
    let inbox = position(&text, "#5");
    assert!(plan.1 < urgent.1, "PLAN must be left of DO: {text}");
    assert!(
        discard.1 < delegate.1,
        "DROP must be left of DELEGATE: {text}"
    );
    assert!(plan.0 < discard.0 && urgent.0 < delegate.0, "{text}");
    assert!(inbox.0 > discard.0 && inbox.0 > delegate.0, "{text}");
    assert!(text.contains("IMPORTANCE") && text.contains("URGENCY"));
    assert!(text.contains('^') && text.contains('>'));

    run(&db, "80", &["task", "done", "2"]);
    run(&db, "80", &["task", "archive", "3"]);
    let text = run(&db, "80", &["matrix"]);
    assert!(!text.contains("Fix outage") && !text.contains("Discard distraction"));
    assert!(text.contains("Plan strategy") && text.contains("Delegate booking"));
}

#[test]
fn long_unicode_titles_wrap_without_losing_content_or_emitting_control_sequences() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("unicode.db");
    let title = format!(
        "{} résumé e\u{301} 计划 🧑🏽‍💻 family 👨‍👩‍👧‍👦 END",
        "abcdefghijklmnopqrstuvwxyz0123456789".repeat(4)
    );
    run(&db, "80", &["task", "add", &title, "--quadrant", "plan"]);
    let compact_title: String = title.chars().filter(|c| !c.is_whitespace()).collect();
    for width in [24, 40, 60, 80, 120] {
        let text = run(&db, &width.to_string(), &["matrix"]);
        bounded(&text, width);
        // Read just the PLAN column in the side-by-side view so the other
        // quadrant's empty-state text does not interrupt the wrapped title.
        let title_column = if width >= 60 {
            text.lines()
                .filter_map(|line| line.strip_prefix('|')?.split('|').next())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            text.clone()
        };
        let compact_output: String = title_column
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '|')
            .collect();
        assert!(
            compact_output.contains(&compact_title),
            "lost title at {width} columns: {text}"
        );
        assert!(text.contains("🧑🏽‍💻") && text.contains("👨‍👩‍👧‍👦"));
        position(&text, "#1");
    }

    run(
        &db,
        "80",
        &["task", "add", "literal\x1b[31m text\nsecond\tcolumn\x07"],
    );
    let text = run(&db, "80", &["matrix"]);
    bounded(&text, 80);
    assert!(text.contains("\\u{1b}[31m"));
    assert!(text.contains("\\nsecond\\tcolumn\\u{7}"));
}

#[test]
fn due_dates_recorded_work_and_active_pomodoro_survive_the_compact_view() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("metrics.db");
    let start = Moment::at("2025-01-02T12:00:00Z".parse().unwrap(), chrono_tz::UTC);
    let end = Moment::at("2025-01-02T12:02:00Z".parse().unwrap(), chrono_tz::UTC);
    let mut store = Store::open(&db).unwrap();
    let task = tasks::add(
        &mut store,
        TaskInput {
            title: "Review deadline".into(),
            activity: ActivityTemplate {
                activity: "Reviewing".into(),
                category: None,
                description: String::new(),
                tags: vec![],
            },
            quadrant: Some(Quadrant::Plan),
            due_date: Some("2000-01-02".parse().unwrap()),
        },
        &start,
    )
    .unwrap();
    workflows::start_task(&mut store, TaskId::new(task.id).unwrap(), &start).unwrap();
    workflows::stop_at(&mut store, end.local().unwrap(), &end).unwrap();
    drop(store);
    let text = run(&db, "120", &["matrix"]);
    assert!(text.contains("2000-01-02 overdue"));
    assert!(text.contains("0h 02m"), "{text}");
    run(
        &db,
        "80",
        &["pomo", "start", "--task", "1", "--focus", "1h"],
    );
    assert!(run(&db, "120", &["matrix"]).contains("focus running"));
    run(&db, "80", &["pomo", "pause"]);
    assert!(run(&db, "120", &["matrix"]).contains("focus paused"));
}

#[test]
fn empty_matrix_and_machine_output_remain_useful_at_every_width() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("empty.db");
    for width in [24, 40, 60, 80, 120] {
        let text = run(&db, &width.to_string(), &["matrix"]);
        bounded(&text, width);
        for heading in ["PLAN", "DO", "DROP", "DELEGATE", "INBOX"] {
            assert!(text.contains(heading), "missing {heading}: {text}");
        }
    }
    let title = "Original\nJSON text 👩‍💻";
    run(&db, "80", &["task", "add", title, "--quadrant", "plan"]);
    let mut expected: Value = serde_json::from_str(&run(&db, "80", &["--json", "matrix"])).unwrap();
    assert_eq!(expected["schema_version"], 1);
    assert_eq!(expected["kind"], "matrix");
    assert_eq!(expected["quadrants"][1]["tasks"][0]["title"], title);
    assert_eq!(
        expected["quadrants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| q["key"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("do"),
            json!("plan"),
            json!("delegate"),
            json!("drop"),
            json!("inbox")
        ]
    );
    expected.as_object_mut().unwrap().remove("as_of_utc");
    for width in ["1", "24", "invalid", "999999999999999999999999"] {
        let mut actual: Value =
            serde_json::from_str(&run(&db, width, &["--json", "matrix"])).unwrap();
        actual.as_object_mut().unwrap().remove("as_of_utc");
        assert_eq!(actual, expected, "JSON changed with COLUMNS={width}");
    }
}
