# Phase 0 Landscape Research — Execution Plan

Status: **Active**  
Date: 2026-07-22  
Source of truth: [`PROJECT.md`](../../PROJECT.md)  
Capability baseline: [`capability-matrix.md`](capability-matrix.md)

## 1. Objective

Produce enough evidence to begin Architecture Review without committing SessionWeft to dependencies, protocols or deployment assumptions that have not been evaluated.

Phase 0 is complete only when all required research streams have reviewed reports, candidate dispositions, risk analysis, reuse scores and effort estimates.

## 2. Research rules

1. Use primary sources first: official specifications, documentation, source repositories, releases and reproducible tests.
2. Record the exact version or commit evaluated.
3. Separate facts, measurements, proposals and approved decisions.
4. Do not treat a successful hello-world prototype as production readiness.
5. Include failure, recovery, security and replacement behavior in every material evaluation.
6. Preserve Session as the source of truth during every prototype.
7. Store raw benchmark inputs and outputs alongside the report or in a linked artifact.
8. A candidate receives one disposition: Adopt, Wrap, Fork, Replace, Reject or Continue Research.

## 3. Workstreams

| Stream | Issue | Capabilities | Primary output |
|---|---:|---|---|
| Session persistence and recovery | #8 | SES, DEP, TST | Session model, storage/recovery evidence |
| Events and outbox | #3 | EVT, SES, OBS | Event semantics and transport recommendation |
| Provider API and routing | #2 | PRV, CTX, API | Provider contract and reference-adapter recommendation |
| Coding-agent architecture | #13 | AGT, COD, TRM, CTX | Reuse patterns and Agent SDK constraints |
| Workflow durability | #5 | WF, SES, AGT | Custom DAG versus Temporal recommendation |
| Hierarchical locking | #14 | LCK, AGT, GIT | Lock state machine and fencing model |
| Git and workspace isolation | #6 | GIT, LCK, WSP | Git library and worker-isolation recommendation |
| Workspace parsing and indexing | #9 | WSP, CTX, TST | Parsing/search stack and benchmark |
| Memory engines | #4 | MEM, CTX, SEC | Memory provider and retrieval benchmark |
| Vector database and embeddings | #16 | VEC, MEM, DEP | Vector-store lifecycle recommendation |
| MCP and plugin isolation | #7 | MCP, PLG, SEC | MCP interoperability and isolation model |
| CLI, TUI, terminal and IDE | #15 | CLI, IDE, TRM, API | Client and process-execution architecture |
| Security and observability | #17 | SEC, OBS, API, PLG | Threat, telemetry and supply-chain baseline |
| Phase 0 synthesis | #18 | All | Shortlist, scorecards, risk and approval |

## 4. Execution waves

### Wave A — State and contract foundations

Run first because these results constrain most other research:

1. **#8 Session persistence, concurrency and crash recovery**
2. **#3 Local events, NATS JetStream and outbox semantics**
3. **#2 Provider API and routing strategy**
4. **#17 Security, observability and supply-chain baseline**

Expected shared outputs:

- Session identity and versioning assumptions
- Transaction and recovery boundaries
- Event envelope and delivery assumptions
- Provider normalized event vocabulary
- Authentication, authorization, audit and correlation requirements

### Wave B — Execution and collaboration

May begin in parallel after Wave A contracts are stable enough for prototypes:

1. **#13 Coding-agent architecture**
2. **#5 Workflow durability**
3. **#14 Hierarchical locking and leases**
4. **#6 Git and workspace isolation**
5. **#7 MCP and plugin isolation**
6. **#15 CLI, TUI, terminal and VS Code architecture**

Required integration scenarios:

- Agent failure and task handover
- Terminal cancellation and persisted result
- Workflow restart without duplicate side effects
- Lock lease expiry with fencing
- Git merge conflict and rollback
- MCP tool permission denial and cancellation
- Client disconnect and reconnect

### Wave C — Retrieval and context

Run after Session, workspace identity and security scope are sufficiently defined:

1. **#9 Workspace parsing, indexing and incremental context**
2. **#4 Memory engines and retrieval benchmark**
3. **#16 Vector database, embeddings and rebuild strategy**

Required integration scenarios:

- File change triggers incremental index update
- Context item carries source and inclusion reason
- Superseded decisions remain historical but not current
- Deleted memory and vectors disappear from retrieval
- Vector index can be rebuilt from authoritative data

