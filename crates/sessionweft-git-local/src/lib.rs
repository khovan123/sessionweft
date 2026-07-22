use std::{path::Path, process::Output, time::Duration};

use async_trait::async_trait;
use sessionweft_git::{GitOperationError, GitWorktreeProvisioner, GitWorktreeRecord};
use tokio::{process::Command, time::timeout};

#[derive(Debug, Clone)]
pub struct GitCliWorktreeProvisioner {
    binary: String,
    command_timeout: Duration,
}

impl Default for GitCliWorktreeProvisioner {
    fn default() -> Self {
        Self {
            binary: "git".into(),
            command_timeout: Duration::from_secs(120),
        }
    }
}

impl GitCliWorktreeProvisioner {
    #[must_use]
    pub fn new(binary: impl Into<String>, command_timeout: Duration) -> Self {
        Self {
            binary: binary.into(),
            command_timeout,
        }
    }

    async fn run_checked<I, S>(&self, arguments: I) -> Result<String, GitOperationError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = self.run(arguments).await?;
        if !output.status.success() {
            return Err(command_error(&output));
        }
        String::from_utf8(output.stdout)
            .map(|value| value.trim().to_owned())
            .map_err(|error| GitOperationError::InvalidOutput(error.to_string()))
    }

    async fn run_status<I, S>(&self, arguments: I) -> Result<bool, GitOperationError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        Ok(self.run(arguments).await?.status.success())
    }

    async fn run<I, S>(&self, arguments: I) -> Result<Output, GitOperationError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut command = Command::new(&self.binary);
        command.args(arguments);
        timeout(self.command_timeout, command.output())
            .await
            .map_err(|_| GitOperationError::Command("Git command timed out".into()))?
            .map_err(|error| GitOperationError::Command(error.to_string()))
    }

    async fn branch_exists(&self, record: &GitWorktreeRecord) -> Result<bool, GitOperationError> {
        self.run_status([
            "-C",
            record.repository_root.as_str(),
            "show-ref",
            "--verify",
            "--quiet",
            format!("refs/heads/{}", record.branch_name).as_str(),
        ])
        .await
    }
}

#[async_trait]
impl GitWorktreeProvisioner for GitCliWorktreeProvisioner {
    async fn create(&self, record: &GitWorktreeRecord) -> Result<String, GitOperationError> {
        let revision = format!("{}^{{commit}}", record.base_commit);
        let verified = self
            .run_checked([
                "-C",
                record.repository_root.as_str(),
                "rev-parse",
                "--verify",
                revision.as_str(),
            ])
            .await?;
        if Path::new(&record.worktree_path).exists() {
            return Err(GitOperationError::Command(format!(
                "worktree path already exists: {}",
                record.worktree_path
            )));
        }
        self.run_checked([
            "-C",
            record.repository_root.as_str(),
            "worktree",
            "add",
            "-b",
            record.branch_name.as_str(),
            record.worktree_path.as_str(),
            verified.as_str(),
        ])
        .await?;
        self.inspect_head(record).await?.ok_or_else(|| {
            GitOperationError::InvalidOutput("created worktree has no readable HEAD".into())
        })
    }

    async fn inspect_head(
        &self,
        record: &GitWorktreeRecord,
    ) -> Result<Option<String>, GitOperationError> {
        if tokio::fs::metadata(&record.worktree_path).await.is_err() {
            return Ok(None);
        }
        let head = self
            .run_checked([
                "-C",
                record.worktree_path.as_str(),
                "rev-parse",
                "HEAD",
            ])
            .await?;
        if head.is_empty() {
            return Err(GitOperationError::InvalidOutput(
                "worktree HEAD is empty".into(),
            ));
        }
        Ok(Some(head))
    }

    async fn remove(&self, record: &GitWorktreeRecord) -> Result<(), GitOperationError> {
        if tokio::fs::metadata(&record.worktree_path).await.is_ok() {
            self.run_checked([
                "-C",
                record.repository_root.as_str(),
                "worktree",
                "remove",
                "--force",
                record.worktree_path.as_str(),
            ])
            .await?;
        }
        if self.branch_exists(record).await? {
            self.run_checked([
                "-C",
                record.repository_root.as_str(),
                "branch",
                "-D",
                record.branch_name.as_str(),
            ])
            .await?;
        }
        Ok(())
    }
}

fn command_error(output: &Output) -> GitOperationError {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let details = if stderr.is_empty() { stdout } else { stderr };
    GitOperationError::Command(if details.is_empty() {
        format!("Git command exited with {}", output.status)
    } else {
        details
    })
}

#[cfg(test)]
mod tests {
    use chrono::{Duration as ChronoDuration, Utc};
    use sessionweft_core::SessionId;
    use sessionweft_git::{
        GitFence, GitWorktreeProvisioner, GitWorktreeRecord, WorktreeAllocationRequest,
    };
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    async fn creates_inspects_and_removes_real_worktree() {
        let root = std::env::temp_dir().join(format!("sessionweft-git-root-{}", Uuid::new_v4()));
        let worktree =
            std::env::temp_dir().join(format!("sessionweft-git-worker-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root)
            .await
            .expect("repository directory");
        run_git(&root, &["init"]).await;
        run_git(&root, &["config", "user.email", "sessionweft@example.invalid"]).await;
        run_git(&root, &["config", "user.name", "SessionWeft Test"]).await;
        tokio::fs::write(root.join("README.md"), "SessionWeft\n")
            .await
            .expect("write fixture");
        run_git(&root, &["add", "README.md"]).await;
        run_git(&root, &["commit", "-m", "initial"]).await;
        let base = git_output(&root, &["rev-parse", "HEAD"]).await;

        let record = GitWorktreeRecord::new(
            WorktreeAllocationRequest {
                session_id: SessionId::new(),
                claim_id: Uuid::new_v4(),
                agent_id: Uuid::new_v4(),
                workspace_id: "workspace".into(),
                repository_root: root.display().to_string(),
                branch_name: format!("sessionweft/test-{}", Uuid::new_v4()),
                worktree_path: worktree.display().to_string(),
                base_commit: base.clone(),
                fence: GitFence {
                    lock_id: Uuid::new_v4(),
                    fencing_token: 1,
                    expires_at: Utc::now() + ChronoDuration::minutes(5),
                },
            },
            Utc::now(),
        )
        .expect("record");
        let provisioner = GitCliWorktreeProvisioner::default();
        let head = provisioner.create(&record).await.expect("create worktree");
        assert_eq!(head, base);
        assert_eq!(
            provisioner
                .inspect_head(&record)
                .await
                .expect("inspect")
                .as_deref(),
            Some(base.as_str())
        );
        provisioner.remove(&record).await.expect("remove worktree");
        assert!(tokio::fs::metadata(&worktree).await.is_err());
        provisioner
            .remove(&record)
            .await
            .expect("idempotent remove");
        let _ = tokio::fs::remove_dir_all(&root).await;
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
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("UTF-8")
            .trim()
            .to_owned()
    }
}
