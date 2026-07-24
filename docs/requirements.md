# systemd-monitoring-mcp Requirements

## 1. Goal and Scope

Implement an MCP server for monitoring systemd units and journald logs over the MCP protocol.

MVP scope is limited to:
- Exposing a standards-compliant MCP JSON-RPC endpoint.
- Providing monitoring capabilities via MCP tools and resources.
- Providing authenticated uptime-check endpoints for system and user systemd manager state.
- Listing systemd `*.service` units and their current state.
- Listing systemd `*.timer` units and their scheduling/trigger state.
- Reading journald logs with optional filtering and limiting.
- Restricting access using a static token configured via environment variable.

Out of scope for MVP:
- Starting, stopping, restarting, or modifying units.
- Non-service/non-timer unit types (sockets, mounts, automounts, targets, etc.).

## 2. Runtime and Configuration

The server must be configurable via environment variables:
- `MCP_API_TOKEN` (required): static bearer token used for API authentication. Must be at least 16 characters long.
- `BIND_ADDR` (optional): bind address, default `127.0.0.1`.
- `BIND_PORT` (optional): bind port, default `8080`.

Startup behavior:
- If `MCP_API_TOKEN` is missing or empty, server startup must fail with a clear error message.
- If `MCP_API_TOKEN` is shorter than 16 characters, server startup must fail with a clear error message.
- If optional bind values are missing, defaults must be applied.
- If systemd is not available on the host/runtime environment, server startup must fail with a clear error message.

## 3. MCP Protocol Requirements

### 3.1 Transport and Routing
- MCP requests must be accepted via HTTP `POST` on `/mcp`.
- `POST /` must not act as an MCP alias.
- `GET /health` may be exposed as an operational endpoint and must not expose sensitive information.
- `GET /systemd/system/status` may be exposed as an authenticated operational endpoint for system manager checks.
- `GET /systemd/user/status` may be exposed as an authenticated operational endpoint for user manager checks.
- Systemd status endpoints must return `200 OK` with `scope` and `status` when the manager reports `running`.
- Systemd status endpoints must return `503 Service Unavailable` with structured HTTP error shape when the manager reports any non-running state, including `degraded`.
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
- `tools/list` must include `list_services` usage guidance for supported `scope`, `state`, and `limit` values.
- `tools/list` must include `list_timers` usage guidance for supported `scope`, `state`, `sort`, `order`, and `limit` values.
- `tools/list` must include `list_logs` usage guidance that optional `priority` and `unit` filters are omitted when unset, `priority` is a journald severity threshold rather than a regex field, and `grep` is used for message text or regex-lite matching.
- `tools/call` success responses must place canonical machine-readable JSON results in `structuredContent`.
- `tools/call` may include optional human-readable `content`, but `structuredContent` is required for successful tool calls.
- Minimum required tools:
  - `list_services`: lists service-unit status records.
  - `list_timers`: lists timer-unit scheduling and trigger state.
  - `list_logs`: queries journald logs.

`list_services` behavior:
- Must return only `*.service` units.
- Input parameters:
  - `scope` optional unit-manager scope selector (`system`, `user`, `both`), default `system`.
  - `state` optional service state filter (`active`, `inactive`, `failed`, `activating`, `deactivating`, `reloading`).
  - `name_contains` optional service unit-name substring filter.
  - `limit` optional result cap, default `200`, maximum `1000`.
  - `summary` optional boolean triage mode toggle.
- If `scope=user`, results must be sourced from the user systemd manager.
- If `scope=both`, results must combine system and user manager results.
- Combined `scope=both` results must preserve distinct system and user manager rows even when unit names match.
- If `state` is provided, only services matching that state must be returned.
- `state` matching must be case-insensitive.
- If `name_contains` is provided, only services whose `unit` contains that substring must be returned.
- Default sorting must be by `unit` ascending.
- If `state=failed`, sorting must be failed-first and then by `unit` ascending.
- Each item must contain:
  - `unit` (string)
  - `scope` (string: `system` or `user`)
  - `description` (string)
  - `load_state` (string)
  - `active_state` (string)
  - `sub_state` (string)
  - `unit_file_state` (string or null)
  - `since_utc` (RFC3339 UTC string or null)
  - `main_pid` (integer or null)
  - `exec_main_status` (integer or null)
  - `result` (string or null)
