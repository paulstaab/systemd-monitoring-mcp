# systemd-monitoring-mcp Requirements

## 1. Goal and Scope

Implement an MCP server for monitoring systemd units and journald logs over the MCP protocol.

MVP scope is limited to:
- Exposing a standards-compliant MCP JSON-RPC endpoint.
- Providing monitoring capabilities via MCP tools and resources.
- Listing systemd `*.service` units and their current state.
- Reading journald logs with optional filtering and limiting.
- Restricting access using a static token configured via environment variable.

Out of scope for MVP:
- Starting, stopping, restarting, or modifying units.
- Non-service unit types (sockets, timers, mounts, etc.).

## 2. Runtime and Configuration

The server must be configurable via environment variables:
- `MCP_API_TOKEN` (required): static bearer token used for API authentication. Must be at least 16 characters long.
- `BIND_ADDR` (optional): bind address, default `127.0.0.1`.
- `BIND_PORT` (optional): bind port, default `8080`.
- `MCP_ALLOWED_CIDR` (optional): CIDR range allowlist for incoming request source IPs.
- `MCP_TRUSTED_PROXIES` (optional): comma-separated list of CIDR ranges identifying trusted reverse proxies. When set and the direct peer IP matches a trusted proxy, the client IP is extracted from the `X-Forwarded-For` header (leftmost entry).

Startup behavior:
- If `MCP_API_TOKEN` is missing or empty, server startup must fail with a clear error message.
- If `MCP_API_TOKEN` is shorter than 16 characters, server startup must fail with a clear error message.
- If optional bind values are missing, defaults must be applied.
- If `MCP_ALLOWED_CIDR` is set but invalid, server startup must fail with a clear error message.
- If `MCP_TRUSTED_PROXIES` is set but contains an invalid CIDR, server startup must fail with a clear error message.
- If systemd is not available on the host/runtime environment, server startup must fail with a clear error message.

## 3. MCP Protocol Requirements

### 3.1 Transport and Routing
- MCP requests must be accepted via HTTP `POST` on `/mcp`.
- `POST /` must not act as an MCP alias.
- `GET /health` may be exposed as an operational endpoint and must not expose sensitive information.
- Discovery metadata endpoint (`/.well-known/mcp`) may be exposed publicly and must advertise MCP endpoint path(s) only.

### 3.2 Core JSON-RPC Semantics
- Must accept JSON-RPC 2.0 request envelopes.
- Must support both single-request and batch-request payloads.
- Must support notifications (`id` absent) and return no response body for notification-only requests.
- Must return JSON-RPC error `-32700` for invalid JSON payloads.
- Must return JSON-RPC error `-32600` for invalid request envelopes.
- Must return JSON-RPC error `-32601` for unknown methods.
- Must return JSON-RPC error `-32602` for invalid method parameters.

### 3.3 MCP Handshake and Capability Advertising
- `initialize` must return:
  - `protocolVersion` selected by server using client-offered version negotiation rules.
  - `serverInfo` object with `name` and `version`.
  - `capabilities` object reflecting actual server behavior.
- `initialize` requests must include required `params` fields: `protocolVersion`, `clientInfo`, and `capabilities`.
- Capability flags and shapes must be consistent with implemented methods.

### 3.4 MCP Tools
- The server must implement `tools/list`.
- The server must implement `tools/call`.
- `tools/list` must advertise strict input schemas and stable output schemas for each tool.
- `tools/call` success responses must place canonical machine-readable JSON results in `structuredContent`.
- `tools/call` may include optional human-readable `content`, but `structuredContent` is required for successful tool calls.
- Minimum required tools:
  - `list_services`: lists service-unit status records.
  - `list_logs`: queries journald logs.

`list_services` behavior:
- Must return only `*.service` units.
- Must return all matching units in a single result.
- Results must be ordered alphabetically by unit name.
- Input parameters:
  - `state` optional service state filter (`active`, `inactive`, `failed`, `activating`, `deactivating`, `reloading`).
