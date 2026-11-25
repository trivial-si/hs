use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    responses: Receiver<String>,
    next_id: u64,
}

impl Client {
    fn new(db: &Path) -> Self {
        Self::with_database(Some(db))
    }

    fn with_database(db: Option<&Path>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hs"));
        if let Some(db) = db {
            command.arg("--db").arg(db);
        }
        let mut child = command
            .arg("mcp")
            .env_remove("HOME")
            .env_remove("XDG_DATA_HOME")
            .env("TZ", "UTC")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (sender, responses) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            responses,
            next_id: 0,
        }
    }

    fn send_line(&mut self, data: &[u8]) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(data).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    fn send(&mut self, message: Value) {
        self.send_line(message.to_string().as_bytes());
    }

    fn read(&self) -> Value {
        let line = self
            .responses
            .recv_timeout(Duration::from_secs(8))
            .expect("one flushed response line");
        let value: Value =
            serde_json::from_str(&line).expect("stdout must contain only JSON-RPC messages");
        assert_eq!(value["jsonrpc"], "2.0");
        value
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        self.send(json!({ "jsonrpc":"2.0", "id":self.next_id, "method":method, "params":params }));
        let response = self.read();
        assert_eq!(response["id"], self.next_id);
        response
    }

    fn initialize(&mut self) {
        let result = self.rpc("initialize", json!({
            "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"hs test","version":"1"}
        }));
        assert_eq!(result["result"]["protocolVersion"], "2025-11-25");
        assert!(
            result["result"]["instructions"]
                .as_str()
                .is_some_and(|instructions| !instructions.trim().is_empty()),
            "The host must receive the server workflow during initialization"
        );
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    }

    fn tool(&mut self, name: &str, args: Value, ok: bool) -> Value {
        let response = self.rpc("tools/call", json!({"name":name,"arguments":args}));
        assert!(response.get("error").is_none(), "{response}");
        let result = &response["result"];
        assert_eq!(result["isError"], !ok, "{response}");
        let envelope = &result["structuredContent"];
        assert_eq!(envelope["ok"], ok);
        assert_eq!(envelope["schema_version"], 1);
        assert_eq!(result["content"][0]["type"], "text");
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(&text, envelope);
        envelope.clone()
    }

    fn finish(mut self) {
        drop(self.stdin.take());
        assert!(self.child.wait().unwrap().success());
        assert!(self.responses.recv_timeout(Duration::from_secs(1)).is_err());
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn discovery_requires_handshake_and_notifications_never_run_tools() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("unopened.db");
    let mut client = Client::new(&db);
    assert_eq!(client.rpc("tools/list", json!({}))["error"]["code"], -32002);
    assert_eq!(client.rpc("ping", json!({}))["result"], json!({}));
    assert_eq!(client.rpc("initialize", json!({}))["error"]["code"], -32602);
    let params = json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}});
    let initialized = client.rpc("initialize", params.clone());
    assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
    assert!(initialized["result"]["capabilities"]["tools"].is_object());
    assert_eq!(client.rpc("tools/list", json!({}))["error"]["code"], -32002);
    client
        .send(json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{"_meta":[]}}));
    assert_eq!(client.rpc("tools/list", json!({}))["error"]["code"], -32002);
    client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    assert_eq!(client.rpc("initialize", params)["error"]["code"], -32600);
    let listed = client.rpc("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 6);
    for name in [
        "hs_capabilities",
        "hs_context",
        "hs_query",
        "hs_receipt",
        "hs_preview",
        "hs_execute",
    ] {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert_eq!(tool["annotations"]["readOnlyHint"], name != "hs_execute");
    }
    assert!(listed["result"].get("nextCursor").is_none());
    client.tool("hs_capabilities", json!({}), true);
    client.send(json!({"jsonrpc":"2.0","method":"tools/call","params":{
        "name":"hs_execute","arguments":{"operation":"initialize","request_id":"must-not-run"}}}));
    client.send(json!({"jsonrpc":"2.0","method":"notifications/unknown"}));
    client.send(json!({"jsonrpc":"2.0","error":{"code":-32700,"message":"Unknown request ID"}}));
    client.send(json!({"jsonrpc":"2.0","id":"opaque","method":"ping"}));
    assert_eq!(
        client.read(),
        json!({"jsonrpc":"2.0","id":"opaque","result":{}})
    );
    assert!(!db.exists());
    client.finish();
}

