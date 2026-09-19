//! Shared response-building utilities for MCP tool/resource handlers.
//!
//! Centralizes canonical success envelope construction so response shape remains
//! consistent across tools and resources while preserving existing behavior.

use chrono::{SecondsFormat, Utc};
use rust_mcp_sdk::schema::{ContentBlock, TextContent};
use serde_json::{Map, Value};

use crate::mcp::rpc::json_rpc_result;

#[derive(Debug)]
pub struct Pagination<T> {
    pub rows: Vec<T>,
    pub total: usize,
    pub returned: usize,
    pub truncated: bool,
}

/// Returns the canonical RFC3339 UTC timestamp string used in tool metadata.
///
/// This keeps `generated_at_utc` formatting consistent across all handlers.
pub fn generated_at_utc_string() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Applies a result limit and returns consistent pagination metadata.
///
/// `total` is the number of matching rows before limiting, `returned` is the
/// number emitted after limiting, and `truncated` is true only when rows were
/// dropped. Use this for in-memory tool results where the full match set is
/// already available.
pub fn paginate_rows<T>(rows: Vec<T>, limit: usize) -> Pagination<T> {
    let total = rows.len();
    let rows = rows.into_iter().take(limit).collect::<Vec<_>>();
    let returned = rows.len();
    let truncated = total > returned;

    Pagination {
        rows,
        total,
        returned,
        truncated,
    }
}

/// Builds a standard successful MCP `tools/call` JSON-RPC response.
///
/// The returned payload keeps the existing `CallToolResult` shape with optional
/// human-readable `content` and required machine-readable `structuredContent`.
pub fn tool_success_response(
    id: Option<Value>,
    message: String,
    structured_content: Map<String, Value>,
) -> Value {
    json_rpc_result(
        id,
        serde_json::json!({
            "content": [ContentBlock::from(TextContent::new(message, None, None))],
            "structuredContent": Value::Object(structured_content)
        }),
    )
}

/// Builds a standard successful MCP `resources/read` JSON-RPC response.
///
/// The response is always encoded in MCP `ReadResourceResult.contents` with JSON
/// text content and no additional top-level fields.
pub fn json_text_resource_response(
    id: Option<Value>,
    uri: &str,
    structured_content: Value,
) -> Value {
    let result = serde_json::json!({
        "contents": [{
            "uri": uri,
            "mimeType": "application/json",
            "text": structured_content.to_string()
        }]
    });

    json_rpc_result(id, result)
}
