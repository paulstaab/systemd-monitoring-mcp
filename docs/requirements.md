# systemd-monitoring-mcp Requirements

## 1. Goal and Scope

Implement an MCP server that exposes a REST API for monitoring systemd units.

MVP scope is limited to:
- Listing systemd service units and their current state.
- Reading journald logs with optional filtering and ordering.
- Exposing the server over HTTP.
- Restricting access using a static token configured via environment variable.

Out of scope for MVP:
- Starting, stopping, restarting, or modifying units.
- Non-service unit types (sockets, timers, mounts, etc.).

## 2. Runtime and Configuration

The server must be configurable via environment variables:
- `MCP_API_TOKEN` (required): static bearer token used for API authentication.
- `BIND_ADDR` (optional): bind address, default `127.0.0.1`.
- `BIND_PORT` (optional): bind port, default `8080`.
- `MCP_ALLOWED_CIDR` (optional): CIDR range allowlist for incoming request source IPs.

Startup behavior:
- If `MCP_API_TOKEN` is missing or empty, server startup must fail with a clear error message.
- If optional bind values are missing, defaults must be applied.
- If `MCP_ALLOWED_CIDR` is set but invalid, server startup must fail with a clear error message.
- If systemd is not available on the host/runtime environment, server startup must fail with a clear error message.

## 3. API Requirements

### 3.1 Endpoint: Health
- Method: `GET`
- Path: `/health`
- Authentication: not required (public endpoint).
- Response on success: HTTP `200` with JSON status payload.

Minimum response body:
```json
{
	"status": "ok"
}
```

### 3.2 Endpoint: List Services
- Method: `GET`
- Path: `/services`
- Authentication: required via `Authorization: Bearer <token>` header.

Behavior:
- Must return only `*.service` units.
- Must return all matching units in a single response (no pagination).
- Results must be ordered alphabetically by unit name.

Response on success:
- HTTP `200`
- JSON array where each item contains:
	- `name` (string): unit name (for example `sshd.service`).
	- `state` (string): raw systemd `ActiveState` value (for example `active`, `inactive`, `failed`, etc.).
	- `description` (string or null): systemd unit description; `null` when unavailable.

Example:
```json
[
	{
		"name": "cron.service",
		"state": "active",
		"description": "Regular background program processing daemon"
	},
	{
		"name": "example.service",
		"state": "failed",
		"description": null
	}
]
```

### 3.3 Endpoint: MCP Discovery
- Method: `GET`
- Path: `/.well-known/mcp`
- Authentication: not required (public endpoint).

Behavior:
- Must return discovery metadata for this server.
- Must advertise the MCP endpoint path.

Minimum response body fields:
- `name` (string): server name.
- `version` (string): server version.
- `mcp_endpoint` (string): MCP protocol endpoint path.
- `services_endpoint` (string): REST endpoint path for service listing.
- `logs_endpoint` (string): REST endpoint path for journald log listing.

### 3.4 Endpoint: MCP Protocol (Minimal JSON-RPC)
- Method: `POST`
- Path: `/mcp` (primary) and `/` (compatibility alias for MCP clients that post to root)
- Authentication: not required for initial infrastructure scaffold.

Behavior:
- Must accept JSON-RPC 2.0 request envelopes.
- Must return JSON-RPC 2.0 response envelopes.
- Both `/mcp` and `/` must provide equivalent JSON-RPC behavior.
- Initial supported methods:
	- `initialize`
	- `ping`

Method semantics:
- `initialize`: returns MCP-style handshake metadata including:
	- `protocolVersion` (string)
	- `serverInfo` object with `name` and `version`
	- `capabilities` object containing `tools`, `resources`, and `prompts` sub-objects
	- `metadata.restEndpoints.services` with value `/services` to advertise the services endpoint to MCP clients
	- `metadata.restEndpoints.logs` with value `/logs` to advertise the logs endpoint to MCP clients
- `ping`: returns an empty JSON object as result.

Error handling:
- Unknown methods must return JSON-RPC error `-32601` (Method not found).
- Invalid request envelopes must return JSON-RPC error `-32600` (Invalid Request).
- Invalid JSON payload must return JSON-RPC error `-32700` (Parse error).

### 3.5 Endpoint: List Logs
- Method: `GET`
- Path: `/logs`
- Authentication: required via `Authorization: Bearer <token>` header.

Behavior:
- Must return journald log entries in a single response.
- Must always sort logs by timestamp.
- Default sort order is ascending (`asc`) by timestamp.
- Optional query parameter `order` supports `asc` or `desc`.
- Optional query parameter `priority` applies a minimum severity threshold for systemd/journald priority (`0..7`) or common aliases (`emerg`, `alert`, `crit`, `err`, `warning`, `notice`, `info`, `debug`), returning that priority and higher-severity entries.
- Optional query parameter `unit` filters by systemd unit identifier.
- `unit` must use strict parameter validation and contain only ASCII alphanumeric characters, dots (`.`), dashes (`-`), underscores (`_`), at-sign (`@`), and colon (`:`).
- Required query parameters `start_utc` and `end_utc` filter by inclusive UTC timerange.
- Timerange values must be RFC3339 UTC timestamps (`Z` suffix).
- Optional query parameter `limit` caps number of returned entries.
- `limit` must be `>= 1` and `<= 1000`.
- If `limit` is omitted, a default limit is applied by the server.

Response on success:
- HTTP `200`
- JSON array where each item contains:
	- `timestamp_utc` (string): UTC timestamp (RFC3339 format).
	- `timestamp_unix_usec` (number): journal realtime timestamp in microseconds.
	- `unit` (string or null): `_SYSTEMD_UNIT` when available.
	- `priority` (number or null): journal priority when available.
	- `message` (string or null): log message when available.

## 4. Authentication and Security

Token validation:
- Requests to `/services` without an `Authorization` header must be rejected.
- Requests to `/services` with a non-bearer scheme or invalid token must be rejected.

Status codes:
- `401 Unauthorized` for missing or invalid token.
- `500 Internal Server Error` for server-side failures (for example systemd query failure).

CORS:
- CORS must not be enabled in MVP (server-to-server usage only).

Input Validation:
- All API parameters must be strictly validated.

Request source IP allowlist:
- If `MCP_ALLOWED_CIDR` is not set, no source-IP filtering is applied.
- If `MCP_ALLOWED_CIDR` is set, requests from source IPs outside the range must be rejected.
- Rejected requests must return `403 Forbidden` with the standard JSON error shape.

## 5. Error Response Format

All non-2xx responses from API endpoints must use structured JSON:

```json
{
	"code": "string",
	"message": "string",
	"details": {}
}
```

Rules:
- `code`: stable, machine-readable error identifier.
- `message`: human-readable summary.
- `details`: optional object for additional context; use `{}` when no additional data is provided.

## 6. Logging Requirements

Minimum required logs:
- Startup logs including effective bind address/port.
- Authentication failure logs for rejected `/services` requests.
- Request summary logs (method, path, status, duration).

Sensitive data handling:
- Never log `MCP_API_TOKEN` value.
- Never log bearer token values from requests.
