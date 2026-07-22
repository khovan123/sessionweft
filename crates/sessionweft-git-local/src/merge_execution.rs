use std::{path::Path, process::Output, time::Duration};

use async_trait::async_trait;
use sessionweft_git::{
    FastForwardOutcome, GitMergeExecutor, GitOperationError, MergeInspection, MergeQueueEntry,
    MergeRecoveryObservation, RebaseOutcome, RollbackOutcome,
};
use tokio::{process::Command, time::timeout};

#[derive(Debug, Clone)]
pub struct GitCliMergeExecutor {
    binary: String,
    command_timeout: Duration,
}

impl Default for GitCliMergeExecutor {
    fn default() -> Self {
        Self {
            binary: "git".into(),
            command_timeout: Duration::from_secs(180),
        }
    }
}

impl GitCliMergeExecutor {
    #[must_use]
    pub fn new(binary: impl Into<String>, command_timeout: Duration) -> Self {
        Self {
            binary: binary.into(),
            command_timeout,
        }
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

    async fn checked<I, S>(&self, arguments: I) -> Result<String, GitOperationError>
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

    async fn source_worktree_path(
        &self,
        entry: &MergeQueueEntry,
    ) -> Result<String, GitOperationError> {
        let listing = self
            .checked([
                "-C",
                entry.repository_root.as_str(),
                "worktree",
                "list",
                "--porcelain",
            ])
            .await?;
        let expected_branch = format!("refs/heads/{}", entry.source_branch);
        for block in listing.split("\n\n") {
            let mut path = None;
            let mut branch = None;
            for line in block.lines() {
                if let Some(value) = line.strip_prefix("worktree ") {
                    path = Some(value.to_owned());
                } else if let Some(value) = line.strip_prefix("branch ") {
                    branch = Some(value.to_owned());
                }
            }
            if branch.as_deref() == Some(expected_branch.as_str()) {
                return path.ok_or_else(|| {
                    GitOperationError::InvalidOutput(
                        "Git worktree registry entry is missing its path".into(),
                    )
                });
            }
        }
        Err(GitOperationError::InvalidOutput(format!(
            "no registered worktree found for source branch {}",
            entry.source_branch
        )))
    }

    async fn target_head(&self, entry: &MergeQueueEntry) -> Result<String, GitOperationError> {
        let reference = format!("refs/heads/{}^{{commit}}", entry.target_branch);
        self.checked([
            "-C",
            entry.repository_root.as_str(),
            "rev-parse",
            "--verify",
            reference.as_str(),
        ])
        .await
    }

    async fn source_head(&self, entry: &MergeQueueEntry) -> Result<String, GitOperationError> {
        let worktree_path = self.source_worktree_path(entry).await?;
        self.checked([
            "-C",
            worktree_path.as_str(),
            "rev-parse",
            "--verify",
            "HEAD^{commit}",
        ])
        .await
    }

    async fn is_ancestor(
        &self,
        entry: &MergeQueueEntry,
        ancestor: &str,
        descendant: &str,
    ) -> Result<bool, GitOperationError> {
        let output = self
            .run([
                "-C",
                entry.repository_root.as_str(),
                "merge-base",
                "--is-ancestor",
                ancestor,
                descendant,
            ])
            .await?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(command_error(&output)),
        }
    }

    async fn rebase_state_exists(&self, entry: &MergeQueueEntry) -> Result<bool, GitOperationError> {
        let worktree_path = self.source_worktree_path(entry).await?;
        for state in ["rebase-merge", "rebase-apply"] {
            let path = self
                .checked([
                    "-C",
                    worktree_path.as_str(),
                    "rev-parse",
                    "--git-path",
                    state,
                ])
                .await?;
            if Path::new(&path).exists() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn abort_rebase(&self, entry: &MergeQueueEntry) -> Result<(), GitOperationError> {
        let worktree_path = self.source_worktree_path(entry).await?;
        let output = self
            .run([
                "-C",
                worktree_path.as_str(),
                "rebase",
                "--abort",
            ])
            .await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(command_error(&output))
        }
    }
}

#[async_trait]
impl GitMergeExecutor for GitCliMergeExecutor {
    async fn inspect(&self, entry: &MergeQueueEntry) -> Result<MergeInspection, GitOperationError> {
        let target_commit = self.target_head(entry).await?;
        let source_commit = self.source_head(entry).await?;
        let worktree_path = self.source_worktree_path(entry).await?;
        let status = self
            .checked([
                "-C",
                worktree_path.as_str(),
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
            ])
            .await?;
        Ok(MergeInspection {
            target_commit,
            source_commit,
            worktree_clean: status.is_empty(),
        })
    }

