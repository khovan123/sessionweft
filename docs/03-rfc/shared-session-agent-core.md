# Shared Session agent core

## Status

Initial implementation for local development.

## Product core

SessionWeft is not a Workflow-first agent launcher. Its primary product contract is:

```text
One durable Session
    owns history, context and decisions
Many interchangeable agent adapters
    read from and append to that same Session
Workflow
    is an optional orchestration layer above the Session
```

The Session remains the source of truth when the active agent changes. Codex, Claude, Gemini and Antigravity do not own durable conversation state. They are execution adapters attached to a Session for one turn or one interactive period.

## Required behavior

A conforming agent adapter must:

1. Create or select a durable Session.
2. Load the latest committed Session version.
3. Read shared history and reconstruct context from the Session.
4. Execute the selected agent without requiring a Workflow.
5. Append the user request and agent result back to the same Session.
6. Preserve provenance so later agents can identify prior work.
7. Resume after Runtime or client restart by selecting the same Session ID.
8. Allow another agent adapter to continue without copying or migrating history.

The adapter must never maintain a private canonical chat history that can diverge from the Runtime.

## Supported adapters

The initial adapter set is:

- Codex CLI
- Claude Code CLI
- Gemini CLI
- Antigravity IDE launcher

Codex, Claude and Gemini are terminal adapters whose output is appended to the Session as an assistant message. Antigravity is a GUI adapter: Session context is materialized under `.sessionweft/contexts/<session-id>.md`, and the context path is passed through `SESSIONWEFT_SESSION_CONTEXT`.

## Shared Session lifecycle

```text
Create/select Session
        ↓
Append latest user request
        ↓
Reload exact committed Session
        ↓
Build context from shared history
        ↓
Run selected agent adapter
        ↓
Append result to the same Session
        ↓
Select the same Session later with any adapter
```

Switching adapters does not create a new Session unless the user explicitly requests one.

## Commands

Run the Runtime:

```bash
cargo run -p sessionweftd --bin sessionweftd
```

Create a Session:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- new "Repository investigation"
```

List Sessions that can be resumed:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- sessions
```

Start with Codex:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- run codex \
  "Inspect the repository and identify the next implementation task" \
  --title "Repository investigation" \
  --cwd .
```

Continue the same Session with Claude:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- resume claude \
  "Continue from the prior analysis and review the proposed change" \
  --session <SESSION_ID> \
  --cwd .
```

Continue with Gemini:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- resume gemini \
  "Check the implementation for missing tests" \
  --session <SESSION_ID> \
  --cwd .
```

Open Antigravity with the same context:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- resume antigravity \
  "Open this repository with the current shared context" \
  --session <SESSION_ID> \
  --cwd .
```

Inspect durable history and reconstructed context:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- history <SESSION_ID>
cargo run -p sessionweft --bin sessionweft-agent -- context <SESSION_ID>
```

## Adapter commands

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

## Provenance

Until structured message provenance is promoted into the stable Session schema, the initial implementation stores visible markers:

- `[to-agent:<name>]` on the user request sent to an adapter
- `[agent:<name>]` on the adapter response

These markers are part of the shared history and are available to every later adapter.

## Boundary

Workflow scheduling, heartbeat ownership and task assignment remain optional control-plane capabilities. They may orchestrate a shared Session, but they do not own its chat history or context.
