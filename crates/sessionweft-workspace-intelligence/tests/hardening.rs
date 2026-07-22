use std::{fs, io::Write, path::Path, time::{Duration, Instant}};

use sessionweft_workspace_intelligence::{
    WorkspaceIntelligence, WorkspaceIntelligenceConfig,
};

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("directories");
    let mut file = fs::File::create(path).expect("file");
    file.write_all(content.as_bytes()).expect("content");
}

#[test]
#[ignore = "capacity gate executed by production-hardening workflow"]
fn indexes_one_thousand_files_and_keeps_incremental_work_bounded() {
    let directory = tempfile::tempdir().expect("tempdir");
    for index in 0..1_000 {
        write(
            directory.path(),
            &format!("src/module_{index}.rs"),
            &format!(
                "pub struct Value{index};\nimpl Value{index} {{ pub fn read(&self) -> usize {{ {index} }} }}\n"
            ),
        );
    }
    write(
        directory.path(),
        "src/main.rs",
        "mod module_1;\nuse crate::module_1::Value1;\nfn main() { let _ = Value1.read(); }\n",
    );

    let started = Instant::now();
    let mut intelligence = WorkspaceIntelligence::build(
        "hardening-capacity",
        directory.path(),
        WorkspaceIntelligenceConfig {
            max_files: 2_000,
            ..Default::default()
        },
    )
    .expect("workspace build");
    assert!(
        started.elapsed() <= Duration::from_secs(45),
        "one-thousand-file build exceeded RC capacity target"
    );
    assert_eq!(intelligence.files().len(), 1_001);

    write(
        directory.path(),
        "src/module_1.rs",
        "pub struct Value1;\nimpl Value1 { pub fn read(&self) -> usize { 2 } }\n",
    );
    let update_started = Instant::now();
    let report = intelligence
        .update_paths([Path::new("src/module_1.rs")])
        .expect("incremental update");
    assert!(
        update_started.elapsed() <= Duration::from_secs(5),
        "changed-file update exceeded RC target"
    );
    assert!(report.reindexed_files.len() <= 2);
    assert!(report.affected_files.contains(&"src/main.rs".to_owned()));

    let search_started = Instant::now();
    for _ in 0..100 {
        let hits = intelligence.search("Value1 read", 20).expect("search");
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|hit| hit.estimated_tokens > 0));
    }
    assert!(
        search_started.elapsed() <= Duration::from_secs(10),
        "one hundred context searches exceeded RC target"
    );
}
