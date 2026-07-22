from pathlib import Path

path = Path("crates/sessionweft-scheduler-sqlite/src/lib.rs")
text = path.read_text()
old = '''        let (scheduler, workflow, mut agent, _path) = setup().await;
        agent.manifest.capabilities.clear();
        let request = ClaimRequest {
            workflow_id: workflow.id,
            agent_id: agent.id,
            correlation_id: Uuid::new_v4(),
            actor_id: Some("scheduler".into()),
        };
'''
new = '''        let (scheduler, workflow, agent, _path) = setup().await;
        let incompatible_plan = SchedulerPlan::new(
            &workflow,
            BTreeMap::from([(
                "worker".into(),
                TaskRequirement {
                    role: Some(AgentRole::Worker),
                    capabilities: BTreeSet::from([Capability::Network]),
                },
            )]),
        )
        .expect("incompatible plan");
        scheduler
            .register_plan(&incompatible_plan)
            .await
            .expect("replace plan");
        let request = ClaimRequest {
            workflow_id: workflow.id,
            agent_id: agent.id,
            correlation_id: Uuid::new_v4(),
            actor_id: Some("scheduler".into()),
        };
'''
if old not in text:
    raise SystemExit("scheduler capability test marker not found")
path.write_text(text.replace(old, new, 1))