### Wave D — Synthesis and gate review

1. Complete **#18 Phase 0 Synthesis**.
2. Update the technology shortlist and risk register.
3. Verify every candidate has a disposition and replacement plan.
4. Update implementation effort estimates.
5. Record Phase 0 approval or documented blockers.
6. Start **#10 Architecture Review** only after approval.

## 5. Dependency map

```text
#8 Session ───────┬──> #3 Events
                  ├──> #5 Workflow
                  ├──> #14 Locking
                  ├──> #4 Memory
                  └──> #10 Architecture Review

#2 Providers ─────┬──> #13 Coding Agents
                  ├──> #15 Clients
                  └──> #10 Architecture Review

#17 Security/Obs ─┬──> #7 MCP/Plugins
                  ├──> #15 Clients
                  ├──> #4 Memory
                  └──> #10 Architecture Review

#14 Locking ──────┬──> #6 Git
                  └──> #10 Architecture Review

#6 Git + #9 Workspace ──> #13 Coding Agents / #10 Architecture Review
#4 Memory + #16 Vector + #9 Workspace ──> Context architecture
All research streams ──> #18 Synthesis ──> #10 Architecture Review
```

Dependencies indicate required contract alignment, not a requirement that all work be strictly sequential.

## 6. Standard scorecard

Each candidate is scored from 0 to 5.

| Criterion | Weight | Minimum evidence |
|---|---:|---|
| Required capability coverage | 18 | Capability mapping and prototype |
| Reliability and recovery | 14 | Failure-injection results |
| Security and isolation | 12 | Threat and permission analysis |
| Production operations | 10 | Deployment, backup, upgrade and telemetry |
| Integration effort | 10 | Prototype and implementation estimate |
| Extensibility and replacement | 10 | Adapter boundary and migration plan |
| Performance and scalability | 8 | Reproducible benchmark |
| License and governance | 7 | License and project ownership review |
| Maintenance and community | 6 | Releases, issues and contributor evidence |
| Documentation and developer experience | 5 | Setup and API evaluation |

A high total score does not override a failed mandatory requirement. Candidates with unacceptable license, security, durability or state-ownership behavior must be rejected or wrapped behind a compensating boundary.

## 7. Evidence quality

| Level | Description |
|---|---|
| E0 | Unsupported claim or memory; not acceptable |
| E1 | Marketing or secondary summary |
| E2 | Official documentation or specification |
| E3 | Source-code/release inspection |
| E4 | Reproducible prototype or benchmark |
| E5 | Failure, security or compatibility test under SessionWeft constraints |

Material recommendations require E2 plus E4 evidence. Durability, security and compatibility decisions require E5 evidence where practical.

## 8. Shared benchmark scope

### Languages

- Rust
- TypeScript/JavaScript
- Python

### Repository tiers

- **Small:** unit and development corpus for fast CI.
- **Medium:** realistic application with multiple packages and history.
- **Large:** stress corpus with versioned source and documented hardware.

Exact file, symbol and line-count thresholds are defined by #9 and reused by Git, Context, Memory and performance research.

### Failure scenarios

- Forced runtime termination
- Provider disconnect and rate limit
- Duplicate and delayed event delivery
- Database transaction failure
- Agent heartbeat loss
- Stale lock owner
- Workflow worker restart
- MCP/plugin timeout or malicious access attempt
- Client disconnect during streaming
- Index corruption and rebuild

## 9. Required Phase 0 aggregate artifacts

- Research reports for every stream
- Raw prototype and benchmark evidence
- Candidate and license inventory
- License compatibility matrix
- Security and maintenance risk matrix
- Reuse scorecard
- Technology shortlist
- Adopt/Wrap/Fork/Replace/Reject dispositions
- Replacement and migration strategies
- Implementation and operational effort estimate
- Architecture risk register
- Phase 0 review record

## 10. Phase 0 exit criteria

Phase 0 may be approved only when:

- all `PROJECT.md` research categories are covered;
- every selected technology maps to required capabilities;
- every material recommendation has primary-source and reproducible evidence;
- durable state remains Runtime-owned in every proposed design;
- security, failure and recovery behavior are explicit;
- license and maintenance risks are known;
- local and production deployment impacts are understood;
- unresolved questions are either blockers or assigned to Architecture/ADR work;
- #18 records the final review result.

Production implementation remains blocked after Phase 0. The next authorized activity is Architecture Review, followed by ADR and RFC gates.
