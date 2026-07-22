# SQLx Dependency Hardening Rationale

## Decision

SessionWeft disables the SQLx `derive` and query-macro feature path. Service-mode rows are decoded through typed tuples and explicit domain mapping.

## Reason

The Runtime supports PostgreSQL and SQLite only. Enabling SQLx derive macros caused the resolved lockfile to include `sqlx-macros-core`, its MySQL driver dependency and the `rsa` crate. `rsa 0.9.10` is affected by `RUSTSEC-2023-0071`, which has no patched release.

SessionWeft did not use MySQL and only used `sqlx::FromRow` for two internal PostgreSQL result structs. Those rows now use tuple decoding provided by `sqlx-core`, preserving the SQL and public domain API without carrying an unused vulnerable driver path.

## Enforcement

- Workspace SQLx features are limited to Tokio/Rustls, SQLite, PostgreSQL, Chrono, UUID and JSON.
- `cargo audit` is a release-blocking hardening job.
- The dependency diagnostic captures `cargo tree -i rsa -e features` on an audit failure.
- RC approval requires a lockfile without known vulnerable packages; advisories are not ignored solely because a package is believed to be unused.
