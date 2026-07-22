mod graph;
mod model;
mod parser;

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use chrono::Utc;
use graph::{build_edges, dependency_degree, reverse_dependents, workspace_revision};
pub use model::*;
use parser::{
    canonical_directory, index_file, normalize_path, relative_path, revision_for,
    scan_supported_files, validate_relative_path,
};

#[derive(Debug, Clone)]
pub struct WorkspaceIntelligence {
    workspace_id: String,
    root: PathBuf,
    config: WorkspaceIntelligenceConfig,
    files: BTreeMap<String, IndexedFile>,
    edges: Vec<DependencyEdge>,
    workspace_revision: String,
    indexed_at: chrono::DateTime<Utc>,
}

impl WorkspaceIntelligence {
    pub fn build(
        workspace_id: impl Into<String>,
        root: impl AsRef<Path>,
        config: WorkspaceIntelligenceConfig,
    ) -> Result<Self, WorkspaceIntelligenceError> {
        config.validate()?;
        let workspace_id = workspace_id.into().trim().to_owned();
        if workspace_id.is_empty() || workspace_id.len() > 256 {
            return Err(WorkspaceIntelligenceError::Validation(
                "workspace ID must be between 1 and 256 bytes".into(),
            ));
        }
        let root = canonical_directory(root.as_ref())?;
        let mut files = BTreeMap::new();
        for path in scan_supported_files(&root, config)? {
            let file = index_file(&workspace_id, &root, &path, config)?;
            files.insert(file.relative_path.clone(), file);
        }
        let edges = build_edges(&files);
        Ok(Self {
            workspace_revision: workspace_revision(&files),
            workspace_id,
            root,
            config,
            files,
            edges,
            indexed_at: Utc::now(),
        })
    }

    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn workspace_revision(&self) -> &str {
        &self.workspace_revision
    }

    #[must_use]
    pub fn files(&self) -> &BTreeMap<String, IndexedFile> {
        &self.files
    }

    #[must_use]
    pub fn edges(&self) -> &[DependencyEdge] {
        &self.edges
    }

    pub fn detect_changes(&self) -> Result<Vec<String>, WorkspaceIntelligenceError> {
        let mut current = BTreeMap::new();
        for path in scan_supported_files(&self.root, self.config)? {
            let relative = relative_path(&self.root, &path)?;
            current.insert(
                relative.clone(),
                revision_for(&path, &relative, self.config.max_file_bytes)?,
            );
        }
        let mut changed = BTreeSet::new();
        for (path, revision) in &current {
            if self
                .files
                .get(path)
                .is_none_or(|file| file.revision != *revision)
            {
                changed.insert(path.clone());
            }
        }
        for path in self.files.keys() {
            if !current.contains_key(path) {
                changed.insert(path.clone());
            }
        }
        Ok(changed.into_iter().collect())
    }

    pub fn update_paths<P, I>(
        &mut self,
        changed_paths: I,
    ) -> Result<IndexUpdateReport, WorkspaceIntelligenceError>
    where
        P: AsRef<Path>,
        I: IntoIterator<Item = P>,
    {
        let mut changed = BTreeSet::new();
        for path in changed_paths {
            changed.insert(self.normalize_requested_path(path.as_ref())?);
        }
        if changed.is_empty() {
            return Ok(IndexUpdateReport {
                changed_files: Vec::new(),
                reindexed_files: Vec::new(),
                affected_files: Vec::new(),
                removed_files: Vec::new(),
                workspace_revision: self.workspace_revision.clone(),
            });
        }
        let mut affected = reverse_dependents(&self.files, &self.edges, &changed);
        let mut removed = Vec::new();
        for relative in &changed {
            let absolute = self.root.join(relative);
            if absolute.exists() {
                let file = index_file(&self.workspace_id, &self.root, &absolute, self.config)?;
                self.files.insert(relative.clone(), file);
            } else if self.files.remove(relative).is_some() {
                removed.push(relative.clone());
            }
        }
        self.edges = build_edges(&self.files);
        affected.extend(reverse_dependents(&self.files, &self.edges, &changed));
        affected.retain(|path| !changed.contains(path) && self.files.contains_key(path));
        let mut reindexed = changed.clone();
        for relative in &affected {
            let absolute = self.root.join(relative);
            if absolute.exists() {
                let file = index_file(&self.workspace_id, &self.root, &absolute, self.config)?;
                self.files.insert(relative.clone(), file);
                reindexed.insert(relative.clone());
            }
        }
        self.edges = build_edges(&self.files);
        self.workspace_revision = workspace_revision(&self.files);
        self.indexed_at = Utc::now();
        Ok(IndexUpdateReport {
            changed_files: changed.into_iter().collect(),
            reindexed_files: reindexed.into_iter().collect(),
            affected_files: affected.into_iter().collect(),
            removed_files: removed,
            workspace_revision: self.workspace_revision.clone(),
        })
    }

