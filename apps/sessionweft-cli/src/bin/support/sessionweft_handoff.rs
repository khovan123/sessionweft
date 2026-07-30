use std::{
    fs,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use anyhow::{Context, ensure};
use serde_json::Value;

const MAX_ROLLOUT_TAIL_BYTES: u64 = 8 * 1024 * 1024;
const MAX_HANDOFF_CHARACTERS: usize = 256 * 1024;
const MAX_HANDOFF_MESSAGES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
struct VisibleMessage {
    role: String,
    text: String,
}

pub(crate) fn handoff_path(cwd: &Path, session_id: &str) -> PathBuf {
    cwd.join(".sessionweft")
        .join("handoffs")
        .join(format!("{session_id}-codex.md"))
}

pub(crate) fn write_codex_handoff(
    cwd: &Path,
    session_id: &str,
    native_session_id: &str,
    rollout_path: &Path,
) -> anyhow::Result<PathBuf> {
    let messages = extract_visible_messages(rollout_path)?;
    let path = handoff_path(cwd, session_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut content = format!(
        "# SessionWeft cross-agent handoff\n\n- Source agent: Codex\n- SessionWeft Session: `{session_id}`\n- Native Codex session: `{native_session_id}`\n- Rollout source: `{}`\n\n## Visible conversation\n",
        rollout_path.display()
    );
    if messages.is_empty() {
        content.push_str(
            "\n_No visible user or assistant messages were found in the recent rollout tail._\n",
        );
    } else {
        for message in messages {
            content.push_str(&format!("\n### {}\n\n{}\n", message.role, message.text));
        }
    }

    fs::write(&path, content)?;
    ensure!(
        load_codex_handoff(cwd, session_id)?.is_some(),
        "Codex handoff was not persisted at {}",
        path.display()
    );
    Ok(path)
}

pub(crate) fn load_codex_handoff(cwd: &Path, session_id: &str) -> anyhow::Result<Option<String>> {
    let path = handoff_path(cwd, session_id);
    if !path.is_file() {
        return Ok(None);
    }
    fs::read_to_string(&path)
        .with_context(|| format!("read Codex handoff {}", path.display()))
        .map(Some)
}

fn extract_visible_messages(path: &Path) -> anyhow::Result<Vec<VisibleMessage>> {
    let mut file =
        fs::File::open(path).with_context(|| format!("open Codex rollout {}", path.display()))?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(MAX_ROLLOUT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(file);
    if start > 0 {
        let mut partial = String::new();
        reader.read_line(&mut partial)?;
    }

    let mut event_messages = Vec::new();
    let mut response_messages = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("event_msg") => {
                if let Some(message) = parse_event_message(value.get("payload").unwrap_or(&value)) {
                    push_unique(&mut event_messages, message);
                }
            }
            Some("response_item") => {
                if let Some(message) =
                    parse_response_message(value.get("payload").unwrap_or(&value))
                {
                    push_unique(&mut response_messages, message);
                }
            }
            _ => {}
        }
    }

    let messages = if event_messages.is_empty() {
        response_messages
    } else {
        event_messages
    };
    Ok(limit_messages(messages))
}

fn parse_event_message(payload: &Value) -> Option<VisibleMessage> {
    let role = match payload.get("type")?.as_str()? {
        "user_message" => "User",
        "agent_message" => "Codex",
        _ => return None,
    };
    let text = payload.get("message")?.as_str()?.trim();
    (!text.is_empty()).then(|| VisibleMessage {
        role: role.to_owned(),
        text: text.to_owned(),
    })
}

fn parse_response_message(payload: &Value) -> Option<VisibleMessage> {
    if payload.get("type")?.as_str()? != "message" {
        return None;
    }
    let role = match payload.get("role")?.as_str()? {
        "user" => "User",
        "assistant" => "Codex",
        _ => return None,
    };
    let text = payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    (!text.is_empty()).then(|| VisibleMessage {
        role: role.to_owned(),
        text,
    })
}

fn push_unique(messages: &mut Vec<VisibleMessage>, message: VisibleMessage) {
    if messages.last() != Some(&message) {
        messages.push(message);
    }
}

fn limit_messages(messages: Vec<VisibleMessage>) -> Vec<VisibleMessage> {
    let mut selected = Vec::new();
    let mut characters = 0usize;
    for message in messages.into_iter().rev().take(MAX_HANDOFF_MESSAGES) {
        let message_characters = message.text.chars().count();
        if !selected.is_empty()
            && characters.saturating_add(message_characters) > MAX_HANDOFF_CHARACTERS
        {
            break;
        }
        characters = characters.saturating_add(message_characters);
        selected.push(message);
    }
    selected.reverse();
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_visible_event_messages() {
        let user = serde_json::json!({"type": "user_message", "message": "hello"});
        let assistant = serde_json::json!({"type": "agent_message", "message": "hi"});
        assert_eq!(
            parse_event_message(&user),
            Some(VisibleMessage {
                role: "User".into(),
                text: "hello".into(),
            })
        );
        assert_eq!(
            parse_event_message(&assistant),
            Some(VisibleMessage {
                role: "Codex".into(),
                text: "hi".into(),
            })
        );
    }

    #[test]
    fn parses_visible_response_messages() {
        let value = serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "done"}]
        });
        assert_eq!(
            parse_response_message(&value),
            Some(VisibleMessage {
                role: "Codex".into(),
                text: "done".into(),
            })
        );
    }
}
