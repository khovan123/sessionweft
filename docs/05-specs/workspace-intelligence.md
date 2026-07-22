# Workspace Intelligence Specification

Status: implementation baseline for issue #33.

## Ownership

Runtime owns workspace revisions, symbol records, dependency edges, persistence and retrieval decisions. Tree-sitter is a parser dependency only. A parser, LSP process or client cannot mutate Session state or return unbounded context directly to a provider.

## Supported baseline

The first production baseline indexes:

- Rust (`.rs`)
- TypeScript and TSX (`.ts`, `.tsx`)
- JavaScript and JSX (`.js`, `.jsx`, `.mjs`, `.cjs`)
- Python (`.py`, `.pyi`)

The index records files, modules, types, functions, methods and imports. LSP reference indexing remains a later adapter and is not required for this baseline.

## Identity and revision rules

A symbol ID is deterministic from workspace ID, canonical relative path, symbol kind and qualified name. It remains stable when an unchanged symbol is re-indexed. Every record also carries the exact file revision from which its source range was produced.

The workspace revision is derived from the ordered set of file paths and file revisions. Persisted snapshots are rejected when the stored revision does not match their content.

## Incremental indexing

The watcher compares current file revisions with the last indexed graph. An update parses:

1. changed or newly created files;
2. reverse import dependencies affected by those files;
3. no unrelated files.

Deleted files are removed before edges are rebuilt. Symlinks, parent traversal, absolute paths outside the canonical root, oversized files and unsupported source files are rejected.

## Dependency graph

Two baseline edge types are persisted:

- `contains`: file/type/module ownership of nested symbols;
- `imports`: source file to resolved target file.

Import resolution is intentionally conservative and language-neutral. Semantic reference/call edges may be added through a later LSP adapter after the tree-sitter baseline is stable.

## Retrieval

Hybrid ranking combines:

- exact and partial symbol-name matches;
- path matches;
- signature lexical matches;
- dependency degree;
- file recency.

Each returned context item includes:

- symbol ID;
- relative path;
- file and workspace revisions;
- one-based source range;
- estimated tokens;
- inclusion reasons;
- bounded source content.

## Persistence

The baseline supports an atomic JSON snapshot for local mode. The snapshot is schema-versioned, revision-verified and rooted at a canonical workspace directory. PostgreSQL persistence belongs to issue #34 and must preserve the same domain contract.

## Benchmarks

CI includes a generated 250-file repository test. The changed-file case verifies that modifying one imported module re-indexes only that module and its direct reverse dependency, not the entire repository. Larger capacity targets are finalized during production hardening in issue #36.
