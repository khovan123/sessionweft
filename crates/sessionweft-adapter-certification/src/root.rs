#[path = "lib.rs"]
mod certification;

pub use certification::*;

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use serde::Deserialize;
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
    let executable_name = executable
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Runtime executable name is not valid UTF-8".to_owned())?;
    let package_root = executable
        .parent()
        .and_then(|directory| directory.parent())
        .ok_or_else(|| {
            "Runtime executable is not inside a release package bin directory".to_owned()
        })?;
    let manifests = env_path("SESSIONWEFT_ADAPTER_MANIFESTS_DIR")
        .unwrap_or_else(|| package_root.join("config/adapter-manifests"));
    let certifications = env_path("SESSIONWEFT_ADAPTER_CERTIFICATIONS_DIR")
        .unwrap_or_else(|| package_root.join("config/adapter-certifications"));
    let activation_path = env_path("SESSIONWEFT_ADAPTER_ACTIVATION_FILE")
        .unwrap_or_else(|| package_root.join("config/adapter-activation.json"));
    let repository_root =
        env_path("SESSIONWEFT_ADAPTER_SOURCE_ROOT").unwrap_or_else(|| package_root.to_path_buf());

    let set =
        VerifiedCertificationSet::load(manifests, certifications, repository_root, build_commit)
            .map_err(|error| error.to_string())?;
    verify_runtime_activation(&set, &activation_path, executable_name)?;
    Ok(Some(set))
}

fn verify_runtime_activation(
    set: &VerifiedCertificationSet,
    activation_path: &Path,
    executable_name: &str,
) -> Result<(), String> {
    let bytes = fs::read(activation_path).map_err(|error| {
        format!(
            "read adapter activation file '{}': {error}",
            activation_path.display()
        )
    })?;
    let activation: RuntimeActivationManifest =
        serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "parse adapter activation file '{}': {error}",
                activation_path.display()
            )
        })?;
    if activation.schema_version != 1 {
        return Err(format!(
            "unsupported adapter activation schema version {}",
            activation.schema_version
        ));
    }
    let adapters = activation.runtimes.get(executable_name).ok_or_else(|| {
        format!("adapter activation file has no entry for Runtime binary '{executable_name}'")
    })?;
    if adapters.is_empty() {
        return Err(format!(
            "adapter activation entry for Runtime binary '{executable_name}' is empty"
        ));
    }
    for adapter in adapters {
        set.require(&adapter.adapter_id, &adapter.version, adapter.kind)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[derive(Debug, Deserialize)]
struct RuntimeActivationManifest {
    schema_version: u32,
    runtimes: BTreeMap<String, Vec<RuntimeAdapterActivation>>,
}

#[derive(Debug, Deserialize)]
struct RuntimeAdapterActivation {
    adapter_id: String,
    version: String,
    kind: AdapterKind,
}

#[derive(Debug, Error)]
pub enum AdapterActivationError {
    #[error("verified adapter certification set could not be loaded: {0}")]
    Load(String),
    #[error(transparent)]
    Certification(#[from] CertificationError),
}
