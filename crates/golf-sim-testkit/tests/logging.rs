use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use golf_sim_base::digest::digest;
use golf_sim_base::log::tracing::{self, Event, Level, Metadata, Subscriber, span};
use golf_sim_testkit::determinism::LAYERS;
use serde_json::{Value, json};

#[test]
fn trace_logging_preserves_every_determinism_digest() {
    let without_logging = suite_digests();
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let subscriber = JsonEvents {
        buffer: buffer.clone(),
        next_span: AtomicU64::new(1),
    };
    let with_logging = tracing::subscriber::with_default(subscriber, suite_digests);
    assert_eq!(with_logging, without_logging);

    let buffer = buffer.lock().unwrap();
    let item_events = std::str::from_utf8(&buffer)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["name"] == "determinism_item" && event["level"] == "TRACE")
        .count();
    assert!(!with_logging.is_empty());
    assert_eq!(item_events, with_logging.len());
    println!(
        "{} digests matched across {} registered layers with {item_events} TRACE item events",
        with_logging.len(),
        LAYERS.len()
    );
}

fn suite_digests() -> Vec<(&'static str, &'static str, String)> {
    let mut digests = Vec::new();
    for (layer, items) in LAYERS {
        for item in items() {
            tracing::event!(name: "determinism_item", Level::TRACE, "running determinism item");
            let hash = digest(&(item.run)()).unwrap_or_else(|e| panic!("{layer}/{}: {e}", item.id));
            digests.push((*layer, item.id, hash));
        }
    }
    digests
}

struct JsonEvents {
    buffer: Arc<Mutex<Vec<u8>>>,
    next_span: AtomicU64,
}

impl Subscriber for JsonEvents {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed))
    }

    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
    fn enter(&self, _: &span::Id) {}
    fn exit(&self, _: &span::Id) {}

    fn event(&self, event: &Event<'_>) {
        let metadata = event.metadata();
        let mut buffer = self.buffer.lock().unwrap();
        serde_json::to_writer(
            &mut *buffer,
            &json!({ "name": metadata.name(), "level": metadata.level().as_str() }),
        )
        .unwrap();
        buffer.write_all(b"\n").unwrap();
    }
}
