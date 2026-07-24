# Structured Runtime Inspection and Paginated Logs Implementation Plan

## Scope

This release adds detailed systemd service inspection, optional read-only Podman inspection, and resumable journal pages with projection and page-local grouping.

## Work

1. Extend service output with restart and timestamp metadata while retaining compatibility fields.
2. Add mockable single-unit inspection, direct dependency classification, transition lookup, and latest process-start boundaries.
3. Add a mockable Podman provider using only fixed-argument, bounded, timed CLI calls and compact DTOs.
4. Add native journal cursor seeking, continuation metadata, field projection, and page-local message grouping.
5. Synchronize MCP schemas, error details, smoke checks, tests, and documentation comments.
6. Run formatting, Clippy with warnings denied, and the full test suite.

## Compatibility

Existing tools and resources retain their names and response fields. Additive service fields and `next_cursor` preserve existing clients. Grouped results use `groups`; ungrouped results continue to use `logs`.

## Deferred

Deployment-settle waiting, recursive dependency traversal, and Podman list/search operations remain backlog items.

## Security Hardening: Podman Response Minimization

1. Remove raw create commands and host mount sources from the public DTO.
2. Sanitize credential-like argv flags, flag assignments, and environment-style assignments.
3. Project health configuration onto sanitized test argv and non-secret scheduling fields only.
4. Add regression tests proving raw secrets and excluded fields cannot appear in serialized responses.
