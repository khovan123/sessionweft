# SessionWeft

**One session. Many agents. One runtime.**

SessionWeft is a session-first, provider-agnostic Runtime for coordinating AI agents over a shared workspace. The Runtime owns durable state; IDEs, CLIs, providers and agents act as clients or pluggable execution components.

## Status

The project is in **Phase 2: first Runtime vertical slice**. Phase -1, the Phase 0 decision baseline, Architecture baseline, ADRs, RFC-0001 and Production Specification v0 are complete for this constrained implementation.

The current code implements:

- versioned Session aggregate;
- optimistic concurrency;
- SQLite WAL persistence;
- transactional outbox;
- bounded local event delivery;
- provider registry;
- Echo and Ollama-compatible provider adapters;
- Runtime service;
- authenticated bootstrap HTTP API;
- scriptable CLI;
- structured logging and recovery-oriented tests.

Workflow, locks, Git collaboration, workspace indexing, memory, MCP, TUI and IDE remain separate implementation streams.

## Requirements

- Rust 1.88 or newer
- A local filesystem for the SQLite database
- Ollama only when using the `ollama` provider

## Run locally

```bash
cargo run -p sessionweftd
```

The Runtime binds to `127.0.0.1:7447` and creates `sessionweft.db` by default.

Check readiness:

```bash
cargo run -p sessionweft -- health
```

Create a Session:

```bash
cargo run -p sessionweft -- create "Demo session"
```

Select the deterministic Echo provider using the returned Session ID and version:

```bash
cargo run -p sessionweft -- provider <SESSION_ID> 0 echo test-model
```

Run a turn:

```bash
cargo run -p sessionweft -- run <SESSION_ID> 1 "Hello SessionWeft"
```

Every mutation requires the expected Session version. A stale version returns HTTP `409` instead of overwriting concurrent work.

## Configuration

| Variable | Default | Purpose |
|---|---|---|
| `SESSIONWEFT_BIND` | `127.0.0.1:7447` | Runtime listen address |
| `SESSIONWEFT_DATABASE_URL` | `sqlite://sessionweft.db` | SQLite connection URL |
| `SESSIONWEFT_API_TOKEN` | unset | Bearer token; required for non-loopback bind |
| `SESSIONWEFT_OLLAMA_URL` | `http://127.0.0.1:11434` | Ollama-compatible endpoint |
| `SESSIONWEFT_ENDPOINT` | `http://127.0.0.1:7447` | CLI Runtime endpoint |
| `RUST_LOG` | `info` | Structured log filter |

Example authenticated team-style bind:

```bash
export SESSIONWEFT_BIND=0.0.0.0:7447
export SESSIONWEFT_API_TOKEN='replace-with-a-secret'
cargo run -p sessionweftd
```

Then provide the same token to the CLI through `SESSIONWEFT_API_TOKEN` or `--token`.

## Test

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Architecture rules

- Session identity never comes from a provider.
- State and outbox records commit atomically.
- Durable delivery is treated as at least once.
- Provider, CLI and API adapters never access the database directly.
- Tool and plugin execution remain default-deny.
- Search, memory and vector indexes must be rebuildable projections.

## Documentation

- [`PROJECT.md`](PROJECT.md): project source of truth and complete roadmap
- [`docs/00-product/current-status.md`](docs/00-product/current-status.md): current gate and active scope
- [`docs/01-research/phase-0-synthesis.md`](docs/01-research/phase-0-synthesis.md): technology decisions
- [`docs/02-architecture/baseline-v1.md`](docs/02-architecture/baseline-v1.md): architecture baseline
- [`docs/04-adr`](docs/04-adr): accepted decisions
- [`docs/03-rfc/0001-runtime-vertical-slice.md`](docs/03-rfc/0001-runtime-vertical-slice.md): implementation contract
- [`docs/05-specs/production-spec-v0.md`](docs/05-specs/production-spec-v0.md): production constraints

## Contribution rule

Do not introduce a material dependency or architectural commitment without a linked research result and, where applicable, an approved ADR or RFC.
