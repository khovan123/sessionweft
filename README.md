# SessionWeft

**One session. Many agents. One runtime.**

SessionWeft is a session-first, provider-agnostic Runtime for coordinating AI agents over a shared workspace. The Runtime owns durable state; IDEs, CLIs, providers and agents act as clients or pluggable execution components.

## Core product contract

The durable `Session` is the product core:

- every agent reads the same Session history and context;
- switching Codex, Claude, Gemini or an IDE adapter does not create a new conversation;
- selecting the same Session ID resumes after a client or Runtime restart;
- agent adapters never own canonical persistent history;
- Workflow is optional orchestration above a Session, not a prerequisite for using agents.

```text
Session owns history, context and decisions
    ├── Codex adapter
    ├── Claude adapter
    ├── Gemini adapter
    ├── Free Claude Code adapters
    └── Antigravity IDE adapter
```

## Status

SessionWeft `0.2.0` is approved for General Availability on the exact commit that passes every release gate. The GA scope includes:

- SQLite local single-user Runtime mode;
- authenticated single-tenant service mode using PostgreSQL and NATS JetStream;
- multi-tenant SaaS Runtime with isolated PostgreSQL schemas, tenant identity, membership, quota and token authority;
- tenant-scoped Session, Agent, Workflow and Lock APIs with cross-tenant not-found semantics;
- billing plans, subscriptions, entitlements, usage records and an idempotent Stripe Billing adapter;
- durable Session, Workflow, Agent, Memory, Lock, Git, Provider, Tool and event state;
- CLI, Ratatui TUI and VS Code clients as stateless Runtime adapters;
- Linux native plugin isolation using bubblewrap and portable Wasmtime/WASI isolation on Linux, macOS and Windows;
- exact-commit certification and fail-closed activation for production provider, plugin, deployment and billing adapters.

Release: [`v0.2.0`](../../releases/tag/v0.2.0)  
Previous release: [`v0.1.0`](../../releases/tag/v0.1.0)

The `v0.2.0` tag is created only after CI, security, production hardening, SaaS Runtime, Phase 3 qualification and GA approval all pass for the same main-branch commit. Publication verification then checks checksums, SBOM, exact-commit evidence and packaged adapter certifications.

## Requirements

- Rust 1.88 or newer
- A local filesystem for SQLite local mode
- PostgreSQL 17+ and NATS JetStream for service mode
- Ollama only when using the `ollama` provider
- Bubblewrap for native production MCP/plugin processes on Linux
- Codex CLI, Claude Code, Gemini CLI or Antigravity IDE only when using persistent standalone agent processes

## Run locally

```bash
cargo run -p sessionweftd --bin sessionweftd
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

## Share one Session across one-shot agents

Start a new shared Session with Codex:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- run codex \
  "Inspect this repository" \
  --title "Repository review" \
  --cwd .
```

Reuse the returned Session ID with Claude:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- resume claude \
  "Continue from Codex and review its conclusions" \
  --session <SESSION_ID> \
  --cwd .
```

Continue the same history with Gemini or a Free Claude Code launcher:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- resume gemini \
  "Find missing tests in the previous proposal" \
  --session <SESSION_ID> \
  --cwd .

cargo run -p sessionweft --bin sessionweft-agent -- resume fcc-claude \
  "Review the shared history through Free Claude Code" \
  --session <SESSION_ID> \
  --cwd .
```

Inspect or resume the durable Session later:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- sessions
cargo run -p sessionweft --bin sessionweft-agent -- history <SESSION_ID>
cargo run -p sessionweft --bin sessionweft-agent -- context <SESSION_ID>
```

## Persistent standalone agent sessions

`sessionweft-agentd` keeps Codex CLI, Claude Code, Gemini CLI and Antigravity IDE processes attached to the same durable Session without creating a Workflow. PTY output is captured into attributed history, and context is materialized under `.sessionweft/sessions/<SESSION_ID>/`.

The persistent daemon is additive. The existing `sessionweft-agent` binary remains available for one-shot native and Free Claude Code adapters. Use `sessionweft-agentctl` to control persistent processes.

Start the standalone daemon from the workspace that agents may access:

```bash
SESSIONWEFT_DATABASE_URL='sqlite://sessionweft.db' \
SESSIONWEFT_WORKSPACE_ROOT="$PWD" \
cargo run -p sessionweftd --bin sessionweft-agentd
```

The daemon binds to `127.0.0.1:7449`. In another terminal, create or select a Session:

```bash
cargo run -p sessionweft --bin sessionweft-agentctl -- sessions
cargo run -p sessionweft --bin sessionweft-agentctl -- create "Shared coding session"
```

Start Codex in that Session and send work:

```bash
cargo run -p sessionweft --bin sessionweft-agentctl -- \
  start <SESSION_ID> codex --cwd .

cargo run -p sessionweft --bin sessionweft-agentctl -- \
  send <SESSION_ID> "Inspect this repository and propose the next change"
```

Switch to Claude or Gemini without replacing the Session or stopping the previous process:

```bash
cargo run -p sessionweft --bin sessionweft-agentctl -- switch <SESSION_ID> claude
cargo run -p sessionweft --bin sessionweft-agentctl -- send <SESSION_ID> "Review Codex's approach"

