use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sessionweft_core::{EventEnvelope, SessionId};
use thiserror::Error;
use uuid::Uuid;

pub const MEMORY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryClass {
    Conversation,
    Repository,
    Decision,
    Preference,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySource {
    pub kind: String,
    pub locator: String,
    pub revision: Option<String>,
}

impl MemorySource {
    pub fn validate(&self) -> Result<(), KnowledgeError> {
        if self.kind.trim().is_empty() {
            return Err(KnowledgeError::Validation(
                "memory source kind cannot be empty".into(),
            ));
        }
        if self.locator.trim().is_empty() {
            return Err(KnowledgeError::Validation(
                "memory source locator cannot be empty".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub schema_version: u32,
    pub id: Uuid,
    pub session_id: SessionId,
    pub class: MemoryClass,
    pub content: String,
    pub source: MemorySource,
    pub tags: BTreeSet<String>,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub supersedes: Option<Uuid>,
    pub superseded_by: Option<Uuid>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MemoryRecord {
    pub fn new(
        session_id: SessionId,
        class: MemoryClass,
        content: impl Into<String>,
        source: MemorySource,
        tags: impl IntoIterator<Item = String>,
    ) -> Result<Self, KnowledgeError> {
        source.validate()?;
        let content = content.into().trim().to_owned();
        if content.is_empty() {
            return Err(KnowledgeError::Validation(
                "memory content cannot be empty".into(),
            ));
        }
        if content.len() > 1_000_000 {
            return Err(KnowledgeError::Validation(
                "memory content exceeds one megabyte".into(),
            ));
        }
        let now = Utc::now();
        Ok(Self {
            schema_version: MEMORY_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            session_id,
            class,
            content,
            source,
            tags: tags
                .into_iter()
                .map(|tag| tag.trim().to_lowercase())
                .filter(|tag| !tag.is_empty())
                .collect(),
            valid_from: now,
            valid_until: None,
            supersedes: None,
            superseded_by: None,
            deleted_at: None,
            created_at: now,
            updated_at: now,
        })
    }

    #[must_use]
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        self.deleted_at.is_none()
            && self.superseded_by.is_none()
            && self.valid_from <= now
            && self.valid_until.is_none_or(|until| until > now)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryQuery {
    pub session_id: SessionId,
    pub text: String,
    pub classes: BTreeSet<MemoryClass>,
    pub tags: BTreeSet<String>,
    pub limit: usize,
}

impl MemoryQuery {
    pub fn validate(&self) -> Result<(), KnowledgeError> {
        if self.text.trim().is_empty() {
            return Err(KnowledgeError::Validation(
                "memory query text cannot be empty".into(),
            ));
        }
        if self.limit == 0 || self.limit > 100 {
            return Err(KnowledgeError::Validation(
                "memory query limit must be between 1 and 100".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryHit {
    pub record: MemoryRecord,
    pub score: f32,
    pub matched_terms: Vec<String>,
}

#[async_trait]
pub trait MemoryRepository: Send + Sync {
    async fn put(
        &self,
        record: &MemoryRecord,
        events: &[EventEnvelope],
    ) -> Result<MemoryRecord, RepositoryError>;

    async fn get(
        &self,
        session_id: SessionId,
        memory_id: Uuid,
    ) -> Result<Option<MemoryRecord>, RepositoryError>;

    async fn active_candidates(
        &self,
        session_id: SessionId,
        classes: &BTreeSet<MemoryClass>,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>, RepositoryError>;

    async fn mark_superseded(
        &self,
        session_id: SessionId,
        old_memory_id: Uuid,
        replacement: &MemoryRecord,
        events: &[EventEnvelope],
    ) -> Result<MemoryRecord, RepositoryError>;

    async fn delete(
        &self,
        session_id: SessionId,
        memory_id: Uuid,
        deleted_at: DateTime<Utc>,
        events: &[EventEnvelope],
    ) -> Result<(), RepositoryError>;
}

#[derive(Clone)]
pub struct MemoryService<R>
where
    R: MemoryRepository,
{
    repository: Arc<R>,
}

impl<R> MemoryService<R>
where
    R: MemoryRepository,
{
    #[must_use]
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn remember(
        &self,
        record: MemoryRecord,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MemoryRecord, KnowledgeError> {
        record.source.validate()?;
        let event = EventEnvelope::new(
            "memory.created",
            Some(record.session_id),
            correlation_id,
            actor_id,
            serde_json_compatible_memory_event(&record),
        );
        self.repository
            .put(&record, &[event])
            .await
            .map_err(KnowledgeError::Repository)
    }

    pub async fn supersede(
        &self,
        session_id: SessionId,
        old_memory_id: Uuid,
        mut replacement: MemoryRecord,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MemoryRecord, KnowledgeError> {
        if replacement.session_id != session_id {
            return Err(KnowledgeError::Validation(
                "replacement memory belongs to another session".into(),
            ));
        }
        replacement.supersedes = Some(old_memory_id);
        let event = EventEnvelope::new(
            "memory.superseded",
            Some(session_id),
            correlation_id,
            actor_id,
            serde_json_compatible_supersede_event(old_memory_id, &replacement),
        );
        self.repository
            .mark_superseded(session_id, old_memory_id, &replacement, &[event])
            .await
            .map_err(KnowledgeError::Repository)
    }

    pub async fn forget(
        &self,
        session_id: SessionId,
        memory_id: Uuid,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<(), KnowledgeError> {
        let deleted_at = Utc::now();
        let event = EventEnvelope::new(
            "memory.deleted",
            Some(session_id),
            correlation_id,
            actor_id,
            serde_json_string_map([
                ("memory_id", memory_id.to_string()),
                ("deleted_at", deleted_at.to_rfc3339()),
            ]),
        );
        self.repository
            .delete(session_id, memory_id, deleted_at, &[event])
            .await
            .map_err(KnowledgeError::Repository)
    }

    pub async fn search(&self, query: &MemoryQuery) -> Result<Vec<MemoryHit>, KnowledgeError> {
        query.validate()?;
        let query_terms = tokenize(&query.text);
        let candidates = self
            .repository
            .active_candidates(query.session_id, &query.classes, Utc::now(), 2_000)
            .await
            .map_err(KnowledgeError::Repository)?;

        let mut hits = candidates
            .into_iter()
            .filter(|record| {
                query.tags.is_empty() || query.tags.iter().all(|tag| record.tags.contains(tag))
            })
            .filter_map(|record| {
                let terms = tokenize(&record.content);
                let source_terms = tokenize(&record.source.locator);
                let mut matched = BTreeSet::new();
                let mut score = 0.0_f32;
                for query_term in query_terms.keys() {
                    let content_count = terms.get(query_term).copied().unwrap_or(0);
                    let source_count = source_terms.get(query_term).copied().unwrap_or(0);
                    if content_count > 0 || source_count > 0 {
                        matched.insert(query_term.clone());
                        score += (content_count as f32).ln_1p() + (source_count as f32 * 0.5);
                    }
                    if record.tags.contains(query_term) {
                        matched.insert(query_term.clone());
                        score += 1.5;
                    }
                }
                if matched.is_empty() {
                    return None;
                }
                let recency_days = (Utc::now() - record.updated_at).num_days().max(0) as f32;
                let recency_bonus = 1.0 / (1.0 + recency_days / 30.0);
                Some(MemoryHit {
                    record,
                    score: score + recency_bonus,
                    matched_terms: matched.into_iter().collect(),
                })
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            compare_score_then_id(right.score, left.score, left.record.id, right.record.id)
        });
        hits.truncate(query.limit);
        Ok(hits)
    }
}

fn compare_score_then_id(
    left_score: f32,
    right_score: f32,
    left_id: Uuid,
    right_id: Uuid,
) -> Ordering {
    left_score
        .partial_cmp(&right_score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| left_id.cmp(&right_id))
}

fn serde_json_compatible_memory_event(record: &MemoryRecord) -> serde_json::Value {
    serde_json::json!({
        "memory_id": record.id,
        "class": record.class,
        "source": record.source,
        "valid_from": record.valid_from,
    })
}

fn serde_json_compatible_supersede_event(
    old_memory_id: Uuid,
    replacement: &MemoryRecord,
) -> serde_json::Value {
    serde_json::json!({
        "old_memory_id": old_memory_id,
        "replacement_memory_id": replacement.id,
        "class": replacement.class,
    })
}

fn serde_json_string_map<const N: usize>(entries: [(&str, String); N]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, value) in entries {
        map.insert(key.to_owned(), serde_json::Value::String(value));
    }
    serde_json::Value::Object(map)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    Task,
    Dependency,
    Summary,
    Workspace,
    Memory,
    Decision,
    Lock,
    GitDiff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextCandidate {
    pub id: String,
    pub kind: ContextKind,
    pub content: String,
    pub source: String,
    pub inclusion_reason: String,
    pub priority: u8,
    pub relevance: f32,
    pub required: bool,
}

impl ContextCandidate {
    pub fn validate(&self) -> Result<(), KnowledgeError> {
        if self.id.trim().is_empty() || self.source.trim().is_empty() {
            return Err(KnowledgeError::Validation(
                "context candidate ID and source are required".into(),
            ));
        }
        if self.content.trim().is_empty() {
            return Err(KnowledgeError::Validation(
                "context candidate content cannot be empty".into(),
            ));
        }
        if !self.relevance.is_finite() || !(0.0..=1.0).contains(&self.relevance) {
            return Err(KnowledgeError::Validation(
                "context relevance must be finite and between 0 and 1".into(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn estimated_tokens(&self) -> usize {
        estimate_tokens(&self.content)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    pub max_tokens: usize,
    pub reserved_tokens: usize,
}

impl ContextBudget {
    pub fn validate(self) -> Result<usize, KnowledgeError> {
        if self.max_tokens == 0 || self.reserved_tokens >= self.max_tokens {
            return Err(KnowledgeError::Validation(
                "context budget must leave at least one usable token".into(),
            ));
        }
        Ok(self.max_tokens - self.reserved_tokens)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextItem {
    pub candidate: ContextCandidate,
    pub estimated_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OmittedContextItem {
    pub id: String,
    pub source: String,
    pub reason: String,
    pub estimated_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPackage {
    pub items: Vec<ContextItem>,
    pub omitted: Vec<OmittedContextItem>,
    pub estimated_tokens: usize,
    pub usable_budget: usize,
}

pub struct ContextBuilder;

impl ContextBuilder {
    pub fn build(
        candidates: impl IntoIterator<Item = ContextCandidate>,
        budget: ContextBudget,
    ) -> Result<ContextPackage, KnowledgeError> {
        let usable_budget = budget.validate()?;
        let mut candidates = candidates.into_iter().collect::<Vec<_>>();
        for candidate in &candidates {
            candidate.validate()?;
        }
        candidates.sort_by(|left, right| {
            right
                .required
                .cmp(&left.required)
                .then_with(|| right.priority.cmp(&left.priority))
                .then_with(|| {
                    right
                        .relevance
                        .partial_cmp(&left.relevance)
                        .unwrap_or(Ordering::Equal)
                })
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.id.cmp(&right.id))
        });

        let required_tokens = candidates
            .iter()
            .filter(|candidate| candidate.required)
            .map(ContextCandidate::estimated_tokens)
            .sum::<usize>();
        if required_tokens > usable_budget {
            return Err(KnowledgeError::RequiredContextOverflow {
                required_tokens,
                usable_budget,
            });
        }

        let mut items = Vec::new();
        let mut omitted = Vec::new();
        let mut used = 0_usize;
        for candidate in candidates {
            let tokens = candidate.estimated_tokens();
            if candidate.required || used.saturating_add(tokens) <= usable_budget {
                used = used.saturating_add(tokens);
                items.push(ContextItem {
                    candidate,
                    estimated_tokens: tokens,
                });
            } else {
                omitted.push(OmittedContextItem {
                    id: candidate.id,
                    source: candidate.source,
                    reason: "token_budget_exceeded".into(),
                    estimated_tokens: tokens,
                });
            }
        }
        Ok(ContextPackage {
            items,
            omitted,
            estimated_tokens: used,
            usable_budget,
        })
    }
}

#[must_use]
pub fn estimate_tokens(content: &str) -> usize {
    content.chars().count().div_ceil(4).max(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceScanConfig {
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub include_content: bool,
}

impl Default for WorkspaceScanConfig {
    fn default() -> Self {
        Self {
            max_files: 20_000,
            max_file_bytes: 1_000_000,
            include_content: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub relative_path: String,
    pub size_bytes: u64,
    pub modified_at: Option<DateTime<Utc>>,
    pub revision: String,
    pub text_content: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceIndex {
    root: PathBuf,
    files: Vec<FileSnapshot>,
}

impl WorkspaceIndex {
    pub fn build(
        root: impl AsRef<Path>,
        config: WorkspaceScanConfig,
    ) -> Result<Self, KnowledgeError> {
        if config.max_files == 0 || config.max_file_bytes == 0 {
            return Err(KnowledgeError::Validation(
                "workspace scan limits must be greater than zero".into(),
            ));
        }
        let root = fs::canonicalize(root.as_ref()).map_err(KnowledgeError::Io)?;
        if !root.is_dir() {
            return Err(KnowledgeError::Validation(
                "workspace root must be a directory".into(),
            ));
        }

        let mut queue = VecDeque::from([root.clone()]);
        let mut files = Vec::new();
        while let Some(directory) = queue.pop_front() {
            let mut entries = fs::read_dir(&directory)
                .map_err(KnowledgeError::Io)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(KnowledgeError::Io)?;
            entries.sort_by_key(fs::DirEntry::file_name);

            for entry in entries {
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).map_err(KnowledgeError::Io)?;
                if metadata.file_type().is_symlink() {
                    continue;
                }
                if metadata.is_dir() {
                    let canonical = fs::canonicalize(&path).map_err(KnowledgeError::Io)?;
                    ensure_inside_root(&root, &canonical)?;
                    queue.push_back(canonical);
                    continue;
                }
                if !metadata.is_file() {
                    continue;
                }
                if files.len() >= config.max_files {
                    return Err(KnowledgeError::WorkspaceLimitExceeded {
                        limit: config.max_files,
                    });
                }
                let canonical = fs::canonicalize(&path).map_err(KnowledgeError::Io)?;
                ensure_inside_root(&root, &canonical)?;
                let relative = canonical
                    .strip_prefix(&root)
                    .map_err(|_| KnowledgeError::PathEscapesWorkspace(canonical.clone()))?;
                let relative_path = normalize_relative_path(relative)?;
                let bytes = if metadata.len() <= config.max_file_bytes {
                    Some(fs::read(&canonical).map_err(KnowledgeError::Io)?)
                } else {
                    None
                };
                let text_content = if config.include_content {
                    bytes
                        .as_deref()
                        .filter(|bytes| !bytes.contains(&0))
                        .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
                } else {
                    None
                };
                let modified_at = metadata.modified().ok().map(DateTime::<Utc>::from);
                let revision = file_revision(
                    &relative_path,
                    metadata.len(),
                    modified_at,
                    bytes.as_deref(),
                );
                files.push(FileSnapshot {
                    relative_path,
                    size_bytes: metadata.len(),
                    modified_at,
                    revision,
                    text_content,
                });
            }
        }
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(Self { root, files })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn files(&self) -> &[FileSnapshot] {
        &self.files
    }

    pub fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<WorkspaceSearchHit>, KnowledgeError> {
        let query_terms = tokenize(query);
        if query_terms.is_empty() {
            return Err(KnowledgeError::Validation(
                "workspace query must contain searchable terms".into(),
            ));
        }
        if limit == 0 || limit > 1_000 {
            return Err(KnowledgeError::Validation(
                "workspace search limit must be between 1 and 1000".into(),
            ));
        }

        let mut hits = self
            .files
            .iter()
            .filter_map(|file| {
                let content = file.text_content.as_deref()?;
                let path_terms = tokenize(&file.relative_path);
                let content_terms = tokenize(content);
                let mut matched = BTreeSet::new();
                let mut score = 0.0_f32;
                for term in query_terms.keys() {
                    let path_count = path_terms.get(term).copied().unwrap_or(0);
                    let content_count = content_terms.get(term).copied().unwrap_or(0);
                    if path_count > 0 || content_count > 0 {
                        matched.insert(term.clone());
                        score += path_count as f32 * 2.0 + (content_count as f32).ln_1p();
                    }
                }
                if matched.is_empty() {
                    return None;
                }
                Some(WorkspaceSearchHit {
                    relative_path: file.relative_path.clone(),
                    revision: file.revision.clone(),
                    score,
                    matched_terms: matched.into_iter().collect(),
                    snippet: make_snippet(content, query_terms.keys()),
                })
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.relative_path.cmp(&right.relative_path))
        });
        hits.truncate(limit);
        Ok(hits)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSearchHit {
    pub relative_path: String,
    pub revision: String,
    pub score: f32,
    pub matched_terms: Vec<String>,
    pub snippet: String,
}

fn ensure_inside_root(root: &Path, candidate: &Path) -> Result<(), KnowledgeError> {
    if candidate.starts_with(root) {
        Ok(())
    } else {
        Err(KnowledgeError::PathEscapesWorkspace(candidate.to_owned()))
    }
}

fn normalize_relative_path(path: &Path) -> Result<String, KnowledgeError> {
    let components = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if components.is_empty() || components.iter().any(|component| component == "..") {
        return Err(KnowledgeError::Validation(
            "workspace path is not a valid relative path".into(),
        ));
    }
    Ok(components.join("/"))
}

fn file_revision(
    relative_path: &str,
    size: u64,
    modified_at: Option<DateTime<Utc>>,
    bytes: Option<&[u8]>,
) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    relative_path.hash(&mut hasher);
    size.hash(&mut hasher);
    modified_at
        .map(|value| value.timestamp_nanos_opt())
        .hash(&mut hasher);
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn make_snippet<'a>(content: &str, query_terms: impl Iterator<Item = &'a String>) -> String {
    let terms = query_terms.cloned().collect::<Vec<_>>();
    let lines = content.lines().collect::<Vec<_>>();
    let matching_line = lines
        .iter()
        .position(|line| {
            let line = line.to_lowercase();
            terms.iter().any(|term| line.contains(term))
        })
        .unwrap_or(0);
    let start = matching_line.saturating_sub(2);
    let end = (matching_line + 3).min(lines.len());
    lines[start..end]
        .join(
            "
",
        )
        .chars()
        .take(320)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn tokenize(value: &str) -> BTreeMap<String, usize> {
    let mut terms = BTreeMap::new();
    for token in value
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .map(str::trim)
        .filter(|token| token.len() >= 2)
        .map(str::to_lowercase)
    {
        *terms.entry(token).or_insert(0) += 1;
    }
    terms
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorRecord {
    pub namespace: String,
    pub id: String,
    pub vector: Vec<f32>,
    pub source: String,
}

impl VectorRecord {
    pub fn validate(&self) -> Result<(), KnowledgeError> {
        if self.namespace.trim().is_empty() || self.id.trim().is_empty() {
            return Err(KnowledgeError::Validation(
                "vector namespace and ID are required".into(),
            ));
        }
        if self.vector.is_empty() || self.vector.iter().any(|value| !value.is_finite()) {
            return Err(KnowledgeError::Validation(
                "vector must contain finite dimensions".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorHit {
    pub id: String,
    pub source: String,
    pub score: f32,
}

#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn upsert(&self, record: VectorRecord) -> Result<(), RepositoryError>;
    async fn search(
        &self,
        namespace: &str,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<VectorHit>, RepositoryError>;
    async fn delete_namespace(&self, namespace: &str) -> Result<usize, RepositoryError>;
}

#[derive(Default)]
pub struct InMemoryVectorStore {
    records: RwLock<BTreeMap<(String, String), VectorRecord>>,
}

#[async_trait]
impl VectorStore for InMemoryVectorStore {
    async fn upsert(&self, record: VectorRecord) -> Result<(), RepositoryError> {
        record
            .validate()
            .map_err(|error| RepositoryError::Backend(error.to_string()))?;
        self.records
            .write()
            .map_err(|_| RepositoryError::Backend("vector store lock poisoned".into()))?
            .insert((record.namespace.clone(), record.id.clone()), record);
        Ok(())
    }

    async fn search(
        &self,
        namespace: &str,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<VectorHit>, RepositoryError> {
        if namespace.trim().is_empty()
            || query.is_empty()
            || query.iter().any(|value| !value.is_finite())
        {
            return Err(RepositoryError::Backend(
                "vector search namespace and finite query are required".into(),
            ));
        }
        let records = self
            .records
            .read()
            .map_err(|_| RepositoryError::Backend("vector store lock poisoned".into()))?;
        let mut hits = records
            .values()
            .filter(|record| record.namespace == namespace && record.vector.len() == query.len())
            .filter_map(|record| {
                cosine_similarity(&record.vector, query).map(|score| VectorHit {
                    id: record.id.clone(),
                    source: record.source.clone(),
                    score,
                })
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        });
        hits.truncate(limit.clamp(1, 1_000));
        Ok(hits)
    }

    async fn delete_namespace(&self, namespace: &str) -> Result<usize, RepositoryError> {
        let mut records = self
            .records
            .write()
            .map_err(|_| RepositoryError::Backend("vector store lock poisoned".into()))?;
        let before = records.len();
        records.retain(|(record_namespace, _), _| record_namespace != namespace);
        Ok(before - records.len())
    }
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    let dot = left
        .iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum::<f32>();
    let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    let denominator = left_norm * right_norm;
    (denominator > 0.0).then_some(dot / denominator)
}

#[derive(Debug, Error)]
pub enum KnowledgeError {
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("knowledge repository error: {0}")]
    Repository(RepositoryError),
    #[error(
        "required context needs {required_tokens} tokens but only {usable_budget} are available"
    )]
    RequiredContextOverflow {
        required_tokens: usize,
        usable_budget: usize,
    },
    #[error("workspace path escapes root: {0}")]
    PathEscapesWorkspace(PathBuf),
    #[error("workspace file limit {limit} exceeded")]
    WorkspaceLimitExceeded { limit: usize },
    #[error("workspace I/O error: {0}")]
    Io(std::io::Error),
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("memory {0} not found")]
    MemoryNotFound(Uuid),
    #[error("memory {0} is already deleted or superseded")]
    MemoryInactive(Uuid),
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use super::*;

    #[test]
    fn context_builder_prioritizes_required_and_records_omissions() {
        let package = ContextBuilder::build(
            [
                ContextCandidate {
                    id: "task".into(),
                    kind: ContextKind::Task,
                    content: "implement the session invariant".into(),
                    source: "task:1".into(),
                    inclusion_reason: "active task".into(),
                    priority: 100,
                    relevance: 1.0,
                    required: true,
                },
                ContextCandidate {
                    id: "memory".into(),
                    kind: ContextKind::Memory,
                    content:
                        "a long optional memory that will not fit into the remaining token budget"
                            .into(),
                    source: "memory:1".into(),
                    inclusion_reason: "lexical match".into(),
                    priority: 10,
                    relevance: 0.4,
                    required: false,
                },
            ],
            ContextBudget {
                max_tokens: 12,
                reserved_tokens: 2,
            },
        )
        .expect("context");
        assert_eq!(package.items.len(), 1);
        assert_eq!(package.items[0].candidate.id, "task");
        assert_eq!(package.omitted.len(), 1);
    }

    #[test]
    fn required_context_overflow_is_explicit() {
        let error = ContextBuilder::build(
            [ContextCandidate {
                id: "required".into(),
                kind: ContextKind::Task,
                content: "required content that cannot fit".into(),
                source: "task:required".into(),
                inclusion_reason: "active task".into(),
                priority: 100,
                relevance: 1.0,
                required: true,
            }],
            ContextBudget {
                max_tokens: 3,
                reserved_tokens: 2,
            },
        )
        .expect_err("required context must not be silently truncated");
        assert!(matches!(
            error,
            KnowledgeError::RequiredContextOverflow { .. }
        ));
    }

    #[test]
    fn workspace_search_handles_unicode_snippets() {
        let root = env::temp_dir().join(format!("sessionweft-unicode-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("directory");
        fs::write(
            root.join("architecture.md"),
            "Dòng đầu tiên\nQuyết định kiến trúc phải giữ Session làm nguồn dữ liệu chính.\nDòng cuối.",
        )
        .expect("source");

        let index = WorkspaceIndex::build(&root, WorkspaceScanConfig::default()).expect("index");
        let hits = index.search("kiến trúc", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("Quyết định kiến trúc"));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn workspace_scan_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let root = env::temp_dir().join(format!("sessionweft-root-{}", Uuid::new_v4()));
        let outside = env::temp_dir().join(format!("sessionweft-outside-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("root");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(outside.join("secret.txt"), "outside secret").expect("outside file");
        symlink(outside.join("secret.txt"), root.join("secret-link.txt")).expect("symlink");

        let index = WorkspaceIndex::build(&root, WorkspaceScanConfig::default()).expect("index");
        assert!(index.files().is_empty());
        fs::remove_dir_all(root).expect("cleanup root");
        fs::remove_dir_all(outside).expect("cleanup outside");
    }

    #[test]
    fn workspace_search_stays_inside_root_and_skips_binary_files() {
        let root = env::temp_dir().join(format!("sessionweft-workspace-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).expect("directory");
        fs::write(root.join("src/lib.rs"), "pub struct SessionRuntime;").expect("source");
        fs::write(root.join("binary.bin"), [0_u8, 1, 2, 3]).expect("binary");

        let index = WorkspaceIndex::build(&root, WorkspaceScanConfig::default()).expect("index");
        let hits = index.search("SessionRuntime", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].relative_path, "src/lib.rs");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test]
    async fn vector_namespaces_are_isolated_and_deletable() {
        let store = InMemoryVectorStore::default();
        store
            .upsert(VectorRecord {
                namespace: "session-a".into(),
                id: "one".into(),
                vector: vec![1.0, 0.0],
                source: "memory:one".into(),
            })
            .await
            .expect("upsert");
        store
            .upsert(VectorRecord {
                namespace: "session-b".into(),
                id: "two".into(),
                vector: vec![1.0, 0.0],
                source: "memory:two".into(),
            })
            .await
            .expect("upsert");
        assert_eq!(
            store
                .search("session-a", &[1.0, 0.0], 10)
                .await
                .expect("search")
                .len(),
            1
        );
        assert_eq!(
            store.delete_namespace("session-a").await.expect("delete"),
            1
        );
        assert!(
            store
                .search("session-a", &[1.0, 0.0], 10)
                .await
                .expect("search")
                .is_empty()
        );
        assert_eq!(
            store
                .search("session-b", &[1.0, 0.0], 10)
                .await
                .expect("search")
                .len(),
            1
        );
    }

    #[test]
    fn superseded_memory_is_not_active() {
        let mut record = MemoryRecord::new(
            SessionId::new(),
            MemoryClass::Decision,
            "use SQLite for local mode",
            MemorySource {
                kind: "adr".into(),
                locator: "ADR-0001".into(),
                revision: Some("1".into()),
            },
            ["storage".into()],
        )
        .expect("memory");
        record.superseded_by = Some(Uuid::new_v4());
        assert!(!record.is_active_at(Utc::now()));
    }
}
