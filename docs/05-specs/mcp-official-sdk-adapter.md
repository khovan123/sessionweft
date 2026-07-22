# Official MCP SDK Adapter Specification

Status: implementation slice for issue #32.

## Ownership boundary

The official `rmcp` SDK owns only MCP protocol framing and transport lifecycle. It does not own Session, Agent, approval, policy, task, memory or audit state.

SessionWeft remains authoritative for:

- Agent and Session scope validation;
- normalized tool namespace;
- permissions and risk classification;
- approval requirements;
- operation timeout and cancellation;
- rate limits and result-size limits;
- filesystem, environment and endpoint allowlists;
- durable approval consumption and audit records.

## Supported transports

### stdio

The adapter creates a new child process through `TokioChildProcess` for each bounded discovery or invocation operation.

The child process:

- uses a canonical executable path;
- runs inside the canonical workspace root;
- receives an empty environment plus explicit allowlisted variables;
- uses `kill_on_drop`;
- is cancelled when Runtime cancellation fires or the operation finishes.

This slice does not claim operating-system network isolation. Enforced filesystem/network sandbox profiles are implemented in the plugin-isolation slice before issue #32 can close.

### Streamable HTTP

The adapter uses `StreamableHttpClientTransport` and permits:

- HTTPS endpoints on an explicit host allowlist;
- plaintext HTTP only for explicitly allowed loopback endpoints;
- no embedded URL credentials.

## Compatibility checks

After MCP initialization the adapter verifies:

- the server negotiated tool capability;
- optional exact server implementation name;
- optional exact server version;
- optional protocol-version allowlist.

## Tool normalization

Remote tool names are normalized to `<server_id>.<remote_tool_name>`.

The adapter rejects:

- empty tool names;
- duplicate normalized names;
- more than 10,000 discovered tools;
- non-object input schemas;
- schemas declaring a non-object input type.

MCP tool annotations are not trusted for authorization. Risk and permissions come from Runtime configuration.

## Invocation controls

Each invocation has:

- Runtime cancellation;
- bounded timeout;
- fixed-window rate limiting;
- bounded JSON result size;
- normalized MCP server/protocol metadata;
- tool-level MCP errors converted into SessionWeft execution errors.

## Remaining work for issue #32

- durable one-time approval consumption and Outbox audit;
- enforced child-process filesystem/network isolation profiles;
- malicious plugin fixtures for hangs, output floods, path escape, secret access, schema spoofing and tool collision;
- optional WASM isolation benchmark document.
