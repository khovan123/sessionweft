# Scheduler Approval and Lock Prerequisites

Status: implementation and verification slice for issue #30.

Approval nodes remain controlled by the Workflow state machine. A task depending on an approval node cannot become `ready` until that approval is persisted as granted.

A SchedulerPlan may require a lock for a task node. Before claim or handover, the scheduler must find an unexpired persisted lease that:

- belongs to the same Session;
- is owned by the selected Agent ID;
- overlaps the required resource;
- satisfies the required lock mode.

When eligible, the lease ID, resource, mode, fencing token and expiry are snapshotted into the TaskClaim. When no eligible lease exists, the scheduler performs no state transition and emits no claim event.

Provider and Tool execution must validate the snapshotted fence again immediately before any external or workspace side effect.
