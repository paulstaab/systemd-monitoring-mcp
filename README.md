# systemd-monitoring-mcp

MCP server for monitoring a Linux server over HTTP.

## Features (MVP)

- `GET /health` public health endpoint.
- `POST /` and `POST /mcp` MCP JSON-RPC endpoints (both supported for client compatibility).
- `GET /services` protected endpoint returning systemd `*.service` services.
- `GET /logs` protected endpoint returning journald logs with filter/sort options.
- Bearer-token authentication using `MCP_API_TOKEN`.

## Configuration

- `MCP_API_TOKEN` (required): static API token.
- `BIND_ADDR` (optional, default: `127.0.0.1`)
- `BIND_PORT` (optional, default: `8080`)
- `MCP_ALLOWED_CIDR` (optional): if set, only requests originating from this CIDR range are accepted.

## Run

```bash
export MCP_API_TOKEN="change-me"
# optional:
# export BIND_ADDR="127.0.0.1"
# export BIND_PORT="8080"
# export MCP_ALLOWED_CIDR="10.0.0.0/8"

cargo run
```

## API examples

### Health

```bash
curl -s http://127.0.0.1:8080/health
```

### MCP initialize (Copilot-compatible root path)

```bash
curl -s \
	-H "Content-Type: application/json" \
	-d '{"jsonrpc":"2.0","id":1,"method":"initialize"}' \
	http://127.0.0.1:8080/
```

### List services (authorized)

```bash
curl -s \
	-H "Authorization: Bearer change-me" \
	http://127.0.0.1:8080/services
```

### List logs (authorized)

```bash
curl -s \
	-H "Authorization: Bearer change-me" \
	"http://127.0.0.1:8080/logs?priority=err&unit=sshd_service&start_utc=2026-02-27T00:00:00Z&end_utc=2026-02-27T01:00:00Z&limit=100&order=desc"
```

Supported `/logs` query parameters:
- `priority`: minimum severity threshold (`0` to `7` or aliases like `err`); returns that priority and higher-severity entries
- `unit`: unit identifier containing only ASCII letters/digits, dashes (`-`), underscores (`_`), at-sign (`@`), and colon (`:`)
- `start_utc`, `end_utc` (required): RFC3339 UTC (`Z`) timestamps
- `limit`: integer `1..1000`
- `order`: `asc` (default) or `desc`

### List services (unauthorized)

```bash
curl -i -s http://127.0.0.1:8080/services
```