    async fn rebase(
        &self,
        entry: &MergeQueueEntry,
        target_commit: &str,
    ) -> Result<RebaseOutcome, GitOperationError> {
        let worktree_path = self.source_worktree_path(entry).await?;
        let output = self
            .run([
                "-C",
                worktree_path.as_str(),
                "rebase",
                target_commit,
            ])
            .await?;
        if output.status.success() {
            return Ok(RebaseOutcome::Rebased {
                target_commit: target_commit.to_owned(),
                head_commit: self.source_head(entry).await?,
            });
        }
        let conflicts = self
            .checked([
                "-C",
                worktree_path.as_str(),
                "diff",
                "--name-only",
                "--diff-filter=U",
            ])
            .await?
            .lines()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if conflicts.is_empty() {
            return Err(command_error(&output));
        }
        self.abort_rebase(entry).await?;
        Ok(RebaseOutcome::Conflict {
            target_commit: target_commit.to_owned(),
            paths: conflicts,
        })
    }

    async fn fast_forward(
        &self,
        entry: &MergeQueueEntry,
        expected_target: &str,
        source_head: &str,
    ) -> Result<FastForwardOutcome, GitOperationError> {
        let actual = self.target_head(entry).await?;
        if actual != expected_target {
            return Ok(FastForwardOutcome::TargetMoved {
                actual_target: actual,
            });
        }
        if !self.is_ancestor(entry, expected_target, source_head).await? {
            return Err(GitOperationError::Command(
                "source head is not a fast-forward descendant of target".into(),
            ));
        }
        let reference = format!("refs/heads/{}", entry.target_branch);
        let output = self
            .run([
                "-C",
                entry.repository_root.as_str(),
                "update-ref",
                reference.as_str(),
                source_head,
                expected_target,
            ])
            .await?;
        if output.status.success() {
            return Ok(FastForwardOutcome::Applied {
                merge_commit: source_head.to_owned(),
            });
        }
        let current = self.target_head(entry).await?;
        if current != expected_target {
            Ok(FastForwardOutcome::TargetMoved {
                actual_target: current,
            })
        } else {
            Err(command_error(&output))
        }
    }

    async fn rollback(
        &self,
        entry: &MergeQueueEntry,
        expected_current: &str,
        restore_target: &str,
    ) -> Result<RollbackOutcome, GitOperationError> {
        let actual = self.target_head(entry).await?;
        if actual != expected_current {
            return Ok(RollbackOutcome::TargetMoved {
                actual_target: actual,
            });
        }
        let reference = format!("refs/heads/{}", entry.target_branch);
        let output = self
            .run([
                "-C",
                entry.repository_root.as_str(),
                "update-ref",
                reference.as_str(),
                restore_target,
                expected_current,
            ])
            .await?;
        if output.status.success() {
            Ok(RollbackOutcome::Applied)
        } else {
            let current = self.target_head(entry).await?;
            if current != expected_current {
                Ok(RollbackOutcome::TargetMoved {
                    actual_target: current,
                })
            } else {
                Err(command_error(&output))
            }
        }
    }

    async fn recover(
        &self,
        entry: &MergeQueueEntry,
    ) -> Result<MergeRecoveryObservation, GitOperationError> {
        let worktree_path = match self.source_worktree_path(entry).await {
            Ok(path) => path,
            Err(GitOperationError::InvalidOutput(_)) => {
                return Ok(MergeRecoveryObservation::MissingWorktree);
            }
            Err(error) => return Err(error),
        };
        if tokio::fs::metadata(&worktree_path).await.is_err() {
            return Ok(MergeRecoveryObservation::MissingWorktree);
        }
        let target_commit = self.target_head(entry).await?;
        if self.rebase_state_exists(entry).await? {
            self.abort_rebase(entry).await?;
            return Ok(MergeRecoveryObservation::InterruptedRebase {
                target_commit,
                source_commit: self.source_head(entry).await?,
            });
        }
        let source_commit = self.source_head(entry).await?;
        if target_commit == entry.head_commit {
            return Ok(MergeRecoveryObservation::Merged {
                merge_commit: target_commit,
            });
        }
        if source_commit != entry.head_commit
            && self
                .is_ancestor(entry, &target_commit, &source_commit)
                .await?
        {
            return Ok(MergeRecoveryObservation::Rebased {
                target_commit,
                head_commit: source_commit,
            });
        }
        Ok(MergeRecoveryObservation::Diverged {
            target_commit,
            source_commit,
        })
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
