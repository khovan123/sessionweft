# Standalone agent sessions

## Status

Initial implementation for local development.

## Goal

Allow a user to run coding agents without creating a Workflow. A durable Session remains the source of truth for chat history and context, so the user can stop, resume, or switch agents without losing prior decisions.

## Supported adapters

- Codex CLI
- Claude Code CLI
- Gemini CLI
- Antigravity IDE launcher

Codex, Claude and Gemini are terminal adapters whose stdout is appended to the Session as an assistant message. Antigravity is a GUI launcher: Session context is materialized under `.sessionweft/contexts/<session-id>.md`, and the context path is passed through `SESSIONWEFT_SESSION_CONTEXT`.

## Commands

Run the Runtime first:

```bash
cargo run -p sessionweftd --bin sessionweftd
```

Create a Session:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- new "Repository investigation"
```

List resumable Sessions:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- sessions
```

Start a new Codex-backed Session:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- run codex \
  "Inspect the repository and identify the next implementation task" \
  --title "Repository investigation" \
  --cwd .
```

Resume the same Session with Claude:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- run claude \
  "Continue from the prior analysis and review the proposed change" \
  --session <SESSION_ID> \
  --cwd .
```

Switch to Gemini while retaining the same history:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- run gemini \
  "Check the implementation for missing tests" \
  --session <SESSION_ID> \
  --cwd .
```

Open Antigravity on that Session:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- run antigravity \
  "Open this repository with the current shared context" \
  --session <SESSION_ID> \
  --cwd .
```

Show history or inspect the generated context:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- history <SESSION_ID>
cargo run -p sessionweft --bin sessionweft-agent -- context <SESSION_ID>
```

## Adapter commands

Defaults:

| Adapter | Executable | Invocation |
|---|---|---|
| Codex | `codex` | `codex exec --skip-git-repo-check -` |
| Claude | `claude` | `claude -p <context>` |
| Gemini | `gemini` | `gemini -p <context>` |
| Antigravity | `antigravity-ide` | `antigravity-ide <workspace>` |

Executable paths may be overridden with:

- `SESSIONWEFT_CODEX_BIN`
- `SESSIONWEFT_CLAUDE_BIN`
- `SESSIONWEFT_GEMINI_BIN`
- `SESSIONWEFT_ANTIGRAVITY_BIN`

## Persistence and switching

Every invocation performs this transaction sequence:

1. Create or load a Session.
2. Append the tagged user prompt.
3. Reload the latest Session state.
4. Build context from the most recent Session messages.
5. Run the selected adapter independently of Workflow state.
6. Append the tagged assistant response.

Messages include an `[agent:<name>]` prefix so provenance remains visible when several agents share one Session.

## Current boundary

This feature intentionally does not map external CLI processes to the existing `AgentRecord` lifecycle. It is an independent interactive runner. Workflow scheduling, heartbeat ownership and task assignment remain separate control-plane features.