#[test]
fn malformed_and_oversized_frames_recover_without_desynchronizing_the_stream() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("unopened.db");
    let mut client = Client::new(&db);
    client.send_line(b"{");
    assert_eq!(client.read()["error"]["code"], -32700);
    client.send_line(&vec![b'x'; 1024 * 1024 + 64]);
    assert_eq!(client.read()["error"]["code"], -32600);
    client.send(json!([{"id":1,"jsonrpc":"2.0","method":"ping"}]));
    assert_eq!(client.read()["error"]["code"], -32600);
    client.send(json!({"id":null,"jsonrpc":"2.0","method":"ping"}));
    assert_eq!(client.read()["error"]["code"], -32600);
    client.initialize();
    assert_eq!(
        client.rpc("unavailable", json!({}))["error"]["code"],
        -32601
    );
    assert_eq!(
        client.rpc("tools/list", json!({"cursor":"unknown"}))["error"]["code"],
        -32602
    );
    for args in [
        json!({"name":"absent"}),
        json!({"name":"hs_execute","arguments":[]}),
        json!({"name":"hs_execute","task":{"ttl":"60"}}),
    ] {
        assert_eq!(client.rpc("tools/call", args)["error"]["code"], -32602);
    }
    let task_call = json!({"name":"hs_execute","task":{"ttl":1000},"arguments":{"operation":"initialize","request_id":"async"}});
    assert_eq!(client.rpc("tools/call", task_call)["error"]["code"], -32601);
    client.tool("hs_preview", json!({"operation":"initialize"}), false);
    client.tool("hs_execute", json!({"operation":"context"}), false);
    client.tool("hs_context", json!({"operation":"initialize"}), false);
    client.tool("hs_query", json!({"entity":"tasks","limit":101}), false);
    client.tool("hs_capabilities", json!({"unexpected":true}), false);
    assert_eq!(client.rpc("ping", json!({}))["result"], json!({}));
    assert!(!db.exists());
    client.finish();
}

#[test]
fn malformed_handshakes_can_be_corrected_and_extensions_remain_open() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("unopened.db");
    let mut client = Client::new(&db);
    let init = json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"client","version":"1"}});
    for (field, invalid) in [
        ("capabilities", json!({"roots":{"listChanged":"true"}})),
        (
            "capabilities",
            json!({"tasks":{"requests":{"sampling":{"createMessage":[]}}}}),
        ),
        ("capabilities", json!({"experimental":{"custom":false}})),
        (
            "clientInfo",
            json!({"name":"client","version":"1","icons":[{"theme":"dark"}]}),
        ),
        (
            "clientInfo",
            json!({"name":"client","version":"1","icons":[{"src":"file:x","sizes":[24]}]}),
        ),
    ] {
        let mut malformed = init.clone();
        malformed[field] = invalid;
        assert_eq!(client.rpc("initialize", malformed)["error"]["code"], -32602);
    }
    let mut valid = init;
    valid["capabilities"] =
        json!({"example.org/experimental":true,"roots":{"listChanged":false,"extension":[3]}});
    valid["clientInfo"]["icons"] =
        json!([{"src":"https://example.org/icon","sizes":["24x24"],"theme":"dark"}]);
    assert_eq!(
        client.rpc("initialize", valid)["result"]["protocolVersion"],
        "2025-11-25"
    );
    client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    // This server never issues peer requests. A null error ID from a generic
    // JSON-RPC client must not start a response-to-response loop.
    client.send(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}));
    client
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}));
    assert_eq!(client.rpc("ping", json!({}))["result"], json!({}));
    assert!(!db.exists());
    client.finish();
}

