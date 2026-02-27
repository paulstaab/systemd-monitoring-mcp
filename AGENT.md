# Agent Instructions for systemd-monitoring-mcp

`systemd-monitoring-mcp` is an MCP server that exposes selective functionality for monitoring
a Linux server over HTTP.

## Project Overview
- Runtime: single Rust binary.
- API scope (MVP):
  - `GET /health` (public)
  - `GET /services` (requires bearer token)
- Auth: static token from environment variable `MCP_API_TOKEN`.
- Unit source: systemd via `libsystemd` + D-Bus integration.

## Repository Pointers
- Requirements: `docs/requirements.md`
- Implementation plan: `docs/implementation-plan.md`
- Entry point: `src/main.rs`
- App wiring and route composition: `src/lib.rs`
- Key modules:
  - `src/config.rs`
  - `src/auth.rs`
  - `src/api.rs`
  - `src/systemd_client.rs`
  - `src/errors.rs`
  - `src/logging.rs`

## Requirements
- Collect requirements in `docs/requirements.md`. 
- When asked to implement new features or changes, execute the following steps
  1. If neccessary, ask the user clarifying questions.
  2. Update the requirements in `docs/requirements.md`
  3. Generate a plan for implementing the changes
  4. Generate test cases for the requirements in a separate artifact (not in `docs/requirements.md`).
  5. Extend or update the smoke test script `scripts/smoke-test.sh`
  5. Implement the changes.
  6. Run tests and linters and investigate and fix any problems.
- The MCP should only offer capabilities described in the requirements.
- Do not include acceptance criteria or test cases in the requirements document.
- Do not run the smoke-test script. Ask to user to do it instead.

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

## Run and Local Smoke Checks
- Minimal run:
  - `export MCP_API_TOKEN="change-me"`
  - `cargo run`
- Optional bind settings:
  - `export BIND_ADDR="0.0.0.0"`
  - `export BIND_PORT="8080"`
- Smoke checks:
  - `curl -s http://127.0.0.1:8080/health`
  - `curl -i -s http://127.0.0.1:8080/services`
  - `curl -s -H "Authorization: Bearer change-me" http://127.0.0.1:8080/services`

## Implementation Notes
- Keep API behavior strictly aligned with `docs/requirements.md`.
- Preserve structured error responses (`code`, `message`, `details`) for non-2xx responses.
- Never log secrets (`MCP_API_TOKEN` or bearer token values).
- Keep non-systemd-dependent logic testable via abstractions/mocks.

## Technology
- `systemd-monitoring-mcp` is a single binary written in Rust
- It uses [`libsystemd`](https://github.com/lucab/libsystemd-rs) to interact with systemd
