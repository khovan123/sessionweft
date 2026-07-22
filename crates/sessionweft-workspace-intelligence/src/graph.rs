use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    path::Path,
};

use crate::{DependencyEdge, DependencyKind, IndexedFile, SymbolId, hash_parts};

pub(crate) fn build_edges(files: &BTreeMap<String, IndexedFile>) -> Vec<DependencyEdge> {
    let mut edges = Vec::new();
    for file in files.values() {
        for symbol in &file.symbols {
            if let Some(parent) = &symbol.parent {
                edges.push(DependencyEdge {
                    from: parent.clone(),
                    to: symbol.id.clone(),
                    kind: DependencyKind::Contains,
                    reason: "syntax_parent".into(),
                });
            }
        }
        let Some(from) = file.file_symbol() else {
            continue;
        };
        for import in &file.imports {
            for target in resolve_import_targets(file, import, files) {
                if let Some(to) = target.file_symbol() {
                    edges.push(DependencyEdge {
                        from: from.id.clone(),
                        to: to.id.clone(),
                        kind: DependencyKind::Imports,
                        reason: import.clone(),
                    });
                }
            }
        }
    }
    edges.sort_by(|left, right| {
        left.from
            .cmp(&right.from)
            .then_with(|| left.to.cmp(&right.to))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    edges.dedup_by(|left, right| {
        left.from == right.from && left.to == right.to && left.kind == right.kind
    });
    edges
}

pub(crate) fn workspace_revision(files: &BTreeMap<String, IndexedFile>) -> String {
    let parts = files
        .iter()
        .flat_map(|(path, file)| [path.as_bytes(), file.revision.as_bytes()])
        .collect::<Vec<_>>();
    hash_parts(parts)
}

pub(crate) fn reverse_dependents(
    files: &BTreeMap<String, IndexedFile>,
    edges: &[DependencyEdge],
    changed: &BTreeSet<String>,
) -> BTreeSet<String> {
    let symbol_to_path = files
        .values()
        .flat_map(|file| {
            file.symbols
                .iter()
                .map(move |symbol| (symbol.id.clone(), file.relative_path.clone()))
        })
        .collect::<HashMap<_, _>>();
    let mut reverse = HashMap::<String, BTreeSet<String>>::new();
    for edge in edges.iter().filter(|edge| edge.kind == DependencyKind::Imports) {
        let (Some(from), Some(to)) = (symbol_to_path.get(&edge.from), symbol_to_path.get(&edge.to))
        else {
            continue;
        };
        reverse.entry(to.clone()).or_default().insert(from.clone());
    }
    let mut queue = VecDeque::from_iter(changed.iter().cloned());
    let mut result = BTreeSet::new();
    while let Some(path) = queue.pop_front() {
        if let Some(dependents) = reverse.get(&path) {
            for dependent in dependents {
                if result.insert(dependent.clone()) {
                    queue.push_back(dependent.clone());
                }
            }
        }
    }
    result
}

pub(crate) fn dependency_degree(edges: &[DependencyEdge]) -> HashMap<SymbolId, usize> {
    let mut degree = HashMap::new();
    for edge in edges {
        *degree.entry(edge.from.clone()).or_insert(0) += 1;
        *degree.entry(edge.to.clone()).or_insert(0) += 1;
    }
    degree
}

fn resolve_import_targets<'a>(
    source_file: &IndexedFile,
    import: &str,
    files: &'a BTreeMap<String, IndexedFile>,
) -> Vec<&'a IndexedFile> {
    let normalized = import_specifier(import)
        .replace("crate::", "")
        .replace("super::", "")
        .replace("self::", "")
        .replace("::", "/")
        .trim_start_matches("./")
        .trim_matches('/')
        .to_owned();
    if normalized.is_empty() {
        return Vec::new();
    }
    let parent = Path::new(&source_file.relative_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let relative_candidate = crate::normalize_path(&parent.join(&normalized));
    let segments = normalized.split('/').collect::<BTreeSet<_>>();
    files
        .values()
        .filter(|candidate| candidate.relative_path != source_file.relative_path)
        .filter(|candidate| {
            let stem = Path::new(&candidate.relative_path)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            candidate.relative_path.starts_with(&relative_candidate)
                || candidate.relative_path.ends_with(&format!("/{normalized}.rs"))
                || candidate.relative_path.ends_with(&format!("/{normalized}.ts"))
                || candidate.relative_path.ends_with(&format!("/{normalized}.js"))
                || candidate.relative_path.ends_with(&format!("/{normalized}.py"))
                || segments.contains(stem)
        })
        .collect()
}

fn import_specifier(import: &str) -> String {
    for quote in ['\'', '"'] {
        if let Some(start) = import.find(quote) {
            let remainder = &import[start + 1..];
            if let Some(end) = remainder.find(quote) {
                return remainder[..end].to_owned();
            }
        }
    }
    import
        .trim()
        .trim_start_matches("use ")
        .trim_start_matches("from ")
        .trim_start_matches("import ")
        .trim_end_matches(';')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}