#[test]
fn malformed_metadata_and_unterminated_final_frames_cannot_execute_writes() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("unopened.db");
    let mut client = Client::new(&db);
    client.initialize();
    for frame in [
        br#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"hs_execute","arguments":{"operation":"context","\u006fperation":"initialize","request_id":"ambiguous-operation"}}}"#.as_slice(),
        br#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"hs_execute","arguments":{"operation":"initialize","request_id":"ambiguous-meta"},"_meta":{"progressToken":"first","progressToken":"second"}}}"#,
    ] {
        client.send_line(frame);
        let response = client.read();
        assert_eq!(response["error"]["code"], -32700);
        assert!(response["error"]["message"].as_str().unwrap().contains("Duplicate"));
        assert!(!db.exists());
    }
    let params =
        json!({"name":"hs_execute","arguments":{"operation":"initialize","request_id":"rejected"}});
    for metadata in [json!("string"), Value::Null, json!({"progressToken": []})] {
        let mut malformed = params.clone();
        malformed["_meta"] = metadata;
        assert_eq!(client.rpc("tools/call", malformed)["error"]["code"], -32602);
        assert!(!db.exists());
    }
    assert_eq!(
        client.rpc(
            "ping",
            json!({"_meta":{"progressToken":12,"client.example/info":{"labels":[]}}})
        )["result"],
        json!({})
    );
    let frame = json!({"jsonrpc":"2.0","id":123,"method":"tools/call","params":params}).to_string();
    let mut stdin = client.stdin.take().unwrap();
    stdin.write_all(frame.as_bytes()).unwrap();
    drop(stdin);
    let error = client.read();
    assert_eq!(error["error"]["code"], -32700);
    assert!(error.get("id").is_none());
    client.finish();
    assert!(!db.exists());
}

#[test]
fn clients_without_a_default_database_can_discover_tools_and_fix_input() {
    let mut client = Client::with_database(None);
    client.initialize();
    assert!(client.rpc("tools/list", json!({}))["result"]["tools"].is_array());
    let capabilities = client.tool("hs_capabilities", json!({}), true);
    assert!(capabilities["data"]["operations"]["task.add"].is_object());
    assert_eq!(
        client.tool("hs_query", json!({"entity":"unknown"}), false)["error"]["code"],
        "invalid_input"
    );
    let context = client.tool("hs_context", json!({}), false);
    assert_eq!(context["error"]["code"], "database_path_unavailable");
    assert!(
        context["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--db")
    );
    client.finish();
}

