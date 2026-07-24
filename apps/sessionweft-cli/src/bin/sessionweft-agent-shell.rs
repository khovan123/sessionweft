#[allow(clippy::items_after_test_module)]
mod shell {
    include!("support/sessionweft_agent_shell_impl.rs");

    pub(super) fn run() -> anyhow::Result<()> {
        main()
    }
}

fn main() -> anyhow::Result<()> {
    shell::run()
}
