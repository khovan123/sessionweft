use std::{
    collections::HashMap,
    env, fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) const AGENT: &str = "codex";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeBinding {
    pub(crate) sessionweft_session_id: String,
    pub(crate) agent: String,
    pub(crate) native_session_id: String,
    pub(crate) last_started_at: u64,
    pub(crate) last_ended_at: u64,
}

pub(crate) struct BindingStore {
    path: PathBuf,
    bindings: Vec<NativeBinding>,
}

impl BindingStore {
    pub(crate) fn load(cwd: &Path) -> anyhow::Result<Self> {
        let path = cwd.join(".sessionweft/native-session-bindings.json");
        let bindings = if path.is_file() {
            let value: Value = serde_json::from_slice(&fs::read(&path)?)
                .with_context(|| format!("decode {}", path.display()))?;
            value
                .get("bindings")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(parse_binding).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(Self { path, bindings })
    }

    pub(crate) fn get(&self, session_id: &str) -> Option<&NativeBinding> {
        self.bindings
            .iter()
            .find(|binding| binding.sessionweft_session_id == session_id && binding.agent == AGENT)
    }

    pub(crate) fn upsert(&mut self, binding: NativeBinding) -> anyhow::Result<()> {
        if let Some(existing) = self.bindings.iter_mut().find(|existing| {
            existing.sessionweft_session_id == binding.sessionweft_session_id
                && existing.agent == binding.agent
        }) {
            *existing = binding;
        } else {
            self.bindings.push(binding);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bindings = self.bindings.iter().map(binding_json).collect::<Vec<_>>();
        fs::write(
            &self.path,
            serde_json::to_vec_pretty(&json!({
                "schema_version": 1,
                "bindings": bindings,
            }))?,
        )?;
        Ok(())
    }
}

fn parse_binding(value: &Value) -> Option<NativeBinding> {
    Some(NativeBinding {
        sessionweft_session_id: value.get("sessionweft_session_id")?.as_str()?.to_owned(),
        agent: value.get("agent")?.as_str()?.to_owned(),
        native_session_id: value.get("native_session_id")?.as_str()?.to_owned(),
        last_started_at: value.get("last_started_at")?.as_u64()?,
        last_ended_at: value.get("last_ended_at")?.as_u64()?,
    })
}

fn binding_json(binding: &NativeBinding) -> Value {
    json!({
        "sessionweft_session_id": binding.sessionweft_session_id,
        "agent": binding.agent,
        "native_session_id": binding.native_session_id,
        "last_started_at": binding.last_started_at,
        "last_ended_at": binding.last_ended_at,
    })
}

#[derive(Debug)]
pub(crate) struct CodexSessionRecord {
    pub(crate) id: String,
    pub(crate) path: PathBuf,
    modified: SystemTime,
}

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

pub(crate) fn format_unix_utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
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
        Uuid::parse_str(candidate)
            .ok()
            .map(|_| candidate.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_uuid_is_detected() {
        let path =
            Path::new("rollout-2026-07-30T08-00-00-019fb216-bc5b-7fc1-b0f6-badcb1cba63e.jsonl");
        assert_eq!(
            uuid_from_filename(path).as_deref(),
            Some("019fb216-bc5b-7fc1-b0f6-badcb1cba63e")
        );
    }

    #[test]
    fn unix_time_formats_as_utc_date() {
        assert_eq!(format_unix_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_unix_utc(1_722_470_400), "2024-08-01 00:00 UTC");
    }
}
