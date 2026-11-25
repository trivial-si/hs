//! Versioned local automation contract. A receipt and its mutation commit together.
mod command;
mod preconditions;
mod preview;
mod schema;
mod views;
mod writes;
pub use schema::{capabilities, tool_definitions, write_operations};

use crate::{Error, Store, storage::agent as persistence};
use command::Command;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const MAX_REQUEST_BYTES: usize = crate::wire::MAX_JSON_BYTES;
type WireResult<T> = std::result::Result<T, Fault>;

#[derive(Debug)]
struct Fault {
    code: &'static str,
    message: String,
    retryable: bool,
    details: Value,
}
impl Fault {
    fn input(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_input",
            message: message.into(),
            retryable: false,
            details: Value::Null,
        }
    }
    fn conflict(code: &'static str, message: impl Into<String>, details: Value) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
            details,
        }
    }
}
impl From<Error> for Fault {
    fn from(error: Error) -> Self {
        Self {
            code: error.classification().code,
            retryable: error.classification().code == "database_busy",
            message: error.to_string(),
            details: Value::Null,
        }
    }
}
impl From<rusqlite::Error> for Fault {
    fn from(error: rusqlite::Error) -> Self {
        Error::from(error).into()
    }
}
impl From<serde_json::Error> for Fault {
    fn from(error: serde_json::Error) -> Self {
        Error::from(error).into()
    }
}

pub fn error_response(code: &str, message: &str) -> Value {
    json!({"schema_version":1,"ok":false,"operation":null,"error":{"code":code,"message":message,"retryable":false,"details":null}})
}
fn success(operation: &str, data: Value) -> Value {
    json!({"schema_version":1,"ok":true,"operation":operation,"data":data})
}

/// Complete one request; errors are data. Transport adapters decide exit status.
pub fn handle(path: &Path, request: Value) -> Value {
    dispatch(Some(path), request)
}

/// Discovery and request validation remain available when no profile can be located.
pub fn handle_without_database(request: Value) -> Value {
    dispatch(None, request)
}

fn dispatch(path: Option<&Path>, request: Value) -> Value {
    let operation = request
        .get("operation")
        .filter(|v| v.is_string())
        .cloned()
        .unwrap_or(Value::Null);
    match handle_inner(path, request) {
        Ok(response) => response,
        Err(error) => {
            json!({"schema_version":1,"ok":false,"operation":operation,"error":{"code":error.code,"message":error.message,"retryable":error.retryable,"details":error.details}})
        }
    }
}

fn handle_inner(path: Option<&Path>, mut request: Value) -> WireResult<Value> {
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .ok_or_else(|| Fault::input("operation must be a string"))?
        .to_owned();
    let validation = schema::request(&operation, false)
        .ok_or_else(|| Fault::input(format!("Unknown operation {operation:?}; use capabilities")))
        .and_then(|contract| schema::validate(&contract, &request, "request"));
    if request.get("input").is_none() {
        request["input"] = json!({});
    }
    let command = match validation.and_then(|()| Command::decode(&operation, &request["input"])) {
        Ok(command) => command,
        Err(error) => return archived_replay(path, &request)?.ok_or(error),
    };
    if matches!(command, Command::Capabilities) {
        return Ok(success(command.name(), capabilities()));
    }
    if matches!(command, Command::Initialize) && request["preview"].as_bool() == Some(true) {
        return Err(Fault::input("initialize does not support preview"));
    }
    let path=path.ok_or_else(||Fault::conflict("database_path_unavailable","No database path is available. Supply --db PATH or configure the default data directory.",Value::Null))?;
    if !command.is_write() {
        return read(path, &command);
    }
    write(path, &command, request)
}

fn read(path: &Path, command: &Command) -> WireResult<Value> {
    let mut store = Store::open_read_only(path).map_err(existing_database_error)?;
    let tx = store.conn.transaction()?;
    let data = match command {
        Command::Context { limit } => views::context(&tx, &super::clock::now()?, *limit)?,
        Command::Query(query) => views::query(&tx, query)?,
        Command::Receipt { request_id } => {
            let receipt = persistence::lookup_receipt(&tx, request_id)?.ok_or_else(|| {
                Fault::conflict(
                    "receipt_not_found",
                    "No committed receipt has this request ID",
                    Value::Null,
                )
            })?;
            recorded_response(&receipt, &receipt.operation)?
        }
        _ => return Err(Fault::input("Expected an agent read operation")),
    };
    tx.commit()?;
    Ok(success(command.name(), data))
}

/// Use the normalized v1 wire envelope for hashes, preserving archived receipts
/// when internal Rust input types or their defaults change.
fn fingerprint(request: &Value) -> WireResult<String> {
    let mut fields = request
        .as_object()
        .ok_or_else(|| Fault::input("Expected a request object"))?
        .clone();
    fields.remove("preview");
    fields.remove("request_id");
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&fields)?)
    ))
}

fn recorded_response(
    receipt: &persistence::Receipt,
    expected_operation: &str,
) -> WireResult<Value> {
    let invalid = || {
        Fault::conflict(
            "invalid_receipt",
            "The stored receipt is damaged or does not match its recorded operation and request ID",
            json!({"request_id":receipt.request_id}),
        )
    };
    let response: Value = serde_json::from_str(&receipt.response_json).map_err(|_| invalid())?;
    if schema::validate(&schema::response_schema(), &response, "stored receipt").is_err()
        || response["ok"] != true
        || receipt.operation != expected_operation
        || response["operation"] != receipt.operation
        || response["data"]["request_id"] != receipt.request_id
    {
        return Err(invalid());
    }
    Ok(response)
}

