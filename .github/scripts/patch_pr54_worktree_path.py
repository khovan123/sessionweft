from pathlib import Path

queue = Path("crates/sessionweft-git/src/merge_queue.rs")
text = queue.read_text()
text = text.replace(
    "pub const GIT_MERGE_QUEUE_SCHEMA_VERSION: u32 = 1;",
    "pub const GIT_MERGE_QUEUE_SCHEMA_VERSION: u32 = 2;",
    1,
)
field_marker = "    pub repository_root: String,\n    pub source_branch: String,\n"
field_replacement = (
    "    pub repository_root: String,\n"
    "    pub worktree_path: String,\n"
    "    pub source_branch: String,\n"
)
if field_replacement not in text:
    if field_marker not in text:
        raise SystemExit("merge queue worktree field marker not found")
    text = text.replace(field_marker, field_replacement, 1)
initializer_marker = (
    "            repository_root: worktree.repository_root.clone(),\n"
    "            source_branch: worktree.branch_name.clone(),\n"
)
initializer_replacement = (
    "            repository_root: worktree.repository_root.clone(),\n"
    "            worktree_path: worktree.worktree_path.clone(),\n"
    "            source_branch: worktree.branch_name.clone(),\n"
)
if initializer_replacement not in text:
    if initializer_marker not in text:
        raise SystemExit("merge queue worktree initializer marker not found")
    text = text.replace(initializer_marker, initializer_replacement, 1)
queue.write_text(text)

executor = Path("crates/sessionweft-git-local/src/merge_execution.rs")
text = executor.read_text()
start_marker = "    async fn source_worktree_path(\n"
end_marker = (
    "    async fn target_head(&self, entry: &MergeQueueEntry) "
    "-> Result<String, GitOperationError> {\n"
)
start = text.find(start_marker)
end = text.find(end_marker, start)
if start < 0 or end < 0:
    raise SystemExit("source_worktree_path function boundaries not found")
replacement = '''    async fn source_worktree_path(
        &self,
        entry: &MergeQueueEntry,
    ) -> Result<String, GitOperationError> {
        let worktree_path = entry.worktree_path.trim();
        if worktree_path.is_empty() {
            return Err(GitOperationError::InvalidOutput(
                "merge queue entry is missing its durable worktree path".into(),
            ));
        }
        let actual_branch = self
            .checked([
                "-C",
                worktree_path,
                "symbolic-ref",
                "--quiet",
                "HEAD",
            ])
            .await?;
        let expected_branch = format!("refs/heads/{}", entry.source_branch);
        if actual_branch != expected_branch {
            return Err(GitOperationError::InvalidOutput(format!(
                "worktree path {worktree_path} is checked out at {actual_branch}, expected {expected_branch}"
            )));
        }
        Ok(worktree_path.to_owned())
    }

'''
text = text[:start] + replacement + text[end:]
executor.write_text(text)

tests = Path("crates/sessionweft-git-local/src/merge_execution_tests.rs")
text = tests.read_text()
start_marker = '    println!(\n        "worktree registry:\\n{}",\n'
start = text.find(start_marker)
if start >= 0:
    end_marker = "\n\n    match executor\n"
    end = text.find(end_marker, start)
    if end < 0:
        raise SystemExit("diagnostic print block end not found")
    text = text[:start] + "    match executor\n" + text[end + len(end_marker):]
tests.write_text(text)
