use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::{Value, json};

fn command(path: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hs"));
    cmd.env("TZ", "UTC").arg("--db").arg(path);
    cmd
}

fn api(path: &Path, request: &Value) -> Value {
    let mut child = command(path)
        .arg("api")
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
    let output = child.wait_with_output().unwrap();
    let json: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("No API envelope for {request}: {output:?}"));
    assert_eq!(json["schema_version"], 1);
    assert_eq!(
        Some(output.status.success()),
        json["ok"].as_bool(),
        "{json}"
    );
    json
}

fn read(path: &Path, operation: &str, input: Value) -> Value {
    let request = json!({"schema_version":1,"operation":operation,"input":input});
    let response = api(path, &request);
    assert_eq!(response["ok"], true, "{response}");
    response["data"].clone()
}

fn context(path: &Path) -> Value {
    read(path, "context", json!({}))
}

fn write_request(op: &str, request_id: &str, input: Value, conditions: Value) -> Value {
    let mut request = json!({"schema_version":1,"operation":op,"request_id":request_id,
        "actor":"integration-test","session":"test-session","input":input});
    if op != "initialize" {
        request["preconditions"] = conditions;
    }
    request
}

fn commit(path: &Path, request: &Value) -> Value {
    let response = api(path, request);
    assert_eq!(response["ok"], true, "{request} -> {response}");
    response
}

fn initialize(path: &Path) -> Value {
    let result = commit(
        path,
        &write_request("initialize", "init", json!({}), Value::Null),
    );
    assert_eq!(result["data"]["result"]["initialized"], true);
    result["data"]["context"].clone()
}

fn timer_token(context: &Value) -> Value {
    json!({"database_id":context["database_id"],"timer_revision":context["timer_revision"]})
}

fn human(path: &Path, args: &[&str]) {
    let output = command(path).args(args).output().unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
}

fn query(path: &Path, input: Value) -> Value {
    read(path, "query", input)
}

fn receipt_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM operation_receipts", [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn wait_for_deadline(value: &Value) {
    let deadline: DateTime<Utc> = value.as_str().unwrap().parse().unwrap();
    let start = Instant::now();
    while Utc::now() < deadline {
        assert!(start.elapsed() < Duration::from_secs(8));
        thread::sleep(Duration::from_millis(10));
    }
}

