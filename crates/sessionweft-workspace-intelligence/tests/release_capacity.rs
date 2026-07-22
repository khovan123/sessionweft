use std::{fs, io::Write, time::Instant};

use sessionweft_workspace_intelligence::{WorkspaceIntelligence, WorkspaceIntelligenceConfig};

#[test]
#[ignore = "release capacity profile"]
fn indexes_release_capacity_and_updates_one_dependency_slice() {
    let file_count = std::env::var("SESSIONWEFT_CAPACITY_FILES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(10_000);
    assert!((1..=50_000).contains(&file_count));

    let directory = tempfile::tempdir().expect("tempdir");
    let src = directory.path().join("src");
    fs::create_dir_all(&src).expect("src");
    for index in 0..file_count.saturating_sub(1) {
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
        "release-capacity",
        directory.path(),
        WorkspaceIntelligenceConfig::default(),
    )
    .expect("build");
    let build_elapsed = started.elapsed();
    assert_eq!(intelligence.files().len(), file_count);
    assert!(
        build_elapsed.as_secs() <= 300,
        "index build exceeded five minutes"
    );

    fs::write(src.join("module_1.rs"), "pub fn value_1() -> usize { 2 }\n").expect("change");
    let update_started = Instant::now();
    let update = intelligence
        .update_paths([std::path::Path::new("src/module_1.rs")])
        .expect("incremental update");
    assert!(update.reindexed_files.len() <= 2);
    assert!(update.affected_files.contains(&"src/main.rs".to_owned()));
    assert!(
        update_started.elapsed().as_secs() <= 10,
        "changed-file update exceeded ten seconds"
    );
}
