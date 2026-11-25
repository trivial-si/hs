//! Validate the client capabilities and identity defined by MCP 2025-11-25.
//! Unrecognized extension fields are deliberately left open.
use serde_json::{Map, Value};

use super::RpcError;

pub(super) fn validate(params: &Map<String, Value>) -> Result<(), RpcError> {
    if !params.get("protocolVersion").is_some_and(Value::is_string) {
        return Err(RpcError::params("protocolVersion must be a string"));
    }
    let capabilities = params
        .get("capabilities")
        .filter(|value| value.is_object())
        .ok_or_else(|| RpcError::params("capabilities must be an object"))?;
    for path in [
        "/elicitation",
        "/elicitation/form",
        "/elicitation/url",
        "/experimental",
        "/roots",
        "/sampling",
        "/sampling/context",
        "/sampling/tools",
        "/tasks",
        "/tasks/cancel",
        "/tasks/list",
        "/tasks/requests",
        "/tasks/requests/elicitation",
        "/tasks/requests/elicitation/create",
        "/tasks/requests/sampling",
        "/tasks/requests/sampling/createMessage",
    ] {
        if capabilities
            .pointer(path)
            .is_some_and(|value| !value.is_object())
        {
            return Err(RpcError::params(format!(
                "capabilities{path} must be an object"
            )));
        }
    }
    if capabilities
        .pointer("/roots/listChanged")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(RpcError::params(
            "capabilities/roots/listChanged must be a boolean",
        ));
    }
    if let Some(experimental) = capabilities.get("experimental").and_then(Value::as_object)
        && experimental.values().any(|value| !value.is_object())
    {
        return Err(RpcError::params(
            "Each experimental capability must be an object",
        ));
    }
    let info = params
        .get("clientInfo")
        .and_then(Value::as_object)
        .ok_or_else(|| RpcError::params("clientInfo must be an object"))?;
    for field in ["name", "version"] {
        if !info.get(field).is_some_and(Value::is_string) {
            return Err(RpcError::params(format!(
                "clientInfo.{field} must be a string"
            )));
        }
    }
    for field in ["title", "description", "websiteUrl"] {
        if info.get(field).is_some_and(|value| !value.is_string()) {
            return Err(RpcError::params(format!(
                "clientInfo.{field} must be a string"
            )));
        }
    }
    if let Some(icons) = info.get("icons") {
        let icons = icons
            .as_array()
            .ok_or_else(|| RpcError::params("clientInfo.icons must be an array"))?;
        for (index, icon) in icons.iter().enumerate() {
            if !valid_icon(icon) {
                return Err(RpcError::params(format!(
                    "Invalid clientInfo.icons[{index}]: expected src and correctly typed icon metadata"
                )));
            }
        }
    }
    Ok(())
}

fn valid_icon(icon: &Value) -> bool {
    icon.get("src").is_some_and(Value::is_string)
        && icon.get("mimeType").is_none_or(Value::is_string)
        && icon
            .get("theme")
            .is_none_or(|value| matches!(value.as_str(), Some("light" | "dark")))
        && icon.get("sizes").is_none_or(|value| {
            value
                .as_array()
                .is_some_and(|sizes| sizes.iter().all(Value::is_string))
        })
}
