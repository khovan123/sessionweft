# MCP WASM Isolation Benchmark Decision

Status: benchmark and adoption decision for issue #32.

## Question

Should SessionWeft make WebAssembly the mandatory first-release isolation boundary for MCP plugins?

## Compared paths

| Criterion | Isolated child process | WASM component/runtime |
|---|---|---|
| Existing MCP stdio compatibility | Direct | Requires wrapping or recompilation |
| Existing MCP Streamable HTTP compatibility | Direct | Requires host networking interface |
| Filesystem policy | Bubblewrap bind policy | Capability-oriented host imports |
| Network policy | Namespace deny/allow | Host networking imports |
| Process cancellation | Kill child/process group | Interrupt/fuel/epoch controls |
| Language/tool ecosystem reuse | High | Limited to WASM-compatible builds |
| Startup overhead | OS process startup | Runtime/module instantiation |
| Memory isolation | OS process boundary | Runtime linear-memory boundary |
| Host-call attack surface | Launcher/profile configuration | Custom host capability API |
| First-release integration risk | Lower | Higher |

## Benchmark conclusion

The first production adapter remains an isolated child process using:

- cleared environment;
- canonical executable and workspace paths;
- bubblewrap filesystem bindings;
- network namespace denied by default;
- bounded output, timeout and Runtime cancellation;
- durable policy, approval and audit outside the plugin process.

A mandatory WASM dependency is **deferred** because most reusable MCP servers currently expose stdio or Streamable HTTP rather than a WASM component contract. Adopting WASM now would require either recompiling third-party servers or introducing a custom host bridge whose permissions, networking and filesystem imports would become a second plugin API.

## Later adapter entry criteria

A WASM MCP adapter may be added after the following are available:

1. a versioned SessionWeft host capability interface;
2. component-model compatible MCP server packaging;
3. fuel or epoch-based cancellation benchmarks;
4. startup, throughput and memory benchmarks against the child-process adapter;
5. malicious-module tests for host-call escape, memory growth and output flooding;
6. compatibility evidence for at least two independently maintained MCP servers.

## Decision

- Child-process isolation: **adopt for first production release**.
- WASM isolation: **optional later adapter; not a first-release dependency**.
- This document is a design benchmark, not a claim that WASM is inherently weaker or stronger than an OS sandbox. The decision is based on ecosystem compatibility and implementation risk for the current release.
