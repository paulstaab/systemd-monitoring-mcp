# Test Cases

## HTTP Transport and Security

Global admission limiting:

- Configuration defaults to 10 requests per second and burst 20; explicit valid values are retained.
- Zero, malformed, overflowing, and above-maximum rate or burst values fail configuration parsing with field-specific errors.
- A new bucket starts with its full burst, admits the first 20 immediate requests under defaults, and rejects the next request.
- Refill is continuous, reaches admission at the correct fractional interval, never exceeds burst, and computes `Retry-After` rounded up to at least one second without wall-clock sleeps.
- Public, authenticated, unauthenticated, invalid-credential, and unmatched requests consume one shared budget across router clones.
- A rejected `/mcp` or protected request returns HTTP `429` with exact standard JSON body and `Retry-After`, and invokes no authentication, JSON-RPC, systemd, journal, or Podman provider work.
- Request-summary logging observes `429` responses without recording authorization headers or token values.
- Separately constructed application states have independent buckets.
- Existing authentication and HTTP/JSON-RPC error behavior remains unchanged while tokens are available.

Case 1 — network access without a valid token:

- `GET /health` returns `200` with `{ "status": "ok" }` and no sensitive fields.
- `GET /health` has an exact fixed response shape and exposes no host-derived fields.
- `GET /.well-known/mcp` exposes only package name, package version, and the `/mcp` endpoint path.
- Unauthenticated systemd status requests do not invoke providers or include manager state.
- `GET /systemd/system/status` without authorization header returns `401` with `missing_token` code.
- `POST /mcp` without authorization header returns `401` with `missing_token` code.
- `POST /mcp` with non-bearer auth scheme returns `401` with `invalid_token` code.
- `POST /mcp` with malformed authorization header value (header present but unparsable) returns `401` with `invalid_token` code.
- `POST /mcp` with invalid bearer token returns `401` with `invalid_token` code.
- Unauthenticated and invalid-token requests are rejected before any systemd, journal, or Podman provider work.
- Unauthenticated and invalid-token `/mcp` requests cannot invoke tools or resources and include no monitoring data.

Case 2 — network access with a valid token, including a malicious client or runaway agent:

- A valid token permits only documented read-only monitoring operations and non-critical monitoring disclosure.
- `GET /systemd/system/status` with valid bearer token returns `200` with `scope=system` and `status=running` when the system manager is running.
- Repeated or expensive authenticated requests may deny service; this is an accepted risk for Case 2 only.
- `GET /systemd/user/status` with valid bearer token returns `200` with `scope=user` and `status=running` when the user manager is running.
- Systemd status endpoints with valid bearer token return `503` with structured HTTP error body when the manager status is `degraded`.
- Bearer token validation uses HMAC-based fixed-size comparison and rejects same-length and different-length mismatches.
- Advertised and implemented capabilities contain no persistent host- or workload-mutating operation; guessed mutation methods fail closed.
- Hostile identifiers cannot add shell arguments or execute commands.
- Successful responses omit documented secret-bearing provider fields and redact credential-like argv values.
- Application logs omit bearer tokens, environment secrets, and known credential-bearing parameters.
- Client-facing failures do not expose host paths, bus diagnostics, command output, or environment values.
- Free-form fixture data containing secret-like values verifies documented sanitization boundaries without claiming arbitrary journal-secret detection.

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

- `tools/list` returns `200` JSON-RPC result containing at least `list_services`, `list_timers`, and `list_logs`.
- `tools/list` entries include strict input schema and stable output schema metadata.
- `tools/list` `list_services` description explains supported `scope`, `state`, and `limit` values.
- `tools/list` `list_timers` description explains supported `scope`, non-empty `state`, `sort`, `order`, and `limit` values.
- `tools/list` `list_logs` description explains that unset optional `priority` and `unit` filters must be omitted, `priority` is a severity threshold rather than regex, and `grep` is for message matching.
- Successful `tools/call` responses include canonical machine-readable JSON in `structuredContent`.
- `tools/call` with unknown tool name returns JSON-RPC error `-32601` (or project-defined equivalent for tool-not-found) with stable error data.

### Tool: `list_services`

