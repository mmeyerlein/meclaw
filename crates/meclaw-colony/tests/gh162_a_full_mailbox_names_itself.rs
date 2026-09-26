//! GH #162 — a full mailbox says which mailbox.
//!
//! Until GH #850, `route()` delivered with `entry.handle.send(routed).await` and
//! the waiter was the colony's own routing loop: while one mailbox was full the
//! colony routed nothing at all, and the corridor is byte-frozen and silent.
//! What that looked like during GH #161 was a colony that stopped after twenty
//! seconds with an **empty** dead-letter queue and **nothing in the message
//! log**. GH #162 added a pre-check at the call site that named the mailbox
//! before the loop blocked on it.
//!
//! Since GH #850 (R-SN-5, ADR-0045) a full mailbox no longer blocks: the
//! message goes to the cell's overflow and the colony keeps routing. The line
//! stays — it is the one moment an operator needs to know about — and now says
//! `mailbox_overflow`, once, when the cell's overflow comes into being. The
//! reason `mailbox_full` is reserved for the overflow's cap, where a message is
//! really refused. This file proves the line is there, names the mailbox, does
//! not claim `mailbox_full`, and that a colony routing normally emits neither.
//!
//! The recorder is hand-rolled on `tracing` alone (same approach as
//! `gh80_edge_condition_log_level.rs`); no crate is added to read one log line.
//! It is installed as the GLOBAL default because the line is emitted on the
//! colony's task, not on the test's.

use meclaw_colony::ColonyMsg;
use meclaw_core::{Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{mpsc, oneshot};

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
/// for the target path as easily as for the message.
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
/// be called once per process, and the events under test are emitted on the
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

/// Register a cell whose mailbox holds exactly ONE message and which never
/// drains: the receiver is handed back to the test and never read.
///
/// A mailbox of one makes the condition exact rather than statistical — the first
/// delivery fills it, the second is the one that blocks.
async fn register_never_draining(h: &ColonyHandle, path: Path) -> mpsc::Receiver<Message> {
    let (sender, receiver) = mpsc::channel::<Message>(1);
    let (peace_tx, peace_rx) = oneshot::channel();
    let (_backstop_tx, backstop_rx) = oneshot::channel();
    // A task that never ends and never receives: the cell is "alive" as far as
    // the watcher is concerned, so nothing else in the colony reacts.
    let join = tokio::spawn(async move {
        let _peace_keep = peace_tx;
        std::future::pending::<()>().await;
    });
    let (ack, ack_rx) = oneshot::channel();
    h.runtime()
        .inbox_tx
        .send(ColonyMsg::Register {
            path,
            sender,
            join,
            peace_rx,
            backstop_rx,
            stop_tx: None,
            death_ack_rx: None,
            respawn: Box::new(|| unreachable!("this cell never dies")),
            wake: None,
            restart_limit: None,
            cell_id: Uuid::now_v7(),
            cell_type: "never-drains".into(),
            active: true,
            ack,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("register ack");
    receiver
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_mailbox_names_itself_and_the_colony_keeps_routing() {
    let rec = recorder();
    let td = tempfile::TempDir::new().unwrap();
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);
    let _held = register_never_draining(&h, Path::new("/wedged")).await;

    // Fills the single slot. At the moment of the pre-check the mailbox still had
    // room, so this delivery must NOT warn.
    h.send(MessageBuilder::new(Path::new("/wedged")).build())
        .await;

    // The second and third deliveries find capacity 0: the first of them opens
    // the cell's overflow (one line), the second joins it (no second line).
    h.send(MessageBuilder::new(Path::new("/wedged")).build())
        .await;
    h.send(MessageBuilder::new(Path::new("/wedged")).build())
        .await;

    // Failure marker generous (30 s convention); the discriminator is the
    // presence of the line, not its latency.
    let mut hits = Vec::new();
    for _ in 0..300 {
        hits = rec.warnings_containing("mailbox_overflow", "/wedged");
        if !hits.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        hits.len(),
        1,
        "exactly one line, when the cell's overflow came into being, got: {hits:?}"
    );
    assert!(
        hits[0].contains("/wedged"),
        "the line must NAME the mailbox — that is the whole issue: {}",
        hits[0]
    );
    assert!(
        hits[0].contains("mailbox_capacity=1"),
        "and say how big it is, so `cell.mailbox_size` is actionable: {}",
        hits[0]
    );
    assert!(
        rec.warnings_containing("mailbox_full", "/wedged")
            .is_empty(),
        "`mailbox_full` is the cap's word — below the cap nothing is refused"
    );

    // The colony did not stop: it still routes, and it shuts down cleanly.
    let (ack, ack_rx) = oneshot::channel();
    h.runtime()
        .inbox_tx
        .send(ColonyMsg::ReadInboundEdges {
            of: Path::new("/wedged"),
            ack,
        })
        .await
        .expect("colony inbox closed");
    tokio::time::timeout(std::time::Duration::from_secs(30), ack_rx)
        .await
        .expect("the loop answers while the mailbox is full")
        .expect("ack");
    h.shutdown().await;
}

/// The other half: a colony delivering normally must stay quiet. A pre-check that
/// warned on every delivery would be worse than none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_colony_with_room_in_its_mailboxes_says_nothing() {
    let rec = recorder();
    let before = rec.warnings_containing("mailbox_full", "/fine").len()
        + rec.warnings_containing("mailbox_overflow", "/fine").len();
    let td = tempfile::TempDir::new().unwrap();
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);
    h.spawn(Path::new("/fine"), || EchoMockCell::new(Path::new("/fine")))
        .await;

    for _ in 0..20 {
        h.send(MessageBuilder::new(Path::new("/fine")).build())
            .await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    assert_eq!(
        rec.warnings_containing("mailbox_full", "/fine").len()
            + rec.warnings_containing("mailbox_overflow", "/fine").len(),
        before,
        "a healthy delivery path must not emit the line"
    );
    h.shutdown().await;
}
