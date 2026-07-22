# Session-Scoped Lock Security

Status: implementation in progress for security blocker #41.

Before lock lifecycle is exposed through HTTP, CLI or IDE clients, Runtime must resolve the persisted lease and authorize all reads and mutations against:

- Session ID;
- workspace ID;
- lock ID;
- owner ID;
- current fencing token;
- lease expiry.

Two Sessions may share a workspace name, but neither Session may list, heartbeat or release the other Session's lease.