fn data_version(conn: &Connection) -> i64 {
    conn.query_row("PRAGMA data_version", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn receipts_replay_original_response_and_preview_leaves_the_real_profile_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("fresh.sqlite");
    let missing = api(&path, &json!({"schema_version":1,"operation":"context"}));
    assert_eq!(missing["ok"], false);
    assert!(!path.exists());
    let initial = initialize(&path);
    let observer = Connection::open(&path).unwrap();
    let version = data_version(&observer);
    let token = json!({"database_id":initial["database_id"]});
    let create = write_request(
        "task.add",
        "capture",
        json!({"title":"Build"}),
        token.clone(),
    );
    let mut preview = create.clone();
    preview["preview"] = json!(true);
    let proposed = commit(&path, &preview);
    assert_eq!(proposed["data"]["preview"], true);
    assert_eq!(proposed["data"]["result"]["title"], "Build");
    assert_eq!(data_version(&observer), version);
    assert_eq!(receipt_count(&observer), 1);
    assert_eq!(query(&path, json!({"entity":"tasks"}))["items"], json!([]));
    let actual = commit(&path, &create);
    let id = actual["data"]["result"]["id"].as_i64().unwrap();
    human(
        &path,
        &["task", "set", &id.to_string(), "--title", "Human change"],
    );
    let replay = commit(&path, &create);
    assert_eq!(replay, actual);
    assert_eq!(
        read(&path, "receipt", json!({"request_id":"capture"})),
        actual
    );
    let mut conflicting = create;
    conflicting["input"]["title"] = json!("Another title");
    assert_eq!(
        api(&path, &conflicting)["error"]["code"],
        "request_id_conflict"
    );
    assert_eq!(receipt_count(&observer), 2);
    assert_eq!(
        query(&path, json!({"entity":"tasks"}))["items"][0]["title"],
        "Human change"
    );
}

#[test]
fn stale_task_or_timer_tokens_cannot_replace_a_human_timer_or_control_a_new_focus() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ownership.sqlite");
    let idle = initialize(&path);
    let task = commit(
        &path,
        &write_request(
            "task.add",
            "create",
            json!({"title":"A task"}),
            json!({"database_id":idle["database_id"]}),
        ),
    )["data"]["result"]
        .clone();
    let id = task["id"].as_i64().unwrap();
    let mut stale_token = timer_token(&idle);
    stale_token["task_revision"] = task["revision"].clone();
    let stale = write_request(
        "tracking.start_task",
        "task-start",
        json!({"id":id}),
        stale_token.clone(),
    );
    human(&path, &["start", "Human work"]);
    assert_eq!(api(&path, &stale)["error"]["code"], "precondition_failed");
    let working = context(&path);
    assert_eq!(working["current_entry"]["activity"], "Human work");
    human(
        &path,
        &["task", "set", &id.to_string(), "--title", "Revised"],
    );
    let mut stale_task = stale;
    stale_task["preconditions"]["timer_revision"] = working["timer_revision"].clone();
    assert_eq!(
        api(&path, &stale_task)["error"]["code"],
        "precondition_failed"
    );
    let task = query(&path, json!({"entity":"tasks"}))["items"][0].clone();
    let mut authorized = timer_token(&working);
    authorized["task_revision"] = task["revision"].clone();
    let focus = commit(
        &path,
        &write_request("pomo.start_task", "focus", json!({"id":id}), authorized),
    );
    assert_eq!(focus["data"]["result"]["task_id"], id);
    let focus_id = focus["data"]["result"]["id"].as_i64().unwrap();
    let stale_stop = write_request(
        "pomo.stop",
        "old-stop",
        json!({}),
        timer_token(&focus["data"]["context"]),
    );
    human(&path, &["pomo", "stop"]);
    human(&path, &["pomo", "start", "Another focus"]);
    assert_eq!(
        api(&path, &stale_stop)["error"]["code"],
        "precondition_failed"
    );
    let current = context(&path);
    assert_eq!(current["pomodoro"]["activity"], "Another focus");
    assert_ne!(current["pomodoro"]["id"], focus_id);
}

#[test]
fn reading_or_previewing_an_expired_focus_leaves_its_deadline_for_a_committed_transition() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("expiry.sqlite");
    let idle = initialize(&path);
    let start = write_request(
        "pomo.start",
        "one-second",
        json!({"activity":{"activity":"Focus"},"config":{"focus_seconds":1}}),
        timer_token(&idle),
    );
    let running = commit(&path, &start);
    let deadline: DateTime<Utc> = running["data"]["result"]["deadline_utc"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let wait_started = Instant::now();
    while Utc::now() < deadline {
        assert!(wait_started.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(10));
    }
    let observer = Connection::open(&path).unwrap();
    let version = data_version(&observer);
    let expired = context(&path);
    assert_eq!(expired["pomodoro"]["state"], "running");
    assert_eq!(expired["pomodoro"]["reconciliation_due"], true);
    assert_eq!(expired["pomodoro"]["remaining_seconds"], 0);
    assert_eq!(
        expired["timer_revision"],
        running["data"]["context"]["timer_revision"]
    );
    assert_eq!(
        query(&path, json!({"entity":"entries"}))["items"][0]["running"],
        true
    );
    let mut pause = write_request("pomo.pause", "finish", json!({}), timer_token(&expired));
    pause["preview"] = json!(true);
    let hypothetical = commit(&path, &pause);
    assert_eq!(hypothetical["data"]["result"]["state"], "ready");
    assert_eq!(data_version(&observer), version);
    assert_eq!(receipt_count(&observer), 2);
    pause.as_object_mut().unwrap().remove("preview");
    let completed = commit(&path, &pause);
    assert_eq!(completed["data"]["result"]["state"], "ready");
    assert_eq!(completed["data"]["result"]["completed_focus"], 1);
    let entry = query(&path, json!({"entity":"entries"}))["items"][0].clone();
    assert_eq!(entry["running"], false);
    assert_eq!(entry["end_utc"], running["data"]["result"]["deadline_utc"]);
    assert_eq!(receipt_count(&observer), 3);
}

