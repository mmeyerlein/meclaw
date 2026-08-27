//! GH #47: a drain that cannot finish ends at its deadline and SAYS WHAT IT
//! LEFT. An unbounded drain would turn one hung cell into a process that systemd
//! has to SIGKILL — and a silent cut would leave the next "where did that message
//! go" unanswerable.
//!
//! The recorder is hand-rolled on `tracing` alone (same approach as
//! `gh162_a_full_mailbox_names_itself.rs` and `gh80_edge_condition_log_level.rs`);
//! no crate is added to read one log line. It is installed as the GLOBAL default
//! because the line is emitted on the colony's task, not on the test's.

use meclaw_core::{Cell, Message, OutputSink, Path};
use meclaw_testing::{ColonyHandle, MessageBuilder};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Failure marker, generous per the 30s convention (robust under cargo load).
const MARKER: Duration = Duration::from_secs(30);

/// The drain budget of the hung colony. Small so the test is fast; the semantic
/// discriminator below is what carries the claim, not this value.
const HUNG_BUDGET_MS: u64 = 600;

// ---- a minimal recorder over every field of every event ----------------------

#[derive(Clone, Default)]
struct Recorder {
    log: Arc<Mutex<Vec<(tracing::Level, String)>>>,
}

impl Recorder {
    /// WARN lines carrying BOTH needles. Two needles rather than one because the
    /// recorder is global and the tests in this binary run concurrently — a line
    /// is only this test's if it also names this test's cell.
    fn warnings_containing(&self, needle: &str, and: &str) -> Vec<String> {
        self.log
            .lock()
            .expect("log mutex")
            .iter()
            .filter(|(l, m)| *l == tracing::Level::WARN && m.contains(needle) && m.contains(and))
            .map(|(_, m)| m.clone())
            .collect()
    }
}

/// Flattens every field of an event into one string, so an assertion can look
/// for the named cell as easily as for the counter.
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

impl tracing::Subscriber for Recorder {
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
            .expect("log mutex")
            .push((*event.metadata().level(), v.0));
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

/// One global recorder for the whole test binary — `set_global_default` may only
/// be called once per process, and the event under test is emitted on the
/// colony's task.
fn recorder() -> &'static Recorder {
    static REC: OnceLock<Recorder> = OnceLock::new();
    REC.get_or_init(|| {
        let r = Recorder::default();
        tracing::subscriber::set_global_default(r.clone())
            .expect("no other subscriber may be installed in this test binary");
        r
    })
}

/// A cell whose `handle()` announces that it started and then never returns.
///
/// The announcement is the positive receipt: the cell TOOK the message out of
/// its mailbox, so the mailbox is empty and the only thing still owed is the
/// handler itself — which is exactly what the deadline has to cut.
struct HungCell {
    entered: mpsc::Sender<()>,
}

impl Cell for HungCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(
        &mut self,
        _msg: Message,
        _sink: &OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send {
        let entered = self.entered.clone();
        async move {
            let _ = entered.send(()).await;
            std::future::pending::<()>().await;
        }
    }
}

/// A cell whose `handle()` never returns costs the drain its budget and no more,
/// and the warning names the cell.
///
/// The two-sided timing assertion is the semantic discriminator and is
/// deliberately tight: a one-sided ">600 ms" would also pass if the drain simply
/// hung, and a one-sided "<5 s" would pass if there were no drain at all. Only
/// the pair says "it waited, AND it stopped waiting".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hung_handler_ends_the_drain_at_the_deadline_and_is_named() {
    /// SEMANTIC lower bound: the drain really waited out its budget.
    const REALLY_WAITED: Duration = Duration::from_millis(HUNG_BUDGET_MS);
    /// SEMANTIC upper bound: the deadline really cut. Eight times the budget, so
    /// scheduler jitter cannot reach it, and still an order of magnitude below
    /// anything that would read as "the colony hung".
    const REALLY_CUT: Duration = Duration::from_secs(5);

    let rec = recorder();
    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        td.path().join("colony.json"),
        format!(r#"{{"shutdown_drain_timeout_ms": {HUNG_BUDGET_MS}}}"#),
    )
    .expect("write the test colony.json");
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    let (entered_tx, mut entered_rx) = mpsc::channel::<()>(4);
    h.spawn(Path::new("/hung"), {
        let entered = entered_tx.clone();
        move || HungCell {
            entered: entered.clone(),
        }
    })
    .await;

    h.send_from(Path::new("/"), MessageBuilder::new("/hung").build())
        .await;
    tokio::time::timeout(MARKER, entered_rx.recv())
        .await
        .expect("the hung handler must be entered within the failure marker")
        .expect("the entered-channel sender lives in the cell");

    let started = Instant::now();
    tokio::time::timeout(MARKER, h.shutdown())
        .await
        .expect("the shutdown must return within the failure marker");
    let took = started.elapsed();

    assert!(
        took >= REALLY_WAITED,
        "the drain must actually wait out its {HUNG_BUDGET_MS} ms budget before \
         it cuts — took {took:?}"
    );
    assert!(
        took < REALLY_CUT,
        "one hung handler must not hold the colony past its deadline — took {took:?}"
    );

    let named = rec.warnings_containing("drain_incomplete", "/hung");
    assert_eq!(
        named.len(),
        1,
        "the cut drain must say exactly once what it left behind, got: {named:?}"
    );
    assert!(
        named[0].contains("drain_incomplete=1"),
        "the warning must count the outstanding delivery: {}",
        named[0]
    );
    assert!(
        named[0].contains("busy=/hung"),
        "the warning must NAME the cell that is still busy: {}",
        named[0]
    );
}

/// The counter-test: a colony with nothing in flight does not spend its budget.
/// Without this, every one of the existing `Shutdown` call sites would pay the
/// drain deadline and the suite would crawl.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_colony_shuts_down_immediately() {
    let h = ColonyHandle::new(); // budget is the harness default
    let t0 = std::time::Instant::now();
    h.shutdown().await;
    assert!(
        t0.elapsed() < std::time::Duration::from_millis(500),
        "an idle colony must not sit out its drain budget, took {:?}",
        t0.elapsed()
    );
}
