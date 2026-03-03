//! Shared response-building utilities for MCP tool/resource handlers.
//!
//! Centralizes canonical success envelope construction so response shape remains
//! consistent across tools and resources while preserving existing behavior.

use chrono::{SecondsFormat, Utc};
use rust_mcp_sdk::schema::{
    CallToolResult, ContentBlock, ReadResourceContent, ReadResourceResult, TextContent,
    TextResourceContents,
};
use serde_json::{Map, Value};

use crate::mcp::rpc::json_rpc_result;

/// Returns the canonical RFC3339 UTC timestamp string used in tool metadata.
///
/// This keeps `generated_at_utc` formatting consistent across all handlers.
pub fn generated_at_utc_string() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
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
        serde_json::to_value(CallToolResult {
            content: vec![ContentBlock::from(TextContent::new(message, None, None))],
            is_error: None,
            meta: None,
            structured_content: Some(structured_content),
        })
        .expect("tool success result serialization"),
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
    let result = serde_json::to_value(ReadResourceResult {
        contents: vec![ReadResourceContent::from(TextResourceContents {
            meta: None,
            mime_type: Some("application/json".to_string()),
            text: structured_content.to_string(),
            uri: uri.to_string(),
        })],
        meta: None,
    })
    .expect("resource read result serialization");

    json_rpc_result(id, result)
}
