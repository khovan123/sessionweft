use std::path::Path;

use chrono::{Duration as ChronoDuration, Utc};
use sessionweft_core::SessionId;
use sessionweft_git::{
    FastForwardOutcome, GitFence, GitMergeExecutor, GitWorktreeRecord, MergeQueueEntry,
    MergeQueueRequest, MergeRecoveryObservation, RebaseOutcome, RollbackOutcome,
    WorktreeAllocationRequest,
};
use tokio::{process::Command, sync::Mutex};
use uuid::Uuid;

use super::GitCliMergeExecutor;

static GIT_MERGE_TEST_LOCK: Mutex<()> = Mutex::const_new(());

struct RepositoryFixture {
    root: std::path::PathBuf,
    worktree: std::path::PathBuf,
    branch: String,
    initial: String,
}

impl RepositoryFixture {
    async fn new(shared_content: &str) -> Self {
        let root = std::env::temp_dir().join(format!("sessionweft-merge-root-{}", Uuid::new_v4()));
        let worktree =
            std::env::temp_dir().join(format!("sessionweft-merge-source-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root)
            .await
            .expect("repository directory");
        run_git(&root, &["init", "-b", "main"]).await;
        run_git(
            &root,
            &["config", "user.email", "sessionweft@example.invalid"],
        )
        .await;
        run_git(&root, &["config", "user.name", "SessionWeft Test"]).await;
        tokio::fs::write(root.join("shared.txt"), shared_content)
            .await
            .expect("write initial file");
        run_git(&root, &["add", "shared.txt"]).await;
        run_git(&root, &["commit", "-m", "initial"]).await;
        let initial = git_output(&root, &["rev-parse", "HEAD"]).await;
        let branch = format!("sessionweft/test-{}", Uuid::new_v4());
        run_git(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                branch.as_str(),
                worktree.to_str().expect("worktree path"),
                initial.as_str(),
            ],
        )
        .await;
        Self {
            root,
            worktree,
            branch,
            initial,
        }
    }

    async fn source_commit(&self, path: &str, content: &str, message: &str) -> String {
        tokio::fs::write(self.worktree.join(path), content)
            .await
            .expect("write source file");
        run_git(&self.worktree, &["add", path]).await;
        run_git(&self.worktree, &["commit", "-m", message]).await;
        git_output(&self.worktree, &["rev-parse", "HEAD"]).await
    }

    async fn target_commit(&self, path: &str, content: &str, message: &str) -> String {
        tokio::fs::write(self.root.join(path), content)
            .await
            .expect("write target file");
        run_git(&self.root, &["add", path]).await;
        run_git(&self.root, &["commit", "-m", message]).await;
        git_output(&self.root, &["rev-parse", "HEAD"]).await
    }

    fn queue_entry(&self, source_head: &str) -> MergeQueueEntry {
        let now = Utc::now();
        let mut worktree = GitWorktreeRecord::new(
            WorktreeAllocationRequest {
                session_id: SessionId::new(),
                claim_id: Uuid::new_v4(),
                agent_id: Uuid::new_v4(),
                workspace_id: "workspace".into(),
                repository_root: self.root.display().to_string(),
                branch_name: self.branch.clone(),
                worktree_path: self.worktree.display().to_string(),
                base_commit: self.initial.clone(),
                fence: GitFence {
                    lock_id: Uuid::new_v4(),
                    fencing_token: 1,
                    expires_at: now + ChronoDuration::minutes(5),
                },
            },
            now,
        )
        .expect("worktree record");
        worktree
            .mark_ready(source_head, now)
            .expect("ready worktree");
        MergeQueueEntry::new(
            MergeQueueRequest {
                worktree_id: worktree.id,
                target_branch: "main".into(),
                priority: 0,
            },
            &worktree,
            1,
            now,
        )
        .expect("queue entry")
    }

    async fn cleanup(self) {
        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args([
                "worktree",
                "remove",
                "--force",
                self.worktree.to_str().expect("worktree path"),
            ])
            .output()
            .await;
        let _ = tokio::fs::remove_dir_all(&self.worktree).await;
        let _ = tokio::fs::remove_dir_all(&self.root).await;
    }
}

#[tokio::test]
async fn rebases_fast_forwards_recovers_and_rolls_back() {
    let _guard = GIT_MERGE_TEST_LOCK.lock().await;
    let fixture = RepositoryFixture::new("base\n").await;
    let source_head = fixture
        .source_commit("source.txt", "source\n", "source change")
        .await;
    let target_head = fixture
        .target_commit("target.txt", "target\n", "target change")
        .await;
    let entry = fixture.queue_entry(&source_head);
    let executor = GitCliMergeExecutor::default();

    let inspection = executor.inspect(&entry).await.expect("inspect");
    assert_eq!(inspection.target_commit, target_head);
    assert_eq!(inspection.source_commit, source_head);
    assert!(inspection.worktree_clean);

    let rebased_head = match executor
        .rebase(&entry, &inspection.target_commit)
        .await
        .expect("rebase")
    {
        RebaseOutcome::Rebased {
            target_commit,
            head_commit,
        } => {
            assert_eq!(target_commit, target_head);
            assert_ne!(head_commit, source_head);
            head_commit
        }
        RebaseOutcome::Conflict { paths, .. } => panic!("unexpected conflict: {paths:?}"),
    };

    assert!(matches!(
        executor
            .fast_forward(&entry, &target_head, &rebased_head)
            .await
            .expect("fast forward"),
        FastForwardOutcome::Applied { ref merge_commit } if merge_commit == &rebased_head
    ));
    assert_eq!(
        git_output(&fixture.root, &["rev-parse", "main"]).await,
        rebased_head
    );

    let mut recovered_entry = entry.clone();
    recovered_entry.head_commit = rebased_head.clone();
    assert!(matches!(
        executor.recover(&recovered_entry).await.expect("recover"),
        MergeRecoveryObservation::Merged { ref merge_commit } if merge_commit == &rebased_head
    ));

    assert_eq!(
        executor
            .rollback(&entry, &rebased_head, &target_head)
            .await
            .expect("rollback"),
        RollbackOutcome::Applied
    );
    assert_eq!(
        git_output(&fixture.root, &["rev-parse", "main"]).await,
        target_head
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn conflicting_rebase_returns_paths_and_aborts_cleanly() {
    let _guard = GIT_MERGE_TEST_LOCK.lock().await;
    let fixture = RepositoryFixture::new("base\n").await;
    let source_head = fixture
        .source_commit("shared.txt", "source\n", "source conflict")
        .await;
    let target_head = fixture
        .target_commit("shared.txt", "target\n", "target conflict")
        .await;
    let entry = fixture.queue_entry(&source_head);
    let executor = GitCliMergeExecutor::default();

    match executor
        .rebase(&entry, &target_head)
        .await
        .expect("conflict outcome")
    {
        RebaseOutcome::Conflict {
            target_commit,
            paths,
        } => {
            assert_eq!(target_commit, target_head);
            assert_eq!(paths, vec!["shared.txt"]);
        }
        RebaseOutcome::Rebased { head_commit, .. } => {
            panic!("unexpected successful rebase: {head_commit}")
        }
    }
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).await,
        source_head
    );
    assert!(
        git_output(&fixture.worktree, &["status", "--porcelain"])
            .await
            .is_empty()
    );
    fixture.cleanup().await;
}

async fn run_git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .await
        .expect("run git");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn git_output(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .await
        .expect("run git");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("UTF-8")
        .trim()
        .to_owned()
}
