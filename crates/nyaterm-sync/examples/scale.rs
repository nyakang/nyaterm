//! Reproducible synthetic workload; never reads actual user configuration.
use std::time::Instant;

use nyaterm_core::sync_schema::{SyncProjection, SyncScope, SyncValues};
use nyaterm_sync::engine::prepare;
use serde_json::json;
use uuid::Uuid;

fn main() {
    for count in [1_000, 5_000, 10_000] {
        let mut current = SyncProjection::default();
        current.complete.insert("connection".into());
        for index in 0..count {
            current.record("connection", &format!("resource-{index}"), &json!({"name":format!("Server {index}"), "description":"Synthetic performance fixture", "endpoint":{"type":"ssh","host":"example.invalid","port":22,"username":"user"},"tags":["fixture"]})).unwrap();
        }
        let actor = Uuid::new_v4();
        let started = Instant::now();
        let first = prepare(
            actor,
            None,
            &SyncValues::new(),
            current.clone(),
            &[],
            SyncScope::default(),
        )
        .unwrap();
        let initial = started.elapsed();
        let started = Instant::now();
        let next = prepare(
            actor,
            Some(&first.replica),
            &first.effective,
            current,
            &[],
            SyncScope::default(),
        )
        .unwrap();
        assert_eq!(first.replica.document, next.replica.document);
        println!(
            "resources={count} initial_ms={} unchanged_ms={} document_bytes={}",
            initial.as_millis(),
            started.elapsed().as_millis(),
            first.replica.document.len()
        );
    }
}
