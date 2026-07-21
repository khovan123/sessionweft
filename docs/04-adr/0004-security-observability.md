# ADR-0004: Default-Deny Security and Correlated Telemetry

- Status: Accepted
- Date: 2026-07-22
- Issues: #17, #10

## Context

SessionWeft executes provider calls, terminal commands, filesystem changes, Git operations and MCP tools. These boundaries can expose secrets or create external side effects. Production incidents must be traceable without logging sensitive content.

## Decision

1. Default deny for tools, plugins, terminal execution and workspace writes.
2. Separate discovery from authorization and execution.
3. Attach actor, session, task, correlation and policy-decision IDs to external actions.
4. Do not log prompt, tool arguments, provider body, secret values or file contents by default.
5. Apply secret redaction before writing logs, events or memory.
6. Local mode binds to loopback unless explicitly overridden.
7. Team service mode requires an API bearer token in the first slice.
8. Use structured `tracing` fields compatible with OpenTelemetry export.
9. Track Session latency, conflict count, outbox age, provider latency/error, usage, cost and authorization denial.
10. Lock dependencies, run vulnerability/license checks, generate an SBOM and sign release artifacts before GA.

## Consequences

- Initial authentication is intentionally narrow and must be replaced or wrapped by OIDC for broader deployments.
- Debugging content-related issues may require an explicit, time-bounded diagnostic mode with stronger access controls.
- Metrics use bounded labels; Session IDs belong in traces/logs, not metric labels.

## Alternatives

- Trust provider/MCP annotations as authorization: rejected because descriptions are untrusted input.
- Log full bodies for convenience: rejected.
- No authentication because the first client is local: rejected for team service mode.

## Required tests

- token rejection;
- loopback default;
- secret-redaction fixtures;
- tool permission denial;
- audit record presence;
- no request-body logging;
- bounded metric cardinality review.
