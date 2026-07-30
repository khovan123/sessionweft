mod launcher {
    include!("support/sessionweft_claude_native_history_impl.rs");
}
#[path = "support/sessionweft_handoff.rs"]
mod sessionweft_handoff;
#[path = "support/shared_session_picker.rs"]
mod shared_session_picker;

fn main() -> anyhow::Result<()> {
    launcher::run()
}
