# Portable Production Plugin Sandbox

Status: implementation baseline for SessionWeft 0.2.0.

## Decision

SessionWeft supports two production plugin execution forms:

1. native MCP stdio processes isolated with bubblewrap on Linux;
2. import-free WebAssembly modules executed by Wasmtime on Linux, macOS and Windows.

Native processes outside Linux are not considered production-isolated. Cross-platform production plugins must use the WebAssembly ABI unless a future operating-system adapter passes the same certification gate.

## WebAssembly boundary

The portable runtime deliberately does not link WASI or arbitrary host imports. This means plugins cannot access files, sockets, clocks, random devices, processes, environment variables or secrets through a host API. Any module declaring an import is rejected before instantiation.

The ABI is:

```text
memory: exported linear memory
sessionweft_alloc(length: i32) -> pointer: i32
sessionweft_invoke_v1(pointer: i32, length: i32) -> packed(pointer: u32, length: u32)
sessionweft_dealloc(pointer: i32, length: i32) -> ()  # optional
```

Input and output are opaque bytes. Higher-level plugin protocols must use a versioned envelope inside those bytes.

## Enforcement

Every invocation verifies:

- manifest identifiers and limits;
- SHA-256 of the exact Wasm module;
- no module imports;
- one memory, one table and one instance maximum;
- bounded linear memory;
- bounded input and output;
- deterministic fuel;
- epoch interruption for wall-clock timeout;
- bounded native Wasm stack.

A fuel or epoch trap terminates the invocation and is not blindly retried when the external side-effect status is uncertain.

## Platform scope

Wasmtime core executes the same import-free ABI on supported Linux, macOS and Windows hosts. CI executes the sandbox tests on all three GitHub-hosted operating systems. Because no WASI host functions are linked, host filesystem and network differences are outside the plugin capability boundary.

## Security update policy

The runtime uses Wasmtime core without `wasmtime-wasi`. This avoids granting WASI capabilities and avoids relying on path permission behavior for isolation. Wasmtime remains subject to RustSec, source/license policy, SBOM and adapter certification checks. A security advisory affecting the configured Wasmtime version blocks release.

## Release blockers

- any accepted import not explicitly represented by a future capability contract;
- module hash mismatch;
- successful memory growth beyond the declared limit;
- infinite execution that escapes fuel and epoch interruption;
- output allocation before output-length validation;
- platform-specific test failure;
- production activation without exact-commit adapter certification.
