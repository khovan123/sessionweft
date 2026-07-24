# Unified agent `/resume` shell

## Goal

Expose one Session selector across every one-shot SessionWeft adapter without copying provider-owned conversations between tools.

The Runtime-owned Session remains canonical. Codex, Claude, Gemini, Antigravity IDE, `fcc-claude` and `fcc-codex` all receive the same selected Session ID, history and reconstructed context.

## Start

Run the normal SessionWeft Runtime first:

```bash
cargo run -p sessionweftd --bin sessionweftd
```

Start the managed agent shell with any supported adapter:

```bash
cargo run -p sessionweft --bin sessionweft-agent-shell -- codex --cwd .
```

Start from an existing Session:

```bash
cargo run -p sessionweft --bin sessionweft-agent-shell -- claude \
  --session <SESSION_ID> \
  --cwd .
```

## Slash commands

```text
/resume                 list every durable Session
/resume <SESSION_ID>    select an existing Session
/agent                  list supported adapters
/agent <AGENT>          switch adapter without changing Session
/new [TITLE]            create and select a new Session
/session                display the current Session
/history                display shared cross-agent history
/context                display reconstructed shared context
/help                   display commands
/quit                   exit
```

Any non-slash input is delegated to `sessionweft-agent resume`, using the currently selected Session and adapter.

Example:

```text
[8f6c021a | codex] > inspect the repository
[8f6c021a | codex] > /agent claude
[8f6c021a | claude] > review the Codex result
[8f6c021a | claude] > /resume
[8f6c021a | claude] > /resume 3ec8be50-...
[3ec8be50 | claude] > continue this Session
```

## Adapter coverage

- `codex`
- `claude`
- `gemini`
- `antigravity`
- `fcc-claude`
- `fcc-codex`

Antigravity remains a launch/context bridge: SessionWeft opens the IDE and materializes the selected Session context. Native Antigravity chat interception still requires an IDE extension or supported protocol, but `/resume` and Session selection remain available in the managed shell.

## Boundary

This feature intentionally implements `/resume` in the SessionWeft-managed shell rather than modifying private provider conversation stores. A Session exists once in the Runtime and is referenced by every adapter. Provider-native sessions are not treated as canonical state.
