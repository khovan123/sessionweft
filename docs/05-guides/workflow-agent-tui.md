# Workflow Agent TUI

`sessionweft-workflow` is a dedicated Ratatui client for selecting workflow nodes, configuring agent presets, starting Runtime-owned executions, and attaching to their native terminals.

## Runtime ownership

The TUI never starts Codex, Claude, Gemini, FCC, or Antigravity directly. It sends the selected task and agent configuration to the SessionWeft Runtime workflow execution API. The Runtime owns the process, PTY, terminal frames, fencing token, workflow state, skills, and plugins.

## Build

```bash
cargo build -p sessionweft-tui --bin sessionweft-workflow
```

## First run

Provide the Session and Workflow once:

```bash
./target/debug/sessionweft-workflow \
  --session-id <SESSION_ID> \
  --workflow-id <WORKFLOW_ID>
```

The IDs and agent presets are persisted to:

```text
.sessionweft/workflow-agents.json
```

Subsequent runs only require:

```bash
./target/debug/sessionweft-workflow
```

The IDs can also be provided through `SESSIONWEFT_SESSION_ID` and `SESSIONWEFT_WORKFLOW_ID`.

## Workflow Agents tab

The first tab shows:

- supported Runtime-owned agents;
- workflow nodes in definition order;
- each node's current status;
- selected workspace and owner;
- resolved skills and plugins;
- active execution status.

Controls:

```text
Up/Down or j/k   Select agent
Left/Right       Select workflow node
Enter or t       Open task editor
c or 2           Open Agent Config
r                Refresh Runtime data
o                Attach active terminal
x                Stop active execution
q                Quit
```

## Agent Config tab

Configuration is stored independently for each agent:

- `workspace_id`;
- `owner_id`;
- comma-separated skills;
- comma-separated plugins.

Controls:

```text
Left/Right or h/l   Select agent preset
Up/Down or j/k      Select field
Enter               Edit selected field
Enter or Esc        Finish editing field
Tab                 Move to next field
s                   Validate and save config
Esc or 1            Return to Workflow Agents
```

Example config:

```json
{
  "schema_version": 1,
  "session_id": "<SESSION_ID>",
  "workflow_id": "<WORKFLOW_ID>",
  "selected_agent": "claude",
  "agents": {
    "claude": {
      "workspace_id": "default",
      "owner_id": "operator",
      "skills": ["rust", "github"],
      "plugins": ["mcp:github", "mcp:filesystem"]
    }
  }
}
```

Missing supported agents are filled with safe defaults when the file is loaded.

## Task and terminal

The task editor displays the resolved agent, workflow node, workspace, owner, skills, and plugins before execution begins.

```text
Enter              Start through Runtime
Tab                Select next agent
Ctrl-Left/Right    Select workflow node
Esc                Return to Workflow Agents
```

After the Runtime accepts the execution, the TUI attaches to the Runtime-owned terminal. Native terminal input is forwarded to the Runtime. Use `Esc` to return to the dashboard or `Ctrl-X` to stop the execution.
