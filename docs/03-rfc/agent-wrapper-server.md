# Agent wrapper server

## Goal

Run SessionWeft in front of supported agent executables while preserving each tool's real terminal or IDE surface.

## Launch model

```text
sessionweft-agent-server
    ├── sw-codex        -> codex PTY
    ├── sw-claude       -> claude PTY
    ├── sw-gemini       -> gemini PTY
    ├── sw-fcc-claude   -> fcc-claude PTY
    ├── sw-fcc-codex    -> fcc-codex PTY
    └── sw-antigravity  -> antigravity-ide + context bridge
```

The wrapper owns only Session selection and PTY transport. The original agent process still renders its native terminal UI.

## Wrapper commands

The wrapper intercepts these lines before forwarding input to the underlying agent:

```text
/resume                 show all Runtime-owned Sessions
/resume <SESSION_ID>    select a Session and refresh shared context
/session                show the selected Session
/history                show cross-agent Session history
/context                show the materialized context path
```

All other bytes are forwarded to the real agent PTY unchanged.

## Canonical state

A Session exists once in SessionWeft Runtime. Wrapper launchers do not import or clone provider-native conversations. Every wrapper references the same Session ID and materialized history.

## Antigravity boundary

`sw-antigravity` launches the real IDE and writes the selected Session context under `.sessionweft/sessions/<SESSION_ID>/`. Direct interception of the proprietary IDE chat input is not assumed; Session selection remains controlled by the wrapper launcher and Runtime.
