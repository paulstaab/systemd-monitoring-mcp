# Test Cases

## Logs Endpoint

- `GET /logs` without authorization header returns `401` with `missing_token` code.
- `GET /logs` with valid bearer token returns `200` and a JSON array.
- `GET /logs` returns log entries in descending timestamp order (newest first).
- `GET /logs?limit=0` returns `400` with `invalid_limit` code.
- `GET /logs?limit=1001` returns `400` with `invalid_limit` code.
- `GET /logs?priority=error` maps to journald priority `3`, applies minimum-threshold filtering, and returns `200`.
- `GET /logs?priority=9` returns `400` with `invalid_priority` code.
- `GET /logs?unit=sshd_service-01@host:prod` returns only entries matching that unit identifier.
- `GET /logs?unit=sshd.service` returns `400` with `invalid_unit` code.
- `GET /logs` with missing `start_utc` and/or `end_utc` returns `400` with `missing_time_range` code.
- `GET /logs?start_utc=2026-02-27T00:00:00Z&end_utc=2026-02-27T01:00:00Z` returns entries within range and `200`.
- `GET /logs?start_utc=2026-02-27T01:00:00Z&end_utc=2026-02-27T00:00:00Z` returns `400` with `invalid_time_range` code.
- `GET /logs?start_utc=2026-02-27T01:00:00+01:00` returns `400` with `invalid_utc_time` code.

## Discovery Metadata

- `GET /.well-known/mcp` includes `logs_endpoint` set to `/logs`.
- `POST /mcp` with `initialize` includes `metadata.restEndpoints.logs` set to `/logs`.
- `POST /` with `initialize` returns `200` and includes `metadata.restEndpoints.logs` set to `/logs`.