- `tools/call` for `list_services` returns only `*.service` units.
- Each output item includes: `unit`, `description`, `load_state`, `active_state`, `sub_state`, `unit_file_state`, `since_utc`, `main_pid`, `exec_main_status`, `result`.
- `list_services` with `state=failed` returns only services where `active_state` is `failed`.
- `list_services` with mixed-case state input (for example `FaIlEd`) applies a case-insensitive match.
- `list_services` with unsupported `state` value returns JSON-RPC error `-32602` with stable error code `invalid_state`.
- `list_services` with `name_contains=ssh` returns only services whose unit names contain `ssh`.
- `list_services` with both `state` and `name_contains` applies both filters.
- `list_services` defaults to `scope=system` when `scope` is omitted.
- `list_services` with `scope=user` returns user-manager services.
- `list_services` with `scope=both` returns combined system and user-manager services.
- `list_services` with `scope=both` preserves both rows when system and user managers contain the same unit name and identifies each row's source scope.
- `list_services` with unsupported `scope` value returns JSON-RPC error `-32602` with stable error code `invalid_scope`.
- `list_services` defaults to `limit=200` and enforces max `limit=1000`.
- `list_services` with `limit=0` or `limit=1001` returns JSON-RPC error `-32602` with stable error code `invalid_limit`.
- `list_services` default sorting is by `unit` ascending.
- `list_services` with `state=failed` applies failed-first then unit sort order.
- `list_services` structured output includes per-row `scope` and metadata: `total`, `returned`, `truncated`, `generated_at_utc`.
- `list_services` with `summary=true` returns compact `summary` block with `counts_by_active_state`, `failed_units`, and `degraded_hint`, plus metadata: `total`, `returned`, `truncated`, `generated_at_utc`.

### Tool: `list_logs`

- `tools/call` for `list_logs` defaults to descending timestamp order (newest first).
- `list_logs` with `order=asc` returns entries oldest-first within the requested window.
- `list_logs` with `limit=0` returns JSON-RPC error `-32602` with stable error code `invalid_limit`.
- `list_logs` with `limit=1001` returns JSON-RPC error `-32602` with stable error code `invalid_limit`.
- `list_logs` with `priority=error` normalizes to journald priority `3` and applies minimum-threshold filtering.
- `list_logs` with `priority=9` returns JSON-RPC error `-32602` with stable error code `invalid_priority`.
- `list_logs` with `unit=sshd_service-01@host:prod` returns only matching unit entries.
- `list_logs` with disallowed unit characters (for example `/`) returns JSON-RPC error `-32602` with stable error code `invalid_unit`.
- `list_logs` defaults to `scope=system` when `scope` is omitted.
- `list_logs` with `scope=user` returns user-unit journal entries.
- `list_logs` with `scope=both` returns combined system and user-unit journal entries.
- `list_logs` with unsupported `scope` value returns JSON-RPC error `-32602` with stable error code `invalid_scope`.
- `list_logs` with `exclude_units=["sshd.service"]` excludes matching unit entries.
- `list_logs` with invalid `exclude_units` entry characters returns JSON-RPC error `-32602` with stable error code `invalid_unit`.
- `list_logs` with `grep` as substring returns only matching message entries.
- `list_logs` with `grep` regex-lite syntax (for example `/timeout|refused/`) applies regex filtering.
- `list_logs` with invalid regex-lite `grep` returns JSON-RPC error `-32602` with stable error code `invalid_grep`.
- `list_logs` with missing `start_utc` and/or `end_utc` returns JSON-RPC error `-32602` with stable error code `missing_time_range`.
- `list_logs` with `start_utc >= end_utc` returns JSON-RPC error `-32602` with stable error code `invalid_time_range`.
- `list_logs` with non-UTC timestamp offset (`+01:00`) returns JSON-RPC error `-32602` with stable error code `invalid_utc_time`.
- `list_logs` with a window larger than 7 days returns JSON-RPC error `-32602` with stable error code `time_range_too_large` unless `allow_large_window=true`.
- Each log entry includes `timestamp_utc`, `unit`, `priority`, `hostname`, `pid`, `message`, and `cursor`.
- `list_logs` structured output includes metadata: `total_scanned`, `returned`, `truncated`, `generated_at_utc`, and `window` object.
- `list_logs` with exactly `limit` matching rows and no additional matching row returns `truncated=false`.
- `list_logs` with more matching rows than `limit` returns only `limit` rows and `truncated=true`.
- `list_logs` with `summary=true` returns compact `summary` block with `counts_by_unit`, `counts_by_priority`, `top_messages`, and `error_hotspots`, plus metadata: `total_scanned`, `returned`, `truncated`, `generated_at_utc`, and `window`.

### Tool: `list_timers`

