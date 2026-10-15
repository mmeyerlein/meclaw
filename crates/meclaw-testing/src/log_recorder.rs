//! One process-wide `tracing` recorder for integration tests that assert a log
//! line.
//!
//! Lines emitted on the colony's task are only seen by the GLOBAL default
//! subscriber, and `set_global_default` may be called once per process. Under
//! nextest every test is its own process, but a plain `cargo test --test <bin>`
//! runs a whole harness binary in one: two tests that each installed their own
//! global subscriber panicked there (GH #1176 re-check). Every test that reads
//! the log goes through [`global`] instead, and filters by something only it
//! emits (its colony root, its cell path), because the tests of a binary run
//! concurrently.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// Every event of the process as `(level, " name=value" for each field)`;
/// the message is the field `message`.
#[derive(Clone, Default)]
pub struct LogRecorder {
    log: Arc<Mutex<Vec<(tracing::Level, String)>>>,
}

impl LogRecorder {
    /// Every recorded event at `level` whose flattened fields contain all
    /// `needles`.
    #[must_use]
    pub fn lines(&self, level: tracing::Level, needles: &[&str]) -> Vec<String> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(l, m)| *l == level && needles.iter().all(|n| m.contains(n)))
            .map(|(_, m)| m.clone())
            .collect()
    }
}

/// Flattens every field of an event into one string.
struct AllFields(String);

impl tracing::field::Visit for AllFields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={:?}", field.name(), value));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push_str(&format!(" {}={}", field.name(), value));
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.push_str(&format!(" {}={}", field.name(), value));
    }
}

impl tracing::Subscriber for LogRecorder {
    fn enabled(&self, _meta: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut v = AllFields(String::new());
        event.record(&mut v);
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((*event.metadata().level(), v.0));
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The process's recorder, installed as the global default on first use.
///
/// A lock poisoned by a panicking test still yields the lines recorded so far.
/// When another global subscriber was installed before (a test that bypasses
/// this module), the recorder stays empty, says so on stderr, and the log pin
/// that reads it fails on its own assertion instead of panicking here.
pub fn global() -> &'static LogRecorder {
    static REC: OnceLock<LogRecorder> = OnceLock::new();
    REC.get_or_init(|| {
        let r = LogRecorder::default();
        if let Err(e) = tracing::subscriber::set_global_default(r.clone()) {
            eprintln!(
                "log_recorder: another global subscriber is installed in this test binary: {e}"
            );
        }
        r
    })
}
