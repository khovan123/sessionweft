# Contributing to SessionWeft

SessionWeft is currently research- and specification-first. Contributions must preserve the project gates in `PROJECT.md`.

## Language

Use English for source code, identifiers, commit messages and repository documentation intended for public consumption. Existing Vietnamese planning material may remain until it is normalized through an approved documentation task.

## Before implementation

A code change introducing a material dependency or protocol must link to:

1. the applicable Capability Matrix IDs;
2. a research report using primary sources and reproducible evidence;
3. an approved ADR when the architecture changes;
4. an approved RFC when a public or durable contract changes.

Small documentation, test-harness and research-spike changes may precede ADR approval if they do not create production commitments.

## Branches and commits

Suggested branch prefixes:

- `research/`
- `adr/`
- `rfc/`
- `spike/`
- `feature/`
- `fix/`
- `docs/`
- `chore/`

Use focused conventional-style commit messages, for example:

- `research: compare provider streaming semantics`
- `adr: select durable event transport`
- `feat(session): add optimistic concurrency`
- `test(provider): add tool-call conformance cases`

## Pull request expectations

Every pull request should state:

- project phase and gate;
- linked capabilities, research, ADRs and RFCs;
- what is deliberately out of scope;
- failure and recovery behavior;
- security impact;
- test and benchmark evidence;
- documentation changes.

## Definition of done

A production implementation change is not complete until:

- code review is approved;
- relevant unit, contract and integration tests pass;
- failure paths are tested;
- logging, metrics and tracing are considered;
- security implications are documented;
- public contracts and `PROJECT.md` status are updated when required.

## Prohibited shortcuts

- Do not store durable state only inside an agent, IDE or CLI process.
- Do not make provider conversation IDs the SessionWeft session identity.
- Do not execute tools or plugins without runtime policy checks.
- Do not use unbounded queues without an explicit capacity decision.
- Do not retry side effects without idempotency or compensation.
- Do not mark a research candidate as adopted before review and ADR approval.
