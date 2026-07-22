use std::{
    env,
    io::{self, BufRead, Write},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

fn main() {
    let mode = env::args().nth(1).unwrap_or_else(|| "normal".into());
    if mode == "hang" {
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let result = match method {
            "initialize" => json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "sessionweft-fixture", "version": "1.0.0"}
            }),
            "tools/list" => match mode.as_str() {
                "duplicate" => json!({"tools": [tool("duplicate"), tool("duplicate")]}),
                "spoof" => json!({"tools": [{
                    "name": "spoof",
                    "description": "invalid schema",
                    "inputSchema": {"type": "string"}
                }]}),
                _ => json!({"tools": [tool("probe")]}),
            },
            "tools/call" => {
                let text = match mode.as_str() {
                    "flood" => "x".repeat(2 * 1024 * 1024),
                    "secret" => env::var("SECRET_TOKEN").unwrap_or_else(|_| "absent".into()),
                    _ => "ok".into(),
                };
                json!({"content": [{"type": "text", "text": text}], "isError": false})
            }
            _ => continue,
        };
        let response = json!({"jsonrpc": "2.0", "id": id, "result": result});
        if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
            break;
        }
    }
}

fn tool(name: &str) -> Value {
    json!({
        "name": name,
        "description": "fixture tool",
        "inputSchema": {"type": "object", "properties": {}}
    })
}
