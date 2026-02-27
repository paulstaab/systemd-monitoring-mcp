# systemd-monitoring-mcp

MCP server for monitoring a Linux server over JSON-RPC.

## Features (MVP)

- `GET /health` public health endpoint.
- `GET /.well-known/mcp` public MCP discovery endpoint.
- `POST /mcp` MCP JSON-RPC endpoint (bearer-token protected).
- MCP tools: `list_services`, `list_logs`.
- MCP resources: `resource://services/snapshot`, `resource://logs/recent`.
- Bearer-token authentication using `MCP_API_TOKEN` (constant-time HMAC comparison).
- Optional CIDR-based IP allowlist with trusted-proxy support (`X-Forwarded-For`).

## Configuration

| Variable | Required | Default | Description |
|---|---|---|---|
| `MCP_API_TOKEN` | **yes** | — | Static API token (minimum 16 characters). |
| `BIND_ADDR` | no | `127.0.0.1` | Bind address. |
| `BIND_PORT` | no | `8080` | Bind port. |
| `MCP_ALLOWED_CIDR` | no | — | If set, only requests originating from this CIDR range are accepted. |
| `MCP_TRUSTED_PROXIES` | no | — | Comma-separated CIDR list of trusted reverse-proxy addresses. When the direct peer matches, the client IP is read from `X-Forwarded-For`. |

## Run

```bash
export MCP_API_TOKEN="a-secure-token-at-least-16-chars"
# optional:
# export BIND_ADDR="127.0.0.1"
# export BIND_PORT="8080"
# export MCP_ALLOWED_CIDR="10.0.0.0/8"
# export MCP_TRUSTED_PROXIES="172.16.0.1/32,10.0.0.1/32"

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
	-d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","clientInfo":{"name":"example-client","version":"1.0.0"},"capabilities":{}}}' \
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
	-d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_services","arguments":{}}}' \
	http://127.0.0.1:8080/mcp
```

### MCP tools/call list_logs

```bash
curl -s \
	-H "Content-Type: application/json" \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	-d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_logs","arguments":{"priority":"err","unit":"sshd_service","start_utc":"2026-02-27T00:00:00Z","end_utc":"2026-02-27T01:00:00Z","limit":100}}}' \
	http://127.0.0.1:8080/mcp
```

## Verification

Use this sequence before handoff or release:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
