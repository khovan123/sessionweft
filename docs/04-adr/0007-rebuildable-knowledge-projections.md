# ADR-0007: Runtime-Owned Memory and Rebuildable Knowledge Projections

- Status: Accepted
- Date: 2026-07-22
- Issues: #4, #9, #16

## Context

SessionWeft needs repository retrieval, durable decisions, preferences, error memory and provider-sized context. External memory and vector products must not become a second source of truth or make local mode require additional services.

## Decision

1. Persist typed memory records with Conversation, Repository, Decision, Preference and Error classes.
2. Require session scope, provenance, source locator, timestamps and lifecycle fields.
3. Model replacement through explicit supersession rather than destructive overwrite.
4. Support verified deletion and exclude deleted/superseded/expired records from retrieval.
5. Start with deterministic lexical retrieval owned by Runtime.
6. Build provider context from ranked candidates under an explicit token budget.
7. Record source, inclusion reason and omitted items for every context package.
8. Scan workspaces only inside a canonical root, do not follow symlinks, and cap files and bytes.
9. Treat workspace indexes, embeddings and vector records as rebuildable projections.
10. Keep embedding and vector-store boundaries replaceable and namespace-scoped.
11. Do not require Qdrant in local mode. Prefer PostgreSQL/pgvector first in service mode when vector evidence justifies it.

## Consequences

- The lexical baseline is less semantically powerful than embeddings but deterministic, inspectable and cheap.
- Memory provenance and deletion can be tested independently of external services.
- Context selection can be benchmarked before adopting an advanced memory engine.
- File scanning does not yet provide tree-sitter symbols or LSP references; those remain later adapters.
- Vector indexes can be deleted and rebuilt without losing authoritative memory or repository state.

## Alternatives

- Make Mem0, Graphiti or Zep authoritative: rejected.
- Require Qdrant for every deployment: rejected.
- Broadcast full Session history to every provider request: rejected.
- Follow workspace symlinks automatically: rejected due to boundary escape risk.
