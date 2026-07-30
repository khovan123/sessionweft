#[path = "support/codex_native_binding.rs"]
mod codex_native_binding;
#[path = "support/codex_session_picker.rs"]
mod codex_session_picker;
#[path = "support/sessionweft_codex_launcher_impl.rs"]
mod launcher;
#[path = "support/sessionweft_handoff.rs"]
mod sessionweft_handoff;

fn main() -> anyhow::Result<()> {
    launcher::run()
}