#[test]
fn task_patches_preserve_unspecified_fields_and_revisions_protect_status_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("patch.sqlite");
    let idle = initialize(&path);
    let add = write_request(
        "task.add",
        "capture",
        json!({"title":"Design review",
        "activity":{"activity":"Design","category":"Product","description":"Check the model","tags":["ui,ux"]},
        "quadrant":"plan","due_date":"2027-01-03"}),
        json!({"database_id":idle["database_id"]}),
    );
    let original = commit(&path, &add)["data"]["result"].clone();
    let id = original["id"].as_i64().unwrap();
    let edit = write_request(
        "task.edit",
        "rename",
        json!({"id":id,"activity":{"activity":"Review"}}),
        json!({"database_id":idle["database_id"],"task_revision":original["revision"]}),
    );
    let updated = commit(&path, &edit)["data"]["result"].clone();
    for field in [
        "title",
        "category",
        "description",
        "tags",
        "quadrant",
        "due_date",
    ] {
        assert_eq!(updated[field], original[field], "{field}");
    }
    let mut timer = timer_token(&idle);
    timer["task_revision"] = original["revision"].clone();
    let mut done = write_request(
        "task.status",
        "done",
        json!({"id":id,"status":"done"}),
        timer,
    );
    assert_eq!(api(&path, &done)["error"]["code"], "precondition_failed");
    let clear = write_request(
        "task.edit",
        "clear",
        json!({"id":id,"activity":{"activity":"Review","category":null,"description":"","tags":[]},"due_date":null,"quadrant":null}),
        json!({"database_id":idle["database_id"],"task_revision":updated["revision"]}),
    );
    let cleared = commit(&path, &clear)["data"]["result"].clone();
    assert!(cleared["category"].is_null());
    assert_eq!(cleared["description"], "");
    assert_eq!(cleared["tags"], json!([]));
    assert!(cleared["due_date"].is_null());
    assert!(cleared["quadrant"].is_null());
    done["preconditions"]["task_revision"] = cleared["revision"].clone();
    let closed = commit(&path, &done)["data"]["result"].clone();
    assert_eq!(closed["status"], "done");
    assert_eq!(
        query(&path, json!({"entity":"tasks","status":"open"}))["items"],
        json!([])
    );
    assert_eq!(
        query(&path, json!({"entity":"tasks","status":"done"}))["items"][0],
        closed
    );
}

fn history_token(c: &Value) -> Value {
    let mut token = timer_token(c);
    token["history_revision"] = c["history_revision"].clone();
    token
}

#[test]
fn history_pages_and_edits_preserve_tag_boundaries_and_detect_changes_to_neighbors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let mut state = initialize(&path);
    let mut ids = Vec::new();
    for (i, start, end) in [
        (0, "09:00", "10:00"),
        (1, "11:00", "12:00"),
        (2, "13:00", "14:00"),
    ] {
        let request = write_request(
            "tracking.add",
            &format!("entry-{i}"),
            json!({"activity":{"activity":format!("Work {i}"),"tags":["history"]},
            "start":format!("2025-01-02T{start}:00Z"),"end":format!("2025-01-02T{end}:00Z"),"zone":"UTC"}),
            history_token(&state),
        );
        let result = commit(&path, &request);
        ids.push(result["data"]["result"]["entry"]["id"].as_i64().unwrap());
        state = result["data"]["context"].clone();
    }
    let first = query(
        &path,
        json!({"entity":"entries","search":"history","limit":2}),
    );
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    assert_eq!(first["items"][0]["id"], ids[0]);
    assert_eq!(first["items"][1]["id"], ids[1]);
    let second = query(
        &path,
        json!({"entity":"entries","search":"history","limit":2,"after_id":first["next_after_id"]}),
    );
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["items"][0]["id"], ids[2]);
    assert!(second["next_after_id"].is_null());
    let mut conditions = history_token(&state);
    conditions["entry_revision"] = first["items"][0]["revision"].clone();
    let mut edit = write_request(
        "tracking.edit",
        "edit",
        json!({"id":ids[0],"end":"2025-01-02T11:30:00Z","tags":["a,b","Žuta oznaka"]}),
        conditions,
    );
    let mut preview = edit.clone();
    preview["preview"] = json!(true);
    let effects = commit(&path, &preview);
    assert_eq!(
        effects["data"]["result"]["adjusted_entry_ids"],
        json!([ids[1]])
    );
    human(
        &path,
        &[
            "edit",
            &ids[1].to_string(),
            "--description",
            "A later human edit",
        ],
    );
    assert_eq!(api(&path, &edit)["error"]["code"], "precondition_failed");
    let fresh = context(&path);
    assert_eq!(fresh["timer_revision"], state["timer_revision"]);
    assert_ne!(fresh["history_revision"], state["history_revision"]);
    edit["preconditions"]["history_revision"] = fresh["history_revision"].clone();
    let changed = commit(&path, &edit);
    assert_eq!(
        changed["data"]["result"]["entry"]["tags"],
        json!(["a,b", "Žuta oznaka"])
    );
    let search = query(&path, json!({"entity":"entries","search":"a,b"}));
    assert_eq!(search["items"].as_array().unwrap().len(), 1);
    let mut conditions = timer_token(&changed["data"]["context"]);
    conditions["entry_revision"] = changed["data"]["result"]["entry"]["revision"].clone();
    let deleted = commit(
        &path,
        &write_request(
            "tracking.delete",
            "delete",
            json!({"id":ids[0]}),
            conditions,
        ),
    );
    assert_eq!(deleted["data"]["result"]["deleted_entry_id"], ids[0]);
    assert_eq!(
        query(&path, json!({"entity":"entries","search":"a,b"}))["items"],
        json!([])
    );
}

