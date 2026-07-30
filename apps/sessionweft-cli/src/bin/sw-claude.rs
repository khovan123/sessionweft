#[path = "support/sessionweft_claude_launcher_impl.rs"]
mod launcher;
#[path = "support/sessionweft_handoff.rs"]
mod sessionweft_handoff;
#[path = "support/shared_session_picker.rs"]
mod shared_session_picker;

fn main() -> anyhow::Result<()> {
    launcher::run()
}
