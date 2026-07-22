use std::time::{Duration, Instant};

use sessionweft_core::{MessageRole, ProviderMessage, ProviderRequest, SessionId};
use sessionweft_knowledge::{ContextBudget, ContextBuilder, ContextCandidate, ContextKind};
use sessionweft_provider::{EchoProvider, Provider};

#[test]
fn context_builder_keeps_large_candidate_sets_within_budget() {
    let candidates = (0..10_000).map(|index| ContextCandidate {
        id: format!("candidate-{index}"),
        kind: if index % 5 == 0 {
            ContextKind::Workspace
        } else {
            ContextKind::Memory
        },
        content: format!(
            "candidate {index} contains deterministic repository context and dependency evidence"
        ),
        source: format!("workspace://src/module_{index}.rs"),
        inclusion_reason: "release_capacity_candidate".into(),
        priority: u8::try_from(index % 10).expect("priority"),
        relevance: (index % 100) as f32 / 100.0,
        required: index < 3,
    });
    let started = Instant::now();
    let package = ContextBuilder::build(
        candidates,
        ContextBudget {
            max_tokens: 32_000,
            reserved_tokens: 4_000,
        },
    )
    .expect("context package");
    assert!(package.estimated_tokens <= package.usable_budget);
    assert!(
        package
            .items
            .iter()
            .filter(|item| item.candidate.required)
            .count()
            == 3
    );
    assert!(!package.omitted.is_empty());
    assert!(started.elapsed() <= Duration::from_secs(2));
}

#[tokio::test]
async fn echo_provider_latency_stays_below_release_contract() {
    let provider = EchoProvider;
    let mut samples = Vec::with_capacity(500);
    for index in 0..500 {
        let started = Instant::now();
        let response = provider
            .complete(ProviderRequest {
                session_id: SessionId::new(),
                model: "latency-contract".into(),
                messages: vec![ProviderMessage {
                    role: MessageRole::User,
                    content: format!("request-{index}"),
                }],
            })
            .await
            .expect("echo response");
        assert!(response.text.contains("latency-contract"));
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let p95_index = (samples.len() * 95 / 100).min(samples.len() - 1);
    assert!(samples[p95_index] <= Duration::from_millis(50));
}
