# systemd-monitoring-mcp Requirements

## 1. Goal and Scope

Implement an MCP server that exposes a REST API for monitoring systemd units.

MVP scope is limited to:
- Listing systemd service units and their current state.
- Exposing the server over HTTP.
- Restricting access using a static token configured via environment variable.

Out of scope for MVP:
- Starting, stopping, restarting, or modifying units.
- Pagination, filtering, or search.
- Non-service unit types (sockets, timers, mounts, etc.).

## 2. Runtime and Configuration

The server must be configurable via environment variables:
- `MCP_API_TOKEN` (required): static bearer token used for API authentication.
- `BIND_ADDR` (optional): bind address, default `0.0.0.0`.
- `BIND_PORT` (optional): bind port, default `8080`.

Startup behavior:
- If `MCP_API_TOKEN` is missing or empty, server startup must fail with a clear error message.
- If optional bind values are missing, defaults must be applied.

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

### 3.2 Endpoint: List Units
- Method: `GET`
- Path: `/units`
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

## 4. Authentication and Security

Token validation:
- Requests to `/units` without an `Authorization` header must be rejected.
- Requests to `/units` with a non-bearer scheme or invalid token must be rejected.

Status codes:
- `401 Unauthorized` for missing or invalid token.
- `500 Internal Server Error` for server-side failures (for example systemd query failure).

CORS:
- CORS must not be enabled in MVP (server-to-server usage only).

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
- Authentication failure logs for rejected `/units` requests.
- Request summary logs (method, path, status, duration).

Sensitive data handling:
- Never log `MCP_API_TOKEN` value.
- Never log bearer token values from requests.
