# Free Claude Code adapters

## Status

Initial local integration for the shared Session agent client.

## Supported launchers

SessionWeft supports the Free Claude Code launchers that currently exist:

- `fcc-claude`
- `fcc-codex`

They are exposed as the agent values `fcc-claude` and `fcc-codex`. They remain separate from the native `claude` and `codex` adapters, so users may switch between native and FCC-backed launchers while keeping the same Session ID, history and reconstructed context.

## Requirements

Start the Free Claude Code proxy before using either adapter:

```bash
fcc-server
```

Confirm the launchers are installed:

```bash
command -v fcc-claude
command -v fcc-codex
```

## Usage

Start a new Session through FCC Claude:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- run fcc-claude \
  "Inspect this repository" \
  --title "FCC repository review" \
  --cwd .
```

Resume the same Session through FCC Codex:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- resume fcc-codex \
  "Continue from the shared history and implement the next change" \
  --session <SESSION_ID> \
  --cwd .
```

A native adapter may continue the same Session afterward:

```bash
cargo run -p sessionweft --bin sessionweft-agent -- resume claude \
  "Review the FCC Codex result" \
  --session <SESSION_ID> \
  --cwd .
```

## Executable overrides

| Variable | Default |
|---|---|
| `SESSIONWEFT_FCC_CLAUDE_BIN` | `fcc-claude` |
| `SESSIONWEFT_FCC_CODEX_BIN` | `fcc-codex` |

## Invocation contract

- FCC Claude: `fcc-claude -p <shared-context>`
- FCC Codex: `fcc-codex exec --skip-git-repo-check -`, with shared context written to stdin

Both adapters append provenance tags to the durable Session:

- user request: `[to-agent:fcc-claude]` or `[to-agent:fcc-codex]`
- response: `[agent:fcc-claude]` or `[agent:fcc-codex]`

Free Claude Code currently provides launcher commands for Claude Code and Codex. SessionWeft does not advertise an `fcc-gemini` adapter because that launcher is not currently part of the upstream CLI contract.
