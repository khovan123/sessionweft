use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, ValueEnum};
use sessionweft_release_gate::{GateLevel, evaluate, load_evidence, load_policy};

#[derive(Debug, Parser)]
#[command(name = "sessionweft-release-gate")]
#[command(about = "Validate SessionWeft release policy and evidence")]
struct Arguments {
    #[arg(long, default_value = "release/release-policy.json")]
    policy: PathBuf,
    #[arg(long, default_value = "release/evidence/rc-0.1.0.json")]
    evidence: PathBuf,
    #[arg(long, value_enum, default_value_t = Level::Preflight)]
    level: Level,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Level {
    Preflight,
    Rc,
    Ga,
}

impl From<Level> for GateLevel {
    fn from(value: Level) -> Self {
        match value {
            Level::Preflight => Self::Preflight,
            Level::Rc => Self::ReleaseCandidate,
            Level::Ga => Self::GeneralAvailability,
        }
    }
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    let result = load_policy(&arguments.policy)
        .and_then(|policy| load_evidence(&arguments.evidence).map(|evidence| (policy, evidence)));
    let (policy, evidence) = match result {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let report = evaluate(&policy, &evidence, arguments.level.into());
    match serde_json::to_string_pretty(&report) {
        Ok(value) => println!("{value}"),
        Err(error) => {
            eprintln!("failed to serialize gate report: {error}");
            return ExitCode::FAILURE;
        }
    }
    if report.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