#[test]
fn two_clients_with_the_same_request_create_one_task_and_a_false_idle_claim_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("concurrent.sqlite");
    let c = initialize(&path);
    let req = write_request(
        "task.add",
        "shared",
        json!({"title":"Only once"}),
        json!({"database_id":c["database_id"]}),
    );
    let (a, b) = thread::scope(|s| {
        let a = s.spawn(|| api(&path, &req));
        let b = s.spawn(|| api(&path, &req));
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!(a["ok"], true, "{a}");
    assert_eq!(a, b);
    assert_eq!(
        query(&path, json!({"entity":"tasks"}))["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let mut token = timer_token(&c);
    token["timer_idle"] = json!(false);
    let stop = write_request("tracking.stop", "stop-if-running", json!({}), token);
    assert_eq!(api(&path, &stop)["error"]["code"], "precondition_failed");
}

#[test]
fn explicit_commands_can_switch_pause_and_resume_without_losing_actor_or_work_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("commands.sqlite");
    let idle = initialize(&path);
    let mut token = timer_token(&idle);
    token["timer_idle"] = json!(true);
    let tracked = commit(
        &path,
        &write_request(
            "tracking.start",
            "track",
            json!({
                "activity":{"activity":"Manual","tags":["one,two"]}
            }),
            token,
        ),
    );
    let entry = &tracked["data"]["result"]["entry"];
    assert_eq!(entry["running"], true);
    assert!(entry["start_utc"].is_string());
    let saved = commit(
        &path,
        &write_request(
            "tracking.stop",
            "stop",
            json!({}),
            timer_token(&tracked["data"]["context"]),
        ),
    );
    let stopped = &saved["data"]["result"]["entry"];
    assert_eq!(stopped["id"], entry["id"]);
    assert!(stopped["end_utc"].is_string());
    let mut token = timer_token(&saved["data"]["context"]);
    token["entry_revision"] = stopped["revision"].clone();
    let resumed = commit(
        &path,
        &write_request(
            "tracking.resume",
            "resume-entry",
            json!({"id":entry["id"]}),
            token,
        ),
    );
    assert_ne!(resumed["data"]["result"]["entry"]["id"], entry["id"]);
    assert_eq!(
        resumed["data"]["result"]["entry"]["tags"],
        json!(["one,two"])
    );
    let capture = commit(
        &path,
        &write_request(
            "task.add",
            "task",
            json!({"title":"Client work"}),
            json!({"database_id":idle["database_id"]}),
        ),
    );
    let task = &capture["data"]["result"];
    let mut token = timer_token(&capture["data"]["context"]);
    token["task_revision"] = task["revision"].clone();
    let task_work = commit(
        &path,
        &write_request(
            "tracking.start_task",
            "task-work",
            json!({"id":task["id"]}),
            token,
        ),
    );
    let task_entry = &task_work["data"]["result"]["entry"];
    assert_eq!(task_entry["task_id"], task["id"]);
    assert_eq!(task_entry["running"], true);
    let focus = commit(
        &path,
        &write_request(
            "pomo.start",
            "pomo",
            json!({"activity":{"activity":"Focus"}}),
            timer_token(&task_work["data"]["context"]),
        ),
    );
    let paused = commit(
        &path,
        &write_request(
            "pomo.pause",
            "pause",
            json!({}),
            timer_token(&focus["data"]["context"]),
        ),
    );
    assert_eq!(paused["data"]["result"]["state"], "paused");
    assert!(paused["data"]["context"]["current_entry"].is_null());
    let continued = commit(
        &path,
        &write_request(
            "pomo.resume",
            "continue",
            json!({}),
            timer_token(&paused["data"]["context"]),
        ),
    );
    assert_ne!(
        continued["data"]["result"]["entry_id"],
        focus["data"]["result"]["entry_id"]
    );
    assert_eq!(
        continued["data"]["result"]["phase_id"],
        focus["data"]["result"]["phase_id"]
    );
    let next = commit(
        &path,
        &write_request(
            "pomo.next",
            "next",
            json!({}),
            timer_token(&continued["data"]["context"]),
        ),
    );
    assert_eq!(
        next["data"]["result"]["entry_id"],
        continued["data"]["result"]["entry_id"]
    );
    let done = commit(
        &path,
        &write_request(
            "pomo.stop",
            "finish",
            json!({}),
            timer_token(&next["data"]["context"]),
        ),
    );
    assert_eq!(done["data"]["context"]["timer_idle"], true);
    let history = query(&path, json!({"entity":"entries","task_id":task["id"]}));
    assert_eq!(history["items"].as_array().unwrap().len(), 1);
    assert_eq!(history["items"][0]["id"], task_entry["id"]);
    let recent = query(&path, json!({"entity":"operations","limit":2}));
    for (row, request_id) in recent["items"]
        .as_array()
        .unwrap()
        .iter()
        .zip(["finish", "next"])
    {
        assert_eq!(row["request_id"], request_id);
        assert_eq!(row["actor"], "integration-test");
        assert_eq!(row["session"], "test-session");
    }
}

#[test]
fn failure_to_persist_a_receipt_restores_the_original_timer_and_open_task() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("atomic.sqlite");
    let empty = initialize(&path);
    let task = commit(
        &path,
        &write_request(
            "task.add",
            "capture",
            json!({"title":"Owned task"}),
            json!({"database_id":empty["database_id"]}),
        ),
    )["data"]["result"]
        .clone();
    let mut conditions = timer_token(&empty);
    conditions["task_revision"] = task["revision"].clone();
    let active = commit(
        &path,
        &write_request(
            "pomo.start_task",
            "start",
            json!({"id":task["id"]}),
            conditions,
        ),
    );
    let before = active["data"]["context"].clone();
    let old_entry = before["current_entry"].clone();
    let old_session = before["pomodoro"]["id"].clone();
    let mut token = timer_token(&before);
    token["task_revision"] = task["revision"].clone();
    let done = write_request(
        "task.status",
        "atomic",
        json!({"id":task["id"],"status":"done"}),
        token,
    );
    let observer = Connection::open(&path).unwrap();
    observer.execute_batch("CREATE TRIGGER reject_receipt BEFORE INSERT ON operation_receipts BEGIN SELECT RAISE(ABORT, 'simulate persistence failure'); END;").unwrap();
    let old_version = data_version(&observer);
    let response = api(&path, &done);
    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(data_version(&observer), old_version);
    let restored = context(&path);
    assert_eq!(restored["current_entry"], old_entry);
    assert_eq!(restored["timer_revision"], before["timer_revision"]);
    assert_eq!(restored["history_revision"], before["history_revision"]);
    assert_eq!(restored["pomodoro"]["id"], old_session);
    assert_eq!(restored["pomodoro"]["state"], "running");
    assert_eq!(restored["open_tasks"]["items"], json!([task]));
    assert_eq!(receipt_count(&observer), 3);
    observer
        .execute_batch("DROP TRIGGER reject_receipt")
        .unwrap();
    let retried = commit(&path, &done);
    assert_eq!(retried["data"]["result"]["status"], "done");
    assert_eq!(retried["data"]["context"]["timer_idle"], true);
    assert_eq!(receipt_count(&observer), 4);
}

#[test]
fn preview_shows_every_row_of_a_split_with_its_actual_before_and_after_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("split.sqlite");
    let empty = initialize(&path);
    let add = write_request(
        "tracking.add",
        "design",
        json!({
            "activity":{"activity":"Design","category":"Product","description":"Draft","tags":["ui,ux"]},
            "start":"2025-01-02T09:00:00Z","end":"2025-01-02T12:00:00Z","zone":"UTC"
        }),
        history_token(&empty),
    );
    let created = commit(&path, &add);
    let original = &created["data"]["result"]["entry"];
    let meeting = write_request(
        "tracking.add",
        "meeting",
        json!({
            "activity":{"activity":"Meeting","tags":["team"]},
            "start":"2025-01-02T10:00:00Z","end":"2025-01-02T10:30:00Z","zone":"UTC"
        }),
        history_token(&created["data"]["context"]),
    );
    let mut preview = meeting.clone();
    preview["preview"] = json!(true);
    let observer = Connection::open(&path).unwrap();
    let version = data_version(&observer);
    let proposed = commit(&path, &preview);
    let effects = proposed["data"]["entry_effects"].as_array().unwrap();
    assert_eq!(effects.len(), 3);
    let retained = effects
        .iter()
        .find(|row| row["entry_id"] == original["id"])
        .unwrap();
    assert_eq!(&retained["before"], original);
    assert_eq!(retained["after"]["end_local"], "2025-01-02 10:00:00");
    assert_eq!(retained["after"]["start_utc"], original["start_utc"]);
    let tail = effects
        .iter()
        .find(|row| row["before"].is_null() && row["after"]["activity"] == "Design")
        .unwrap();
    assert_eq!(tail["after"]["start_local"], "2025-01-02 10:30:00");
    for field in ["end_utc", "description", "category", "tags"] {
        assert_eq!(tail["after"][field], original[field], "{field}");
    }
    let inserted = effects
        .iter()
        .find(|row| row["after"]["activity"] == "Meeting")
        .unwrap();
    assert!(inserted["before"].is_null());
    assert_eq!(inserted["after"], proposed["data"]["result"]["entry"]);
    assert_eq!(
        query(&path, json!({"entity":"entries"}))["items"],
        json!([original])
    );
    assert_eq!(receipt_count(&observer), 2);
    assert_eq!(data_version(&observer), version);
    let recorded = commit(&path, &meeting);
    assert_eq!(recorded["data"]["result"], proposed["data"]["result"]);
    let rows = query(&path, json!({"entity":"entries"}));
    assert_eq!(rows["items"].as_array().unwrap().len(), effects.len());
    for effect in effects {
        let saved = rows["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == effect["entry_id"])
            .unwrap();
        assert_eq!(saved, &effect["after"]);
    }
}

#[test]
fn task_preview_includes_the_target_even_when_it_is_outside_the_open_task_page() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("task-preview.sqlite");
    let empty = initialize(&path);
    let conditions = json!({"database_id":empty["database_id"]});
    let mut target = Value::Null;
    for n in 1..=11 {
        let request = write_request(
            "task.add",
            &format!("task-{n}"),
            json!({"title":format!("Task {n}")}),
            conditions.clone(),
        );
        target = commit(&path, &request)["data"]["result"].clone();
    }
    let mut update = write_request(
        "task.edit",
        "rename",
        json!({"id":target["id"],"title":"Renamed"}),
        json!({"database_id":empty["database_id"],"task_revision":target["revision"]}),
    );
    update["preview"] = json!(true);
    let simulated = commit(&path, &update);
    let page = simulated["data"]["before"]["open_tasks"]["items"]
        .as_array()
        .unwrap();
    assert_eq!(page.len(), 10);
    assert!(!page.iter().any(|row| row["id"] == target["id"]));
    assert_eq!(simulated["data"]["task_before"], target);
    assert_eq!(simulated["data"]["result"]["title"], "Renamed");
    assert_eq!(simulated["data"]["entry_effects"], json!([]));
    let lookup = query(&path, json!({"entity":"tasks","search":"Task 11"}));
    assert_eq!(lookup["items"], json!([target]));
}

