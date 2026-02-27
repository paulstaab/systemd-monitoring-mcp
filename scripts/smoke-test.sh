#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

HOST="${SMOKE_HOST:-127.0.0.1}"
PORT="${SMOKE_PORT:-8080}"
TOKEN="${SMOKE_TOKEN:-change-me}"
BINARY_PATH="${SMOKE_BINARY:-${ROOT_DIR}/target/release/systemd-monitoring-mcp}"
BASE_URL="http://${HOST}:${PORT}"

SERVER_LOG="$(mktemp -t systemd-monitoring-mcp-smoke.XXXXXX.log)"
SERVER_PID=""

cleanup() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

fail() {
  echo "[smoke] FAIL: $1" >&2
  echo "[smoke] server log: $SERVER_LOG" >&2
  if [[ -f "$SERVER_LOG" ]]; then
    echo "[smoke] --- last server log lines ---" >&2
    tail -n 40 "$SERVER_LOG" >&2 || true
  fi
  exit 1
}

assert_contains() {
  local haystack="$1"
  local needle="$2"
  local message="$3"
  if [[ "$haystack" != *"$needle"* ]]; then
    fail "$message"
  fi
}

wait_for_health() {
  local attempts=0
  local max_attempts=60

  while (( attempts < max_attempts )); do
    if [[ -n "$SERVER_PID" ]] && ! kill -0 "$SERVER_PID" 2>/dev/null; then
      fail "server process exited before becoming healthy"
    fi

    if curl -sS "${BASE_URL}/health" >/dev/null 2>&1; then
      return 0
    fi
    attempts=$((attempts + 1))
    sleep 0.25
  done

  return 1
}

check_binary_available() {
  echo "[smoke] checking binary availability"

  if [[ ! -e "$BINARY_PATH" ]]; then
    fail "binary not found at ${BINARY_PATH}; build it first (for example: cargo build --release)"
  fi

  if [[ ! -x "$BINARY_PATH" ]]; then
    fail "binary exists but is not executable: ${BINARY_PATH}"
  fi
}

check_systemd_available() {
  echo "[smoke] checking systemd availability"

  local system_bus_socket="/run/dbus/system_bus_socket"
  if [[ ! -S "$system_bus_socket" ]]; then
    fail "system D-Bus socket is not available at ${system_bus_socket}"
  fi

  if command -v busctl >/dev/null 2>&1; then
    if busctl --system call \
      org.freedesktop.systemd1 \
      /org/freedesktop/systemd1 \
      org.freedesktop.DBus.Peer \
      Ping >/dev/null 2>&1; then
      return 0
    fi
    fail "systemd is not reachable on the system D-Bus (busctl ping failed)"
  fi

  if command -v dbus-send >/dev/null 2>&1; then
    if dbus-send --system \
      --dest=org.freedesktop.systemd1 \
      --type=method_call \
      --print-reply \
      /org/freedesktop/systemd1 \
      org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1; then
      return 0
    fi
    fail "systemd is not reachable on the system D-Bus (dbus-send ping failed)"
  fi

  echo "[smoke] busctl/dbus-send not found; proceeding after socket-level availability check"
}

check_binary_available
check_systemd_available

echo "[smoke] starting server binary ${BINARY_PATH} on ${HOST}:${PORT}"
MCP_API_TOKEN="$TOKEN" BIND_ADDR="$HOST" BIND_PORT="$PORT" "$BINARY_PATH" >"$SERVER_LOG" 2>&1 &
SERVER_PID="$!"

wait_for_health || fail "server did not become healthy in time"

echo "[smoke] checking GET /health"
health_body="$(curl -sS "${BASE_URL}/health")"
health_status="$(curl -sS -o /dev/null -w "%{http_code}" "${BASE_URL}/health")"
[[ "$health_status" == "200" ]] || fail "/health returned status ${health_status}, expected 200"
assert_contains "$health_body" '"status":"ok"' "/health body did not contain expected status"

echo "[smoke] checking GET /.well-known/mcp"
discovery_body="$(curl -sS "${BASE_URL}/.well-known/mcp")"
discovery_status="$(curl -sS -o /dev/null -w "%{http_code}" "${BASE_URL}/.well-known/mcp")"
[[ "$discovery_status" == "200" ]] || fail "/.well-known/mcp returned status ${discovery_status}, expected 200"
assert_contains "$discovery_body" '"mcp_endpoint":"/mcp"' "discovery did not advertise mcp endpoint"
assert_contains "$discovery_body" '"services_endpoint":"/services"' "discovery did not advertise services endpoint"
assert_contains "$discovery_body" '"logs_endpoint":"/logs"' "discovery did not advertise logs endpoint"

echo "[smoke] checking POST /mcp initialize"
mcp_initialize_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize"}' \
  "${BASE_URL}/mcp")"
mcp_initialize_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize"}' \
  "${BASE_URL}/mcp")"
