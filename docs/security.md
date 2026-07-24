# Security and Threat Model

## Security Objective

The threat model distinguishes two attacker cases. Both may issue arbitrary valid, invalid, repeated,
or adversarial network requests.

### Case 1: Network access without the token

This actor can reach the HTTP listener but does not possess a valid `MCP_API_TOKEN`.

This actor must not be able to:

- invoke MCP tools or resources or access the authenticated systemd status endpoints;
- obtain host monitoring data or secrets; or
- make persistent changes to the server host or its workloads through this service.

This actor may obtain the fixed public health response, public discovery metadata, and network-level
signals such as reachability, response timing, and response size. These are accepted non-critical
disclosures. Application-level denial of service by this actor is not accepted: unauthenticated
requests must be rejected before monitoring-provider work. Volumetric attacks against the network or
reverse proxy remain an infrastructure concern outside the application boundary.

### Case 2: Network access with the token

This actor possesses a valid `MCP_API_TOKEN`. It includes a malicious authorized client, a compromised
client, or a runaway agent using legitimately configured credentials.

This actor may invoke every documented read-only monitoring capability and therefore may obtain the
non-critical monitoring information explicitly documented in `docs/requirements.md`.

This actor must not be able to:

- make persistent changes to the server host or its workloads through this service; or
- obtain secrets intentionally held by the host, providers, server configuration, or other clients.

It is acceptable for this actor to exhaust service resources or disrupt availability.

“Persistent changes” includes starting, stopping, restarting, enabling, disabling, or editing systemd
units; changing timers; writing or deleting files; changing processes, containers, pods, networking,
packages, or host configuration; and executing arbitrary commands. Incidental effects of reads, such
as audit records, cache activity, access timestamps, CPU/memory use, and temporary provider load, are
not considered persistent modification.

“Secrets” includes bearer tokens, credentials, environment secrets, private keys, authorization
headers, host mount source paths, and raw provider configuration likely to contain credentials. The
server minimizes and redacts known secret-bearing data rather than returning raw provider objects.

This guarantee has a content boundary: journald messages and other permitted free-form monitoring
fields can contain arbitrary application text. The server cannot reliably determine whether such text
is secret. Operators must not write secrets to monitoring sources exposed by this server, and should
treat authorized log access as sensitive.

## Public and Authenticated Surfaces

- `GET /health` is public and returns only `{"status":"ok"}`.
- `GET /.well-known/mcp` is public and returns package name, package version, and `/mcp` path.
- `/mcp` and both systemd status endpoints require a valid bearer token.
- Authenticated MCP methods expose only the read-only monitoring capabilities in the requirements.

Public endpoints and network behavior reveal service existence, reachability, timing, response size,
package identity, and version. These are accepted non-critical disclosures. Binding to loopback or a
private interface and applying firewall rules reduces even this observable surface.

## Trust and Deployment Assumptions

- The binary, host OS, Rust dependencies, systemd D-Bus, journal, and optional Podman installation are
  trusted. Host-local compromise is outside this network-client threat model.
- The server uses plain HTTP. TLS must be supplied by a trusted reverse proxy or equivalent transport
  whenever traffic crosses an untrusted network; otherwise the bearer token can be captured and
  replayed.
- A valid token identifies an authorized connection, not a trustworthy agent. Authorization therefore
  does not relax read-only, validation, minimization, or redaction controls.
- The process should run as a dedicated, least-privileged account with only the D-Bus, journal, and
  Podman access required for monitoring.
- Denial of service is accepted only from an actor with a valid token. Network-edge connection,
  request-body, concurrency, timeout, and rate limits should protect against unauthenticated volumetric
  attacks.

## Attack Scenarios

- **Host mutation:** guessed methods or crafted inputs attempt to change host or workload state.
- **Injection:** hostile identifiers, filters, cursors, regexes, or JSON attempt command execution.
- **Response leakage:** broad monitoring requests seek credentials or raw provider configuration.
- **Log leakage:** credentials in headers or parameters attempt to enter application logs.
- **Token attacks:** guessing, timing analysis, interception, and replay target the static token.
- **Resource exhaustion:** disruption is accepted only after successful token authentication.

Controls include a read-only capability allowlist, strict validation, no shell execution, projected
responses, secret-field omission and redaction, opaque errors, HMAC-based token comparison, bounded
provider work, least privilege, TLS, and network access controls.

## Review Checklist for New Capabilities

1. Add the exposed behavior and fields to `docs/requirements.md`.
2. Prove that no request path can persistently mutate host or workload state.
3. Return projected fields only; exclude raw objects and known secret-bearing data.
4. Validate untrusted input before provider access and never pass it through a shell.
5. Keep errors opaque and update parameter/log redaction.
6. Reject unauthenticated work early and bound authenticated work to reduce host impact.
7. Add negative tests for mutation attempts, injection, authentication bypass, and secret leakage.
8. Update smoke checks when the externally observable security boundary changes.

## Reporting Security Issues

Do not put tokens, host logs, environment values, or exploit details in a public issue. Use the
repository's private vulnerability-reporting channel, or contact maintainers privately before sharing
sensitive details.

