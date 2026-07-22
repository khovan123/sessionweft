use std::{
    collections::VecDeque,
    fs,
    hash::{Hash, Hasher},
    path::{Component, Path, PathBuf},
};

use chrono::{DateTime, Utc};
use tree_sitter::{Language, Node, Parser};

use crate::{
    IndexedFile, SourceLanguage, SourceRange, SymbolId, SymbolKind, SymbolRecord,
    WorkspaceIntelligenceConfig, WorkspaceIntelligenceError,
};

impl SourceLanguage {
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "rs" => Some(Self::Rust),
            "ts" | "tsx" => Some(Self::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "py" | "pyi" => Some(Self::Python),
            _ => None,
        }
    }

    fn grammar(self, path: &Path) -> Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::TypeScript if path.extension().and_then(|value| value.to_str()) == Some("tsx") => {
                tree_sitter_typescript::LANGUAGE_TSX.into()
            }
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
        }
    }
}

impl SourceRange {
    fn from_node(node: Node<'_>) -> Self {
        let start = node.start_position();
        let end = node.end_position();
        Self {
            start_line: start.row + 1,
            start_column: start.column + 1,
            end_line: end.row + 1,
            end_column: end.column + 1,
        }
    }
}

pub(crate) fn scan_supported_files(
    root: &Path,
    config: WorkspaceIntelligenceConfig,
) -> Result<Vec<PathBuf>, WorkspaceIntelligenceError> {
    let mut queue = VecDeque::from([root.to_owned()]);
    let mut files = Vec::new();
    while let Some(directory) = queue.pop_front() {
        let mut entries = fs::read_dir(&directory)
            .map_err(WorkspaceIntelligenceError::Io)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(WorkspaceIntelligenceError::Io)?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(WorkspaceIntelligenceError::Io)?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                if matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "target" | "node_modules" | ".venv")
                ) {
                    continue;
                }
                let canonical = fs::canonicalize(path).map_err(WorkspaceIntelligenceError::Io)?;
                if !canonical.starts_with(root) {
                    return Err(WorkspaceIntelligenceError::PathEscapesWorkspace(canonical));
                }
                queue.push_back(canonical);
            } else if metadata.is_file() && SourceLanguage::from_path(&path).is_some() {
                if files.len() >= config.max_files {
                    return Err(WorkspaceIntelligenceError::FileLimitExceeded(config.max_files));
                }
                files.push(fs::canonicalize(path).map_err(WorkspaceIntelligenceError::Io)?);
            }
        }
    }
    files.sort();
    Ok(files)
}

pub(crate) fn index_file(
    workspace_id: &str,
    root: &Path,
    path: &Path,
    config: WorkspaceIntelligenceConfig,
) -> Result<IndexedFile, WorkspaceIntelligenceError> {
    let canonical = fs::canonicalize(path).map_err(WorkspaceIntelligenceError::Io)?;
    if !canonical.starts_with(root) {
        return Err(WorkspaceIntelligenceError::PathEscapesWorkspace(canonical));
    }
    let metadata = fs::metadata(&canonical).map_err(WorkspaceIntelligenceError::Io)?;
    if metadata.len() > config.max_file_bytes {
        return Err(WorkspaceIntelligenceError::FileTooLarge {
            path: canonical,
            size: metadata.len(),
            limit: config.max_file_bytes,
        });
    }
    let language = SourceLanguage::from_path(&canonical)
        .ok_or_else(|| WorkspaceIntelligenceError::UnsupportedLanguage(canonical.clone()))?;
    let content = fs::read_to_string(&canonical).map_err(WorkspaceIntelligenceError::Io)?;
    let relative_path = relative_path(root, &canonical)?;
    let revision = hash_parts([relative_path.as_bytes(), content.as_bytes()]);
    let mut parser = Parser::new();
    parser
        .set_language(&language.grammar(&canonical))
        .map_err(|error| WorkspaceIntelligenceError::Parser(error.to_string()))?;
    let tree = parser
        .parse(&content, None)
        .ok_or_else(|| WorkspaceIntelligenceError::Parser("parser returned no tree".into()))?;
    let file_id = stable_symbol_id(workspace_id, &relative_path, SymbolKind::File, &relative_path);
    let mut symbols = vec![SymbolRecord {
        id: file_id.clone(),
        workspace_id: workspace_id.to_owned(),
        relative_path: relative_path.clone(),
        file_revision: revision.clone(),
        language,
        kind: SymbolKind::File,
        name: relative_path.clone(),
        qualified_name: relative_path.clone(),
        range: SourceRange {
            start_line: 1,
            start_column: 1,
            end_line: content.lines().count().max(1),
            end_column: 1,
        },
        parent: None,
        signature: relative_path.clone(),
    }];
    let mut imports = Vec::new();
    walk(
        tree.root_node(),
        content.as_bytes(),
        workspace_id,
        &relative_path,
        &revision,
        language,
        Some(file_id),
        None,
        false,
        &mut symbols,
        &mut imports,
        config.max_symbols_per_file,
    )?;
    symbols.sort_by(|left, right| {
        left.range
            .start_line
            .cmp(&right.range.start_line)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.qualified_name.cmp(&right.qualified_name))
    });
    imports.sort();
    imports.dedup();
    Ok(IndexedFile {
        relative_path,
        revision,
        language,
        size_bytes: metadata.len(),
        modified_at: metadata.modified().ok().map(DateTime::<Utc>::from),
        content,
        symbols,
        imports,
    })
}

