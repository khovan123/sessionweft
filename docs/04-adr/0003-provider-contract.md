# ADR-0003: Runtime-Owned Provider Contract

- Status: Accepted
- Date: 2026-07-22
- Issues: #2, #10

## Context

OpenAI, Anthropic, Gemini and Ollama expose different message, streaming, tool-use, usage and continuation models. SessionWeft must switch providers without transferring state ownership to a provider.

## Decision

1. Define a provider-neutral `Provider` trait.
2. Build every request from Runtime-owned context.
3. Normalize output into typed events: started, text delta, tool-call delta, tool call, usage, completed and failed.
4. Store provider request/response IDs only as optional audit metadata.
5. Never use provider conversation IDs as Session identity.
6. Require cancellation, timeout and normalized errors.
7. Add providers only after passing the provider conformance suite.
8. Implement Echo as the deterministic test adapter and an Ollama-compatible adapter as the offline reference.
9. Keep gateways optional behind the same contract.

## Consequences

- Some provider-specific features cannot be represented losslessly and must be exposed through declared capabilities or opaque metadata.
- Runtime-owned context may use more input tokens than provider-side continuation, but it preserves portability and auditability.
- Tool calls remain requests; Runtime policy decides whether execution is allowed.

## Alternatives

- Common lowest-denominator text-only interface: rejected because tool and streaming behavior are core requirements.
- Provider SDK objects in domain models: rejected because they leak provider versioning into Session contracts.
- Mandatory gateway: rejected because it creates a second state and availability dependency.

## Compatibility

Each adapter is tested against common fixtures for text, tool calls, malformed partial JSON, cancellation, timeout, usage and error classification.
