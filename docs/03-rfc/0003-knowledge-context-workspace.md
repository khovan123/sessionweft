# RFC-0003: Knowledge, Context and Workspace Retrieval Baseline

- Status: Accepted for implementation
- Date: 2026-07-22
- ADR: ADR-0007

## Scope

This RFC adds the first Runtime-owned knowledge layer:

- typed memory records and lifecycle;
- SQLite memory persistence with audit outbox;
- deterministic lexical memory retrieval;
- explicit context candidate ranking and token budgeting;
- canonical-root workspace discovery and lexical search;
- replaceable vector-store contract and in-memory conformance implementation.

Deferred:

- tree-sitter symbol graphs;
- LSP lifecycle and reference indexing;
- file watchers and incremental persistence;
- embedding provider adapters;
- pgvector and Qdrant adapters;
- advanced memory adapters such as Mem0, Graphiti and Zep;
- automatic context summarization.

## Memory contract

Every memory record contains:

- Runtime-generated memory ID;
- Session ID;
- memory class;
- content;
- source kind, locator and optional revision;
- tags;
- validity interval;
- supersession links;
- deletion timestamp;
- creation and update timestamps.

Active retrieval excludes deleted, superseded and expired records. Supersession inserts the replacement and updates the old record in the same transaction. Memory mutations and audit events commit together.

## Retrieval baseline

The first ranking implementation is lexical and deterministic:

- normalized alphanumeric/underscore terms;
- content frequency contribution;
- source-locator contribution;
- tag bonus;
- bounded recency bonus;
- stable score and ID ordering.

Search is scoped by Session and may filter memory classes and tags. This is a baseline for benchmark comparison, not a claim of semantic equivalence to embedding retrieval.

## Context package

A context candidate includes:

- stable candidate ID;
- kind;
- content;
- source;
- inclusion reason;
- priority;
- relevance;
- required flag.

The builder reserves provider output tokens, validates required content fits, sorts required/priority/relevance deterministically, includes optional items while budget remains, and records every omitted item with a reason.

The first token estimate is a conservative character-based approximation. Provider-specific tokenizers remain adapter extensions.

## Workspace boundaries

- Canonicalize the configured root.
- Reject a non-directory root.
- Do not follow symlinks.
- Canonicalize discovered directories/files and verify they remain inside root.
- Store normalized relative paths.
- Cap file count and bytes read per file.
- Ignore binary content for text retrieval.
- Produce a deterministic file revision identifier for projection comparison.

Workspace search ranks path and content term matches and returns bounded snippets with source revision.

## Vector boundary

A vector record contains namespace, ID, finite dimensions and source. The store contract supports:

- upsert;
- namespace-scoped similarity search;
- namespace deletion.

The in-memory implementation proves conformance, namespace isolation and deletion. Vector records never replace source memory or files.

## Mandatory tests

- active memory is searchable;
- superseded and deleted memory disappears;
- memory mutation and outbox event are atomic;
- required context is included before optional context;
- omitted context records token-budget reason;
- required context overflow fails explicitly;
- workspace search cannot escape root;
- symlink and binary behavior is bounded;
- vector namespaces are isolated and deletable.
