use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
fn api(path: &Path, request: Value, tz: &str) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hs"))
        .env("TZ", tz)
        .arg("--db")
        .arg(path)
        .arg("api")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.success(), reply["ok"].as_bool().unwrap());
    reply
}
#[test]
fn receipts_retries_and_static_queries_do_not_depend_on_current_timezone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recovery.sqlite");
    let init = json!({"schema_version":1,"operation":"initialize","request_id":"initialize"});
    let created = api(&path, init.clone(), "UTC");
    assert_eq!(created["ok"], true);
    let request = json!({"schema_version":1,"operation":"task.add","request_id":"task","preconditions":{"database_id":created["data"]["context"]["database_id"]},"input":{"title":"Recover result after client configuration changed"}});
    let receipt = api(&path, request.clone(), "UTC");
    assert_eq!(receipt["ok"], true);
    assert_eq!(api(&path, request, "Invalid/Timezone"), receipt);
    assert_eq!(api(&path, init, "Invalid/Timezone"), created);
    let recovered = api(
        &path,
        json!({"schema_version":1,"operation":"receipt","input":{"request_id":"task"}}),
        "Invalid/Timezone",
    );
    assert_eq!(recovered["data"], receipt);
    for entity in ["tasks", "entries", "operations"] {
        let result = api(
            &path,
            json!({"schema_version":1,"operation":"query","input":{"entity":entity}}),
            "Invalid/Timezone",
        );
        assert_eq!(result["ok"], true, "{result}");
    }
    let new_write = api(
        &path,
        json!({"schema_version":1,"operation":"task.add","request_id":"new","preconditions":{"database_id":created["data"]["context"]["database_id"]},"input":{"title":"Cannot resolve clock"}}),
        "Invalid/Timezone",
    );
    assert_eq!(new_write["error"]["code"], "invalid_input");
    let tasks = api(
        &path,
        json!({"schema_version":1,"operation":"query","input":{"entity":"tasks"}}),
        "Invalid/Timezone",
    );
    assert_eq!(tasks["data"]["items"].as_array().unwrap().len(), 1);
}

#[test]
fn a_deadline_cannot_reopen_after_the_observed_pomodoro_clock_moves_back() {
    use chrono::{Duration, Utc};
    use hs::{Store, pomodoro, time::Moment};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rollback.sqlite");
    let initialized = api(
        &path,
        json!({"schema_version":1,"operation":"initialize","request_id":"init"}),
        "UTC",
    );
    let initial = &initialized["data"]["context"];
    let focus = api(
        &path,
        json!({"schema_version":1,"operation":"pomo.start","request_id":"focus",
        "preconditions":{"database_id":initial["database_id"],"timer_revision":initial["timer_revision"]},
        "input":{"activity":{"activity":"Design"}}}),
        "UTC",
    );
    let c = &focus["data"]["context"];
    let reference = Utc::now();
    let future = Moment::at(reference + Duration::minutes(2), chrono_tz::UTC);
    // This is a valid observation of a still-running 25-minute phase. The
    // next process sees the system wall clock back at the original time.
    let mut store = Store::open(&path).unwrap();
    let session = pomodoro::status(&mut store, &future).unwrap().unwrap();
    assert_eq!(session.last_observed(), future.utc);
    drop(store);
    let expired = Moment::at(reference + Duration::minutes(1), chrono_tz::UTC).utc;
    let mut pause = json!({"schema_version":1,"operation":"pomo.pause","request_id":"pause",
        "preconditions":{"database_id":c["database_id"],"timer_revision":c["timer_revision"],"before_utc":expired.to_rfc3339()}});
    for preview in [true, false] {
        pause["preview"] = json!(preview);
        let refused = api(&path, pause.clone(), "UTC");
        assert_eq!(refused["error"]["code"], "precondition_failed", "{refused}");
        assert_eq!(refused["error"]["details"]["field"], "before_utc");
        assert_eq!(
            refused["error"]["details"]["actual"],
            future.utc.to_rfc3339()
        );
    }
    let unchanged = api(
        &path,
        json!({"schema_version":1,"operation":"context"}),
        "UTC",
    );
    assert_eq!(unchanged["data"]["timer_revision"], c["timer_revision"]);
    assert_eq!(unchanged["data"]["pomodoro"]["state"], "running");
    pause["preconditions"]["before_utc"] = json!((future.utc + Duration::seconds(1)).to_rfc3339());
    let accepted = api(&path, pause, "UTC");
    assert_eq!(accepted["data"]["result"]["state"], "paused", "{accepted}");
}

