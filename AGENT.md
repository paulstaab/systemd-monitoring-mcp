# Agent Instructions for systemd-monitoring-mcp

`systemd-monitoring-mcp` is an MCP server that exposes selective functionality for monitoring
a Linux server over JSON-RPC.

## Project Overview
- Runtime: single Rust binary.
- API scope (MVP):
  - `GET /health` (public)
  - `POST /mcp` (requires bearer token)
- Auth: static token from environment variable `MCP_API_TOKEN`.
- Unit source: systemd via `systemd` crate + D-Bus integration.

## Repository Pointers
- Requirements: `docs/requirements.md`
- Implementation plan: `docs/implementation-plan.md`
- Entry point: `src/main.rs`
- App wiring and route composition: `src/lib.rs`

## Code Structure
The application is organized into modules with a strict separation of concerns:
- **`src/domain/`**: The core business logic and MCP feature implementations.
  - `tools.rs`: Logic for executable tools (e.g., `list_services`, `list_logs`) and their parameters.
  - `resources.rs`: Logic for serving static read-only resources (e.g., `resource://services/snapshot`).
  - `utils.rs`: Shared parsing, formatting, and validation helpers.
- **`src/mcp/`**: Context protocol decoding and JSON-RPC implementations.
  - `server.rs`: The MCP engine tracking capabilities validation, batch requests, notifications, and inner tool routing.
  - `rpc.rs`: Low-level mapping of JSON-RPC semantics like standard protocol errors and request shaping.
- **`src/http/`**: The network edge and HTTP frameworks representations.
  - `handlers.rs`: Direct Axum handler implementations (`/health`, `/.well-known/mcp`, and the main `/mcp` listener).
- **`src/systemd_client.rs`**: System adapters connecting the application to DBus to fetch from Systemd and Journald. Included with traits to allow mocking in test scenarios.
- **Cross-Cutting Modules** (`src/*.rs`):
  - `config.rs`: Extracting application configuration from environment properties securely.
  - `auth.rs`: Access control middleware validating Bearer tokens
  - `errors.rs`: Universal app errors defining business bounds instead of internal library faults.
  - `logging.rs`: Custom structured logging and JSON-RPC parameter redaction definitions.

## Workflow
When asked to implement new features or changes, execute the following steps
  1. If neccessary, ask the user clarifying questions.
  2. Update the requirements in `docs/requirements.md`
  3. Generate a plan for implementing the changes
  4. Generate test cases for the requirements in a separate artifact (not in `docs/requirements.md`).
  5. Extend or update the smoke test script `scripts/smoke-test.sh`
  6. Implement the changes.
  7. Run tests and linters and investigate and fix any problems.

## Requirements
- Collect requirements in `docs/requirements.md`. 
- The MCP should only offer capabilities described in the requirements.
- Do not include acceptance criteria or test cases in the requirements document.

## Build, Lint, Test
- Fast compile check:
  - `cargo check`
- Formatting:
  - `cargo fmt`
  - `cargo fmt --check`
- Linting (warnings are errors):
  - `cargo clippy --all-targets -- -D warnings`
- Tests:
  - `cargo test`

Recommended verification sequence before handoff:
1. `cargo fmt`
2. `cargo fmt --check`
3. `cargo clippy --all-targets -- -D warnings`
4. `cargo test`

Copy-paste one-liner:
- `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`

## Run and Local Smoke Checks
- Minimal run:
  - `export MCP_API_TOKEN="change-me"`
  - `cargo run`
- Optional bind settings:
  - `export BIND_ADDR="127.0.0.1"`
  - `export BIND_PORT="8080"`
- Smoke checks:
  - `curl -s http://127.0.0.1:8080/health`
  - `curl -s -H "Authorization: Bearer change-me" -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","clientInfo":{"name":"example-client","version":"1.0.0"},"capabilities":{}}}' http://127.0.0.1:8080/mcp`
  - `curl -s -H "Authorization: Bearer change-me" -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' http://127.0.0.1:8080/mcp`

## Implementation Notes
- Keep API behavior strictly aligned with `docs/requirements.md`.
- Preserve structured HTTP error responses (`code`, `message`, `details`) for non-2xx responses.
- Preserve JSON-RPC error shape for MCP method failures.
- Never log secrets (`MCP_API_TOKEN` or bearer token values).
- Keep non-systemd-dependent logic testable via abstractions/mocks.

## MCP Protocol Notes
- Prefer implementing monitoring functionality through MCP methods (`tools/list`, `tools/call`, `resources/list`, `resources/read`) instead of REST business endpoints.
- Keep MCP transport strict to `POST /mcp` (no root `/` alias behavior).
- Use fixed resource URIs: `resource://services/snapshot` and `resource://logs/recent`.
- Return canonical machine-readable JSON in `structuredContent` for successful `tools/call` and `resources/read` responses.
- Keep capability advertising in `initialize` synchronized with what is actually implemented.
- Maintain strict JSON-RPC behavior for notifications, batch requests, and standard error codes (`-32700`, `-32600`, `-32601`, `-32602`).

## Technology
- `systemd-monitoring-mcp` is a single binary written in Rust
- It uses the [`systemd`](https://github.com/codyps/rust-systemd) crate to interact with systemd and journald
