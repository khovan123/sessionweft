#[path = "lib.rs"]
mod certification;

pub use certification::*;

use std::{env, path::PathBuf, sync::OnceLock};

use thiserror::Error;

static RELEASE_CERTIFICATIONS: OnceLock<Result<Option<VerifiedCertificationSet>, String>> =
    OnceLock::new();

pub fn require_compiled_adapter(
    adapter_id: &str,
    version: &str,
    kind: AdapterKind,
) -> Result<(), AdapterActivationError> {
    let set = RELEASE_CERTIFICATIONS
        .get_or_init(load_compiled_certifications)
        .as_ref()
        .map_err(|message| AdapterActivationError::Load(message.clone()))?;
    match set {
        Some(set) => set
            .require(adapter_id, version, kind)
            .map_err(AdapterActivationError::Certification),
        None => Ok(()),
    }
}

#[must_use]
pub fn compiled_build_commit() -> Option<&'static str> {
    option_env!("SESSIONWEFT_BUILD_COMMIT")
}

fn load_compiled_certifications() -> Result<Option<VerifiedCertificationSet>, String> {
    let force = env::var("SESSIONWEFT_REQUIRE_CERTIFIED_ADAPTERS")
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES"));
    let Some(build_commit) = compiled_build_commit() else {
        if force {
            return Err(
                "SESSIONWEFT_REQUIRE_CERTIFIED_ADAPTERS is enabled but the binary has no compile-time SESSIONWEFT_BUILD_COMMIT"
                    .into(),
            );
        }
        return Ok(None);
    };

    let executable = env::current_exe()
        .map_err(|error| format!("resolve Runtime executable for adapter activation: {error}"))?;
    let package_root = executable
        .parent()
        .and_then(|directory| directory.parent())
        .ok_or_else(|| "Runtime executable is not inside a release package bin directory".to_owned())?;
    let manifests = env_path("SESSIONWEFT_ADAPTER_MANIFESTS_DIR")
        .unwrap_or_else(|| package_root.join("config/adapter-manifests"));
    let certifications = env_path("SESSIONWEFT_ADAPTER_CERTIFICATIONS_DIR")
        .unwrap_or_else(|| package_root.join("config/adapter-certifications"));
    let repository_root = env_path("SESSIONWEFT_ADAPTER_SOURCE_ROOT")
        .unwrap_or_else(|| package_root.to_path_buf());

    VerifiedCertificationSet::load(
        manifests,
        certifications,
        repository_root,
        build_commit,
    )
    .map(Some)
    .map_err(|error| error.to_string())
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[derive(Debug, Error)]
pub enum AdapterActivationError {
    #[error("verified adapter certification set could not be loaded: {0}")]
    Load(String),
    #[error(transparent)]
    Certification(#[from] CertificationError),
}