/// A previously accepted v1 request keeps its receipt when validation is
/// tightened. A fingerprint miss always goes back to the current validator.
fn archived_replay(path: Option<&Path>, request: &Value) -> WireResult<Option<Value>> {
    let Some(path) = path else { return Ok(None) };
    let Some(id) = request["request_id"].as_str() else {
        return Ok(None);
    };
    if request["schema_version"] != 1
        || id.trim().is_empty()
        || id.chars().count() > 512
        || request.get("preview").is_some_and(|p| !p.is_boolean())
    {
        return Ok(None);
    }
    let source = match Store::open_read_only(path) {
        Ok(store) => store,
        Err(error) if error.classification().code == "database_busy" => return Err(error.into()),
        Err(_) => return Ok(None),
    };
    let Some(receipt) = persistence::lookup_receipt(&source.conn, id)? else {
        return Ok(None);
    };
    if fingerprint(request)? != receipt.fingerprint {
        return Ok(None);
    }
    let operation = request["operation"]
        .as_str()
        .ok_or_else(|| Fault::input("operation must be a string"))?;
    recovered(
        &receipt,
        operation,
        request["preview"].as_bool().unwrap_or(false),
    )
    .map(Some)
}

fn replay(
    conn: &Connection,
    request_id: &str,
    fingerprint: &str,
    operation: &str,
    preview: bool,
) -> WireResult<Option<Value>> {
    let Some(receipt) = persistence::lookup_receipt(conn, request_id)? else {
        return Ok(None);
    };
    if receipt.fingerprint != fingerprint {
        return Err(Fault::conflict(
            "request_id_conflict",
            "This request ID already committed a different request",
            json!({"request_id":request_id}),
        ));
    }
    recovered(&receipt, operation, preview).map(Some)
}

fn recovered(receipt: &persistence::Receipt, operation: &str, preview: bool) -> WireResult<Value> {
    let response = recorded_response(receipt, operation)?;
    if preview {
        return Err(Fault::conflict(
            "already_committed",
            "This request was already committed; use receipt to recover its response",
            response,
        ));
    }
    Ok(response)
}

fn write(path: &Path, command: &Command, request: Value) -> WireResult<Value> {
    let initializing = matches!(command, Command::Initialize);
    let preview = request["preview"].as_bool().unwrap_or(false);
    let request_id = request["request_id"]
        .as_str()
        .ok_or_else(|| Fault::input("Expected a request ID"))?;
    let fingerprint = fingerprint(&request)?;

    // A committed response is immutable. It can be recovered using a reader
    // while another process has the write lock, even on a read-only profile.
    let mut store = match Store::open_read_only(path) {
        Ok(source) => {
            if let Some(response) = replay(
                &source.conn,
                request_id,
                &fingerprint,
                command.name(),
                preview,
            )? {
                return Ok(response);
            }
            if preview {
                source.preview_copy()?
            } else {
                Store::open_current(path).map_err(existing_database_error)?
            }
        }
        Err(_) if initializing => {
            // File creation/migration is its own transaction. Reject a known
            // unusable clock before changing the filesystem or schema.
            super::clock::now()?;
            Store::open(path)?
        }
        Err(error) => return Err(existing_database_error(error)),
    };
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    // A second client could have committed after the first lookup.
    if let Some(response) = replay(&tx, request_id, &fingerprint, command.name(), preview)? {
        return Ok(response);
    }
    let now = super::clock::now()?;
    let simulation = if initializing {
        None
    } else {
        let conditions = preconditions::check(&tx, command, &request["preconditions"], &now)?;
        if preview {
            Some(preview::Simulation::begin(&tx, command, conditions, &now)?)
        } else {
            None
        }
    };
    let result = if initializing {
        json!({"initialized":true})
    } else {
        writes::execute(&tx, command, &now)?
    };
    let mut data =
        json!({"request_id":request_id,"result":result,"context":views::context(&tx,&now,10)?});
    if let Some(simulation) = simulation {
        simulation.finish(&tx, &mut data)?;
        tx.rollback()?;
        return Ok(success(command.name(), data));
    }
    let response = success(command.name(), data);
    persistence::insert_receipt(
        &tx,
        &persistence::Receipt {
            request_id: request_id.to_owned(),
            fingerprint,
            response_json: serde_json::to_string(&response)?,
            actor: request["actor"].as_str().map(str::to_owned),
            session: request["session"].as_str().map(str::to_owned),
            operation: command.name().to_owned(),
            created_utc: now.utc.timestamp(),
        },
    )?;
    tx.commit()?;
    Ok(response)
}

fn existing_database_error(error: Error) -> Fault {
    if matches!(&error,Error::Io(e) if e.kind()==std::io::ErrorKind::NotFound) {
        Fault::conflict(
            "database_not_found",
            "Database does not exist. Use operation initialize to create it, or select an existing database with --db.",
            Value::Null,
        )
    } else {
        error.into()
    }
}