#[allow(clippy::too_many_arguments)]
fn walk(
    node: Node<'_>,
    source: &[u8],
    workspace_id: &str,
    relative_path: &str,
    revision: &str,
    language: SourceLanguage,
    parent: Option<SymbolId>,
    parent_name: Option<&str>,
    inside_type: bool,
    symbols: &mut Vec<SymbolRecord>,
    imports: &mut Vec<String>,
    max_symbols: usize,
) -> Result<(), WorkspaceIntelligenceError> {
    if symbols.len() >= max_symbols {
        return Err(WorkspaceIntelligenceError::SymbolLimitExceeded {
            path: relative_path.to_owned(),
            limit: max_symbols,
        });
    }
    let symbol_kind = classify(language, node, inside_type);
    let mut child_parent = parent.clone();
    let mut child_parent_name = parent_name.map(ToOwned::to_owned);
    let child_inside_type = inside_type || is_type_scope(language, node.kind());
    if let Some(kind) = symbol_kind {
        let raw = node.utf8_text(source).unwrap_or_default();
        let name = symbol_name(node, source, kind);
        if kind == SymbolKind::Import {
            imports.push(name.clone());
        }
        let qualified_name = parent_name
            .filter(|_| kind != SymbolKind::Import)
            .map_or_else(|| name.clone(), |value| format!("{value}::{name}"));
        let id = stable_symbol_id(workspace_id, relative_path, kind, &qualified_name);
        symbols.push(SymbolRecord {
            id: id.clone(),
            workspace_id: workspace_id.to_owned(),
            relative_path: relative_path.to_owned(),
            file_revision: revision.to_owned(),
            language,
            kind,
            name,
            qualified_name: qualified_name.clone(),
            range: SourceRange::from_node(node),
            parent: parent.clone(),
            signature: raw.lines().next().unwrap_or(raw).trim().chars().take(512).collect(),
        });
        if kind != SymbolKind::Import {
            child_parent = Some(id);
            child_parent_name = Some(qualified_name);
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(
            child,
            source,
            workspace_id,
            relative_path,
            revision,
            language,
            child_parent.clone(),
            child_parent_name.as_deref(),
            child_inside_type,
            symbols,
            imports,
            max_symbols,
        )?;
    }
    Ok(())
}

fn classify(language: SourceLanguage, node: Node<'_>, inside_type: bool) -> Option<SymbolKind> {
    match language {
        SourceLanguage::Rust => match node.kind() {
            "mod_item" => Some(SymbolKind::Module),
            "struct_item" | "enum_item" | "trait_item" | "type_item" | "union_item" => {
                Some(SymbolKind::Type)
            }
            "function_item" if inside_type => Some(SymbolKind::Method),
            "function_item" => Some(SymbolKind::Function),
            "use_declaration" | "extern_crate_declaration" => Some(SymbolKind::Import),
            _ => None,
        },
        SourceLanguage::TypeScript | SourceLanguage::JavaScript => match node.kind() {
            "class_declaration" | "interface_declaration" | "type_alias_declaration"
            | "enum_declaration" => Some(SymbolKind::Type),
            "function_declaration" | "generator_function_declaration" => Some(SymbolKind::Function),
            "method_definition" => Some(SymbolKind::Method),
            "import_statement" => Some(SymbolKind::Import),
            "variable_declarator"
                if node.child_by_field_name("value").is_some_and(|value| {
                    matches!(value.kind(), "arrow_function" | "function_expression")
                }) => Some(SymbolKind::Function),
            _ => None,
        },
        SourceLanguage::Python => match node.kind() {
            "class_definition" => Some(SymbolKind::Type),
            "function_definition" if inside_type => Some(SymbolKind::Method),
            "function_definition" => Some(SymbolKind::Function),
            "import_statement" | "import_from_statement" => Some(SymbolKind::Import),
            _ => None,
        },
    }
}

fn is_type_scope(language: SourceLanguage, kind: &str) -> bool {
    match language {
        SourceLanguage::Rust => matches!(kind, "impl_item" | "trait_item"),
        SourceLanguage::TypeScript | SourceLanguage::JavaScript => matches!(
            kind,
            "class_declaration" | "class_body" | "interface_declaration" | "object_type"
        ),
        SourceLanguage::Python => kind == "class_definition",
    }
}

fn symbol_name(node: Node<'_>, source: &[u8], kind: SymbolKind) -> String {
    if kind == SymbolKind::Import {
        return node
            .utf8_text(source)
            .unwrap_or_default()
            .trim()
            .chars()
            .take(512)
            .collect();
    }
    node.child_by_field_name("name")
        .and_then(|value| value.utf8_text(source).ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("anonymous@{}", node.start_position().row + 1))
}

pub(crate) fn canonical_directory(path: &Path) -> Result<PathBuf, WorkspaceIntelligenceError> {
    let canonical = fs::canonicalize(path).map_err(WorkspaceIntelligenceError::Io)?;
    if !canonical.is_dir() {
        return Err(WorkspaceIntelligenceError::Validation(
            "workspace root must be a directory".into(),
        ));
    }
    Ok(canonical)
}

pub(crate) fn relative_path(
    root: &Path,
    candidate: &Path,
) -> Result<String, WorkspaceIntelligenceError> {
    if !candidate.starts_with(root) {
        return Err(WorkspaceIntelligenceError::PathEscapesWorkspace(candidate.to_owned()));
    }
    let relative = candidate
        .strip_prefix(root)
        .map_err(|_| WorkspaceIntelligenceError::PathEscapesWorkspace(candidate.to_owned()))?;
    validate_relative_path(relative)?;
    Ok(normalize_path(relative))
}

pub(crate) fn validate_relative_path(path: &Path) -> Result<(), WorkspaceIntelligenceError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(WorkspaceIntelligenceError::Validation(
            "workspace path must be non-empty and relative".into(),
        ));
    }
    if path.components().any(|component| !matches!(component, Component::Normal(_))) {
        return Err(WorkspaceIntelligenceError::PathEscapesWorkspace(path.to_owned()));
    }
    Ok(())
}

pub(crate) fn normalize_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn revision_for(
    path: &Path,
    relative_path: &str,
    max_file_bytes: u64,
) -> Result<String, WorkspaceIntelligenceError> {
    let metadata = fs::metadata(path).map_err(WorkspaceIntelligenceError::Io)?;
    if metadata.len() > max_file_bytes {
        return Err(WorkspaceIntelligenceError::FileTooLarge {
            path: path.to_owned(),
            size: metadata.len(),
            limit: max_file_bytes,
        });
    }
    let content = fs::read(path).map_err(WorkspaceIntelligenceError::Io)?;
    Ok(hash_parts([relative_path.as_bytes(), content.as_slice()]))
}

pub(crate) fn hash_parts<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for part in parts {
        part.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

fn stable_symbol_id(
    workspace_id: &str,
    relative_path: &str,
    kind: SymbolKind,
    qualified_name: &str,
) -> SymbolId {
    let kind_name = format!("{kind:?}");
    SymbolId(hash_parts([
        workspace_id.as_bytes(),
        relative_path.as_bytes(),
        kind_name.as_bytes(),
        qualified_name.as_bytes(),
    ]))
}
