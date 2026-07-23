#[allow(dead_code, clippy::if_same_then_else)]
#[path = "support/session_cli_display.rs"]
mod session_cli_display;

mod legacy {
    include!("support/sessionweft_agentctl_legacy.rs");

    pub(super) fn run() -> anyhow::Result<()> {
        main()
    }
}

fn main() -> anyhow::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let plan = session_cli_display::before(session_cli_display::CliFlavor::Persistent, &args);
    if plan.handled {
        return Ok(());
    }

    let result = legacy::run();
    session_cli_display::after(
        session_cli_display::CliFlavor::Persistent,
        &plan,
        result.is_ok(),
    );
    result
}