#[test]
fn preview_then_execute_and_retrieve_the_same_receipt_across_mcp_calls() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hs.db");
    let mut client = Client::new(&db);
    client.initialize();
    client.tool(
        "hs_execute",
        json!({"operation":"initialize","request_id":"mcp-init"}),
        true,
    );
    let initial = fs::read(&db).unwrap();
    let context = client.tool("hs_context", json!({}), true);
    let database_id = context["data"]["database_id"]
        .as_str()
        .expect("context contains database lineage");
    let write = json!({
        "operation":"task.add",
        "request_id":"mcp-task-1",
        "input":{"title":"MCP task\n新"},
        "preconditions":{"database_id":database_id}
    });
    client.tool("hs_preview", write.clone(), true);
    client.tool("hs_context", json!({"limit":5}), true);
    client.tool("hs_query", json!({"entity":"tasks","limit":5}), true);
    assert_eq!(fs::read(&db).unwrap(), initial);
    let mut injected = write.clone();
    injected["preview"] = json!(false);
    client.tool("hs_preview", injected, false);
    assert_eq!(fs::read(&db).unwrap(), initial);
    client.tool("hs_execute", write.clone(), true);
    client.tool("hs_execute", write, true);
    client.tool("hs_receipt", json!({"request_id":"mcp-task-1"}), true);
    client.finish();
    let output = Command::new(env!("CARGO_BIN_EXE_hs"))
        .arg("--db")
        .arg(db)
        .env("TZ", "UTC")
        .args(["--json", "task", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let tasks: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(tasks["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(tasks["tasks"][0]["title"], "MCP task\n新");
}

fn human(db: &Path, args: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_hs"))
        .arg("--db")
        .arg(db)
        .env("TZ", "UTC")
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
}

fn stored_entry_fields(mut entry: Value) -> Value {
    // This is the current task token, not a historical entry field. Task edits
    // correctly update it even though the entry's saved data stays unchanged.
    entry
        .as_object_mut()
        .unwrap()
        .remove("linked_task_revision");
    entry
}

#[test]
fn host_classification_uses_existing_tools_without_changing_work_or_manual_choices() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("classification.db");
    human(
        &db,
        &[
            "task",
            "add",
            "Send proposal",
            "--time-label",
            "Writing",
            "--tags",
            "client",
            "--notes",
            "Check the figures",
            "--due",
            "2099-01-01",
        ],
    );
    human(&db, &["task", "add", "Call Alex"]);
    human(
        &db,
        &[
            "task",
            "add",
            "Budget review",
            "--category",
            "Work",
            "--tags",
            "billable",
            "--quadrant",
            "plan",
        ],
    );
    human(&db, &["task", "add", "Already finished"]);
    human(&db, &["task", "done", "4"]);
    human(
        &db,
        &[
            "add",
            "2000-01-02 09:00-09:30 Writing@Work, old work #archive",
        ],
    );
    human(
        &db,
        &["pomo", "start", "--task", "1", "--focus", "1h", "--detach"],
    );

    let mut client = Client::new(&db);
    client.initialize();
    let context = client.tool("hs_context", json!({"limit":1}), true)["data"].clone();
    let entries_before = client.tool("hs_query", json!({"entity":"entries"}), true)["data"].clone();
    let tasks_before =
        client.tool("hs_query", json!({"entity":"tasks"}), true)["data"]["items"].clone();

    // A bounded inbox query is distinct from the context's open-task page.
    let first = client.tool(
        "hs_query",
        json!({"entity":"tasks","status":"open","quadrant":null,"limit":1}),
        true,
    )["data"]
        .clone();
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["id"], 1);
    assert_eq!(first["next_after_id"], 1);
    let second = client.tool("hs_query", json!({"entity":"tasks","status":"open","quadrant":null,"limit":1,"after_id":first["next_after_id"]}), true)["data"].clone();
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["items"][0]["id"], 2);
    assert!(second["next_after_id"].is_null());

    // The host supplies a classification. Category-only metadata changes must
    // carry the saved time label, and a tag suggestion must merge existing tags.
    let task = &first["items"][0];
    let mut tags = task["tags"].as_array().unwrap().clone();
    tags.push(json!("billable"));
    let mut request = json!({
        "operation":"task.edit", "request_id":"classify-proposal",
        "actor":"codex", "session":"classification-test",
        "preconditions":{"database_id":context["database_id"],"task_revision":task["revision"]},
        "input":{"id":task["id"],"activity":{"activity":task["activity"],"category":"Work"},"tags":tags,"quadrant":"plan"}
    });
    let before_preview = fs::read(&db).unwrap();
    let preview = client.tool("hs_preview", request.clone(), true)["data"].clone();
    assert_eq!(fs::read(&db).unwrap(), before_preview);
    assert_eq!(preview["provisional"], true);
    assert_eq!(preview["task_before"], *task);
    assert_eq!(preview["entry_effects"], json!([]));
    assert_eq!(preview["result"]["activity"], "Writing");
    assert_eq!(preview["result"]["category"], "Work");
    request["preconditions"] = preview["commit_preconditions"].clone();
    let applied = client.tool("hs_execute", request.clone(), true);
    assert_eq!(client.tool("hs_execute", request, true), applied);
    assert_eq!(
        client.tool(
            "hs_receipt",
            json!({"request_id":"classify-proposal"}),
            true
        )["data"],
        applied
    );

    let task = &applied["data"]["result"];
    assert_eq!(task["quadrant"], "plan");
    assert_eq!(task["tags"], json!(["billable", "client"]));
    for field in [
        "title",
        "activity",
        "description",
        "due_date",
        "status",
        "created_utc",
        "completed_utc",
    ] {
        assert_eq!(task[field], tasks_before[0][field], "{field}");
    }

    // Uncertain priority is omitted while independently supported metadata can
    // still be applied. This does not move the task out of the inbox.
    let uncertain = &second["items"][0];
    let partial = json!({
        "operation":"task.edit", "request_id":"classify-call",
        "preconditions":{"database_id":context["database_id"],"task_revision":uncertain["revision"]},
        "input":{"id":uncertain["id"],"activity":{"activity":uncertain["activity"],"category":"Work"}}
    });
    let partial_result = client.tool("hs_execute", partial, true);
    assert!(partial_result["data"]["result"]["quadrant"].is_null());
    assert_eq!(partial_result["data"]["result"]["activity"], "Call Alex");

    let tasks_after =
        client.tool("hs_query", json!({"entity":"tasks"}), true)["data"]["items"].clone();
    assert_eq!(
        tasks_after[2], tasks_before[2],
        "Manually classified task changed"
    );
    assert_eq!(tasks_after[3], tasks_before[3], "Completed task changed");
    let entries_after = client.tool("hs_query", json!({"entity":"entries"}), true)["data"].clone();
    let saved_entries = |page: &Value| {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(stored_entry_fields)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        saved_entries(&entries_after),
        saved_entries(&entries_before)
    );
    let after = client.tool("hs_context", json!({}), true)["data"].clone();
    assert_eq!(after["timer_revision"], context["timer_revision"]);
    assert_eq!(after["history_revision"], context["history_revision"]);
    assert_eq!(
        stored_entry_fields(after["current_entry"].clone()),
        stored_entry_fields(context["current_entry"].clone())
    );
    for field in ["id", "phase_id", "state", "phase", "deadline_utc"] {
        assert_eq!(
            after["pomodoro"][field], context["pomodoro"][field],
            "{field}"
        );
    }
    client.finish();
}

