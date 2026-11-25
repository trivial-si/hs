//! A small, explicit wire contract. The same schemas drive discovery and validation.
use super::{Fault, WireResult};
use serde_json::{Value, json};

const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

fn publish(mut schema: Value) -> Value {
    schema["$schema"] = json!(DIALECT);
    schema
}

pub(super) fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn string() -> Value {
    json!({"type":"string","maxLength":16384})
}
fn name() -> Value {
    json!({"type":"string","minLength":1,"maxLength":512,"pattern":r"\S"})
}
fn id() -> Value {
    json!({"type":"integer","minimum":1,"maximum":i64::MAX})
}
fn revision() -> Value {
    json!({"type":"integer","minimum":0,"maximum":i64::MAX})
}
fn nullable(value: Value) -> Value {
    json!({"anyOf":[value,{"type":"null"}]})
}
fn tags() -> Value {
    json!({"type":"array","items":name(),"maxItems":100})
}
fn quadrant() -> Value {
    json!({"enum":["do","plan","delegate","drop"]})
}
fn status() -> Value {
    json!({"enum":["open","done","archived"]})
}
fn activity() -> Value {
    object(
        json!({"activity":name(),"category":nullable(name()),"description":string(),"tags":tags()}),
        &["activity"],
    )
}
fn config() -> Value {
    object(
        json!({"focus_seconds":id(),"break_seconds":id(),"long_break_seconds":id(),"rounds":{"type":"integer","minimum":1,"maximum":u32::MAX}}),
        &[],
    )
}

pub fn write_operations() -> &'static [&'static str] {
    &[
        "initialize",
        "task.add",
        "task.edit",
        "task.status",
        "tracking.start",
        "tracking.add",
        "tracking.start_task",
        "tracking.stop",
        "tracking.resume",
        "tracking.edit",
        "tracking.delete",
        "pomo.start",
        "pomo.start_task",
        "pomo.pause",
        "pomo.resume",
        "pomo.next",
        "pomo.stop",
    ]
}

fn query_input() -> Value {
    let common = json!({
        "entity": {"const": "tasks"},
        "after_id": revision(),
        "limit": {"type":"integer","minimum":1,"maximum":100},
        "quadrant": nullable(quadrant()),
        "search": string()
    });
    let mut tasks = common.clone();
    tasks["status"] = status();
    let mut entries = common;
    entries["entity"] = json!({"const":"entries"});
    entries["task_id"] = id();
    let operations = json!({"entity":{"const":"operations"},"limit":{"type":"integer","minimum":1,"maximum":100}});
    json!({
        "type":"object", "required":["entity"],
        "oneOf":[object(tasks,&[]), object(entries,&[]), object(operations,&[])]
    })
}

pub(super) fn input(operation: &str) -> Option<Value> {
    Some(match operation {
        "context" => object(
            json!({"limit":{"type":"integer","minimum":1,"maximum":100}}),
            &[],
        ),
        "initialize" | "capabilities" | "tracking.stop" | "pomo.pause" | "pomo.resume"
        | "pomo.next" | "pomo.stop" => object(json!({}), &[]),
        "receipt" => object(json!({"request_id":name()}), &["request_id"]),
        "query" => query_input(),
        "task.add" => object(
            json!({"title":name(),"activity":activity(),"quadrant":nullable(quadrant()),"due_date":nullable(name())}),
            &["title"],
        ),
        "task.edit" => object(
            json!({"id":id(),"title":name(),"activity":activity(),"description":string(),"tags":tags(),"quadrant":nullable(quadrant()),"due_date":nullable(name())}),
            &["id"],
        ),
        "task.status" => object(json!({"id":id(),"status":status()}), &["id", "status"]),
        "tracking.start" => object(json!({"activity":activity()}), &["activity"]),
        "tracking.add" => object(
            json!({"activity":activity(),"start":name(),"end":name(),"zone":name()}),
            &["activity", "start", "end"],
        ),
        "tracking.start_task" => object(json!({"id":id()}), &["id"]),
        "tracking.resume" => object(json!({"id":id()}), &["id"]),
        "tracking.delete" => object(json!({"id":id()}), &["id"]),
        "tracking.edit" => object(
            json!({"id":id(),"activity":name(),"category":nullable(name()),"description":string(),"tags":tags(),"start":name(),"end":name()}),
            &["id"],
        ),
        "pomo.start" => object(
            json!({"activity":activity(),"config":config()}),
            &["activity"],
        ),
        "pomo.start_task" => object(json!({"id":id(),"config":config()}), &["id"]),
        _ => return None,
    })
}