#[test]
fn resuming_a_historical_entry_checks_the_linked_task_even_when_their_ids_differ() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resume-dependencies.sqlite");
    let empty = initialize(&path);
    let account = json!({"database_id":empty["database_id"]});
    commit(
        &path,
        &write_request(
            "task.add",
            "unrelated",
            json!({"title":"Other"}),
            account.clone(),
        ),
    );
    let task = commit(
        &path,
        &write_request(
            "task.add",
            "target",
            json!({
                "title":"Prepare contract","activity":{"activity":"Research"}
            }),
            account,
        ),
    )["data"]["result"]
        .clone();
    let mut start = timer_token(&empty);
    start["task_revision"] = task["revision"].clone();
    let tracked = commit(
        &path,
        &write_request(
            "tracking.start_task",
            "start",
            json!({"id":task["id"]}),
            start,
        ),
    );
    let end = commit(
        &path,
        &write_request(
            "tracking.stop",
            "stop",
            json!({}),
            timer_token(&tracked["data"]["context"]),
        ),
    );
    let row = &end["data"]["result"]["entry"];
    assert_ne!(row["id"], task["id"]);
    let mut token = timer_token(&end["data"]["context"]);
    token["entry_revision"] = row["revision"].clone();
    let mut resume = write_request("tracking.resume", "resume", json!({"id":row["id"]}), token);
    let missing = api(&path, &resume);
    assert_eq!(missing["ok"], false, "{missing}");
    assert!(
        missing["error"]["message"]
            .as_str()
            .unwrap()
            .contains("task_revision")
    );
    assert_eq!(row["linked_task_revision"], task["revision"]);
    resume["preconditions"]["task_revision"] = row["linked_task_revision"].clone();
    let mut preview = resume.clone();
    preview["preview"] = json!(true);
    let old = commit(&path, &preview);
    assert_eq!(old["data"]["result"]["entry"]["activity"], "Research");
    human(
        &path,
        &[
            "task",
            "set",
            &task["id"].to_string(),
            "--activity",
            "Negotiation",
        ],
    );
    let stale = api(&path, &resume);
    assert_eq!(stale["error"]["code"], "precondition_failed", "{stale}");
    let fresh = query(&path, json!({"entity":"entries"}))["items"][0].clone();
    assert_eq!(row["revision"], fresh["revision"]);
    assert_ne!(row["linked_task_revision"], fresh["linked_task_revision"]);
    assert!(context(&path)["timer_idle"].as_bool().unwrap());
    resume["preconditions"]["task_revision"] = fresh["linked_task_revision"].clone();
    let accepted = commit(&path, &resume);
    assert_eq!(
        accepted["data"]["result"]["entry"]["activity"],
        "Negotiation"
    );
    let active = &accepted["data"]["context"]["current_entry"];
    let mut again = resume;
    again["request_id"] = json!("resume-deleted-source");
    again["preconditions"]["timer_revision"] =
        accepted["data"]["context"]["timer_revision"].clone();
    let mut preview = again.clone();
    preview["preview"] = json!(true);
    commit(&path, &preview);
    human(&path, &["delete", &row["id"].to_string()]);
    let deleted = api(&path, &again);
    assert_eq!(deleted["error"]["code"], "precondition_failed", "{deleted}");
    assert_eq!(deleted["error"]["details"]["field"], "entry_revision");
    assert!(deleted["error"]["details"]["actual"].is_null());
    assert_eq!(&context(&path)["current_entry"], active);
}

