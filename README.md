# systemd-monitoring-mcp

MCP server for monitoring a Linux server over JSON-RPC.

## Features (MVP)

- `GET /health` public health endpoint.
- `GET /.well-known/mcp` public MCP discovery endpoint.
- `POST /mcp` MCP JSON-RPC endpoint (bearer-token protected).
- `initialize` accepts modern protocol versions (including `2025-03-26`) and negotiates gracefully.
- MCP tools: `list_services`, `list_timers`, `list_logs`.
- MCP resources: `resource://services/snapshot`, `resource://services/failed`, `resource://logs/recent`.
- Bearer-token authentication using `MCP_API_TOKEN`.

### MCP tool capabilities

- `list_services`: lists `*.service` units with optional `scope`, `state`, `name_contains`, `limit`, and `summary`.
- `list_timers`: lists `*.timer` units with optional `scope`, `name_contains`, `state`, `limit`, `sort`, `order`, `overdue_only`, `include_persistent`, and `summary`.
- `list_logs`: lists journald logs with required `start_utc`/`end_utc` and optional `scope`, `priority`, `unit`, `exclude_units`, `grep`, `order`, `limit`, `allow_large_window`, and `summary`.

`scope` supports `system|user|both` and defaults to `system` for all three list tools.

## Configuration

**Note:** It is strongly recommended to run this service behind a reverse proxy (e.g., Nginx, HAProxy, Envoy)
that takes care of TLS termination and restricts network access.

**DO NOT EXPOSE THIS TO THE INTERNET!**

| Variable | Required | Default | Description |
|---|---|---|---|
| `MCP_API_TOKEN` | **yes** | — | Static API token (minimum 16 characters). |
| `BIND_ADDR` | no | `127.0.0.1` | Bind address. |
| `BIND_PORT` | no | `8080` | Bind port. |

## Run

```bash
export MCP_API_TOKEN="a-secure-token-at-least-16-chars"
# optional:
# export BIND_ADDR="127.0.0.1"
# export BIND_PORT="8080"

cargo run
```

## API examples

### Health

```bash
curl -s http://127.0.0.1:8080/health
```

### MCP initialize

```bash
curl -s \
	-H "Content-Type: application/json" \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	-d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","clientInfo":{"name":"example-client","version":"1.0.0"},"capabilities":{}}}' \
	http://127.0.0.1:8080/mcp
```

### MCP tools/list

```bash
curl -s \
	-H "Content-Type: application/json" \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	-d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
	http://127.0.0.1:8080/mcp
```

### MCP tools/call list_services

```bash
curl -s \
	-H "Content-Type: application/json" \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	-d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_services","arguments":{"state":"failed","name_contains":"sshd","limit":200}}}' \
	http://127.0.0.1:8080/mcp
```

### MCP tools/call list_timers

```bash
curl -s \
	-H "Content-Type: application/json" \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	-d '{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"list_timers","arguments":{"scope":"both","sort":"next","order":"asc","limit":200}}}' \
	http://127.0.0.1:8080/mcp
```

### MCP tools/call list_logs

```bash
curl -s \
	-H "Content-Type: application/json" \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	-d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_logs","arguments":{"scope":"both","priority":"err","unit":"sshd_service","exclude_units":["cron.service"],"grep":"/timeout|failed/","order":"desc","start_utc":"2026-02-27T00:00:00Z","end_utc":"2026-02-27T01:00:00Z","limit":200}}}' \
	http://127.0.0.1:8080/mcp
```

### MCP tools/call summary mode

```bash
curl -s \
	-H "Content-Type: application/json" \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	-d '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"list_services","arguments":{"summary":true}}}' \
	http://127.0.0.1:8080/mcp

curl -s \
	-H "Content-Type: application/json" \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	-d '{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"list_timers","arguments":{"scope":"both","summary":true}}}' \
	http://127.0.0.1:8080/mcp

curl -s \
	-H "Content-Type: application/json" \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	-d '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"list_logs","arguments":{"scope":"both","start_utc":"2026-02-27T00:00:00Z","end_utc":"2026-02-27T01:00:00Z","summary":true}}}' \
	http://127.0.0.1:8080/mcp
```

### MCP tools/call invalid scope (validation example)

```bash
curl -s \
	-H "Content-Type: application/json" \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	-d '{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"list_services","arguments":{"scope":"global"}}}' \
	http://127.0.0.1:8080/mcp
```

Expected: JSON-RPC error `-32602` with stable data code `invalid_scope`.

### MCP resources/read failed services snapshot

```bash
curl -s \
	-H "Content-Type: application/json" \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	-d '{"jsonrpc":"2.0","id":5,"method":"resources/read","params":{"uri":"resource://services/failed"}}' \
	http://127.0.0.1:8080/mcp
```

## Verification

Use this sequence before handoff or release:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Notes

- `list_logs` requires UTC RFC3339 timestamps with `Z` suffix for `start_utc` and `end_utc`.
- Time windows over 7 days require `allow_large_window=true`.
- Timer and service tooling are read-only and do not mutate systemd state.
