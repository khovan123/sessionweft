//! SessionWeft intentionally supports only SQLite local mode and PostgreSQL service mode.
//!
//! This workspace patch replaces SQLx's optional MySQL backend so Cargo does not lock
//! an unused vulnerable RSA dependency graph. The crate deliberately exposes no SQLx
//! MySQL API: accidentally enabling the backend fails compilation instead of silently
//! expanding the supported database and security scope.

#![forbid(unsafe_code)]

/// Marker used by release checks to prove that the unsupported backend is disabled.
pub const SESSIONWEFT_MYSQL_BACKEND_DISABLED: bool = true;