#[test]
fn linked_pomodoro_resume_and_next_both_guard_the_task_read_for_a_new_segment() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session-dependencies.sqlite");
    let empty = initialize(&path);
    let task = commit(
        &path,
        &write_request(
            "task.add",
            "target",
            json!({"title":"Reading"}),
            json!({"database_id":empty["database_id"]}),
        ),
    )["data"]["result"]
        .clone();
    let mut token = timer_token(&empty);
    token["task_revision"] = task["revision"].clone();
    let focus = commit(
        &path,
        &write_request(
            "pomo.start_task",
            "focus",
            json!({"id":task["id"], "config":{"focus_seconds":3,"break_seconds":1}}),
            token,
        ),
    );
    let paused = commit(
        &path,
        &write_request(
            "pomo.pause",
            "pause",
            json!({}),
            timer_token(&focus["data"]["context"]),
        ),
    );
    assert_eq!(paused["data"]["result"]["state"], "paused");
    for (operation, title) in [("pomo.resume", "First edit"), ("pomo.next", "Second edit")] {
        let c = context(&path);
        let mut req = write_request(operation, operation, json!({}), timer_token(&c));
        let missing = api(&path, &req);
        assert_eq!(missing["ok"], false, "{missing}");
        assert!(
            missing["error"]["message"]
                .as_str()
                .unwrap()
                .contains("task_revision")
        );
        let revision = c["pomodoro"]["linked_task_revision"].as_i64().unwrap();
        req["preconditions"]["task_revision"] = json!(revision);
        let mut preview = req.clone();
        preview["preview"] = json!(true);
        commit(&path, &preview);
        human(
            &path,
            &["task", "set", &task["id"].to_string(), "--title", title],
        );
        let stale = api(&path, &req);
        assert_eq!(stale["error"]["code"], "precondition_failed", "{stale}");
        let fresh = context(&path);
        assert_eq!(fresh["timer_revision"], c["timer_revision"]);
        assert_ne!(fresh["pomodoro"]["linked_task_revision"], revision);
        req["preconditions"]["task_revision"] = fresh["pomodoro"]["linked_task_revision"].clone();
        let accepted = commit(&path, &req);
        assert_eq!(accepted["data"]["result"]["state"], "running");
        assert_eq!(
            accepted["data"]["context"]["current_entry"]["description"],
            title
        );
        if operation == "pomo.resume" {
            wait_for_deadline(&accepted["data"]["result"]["deadline_utc"]);
            let mut break_token = timer_token(&accepted["data"]["context"]);
            break_token["task_revision"] = fresh["pomodoro"]["linked_task_revision"].clone();
            let rest = commit(
                &path,
                &write_request("pomo.next", "start-break", json!({}), break_token),
            );
            assert_eq!(rest["data"]["result"]["phase"], "short_break");
            wait_for_deadline(&rest["data"]["result"]["deadline_utc"]);
        }
    }
}

