use std::{collections::BTreeSet, time::Duration};

use sessionweft_client_protocol::{
    PtyStatus, PtySupervisor, StartPtyRequest, discover_programs,
};
use sessionweft_core::SessionId;

#[tokio::test]
async fn dropping_client_poll_does_not_cancel_runtime_owned_pty() {
    let programs = discover_programs(&["sh"]);
    if !programs.contains_key("sh") {
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let supervisor = PtySupervisor::new(directory.path(), programs, BTreeSet::new())
        .expect("supervisor");
    let descriptor = supervisor
        .start(StartPtyRequest {
            session_id: SessionId::new(),
            program: "sh".into(),
            args: vec!["-c".into(), "sleep 0.1; printf runtime-complete".into()],
            cwd: ".".into(),
            environment: Default::default(),
            rows: 24,
            cols: 80,
            max_output_bytes: 1024,
        })
        .expect("start");

    let initial = supervisor
        .output_after(descriptor.pty_id, 0)
        .expect("initial output");
    assert_eq!(initial.status, PtyStatus::Running);
    drop(initial);

    let mut cursor = 0;
    let mut content = String::new();
    for _ in 0..20 {
        let batch = supervisor
            .wait_for_output(descriptor.pty_id, cursor, Duration::from_millis(250))
            .await
            .expect("reconnect output");
        cursor = batch.next;
        for chunk in batch.chunks {
            content.push_str(&chunk.data);
        }
        if batch.status != PtyStatus::Running {
            break;
        }
    }
    assert!(content.contains("runtime-complete"));
    assert_eq!(
        supervisor
            .descriptor(descriptor.pty_id)
            .expect("descriptor")
            .status,
        PtyStatus::Exited
    );
}

#[tokio::test]
async fn output_is_bounded_and_resumable_by_cursor() {
    let programs = discover_programs(&["sh"]);
    if !programs.contains_key("sh") {
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let supervisor = PtySupervisor::new(directory.path(), programs, BTreeSet::new())
        .expect("supervisor");
    let descriptor = supervisor
        .start(StartPtyRequest {
            session_id: SessionId::new(),
            program: "sh".into(),
            args: vec!["-c".into(), "yes x | head -c 4096".into()],
            cwd: ".".into(),
            environment: Default::default(),
            rows: 24,
            cols: 80,
            max_output_bytes: 512,
        })
        .expect("start");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let batch = supervisor
        .output_after(descriptor.pty_id, 0)
        .expect("output");
    assert!(batch.total_retained_bytes <= 512);
    assert!(batch.chunks.iter().any(|chunk| chunk.truncated));
    let empty = supervisor
        .output_after(descriptor.pty_id, batch.next)
        .expect("resumed output");
    assert!(empty.chunks.is_empty());
}
