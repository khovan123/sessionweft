use std::{path::PathBuf, process::ExitCode};

use sessionweft_adapter_certification::evaluate_directory;

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let manifests = PathBuf::from(
        arguments
            .next()
            .unwrap_or_else(|| "release/adapters/manifests".into()),
    );
    let certifications = PathBuf::from(
        arguments
            .next()
            .unwrap_or_else(|| "release/adapters/certifications".into()),
    );
    let root = PathBuf::from(arguments.next().unwrap_or_else(|| ".".into()));
    if arguments.next().is_some() {
        eprintln!("usage: sessionweft-adapter-certification [manifests] [certifications] [root]");
        return ExitCode::FAILURE;
    }
    let reports = match evaluate_directory(manifests, certifications, root) {
        Ok(reports) => reports,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    match serde_json::to_string_pretty(&reports) {
        Ok(json) => println!("{json}"),
        Err(error) => {
            eprintln!("failed to serialize certification report: {error}");
            return ExitCode::FAILURE;
        }
    }
    if reports.is_empty() || reports.iter().any(|report| !report.passed) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