pub(super) fn preconditions(operation: &str) -> Value {
    let mut required = vec!["database_id"];
    if operation != "task.add" && operation != "task.edit" {
        required.push("timer_revision");
    }
    if matches!(
        operation,
        "task.edit" | "task.status" | "tracking.start_task" | "pomo.start_task"
    ) {
        required.push("task_revision");
    }
    if matches!(
        operation,
        "tracking.edit" | "tracking.delete" | "tracking.resume"
    ) {
        required.push("entry_revision");
    }
    if matches!(operation, "tracking.add" | "tracking.edit") {
        required.push("history_revision");
    }
    let mut properties = json!({"database_id":name(),"before_utc":name(),"not_before_utc":name(),"timer_revision":revision(),"history_revision":revision(),"timer_idle":{"type":"boolean"}});
    if matches!(
        operation,
        "task.edit"
            | "task.status"
            | "tracking.start_task"
            | "tracking.resume"
            | "pomo.start_task"
            | "pomo.resume"
            | "pomo.next"
    ) {
        properties["task_revision"] = id();
    }
    if matches!(
        operation,
        "tracking.edit" | "tracking.delete" | "tracking.resume"
    ) {
        properties["entry_revision"] = id();
    }
    object(properties, &required)
}

pub(super) fn request(operation: &str, mcp: bool) -> Option<Value> {
    let input = input(operation)?;
    let needs_input = input["required"]
        .as_array()
        .is_some_and(|fields| !fields.is_empty());
    let mut props =
        json!({"schema_version":{"const":1},"operation":{"const":operation},"input":input});
    let mut required = vec!["operation"];
    if needs_input {
        required.push("input");
    }
    if !mcp {
        required.push("schema_version");
    }
    if write_operations().contains(&operation) {
        props["request_id"] = name();
        props["actor"] = name();
        props["session"] = name();
        if !mcp {
            props["preview"] = json!({"type":"boolean"});
        }
        required.push("request_id");
        if operation != "initialize" {
            props["preconditions"] = preconditions(operation);
            required.push("preconditions");
        }
    }
    Some(object(props, &required))
}

/// Every application response has exactly one of these two shapes.
pub fn response_schema() -> Value {
    let success = object(
        json!({
            "schema_version": {"const":1},
            "ok": {"const":true},
            "operation": {"type":"string"},
            "data": {"type":"object"}
        }),
        &["schema_version", "ok", "operation", "data"],
    );
    let fault = object(
        json!({
            "code":{"type":"string"}, "message":{"type":"string"},
            "retryable":{"type":"boolean"}, "details":{}
        }),
        &["code", "message", "retryable", "details"],
    );
    let failure = object(
        json!({
            "schema_version": {"const":1},
            "ok": {"const":false},
            "operation": nullable(json!({"type":"string"})),
            "error": fault
        }),
        &["schema_version", "ok", "operation", "error"],
    );
    publish(json!({"type":"object","oneOf":[success,failure]}))
}