- `tools/call` for `list_timers` returns only `*.timer` units.
- Each output item includes: `unit`, `active_state`, `sub_state`, `next_run_utc`, `last_run_utc`, `time_until_next_sec`, `time_since_last_sec`, `trigger_unit`, `persistent`, `result`, `load_state`, `unit_file_state`, `overdue`, `overdue_reason`.
- `list_timers` defaults to `limit=200` and enforces max `limit=1000`.
- `list_timers` with `limit=0` or `limit=1001` returns JSON-RPC error `-32602` with stable error code `invalid_limit`.
- `list_timers` with `name_contains=backup` applies case-insensitive matching on timer unit names.
- `list_timers` with `state=ACTIVE` applies case-insensitive matching on timer state.
- `list_timers` defaults to `scope=system` when `scope` is omitted.
- `list_timers` with `scope=user` returns user-manager timers.
- `list_timers` with `scope=both` returns combined system and user-manager timers.
- `list_timers` with `scope=both` preserves both rows when system and user managers contain the same unit name and identifies each row's source scope.
- `list_timers` with unsupported `scope` value returns JSON-RPC error `-32602` with stable error code `invalid_scope`.
- `list_timers` with invalid parameter type (for example `summary="yes"`) returns JSON-RPC error `-32602` with stable error code `invalid_params` (or equivalent stable code).
- `list_timers` with `sort=next` sorts by nearest next run; `sort=last` sorts by most recent last run; `sort=name` sorts by timer unit; `sort=state` sorts by active state.
- `list_timers` with unsupported `sort` returns JSON-RPC error `-32602` with stable error code `invalid_sort`.
- `list_timers` with unsupported `order` returns JSON-RPC error `-32602` with stable error code `invalid_order`.
- `list_timers` with `include_persistent=true` includes `persistent` values where available and preserves `null` where unavailable.
- `list_timers` with `overdue_only=true` returns only rows where `overdue=true`.
- Overdue semantics: timer is overdue only if `next_run_utc` is known, current time exceeds `next_run_utc` by more than 5 minutes, and `active_state=active`.
- Timers without `next_run_utc` are not marked overdue by default and include explanatory `overdue_reason` where applicable.
- Partial metadata failures (for example trigger or persistence not available) do not fail the call; affected fields are `null`.
- `list_timers` structured output includes per-row `scope` and metadata: `total_scanned`, `returned`, `truncated`, `generated_at_utc`.
- `list_timers` with `summary=true` returns compact `summary` block with `counts_by_active_state`, `overdue_count`, `next_due_soon` (top 5), and `failed_or_problem_timers`, plus metadata: `total_scanned`, `returned`, `truncated`, `generated_at_utc`.

## MCP Resources

- `resources/list` returns at least three resources: service snapshot, failed service snapshot, and logs snapshot.
- `resources/list` includes fixed URIs `resource://services/snapshot`, `resource://services/failed`, and `resource://logs/recent`.
- `resources/list` includes stable resource identifiers and human-readable names.
- `resources/read` for service snapshot returns schema-stable data matching service output model.
- `resources/read` for failed service snapshot returns only entries where `active_state` is `failed`.
- `resources/read` for logs snapshot returns schema-stable data matching log output model.
- Successful `resources/read` responses use MCP `contents` shape and do not include non-schema top-level fields.
- `resources/read` for unknown resource returns JSON-RPC error `-32601` (or project-defined equivalent) with stable error data.

## Error and Observability Behavior

- Internal tool execution failures return opaque client-facing errors (no raw system diagnostics in JSON-RPC error message/data).
- HTTP non-2xx errors retain `{ code, message, details }` JSON shape.
- Startup and request logs include method/path/status/duration and authentication failures, without exposing token values.
- Every action executed through the MCP server emits an INFO-level audit log event.
- Audit log events include action parameters with sensitive fields redacted (for example token, password, secret, credentials).

## Structured Runtime Inspection and Pagination

- `get_unit_status` covers complete/partial properties, both concrete scopes, invalid/non-service/missing units, direct failed/missing dependencies, newest-first bounded transitions, and no recursion.
- Podman inspection covers running/stopped/unhealthy/rootless/read-only/mounted and pod-member fixtures, unavailable CLI/runtime, timeout, nonzero/not-found, malformed/oversized JSON, hostile identifiers, and exclusion of verbose metadata.
- Log pagination covers ascending/descending exclusive continuation without gaps or duplicates, exhausted/invalid cursors, filter continuity, all projections, invalid/duplicate fields, grouping counts/order and raw-page continuation.
- Log bounds cover literal versus slash-delimited regex grep, unit-start derivation/unavailability, exact seven-day acceptance, and `maximum_start_utc` error details.
- `tools/list` advertises strict schemas for `get_unit_status`, `get_container_status`, and `get_pod_status`; successful calls use `structuredContent` and failures preserve stable JSON-RPC shapes.

## Podman Inspection Data Minimization

- Container output omits `create_command` even when Podman inspect returns `CreateCommand` containing secrets.
- Mount output omits host `Source` while retaining destination, type, and read-only state.
- Command and health-test argv redact case-insensitive credential values supplied as separate arguments, `--flag=value`, and `NAME=value` assignments.
- Non-sensitive argv ordering and values remain intact.
- Health configuration exposes only sanitized test argv and timing/retry fields and never health logs or unrelated raw metadata.

## Review Follow-up: Unit Inspection

- `since_last_start=true` with an unsupported scope returns stable `invalid_scope`, not `invalid_unit`.
- Production unit inspection reads direct `Requires` and `Wants`, reports failed/missing/unloaded dependencies, preserves relationship type, and never traverses recursively.
- Transition lookup recognizes canonical systemd starting, started, stopping, stopped, failed, reloading, and reloaded message IDs; output is newest-first and capped by `transition_limit`.
- Journal transition scanning is bounded even when no matching unit transition exists.