[[ "$mcp_initialize_status" == "200" ]] || fail "/mcp initialize returned status ${mcp_initialize_status}, expected 200"
assert_contains "$mcp_initialize_body" '"jsonrpc":"2.0"' "initialize did not return jsonrpc envelope"
assert_contains "$mcp_initialize_body" '"protocolVersion":"2024-11-05"' "initialize did not return protocolVersion"
assert_contains "$mcp_initialize_body" '"services":"/services"' "initialize did not advertise services endpoint in metadata"
assert_contains "$mcp_initialize_body" '"logs":"/logs"' "initialize did not advertise logs endpoint in metadata"

echo "[smoke] checking POST / initialize"
root_initialize_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":11,"method":"initialize"}' \
  "${BASE_URL}/")"
root_initialize_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":11,"method":"initialize"}' \
  "${BASE_URL}/")"
[[ "$root_initialize_status" == "200" ]] || fail "/ initialize returned status ${root_initialize_status}, expected 200"
assert_contains "$root_initialize_body" '"jsonrpc":"2.0"' "root initialize did not return jsonrpc envelope"
assert_contains "$root_initialize_body" '"protocolVersion":"2024-11-05"' "root initialize did not return protocolVersion"
assert_contains "$root_initialize_body" '"services":"/services"' "root initialize did not advertise services endpoint in metadata"
assert_contains "$root_initialize_body" '"logs":"/logs"' "root initialize did not advertise logs endpoint in metadata"

echo "[smoke] checking POST /mcp ping"
mcp_ping_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"ping"}' \
  "${BASE_URL}/mcp")"
mcp_ping_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"ping"}' \
  "${BASE_URL}/mcp")"
[[ "$mcp_ping_status" == "200" ]] || fail "/mcp ping returned status ${mcp_ping_status}, expected 200"
assert_contains "$mcp_ping_body" '"jsonrpc":"2.0"' "ping did not return jsonrpc envelope"
assert_contains "$mcp_ping_body" '"result":{}' "ping did not return empty result object"

echo "[smoke] checking GET /services without token"
services_unauth_body="$(curl -sS "${BASE_URL}/services")"
services_unauth_status="$(curl -sS -o /dev/null -w "%{http_code}" "${BASE_URL}/services")"
[[ "$services_unauth_status" == "401" ]] || fail "/services without token returned ${services_unauth_status}, expected 401"
assert_contains "$services_unauth_body" '"code":"missing_token"' "/services without token body did not contain missing_token"

echo "[smoke] checking GET /services with token"
services_auth_body="$(curl -sS -H "Authorization: Bearer ${TOKEN}" "${BASE_URL}/services")"
services_auth_status="$(curl -sS -o /dev/null -w "%{http_code}" -H "Authorization: Bearer ${TOKEN}" "${BASE_URL}/services")"

[[ "$services_auth_status" == "200" ]] || fail "/services with token returned ${services_auth_status}, expected 200"
assert_contains "$services_auth_body" '[' "/services with token returned 200 but not a JSON array"

echo "[smoke] checking GET /logs without token"
logs_unauth_body="$(curl -sS "${BASE_URL}/logs")"
logs_unauth_status="$(curl -sS -o /dev/null -w "%{http_code}" "${BASE_URL}/logs")"
[[ "$logs_unauth_status" == "401" ]] || fail "/logs without token returned ${logs_unauth_status}, expected 401"
assert_contains "$logs_unauth_body" '"code":"missing_token"' "/logs without token body did not contain missing_token"

echo "[smoke] checking GET /logs with token"
logs_auth_body="$(curl -sS -H "Authorization: Bearer ${TOKEN}" "${BASE_URL}/logs?start_utc=1970-01-01T00:00:00Z&end_utc=2100-01-01T00:00:00Z&limit=10")"
logs_auth_status="$(curl -sS -o /dev/null -w "%{http_code}" -H "Authorization: Bearer ${TOKEN}" "${BASE_URL}/logs?start_utc=1970-01-01T00:00:00Z&end_utc=2100-01-01T00:00:00Z&limit=10")"
[[ "$logs_auth_status" == "200" ]] || fail "/logs with token returned ${logs_auth_status}, expected 200"
assert_contains "$logs_auth_body" '[' "/logs with token returned 200 but not a JSON array"

echo "[smoke] checking GET /logs invalid limit"
logs_invalid_limit_body="$(curl -sS -H "Authorization: Bearer ${TOKEN}" "${BASE_URL}/logs?start_utc=1970-01-01T00:00:00Z&end_utc=2100-01-01T00:00:00Z&limit=1001")"
logs_invalid_limit_status="$(curl -sS -o /dev/null -w "%{http_code}" -H "Authorization: Bearer ${TOKEN}" "${BASE_URL}/logs?start_utc=1970-01-01T00:00:00Z&end_utc=2100-01-01T00:00:00Z&limit=1001")"
[[ "$logs_invalid_limit_status" == "400" ]] || fail "/logs with invalid limit returned ${logs_invalid_limit_status}, expected 400"
assert_contains "$logs_invalid_limit_body" '"code":"invalid_limit"' "/logs invalid limit body did not contain invalid_limit"

echo "[smoke] PASS"
