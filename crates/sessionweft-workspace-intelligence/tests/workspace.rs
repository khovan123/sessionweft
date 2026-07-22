use std::{collections::BTreeSet, fs, io::Write, path::Path};

use sessionweft_workspace_intelligence::{
    PollingWorkspaceWatcher, SymbolKind, WorkspaceIntelligence, WorkspaceIntelligenceConfig,
    WorkspaceIntelligenceError,
};
use tempfile::TempDir;

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("directories");
    let mut file = fs::File::create(path).expect("file");
    file.write_all(content.as_bytes()).expect("content");
}

fn fixture() -> TempDir {
    let directory = tempfile::tempdir().expect("tempdir");
    write(
        directory.path(),
        "src/main.rs",
        "mod util;\nuse crate::util::greet;\nfn main() { greet(); }\n",
    );
    write(
        directory.path(),
        "src/util.rs",
        "pub struct Greeter;\nimpl Greeter { pub fn run(&self) {} }\npub fn greet() {}\n",
    );
    write(
        directory.path(),
        "web/api.ts",
        "import { helper } from './helper';\nexport class Api { call() { helper(); } }\n",
    );
    write(
        directory.path(),
        "web/helper.ts",
        "export const helper = () => 1;\n",
    );
    write(
        directory.path(),
        "worker.py",
        "from service import execute\nclass Worker:\n    def run(self):\n        return execute()\n",
    );
    directory
}

#[test]
fn indexes_languages_and_returns_explainable_revision_bound_context() {
    let directory = fixture();
    let index = WorkspaceIntelligence::build(
        "workspace-1",
        directory.path(),
        WorkspaceIntelligenceConfig::default(),
    )
    .expect("index");
    assert_eq!(index.files().len(), 5);
    assert!(index.files().values().any(|file| {
        file.symbols
            .iter()
            .any(|symbol| symbol.kind == SymbolKind::Method && symbol.name == "run")
    }));
    let hits = index.search("Greeter run", 10).expect("search");
    assert!(!hits.is_empty());
    assert!(hits[0].inclusion_reason.contains("revision_bound"));
    assert!(!hits[0].source_revision.is_empty());
    assert!(hits[0].range.start_line > 0);
}

#[test]
fn stable_symbol_ids_survive_unchanged_rebuild() {
    let directory = fixture();
    let first = WorkspaceIntelligence::build(
        "workspace-1",
        directory.path(),
        WorkspaceIntelligenceConfig::default(),
    )
    .expect("first");
    let second = WorkspaceIntelligence::build(
        "workspace-1",
        directory.path(),
        WorkspaceIntelligenceConfig::default(),
    )
    .expect("second");
    let ids = |index: &WorkspaceIntelligence| {
        index
            .files()
            .values()
            .flat_map(|file| file.symbols.iter().map(|symbol| symbol.id.clone()))
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(ids(&first), ids(&second));
    assert_eq!(first.workspace_revision(), second.workspace_revision());
}

#[test]
fn changed_file_reindexes_only_it_and_reverse_dependencies() {
    let directory = fixture();
    let mut index = WorkspaceIntelligence::build(
        "workspace-1",
        directory.path(),
        WorkspaceIntelligenceConfig::default(),
    )
    .expect("index");
    write(
        directory.path(),
        "src/util.rs",
        "pub fn greet() { println!(\"changed\"); }\n",
    );
    let report = index
        .update_paths([Path::new("src/util.rs")])
        .expect("update");
    assert!(report.reindexed_files.contains(&"src/util.rs".to_owned()));
    assert!(report.affected_files.contains(&"src/main.rs".to_owned()));
    assert!(!report.reindexed_files.contains(&"worker.py".to_owned()));
}

#[test]
fn watcher_detects_changes_and_snapshot_round_trips() {
    let directory = fixture();
    let mut index = WorkspaceIntelligence::build(
        "workspace-1",
        directory.path(),
        WorkspaceIntelligenceConfig::default(),
    )
    .expect("index");
    let mut watcher = PollingWorkspaceWatcher::new(&index);
    write(
        directory.path(),
        "web/helper.ts",
        "export function helper() { return 2; }\n",
    );
    let report = watcher.poll_once(&mut index).expect("poll");
    assert!(report.changed_files.contains(&"web/helper.ts".to_owned()));
    let snapshot = directory.path().join(".sessionweft/workspace-graph.json");
    index.save_json(&snapshot).expect("save");
    let loaded = WorkspaceIntelligence::load_json(
        &snapshot,
        WorkspaceIntelligenceConfig::default(),
    )
    .expect("load");
    assert_eq!(loaded.workspace_revision(), index.workspace_revision());
    assert_eq!(loaded.edges(), index.edges());
}

#[test]
fn rejects_parent_traversal() {
    let directory = fixture();
    let mut index = WorkspaceIntelligence::build(
        "workspace-1",
        directory.path(),
        WorkspaceIntelligenceConfig::default(),
    )
    .expect("index");
    assert!(matches!(
        index.update_paths([Path::new("../outside.rs")]),
        Err(WorkspaceIntelligenceError::PathEscapesWorkspace(_))
    ));
}
