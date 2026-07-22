from pathlib import Path

approval = Path("crates/sessionweft-mcp/src/approval.rs")
text = approval.read_text()
text = text.replace(
    "#[derive(Debug, Error)]\npub enum McpApprovalRepositoryError {",
    "#[derive(Debug, Error, PartialEq, Eq)]\npub enum McpApprovalRepositoryError {",
    1,
)
approval.write_text(text)

sqlite = Path("crates/sessionweft-mcp-sqlite/src/lib.rs")
text = sqlite.read_text()
text = text.replace("use chrono::{DateTime, Utc};\n", "", 1)
text = text.replace("use chrono::Utc;\n", "", 1)
text = text.replace(
    "use sqlx::{Row, Sqlite, SqlitePool, Transaction, sqlite::SqliteConnectOptions};",
    "use sqlx::{\n    Row, Sqlite, SqlitePool, Transaction,\n    sqlite::{SqliteConnectOptions, SqlitePoolOptions},\n};",
    1,
)
old_connect = '''        let options = SqliteConnectOptions::from_str(database_url)
            .map_err(backend)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(StdDuration::from_secs(5));
        let pool = SqlitePool::connect_with(options).await.map_err(backend)?;'''
new_connect = '''        let is_memory = database_url.contains(":memory:");
        let options = SqliteConnectOptions::from_str(database_url)
            .map_err(backend)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(StdDuration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(if is_memory { 1 } else { 5 })
            .connect_with(options)
            .await
            .map_err(backend)?;'''
if old_connect in text:
    text = text.replace(old_connect, new_connect, 1)
elif new_connect not in text:
    raise SystemExit("SQLite connect block not found")
text = text.replace(
    "    use chrono::Duration;",
    "    use chrono::{Duration, Utc};",
    1,
)
sqlite.write_text(text)

malicious = Path("crates/sessionweft-mcp/tests/malicious.rs")
text = malicious.read_text()
old_window = '''    assert!(arguments.windows(2).any(|values| values == ["--setenv", "SAFE_FLAG"]));'''
new_window = '''    assert!(arguments.windows(2).any(|values| {
        values[0] == "--setenv" && values[1] == "SAFE_FLAG"
    }));'''
if old_window in text:
    text = text.replace(old_window, new_window, 1)
elif new_window not in text:
    raise SystemExit("bubblewrap setenv assertion not found")
malicious.write_text(text)
