#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

HOST="${SMOKE_HOST:-127.0.0.1}"
PORT="${SMOKE_PORT:-8080}"
TOKEN="${SMOKE_TOKEN:-change-me-token-16}"
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

echo "[smoke] checking POST /mcp initialize"
mcp_initialize_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","clientInfo":{"name":"smoke-client","version":"1.0.0"},"capabilities":{}}}' \
  "${BASE_URL}/mcp")"
mcp_initialize_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","clientInfo":{"name":"smoke-client","version":"1.0.0"},"capabilities":{}}}' \
  "${BASE_URL}/mcp")"
[[ "$mcp_initialize_status" == "200" ]] || fail "/mcp initialize returned status ${mcp_initialize_status}, expected 200"
assert_contains "$mcp_initialize_body" '"jsonrpc":"2.0"' "initialize did not return jsonrpc envelope"
assert_contains "$mcp_initialize_body" '"protocolVersion":"2024-11-05"' "initialize did not return protocolVersion"


echo "[smoke] checking POST /mcp tools/list"
tools_list_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":12,"method":"tools/list","params":{}}' \
  "${BASE_URL}/mcp")"
tools_list_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":12,"method":"tools/list","params":{}}' \
  "${BASE_URL}/mcp")"
[[ "$tools_list_status" == "200" ]] || fail "/mcp tools/list returned status ${tools_list_status}, expected 200"
assert_contains "$tools_list_body" '"list_services"' "tools/list did not include list_services"
assert_contains "$tools_list_body" '"list_logs"' "tools/list did not include list_logs"

echo "[smoke] checking POST /mcp tools/call list_services with state filter"
list_services_filtered_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":121,"method":"tools/call","params":{"name":"list_services","arguments":{"state":"inactive"}}}' \
  "${BASE_URL}/mcp")"
list_services_filtered_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":121,"method":"tools/call","params":{"name":"list_services","arguments":{"state":"inactive"}}}' \
  "${BASE_URL}/mcp")"
[[ "$list_services_filtered_status" == "200" ]] || fail "tools/call list_services filtered returned ${list_services_filtered_status}, expected 200"
assert_contains "$list_services_filtered_body" '"structuredContent"' "tools/call list_services filtered did not return structuredContent"

echo "[smoke] checking POST /mcp tools/call list_services with name_contains filter"
list_services_prefix_filtered_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":124,"method":"tools/call","params":{"name":"list_services","arguments":{"name_contains":"ssh"}}}' \
  "${BASE_URL}/mcp")"
list_services_prefix_filtered_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":124,"method":"tools/call","params":{"name":"list_services","arguments":{"name_contains":"ssh"}}}' \
  "${BASE_URL}/mcp")"
[[ "$list_services_prefix_filtered_status" == "200" ]] || fail "tools/call list_services prefix filtered returned ${list_services_prefix_filtered_status}, expected 200"
assert_contains "$list_services_prefix_filtered_body" '"structuredContent"' "tools/call list_services prefix filtered did not return structuredContent"

echo "[smoke] checking POST /mcp tools/call list_services invalid state"
list_services_invalid_state_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":122,"method":"tools/call","params":{"name":"list_services","arguments":{"state":"running"}}}' \
  "${BASE_URL}/mcp")"
list_services_invalid_state_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":122,"method":"tools/call","params":{"name":"list_services","arguments":{"state":"running"}}}' \
  "${BASE_URL}/mcp")"
[[ "$list_services_invalid_state_status" == "200" ]] || fail "tools/call list_services invalid state returned ${list_services_invalid_state_status}, expected 200"
assert_contains "$list_services_invalid_state_body" '"code":-32602' "tools/call list_services invalid state did not return invalid params error"
assert_contains "$list_services_invalid_state_body" '"invalid_state"' "tools/call list_services invalid state did not include invalid_state code"

echo "[smoke] checking POST /mcp resources/list"
resources_list_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":13,"method":"resources/list","params":{}}' \
  "${BASE_URL}/mcp")"
resources_list_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":13,"method":"resources/list","params":{}}' \
  "${BASE_URL}/mcp")"
[[ "$resources_list_status" == "200" ]] || fail "/mcp resources/list returned status ${resources_list_status}, expected 200"
assert_contains "$resources_list_body" '"resource://services/snapshot"' "resources/list missing service snapshot URI"
assert_contains "$resources_list_body" '"resource://services/failed"' "resources/list missing failed service snapshot URI"
assert_contains "$resources_list_body" '"resource://logs/recent"' "resources/list missing logs snapshot URI"

echo "[smoke] checking POST /mcp resources/read failed services snapshot"
failed_services_resource_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":123,"method":"resources/read","params":{"uri":"resource://services/failed"}}' \
  "${BASE_URL}/mcp")"
failed_services_resource_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":123,"method":"resources/read","params":{"uri":"resource://services/failed"}}' \
  "${BASE_URL}/mcp")"