#[test]
fn task_capture_and_edit_leave_an_unrelated_elapsed_focus_for_a_timer_command() {
    use chrono::{Duration, Utc};
    use hs::{Store, model::ActivityTemplate, pomodoro, time::Moment};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("unrelated.sqlite");
    let initialized = api(
        &path,
        json!({"schema_version":1,"operation":"initialize","request_id":"init"}),
        "UTC",
    );
    let db = initialized["data"]["context"]["database_id"].clone();
    let prior = Moment::at(Utc::now() - Duration::minutes(2), chrono_tz::UTC);
    let mut store = Store::open(&path).unwrap();
    let work = ActivityTemplate {
        activity: "Human focus".into(),
        category: None,
        description: String::new(),
        tags: vec![],
    };
    pomodoro::start(
        &mut store,
        work.starting_at(prior.local().unwrap()),
        pomodoro::Config {
            focus_seconds: 30,
            ..pomodoro::Config::default()
        },
        &prior,
    )
    .unwrap();
    drop(store);
    let original = api(
        &path,
        json!({"schema_version":1,"operation":"context"}),
        "UTC",
    )["data"]
        .clone();
    assert_eq!(original["pomodoro"]["reconciliation_due"], true);
    let mut capture = json!({"schema_version":1,"operation":"task.add","request_id":"capture",
        "preconditions":{"database_id":db},"input":{"title":"Call supplier"},"preview":true});
    let preview = api(&path, capture.clone(), "UTC");
    assert_eq!(preview["data"]["provisional"], true);
    assert_eq!(preview["data"]["entry_effects"], json!([]));
    assert_eq!(
        preview["data"]["commit_preconditions"]["timer_revision"],
        original["timer_revision"]
    );
    capture["preview"] = json!(false);
    capture["preconditions"] = preview["data"]["commit_preconditions"].clone();
    let created = api(&path, capture, "UTC");
    let task = &created["data"]["result"];
    let edited = api(
        &path,
        json!({"schema_version":1,"operation":"task.edit","request_id":"edit",
        "preconditions":{"database_id":db,"task_revision":task["revision"]},"input":{"id":task["id"],"title":"Call electrician"}}),
        "UTC",
    );
    for response in [&created, &edited] {
        assert_eq!(response["ok"], true, "{response}");
        let after = &response["data"]["context"];
        for field in [
            "timer_revision",
            "history_revision",
            "current_entry",
            "pomodoro",
        ] {
            assert_eq!(after[field], original[field], "{field}");
        }
    }
}

#[test]
fn damaged_or_misaddressed_receipts_cannot_return_success_or_repeat_the_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("receipts.sqlite");
    let initialized = api(
        &path,
        json!({"schema_version":1,"operation":"initialize","request_id":"init"}),
        "UTC",
    );
    let add = json!({"schema_version":1,"operation":"task.add","request_id":"task",
        "preconditions":{"database_id":initialized["data"]["context"]["database_id"]},"input":{"title":"Created once"}});
    let original = api(&path, add.clone(), "UTC");
    assert_eq!(original["ok"], true);
    let lookup = json!({"schema_version":1,"operation":"receipt","input":{"request_id":"task"}});
    let conn = rusqlite::Connection::open(&path).unwrap();
    let mut wrong_operation = original.clone();
    wrong_operation["operation"] = json!("tracking.start");
    for damaged in [json!(true), initialized.clone(), wrong_operation] {
        conn.execute(
            "UPDATE operation_receipts SET response_json=? WHERE request_id='task'",
            [damaged.to_string()],
        )
        .unwrap();
        for request in [&add, &lookup] {
            let failure = api(&path, request.clone(), "UTC");
            assert_eq!(failure["error"]["code"], "invalid_receipt", "{failure}");
            assert_eq!(failure["error"]["details"]["request_id"], "task");
        }
    }
    let count: i64 = conn
        .query_row("SELECT count(*) FROM tasks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn an_exact_old_request_can_recover_its_receipt_after_input_validation_tightens() {
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("v1-replay.sqlite");
    let i = api(
        &path,
        json!({"schema_version":1,"operation":"initialize","request_id":"init"}),
        "UTC",
    );
    let db = &i["data"]["context"]["database_id"];
    let t = api(
        &path,
        json!({"schema_version":1,"operation":"task.add","request_id":"add",
        "preconditions":{"database_id":db},"input":{"title":"Neighboring task"}}),
        "UTC",
    );
    let e = api(
        &path,
        json!({"schema_version":1,"operation":"tracking.start","request_id":"start",
        "preconditions":{"database_id":db,"timer_revision":i["data"]["context"]["timer_revision"]},
        "input":{"activity":{"activity":"Remove duplicate"}}}),
        "UTC",
    );
    let entry = &e["data"]["result"]["entry"];
    assert_eq!(entry["id"], t["data"]["result"]["id"]);
    let mut old = json!({"schema_version":1,"operation":"tracking.delete","request_id":"delete",
        "preconditions":{"database_id":db,"entry_revision":entry["revision"],"timer_revision":e["data"]["context"]["timer_revision"]},
        "input":{"id":entry["id"]}});
    let saved = api(&path, old.clone(), "UTC");
    assert_eq!(saved["ok"], true);
    // Early v1 allowed a task_revision referring to a same-numbered task.
    // Reconstruct that archived receipt hash without running an older executable.
    old["preconditions"]["task_revision"] = t["data"]["result"]["revision"].clone();
    let mut fields = old.clone();
    fields.as_object_mut().unwrap().remove("request_id");
    let hash = Sha256::digest(serde_json::to_vec(&fields).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE operation_receipts SET fingerprint=? WHERE request_id='delete'",
        [hash],
    )
    .unwrap();
    assert_eq!(api(&path, old.clone(), "Invalid/Timezone"), saved);
    // Changed fields or a fresh ID cannot use the older revision guard.
    old["request_id"] = json!("fresh");
    let invalid = api(&path, old.clone(), "UTC");
    assert_eq!(invalid["error"]["code"], "invalid_input", "{invalid}");
    old["request_id"] = json!("delete");
    old["preconditions"]["task_revision"] = json!(99);
    assert_eq!(api(&path, old, "UTC")["error"]["code"], "invalid_input");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM operation_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 4);
}
