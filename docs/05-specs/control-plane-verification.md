# Runtime Control Plane Verification

Status: implementation verification in progress.

The first Control Plane vertical slice must satisfy the following gate before merge:

- the workspace dependency lock includes `sessionweft-control-plane`;
- Rust formatting and Clippy pass on Rust 1.88;
- all existing workspace tests remain green;
- integration tests prove Session-scoped Agent, Workflow, Lock and Memory operations;
- dependent resources cannot be created for a missing Session;
- HTTP, CLI and IDE adapters remain outside the repository layer.

This document records verification evidence only. It does not declare issue #29 complete.
