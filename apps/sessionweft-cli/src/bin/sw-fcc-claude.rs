#[allow(clippy::items_after_test_module)]
mod wrapper {
    include!("support/sessionweft_agent_wrapper_impl.rs");

    pub(super) fn run() -> anyhow::Result<()> {
        main()
    }
}

fn main() -> anyhow::Result<()> {
    wrapper::run()
}
