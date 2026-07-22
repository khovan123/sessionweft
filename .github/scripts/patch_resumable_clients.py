from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    if new in text:
        return
    if old not in text:
        raise SystemExit(f"{label} marker not found in {path}")
    path.write_text(text.replace(old, new, 1))


# Keep ownership of the real PTY child in the waiter thread; no dummy child is needed.
pty = Path("crates/sessionweft-client-protocol/src/pty.rs")
text = pty.read_text()
text = text.replace("PtySize, PtySystem, native_pty_system", "PtySize, native_pty_system", 1)
text = text.replace("        let mut child = pair\n", "        let child = pair\n", 1)
text = text.replace(
    "        spawn_waiter(Arc::clone(&session), &mut child);",
    "        spawn_waiter(Arc::clone(&session), child);",
    1,
)
text = text.replace("        Ok(session.descriptor()?)", "        session.descriptor()", 1)
start = text.find("fn spawn_waiter(\n")
end = text.find("fn validate_start(", start)
if start < 0 or end < 0:
    raise SystemExit("PTY waiter boundaries not found")
waiter = '''fn spawn_waiter(
    session: Arc<PtySession>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
) {
    thread::spawn(move || match child.wait() {
        Ok(_) => session.finish(PtyStatus::Exited),
        Err(_) => session.finish(PtyStatus::Failed),
    });
}

'''
text = text[:start] + waiter + text[end:]
pty.write_text(text)

# Avoid eager map_or evaluation moving RequestBuilder twice.
for path in [
    Path("apps/sessionweft-tui/src/main.rs"),
    Path("apps/sessionweft-cli/src/protocol_cli.rs"),
]:
    text = path.read_text()
    old = '''        self.token
            .as_deref()
            .map_or(request.try_clone().unwrap_or(request), |token| {
                request.bearer_auth(token)
            })'''
    new = '''        if let Some(token) = self.token.as_deref() {
            request.bearer_auth(token)
        } else {
            request
        }'''
    if old in text:
        text = text.replace(old, new, 1)
    old_free = '''    token.map_or(builder.try_clone().unwrap_or(builder), |token| {
        builder.bearer_auth(token)
    })'''
    new_free = '''    if let Some(token) = token {
        builder.bearer_auth(token)
    } else {
        builder
    }'''
    if old_free in text:
        text = text.replace(old_free, new_free, 1)
    path.write_text(text)

# Attach the new protocol commands without changing existing CLI command names.
cli = Path("apps/sessionweft-cli/src/main.rs")
text = cli.read_text()
if not text.startswith("mod protocol_cli;"):
    text = "mod protocol_cli;\n\nuse protocol_cli::ClientCommand;\n" + text
variant_marker = '''    MemoryForget {
        session_id: String,
        memory_id: String,
    },
}'''
variant_replacement = '''    MemoryForget {
        session_id: String,
        memory_id: String,
    },
    Client {
        #[command(subcommand)]
        command: ClientCommand,
    },
}'''
if variant_replacement not in text:
    if variant_marker not in text:
        raise SystemExit("CLI command enum marker not found")
    text = text.replace(variant_marker, variant_replacement, 1)
execute_marker = '''    let endpoint = cli.endpoint.trim_end_matches('/');

    let (method, path, body) = match cli.command {'''
execute_replacement = '''    let endpoint = cli.endpoint.trim_end_matches('/');

    if let Command::Client { command } = &cli.command {
        protocol_cli::execute(&client, endpoint, cli.token.as_deref(), command).await?;
        return Ok(());
    }

    let (method, path, body) = match cli.command {'''
if execute_replacement not in text:
    if execute_marker not in text:
        raise SystemExit("CLI execute marker not found")
    text = text.replace(execute_marker, execute_replacement, 1)
match_end = '''        Command::MemoryForget {
            session_id,
            memory_id,
        } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/memories/{memory_id}/forget"),
            None,
        ),
    };'''
match_replacement = '''        Command::MemoryForget {
            session_id,
            memory_id,
        } => (
            Method::POST,
            format!("/v1/sessions/{session_id}/memories/{memory_id}/forget"),
            None,
        ),
        Command::Client { .. } => unreachable!("client commands return before legacy dispatch"),
    };'''
if match_replacement not in text:
    if match_end not in text:
        raise SystemExit("CLI match end marker not found")
    text = text.replace(match_end, match_replacement, 1)
cli.write_text(text)

# Add query encoder dependency once.
root = Path("Cargo.toml")
text = root.read_text()
if 'serde_urlencoded = "0.7"' not in text:
    text = text.replace('serde_json = "1"\n', 'serde_json = "1"\nserde_urlencoded = "0.7"\n', 1)
root.write_text(text)

cli_manifest = Path("apps/sessionweft-cli/Cargo.toml")
text = cli_manifest.read_text()
if "serde_urlencoded.workspace = true" not in text:
    text = text.replace(
        "serde_json.workspace = true\n",
        "serde_json.workspace = true\nserde_urlencoded.workspace = true\n",
        1,
    )
cli_manifest.write_text(text)
