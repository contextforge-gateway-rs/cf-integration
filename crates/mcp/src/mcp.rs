//! Shared MCP streamable-HTTP protocol helpers.

use serde_json::{Map, Value, json};
use uuid::Uuid;

/// MCP protocol version sent by the integration harness.
pub const PROTOCOL_VERSION: &str = "2025-11-25";
/// Accepted MCP streamable-HTTP response media types.
pub const ACCEPT: &str = "application/json, text/event-stream";

/// Builds a JSON-RPC request with a generated v4 UUID string ID.
#[must_use]
pub fn jsonrpc(method: &str, params: Option<Value>) -> Value {
    jsonrpc_with_id(method, params, Value::String(Uuid::new_v4().to_string()))
}

/// Builds a deterministic JSON-RPC request with an explicit ID.
#[must_use]
pub fn jsonrpc_with_id(method: &str, params: Option<Value>, id: Value) -> Value {
    let mut payload = Map::new();
    payload.insert("jsonrpc".to_owned(), Value::String("2.0".to_owned()));
    payload.insert("id".to_owned(), id);
    payload.insert("method".to_owned(), Value::String(method.to_owned()));
    if let Some(params) = params {
        payload.insert("params".to_owned(), params);
    }
    Value::Object(payload)
}

/// Builds an MCP initialize request with a generated v4 UUID string ID.
#[must_use]
pub fn initialize() -> Value {
    initialize_with_id(Value::String(Uuid::new_v4().to_string()))
}

/// Builds a deterministic MCP initialize request with an explicit ID.
#[must_use]
pub fn initialize_with_id(id: Value) -> Value {
    initialize_with_id_and_version(id, PROTOCOL_VERSION)
}

/// Builds a deterministic MCP initialize request for an explicit protocol version.
#[must_use]
pub fn initialize_with_id_and_version(id: Value, protocol_version: &str) -> Value {
    jsonrpc_with_id(
        "initialize",
        Some(json!({
            "protocolVersion": protocol_version,
            "capabilities": {},
            "clientInfo": {
                "name": "cf-integration",
                "version": "1.0"
            }
        })),
        id,
    )
}

/// Parses an MCP JSON or SSE response body.
///
/// Consecutive SSE `data:` fields are joined with newlines until the event's
/// blank-line delimiter. Malformed events are ignored, and the last valid JSON
/// event is returned.
///
/// # Errors
///
/// Returns the JSON parser error for a non-empty malformed JSON response.
pub fn parse_mcp_body(body: &str, content_type: &str) -> serde_json::Result<Option<Value>> {
    if body.is_empty() {
        return Ok(None);
    }
    let media_type = content_type
        .split_once(';')
        .map_or(content_type, |(media_type, _)| media_type)
        .trim();
    if media_type.eq_ignore_ascii_case("text/event-stream") {
        let mut message = None;
        let mut event_data = String::new();
        let mut has_data = false;
        for line in body.lines() {
            if line.is_empty() {
                flush_sse_event(&mut event_data, &mut has_data, &mut message);
                continue;
            }

            let data = match line.strip_prefix("data:") {
                Some(data) => data.strip_prefix(' ').unwrap_or(data),
                None if line == "data" => "",
                None => continue,
            };
            if has_data {
                event_data.push('\n');
            }
            event_data.push_str(data);
            has_data = true;
        }
        flush_sse_event(&mut event_data, &mut has_data, &mut message);
        return Ok(message);
    }

    serde_json::from_str(body).map(Some)
}

fn flush_sse_event(event_data: &mut String, has_data: &mut bool, message: &mut Option<Value>) {
    if *has_data {
        if let Ok(value) = serde_json::from_str(event_data) {
            *message = Some(value);
        }
        event_data.clear();
        *has_data = false;
    }
}

/// Returns arguments for tool names the integration harness knows how to call.
#[must_use]
pub fn tool_call_args(tool_name: &str) -> Option<Value> {
    match tool_name {
        "echo" | "fast_time_echo" | "fast-time-echo" => Some(json!({"message": "cf-integration"})),
        "get_system_time"
        | "get-system-time"
        | "fast_time_get_system_time"
        | "fast-time-get_system_time"
        | "fast-time-get-system-time" => Some(json!({"timezone": "UTC"})),
        _ => None,
    }
}
