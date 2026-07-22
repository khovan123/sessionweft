from pathlib import Path

path = Path("crates/sessionweft-mcp/src/lib.rs")
text = path.read_text()

stdio_old = '''        let service = ().serve(transport).await.map_err(sdk_error)?;'''
stdio_new = '''        let service = tokio::select! {
            () = self.cancellation.cancelled() => return Err(McpAdapterError::Cancelled),
            result = tokio::time::timeout(self.config.operation_timeout, ().serve(transport)) => {
                result
                    .map_err(|_| McpAdapterError::Timeout(self.config.operation_timeout))?
                    .map_err(sdk_error)?
            }
        };'''
http_old = '''        let service = ().serve(transport).await.map_err(sdk_error)?;'''
http_new = stdio_new

occurrences = text.count(stdio_old)
if occurrences == 4:
    text = text.replace(stdio_old, stdio_new)
elif text.count(stdio_new) != 4:
    raise SystemExit(f"expected four MCP serve calls, found {occurrences}")

path.write_text(text)
