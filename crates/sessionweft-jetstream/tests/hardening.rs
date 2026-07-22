use std::{process::Command, time::Duration};

use sessionweft_core::{EventEnvelope, SessionId};
use sessionweft_jetstream::{JetStreamConfig, JetStreamEventTransport};
use uuid::Uuid;

struct UnpauseGuard {
    container: String,
    armed: bool,
}

impl UnpauseGuard {
    fn new(container: String) -> Self {
        Self {
            container,
            armed: true,
        }
    }

    fn unpause(&mut self) {
        if self.armed {
            let status = Command::new("docker")
                .args(["unpause", &self.container])
                .status()
                .expect("docker unpause command");
            assert!(status.success(), "NATS container must unpause");
            self.armed = false;
        }
    }
}

impl Drop for UnpauseGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = Command::new("docker")
                .args(["unpause", &self.container])
                .status();
        }
    }
}

#[tokio::test]
#[ignore = "requires Docker-controlled NATS JetStream service"]
async fn publish_fails_bounded_during_partition_and_recovers_after_unpause() {
    let container =
        std::env::var("SESSIONWEFT_TEST_NATS_CONTAINER").expect("SESSIONWEFT_TEST_NATS_CONTAINER");
    let server_url = std::env::var("SESSIONWEFT_TEST_NATS_URL")
        .unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let suffix = Uuid::new_v4().simple().to_string();
    let config = JetStreamConfig {
        server_url,
        stream_name: format!("SESSIONWEFT_HARDENING_{suffix}"),
        subject_prefix: format!("sessionweft.hardening.{suffix}"),
        ..Default::default()
    };
    let transport = JetStreamEventTransport::connect(config.clone())
        .await
        .expect("initial transport");
    let event = EventEnvelope::new(
        "hardening.partition_probe",
        Some(SessionId::new()),
        Uuid::new_v4(),
        Some("hardening"),
        serde_json::json!({"probe": true}),
    );
    transport
        .publish_event(&event)
        .await
        .expect("baseline publish");

    let pause_status = Command::new("docker")
        .args(["pause", &container])
        .status()
        .expect("docker pause command");
    assert!(pause_status.success(), "NATS container must pause");
    let mut guard = UnpauseGuard::new(container);

    let partition_result = tokio::time::timeout(
        Duration::from_secs(3),
        transport.publish_event(&EventEnvelope::new(
            "hardening.partitioned_publish",
            Some(SessionId::new()),
            Uuid::new_v4(),
            Some("hardening"),
            serde_json::json!({"partitioned": true}),
        )),
    )
    .await;
    assert!(
        partition_result.is_err()
            || partition_result
                .as_ref()
                .is_ok_and(|result| result.is_err()),
        "partitioned publish must fail or time out within three seconds"
    );

    guard.unpause();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let recovered = tokio::time::timeout(
        Duration::from_secs(10),
        JetStreamEventTransport::connect(config),
    )
    .await
    .expect("reconnect timeout")
    .expect("reconnect");
    recovered
        .publish_event(&EventEnvelope::new(
            "hardening.recovered_publish",
            Some(SessionId::new()),
            Uuid::new_v4(),
            Some("hardening"),
            serde_json::json!({"recovered": true}),
        ))
        .await
        .expect("publish after recovery");
}
