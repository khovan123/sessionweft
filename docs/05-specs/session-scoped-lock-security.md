# Session-Scoped Lock Security

Status: implementation verification in progress for security blocker #41.

Before lock lifecycle is exposed through HTTP, CLI or IDE clients, Runtime must resolve the persisted lease and authorize all reads and mutations against:

- Session ID;
- workspace ID;
- lock ID;
- owner ID;
- current fencing token;
- lease expiry.

Two Sessions may share a workspace name, but neither Session may list, heartbeat or release the other Session's lease.

The implementation gate requires cross-Session denial tests and a final read-only CI run.
