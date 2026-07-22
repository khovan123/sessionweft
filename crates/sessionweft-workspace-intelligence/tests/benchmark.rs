use std::{fs, io::Write, time::Instant};

use sessionweft_workspace_intelligence::{
    WorkspaceIntelligence, WorkspaceIntelligenceConfig,
};

#[test]
fn changed_file_work_is_bounded_by_dependency_slice() {
    let directory = tempfile::tempdir().expect("tempdir");
    let src = directory.path().join("src");
    fs::create_dir_all(&src).expect("src");
    for index in 0..250 {
        let mut file = fs::File::create(src.join(format!("module_{index}.rs"))).expect("file");
        writeln!(file, "pub fn value_{index}() -> usize {{ {index} }}").expect("write");
    }
    fs::write(
        src.join("main.rs"),
        "mod module_1;\nuse crate::module_1::value_1;\nfn main() { let _ = value_1(); }\n",
    )
    .expect("main");
    let started = Instant::now();
    let mut intelligence = WorkspaceIntelligence::build(
        "benchmark",
        directory.path(),
        WorkspaceIntelligenceConfig::default(),
    )
    .expect("build");
    assert!(started.elapsed().as_secs() < 30);
    fs::write(
        src.join("module_1.rs"),
        "pub fn value_1() -> usize { 2 }\n",
    )
    .expect("change");
    let update = intelligence
        .update_paths([std::path::Path::new("src/module_1.rs")])
        .expect("incremental update");
    assert!(update.reindexed_files.len() <= 2);
    assert!(update.affected_files.contains(&"src/main.rs".to_owned()));
}
