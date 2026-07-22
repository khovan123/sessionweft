use std::{collections::BTreeMap, fs, path::Path};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateLevel {
    Preflight,
    ReleaseCandidate,
    GeneralAvailability,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleasePolicy {
    pub schema_version: u32,
    pub product: String,
    pub release: String,
    pub slo: ServiceLevelObjectives,
    pub recovery: RecoveryObjectives,
    pub capacity: CapacityTargets,
    pub required_gates: Vec<String>,
    pub required_signoff_roles: Vec<String>,
    pub max_open_critical_findings: u32,
    pub max_open_high_findings: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceLevelObjectives {
    pub monthly_availability_percent: f64,
    pub api_read_p95_ms: u64,
    pub api_mutation_p95_ms: u64,
    pub event_delivery_p95_ms: u64,
    pub scheduler_claim_p95_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryObjectives {
    pub service_rto_minutes: u64,
    pub service_rpo_minutes: u64,
    pub local_committed_state_rpo_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapacityTargets {
    pub concurrent_sessions: u64,
    pub active_agents: u64,
    pub queued_tasks: u64,
    pub indexed_files_per_workspace: u64,
    pub event_backlog: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Passed,
    Failed,
    Waived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateEvidence {
    pub status: GateStatus,
    pub evidence: Vec<String>,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityFindings {
    pub critical_open: u32,
    pub high_open: u32,
    pub medium_open: u32,
    pub low_open: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignoffDecision {
    ApprovedForRc,
    ApprovedForGa,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signoff {
    pub role: String,
    pub reviewer: String,
    pub human: bool,
    pub decision: SignoffDecision,
    pub reviewed_at: DateTime<Utc>,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseEvidence {
    pub schema_version: u32,
    pub product: String,
    pub release: String,
    pub commit: String,
    pub generated_at: DateTime<Utc>,
    pub gates: BTreeMap<String, GateEvidence>,
    pub security_findings: SecurityFindings,
    pub signoffs: Vec<Signoff>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateReport {
    pub level: GateLevel,
    pub passed: bool,
    pub release: String,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn load_policy(path: impl AsRef<Path>) -> Result<ReleasePolicy, ReleaseGateError> {
    let bytes = fs::read(path).map_err(ReleaseGateError::Io)?;
    serde_json::from_slice(&bytes).map_err(ReleaseGateError::Json)
}

pub fn load_evidence(path: impl AsRef<Path>) -> Result<ReleaseEvidence, ReleaseGateError> {
    let bytes = fs::read(path).map_err(ReleaseGateError::Io)?;
    serde_json::from_slice(&bytes).map_err(ReleaseGateError::Json)
}

pub fn evaluate(
    policy: &ReleasePolicy,
    evidence: &ReleaseEvidence,
    level: GateLevel,
) -> GateReport {
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();

    if policy.schema_version != 1 || evidence.schema_version != 1 {
        blockers.push("unsupported release gate schema version".into());
    }
    if policy.product != evidence.product {
        blockers.push("policy and evidence product names differ".into());
    }
    if policy.release != evidence.release {
        blockers.push("policy and evidence release versions differ".into());
    }
    if evidence.commit.trim().is_empty() || evidence.commit == "TBD" {
        blockers.push("release evidence must identify the tested commit".into());
    }
    validate_policy(policy, &mut blockers);

    for gate in &policy.required_gates {
        match evidence.gates.get(gate) {
            Some(item) if item.status == GateStatus::Passed && !item.evidence.is_empty() => {}
            Some(item) if item.status == GateStatus::Waived => {
                if level == GateLevel::Preflight {
                    warnings.push(format!("gate '{gate}' is waived: {}", item.notes));
                } else {
                    blockers.push(format!("release gate '{gate}' cannot be waived at {level:?}"));
                }
            }
            Some(item) => blockers.push(format!(
                "release gate '{gate}' is not passed: {}",
                item.notes
            )),
            None => blockers.push(format!("required release gate '{gate}' has no evidence")),
        }
    }

    if evidence.security_findings.critical_open > policy.max_open_critical_findings {
        blockers.push(format!(
            "{} Critical security findings remain open",
            evidence.security_findings.critical_open
        ));
    }
    if evidence.security_findings.high_open > policy.max_open_high_findings {
        blockers.push(format!(
            "{} High security findings remain open",
            evidence.security_findings.high_open
        ));
    }

    if level != GateLevel::Preflight {
        for role in &policy.required_signoff_roles {
            let decision = evidence.signoffs.iter().find(|signoff| signoff.role == *role);
            match decision {
                Some(signoff)
                    if level == GateLevel::ReleaseCandidate
                        && matches!(
                            signoff.decision,
                            SignoffDecision::ApprovedForRc | SignoffDecision::ApprovedForGa
                        )
                        && !signoff.evidence.is_empty() => {}
                Some(signoff)
                    if level == GateLevel::GeneralAvailability
                        && signoff.decision == SignoffDecision::ApprovedForGa
                        && signoff.human
                        && !signoff.evidence.is_empty() => {}
                Some(signoff) => blockers.push(format!(
                    "sign-off '{}' by '{}' is insufficient for {level:?}",
                    role, signoff.reviewer
                )),
                None => blockers.push(format!("required sign-off role '{role}' is missing")),
            }
        }
    }

    if level == GateLevel::ReleaseCandidate
        && evidence.signoffs.iter().any(|signoff| !signoff.human)
    {
        warnings.push(
            "automated RC sign-offs do not authorize General Availability; human GA sign-offs remain mandatory"
                .into(),
        );
    }

    GateReport {
        level,
        passed: blockers.is_empty(),
        release: policy.release.clone(),
        blockers,
        warnings,
    }
}

fn validate_policy(policy: &ReleasePolicy, blockers: &mut Vec<String>) {
    if !(99.0..=100.0).contains(&policy.slo.monthly_availability_percent) {
        blockers.push("monthly availability SLO must be between 99 and 100 percent".into());
    }
    if policy.slo.api_read_p95_ms == 0
        || policy.slo.api_mutation_p95_ms == 0
        || policy.slo.event_delivery_p95_ms == 0
        || policy.slo.scheduler_claim_p95_ms == 0
    {
        blockers.push("all latency SLOs must be greater than zero".into());
    }
    if policy.recovery.service_rto_minutes == 0 || policy.recovery.service_rpo_minutes == 0 {
        blockers.push("service RTO and RPO must be greater than zero".into());
    }
    if policy.capacity.concurrent_sessions == 0
        || policy.capacity.active_agents == 0
        || policy.capacity.queued_tasks == 0
        || policy.capacity.indexed_files_per_workspace == 0
        || policy.capacity.event_backlog == 0
    {
        blockers.push("all capacity targets must be greater than zero".into());
    }
}

#[derive(Debug, Error)]
pub enum ReleaseGateError {
    #[error("release gate I/O error: {0}")]
    Io(std::io::Error),
    #[error("release gate JSON error: {0}")]
    Json(serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> ReleasePolicy {
        ReleasePolicy {
            schema_version: 1,
            product: "SessionWeft".into(),
            release: "0.1.0-rc.1".into(),
            slo: ServiceLevelObjectives {
                monthly_availability_percent: 99.9,
                api_read_p95_ms: 250,
                api_mutation_p95_ms: 500,
                event_delivery_p95_ms: 2_000,
                scheduler_claim_p95_ms: 2_000,
            },
            recovery: RecoveryObjectives {
                service_rto_minutes: 30,
                service_rpo_minutes: 5,
                local_committed_state_rpo_seconds: 0,
            },
            capacity: CapacityTargets {
                concurrent_sessions: 100,
                active_agents: 50,
                queued_tasks: 10_000,
                indexed_files_per_workspace: 50_000,
                event_backlog: 1_000_000,
            },
            required_gates: vec!["workspace_tests".into(), "security_audit".into()],
            required_signoff_roles: vec![
                "architecture".into(),
                "security".into(),
                "operations".into(),
            ],
            max_open_critical_findings: 0,
            max_open_high_findings: 0,
        }
    }

    fn evidence() -> ReleaseEvidence {
        ReleaseEvidence {
            schema_version: 1,
            product: "SessionWeft".into(),
            release: "0.1.0-rc.1".into(),
            commit: "deadbeef".into(),
            generated_at: Utc::now(),
            gates: BTreeMap::from([
                (
                    "workspace_tests".into(),
                    GateEvidence {
                        status: GateStatus::Passed,
                        evidence: vec!["ci://workspace".into()],
                        notes: "passed".into(),
                    },
                ),
                (
                    "security_audit".into(),
                    GateEvidence {
                        status: GateStatus::Passed,
                        evidence: vec!["ci://security".into()],
                        notes: "passed".into(),
                    },
                ),
            ]),
            security_findings: SecurityFindings {
                critical_open: 0,
                high_open: 0,
                medium_open: 0,
                low_open: 0,
            },
            signoffs: vec!["architecture", "security", "operations"]
                .into_iter()
                .map(|role| Signoff {
                    role: role.into(),
                    reviewer: "sessionweft-automation".into(),
                    human: false,
                    decision: SignoffDecision::ApprovedForRc,
                    reviewed_at: Utc::now(),
                    evidence: vec![format!("review://{role}")],
                })
                .collect(),
        }
    }

    #[test]
    fn rc_accepts_automated_signoffs_but_ga_does_not() {
        assert!(evaluate(&policy(), &evidence(), GateLevel::ReleaseCandidate).passed);
        let report = evaluate(&policy(), &evidence(), GateLevel::GeneralAvailability);
        assert!(!report.passed);
        assert_eq!(report.blockers.len(), 3);
    }

    #[test]
    fn open_high_finding_blocks_release() {
        let mut evidence = evidence();
        evidence.security_findings.high_open = 1;
        assert!(!evaluate(&policy(), &evidence, GateLevel::ReleaseCandidate).passed);
    }
}
