//! JSON-RPC protocol representations and formatting utilities
//!
//! Provides standardized mapping of internal AppErrors to valid JSON-RPC payloads.

use serde_json::{json, Value};
use rust_mcp_sdk::schema::{JsonrpcErrorResponse, JsonrpcResultResponse, RequestId, Result as McpResult, RpcError};
use crate::errors::AppError;

pub fn is_json_rpc_error(value: &Value) -> bool {
    value.get("error").is_some()
}

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
        AppError::Internal { .. } | AppError::NotImplemented { .. } => {
            json_rpc_error(id, -32603, "Internal error")
        }
    }
}

pub fn json_rpc_error(id: Option<Value>, code: i32, message: &str) -> Value {
    json_rpc_error_with_data(id, code, message, None)
}

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

pub fn value_to_request_id(value: &Value) -> Option<RequestId> {
    if let Some(string_id) = value.as_str() {
        return Some(RequestId::String(string_id.to_string()));
    }

    value.as_i64().map(RequestId::Integer)
}

pub fn request_id_to_value(id: RequestId) -> Value {
    match id {
        RequestId::String(value) => Value::String(value),
        RequestId::Integer(value) => Value::Number(value.into()),
    }
}
