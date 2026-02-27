# systemd-monitoring-mcp

MCP server for monitoring a Linux server over HTTP.

## Features (MVP)

- `GET /health` public health endpoint.
- `GET /` and `GET /.well-known/mcp` public MCP discovery endpoints.
- `POST /` and `POST /mcp` MCP JSON-RPC endpoints (bearer-token protected).
- `GET /services` protected endpoint returning systemd `*.service` services.
- `GET /logs` protected endpoint returning journald logs with filter/limit options.
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

### MCP initialize (Copilot-compatible root path)

```bash
curl -s \
	-H "Content-Type: application/json" \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	-d '{"jsonrpc":"2.0","id":1,"method":"initialize"}' \
	http://127.0.0.1:8080/
```

### List services (authorized)

```bash
curl -s \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	http://127.0.0.1:8080/services
```

### List logs (authorized)

```bash
curl -s \
	-H "Authorization: Bearer $MCP_API_TOKEN" \
	"http://127.0.0.1:8080/logs?priority=err&unit=sshd_service&start_utc=2026-02-27T00:00:00Z&end_utc=2026-02-27T01:00:00Z&limit=100"
```

Supported `/logs` query parameters:
- `priority`: minimum severity threshold (`0` to `7` or aliases like `err`); returns that priority and higher-severity entries
- `unit`: unit identifier
- `start_utc`, `end_utc` (required): RFC3339 UTC (`Z`) timestamps
- `limit`: integer `1..1000`
