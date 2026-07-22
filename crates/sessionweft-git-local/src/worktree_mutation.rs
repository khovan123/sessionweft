use std::{ffi::OsString, process::Output, time::Duration};

use async_trait::async_trait;
use sessionweft_git::{
    GitOperationError, GitWorktreeCommitter, GitWorktreeRecord,
};
use tokio::{process::Command, time::timeout};

#[derive(Debug, Clone)]
pub struct GitCliWorktreeCommitter {
    binary: String,
    command_timeout: Duration,
}

impl Default for GitCliWorktreeCommitter {
    fn default() -> Self {
        Self {
            binary: "git".into(),
            command_timeout: Duration::from_secs(120),
        }
    }
}

impl GitCliWorktreeCommitter {
    #[must_use]
    pub fn new(binary: impl Into<String>, command_timeout: Duration) -> Self {
        Self {
            binary: binary.into(),
            command_timeout,
        }
    }

    async fn run(&self, arguments: Vec<OsString>) -> Result<Output, GitOperationError> {
        let mut command = Command::new(&self.binary);
        command.args(arguments);
        timeout(self.command_timeout, command.output())
            .await
            .map_err(|_| GitOperationError::Command("Git command timed out".into()))?
            .map_err(|error| GitOperationError::Command(error.to_string()))
    }

    async fn checked(&self, arguments: Vec<OsString>) -> Result<String, GitOperationError> {
        let output = self.run(arguments).await?;
        if !output.status.success() {
            return Err(command_error(&output));
        }
        String::from_utf8(output.stdout)
            .map(|value| value.trim().to_owned())
            .map_err(|error| GitOperationError::InvalidOutput(error.to_string()))
    }

    async fn index_has_changes(
        &self,
        worktree: &GitWorktreeRecord,
    ) -> Result<bool, GitOperationError> {
        let output = self
            .run(args([
                "-C",
                worktree.worktree_path.as_str(),
                "diff",
                "--cached",
                "--quiet",
                "--exit-code",
            ]))
            .await?;
        match output.status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(command_error(&output)),
        }
    }

    async fn current_head(
        &self,
        worktree: &GitWorktreeRecord,
    ) -> Result<String, GitOperationError> {
        self.checked(args([
            "-C",
            worktree.worktree_path.as_str(),
            "rev-parse",
            "--verify",
            "HEAD^{commit}",
        ]))
        .await
    }
}

#[async_trait]
impl GitWorktreeCommitter for GitCliWorktreeCommitter {
    async fn stage(
        &self,
        worktree: &GitWorktreeRecord,
        paths: &[String],
    ) -> Result<(), GitOperationError> {
        if self.index_has_changes(worktree).await? {
            return Err(GitOperationError::Command(
                "worktree index already contains staged changes".into(),
            ));
        }
        let mut arguments = args([
            "-C",
            worktree.worktree_path.as_str(),
            "add",
            "--",
        ]);
        arguments.extend(paths.iter().map(OsString::from));
        let output = self.run(arguments).await?;
        if !output.status.success() {
            return Err(command_error(&output));
        }
        if !self.index_has_changes(worktree).await? {
            return Err(GitOperationError::Command(
                "declared paths did not produce staged changes".into(),
            ));
        }
        Ok(())
    }

    async fn unstage(
        &self,
        worktree: &GitWorktreeRecord,
        paths: &[String],
    ) -> Result<(), GitOperationError> {
        let mut arguments = args([
            "-C",
            worktree.worktree_path.as_str(),
            "reset",
            "--",
        ]);
        arguments.extend(paths.iter().map(OsString::from));
        let output = self.run(arguments).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(command_error(&output))
        }
    }

    async fn commit(
        &self,
        worktree: &GitWorktreeRecord,
        message: &str,
    ) -> Result<String, GitOperationError> {
        if !self.index_has_changes(worktree).await? {
            return Err(GitOperationError::Command(
                "worktree has no staged changes to commit".into(),
            ));
        }
        let output = self
            .run(args([
                "-C",
                worktree.worktree_path.as_str(),
                "commit",
                "-m",
                message,
            ]))
            .await?;
        if !output.status.success() {
            return Err(command_error(&output));
        }
        self.current_head(worktree).await
    }

    async fn rollback_commit(
        &self,
        worktree: &GitWorktreeRecord,
        expected_current: &str,
        restore_head: &str,
    ) -> Result<(), GitOperationError> {
        let current = self.current_head(worktree).await?;
        if current != expected_current {
            return Err(GitOperationError::Command(format!(
                "cannot rollback commit because HEAD moved to {current}"
            )));
        }
        let output = self
            .run(args([
                "-C",
                worktree.worktree_path.as_str(),
                "reset",
                "--mixed",
                restore_head,
            ]))
            .await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(command_error(&output))
        }
    }
}

fn args<const N: usize>(values: [&str; N]) -> Vec<OsString> {
    values.into_iter().map(OsString::from).collect()
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
