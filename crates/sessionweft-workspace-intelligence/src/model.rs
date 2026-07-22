use std::{collections::BTreeMap, path::PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const WORKSPACE_GRAPH_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceLanguage {
    Rust,
    TypeScript,
    JavaScript,
    Python,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    File,
    Module,
    Type,
    Function,
    Method,
    Import,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SymbolId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRange {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolRecord {
    pub id: SymbolId,
    pub workspace_id: String,
    pub relative_path: String,
    pub file_revision: String,
    pub language: SourceLanguage,
    pub kind: SymbolKind,
    pub name: String,
    pub qualified_name: String,
    pub range: SourceRange,
    pub parent: Option<SymbolId>,
    pub signature: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyKind {
    Contains,
    Imports,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyEdge {
    pub from: SymbolId,
    pub to: SymbolId,
    pub kind: DependencyKind,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedFile {
    pub relative_path: String,
    pub revision: String,
    pub language: SourceLanguage,
    pub size_bytes: u64,
    pub modified_at: Option<DateTime<Utc>>,
    pub content: String,
    pub symbols: Vec<SymbolRecord>,
    pub imports: Vec<String>,
}

impl IndexedFile {
    pub(crate) fn file_symbol(&self) -> Option<&SymbolRecord> {
        self.symbols
            .iter()
            .find(|symbol| symbol.kind == SymbolKind::File)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceIntelligenceConfig {
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_symbols_per_file: usize,
}

impl Default for WorkspaceIntelligenceConfig {
    fn default() -> Self {
        Self {
            max_files: 50_000,
            max_file_bytes: 2 * 1024 * 1024,
            max_symbols_per_file: 20_000,
        }
    }
}

impl WorkspaceIntelligenceConfig {
    pub(crate) fn validate(self) -> Result<(), WorkspaceIntelligenceError> {
        if self.max_files == 0 || self.max_file_bytes == 0 || self.max_symbols_per_file == 0 {
            return Err(WorkspaceIntelligenceError::Validation(
                "workspace intelligence limits must be greater than zero".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexUpdateReport {
    pub changed_files: Vec<String>,
    pub reindexed_files: Vec<String>,
    pub affected_files: Vec<String>,
    pub removed_files: Vec<String>,
    pub workspace_revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceContextItem {
    pub symbol_id: SymbolId,
    pub relative_path: String,
    pub source_revision: String,
    pub workspace_revision: String,
    pub range: SourceRange,
    pub estimated_tokens: usize,
    pub inclusion_reason: String,
    pub score: f32,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkspaceSnapshot {
    pub schema_version: u32,
    pub workspace_id: String,
    pub root: String,
    pub workspace_revision: String,
    pub indexed_at: DateTime<Utc>,
    pub files: BTreeMap<String, IndexedFile>,
}

#[derive(Debug, Error)]
pub enum WorkspaceIntelligenceError {
    #[error("workspace intelligence validation failed: {0}")]
    Validation(String),
    #[error("path escapes canonical workspace root: {0}")]
    PathEscapesWorkspace(PathBuf),
    #[error("unsupported source language: {0}")]
    UnsupportedLanguage(PathBuf),
    #[error("workspace parser error: {0}")]
    Parser(String),
    #[error("workspace file limit exceeded: {0}")]
    FileLimitExceeded(usize),
    #[error("workspace file {path} is {size} bytes, limit is {limit}")]
    FileTooLarge {
        path: PathBuf,
        size: u64,
        limit: u64,
    },
    #[error("symbol limit {limit} exceeded while indexing {path}")]
    SymbolLimitExceeded { path: String, limit: usize },
    #[error("unsupported workspace graph schema version {0}")]
    UnsupportedSchema(u32),
    #[error("workspace snapshot revision mismatch: expected {expected}, actual {actual}")]
    RevisionMismatch { expected: String, actual: String },
    #[error("workspace snapshot serialization error: {0}")]
    Serialization(String),
    #[error("workspace I/O error: {0}")]
    Io(std::io::Error),
}
