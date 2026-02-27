# Implementation Plan: systemd-monitoring-mcp (MVP)

## 1) Objective
Build a single Rust binary that exposes a REST API to:
- `GET /health` (public)
- `GET /units` (bearer-token protected)

`GET /units` must return only `*.service` units with fields:
- `name`
- `state` (raw systemd `ActiveState`)
- `description` (`string | null`)

## 2) Proposed Technical Approach
- Runtime and HTTP: `tokio` + `axum`
- Serialization: `serde`, `serde_json`
- Logging: `tracing`, `tracing-subscriber`
- Systemd integration: `libsystemd` crate (as required by project)
- Error handling: central app error type mapped to structured JSON errors

## 3) Phase Plan

### Phase 0 — Project scaffold
Deliverables:
- Initialize Rust binary project structure (`Cargo.toml`, `src/main.rs`, modules)
- Add dependencies and base module layout:
  - `config`
  - `api` (routes/handlers)
  - `auth`
  - `systemd_client`
  - `errors`
  - `logging`

Done when:
- Project compiles with `cargo check`
- Server starts and binds with defaults

### Phase 1 — Configuration and startup safety
Deliverables:
- Parse env vars:
  - required `MCP_API_TOKEN`
  - optional `BIND_ADDR` (default `0.0.0.0`)
  - optional `BIND_PORT` (default `8080`)
- Fail fast with clear error if token missing/empty
- Build bind socket from address + port

Done when:
- Startup fails without token
- Startup succeeds with token and defaults

### Phase 2 — Core HTTP API and auth middleware
Deliverables:
- Implement routes:
  - `GET /health` (no auth)
  - `GET /units` (auth required)
- Implement bearer token middleware for protected routes
- Return `401` on missing/invalid auth

Done when:
- `/health` returns `200` + `{ "status": "ok" }`
- `/units` rejects missing/invalid token with structured error JSON

### Phase 3 — Systemd unit listing integration
Deliverables:
- Implement systemd query using `libsystemd`
- Filter to `.service` units only
- Map output to response model: `name`, `state`, `description`
- Sort response alphabetically by `name`

Done when:
- `/units` with valid token returns `200` and sorted list
- `description` is `null` when unavailable

### Phase 4 — Error model and observability
Deliverables:
- Standard structured error body for non-2xx:
  - `code`
  - `message`
  - `details`
- Central error-to-status mapping (`401`, `500`)
- Add required logs:
  - startup bind settings
  - auth failures
  - request summaries (method/path/status/duration)
- Ensure no secret/token values are logged

Done when:
- All error responses use required schema
- Logs contain required events and no secrets

### Phase 5 — Verification and quality gates
Deliverables:
- Unit tests for:
  - config validation
  - auth extraction/validation
  - error serialization/mapping
- Integration-style handler tests for:
  - `/health`
  - `/units` auth paths
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`

Done when:
- All checks pass locally

## 4) Execution Order (recommended)
1. Scaffold project and modules
2. Config loading + startup checks
3. Health route
4. Auth middleware
5. Error type and JSON mapper
6. Systemd client integration
7. Units handler + sorting/filtering
8. Logging middleware and security checks
9. Tests, fmt, clippy, final verification

## 5) Risks and Mitigations
- `libsystemd` API shape may require adapter code.
  - Mitigation: isolate in `systemd_client` module with internal DTO mapping.
- Running tests in environments without systemd access.
  - Mitigation: abstract systemd access behind a trait and mock in tests.
- Token leakage in logs.
  - Mitigation: never log authorization header, log only failure reason category.

## 6) Definition of Done
- Implementation matches [requirements](requirements.md)
- No capabilities beyond defined scope are exposed
- Build, lint, and tests pass
- README includes minimal run instructions with env vars and endpoint examples
