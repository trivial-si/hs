//! Map MCP tool calls onto the same versioned commands as the JSON CLI.
use std::path::Path;

use hs::application::agent;
use serde_json::{Map, Value, json};

use super::RpcError;

pub(super) fn list(params: &Map<String, Value>) -> Result<Value, RpcError> {
    if params.contains_key("cursor") {
        return Err(RpcError::params("Invalid tool-list cursor"));
    }
    Ok(json!({ "tools": agent::tool_definitions() }))
}

fn tool_input_error(operation: &str, message: &str) -> Value {
    let mut error = agent::error_response("invalid_input", message);
    if !operation.is_empty() {
        error["operation"] = json!(operation);
    }
    error
}

fn handle(path: Option<&Path>, request: Value) -> Value {
    match path {
        Some(path) => agent::handle(path, request),
        None => agent::handle_without_database(request),
    }
}

fn execute(path: Option<&Path>, arguments: &Map<String, Value>, preview: bool) -> Value {
    let Some(operation) = arguments.get("operation").and_then(Value::as_str) else {
        return tool_input_error("", "A write operation is required");
    };
    if !agent::write_operations().contains(&operation) || (preview && operation == "initialize") {
        return tool_input_error(
            operation,
            "This operation is unavailable on the selected tool",
        );
    }
    if arguments
        .get("schema_version")
        .is_some_and(|v| v.as_u64() != Some(1))
    {
        return tool_input_error(operation, "Only schema_version 1 is supported");
    }
    if arguments.contains_key("preview") {
        return tool_input_error(
            operation,
            "Select hs_preview or hs_execute to set the execution mode",
        );
    }
    let mut request = Value::Object(arguments.clone());
    request["schema_version"] = json!(1);
    request["preview"] = json!(preview);
    handle(path, request)
}

pub(super) fn call(path: Option<&Path>, params: &Map<String, Value>) -> Result<Value, RpcError> {
    if let Some(task) = params.get("task") {
        if !task.is_object()
            || task
                .get("ttl")
                .is_some_and(|value| !(value.is_i64() || value.is_u64()))
        {
            return Err(RpcError::params(
                "task must be an object with an optional integer ttl",
            ));
        }
        return Err(RpcError::new(
            -32601,
            "Task augmentation is unavailable; call the tool synchronously",
        ));
    }
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::params("tools/call requires a tool name"))?;
    let empty = Map::new();
    let arguments = match params.get("arguments") {
        None => &empty,
        Some(Value::Object(args)) => args,
        _ => return Err(RpcError::params("Tool arguments must be an object")),
    };
    let result = match name {
        "hs_capabilities" | "hs_context" | "hs_query" | "hs_receipt" => {
            let operation = name.trim_start_matches("hs_");
            handle(
                path,
                json!({ "schema_version":1, "operation":operation, "input":arguments }),
            )
        }
        "hs_execute" | "hs_preview" => execute(path, arguments, name == "hs_preview"),
        _ => return Err(RpcError::params(format!("Unknown MCP tool {name}"))),
    };
    Ok(json!({
        "isError":result.get("ok").and_then(Value::as_bool) != Some(true),
        "content":[{ "type":"text", "text":result.to_string() }],
        "structuredContent":result
    }))
}
