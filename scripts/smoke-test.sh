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

echo "[smoke] checking GET /units without token"
units_unauth_body="$(curl -sS "${BASE_URL}/units")"
units_unauth_status="$(curl -sS -o /dev/null -w "%{http_code}" "${BASE_URL}/units")"
[[ "$units_unauth_status" == "401" ]] || fail "/units without token returned ${units_unauth_status}, expected 401"
assert_contains "$units_unauth_body" '"code":"missing_token"' "/units without token body did not contain missing_token"

echo "[smoke] checking GET /units with token"
units_auth_body="$(curl -sS -H "Authorization: Bearer ${TOKEN}" "${BASE_URL}/units")"
units_auth_status="$(curl -sS -o /dev/null -w "%{http_code}" -H "Authorization: Bearer ${TOKEN}" "${BASE_URL}/units")"

[[ "$units_auth_status" == "200" ]] || fail "/units with token returned ${units_auth_status}, expected 200"
assert_contains "$units_auth_body" '[' "/units with token returned 200 but not a JSON array"

echo "[smoke] running units (state=active):"
UNITS_AUTH_BODY="$units_auth_body" python3 - <<'PY'
import json
import os
import sys

body = os.environ.get("UNITS_AUTH_BODY", "")

try:
  data = json.loads(body)
except json.JSONDecodeError as exc:
  print(f"[smoke] FAIL: could not parse /units JSON: {exc}", file=sys.stderr)
  sys.exit(1)

if not isinstance(data, list):
  print("[smoke] FAIL: /units response is not a JSON array", file=sys.stderr)
  sys.exit(1)

running = [
  unit.get("name")
  for unit in data
  if isinstance(unit, dict) and unit.get("state") == "active" and unit.get("name")
]

if not running:
  print("[smoke] (none)")
else:
  for name in running:
    print(f"[smoke] - {name}")
PY

echo "[smoke] PASS"
