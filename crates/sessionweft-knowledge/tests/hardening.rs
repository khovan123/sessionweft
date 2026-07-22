use std::time::{Duration, Instant};

use sessionweft_knowledge::{ContextBudget, ContextBuilder, ContextCandidate, ContextKind};

#[test]
#[ignore = "capacity gate executed by production-hardening workflow"]
fn assembles_five_thousand_candidates_within_budget_and_latency_target() {
    let candidates = (0..5_000)
        .map(|index| ContextCandidate {
            id: format!("candidate-{index}"),
            kind: if index % 10 == 0 {
                ContextKind::Dependency
            } else {
                ContextKind::Workspace
            },
            content: format!(
                "revision-bound context candidate {index} with deterministic source material"
            ),
            source: format!("src/module_{}.rs", index % 1_000),
            inclusion_reason: "hardening_capacity_fixture".into(),
            priority: u8::try_from(index % 10).expect("priority"),
            relevance: (index % 100) as f32 / 100.0,
            required: index < 5,
        })
        .collect::<Vec<_>>();

    let started = Instant::now();
    let package = ContextBuilder::build(
        candidates,
        ContextBudget {
            max_tokens: 20_000,
            reserved_tokens: 2_000,
        },
    )
    .expect("context package");
    assert!(started.elapsed() <= Duration::from_secs(5));
    assert!(package.estimated_tokens <= package.usable_budget);
    assert_eq!(
        package
            .items
            .iter()
            .filter(|item| item.candidate.required)
            .count(),
        5
    );
    assert!(!package.omitted.is_empty());
    assert!(
        package
            .omitted
            .iter()
            .all(|item| item.reason == "token_budget_exceeded")
    );
}
