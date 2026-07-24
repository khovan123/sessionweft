#[path = "support/sessionweft_agent_server_impl.rs"]
mod server;

fn main() -> anyhow::Result<()> {
    server::run()
}
