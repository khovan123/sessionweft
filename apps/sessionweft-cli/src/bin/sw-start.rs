use std::{
    env,
    path::PathBuf,
    process::{Command, Stdio},
};

use anyhow::{Context, bail};

fn main() -> anyhow::Result<()> {
    let runtime = resolve_runtime_binary();
    let status = if runtime.is_file() {
        Command::new(&runtime)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .with_context(|| format!("start SessionWeft Runtime through {}", runtime.display()))?
    } else {
        Command::new("cargo")
            .args(["run", "-p", "sessionweftd", "--bin", "sessionweftd"])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("start SessionWeft Runtime through cargo")?
    };

    if !status.success() {
        bail!("SessionWeft Runtime exited with {status}");
    }
    Ok(())
}

fn resolve_runtime_binary() -> PathBuf {
    if let Some(configured) = env::var_os("SESSIONWEFT_RUNTIME_BIN") {
        return PathBuf::from(configured);
    }
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("sessionweftd")))
        .unwrap_or_else(|| PathBuf::from("sessionweftd"))
}