pub fn capabilities() -> Value {
    let mut operations = serde_json::Map::new();
    for operation in
        write_operations()
            .iter()
            .copied()
            .chain(["capabilities", "context", "query", "receipt"])
    {
        operations.insert(
            operation.into(),
            publish(request(operation, false).unwrap()),
        );
    }
    json!({"schema_version":1,"application":"hs","version":env!("CARGO_PKG_VERSION"),"database_schema":5,
        "json_schema_dialect":DIALECT,"response_schema":response_schema(),
        "operations":operations,"max_request_bytes":crate::wire::MAX_JSON_BYTES,"max_page_size":100,
        "idempotency":{"scope":"database lineage","receipt_retention":"until database is removed or restored","fingerprint":"canonical validated request; preview and request_id excluded","replay":"original response"},
        "time":{"timestamps":"RFC3339 with offset","durations":"integer seconds","context":"stored state, with reconciliation_due; reads never advance timers"},
        "preconditions":"Checked before clock reconciliation in the same write transaction. Copy database_id and timer_revision from context; entity revisions from query. For task-linked tracking.resume, pomo.resume and pomo.next supply task_revision from entry.linked_task_revision or context.pomodoro.linked_task_revision. Preview commit_preconditions includes live timer_revision and whole-second effective-clock bounds: exclusive before_utc for a future phase deadline, inclusive not_before_utc for an expired phase.",
        "preview":"Private writable snapshot; source is read-only. Up to 16 MiB stays in memory; larger sources use an anonymous temporary SQLite database deleted on close. Simulated IDs, timestamps and revisions are provisional; commit with commit_preconditions or refresh live state.",
        "mcp":{"transport":"stdio","protocol_version":"2025-11-25"}})
}

pub fn tool_definitions() -> Vec<Value> {
    let choices: Vec<_> = write_operations()
        .iter()
        .map(|op| request(op, true).unwrap())
        .collect();
    let preview_choices: Vec<_> = write_operations()
        .iter()
        .filter(|op| **op != "initialize")
        .map(|op| request(op, true).unwrap())
        .collect();
    [
        ("hs_capabilities","Discover hs operations and JSON schemas",object(json!({}),&[]),true,false),
        ("hs_context","Read database identity, current date/timezone, stored timer state and a bounded open-task page without advancing timers. Start here for client-side task classification; the MCP client supplies the reasoning.",input("context").unwrap(),true,false),
        ("hs_query","Read a bounded page of tasks, entries, or operation receipts. For open Inbox tasks use entity=tasks, status=open, quadrant=null; follow next_after_id as after_id with the same filters. Task rows include activity (time label), category, tags and revision.",input("query").unwrap(),true,false),
        ("hs_receipt","Recover an original successful response by request ID",input("receipt").unwrap(),true,false),
        ("hs_execute","Execute a user-authorized operation with a unique request ID and state preconditions. Apply client-supplied classifications with task.edit; tags replace the array, so merge existing tags for additive changes. Use the preview's commit_preconditions for a reviewed edit. Never count agent runtime as human work automatically.",json!({"type":"object","oneOf":choices}),false,true),
        ("hs_preview","Simulate an operation on a private temporary copy; source data is unchanged. Preview task.edit for classification suggestions. Results are provisional; use commit_preconditions with the same input and request ID to apply through hs_execute when authorized.",json!({"type":"object","oneOf":preview_choices}),true,false),
    ].into_iter().map(|(name,description,input,read_only,destructive)|json!({"name":name,"description":description,"inputSchema":publish(input),"outputSchema":response_schema(),"annotations":{"readOnlyHint":read_only,"destructiveHint":destructive,"idempotentHint":true,"openWorldHint":false}})).collect()
}

