# Project Status

Last updated: 2026-07-22  
Current phase: **Phase -1 — Capability Matrix**  
Implementation status: **Blocked by project gates**

## Completed in the bootstrap branch

- [x] Public project overview
- [x] Documentation map and evidence rules
- [x] Initial Capability Matrix
- [x] Initial technology findings from primary sources
- [x] Research report template
- [x] ADR template
- [x] RFC template
- [x] Production readiness checklist

## Phase -1 remaining

- [ ] Review capability coverage against `PROJECT.md`
- [ ] Resolve product-level open questions
- [ ] Add measurable SLO placeholders for every `Must` capability
- [ ] Define required programming languages for workspace benchmarks
- [ ] Define local, single-user and multi-user deployment expectations
- [ ] Approve the Capability Matrix
- [ ] Record the Phase -1 gate review

## Phase 0 ready queue

1. Provider APIs and routing
2. Local and durable event transport
3. Memory systems
4. Workflow durability
5. Workspace parsing and indexing
6. Git integration
7. MCP and plugin isolation
8. Storage and session recovery
9. CLI, TUI and IDE client architecture
10. Security and observability baseline

## Current provisional direction

- Rust + Tokio runtime
- tonic/prost gRPC boundary
- Runtime-owned session state
- Local event adapter plus NATS JetStream durable adapter
- Official MCP Rust SDK behind SessionWeft policy wrappers
- Direct provider conformance before optional gateways
- Memory provider interface before adopting a memory platform
- Prototype workflow durability alternatives before implementation

These are research recommendations, not approved architectural decisions.

## Immediate exit criteria

Phase -1 exits only when:

- every project domain is represented in the Capability Matrix;
- every `Must` capability has an observable acceptance criterion;
- unresolved product requirements are explicitly tracked;
- the matrix is reviewed and approved;
- Phase 0 research owners and scoring criteria are assigned.