[[ "$failed_services_resource_status" == "200" ]] || fail "resources/read failed services snapshot returned ${failed_services_resource_status}, expected 200"
assert_contains "$failed_services_resource_body" '"result"' "resources/read failed services snapshot did not return result"
assert_contains "$failed_services_resource_body" '"resource://services/failed"' "resources/read failed services snapshot did not include expected uri"

echo "[smoke] checking POST /mcp ping"
mcp_ping_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":2,"method":"ping"}' \
  "${BASE_URL}/mcp")"
mcp_ping_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":2,"method":"ping"}' \
  "${BASE_URL}/mcp")"
[[ "$mcp_ping_status" == "200" ]] || fail "/mcp ping returned status ${mcp_ping_status}, expected 200"
assert_contains "$mcp_ping_body" '"jsonrpc":"2.0"' "ping did not return jsonrpc envelope"
assert_contains "$mcp_ping_body" '"result":{}' "ping did not return empty result object"


echo "[smoke] checking POST /mcp ping with invalid token"
mcp_ping_invalid_token_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer invalid-token" \
  -d '{"jsonrpc":"2.0","id":3,"method":"ping"}' \
  "${BASE_URL}/mcp")"
mcp_ping_invalid_token_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer invalid-token" \
  -d '{"jsonrpc":"2.0","id":3,"method":"ping"}' \
  "${BASE_URL}/mcp")"
[[ "$mcp_ping_invalid_token_status" == "401" ]] || fail "/mcp ping with invalid token returned ${mcp_ping_invalid_token_status}, expected 401"
assert_contains "$mcp_ping_invalid_token_body" '"code":"invalid_token"' "/mcp ping with invalid token body did not contain invalid_token"

echo "[smoke] checking POST /mcp tools/call list_logs"
list_logs_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":14,"method":"tools/call","params":{"name":"list_logs","arguments":{"order":"desc","start_utc":"1970-01-01T00:00:00Z","end_utc":"1970-01-01T01:00:00Z","limit":10}}}' \
  "${BASE_URL}/mcp")"
list_logs_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":14,"method":"tools/call","params":{"name":"list_logs","arguments":{"order":"desc","start_utc":"1970-01-01T00:00:00Z","end_utc":"1970-01-01T01:00:00Z","limit":10}}}' \
  "${BASE_URL}/mcp")"
[[ "$list_logs_status" == "200" ]] || fail "tools/call list_logs returned ${list_logs_status}, expected 200"
assert_contains "$list_logs_body" '"structuredContent"' "tools/call list_logs did not return structuredContent"
assert_contains "$list_logs_body" '"total_scanned"' "tools/call list_logs did not include total_scanned metadata"
assert_contains "$list_logs_body" '"window"' "tools/call list_logs did not include window metadata"

echo "[smoke] checking POST /mcp tools/call list_services"
list_services_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":16,"method":"tools/call","params":{"name":"list_services","arguments":{}}}' \
  "${BASE_URL}/mcp")"
list_services_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":16,"method":"tools/call","params":{"name":"list_services","arguments":{}}}' \
  "${BASE_URL}/mcp")"
[[ "$list_services_status" == "200" ]] || fail "tools/call list_services returned ${list_services_status}, expected 200"
assert_contains "$list_services_body" '"structuredContent"' "tools/call list_services did not return structuredContent"
assert_contains "$list_services_body" '"services"' "tools/call list_services did not include services payload"
assert_contains "$list_services_body" '"total"' "tools/call list_services did not include total metadata"
assert_contains "$list_services_body" '"returned"' "tools/call list_services did not include returned metadata"
assert_contains "$list_services_body" '"truncated"' "tools/call list_services did not include truncated metadata"
assert_contains "$list_services_body" '"generated_at_utc"' "tools/call list_services did not include generated_at_utc metadata"

echo "[smoke] checking POST /mcp tools/call list_logs invalid limit"
list_logs_invalid_limit_body="$(curl -sS -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"name":"list_logs","arguments":{"start_utc":"1970-01-01T00:00:00Z","end_utc":"1970-01-01T01:00:00Z","limit":1001}}}' \
  "${BASE_URL}/mcp")"
list_logs_invalid_limit_status="$(curl -sS -o /dev/null -w "%{http_code}" -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"name":"list_logs","arguments":{"start_utc":"1970-01-01T00:00:00Z","end_utc":"1970-01-01T01:00:00Z","limit":1001}}}' \
  "${BASE_URL}/mcp")"
[[ "$list_logs_invalid_limit_status" == "200" ]] || fail "tools/call list_logs invalid limit returned ${list_logs_invalid_limit_status}, expected 200"
assert_contains "$list_logs_invalid_limit_body" '"code":-32602' "tools/call list_logs invalid limit did not return invalid params error"
assert_contains "$list_logs_invalid_limit_body" '"invalid_limit"' "tools/call list_logs invalid limit did not include invalid_limit code"

echo "[smoke] PASS"
