# RFC: Runtime-Owned Agent Workflow Execution

## Status

Proposed and implemented on `feature/runtime-owned-agent-workflows`.

## Goal

Every agent started by a workflow is owned by SessionWeft Runtime. TUI and CLI are control clients only. They must not spawn Codex, Claude, Gemini, FCC, or Antigravity directly.

## Required execution path

```text
TUI / CLI
  -> SessionWeft Runtime workflow API
  -> Runtime execution supervisor
  -> Runtime-owned PTY
  -> native agent process
  -> terminal frames/events
  -> TUI attach view
```

## Invariants

1. Workflow nodes cannot launch agent commands from the client process.
2. Runtime creates and owns the child process and PTY lifecycle.
3. Native agent terminal rendering is preserved through PTY byte streaming.
4. Runtime records stdin, stdout, stderr, exit status, workflow node, agent, Session, and workspace ownership.
5. Skills and plugins are resolved before launch and materialized into a Runtime-owned execution context.
6. Agent processes receive only the resolved skill/plugin paths and policy-approved environment.
7. TUI input is sent to Runtime, not written to the child process directly.
8. Runtime enforces one active owner and fencing token for every workflow execution.

## Runtime API

### Start workflow agent execution

```http
POST /v1/sessions/{session_id}/workflows/{workflow_id}/nodes/{node_id}/executions
```

```json
{
  "expected_version": 12,
  "agent": "claude",
  "workspace_id": "workspace-1",
  "owner_id": "operator-1",
  "task": "Implement the selected workflow node",
  "skills": ["rust", "github"],
  "plugins": ["mcp:github", "mcp:filesystem"],
  "terminal": {
    "cols": 140,
    "rows": 40
  }
}
```

Response:

```json
{
  "execution_id": "uuid",
  "state": "starting",
  "fencing_token": 9,
  "attach_path": "/v1/executions/{execution_id}/terminal"
}
```

### Send terminal input

```http
POST /v1/executions/{execution_id}/input
```

```json
{
  "fencing_token": 9,
  "data": "base64-or-utf8 terminal input"
}
```

### Resize terminal

```http
POST /v1/executions/{execution_id}/resize
```

### Stop execution

```http
POST /v1/executions/{execution_id}/stop
```

### Read terminal frames

```http
GET /v1/executions/{execution_id}/terminal?after={cursor}&limit=200
```

The first implementation uses cursor-based HTTP polling. WebSocket/SSE transport can be added without changing ownership semantics.

## Execution context

Runtime materializes:

```text
.sessionweft/executions/{execution_id}/
  context.md
  skills/
  plugins/
  manifest.json
  terminal.log
```

The agent receives:

```text
SESSIONWEFT_EXECUTION_ID
SESSIONWEFT_SESSION_ID
SESSIONWEFT_WORKFLOW_ID
SESSIONWEFT_WORKFLOW_NODE_ID
SESSIONWEFT_CONTEXT_FILE
SESSIONWEFT_SKILLS_DIR
SESSIONWEFT_PLUGINS_DIR
SESSIONWEFT_RUNTIME_ENDPOINT
SESSIONWEFT_FENCING_TOKEN
```

## TUI behavior

The TUI gains:

- task input mode;
- agent selection;
- workflow execution start;
- active execution list;
- terminal attach panel;
- terminal input forwarding;
- resize forwarding;
- stop/retry controls;
- resolved skills/plugins panel.

The TUI never calls `Command::new` for an agent.

## Native terminal requirement

Runtime uses a PTY and forwards raw terminal bytes. ANSI escape sequences are retained so the attached terminal surface behaves like the native agent terminal. SessionWeft controls lifecycle, context, plugins, skills, ownership, retries, and workflow state while the agent retains its normal terminal UX.