// This validator deliberately implements only the keywords emitted above. Keeping
// discovery and acceptance together prevents silently ignored misspelled fields.
pub(super) fn validate(schema: &Value, value: &Value, path: &str) -> WireResult<()> {
    let bad = || Fault::input(format!("Invalid value at {path}"));
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array)
        && !options
            .iter()
            .any(|option| validate(option, value, path).is_ok())
    {
        return Err(bad());
    }
    if let Some(options) = schema.get("oneOf").and_then(Value::as_array) {
        let matches = options
            .iter()
            .filter(|option| validate(option, value, path).is_ok())
            .take(2)
            .count();
        if matches != 1 {
            return Err(Fault::input(format!(
                "Expected exactly one matching alternative at {path}"
            )));
        }
    }
    if schema.get("const").is_some_and(|v| v != value) {
        return Err(bad());
    }
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|values| !values.contains(value))
    {
        return Err(bad());
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let map = value.as_object().ok_or_else(bad)?;
            let props = schema.get("properties").and_then(Value::as_object);
            for key in schema["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !map.contains_key(key) {
                    return Err(Fault::input(format!("Missing {path}.{key}")));
                }
            }
            for (key, v) in map {
                if let Some(property) = props.and_then(|properties| properties.get(key)) {
                    validate(property, v, &format!("{path}.{key}"))?;
                } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                    return Err(Fault::input(format!("Unknown field {path}.{key}")));
                }
            }
        }
        Some("array") => {
            let list = value.as_array().ok_or_else(bad)?;
            if list.len() > schema["maxItems"].as_u64().unwrap_or(u64::MAX) as usize {
                return Err(bad());
            }
            for (i, v) in list.iter().enumerate() {
                validate(&schema["items"], v, &format!("{path}[{i}]"))?;
            }
        }
        Some("string") => {
            let s = value.as_str().ok_or_else(bad)?;
            let len = s.chars().count() as u64;
            if len < schema["minLength"].as_u64().unwrap_or(0)
                || len > schema["maxLength"].as_u64().unwrap_or(u64::MAX)
            {
                return Err(bad());
            }
            if schema.get("pattern").and_then(Value::as_str) == Some(r"\S") && s.trim().is_empty() {
                return Err(bad());
            }
        }
        Some("integer") => {
            let n = value.as_i64().ok_or_else(bad)?;
            if n < schema["minimum"].as_i64().unwrap_or(i64::MIN)
                || n > schema["maximum"].as_i64().unwrap_or(i64::MAX)
            {
                return Err(bad());
            }
        }
        Some("boolean") if !value.is_boolean() => return Err(bad()),
        Some("null") if !value.is_null() => return Err(bad()),
        _ => (),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_filters_select_exactly_one_entity_schema() {
        let schema = input("query").unwrap();
        for valid in [
            json!({"entity":"tasks","status":"open","quadrant":null}),
            json!({"entity":"entries","after_id":42,"task_id":7,"search":"meeting"}),
            json!({"entity":"operations","limit":8}),
        ] {
            assert!(validate(&schema, &valid, "input").is_ok(), "{valid}");
        }
        for invalid in [
            json!({}),
            json!({"entity":"tasks","task_id":7}),
            json!({"entity":"entries","status":"open"}),
            json!({"entity":"operations","after_id":99}),
        ] {
            assert!(validate(&schema, &invalid, "input").is_err(), "{invalid}");
        }
        let ambiguous = json!({"oneOf":[{"type":"integer"},{"const":1}]});
        assert!(validate(&ambiguous, &json!(1), "input").is_err());
        assert!(validate(&ambiguous, &json!(2), "input").is_ok());
    }

    #[test]
    fn published_envelopes_match_required_inputs_and_response_alternatives() {
        let catalog = capabilities();
        let create = &catalog["operations"]["task.add"];
        let mut request = json!({"schema_version":1,"operation":"task.add","request_id":"r","preconditions":{"database_id":"d"}});
        assert!(validate(create, &request, "request").is_err());
        request["input"] = json!({"title":"  "});
        assert!(validate(create, &request, "request").is_err());
        request["input"]["title"] = json!("Prepare");
        assert!(validate(create, &request, "request").is_ok());
        let reply = &catalog["response_schema"];
        let mut ok =
            json!({"schema_version":1,"ok":true,"operation":"context","data":{"timer_idle":true}});
        let error = super::super::error_response("invalid_input", "Expected a string operation");
        assert!(validate(reply, &ok, "response").is_ok());
        assert!(validate(reply, &error, "response").is_ok());
        ok["error"] = error["error"].clone();
        assert!(validate(reply, &ok, "response").is_err());
        for tool in tool_definitions() {
            assert_eq!(tool["outputSchema"], *reply);
            assert_eq!(tool["inputSchema"]["$schema"], DIALECT);
        }
    }
}