#[test]
fn a_classification_preview_cannot_overwrite_a_later_human_decision() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("classification-conflict.db");
    human(
        &db,
        &["task", "add", "Research options", "--tags", "research"],
    );
    let mut client = Client::new(&db);
    client.initialize();
    let context = client.tool("hs_context", json!({}), true)["data"].clone();
    let task = &context["open_tasks"]["items"][0];
    let mut request = json!({
        "operation":"task.edit", "request_id":"classification-before-human-edit",
        "preconditions":{"database_id":context["database_id"],"task_revision":task["revision"]},
        "input":{"id":task["id"],"quadrant":"plan","tags":["research","work"]}
    });
    let preview = client.tool("hs_preview", request.clone(), true);
    request["preconditions"] = preview["data"]["commit_preconditions"].clone();
    human(
        &db,
        &[
            "task",
            "set",
            "1",
            "--quadrant",
            "do",
            "--tags",
            "research,personal",
        ],
    );
    let before = fs::read(&db).unwrap();
    let rejected = client.tool("hs_execute", request, false);
    assert_eq!(rejected["error"]["code"], "precondition_failed");
    assert_eq!(rejected["error"]["details"]["field"], "task_revision");
    assert_eq!(fs::read(&db).unwrap(), before);
    assert_eq!(
        client.tool(
            "hs_receipt",
            json!({"request_id":"classification-before-human-edit"}),
            false
        )["error"]["code"],
        "receipt_not_found"
    );

    // After rereading, the host can skip priority and preserve the human's tags.
    let fresh =
        client.tool("hs_query", json!({"entity":"tasks","status":"open"}), true)["data"]["items"]
            [0]
        .clone();
    assert_eq!(fresh["quadrant"], "do");
    assert_eq!(fresh["tags"], json!(["personal", "research"]));
    let reconsidered = json!({
        "operation":"task.edit", "request_id":"classification-reconsidered",
        "preconditions":{"database_id":context["database_id"],"task_revision":fresh["revision"]},
        "input":{"id":fresh["id"],"activity":{"activity":fresh["activity"],"category":"Personal"}}
    });
    let applied = client.tool("hs_execute", reconsidered, true);
    assert_eq!(applied["data"]["result"]["quadrant"], fresh["quadrant"]);
    assert_eq!(applied["data"]["result"]["tags"], fresh["tags"]);
    assert_eq!(applied["data"]["result"]["activity"], fresh["activity"]);
    client.finish();
}