- If `state` is provided, only services matching that state must be returned.
- `state` matching must be case-insensitive.
- Each item must contain:
  - `name` (string)
  - `state` (string)
  - `description` (string or null)

`list_logs` behavior:
- Must return log entries in descending timestamp order (newest first).
- Input parameters:
  - `priority` optional minimum severity threshold (`0..7`) or aliases (`emerg`, `alert`, `crit`, `err`, `warning`, `notice`, `info`, `debug`).
  - `unit` optional systemd unit identifier.
  - `start_utc` required RFC3339 UTC timestamp (`Z` suffix).
  - `end_utc` required RFC3339 UTC timestamp (`Z` suffix).
  - `limit` optional cap in range `1..1000`.
- `unit` must contain only ASCII alphanumeric, `.`, `-`, `_`, `@`, and `:`.
- Output entries must contain:
  - `timestamp_utc` (string)
  - `timestamp_unix_usec` (number)
  - `unit` (string or null)
  - `priority` (number or null)
  - `message` (string or null)

### 3.5 MCP Resources
- The server must implement `resources/list`.
- The server must implement `resources/read`.
- Minimum resources:
  - Service snapshot resource with fixed URI `resource://services/snapshot`.
  - Failed service snapshot resource with fixed URI `resource://services/failed`.
  - Logs snapshot resource with fixed URI `resource://logs/recent`.
- Failed service snapshot resource must return only services where `state` is `failed`.
- Resource metadata in `resources/list` must include stable identifiers and human-readable names.
- `resources/read` must return data in documented, schema-stable shapes.
- `resources/read` successful responses must follow MCP `ReadResourceResult` shape (`contents`) without additional non-schema top-level fields.

## 4. Authentication and Security

Token validation:
- Bearer token comparison must use a constant-time algorithm (HMAC-based) to prevent timing side-channel attacks.
- `MCP_API_TOKEN` must be at least 16 characters; shorter values must be rejected at startup.
- Requests to MCP protocol endpoint(s) without an `Authorization` header must be rejected.
- Requests to MCP protocol endpoint(s) with a non-bearer scheme or invalid token must be rejected.

Status codes:
- `401 Unauthorized` for missing or invalid token.
- `500 Internal Server Error` for server-side transport failures.

CORS:
- CORS must not be enabled in MVP (server-to-server usage only).

Input Validation:
- All tool and resource input parameters must be strictly validated.

Request source IP allowlist:
- If `MCP_ALLOWED_CIDR` is not set, no source-IP filtering is applied.
- If `MCP_ALLOWED_CIDR` is set, requests from source IPs outside the range must be rejected.
- When `MCP_TRUSTED_PROXIES` is configured and the direct peer IP matches a trusted proxy CIDR, the client IP must be extracted from the leftmost entry of the `X-Forwarded-For` header.
- If the direct peer is a trusted proxy but `X-Forwarded-For` is missing or contains an invalid client IP, the request must be rejected with `403 Forbidden`.
- If the direct peer is not a trusted proxy, `X-Forwarded-For` must be ignored and the socket-level peer IP is used.
- Rejected requests must return `403 Forbidden` with the standard JSON error shape.

## 5. Error Model

HTTP-level error responses (non-2xx) must use structured JSON:

```json
{
	"code": "string",
	"message": "string",
	"details": {}
}
```

MCP method failures must use JSON-RPC error objects with:
- stable machine-readable error codes,
- human-readable messages,
- optional structured error data.

Rules:
- Internal failures exposed to MCP clients must remain opaque and must not leak sensitive diagnostics.
- Server logs may include internal diagnostic details for operators.

## 6. Logging Requirements

Minimum required logs:
- Startup logs including effective bind address/port.
- Authentication failure logs for rejected MCP requests.
- Request summary logs (method, path, status, duration).
- MCP action audit logs at INFO level for handled MCP methods, including method name, redacted params, and outcome (`success` or `failure`).
- MCP method-level failure logs with stable error identifiers.

Sensitive data handling:
- Never log `MCP_API_TOKEN` value.
- Never log bearer token values from requests.
- Never log raw credentials contained in MCP params.