#[test]
fn committing_a_preview_after_its_timer_deadline_requires_a_fresh_preview() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preview-time.sqlite");
    let empty = initialize(&path);
    let focus = commit(
        &path,
        &write_request(
            "pomo.start",
            "focus",
            json!({
                "activity":{"activity":"Read"},"config":{"focus_seconds":3,"break_seconds":30}
            }),
            timer_token(&empty),
        ),
    );
    let request = write_request(
        "pomo.next",
        "advance",
        json!({}),
        timer_token(&focus["data"]["context"]),
    );
    let mut preview = request.clone();
    preview["preview"] = json!(true);
    let early = commit(&path, &preview);
    assert_eq!(early["data"]["result"]["phase"], "focus");
    let bound = &early["data"]["commit_preconditions"]["before_utc"];
    let deadline: DateTime<Utc> = bound.as_str().unwrap().parse().unwrap();
    assert_eq!(
        deadline,
        focus["data"]["result"]["deadline_utc"]
            .as_str()
            .unwrap()
            .parse::<DateTime<Utc>>()
            .unwrap()
    );
    let start = Instant::now();
    while Utc::now() < deadline {
        assert!(start.elapsed() < Duration::from_secs(8));
        thread::sleep(Duration::from_millis(10));
    }
    let mut reviewed = request.clone();
    reviewed["preconditions"] = early["data"]["commit_preconditions"].clone();
    let expired = api(&path, &reviewed);
    assert_eq!(expired["error"]["code"], "precondition_failed", "{expired}");
    let past = context(&path);
    assert_eq!(past["pomodoro"]["state"], "running");
    assert_eq!(past["pomodoro"]["reconciliation_due"], true);
    assert_eq!(
        past["timer_revision"],
        focus["data"]["context"]["timer_revision"]
    );
    let rechecked = commit(&path, &preview);
    assert_eq!(rechecked["data"]["result"]["phase"], "short_break");
    assert!(
        rechecked["data"]["commit_preconditions"]
            .get("before_utc")
            .is_none()
    );
    assert_eq!(
        rechecked["data"]["commit_preconditions"]["not_before_utc"],
        focus["data"]["result"]["deadline_utc"]
    );
    reviewed["preconditions"] = rechecked["data"]["commit_preconditions"].clone();
    let committed = commit(&path, &reviewed);
    assert_eq!(committed["data"]["result"]["phase"], "short_break");
    assert_eq!(committed["data"]["result"]["completed_focus"], 1);
    assert_eq!(commit(&path, &reviewed), committed);
}
