use sessionweft_client_protocol::{EventCursor, EventJournal};
use sessionweft_client_protocol_sqlite::SqliteClientEventJournal;
use sessionweft_core::{EventEnvelope, SessionId};
use uuid::Uuid;

#[tokio::test]
async fn cursor_resumes_after_journal_reopen_and_duplicate_append_is_idempotent() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database_url = format!("sqlite://{}", directory.path().join("client.db").display());
    let journal = SqliteClientEventJournal::connect(&database_url)
        .await
        .expect("journal");
    let first = EventEnvelope::new(
        "session.created",
        Some(SessionId::new()),
        Uuid::new_v4(),
        Some("test"),
        serde_json::json!({"version": 0}),
    );
    let second = EventEnvelope::new(
        "workflow.started",
        Some(SessionId::new()),
        Uuid::new_v4(),
        Some("test"),
        serde_json::json!({"version": 1}),
    );
    let first_record = journal.append(&first).await.expect("first append");
    let duplicate = journal.append(&first).await.expect("duplicate append");
    assert_eq!(duplicate.cursor, first_record.cursor);
    let second_record = journal.append(&second).await.expect("second append");
    drop(journal);

    let reopened = SqliteClientEventJournal::connect(&database_url)
        .await
        .expect("reopened journal");
    let batch = reopened
        .list_after(first_record.cursor, 100)
        .await
        .expect("resume batch");
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.events[0].envelope.event_id, second.event_id);
    assert_eq!(batch.next, second_record.cursor);
    assert_eq!(
        reopened.latest_cursor().await.expect("latest"),
        second_record.cursor
    );
    assert!(
        reopened
            .list_after(EventCursor(0), 100)
            .await
            .expect("all events")
            .events
            .len()
            == 2
    );
}
