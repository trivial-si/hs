//! Local MCP stdio transport for protocol version 2025-11-25.
mod initialize;
mod tools;

use std::{
    io::{self, BufRead, Read, Write},
    path::Path,
};

use hs::{Result, wire};
use serde_json::{Map, Value, json};

const PROTOCOL_VERSION: &str = "2025-11-25";
const MAX_FRAME: usize = wire::MAX_JSON_BYTES;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    New,
    AwaitingInitialized,
    Ready,
}

struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn params(message: impl Into<String>) -> Self {
        Self::new(-32602, message)
    }

    fn response(self, id: Option<&Value>) -> Value {
        let mut response = json!({
            "jsonrpc": "2.0",
            "error": { "code": self.code, "message": self.message }
        });
        // The pinned MCP schema omits the ID when it could not be determined.
        if let Some(id) = id {
            response["id"] = id.clone();
        }
        response
    }
}

pub(super) fn run(path: Option<&Path>) -> Result<()> {
    let mut lifecycle = Lifecycle::New;
    let mut input = io::stdin().lock();
    let mut output = io::BufWriter::new(io::stdout().lock());
    let mut frame = Vec::new();
    loop {
        frame.clear();
        // One extra byte for the newline, another to distinguish oversized input.
        let n = (&mut input)
            .take((MAX_FRAME + 2) as u64)
            .read_until(b'\n', &mut frame)?;
        if n == 0 {
            return Ok(());
        }
        let terminated = frame.last() == Some(&b'\n');
        if terminated {
            frame.pop();
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
        }
        let response = if frame.len() > MAX_FRAME {
            if !terminated {
                input.skip_until(b'\n')?;
            }
            Some(RpcError::new(-32600, "MCP message exceeds the 1 MiB limit").response(None))
        } else if !terminated {
            Some(
                RpcError::new(-32700, "Incomplete MCP frame: expected a final newline")
                    .response(None),
            )
        } else {
            match wire::parse(&frame) {
                Ok(message) => route(path, &mut lifecycle, &message),
                Err(error) => {
                    Some(RpcError::new(-32700, format!("Invalid JSON: {error}")).response(None))
                }
            }
        };
        if let Some(response) = response {
            writeln!(output, "{response}")?;
            output.flush()?;
        }
    }
}

fn route(path: Option<&Path>, lifecycle: &mut Lifecycle, message: &Value) -> Option<Value> {
    // This server never issues peer requests. Answering an unsolicited response,
    // even one carrying a generic JSON-RPC null error ID, can cause a feedback loop.
    if message.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
        && message.get("method").is_none()
        && (message.get("result").is_some() || message.get("error").is_some())
    {
        return None;
    }
    let valid_id = message
        .get("id")
        .filter(|id| id.is_string() || id.is_i64() || id.is_u64());
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || (message.get("id").is_some() && valid_id.is_none())
    {
        return Some(
            RpcError::new(-32600, "Invalid JSON-RPC 2.0 envelope or request ID").response(valid_id),
        );
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Some(RpcError::new(-32600, "Expected a JSON-RPC method").response(valid_id));
    };
    if message.get("result").is_some() || message.get("error").is_some() {
        return valid_id.map(|id| {
            RpcError::new(-32600, "A request cannot also contain a response").response(Some(id))
        });
    }
    let empty = Map::new();
    let params = match message.get("params") {
        None => &empty,
        Some(Value::Object(obj)) => obj,
        _ => {
            return valid_id
                .map(|id| RpcError::params("params must be an object").response(Some(id)));
        }
    };
    if let Some(meta) = params.get("_meta") {
        let Some(meta) = meta.as_object() else {
            return valid_id
                .map(|id| RpcError::params("_meta must be an object").response(Some(id)));
        };
        if valid_id.is_some()
            && meta
                .get("progressToken")
                .is_some_and(|token| !(token.is_string() || token.is_i64() || token.is_u64()))
        {
            return valid_id.map(|id| {
                RpcError::params("_meta.progressToken must be a string or integer")
                    .response(Some(id))
            });
        }
    }
    let Some(id) = valid_id else {
        // Notifications have no reply and cannot invoke tools.
        if method == "notifications/initialized" && *lifecycle == Lifecycle::AwaitingInitialized {
            *lifecycle = Lifecycle::Ready;
        }
        return None;
    };
    Some(match request(path, lifecycle, method, params) {
        Ok(result) => json!({ "jsonrpc":"2.0", "id":id, "result":result }),
        Err(error) => error.response(Some(id)),
    })
}

fn request(
    path: Option<&Path>,
    lifecycle: &mut Lifecycle,
    method: &str,
    params: &Map<String, Value>,
) -> std::result::Result<Value, RpcError> {
    match method {
        "initialize" => {
            if *lifecycle != Lifecycle::New {
                return Err(RpcError::new(
                    -32600,
                    "The session has already been initialized",
                ));
            }
            initialize::validate(params)?;
            *lifecycle = Lifecycle::AwaitingInitialized;
            Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name":"hs", "version":env!("CARGO_PKG_VERSION") },
                "instructions": include_str!("mcp/instructions.md")
            }))
        }
        "ping" => Ok(json!({})),
        "notifications/initialized" => Err(RpcError::new(
            -32600,
            "Send notifications/initialized without an ID",
        )),
        _ if *lifecycle != Lifecycle::Ready => Err(RpcError::new(
            -32002,
            "Initialize the session and send notifications/initialized first",
        )),
        "tools/list" => tools::list(params),
        "tools/call" => tools::call(path, params),
        _ => Err(RpcError::new(
            -32601,
            format!("Unknown MCP method {method}"),
        )),
    }
}