- `list_services` response metadata must include:
  - `total` (integer): total matches before applying `limit`
  - `returned` (integer): count of returned rows
  - `truncated` (boolean): true when `total > returned`
  - `generated_at_utc` (RFC3339 UTC string)
- `list_services` summary responses must include the same response metadata fields as detailed responses.
- If `summary=true`, `list_services` must return a compact summary block including:
  - `counts_by_active_state` (map)
  - `failed_units` (array of objects with `unit`, `sub_state`, `result`, `since_utc`)
  - `degraded_hint` (string or null)

`list_logs` behavior:
- Input parameters:
  - `scope` optional journal scope selector (`system`, `user`, `both`), default `system`.
  - `priority` optional minimum severity threshold (`0..7`) or aliases (`emerg`, `alert`, `crit`, `err`, `warning`, `notice`, `info`, `debug`).
  - `unit` optional systemd unit identifier.
  - `start_utc` required RFC3339 UTC timestamp (`Z` suffix).
  - `end_utc` required RFC3339 UTC timestamp (`Z` suffix).
  - `grep` optional substring filter or regex-lite pattern.
  - `exclude_units` optional array of unit names to exclude.
  - `order` optional sort order (`asc` or `desc`), default `desc`.
  - `allow_large_window` optional boolean override for large time ranges.
  - `limit` optional cap in range `1..1000`, default `200`.
  - `summary` optional boolean triage mode toggle.
- `unit` must contain only ASCII alphanumeric, `.`, `-`, `_`, `@`, and `:`.
- `exclude_units` entries must contain only ASCII alphanumeric, `.`, `-`, `_`, `@`, and `:`.
- If `scope=user`, journal reads must target user-unit records.
- If `scope=both`, journal reads must include both system and user-unit records.
- `start_utc` must be strictly less than `end_utc`.
- Time windows larger than 7 days must be rejected unless `allow_large_window=true`.
- Output entries must contain:
  - `timestamp_utc` (string)
  - `unit` (string or null)
  - `priority` (string or number, normalized)
  - `hostname` (string or null)
  - `pid` (number or null)
  - `message` (string or null)
  - `cursor` (string or null)
- `message` values must be trimmed and sanitized for control characters.
- `list_logs` response metadata must include:
  - `total_scanned` (integer or null)
  - `returned` (integer)
  - `truncated` (boolean): true only when additional matching rows are known to exist beyond `limit`
  - `generated_at_utc` (RFC3339 UTC string)
  - `window` object containing `start_utc` and `end_utc`
- `list_logs` summary responses must include the same response metadata fields as detailed responses.
- If `summary=true`, `list_logs` must return a compact summary block including:
  - `counts_by_unit` (top 10)
  - `counts_by_priority`
  - `top_messages` (deduplicated frequent messages, top 10)
  - `error_hotspots` (units with highest error count)

`list_timers` behavior:
- Input parameters:
  - `scope` optional unit-manager scope selector (`system`, `user`, `both`), default `system`.
  - `limit` optional cap in range `1..1000`, default `200`.
  - `name_contains` optional case-insensitive timer unit-name substring filter.
  - `state` optional timer state filter (free-form string; matching must be case-insensitive).
  - `summary` optional boolean triage mode toggle.
  - `include_persistent` optional boolean to include persistent/missed-run capability field.
  - `overdue_only` optional boolean to return only timers that are considered overdue.
  - `sort` optional sort key (`next`, `last`, `name`, `state`), default `name`.
  - `order` optional sort order (`asc` or `desc`), default `asc`.
