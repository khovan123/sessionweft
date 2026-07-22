# Scheduler Approval and Lock Prerequisites

Status: implementation and verification slice for issue #30.

Approval nodes remain controlled by the Workflow state machine. A task depending on an approval node cannot become `ready` until that approval is persisted as granted.

A task may have a persisted lock requirement keyed by Workflow and node. Before guarded claim or handover, the scheduler must find an unexpired persisted lease that:

- belongs to the same Session;
- is owned by the selected Agent ID;
- overlaps the required resource;
- satisfies the required lock mode.

When eligible, the lease ID, resource, mode, fencing token and expiry are persisted in a fence snapshot keyed by Claim ID. When no eligible lease exists, the scheduler performs no state transition and emits no claim event.

The polling worker only calls guarded claim and guarded handover transitions. Provider and Tool execution must load and validate the persisted fence snapshot again immediately before any external or workspace side effect.
