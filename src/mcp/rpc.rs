//! JSON-RPC protocol representations and formatting utilities
//!
//! Provides standardized mapping of internal AppErrors to valid JSON-RPC payloads.

use crate::errors::AppError;
use rust_mcp_sdk::schema::{
    JsonrpcErrorResponse, JsonrpcResultResponse, RequestId, Result as McpResult, RpcError,
};
use serde_json::{json, Value};

/// Returns `true` when a JSON-RPC response payload contains an `error` object.
pub fn is_json_rpc_error(value: &Value) -> bool {
    value.get("error").is_some()
}

/// Maps internal `AppError` values to stable JSON-RPC error responses.
///
/// Validation failures map to `-32602`, auth failures to `-32001`, and internal
/// failures to opaque `-32603` responses.
pub fn app_error_to_json_rpc(id: Option<Value>, err: AppError) -> Value {
    match err {
        AppError::BadRequest { code, message } => json_rpc_error_with_data(
            id,
            -32602,
            "Invalid params",
            Some(json!({
                "code": code,
                "message": message,
                "details": {}
            })),
        ),
        AppError::Unauthorized { code, message } | AppError::Forbidden { code, message } => {
            json_rpc_error_with_data(
                id,
                -32001,
                "Unauthorized",
                Some(json!({
                    "code": code,
                    "message": message,
                    "details": {}
                })),
            )
        }
        AppError::Internal { .. } | AppError::NotImplemented { .. } => json_rpc_internal_error(id),
    }
}

/// Creates the canonical JSON-RPC parse error (`-32700`) response.
///
/// Use this for malformed JSON payloads before request-envelope validation.
pub fn json_rpc_parse_error(id: Option<Value>) -> Value {
    json_rpc_error(id, -32700, "Parse error")
}

/// Creates the canonical JSON-RPC invalid-request error (`-32600`) response.
///
/// Use this when a JSON-RPC envelope is structurally invalid for dispatch.
pub fn json_rpc_invalid_request(id: Option<Value>) -> Value {
    json_rpc_error(id, -32600, "Invalid Request")
}

/// Creates the canonical JSON-RPC invalid-params error (`-32602`) response.
///
/// Use this for malformed method params that fail shape or schema checks.
pub fn json_rpc_invalid_params(id: Option<Value>) -> Value {
    json_rpc_error(id, -32602, "Invalid params")
}

/// Creates the canonical JSON-RPC method-not-found error (`-32601`) response.
///
/// Use this when no registered method/tool/resource handler matches the request.
pub fn json_rpc_method_not_found(id: Option<Value>) -> Value {
    json_rpc_error(id, -32601, "Method not found")
}

/// Creates a JSON-RPC method-not-found error (`-32601`) response with `data`.
///
/// Use this when returning stable machine-readable details for unknown methods.
pub fn json_rpc_method_not_found_with_data(id: Option<Value>, data: Value) -> Value {
    json_rpc_error_with_data(id, -32601, "Method not found", Some(data))
}

/// Creates the canonical JSON-RPC internal-error response (`-32603`).
///
/// Keep this opaque to avoid leaking internal diagnostics to clients.
pub fn json_rpc_internal_error(id: Option<Value>) -> Value {
    json_rpc_error(id, -32603, "Internal error")
}

/// Creates a JSON-RPC error response without additional `data` payload.
pub fn json_rpc_error(id: Option<Value>, code: i32, message: &str) -> Value {
    json_rpc_error_with_data(id, code, message, None)
}

/// Creates a JSON-RPC error response with optional structured `data` details.
pub fn json_rpc_error_with_data(
    id: Option<Value>,
    code: i32,
    message: &str,
    data: Option<Value>,
) -> Value {
    let response = JsonrpcErrorResponse::new(
        RpcError {
            code: i64::from(code),
            data,
            message: message.to_string(),
        },
        id.as_ref().and_then(value_to_request_id),
    );
    serde_json::to_value(response).expect("jsonrpc error response serialization")
}

/// Creates a JSON-RPC result response preserving request id semantics.
///
/// If id conversion fails, this falls back to a raw JSON-RPC result envelope.
pub fn json_rpc_result(id: Option<Value>, result: Value) -> Value {
    if let Some(request_id) = id.as_ref().and_then(value_to_request_id) {
        let extra = result.as_object().cloned();
        let response = JsonrpcResultResponse::new(request_id, McpResult { meta: None, extra });
        return serde_json::to_value(response).expect("jsonrpc result response serialization");
    }

    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

/// Converts a JSON value into MCP `RequestId` when possible.
pub fn value_to_request_id(value: &Value) -> Option<RequestId> {
    if let Some(string_id) = value.as_str() {
        return Some(RequestId::String(string_id.to_string()));
    }

    value.as_i64().map(RequestId::Integer)
}

/// Converts MCP `RequestId` back into a JSON value for response shaping.
pub fn request_id_to_value(id: RequestId) -> Value {
    match id {
        RequestId::String(value) => Value::String(value),
        RequestId::Integer(value) => Value::Number(value.into()),
    }
}