    pub fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<WorkspaceContextItem>, WorkspaceIntelligenceError> {
        let terms = tokenize(query);
        if terms.is_empty() {
            return Err(WorkspaceIntelligenceError::Validation(
                "workspace query must contain searchable terms".into(),
            ));
        }
        if limit == 0 || limit > 1_000 {
            return Err(WorkspaceIntelligenceError::Validation(
                "workspace result limit must be between 1 and 1000".into(),
            ));
        }
        let degree = dependency_degree(&self.edges);
        let now = Utc::now();
        let mut hits = Vec::new();
        for file in self.files.values() {
            for symbol in &file.symbols {
                let path_terms = tokenize(&symbol.relative_path);
                let name_terms = tokenize(&symbol.qualified_name);
                let signature_terms = tokenize(&symbol.signature);
                let mut matched = BTreeSet::new();
                let mut score = 0.0_f32;
                let mut reasons = Vec::new();
                for term in &terms {
                    let path_hit = path_terms.contains(term);
                    let name_hit = name_terms.contains(term);
                    let signature_hit = signature_terms.contains(term);
                    if path_hit || name_hit || signature_hit {
                        matched.insert(term.clone());
                    }
                    score += if name_hit { 4.0 } else { 0.0 };
                    score += if path_hit { 2.0 } else { 0.0 };
                    score += if signature_hit { 1.0 } else { 0.0 };
                }
                if matched.is_empty() {
                    continue;
                }
                if matched
                    .iter()
                    .any(|term| symbol.name.eq_ignore_ascii_case(term))
                {
                    score += 2.0;
                    reasons.push("exact_symbol");
                }
                if matched.iter().any(|term| path_terms.contains(term)) {
                    reasons.push("path_match");
                }
                if matched.iter().any(|term| name_terms.contains(term)) {
                    reasons.push("symbol_match");
                }
                let dependency_bonus = degree.get(&symbol.id).copied().unwrap_or(0) as f32;
                if dependency_bonus > 0.0 {
                    score += dependency_bonus.min(10.0) * 0.15;
                    reasons.push("dependency_signal");
                }
                let recency_days = file
                    .modified_at
                    .map(|value| (now - value).num_days().max(0) as f32)
                    .unwrap_or(365.0);
                score += 1.0 / (1.0 + recency_days / 30.0);
                reasons.push("revision_bound");
                let content = source_slice(&file.content, &symbol.range);
                hits.push(WorkspaceContextItem {
                    symbol_id: symbol.id.clone(),
                    relative_path: symbol.relative_path.clone(),
                    source_revision: file.revision.clone(),
                    workspace_revision: self.workspace_revision.clone(),
                    range: symbol.range.clone(),
                    estimated_tokens: estimate_tokens(&content),
                    inclusion_reason: reasons.join(","),
                    score,
                    content,
                });
            }
        }
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.relative_path.cmp(&right.relative_path))
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    pub fn save_json(&self, path: impl AsRef<Path>) -> Result<(), WorkspaceIntelligenceError> {
        let path = path.as_ref();
        let parent = path.parent().ok_or_else(|| {
            WorkspaceIntelligenceError::Validation("snapshot path must have a parent".into())
        })?;
        fs::create_dir_all(parent).map_err(WorkspaceIntelligenceError::Io)?;
        let snapshot = WorkspaceSnapshot {
            schema_version: WORKSPACE_GRAPH_SCHEMA_VERSION,
            workspace_id: self.workspace_id.clone(),
            root: self.root.to_string_lossy().into_owned(),
            workspace_revision: self.workspace_revision.clone(),
            indexed_at: self.indexed_at,
            files: self.files.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&snapshot)
            .map_err(|error| WorkspaceIntelligenceError::Serialization(error.to_string()))?;
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, bytes).map_err(WorkspaceIntelligenceError::Io)?;
        fs::rename(temporary, path).map_err(WorkspaceIntelligenceError::Io)
    }

    pub fn load_json(
        path: impl AsRef<Path>,
        config: WorkspaceIntelligenceConfig,
    ) -> Result<Self, WorkspaceIntelligenceError> {
        config.validate()?;
        let snapshot: WorkspaceSnapshot =
            serde_json::from_slice(&fs::read(path).map_err(WorkspaceIntelligenceError::Io)?)
                .map_err(|error| WorkspaceIntelligenceError::Serialization(error.to_string()))?;
        if snapshot.schema_version != WORKSPACE_GRAPH_SCHEMA_VERSION {
            return Err(WorkspaceIntelligenceError::UnsupportedSchema(
                snapshot.schema_version,
            ));
        }
        let root = canonical_directory(Path::new(&snapshot.root))?;
        for path in snapshot.files.keys() {
            validate_relative_path(Path::new(path))?;
        }
        let computed_revision = workspace_revision(&snapshot.files);
        if computed_revision != snapshot.workspace_revision {
            return Err(WorkspaceIntelligenceError::RevisionMismatch {
                expected: snapshot.workspace_revision,
                actual: computed_revision,
            });
        }
        let edges = build_edges(&snapshot.files);
        Ok(Self {
            workspace_id: snapshot.workspace_id,
            root,
            config,
            files: snapshot.files,
            edges,
            workspace_revision: computed_revision,
            indexed_at: snapshot.indexed_at,
        })
    }

    fn normalize_requested_path(&self, path: &Path) -> Result<String, WorkspaceIntelligenceError> {
        if path.is_absolute() {
            if path.exists() {
                let canonical = fs::canonicalize(path).map_err(WorkspaceIntelligenceError::Io)?;
                return relative_path(&self.root, &canonical);
            }
            return Err(WorkspaceIntelligenceError::PathEscapesWorkspace(
                path.to_owned(),
            ));
        }
        validate_relative_path(path)?;
        let joined = self.root.join(path);
        if joined.exists() {
            let canonical = fs::canonicalize(joined).map_err(WorkspaceIntelligenceError::Io)?;
            relative_path(&self.root, &canonical)
        } else {
            Ok(normalize_path(path))
        }
    }
}

#[derive(Debug, Clone)]
pub struct PollingWorkspaceWatcher {
    last_revision: String,
}

impl PollingWorkspaceWatcher {
    #[must_use]
    pub fn new(index: &WorkspaceIntelligence) -> Self {
        Self {
            last_revision: index.workspace_revision.clone(),
        }
    }

    pub fn poll_once(
        &mut self,
        index: &mut WorkspaceIntelligence,
    ) -> Result<IndexUpdateReport, WorkspaceIntelligenceError> {
        let report = index.update_paths(index.detect_changes()?)?;
        self.last_revision = report.workspace_revision.clone();
        Ok(report)
    }

    #[must_use]
    pub fn last_revision(&self) -> &str {
        &self.last_revision
    }
}

fn tokenize(value: &str) -> BTreeSet<String> {
    value
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn source_slice(content: &str, range: &SourceRange) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return String::new();
    }
    let start = range.start_line.saturating_sub(1).min(lines.len() - 1);
    let end = range.end_line.max(range.start_line).min(lines.len());
    lines[start..end].join("\n").chars().take(8_000).collect()
}

#[must_use]
pub fn estimate_tokens(content: &str) -> usize {
    content.chars().count().div_ceil(4).max(1)
}
