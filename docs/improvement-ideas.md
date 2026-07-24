# Improvement Ideas

## Purpose

This document collects significant improvement and refactoring ideas encountered during coding sessions.
Use it as a lightweight planning backlog, not as a replacement for issue tracking or required documentation updates.

## Entry Guidelines

- Add ideas only when they are significant enough to deserve future planning, review, or implementation.
- Keep entries concise and actionable.
- Include file, subsystem, or workflow references when that context would help a future contributor.
- Sort entries under the most relevant category.
- It is fine for a coding session to add nothing.

## Security

## Technical Debt

## Code Structure

- Consider recursive dependency graph inspection with cycle detection and strict depth/size bounds; keep `get_unit_status` direct-only.

- Add a bounded deployment-settle workflow after the read-only status and transition APIs prove stable.

## Maintainability

## Performance

## Testing

## Documentation

## Developer & Agent Experience

## Packaging and Deployment

- Consider Podman container/pod list and search tools after identifier-based inspection usage is established.
