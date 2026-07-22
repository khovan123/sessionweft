//! SessionWeft supports SQLite local mode and PostgreSQL service mode only.
//!
//! This workspace patch replaces SQLx's optional MySQL backend so Cargo does not
//! lock an unused vulnerable RSA dependency graph. It intentionally exposes no
//! MySQL API: accidentally enabling the backend fails compilation.

#![forbid(unsafe_code)]

/// Release-check marker proving that the unsupported backend is disabled.
pub const SESSIONWEFT_MYSQL_BACKEND_DISABLED: bool = true;
