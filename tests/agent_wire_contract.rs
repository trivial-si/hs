use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
fn request(path: Option<&Path>, raw: &str) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hs"));
    command
        .env_remove("HOME")
        .env_remove("XDG_DATA_HOME")
        .env("TZ", "UTC");
    if let Some(path) = path {
        command.arg("--db").arg(path);
    }
    let mut child = command
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
        .write_all(raw.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let reply: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("No agent envelope: {output:?}"));
    assert_eq!(reply["schema_version"], 1);
    assert_eq!(output.status.success(), reply["ok"].as_bool().unwrap());
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
    reply
}
#[test]
fn discovery_and_validation_work_without_a_database_location() {
    assert_eq!(
        request(None, r#"{"schema_version":1,"operation":"capabilities"}"#)["ok"],
        true
    );
    assert_eq!(request(None, "{")["error"]["code"], "invalid_json");
    assert_eq!(
        request(None, r#"{"schema_version":1,"operation":"context"}"#)["error"]["code"],
        "database_path_unavailable"
    );
    let invalid = request(
        None,
        r#"{"schema_version":1,"operation":"task.add","request_id":"x","preconditions":{"database_id":"d"},"input":{"title":null}}"#,
    );
    assert_eq!(invalid["error"]["code"], "invalid_input");
}
#[test]
fn ambiguous_json_and_unrelated_revision_checks_cannot_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profile.sqlite");
    let initialized = request(
        Some(&path),
        r#"{"schema_version":1,"operation":"initialize","request_id":"init"}"#,
    );
    let id = initialized["data"]["context"]["database_id"]
        .as_str()
        .unwrap();
    let duplicate = format!(
        r#"{{"schema_version":1,"operation":"task.add","request_id":"duplicate","preconditions":{{"database_id":"{id}"}},"input":{{"title":"Reviewed","\u0074itle":"Different"}}}}"#
    );
    assert_eq!(
        request(Some(&path), &duplicate)["error"]["code"],
        "invalid_json"
    );
    let create = json!({"schema_version":1,"operation":"task.add","request_id":"create","preconditions":{"database_id":id},"input":{"title":"Example"}});
    let task = request(Some(&path), &create.to_string());
    assert_eq!(task["ok"], true);
    let invalid = json!({"schema_version":1,"operation":"task.status","request_id":"status","preconditions":{"database_id":id,"timer_revision":task["data"]["context"]["timer_revision"],"task_revision":task["data"]["result"]["revision"],"entry_revision":1},"input":{"id":task["data"]["result"]["id"],"status":"done"}});
    assert_eq!(
        request(Some(&path), &invalid.to_string())["error"]["code"],
        "invalid_input"
    );
    let reply = request(
        Some(&path),
        r#"{"schema_version":1,"operation":"query","input":{"entity":"tasks"}}"#,
    );
    let items = reply["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["status"], "open");
    let audit = request(
        Some(&path),
        r#"{"schema_version":1,"operation":"query","input":{"entity":"operations"}}"#,
    );
    assert_eq!(audit["data"]["items"].as_array().unwrap().len(), 2);
}
#[cfg(unix)]
#[test]
fn input_read_failures_keep_the_agent_error_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hs"))
        .arg("api")
        .env_remove("HOME")
        .env_remove("XDG_DATA_HOME")
        .stdin(Stdio::from(std::fs::File::open(dir.path()).unwrap()))
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "io_error");
    assert_eq!(value["ok"], false);
    assert!(output.stderr.is_empty());
}
