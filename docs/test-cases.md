# Test Cases

## HTTP Transport and Security

- `GET /health` returns `200` with `{ "status": "ok" }` and no sensitive fields.
- `POST /mcp` without authorization header returns `401` with `missing_token` code.
- `POST /mcp` with non-bearer auth scheme returns `401` with `invalid_token` code.
- `POST /mcp` with invalid bearer token returns `401` with `invalid_token` code.
- If `MCP_ALLOWED_CIDR` is configured, `POST /mcp` from outside range returns `403` with `ip_restricted` code.
- If request comes from a trusted proxy and `X-Forwarded-For` is missing, request is rejected with `403` and `ip_restricted` code.
- If request comes from a trusted proxy and `X-Forwarded-For` leftmost IP is invalid, request is rejected with `403` and `ip_restricted` code.
- If request comes from an untrusted peer, `X-Forwarded-For` is ignored and peer socket IP is used for allowlist checks.

## MCP Discovery and Initialize

- `GET /.well-known/mcp` includes `name`, `version`, and `mcp_endpoint` and does not advertise REST business endpoints.
- `POST /mcp` with `initialize` returns `200` JSON-RPC response with `protocolVersion`, `serverInfo`, and `capabilities`.
- `POST /mcp` with `initialize` missing required params returns JSON-RPC error `-32602`.
- `POST /` does not provide MCP behavior and returns non-success (for example `404` or `405`).
- `initialize` capability object matches implemented MCP methods (no unsupported capability flags).

## JSON-RPC Compliance

- Invalid JSON payload returns JSON-RPC error `-32700`.
- Envelope with missing/invalid required fields returns JSON-RPC error `-32600`.
- Unknown method returns JSON-RPC error `-32601`.
- Method with invalid params returns JSON-RPC error `-32602`.
- Notification request (no `id`) yields no JSON-RPC response body.
- Batch request with all notifications yields no JSON-RPC response body.
- Batch request with mixed notifications and standard calls returns responses only for calls with `id`.
- Batch request preserving request IDs returns corresponding response IDs for each completed call.

## MCP Tools

- `tools/list` returns `200` JSON-RPC result containing at least `list_services` and `list_logs`.
- `tools/list` entries include strict input schema and stable output schema metadata.
- Successful `tools/call` responses include canonical machine-readable JSON in `structuredContent`.
- `tools/call` with unknown tool name returns JSON-RPC error `-32601` (or project-defined equivalent for tool-not-found) with stable error data.

### Tool: `list_services`

- `tools/call` for `list_services` returns only `*.service` units.
- `list_services` output is sorted alphabetically by unit `name`.
- Each output item includes `name`, `state`, and `description` (`string` or `null`).

### Tool: `list_logs`

- `tools/call` for `list_logs` returns log entries in descending timestamp order (newest first).
- `list_logs` with `limit=0` returns JSON-RPC error `-32602` with stable error code `invalid_limit`.
- `list_logs` with `limit=1001` returns JSON-RPC error `-32602` with stable error code `invalid_limit`.
- `list_logs` with `priority=error` normalizes to journald priority `3` and applies minimum-threshold filtering.
- `list_logs` with `priority=9` returns JSON-RPC error `-32602` with stable error code `invalid_priority`.
- `list_logs` with `unit=sshd_service-01@host:prod` returns only matching unit entries.
- `list_logs` with disallowed unit characters (for example `/`) returns JSON-RPC error `-32602` with stable error code `invalid_unit`.
- `list_logs` with missing `start_utc` and/or `end_utc` returns JSON-RPC error `-32602` with stable error code `missing_time_range`.
- `list_logs` with `start_utc > end_utc` returns JSON-RPC error `-32602` with stable error code `invalid_time_range`.
- `list_logs` with non-UTC timestamp offset (`+01:00`) returns JSON-RPC error `-32602` with stable error code `invalid_utc_time`.

## MCP Resources

- `resources/list` returns at least two resources: service snapshot and logs snapshot.
- `resources/list` includes fixed URIs `resource://services/snapshot` and `resource://logs/recent`.
- `resources/list` includes stable resource identifiers and human-readable names.
- `resources/read` for service snapshot returns schema-stable data matching service output model.
- `resources/read` for logs snapshot returns schema-stable data matching log output model.
- Successful `resources/read` responses use MCP `contents` shape and do not include non-schema top-level fields.
- `resources/read` for unknown resource returns JSON-RPC error `-32601` (or project-defined equivalent) with stable error data.

## Error and Observability Behavior

- Internal tool execution failures return opaque client-facing errors (no raw system diagnostics in JSON-RPC error message/data).
- HTTP non-2xx errors retain `{ code, message, details }` JSON shape.
- Startup and request logs include method/path/status/duration and authentication failures, without exposing token values.
- Every action executed through the MCP server emits an INFO-level audit log event.
- Audit log events include action parameters with sensitive fields redacted (for example token, password, secret, credentials).
