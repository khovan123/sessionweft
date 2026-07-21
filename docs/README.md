# SessionWeft Documentation

`PROJECT.md` is the project source of truth. This directory contains the evidence, decisions, specifications, and operational material that support it.

## Structure

| Directory | Purpose | Current phase |
|---|---|---|
| `00-product` | Vision, scope, glossary, product requirements | Phase -1 |
| `01-research` | Capability matrix, landscape research, scoring and evidence | Phase -1 / 0 |
| `02-architecture` | System context, containers, domain and deployment architecture | Architecture Review |
| `03-rfc` | Implementation contracts and protocol specifications | Phase 1 |
| `04-adr` | Approved architectural decisions | ADR |
| `05-specs` | Production and non-functional specifications | Phase 1 |
| `06-api` | Protobuf, event and error catalogs | Phase 1 / 2 |
| `07-sdk` | Provider, agent and plugin SDK documentation | Phase 1 / 2 |
| `08-operations` | Runbooks, incident response and observability | Phase 1 / 2 |
| `09-testing` | Test strategy, conformance suites and benchmarks | All phases |
| `10-deployment` | Local, staging and production deployment | Phase 1 / 2 |

## Evidence levels

Every technical statement should be marked or written so its status is clear:

- **Project requirement**: mandated by `PROJECT.md` or the PDD.
- **Research finding**: supported by primary documentation, source code, release data or reproducible tests.
- **Proposal**: a recommended direction that has not been approved.
- **Decision**: approved through an ADR or RFC.
- **TBD**: unresolved and blocked on research or review.

## Gate discipline

- No Phase 0 recommendation without a capability and scoring criteria.
- No architectural commitment without research evidence.
- No implementation dependency without an approved ADR or RFC when the decision is material.
- No phase is complete until its exit criteria and review record are committed.