- If `state` is provided, only timers matching that state must be returned.
- If `name_contains` is provided, only timers whose `unit` contains that substring (case-insensitive) must be returned.
- If `scope=user`, results must be sourced from the user systemd manager.
- If `scope=both`, results must combine system and user manager results.
- Combined `scope=both` results must preserve distinct system and user manager rows even when unit names match.
- Invalid `limit`, `sort`, or `order` values must return JSON-RPC error `-32602` with stable machine-readable error codes.
- Invalid parameter types for any `list_timers` input must return JSON-RPC error `-32602` with stable machine-readable error codes.
- Timer metadata collection failures must not fail the whole response; unresolved fields must be returned as `null` where applicable.
- Each timer item must include at least:
  - `unit` (string)
  - `scope` (string: `system` or `user`)
  - `active_state` (string)
  - `sub_state` (string)
  - `next_run_utc` (RFC3339 UTC string or null)
  - `last_run_utc` (RFC3339 UTC string or null)
  - `time_until_next_sec` (integer or null)
  - `time_since_last_sec` (integer or null)
  - `trigger_unit` (string or null)
  - `persistent` (boolean or null)
  - `result` (string or null)
  - `load_state` (string or null)
  - `unit_file_state` (string or null)
  - `overdue` (boolean)
  - `overdue_reason` (string or null)
- `list_timers` response metadata must include:
  - `total_scanned` (integer)
  - `returned` (integer)
  - `truncated` (boolean)
  - `generated_at_utc` (RFC3339 UTC string)
- `list_timers` summary responses must include the same response metadata fields as detailed responses.
- Overdue detection rules:
  - A timer is considered overdue only when all are true:
    - `next_run_utc` is known,
    - current UTC time is later than `next_run_utc + 300 seconds` (hard-coded 5 minute grace),
    - `active_state` is `active`.
  - Timers with no `next_run_utc` must not be marked overdue by default (to avoid one-shot/completed false positives).
  - When uncertainty exists, `overdue=false` and `overdue_reason` must explain the uncertainty (for example `no_next_run_known`, `not_active`, `insufficient_schedule_data`).
- If `summary=true`, `list_timers` must return a compact summary block including:
  - `counts_by_active_state` (map)
  - `overdue_count` (integer)
  - `next_due_soon` (array, top 5 upcoming timers)
  - `failed_or_problem_timers` (array of timers with failed/problematic state/result, may be empty)

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

Security objective:

Case 1 — network access without a valid token:
- The actor must not be able to invoke MCP capabilities or authenticated operational endpoints.
- The actor must not obtain host monitoring data or secrets.
- The actor must not make persistent changes to the host or its workloads through this server.
- The actor must not trigger monitoring-provider work; authentication must fail before provider access.
- The actor may obtain the fixed public health response, public discovery metadata, and network-level signals. These are accepted non-critical disclosures.

Case 2 — network access with a valid token, including a malicious client or runaway agent:
- The actor may invoke documented read-only monitoring capabilities and obtain the non-critical monitoring information they return.
- The actor must not obtain intentionally held secrets, including credentials, tokens, environment secrets, host mount sources, or other secret-bearing provider fields.
- The actor must not make persistent changes to the host or its workloads through this server.
- Denial of service against the server is an accepted risk for this actor.

For both cases:
- No response or application log may intentionally expose secrets.
- All current and future MCP capabilities and provider adapters must remain read-only. Mutating systemd, journal, process, filesystem, container, pod, network, package, or host-configuration operations are out of scope.
- Data that an application writes into an otherwise permitted free-form monitoring field, especially a journal message, cannot be reliably classified as secret. Operators must not write secrets to monitoring sources exposed by this server.

Token validation:
- Bearer token comparison must use a constant-time algorithm (HMAC-based) to prevent timing side-channel attacks.
- `MCP_API_TOKEN` must be at least 16 characters; shorter values must be rejected at startup.
- Requests to protected endpoint(s) without an `Authorization` header must be rejected.
- Requests to protected endpoint(s) with a non-bearer scheme or invalid token must be rejected.

Status codes:
- `401 Unauthorized` for missing or invalid token.
- `500 Internal Server Error` for server-side transport failures.

CORS:
- CORS must not be enabled in MVP (server-to-server usage only).

Input Validation:
- All tool and resource input parameters must be strictly validated.

