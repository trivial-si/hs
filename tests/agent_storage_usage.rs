use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn api(path: &Path, mut request: Value) -> Value {
    request["schema_version"] = json!(1);
    let mut child = Command::new(env!("CARGO_BIN_EXE_hs"))
        .args(["--db"])
        .arg(path)
        .arg("api")
        .env("TZ", "UTC")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    let response: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(result.status.success(), "{response}");
    response["data"].clone()
}

#[test]
fn unicode_titles_and_tags_match_on_filtered_task_and_entry_pages() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("usage.sqlite");
    let context =
        api(&db, json!({"operation":"initialize","request_id":"init"}))["context"].clone();
    let task_pre = json!({"database_id":context["database_id"]});
    for (n, title) in ["Unrelated", "Žuti nacrt", "Another match"]
        .iter()
        .enumerate()
    {
        let tags = if n == 0 { vec![] } else { vec!["ŠIFRA"] };
        api(
            &db,
            json!({"operation":"task.add","request_id":format!("task-{n}"),"preconditions":task_pre,
                        "input":{"title":title,"activity":{"activity":"Planning","tags":tags}}}),
        );
    }
    let first = api(
        &db,
        json!({"operation":"query","input":{"entity":"tasks","search":"šifra","limit":1}}),
    );
    assert_eq!(first["items"][0]["id"], 2);
    assert_eq!(first["next_after_id"], 2);
    let next = api(
        &db,
        json!({"operation":"query","input":{"entity":"tasks","search":"šifra","after_id":2,"limit":1}}),
    );
    assert_eq!(next["items"][0]["id"], 3);
    assert!(next["next_after_id"].is_null());
    let title = api(
        &db,
        json!({"operation":"query","input":{"entity":"tasks","search":"žutI"}}),
    );
    assert_eq!(title["items"].as_array().unwrap().len(), 1);
    assert_eq!(title["items"][0]["id"], 2);

    api(
        &db,
        json!({"operation":"tracking.add","request_id":"history",
        "preconditions":{"database_id":context["database_id"],"timer_revision":context["timer_revision"],
                         "history_revision":context["history_revision"]},
        "input":{"start":"2024-01-01T10:00:00Z","end":"2024-01-01T11:00:00Z",
                 "activity":{"activity":"Čitanje","tags":["ŽURNO"]}}}),
    );
    for search in ["žurno", "čitanje"] {
        let page = api(
            &db,
            json!({"operation":"query","input":{"entity":"entries","search":search}}),
        );
        assert_eq!(page["items"].as_array().unwrap().len(), 1, "{search}");
        assert_eq!(page["items"][0]["id"], 1);
    }
}

#[test]
fn unchanged_tags_preserve_task_entry_and_timer_tokens_through_human_edits() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("unchanged.sqlite");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_hs"))
            .arg("--db")
            .arg(&db)
            .args(args)
            .env("TZ", "UTC")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let context = || api(&db, json!({"operation":"context"}));
    let task =
        || api(&db, json!({"operation":"query","input":{"entity":"tasks"}}))["items"][0].clone();
    let initial =
        api(&db, json!({"operation":"initialize","request_id":"init"}))["context"].clone();
    api(
        &db,
        json!({"operation":"task.add","request_id":"capture","preconditions":{"database_id":initial["database_id"]},
                   "input":{"title":"Draft","activity":{"activity":"Planning","tags":["blue","red"]}}}),
    );
    let original = task();
    run(&["task", "set", "1", "--title", "Draft", "--tags", "red,blue"]);
    assert_eq!(task(), original);
    run(&["task", "set", "1", "--tags", "green,red"]);
    let changed = task();
    assert_eq!(changed["tags"], json!(["green", "red"]));
    assert!(changed["revision"].as_i64() > original["revision"].as_i64());
    run(&["task", "set", "1", "--tags", ""]);
    assert_eq!(task()["tags"], json!([]));

    run(&["start", "Work, Review #blue #red"]);
    let before = context();
    assert_eq!(before["current_entry"]["tags"], json!(["blue", "red"]));
    run(&["edit", "1", "--tags", "red,blue"]);
    let after = context();
    for field in ["current_entry", "history_revision", "timer_revision"] {
        assert_eq!(before[field], after[field], "{field}");
    }
    run(&[
        "edit",
        "1",
        "--description",
        "New note",
        "--tags",
        "red,green",
    ]);
    let edited = context();
    assert_eq!(edited["current_entry"]["tags"], json!(["green", "red"]));
    assert_eq!(edited["current_entry"]["description"], "New note");
    for field in ["history_revision", "timer_revision"] {
        assert!(edited[field].as_i64() > after[field].as_i64(), "{field}");
    }
    run(&["edit", "1", "--tags", ""]);
    assert_eq!(context()["current_entry"]["tags"], json!([]));
}
