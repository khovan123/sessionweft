use std::{
    collections::HashMap,
    env, fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub(crate) const AGENT: &str = "codex";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct NativeBinding {
    pub(crate) sessionweft_session_id: String,
    pub(crate) agent: String,
    pub(crate) native_session_id: String,
    pub(crate) last_started_at: String,
    pub(crate) last_ended_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct BindingFile {
    #[serde(default = "binding_schema_version")]
    schema_version: u32,
    #[serde(default)]
    bindings: Vec<NativeBinding>,
}

impl Default for BindingFile {
    fn default() -> Self {
        Self {
            schema_version: binding_schema_version(),
            bindings: Vec::new(),
        }
    }
}

const fn binding_schema_version() -> u32 {
    1
}

pub(crate) struct BindingStore {
    path: PathBuf,
    file: BindingFile,
}

impl BindingStore {
    pub(crate) fn load(cwd: &Path) -> anyhow::Result<Self> {
        let path = cwd.join(".sessionweft/native-session-bindings.json");
        let file = if path.is_file() {
            serde_json::from_slice(&fs::read(&path)?)
                .with_context(|| format!("decode {}", path.display()))?
        } else {
            BindingFile::default()
        };
        Ok(Self { path, file })
    }

    pub(crate) fn get(&self, session_id: &str) -> Option<&NativeBinding> {
        self.file.bindings.iter().find(|binding| {
            binding.sessionweft_session_id == session_id && binding.agent == AGENT
        })
    }

    pub(crate) fn upsert(&mut self, binding: NativeBinding) -> anyhow::Result<()> {
        if let Some(existing) = self.file.bindings.iter_mut().find(|existing| {
            existing.sessionweft_session_id == binding.sessionweft_session_id
                && existing.agent == binding.agent
        }) {
            *existing = binding;
        } else {
            self.file.bindings.push(binding);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, serde_json::to_vec_pretty(&self.file)?)?;
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct CodexSessionRecord {
    pub(crate) id: String,
    pub(crate) path: PathBuf,
    modified: SystemTime,
}

pub(crate) fn snapshot(cwd: &Path) -> HashMap<PathBuf, SystemTime> {
    discover_records(cwd)
        .into_iter()
        .map(|record| (record.path, record.modified))
        .collect()
}

pub(crate) fn discover_new(
    cwd: &Path,
    before: &HashMap<PathBuf, SystemTime>,
) -> Option<CodexSessionRecord> {
    discover_records(cwd)
        .into_iter()
        .filter(|record| {
            before
                .get(&record.path)
                .is_none_or(|modified| record.modified > *modified)
        })
        .max_by_key(|record| record.modified)
}

pub(crate) fn find_by_id(cwd: &Path, native_session_id: &str) -> Option<CodexSessionRecord> {
    discover_records(cwd)
        .into_iter()
        .find(|record| record.id == native_session_id)
}

pub(crate) fn validate_uuid(value: &str) -> anyhow::Result<()> {
    if Uuid::parse_str(value).is_err() {
        bail!("native Codex session id '{value}' is not a UUID");
    }
    Ok(())
}

fn discover_records(cwd: &Path) -> Vec<CodexSessionRecord> {
    let Some(root) = sessions_root() else {
        return Vec::new();
    };
    let mut files = Vec::new();
    collect_jsonl_files(&root, &mut files);
    files
        .into_iter()
        .filter_map(|path| parse_record(&path, cwd))
        .collect()
}

fn sessions_root() -> Option<PathBuf> {
    if let Some(home) = env::var_os("CODEX_HOME") {
        return Some(PathBuf::from(home).join("sessions"));
    }
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex/sessions"))
}

fn collect_jsonl_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
}

fn parse_record(path: &Path, cwd: &Path) -> Option<CodexSessionRecord> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let file = fs::File::open(path).ok()?;
    let canonical_cwd = fs::canonicalize(cwd).ok()?;
    let mut metadata_id = None;
    let mut cwd_matches = None;

    for line in BufReader::new(file).lines().take(32).flatten() {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let payload = value.get("payload").unwrap_or(&value);
        if metadata_id.is_none() {
            metadata_id = payload
                .get("id")
                .or_else(|| payload.get("session_id"))
                .and_then(Value::as_str)
                .filter(|value| Uuid::parse_str(value).is_ok())
                .map(str::to_owned);
        }
        if cwd_matches.is_none()
            && let Some(recorded_cwd) = payload.get("cwd").and_then(Value::as_str)
        {
            cwd_matches = fs::canonicalize(recorded_cwd)
                .ok()
                .map(|recorded_cwd| recorded_cwd == canonical_cwd);
        }
        if metadata_id.is_some() && cwd_matches.is_some() {
            break;
        }
    }

    if cwd_matches == Some(false) {
        return None;
    }
    Some(CodexSessionRecord {
        id: metadata_id.or_else(|| uuid_from_filename(path))?,
        path: path.to_owned(),
        modified,
    })
}

fn uuid_from_filename(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    if name.len() < 36 {
        return None;
    }
    (0..=name.len() - 36).find_map(|start| {
        let candidate = name.get(start..start + 36)?;
        Uuid::parse_str(candidate).ok().map(|_| candidate.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_uuid_is_detected() {
        let path = Path::new(
            "rollout-2026-07-30T08-00-00-019fb216-bc5b-7fc1-b0f6-badcb1cba63e.jsonl",
        );
        assert_eq!(
            uuid_from_filename(path).as_deref(),
            Some("019fb216-bc5b-7fc1-b0f6-badcb1cba63e")
        );
    }
}