Tool safety constraints:
- Timer tooling must remain read-only and must not start, stop, or modify timers/services.
- Provider inputs must be strictly validated and passed through typed APIs or fixed argument vectors without shell interpretation.
- Authentication, validation, and client-facing provider failures must not include host-derived diagnostics or secret data.
- Response minimization and log redaction must exclude known secret-bearing fields, credentials, environment values, and host mount sources.
- Security assumptions, attack scenarios, controls, and residual risks are documented in `docs/security.md`.

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
- Never expose service environment or secret values while reporting timer trigger data.

## 7. Structured Runtime Inspection and Paginated Logs

### 7.1 Detailed Unit Status

- `get_unit_status` requires a valid `*.service` `unit`, accepts `scope=system|user` (default `system`), and accepts `transition_limit=1..100` (default `20`).
- The response contains the service fields plus nullable `exec_main_status`, `result`, `restart_count`, and `timestamps` fields for state change, active/inactive enter/exit, and main-process start/exit.
- Each optional D-Bus property is best-effort independently; one unavailable property must not erase other enrichment.
- `failed_dependencies` contains failed, missing, or unloaded direct `Requires`/`Wants` only and does not recurse.
- `recent_transitions` is newest-first and contains timestamp, transition kind, message, and cursor for recognized systemd transition message IDs.
- `list_services` adds `restart_count` and the timestamp object and retains `since_utc`.

### 7.2 Podman Inspection

- `get_container_status(container)` and `get_pod_status(pod)` provide compact read-only local inspection.
- Podman is optional, is not checked at startup, and is invoked without a shell using validated identifiers and individual fixed arguments.
- CLI execution has a short timeout and bounded stdout/stderr. Missing CLI/runtime maps to `podman_unavailable`, unknown targets to `container_not_found`/`pod_not_found`, and malformed or oversized results to stable provider errors.
- Container results include compact state, exit/error and lifecycle timestamps, restart count, image identity, configured and nullable runtime/host identity, mounts/read-only flags, health status/config without logs, argv commands, and pod ID.
- Pod results include ID, name, state, creation time, restart policy, infra ID, shared namespaces, and compact member state.
- Labels, annotations, OCI blobs, health logs, and other verbose inspect metadata are excluded.

### 7.3 Resumable Logs

- `list_logs` accepts optional `cursor`, unique `fields`, `group_by=message`, and `since_last_start`.
- `fields` may contain only `timestamp_utc`, `unit`, `priority`, `hostname`, `pid`, `message`, and `cursor`; omission returns all fields.
- A cursor resumes exclusively in the selected order. Invalid/expired cursors return `invalid_cursor`; callers must retain the original scope, filters, window, grouping, and projection.
- `next_cursor` is returned only when another matching raw row exists. Page metadata describes the current page.
- `since_last_start=true` requires exactly one unit and no explicit `start_utc`, derives the bound from its latest main-process start, and returns `unit_start_unavailable` when unknown.
- Plain `grep` remains case-sensitive literal matching. Slash-delimited values are Rust regular expressions, including alternation such as `/ERROR|fatal/`.
- `group_by=message` groups identical `(unit, priority, message)` values within the fetched raw page and adds `count`, `first_timestamp_utc`, and `last_timestamp_utc`; continuation advances over raw rows.
- The seven-day window is inclusive. Oversized-window errors include `maximum_start_utc = end_utc - 7 days` in structured details.

### 7.4 Podman Inspection Data Minimization

- Container inspection must not return Podman `CreateCommand`, host mount source paths, environment data, labels, annotations, health logs, or other raw inspect configuration.
- `command` remains argv-shaped when Podman provides an array, but values associated with credential-like flags or assignments must be replaced with `[REDACTED]`.
- `health_config` may contain only sanitized `test` argv and non-secret timing/retry fields: `interval`, `timeout`, `start_period`, `start_interval`, and `retries`.
- Credential detection is case-insensitive and covers password, secret, token, credential, authorization, bearer, API-key, and API-key spelling variants in `--name value`, `--name=value`, and `NAME=value` forms.
- Mount entries may contain type, container destination, and read-only state only.
