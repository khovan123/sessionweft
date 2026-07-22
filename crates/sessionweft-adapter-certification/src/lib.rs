use std::{collections::BTreeSet, fs, path::Path};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    Provider,
    Plugin,
    Deployment,
    Billing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertificationGate {
    Contract,
    Compatibility,
    Security,
    Recovery,
    Observability,
    SupplyChain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterManifest {
    pub schema_version: u32,
    pub adapter_id: String,
    pub version: String,
    pub kind: AdapterKind,
    pub production: bool,
    pub supported_platforms: BTreeSet<String>,
    pub capabilities: BTreeSet<String>,
    pub source_paths: Vec<String>,
    pub required_gates: BTreeSet<CertificationGate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateEvidence {
    pub gate: CertificationGate,
    pub passed: bool,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterCertification {
    pub schema_version: u32,
    pub adapter_id: String,
    pub adapter_version: String,
    pub manifest_sha256: String,
    pub tested_commit: String,
    pub reviewed_at: DateTime<Utc>,
    pub reviewer: String,
    pub approved_for_production: bool,
    pub gates: Vec<GateEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificationReport {
    pub passed: bool,
    pub adapter_id: String,
    pub blockers: Vec<String>,
}

pub fn load_manifest(path: impl AsRef<Path>) -> Result<AdapterManifest, CertificationError> {
    let bytes = fs::read(path).map_err(CertificationError::Io)?;
    serde_json::from_slice(&bytes).map_err(CertificationError::Json)
}

pub fn load_certification(
    path: impl AsRef<Path>,
) -> Result<AdapterCertification, CertificationError> {
    let bytes = fs::read(path).map_err(CertificationError::Io)?;
    serde_json::from_slice(&bytes).map_err(CertificationError::Json)
}

#[must_use]
pub fn manifest_digest(manifest: &AdapterManifest) -> String {
    let bytes = serde_json::to_vec(manifest).expect("adapter manifest serialization cannot fail");
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[must_use]
pub fn evaluate(
    manifest: &AdapterManifest,
    certification: &AdapterCertification,
    repository_root: &Path,
) -> CertificationReport {
    let mut blockers = Vec::new();
    validate_manifest(manifest, &mut blockers);
    if certification.schema_version != 1 {
        blockers.push("unsupported certification schema version".into());
    }
    if certification.adapter_id != manifest.adapter_id {
        blockers.push("certification adapter ID does not match manifest".into());
    }
    if certification.adapter_version != manifest.version {
        blockers.push("certification adapter version does not match manifest".into());
    }
    let expected_digest = manifest_digest(manifest);
    if !certification
        .manifest_sha256
        .eq_ignore_ascii_case(&expected_digest)
    {
        blockers.push("certification manifest digest does not match".into());
    }
    if !is_commit_id(&certification.tested_commit) {
        blockers.push("certification must identify an exact tested commit".into());
    }
    if certification.reviewer.trim().is_empty() {
        blockers.push("certification reviewer is missing".into());
    }
    if manifest.production && !certification.approved_for_production {
        blockers.push("production adapter is not approved for production".into());
    }
    for path in &manifest.source_paths {
        if !is_safe_relative_path(path) {
            blockers.push(format!("adapter source path '{path}' is not a safe relative path"));
        } else if !repository_root.join(path).exists() {
            blockers.push(format!("adapter source path '{path}' does not exist"));
        }
    }
    let evidence_gates = certification
        .gates
        .iter()
        .map(|evidence| evidence.gate)
        .collect::<BTreeSet<_>>();
    for gate in &manifest.required_gates {
        match certification.gates.iter().find(|item| item.gate == *gate) {
            Some(item) if item.passed && !item.evidence.is_empty() => {}
            Some(_) => blockers.push(format!("required adapter gate '{gate:?}' did not pass")),
            None => blockers.push(format!("required adapter gate '{gate:?}' is missing")),
        }
    }
    if evidence_gates.len() != certification.gates.len() {
        blockers.push("certification contains duplicate gate evidence".into());
    }
    CertificationReport {
        passed: blockers.is_empty(),
        adapter_id: manifest.adapter_id.clone(),
        blockers,
    }
}

pub fn evaluate_directory(
    manifests_directory: impl AsRef<Path>,
    certifications_directory: impl AsRef<Path>,
    repository_root: impl AsRef<Path>,
) -> Result<Vec<CertificationReport>, CertificationError> {
    let manifests_directory = manifests_directory.as_ref();
    let certifications_directory = certifications_directory.as_ref();
    let repository_root = repository_root.as_ref();
    let mut reports = Vec::new();
    let mut entries = fs::read_dir(manifests_directory)
        .map_err(CertificationError::Io)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(CertificationError::Io)?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let manifest = load_manifest(&path)?;
        let certification_path = certifications_directory.join(format!(
            "{}-{}.json",
            manifest.adapter_id, manifest.version
        ));
        if !certification_path.exists() {
            reports.push(CertificationReport {
                passed: false,
                adapter_id: manifest.adapter_id,
                blockers: vec![format!(
                    "certification file '{}' is missing",
                    certification_path.display()
                )],
            });
            continue;
        }
        let certification = load_certification(certification_path)?;
        reports.push(evaluate(&manifest, &certification, repository_root));
    }
    Ok(reports)
}

fn validate_manifest(manifest: &AdapterManifest, blockers: &mut Vec<String>) {
    if manifest.schema_version != 1 {
        blockers.push("unsupported adapter manifest schema version".into());
    }
    if !valid_identifier(&manifest.adapter_id, 128) {
        blockers.push("adapter ID is invalid".into());
    }
    if !valid_identifier(&manifest.version, 64) {
        blockers.push("adapter version is invalid".into());
    }
    if manifest.supported_platforms.is_empty() {
        blockers.push("adapter must declare supported platforms".into());
    }
    if manifest.source_paths.is_empty() {
        blockers.push("adapter must declare source paths".into());
    }
    if manifest.production && manifest.required_gates.is_empty() {
        blockers.push("production adapter must require certification gates".into());
    }
}

fn valid_identifier(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn is_commit_id(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_safe_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.trim().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

#[derive(Debug, Error)]
pub enum CertificationError {
    #[error("adapter certification I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("adapter certification JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> AdapterManifest {
        AdapterManifest {
            schema_version: 1,
            adapter_id: "echo-provider".into(),
            version: "1.0.0".into(),
            kind: AdapterKind::Provider,
            production: true,
            supported_platforms: BTreeSet::from(["linux".into()]),
            capabilities: BTreeSet::from(["chat".into()]),
            source_paths: vec!["Cargo.toml".into()],
            required_gates: BTreeSet::from([
                CertificationGate::Contract,
                CertificationGate::Security,
            ]),
        }
    }

    #[test]
    fn production_certification_requires_exact_digest_commit_and_gates() {
        let manifest = manifest();
        let certification = AdapterCertification {
            schema_version: 1,
            adapter_id: manifest.adapter_id.clone(),
            adapter_version: manifest.version.clone(),
            manifest_sha256: manifest_digest(&manifest),
            tested_commit: "0123456789abcdef".into(),
            reviewed_at: Utc::now(),
            reviewer: "release-gate".into(),
            approved_for_production: true,
            gates: vec![
                GateEvidence {
                    gate: CertificationGate::Contract,
                    passed: true,
                    evidence: vec!["cargo test".into()],
                },
                GateEvidence {
                    gate: CertificationGate::Security,
                    passed: true,
                    evidence: vec!["cargo audit".into()],
                },
            ],
        };
        assert!(evaluate(&manifest, &certification, Path::new(".")).passed);
    }

    #[test]
    fn production_certification_rejects_unbound_or_missing_evidence() {
        let manifest = manifest();
        let certification = AdapterCertification {
            schema_version: 1,
            adapter_id: manifest.adapter_id.clone(),
            adapter_version: manifest.version.clone(),
            manifest_sha256: "0".repeat(64),
            tested_commit: "TBD".into(),
            reviewed_at: Utc::now(),
            reviewer: String::new(),
            approved_for_production: false,
            gates: Vec::new(),
        };
        assert!(!evaluate(&manifest, &certification, Path::new(".")).passed);
    }
}