cargo run -p sessionweft --bin sessionweft-agentctl -- switch <SESSION_ID> gemini
```

After restarting `sessionweft-agentd`, resume a persistent agent from durable Session context:

```bash
cargo run -p sessionweft --bin sessionweft-agentctl -- resume <SESSION_ID> claude
```

Inspect or stop persistent agent state:

```bash
cargo run -p sessionweft --bin sessionweft-agentctl -- status <SESSION_ID>
cargo run -p sessionweft --bin sessionweft-agentctl -- history <SESSION_ID>
cargo run -p sessionweft --bin sessionweft-agentctl -- context <SESSION_ID>
cargo run -p sessionweft --bin sessionweft-agentctl -- stop <SESSION_ID> claude
```

Antigravity IDE is supported in launch/context-bridge mode. SessionWeft launches `antigravity-ide` in the selected workspace and writes durable context plus the latest prompt under `.sessionweft/sessions/<SESSION_ID>/`; direct IDE chat injection requires a future Antigravity extension or protocol bridge.

## Configuration

| Variable | Default | Purpose |
|---|---|---|
| `SESSIONWEFT_BIND` | `127.0.0.1:7447` | Runtime listen address |
| `SESSIONWEFT_DATABASE_URL` | `sqlite://sessionweft.db` | SQLite or PostgreSQL connection URL |
| `SESSIONWEFT_API_TOKEN` | unset | Bearer token; required for non-loopback bind |
| `SESSIONWEFT_OLLAMA_URL` | `http://127.0.0.1:11434` | Ollama-compatible endpoint |
| `SESSIONWEFT_ENDPOINT` | `http://127.0.0.1:7447` | CLI Runtime endpoint |
| `SESSIONWEFT_AGENT_BIND` | `127.0.0.1:7449` | Persistent standalone agent daemon listen address |
| `SESSIONWEFT_AGENT_ENDPOINT` | `http://127.0.0.1:7449` | Persistent agent control CLI endpoint |
| `SESSIONWEFT_AGENT_API_TOKEN` | unset | Persistent daemon bearer token; required for non-loopback bind |
| `SESSIONWEFT_STANDALONE_AGENT_PROGRAMS` | `sh,bash,pwsh,cmd.exe,codex,claude,gemini,antigravity-ide` | Executables discoverable by the persistent daemon |
| `SESSIONWEFT_WORKSPACE_ROOT` | current directory | Filesystem boundary and shared context root |
| `SESSIONWEFT_SAAS_DATABASE_URL` | unset | PostgreSQL authority database for `sessionweft-saasd` |
| `SESSIONWEFT_SAAS_BOOTSTRAP_TOKEN` | unset | Bootstrap authority token for creating the first tenant |
| `SESSIONWEFT_REQUIRE_CERTIFIED_ADAPTERS` | release-dependent | Require a packaged exact-commit adapter certification set |
| `SESSIONWEFT_CODEX_BIN` | `codex` | Codex CLI executable for one-shot mode |
| `SESSIONWEFT_CLAUDE_BIN` | `claude` | Claude Code executable for one-shot mode |
| `SESSIONWEFT_GEMINI_BIN` | `gemini` | Gemini CLI executable for one-shot mode |
| `SESSIONWEFT_ANTIGRAVITY_BIN` | `antigravity-ide` | Antigravity IDE executable for one-shot mode |
| `SESSIONWEFT_FCC_CLAUDE_BIN` | `fcc-claude` | Free Claude Code Claude launcher |
| `SESSIONWEFT_FCC_CODEX_BIN` | `fcc-codex` | Free Claude Code Codex launcher |
| `RUST_LOG` | `info` | Structured log filter |

Example authenticated service bind:

```bash
export SESSIONWEFT_BIND=0.0.0.0:7447
export SESSIONWEFT_API_TOKEN='replace-with-a-secret'
export SESSIONWEFT_DATABASE_URL='postgres://sessionweft:secret@db/sessionweft'
cargo run -p sessionweftd --bin sessionweftd
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

- Session identity never comes from a provider or agent adapter.
- Session history and context survive agent switching and client restart.
- Agents never own canonical persistent state.
- Workflow is optional and cannot replace Session ownership.
- Persistent process bindings are operational state; canonical history remains in the Runtime-owned Session.
- State and outbox records commit atomically.
- Durable delivery is treated as at least once.
- Provider, CLI and API adapters never access the database directly.
- Tenant authority is resolved before a tenant Runtime is selected.
- Tool and plugin execution remain default-deny.
- Search, memory and vector indexes are rebuildable projections.
- Production adapters cannot activate without an exact matching certification.
- Production release evidence is bound to the exact tested commit.

## Documentation

- [`PROJECT.md`](PROJECT.md): project source of truth and complete roadmap
- [`docs/03-rfc/shared-session-agent-core.md`](docs/03-rfc/shared-session-agent-core.md): shared Session and interchangeable agent contract
- [`docs/03-rfc/free-claude-code-adapters.md`](docs/03-rfc/free-claude-code-adapters.md): Free Claude Code launcher integration
- [`docs/00-product/current-status.md`](docs/00-product/current-status.md): current release scope and completed gates
- [`docs/09-release/general-availability-0.2.0.md`](docs/09-release/general-availability-0.2.0.md): 0.2.0 GA decision
- [`docs/09-release/general-availability.md`](docs/09-release/general-availability.md): 0.1.0 GA decision
- [`docs/02-architecture/baseline-v1.md`](docs/02-architecture/baseline-v1.md): architecture baseline
- [`docs/04-adr`](docs/04-adr): accepted decisions
- [`docs/03-rfc`](docs/03-rfc): implementation contracts
- [`CONTRIBUTING.md`](CONTRIBUTING.md): automatically generated contributor avatars
