# SessionWeft

**One session. Many agents. One runtime.**

SessionWeft is a session-first, provider-agnostic Runtime for coordinating AI agents over a shared workspace. The Runtime owns durable state; IDEs, CLIs, providers and agents act as clients or pluggable execution components.

## Status

SessionWeft `0.1.0` is approved for General Availability within the declared scope:

- SQLite local single-user Runtime mode;
- authenticated single-tenant service mode using PostgreSQL and NATS JetStream;
- durable Session, Workflow, Agent, Memory, Lock, Git, Provider, Tool and event state;
- CLI, Ratatui TUI and VS Code clients as stateless Runtime adapters;
- Linux production MCP/plugin sandbox using bubblewrap.

Release: [`v0.1.0`](../../releases/tag/v0.1.0)

Phase 3 work for multi-tenant SaaS, billing, portable plugin isolation and adapter certification is tracked separately and must pass a new exact-commit release gate before it can expand the GA scope.

## Requirements

- Rust 1.88 or newer
- A local filesystem for SQLite local mode
- PostgreSQL 17+ and NATS JetStream for service mode
- Ollama only when using the `ollama` provider
- Bubblewrap for native production MCP/plugin processes on Linux

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
| `SESSIONWEFT_DATABASE_URL` | `sqlite://sessionweft.db` | SQLite or PostgreSQL connection URL |
| `SESSIONWEFT_API_TOKEN` | unset | Bearer token; required for non-loopback bind |
| `SESSIONWEFT_OLLAMA_URL` | `http://127.0.0.1:11434` | Ollama-compatible endpoint |
| `SESSIONWEFT_ENDPOINT` | `http://127.0.0.1:7447` | CLI Runtime endpoint |
| `RUST_LOG` | `info` | Structured log filter |

Example authenticated service bind:

```bash
export SESSIONWEFT_BIND=0.0.0.0:7447
export SESSIONWEFT_API_TOKEN='replace-with-a-secret'
export SESSIONWEFT_DATABASE_URL='postgres://sessionweft:secret@db/sessionweft'
cargo run -p sessionweftd
```

Then provide the same token to the CLI through `SESSIONWEFT_API_TOKEN` or `--token`.

## Verify

```bash
cargo metadata --locked --format-version 1 --no-deps >/dev/null
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

## Architecture rules

- Session identity never comes from a provider.
- State and outbox records commit atomically.
- Durable delivery is treated as at least once.
- Provider, CLI and API adapters never access the database directly.
- Tool and plugin execution remain default-deny.
- Search, memory and vector indexes are rebuildable projections.
- Production release evidence is bound to the exact tested commit.

## Documentation

- [`PROJECT.md`](PROJECT.md): project source of truth and complete roadmap
- [`docs/00-product/current-status.md`](docs/00-product/current-status.md): current release scope and completed gates
- [`docs/09-release/general-availability-0.1.0.md`](docs/09-release/general-availability-0.1.0.md): GA decision
- [`docs/02-architecture/baseline-v1.md`](docs/02-architecture/baseline-v1.md): architecture baseline
- [`docs/04-adr`](docs/04-adr): accepted decisions
- [`docs/03-rfc`](docs/03-rfc): implementation contracts
- [`CONTRIBUTING.md`](CONTRIBUTING.md): automatically generated contributor avatars
