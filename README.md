# SessionWeft

**One session. Many agents. One runtime.**

SessionWeft is a session-first, provider-agnostic runtime for coordinating AI agents over a shared workspace. The runtime owns durable state; IDEs, CLIs, providers, and agents act as clients or pluggable execution components.

## Status

The project is currently in **Phase -1: Capability Matrix**. Product implementation must not begin until the research, architecture, ADR, and RFC gates defined in [`PROJECT.md`](PROJECT.md) are satisfied.

## Core principles

- Session-first
- Provider-agnostic
- Event-driven
- Plugin-first
- MCP-native
- Shared memory and workspace
- Lock-based collaboration
- Incremental context assembly
- Production concerns before feature expansion

## Planned architecture

```text
IDE / CLI
    |
    v
Runtime Core
    |-- Session Engine
    |-- Provider Layer
    |-- Agent Runtime
    |-- Workflow Engine
    |-- Workspace Engine
    |-- Collaboration / Locking
    |-- Memory and Context
    `-- MCP / Plugin Runtime
```

## Delivery sequence

1. Capability Matrix
2. Landscape Research
3. Architecture Review
4. ADRs
5. RFCs and Production Specification
6. Implementation
7. Testing and hardening
8. Release and GA

## Documentation

- [`PROJECT.md`](PROJECT.md): source of truth and full delivery plan
- [`docs/README.md`](docs/README.md): documentation map
- [`docs/01-research/capability-matrix.md`](docs/01-research/capability-matrix.md): Phase -1 baseline
- [`docs/01-research/initial-technology-findings.md`](docs/01-research/initial-technology-findings.md): initial research findings

## Contribution rule

Do not introduce an implementation dependency or architectural commitment without a linked research result and, when applicable, an approved ADR or RFC.
