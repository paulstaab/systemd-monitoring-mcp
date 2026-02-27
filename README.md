# systemd-monitoring-mcp

MCP server for monitoring a Linux server over HTTP.

## Features (MVP)

- `GET /health` public health endpoint.
- `GET /services` protected endpoint returning systemd `*.service` services.
- Bearer-token authentication using `MCP_API_TOKEN`.

## Configuration

- `MCP_API_TOKEN` (required): static API token.
- `BIND_ADDR` (optional, default: `0.0.0.0`)
- `BIND_PORT` (optional, default: `8080`)
- `MCP_ALLOWED_CIDR` (optional): if set, only requests originating from this CIDR range are accepted.

## Run

```bash
export MCP_API_TOKEN="change-me"
# optional:
# export BIND_ADDR="0.0.0.0"
# export BIND_PORT="8080"
# export MCP_ALLOWED_CIDR="10.0.0.0/8"

cargo run
```

## API examples

### Health

```bash
curl -s http://127.0.0.1:8080/health
```

### List services (authorized)

```bash
curl -s \
	-H "Authorization: Bearer change-me" \
	http://127.0.0.1:8080/services
```

### List services (unauthorized)

```bash
curl -i -s http://127.0.0.1:8080/services
```
