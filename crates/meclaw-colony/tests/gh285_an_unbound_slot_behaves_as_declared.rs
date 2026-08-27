//! GH #285 — what a message MEETS at a declared slot that nothing is bound
//! behind yet.
//!
//! Tasks 8–10 made a slot declarable, exempt from the dangling-endpoint finding,
//! and wireable by mutation. All three are statements about the TOPOLOGY. This
//! file is about the one moment the declaration was written for: a message is
//! standing at the address, and there is nothing there.
//!
//! Two hives differ in exactly one word — `"unbound": "drop"` against
//! `"unbound": "error"` — and in nothing else. Same shape, same slot name, same
//! kind of inbound lane, same emitting cell type. That is the whole control: if
//! the two lanes end differently, the word is what ended them.
//!
//! * the `drop` lane produces no delivery and NO dead letter — the hive said the
//!   absence is normal, and a substrate that logged an incident for it would be
//!   contradicting the declaration it was given;
//! * the `error` lane produces exactly one dead letter, `slot_unbound`, whose
//!   resolved target is the slot address — the hive path and the slot name, the
//!   two things an operator needs to find the topology that is not finished.
//!
//! The third case is the point of the whole feature: the declaration governs the
//! UNBOUND state and nothing else. A mutation installs a real node at the
//! `error` slot's address, the very same emission runs again, and it is
//! delivered — proven by the occupant's own `cell.db` counter, not by the
//! absence of a complaint.
//!
//! # The third word: `park` (W4 Task 12)
//!
//! `park` is the declaration that says "not yet", and the only one of the three
//! that owes the message a future. A parked slot holds a bounded FIFO queue and
//! releases it, in emission order, to whatever a binding mutation installs at
//! the address. Two things are pinned here that no amount of counting can show
//! on its own:
//!
//! * **order**, read off the colony's own `message_log` in insertion order. The
//!   probes carry distinct TTLs, which ride the envelope rather than the body,
//!   so each released delivery is identifiable without a single new cell type;
//! * **which one dies on overflow**: with `slot_park_max: 1` the SECOND message
//!   dead-letters as `slot_park_overflow` and the FIRST is the one that is
//!   later delivered. Newest dies, earliest context survives.
//!
//! # How "no delivery, no dead letter" is proven positively
//!
//! Absence is not evidence, so every drop assertion rides on a POSITIVE receipt
//! that orders the observation after the event. Each caller fans out over two
//! out-edges — the slot AND a real sink. Both decisions are handled inside one
//! outputs-arm event, so once the sink's `cell.db` counter reads `1` the slot
//! decision has been resolved too, and only then is the dead-letter queue read.
//! For the hive-transit half the same job is done by a message sent afterwards
//! into the inbox, which the colony's single loop cannot handle out of order.

use meclaw_colony::bootstrap_from_filesystem;
use meclaw_colony::dead_letter::DeadLetterReason;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome};
use meclaw_core::{Body, MessageBuilder, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use meclaw_testing::wait::wait_for_cell_db_value;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

/// The three slot addresses under test. They differ in their hive's declared
/// `unbound` word and in nothing else.
const DROP_SLOT: &str = "/hd/gen";
const ERROR_SLOT: &str = "/he/gen";
const PARK_SLOT: &str = "/hp/gen";

/// The park slot of the hive that binds and re-addresses itself in ONE event.
const OVERTAKE_SLOT: &str = "/hx/gen";

/// The slot a hive declares that only a MUTATION brings into the world — the
/// declaration this colony did not have when it booted.
const LATE_SLOT: &str = "/hn/gen";

const CONTRACT: &str = r#""contract":{"version":"0.1.0","settings":{},"consumes":{}}"#;

/// Generous, per the 30 s failure-marker convention — these are liveness
/// barriers, not timing discriminators.
const BARRIER: std::time::Duration = std::time::Duration::from_secs(30);

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// An emitting `persist_mock`: it writes a counter into its own `cell.db` on
/// every message and emits once. `emitted_target` is a FIELD, not a route — the
/// out-edges below decide where the emission goes (GH #226).
fn caller_config(emitted_target: &str) -> String {
    format!(
        r#"{{"cell":{{"type":"persist_mock","idle_timeout_ms":60000}},"params":{{"emitted_target":"{emitted_target}"}},{CONTRACT}}}"#
    )
}

/// A terminal `persist_mock`: it records the delivery and emits nothing, so its
/// counter is a receipt and never a source of further routing.
const SINK_CONFIG: &str = concat!(
    r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"#,
    r#""contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#
);

/// A hive declaring `gen` as a slot with the given `unbound` word, plus the
/// mandated inward lane `<hive> → <hive>/gen` that the hive-transit half rides
/// on.
fn slot_hive_config(unbound: &str) -> String {
    format!(
        r#"{{"cell":{{"type":"hive"}},"params":{{
             "ports":[{{"name":"gen","slot":true,"unbound":"{unbound}"}}],
             "graph":{{"edges":[{{"from":".","to":"./gen"}}]}}
           }}}}"#
    )
}

/// The root hive: the callers, their sinks, the slot hives, and the lanes that
/// make each caller fan out over its slot AND its sink.
///
/// `caller_n` has no lane at all here: its hive `/hn` does not exist until a
/// mutation instantiates it, and neither does the slot it declares.
const ROOT_CONFIG: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
    {"from":"./caller_d","to":"./hd/gen"},
    {"from":"./caller_d","to":"./sink_d"},
    {"from":"./caller_e","to":"./he/gen"},
    {"from":"./caller_e","to":"./sink_e"},
    {"from":"./caller_p","to":"./hp/gen"},
    {"from":"./caller_p","to":"./sink_p"},
    {"from":"./caller_x","to":"./hx/gen"},
    {"from":"./caller_x","to":"./sink_x"}
]}}}"#;

/// The hive that binds a slot and then addresses it again **inside one event**.
///
/// Its one out-edge goes to `/colony/mutations` — boot-time edge validation
/// admits `/colony/*` targets, so this is a topology a colony can really be
/// booted with. A probe carrying a mutation body and a `reply_to` of the hive's
/// own park slot therefore produces, in ONE work drain: the dispatch that
/// installs the occupant, and — pushed onto the same drain by the dispatcher's
/// own reply — a message for the address that became bound one item ago.
///
/// The reply is the trigger rather than a second out-edge because the reply is
/// ORDERED: it cannot exist before the dispatch that produced it, whereas two
/// out-edges of one node fire in whatever order the edge table yields.
const OVERTAKE_HIVE_CONFIG: &str = r#"{"cell":{"type":"hive"},"params":{
     "ports":[{"name":"gen","slot":true,"unbound":"park"}],
     "graph":{"edges":[
       {"from":".","to":"/colony/mutations"}
     ]}
   }}"#;

/// A hive whose declared port owes a drain (GH #147), so that a mutation which
/// wires that port without the drain is refused in the APPLY stage.
///
/// That is the only reject this suite can reach which returns AFTER the apply
/// sequence has already registered a subtree's hive scopes — the validate-stage
/// rejects all return before any of it. Dormant at boot: nothing outside wires
/// into `/hp/hr/gen` until a mutation does.
const DRAIN_HIVE_CONFIG: &str = r#"{"cell":{"type":"hive"},"params":{
     "ports":["gen"],
     "required_drains":[
       {"port":"gen","hop":{"route":"reject"},
        "because":"a refusal has to leave the hive"}
     ]
   }}"#;

/// A hive template whose `config.json` declares a `park` slot — the only way a
/// slot declaration can enter a colony that is already running.
///
/// It carries one real child so the instantiation goes down the SUBTREE path
/// (a hive is a scope marker, never a staged cell), which is also the path that
/// registers the new hive scope.
const PARK_HIVE_TEMPLATE: &str = r#"{"cell":{"type":"hive"},"params":{
     "ports":[{"name":"gen","slot":true,"unbound":"park"}],
     "graph":{"edges":[{"from":".","to":"./keeper"}]}
   }}"#;

/// A live colony holding both lanes. Booted through the real
/// `bootstrap_from_filesystem`, so the slot declarations are read out of the
/// very `config.json` files the boot walked.
async fn live_colony() -> (tempfile::TempDir, ColonyHandle) {
    live_colony_with_config(None).await
}

/// The same colony, with an optional `colony.json` written into the root before
/// the handle reads it — the only way to move `slot_park_max` off its default.
async fn live_colony_with_config(colony_json: Option<&str>) -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    if let Some(cfg) = colony_json {
        std::fs::write(root.join("colony.json"), cfg).unwrap();
    }
    write(root, "main/config.json", ROOT_CONFIG);
    write(root, "main/caller_d/config.json", &caller_config(DROP_SLOT));
    write(
        root,
        "main/caller_e/config.json",
        &caller_config(ERROR_SLOT),
    );
    write(root, "main/caller_p/config.json", &caller_config(PARK_SLOT));
    write(root, "main/caller_n/config.json", &caller_config(LATE_SLOT));
    write(
        root,
        "main/caller_x/config.json",
        &caller_config(OVERTAKE_SLOT),
    );
    write(root, "main/sink_d/config.json", SINK_CONFIG);
    write(root, "main/sink_e/config.json", SINK_CONFIG);
    write(root, "main/sink_p/config.json", SINK_CONFIG);
    write(root, "main/sink_n/config.json", SINK_CONFIG);
    write(root, "main/sink_x/config.json", SINK_CONFIG);
    write(root, "main/hd/config.json", &slot_hive_config("drop"));
    write(root, "main/he/config.json", &slot_hive_config("error"));
    write(root, "main/hp/config.json", &slot_hive_config("park"));
    write(root, "main/hx/config.json", OVERTAKE_HIVE_CONFIG);
    // Inside `/hp`, so ONE mutation scoped to `/hp` can both instantiate the
    // subtree at the park slot and trip the drain rule — an `add_edges` endpoint
    // outside the mutation's scope is refused before the apply stage.
    write(root, "main/hp/probe/config.json", SINK_CONFIG);
    write(root, "main/hp/hr/config.json", DRAIN_HIVE_CONFIG);
    write(root, "main/hp/hr/gen/config.json", SINK_CONFIG);
    // The template a later mutation fills a slot from.
    write(
        root,
        "templates/persist_mock/template.json",
        r#"{"name":"persist_mock"}"#,
    );
    write(root, "templates/persist_mock/config.json", SINK_CONFIG);
    // The template that brings a park-slot DECLARATION into a running colony.
    write(
        root,
        "templates/park_hive/template.json",
        r#"{"name":"park_hive"}"#,
    );
    write(root, "templates/park_hive/config.json", PARK_HIVE_TEMPLATE);
    write(root, "templates/park_hive/keeper/config.json", SINK_CONFIG);

    let factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), factory.clone())],
    );
    let mut reg = CellFactoryRegistry::new();
    reg.insert("persist_mock".into(), factory);
    bootstrap_from_filesystem(root, &reg, &h.runtime())
        .await
        .expect("a colony whose slots stand empty boots");
    (td, h)
}

/// Route one UBF message to `target`.
async fn send_to(h: &ColonyHandle, target: &str) {
    let msg = MessageBuilder::new(Path::new(target))
        .body(Body::Inline(json!({"messages": []})))
        .build();
    h.send(msg).await;
}

/// Route one UBF message to `target` with an explicit TTL.
///
/// The TTL is the identity tag for the park tests. It rides the ENVELOPE, so a
/// `persist_mock` caller — which writes its own body and would erase anything
/// carried in there — passes it on untouched: the follow-up emission inherits
/// the TTL of the message that produced it. Distinct probe TTLs therefore make
/// the queued messages distinguishable in the colony's own `message_log`
/// without inventing a cell type whose only job is to be recognisable.
async fn send_to_with_ttl(h: &ColonyHandle, target: &str, ttl: u32) {
    let msg = MessageBuilder::new(Path::new(target))
        .body(Body::Inline(json!({"messages": []})))
        .ttl(ttl)
        .build();
    h.send(msg).await;
}

/// Every delivery to `to_path` the colony has logged, in the order the rows
/// were inserted — i.e. in ROUTING order, which is what a claim about release
/// order is about. `rowid` is the insertion counter; the primary key is a
/// message id minted when the message was BUILT and would only ever re-tell the
/// emission order the test already knows.
///
/// The value read back is the logged TTL, the identity tag `send_to_with_ttl`
/// stamped on. A parked message never reaches the router, so a row for a slot
/// address exists only once the queue was released.
fn logged_deliveries(colony_root: &std::path::Path, to_path: &str) -> Vec<i64> {
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        colony_root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return Vec::new();
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT ttl FROM message_log WHERE to_path = ?1 ORDER BY rowid")
    else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([to_path], |r| r.get::<_, i64>(0)) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

/// Poll `logged_deliveries` until it holds `expected` rows. The writer thread is
/// asynchronous, so the count is the barrier that orders the assertion after
/// the release.
async fn wait_for_deliveries(
    colony_root: &std::path::Path,
    to_path: &str,
    expected: usize,
    timeout: std::time::Duration,
) -> Vec<i64> {
    let start = std::time::Instant::now();
    loop {
        let got = logged_deliveries(colony_root, to_path);
        if got.len() >= expected {
            return got;
        }
        assert!(
            start.elapsed() < timeout,
            "{to_path}: expected {expected} logged deliveries, saw {got:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// Fill a slot with a fresh `persist_mock` — the ordinary way a slot stops
/// being empty.
async fn fill_slot(h: &ColonyHandle, hive: &str) -> MutationOutcome {
    send_mutation(
        h,
        json!({"scope": hive, "diff":{"add_nodes":[{"name":"gen","template":"persist_mock"}]}}),
    )
    .await
}

async fn send_mutation(h: &ColonyHandle, payload: meclaw_core::JsonValue) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

// ---------------------------------------------------------------------------
// 1. A cell emission onto an unbound slot.
// ---------------------------------------------------------------------------

/// Both lanes, one colony, one drain: `drop` contributes nothing, `error`
/// contributes exactly one `slot_unbound` naming the slot address.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unbound_slot_drops_or_errors_as_its_hive_declared() {
    let (td, h) = live_colony().await;

    send_to(&h, "/caller_d").await;
    send_to(&h, "/caller_e").await;

    // Positive receipts: each caller's OTHER decision arrived. Both decisions of
    // one emission are handled inside one outputs-arm event, so the slot
    // decision is resolved by the time these read `1`.
    wait_for_cell_db_value(&td.path().join("main/sink_d"), "counter", "1", BARRIER).await;
    wait_for_cell_db_value(&td.path().join("main/sink_e"), "counter", "1", BARRIER).await;

    let dls = h.drain_dead_letters().await;

    assert!(
        !dls.iter().any(|d| d.resolved_target.as_str() == DROP_SLOT),
        "a `drop` slot discards the message without an incident; got {:?}",
        dls.iter()
            .map(|d| (d.resolved_target.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        dls.len(),
        1,
        "the `error` lane is the only one that may speak; got {:?}",
        dls.iter()
            .map(|d| (d.resolved_target.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
    let dl = &dls[0];
    assert_eq!(
        dl.reason,
        DeadLetterReason::SlotUnbound,
        "an unbound `error` slot is its own diagnosis, not `unresolved_path`"
    );
    assert_eq!(dl.reason.as_code(), "slot_unbound");
    assert_eq!(
        dl.resolved_target.as_str(),
        ERROR_SLOT,
        "the entry must name the hive path and the slot name"
    );
    assert_eq!(
        dl.sender_path.as_str(),
        "/caller_e",
        "the sender is the cell whose emission met the empty slot"
    );

    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// 2. A hive's own out-edge onto an unbound slot — the second delivery site.
// ---------------------------------------------------------------------------

/// A message that transits a hive onto its own unbound slot meets the same
/// declaration. `hive_no_route` is NOT the answer here: the edge matched, the
/// address is declared, and what happens at it is the hive's call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hive_transit_onto_an_unbound_slot_obeys_the_declaration() {
    let (td, h) = live_colony().await;

    // The `drop` half. `/sink_d` is addressed afterwards through the same inbox,
    // which the colony's single loop handles in order — so its counter is a
    // barrier past the transit above.
    send_to(&h, "/hd").await;
    send_to(&h, "/sink_d").await;
    wait_for_cell_db_value(&td.path().join("main/sink_d"), "counter", "1", BARRIER).await;
    let dls = h.drain_dead_letters().await;
    assert!(
        dls.is_empty(),
        "a transit onto a `drop` slot leaves nothing behind; got {:?}",
        dls.iter()
            .map(|d| (d.resolved_target.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );

    // The `error` half.
    send_to(&h, "/he").await;
    send_to(&h, "/sink_e").await;
    wait_for_cell_db_value(&td.path().join("main/sink_e"), "counter", "1", BARRIER).await;
    let dls = h.drain_dead_letters().await;
    assert_eq!(
        dls.len(),
        1,
        "a transit onto an `error` slot is exactly one incident; got {:?}",
        dls.iter()
            .map(|d| (d.resolved_target.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
    assert_eq!(dls[0].reason, DeadLetterReason::SlotUnbound);
    assert_eq!(dls[0].resolved_target.as_str(), ERROR_SLOT);

    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// 3. The declaration governs the UNBOUND state and nothing else.
// ---------------------------------------------------------------------------

/// Fill the `error` slot by mutation and run the very same emission again: it is
/// delivered, and the occupant's own `cell.db` says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_filled_slot_delivers_and_the_declaration_falls_silent() {
    let (td, h) = live_colony().await;
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/he","diff":{"add_nodes":[{"name":"gen","template":"persist_mock"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "filling a declared slot is the ordinary way it stops being empty; got {outcome:?}"
    );

    send_to(&h, "/caller_e").await;

    // The receipt is the OCCUPANT's, not the absence of a complaint.
    wait_for_cell_db_value(&td.path().join("main/he/gen"), "counter", "1", BARRIER).await;
    wait_for_cell_db_value(&td.path().join("main/sink_e"), "counter", "1", BARRIER).await;

    let dls = h.drain_dead_letters().await;
    assert!(
        dls.is_empty(),
        "a bound slot is an ordinary address — the declaration has nothing to say; got {:?}",
        dls.iter()
            .map(|d| (d.resolved_target.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );

    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// 4. `park` — the declaration that owes the message a future (W4 Task 12).
// ---------------------------------------------------------------------------

/// Three emissions meet an unbound `park` slot: nothing is delivered, nothing
/// is complained about. A mutation then binds the address and all three arrive
/// at the occupant — in the order they were emitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_park_slot_holds_its_queue_and_releases_it_in_emission_order() {
    let (td, h) = live_colony().await;
    rescan_templates(&h, td.path().join("templates")).await;

    for ttl in [21u32, 22, 23] {
        send_to_with_ttl(&h, "/caller_p", ttl).await;
    }

    // Positive receipt: the OTHER decision of each of the three emissions
    // arrived. All decisions of one emission are handled inside one outputs-arm
    // event, so once this reads `3` the three slot decisions are resolved too.
    wait_for_cell_db_value(&td.path().join("main/sink_p"), "counter", "3", BARRIER).await;

    let dls = h.drain_dead_letters().await;
    assert!(
        dls.is_empty(),
        "`park` is not `error` — a held message is not an incident; got {:?}",
        dls.iter()
            .map(|d| (d.resolved_target.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
    assert!(
        logged_deliveries(&h.tempdir_path(), PARK_SLOT).is_empty(),
        "`park` is not `drop` either — nothing may reach the address while it is empty"
    );

    let outcome = fill_slot(&h, "/hp").await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "filling a park slot is the ordinary way it stops being empty; got {outcome:?}"
    );

    // The occupant's OWN receipt: three deliveries, counted in its `cell.db`.
    wait_for_cell_db_value(&td.path().join("main/hp/gen"), "counter", "3", BARRIER).await;

    // And the order. The probes went out with rising TTLs, so a FIFO release
    // reads as a rising sequence; a LIFO release, or any reordering, does not.
    let ttls = wait_for_deliveries(&h.tempdir_path(), PARK_SLOT, 3, BARRIER).await;
    assert_eq!(
        ttls.len(),
        3,
        "exactly the three that were held; got {ttls:?}"
    );
    assert!(
        ttls.windows(2).all(|w| w[0] < w[1]),
        "the queue is a FIFO — released in emission order; got {ttls:?}"
    );

    let dls = h.drain_dead_letters().await;
    assert!(
        dls.is_empty(),
        "a released queue leaves no incident behind; got {:?}",
        dls.iter()
            .map(|d| (d.resolved_target.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );

    h.shutdown().await;
}

/// The queue is bounded, and the bound is loud. With `slot_park_max: 1` the
/// SECOND message dead-letters as `slot_park_overflow` while the FIRST stays
/// held — and it is the first that is delivered when the slot is filled.
///
/// Which end dies is the whole point: dropping the earliest would silently
/// rewrite the beginning of a conversation, and the beginning is the part a
/// later reader cannot reconstruct.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_park_queue_refuses_the_newest_and_keeps_the_earliest() {
    let (td, h) = live_colony_with_config(Some(r#"{"slot_park_max": 1}"#)).await;
    rescan_templates(&h, td.path().join("templates")).await;

    send_to_with_ttl(&h, "/caller_p", 21).await;
    send_to_with_ttl(&h, "/caller_p", 31).await;
    wait_for_cell_db_value(&td.path().join("main/sink_p"), "counter", "2", BARRIER).await;

    let dls = h.drain_dead_letters().await;
    assert_eq!(
        dls.len(),
        1,
        "one message over the bound is one incident; got {:?}",
        dls.iter()
            .map(|d| (d.resolved_target.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
    assert_eq!(dls[0].reason.as_code(), "slot_park_overflow");
    assert_eq!(
        dls[0].resolved_target.as_str(),
        PARK_SLOT,
        "the entry must name the slot whose queue is full"
    );
    let refused_ttl = i64::from(dls[0].message.ttl);
    assert!(
        refused_ttl > 21,
        "the NEWEST message is the one that is refused; got ttl {refused_ttl}"
    );

    let outcome = fill_slot(&h, "/hp").await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{outcome:?}"
    );

    wait_for_cell_db_value(&td.path().join("main/hp/gen"), "counter", "1", BARRIER).await;
    let ttls = wait_for_deliveries(&h.tempdir_path(), PARK_SLOT, 1, BARRIER).await;
    assert_eq!(ttls.len(), 1, "only the held one is released; got {ttls:?}");
    assert!(
        ttls[0] < refused_ttl,
        "the survivor is the EARLIEST message, not the newest; got {ttls:?} against {refused_ttl}"
    );

    h.shutdown().await;
}

/// A slot declaration that did not exist at boot still governs.
///
/// This is the regression lock for the slot table's refresh: a mutation
/// instantiates a hive whose own `config.json` declares a `park` slot, an
/// emission onto that address is held rather than dead-lettered as
/// `unresolved_path` — which is only possible if the table was re-read after
/// the mutation — and a second mutation then binds the address and releases the
/// queue. Every other test in this file reads declarations the boot walk saw.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slot_declared_after_boot_parks_and_releases_just_the_same() {
    let (td, h) = live_colony().await;
    rescan_templates(&h, td.path().join("templates")).await;

    // The declaration enters the running colony.
    let outcome = send_mutation(
        &h,
        json!({"diff":{"add_nodes":[{"name":"hn","template":"park_hive"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "instantiating a hive that declares a slot; got {outcome:?}"
    );
    // …and the lanes onto it. Wiring an empty declared slot is Task 10's rule.
    let outcome = send_mutation(
        &h,
        json!({"diff":{"add_edges":[
            {"from":"./caller_n","to":"./hn/gen"},
            {"from":"./caller_n","to":"./sink_n"}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "wiring the freshly declared slot; got {outcome:?}"
    );

    send_to_with_ttl(&h, "/caller_n", 21).await;
    send_to_with_ttl(&h, "/caller_n", 22).await;
    wait_for_cell_db_value(&td.path().join("main/sink_n"), "counter", "2", BARRIER).await;

    let dls = h.drain_dead_letters().await;
    assert!(
        dls.is_empty(),
        "a declaration the boot never saw still holds the message; got {:?}",
        dls.iter()
            .map(|d| (d.resolved_target.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );

    let outcome = fill_slot(&h, "/hn").await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{outcome:?}"
    );

    wait_for_cell_db_value(&td.path().join("main/hn/gen"), "counter", "2", BARRIER).await;
    let ttls = wait_for_deliveries(&h.tempdir_path(), LATE_SLOT, 2, BARRIER).await;
    assert!(
        ttls.len() == 2 && ttls[0] < ttls[1],
        "both held messages are released, in emission order; got {ttls:?}"
    );

    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// 5. What must NOT release the queue, and what must not overtake it.
// ---------------------------------------------------------------------------

/// A REJECTED mutation does not count as a binding.
///
/// The release asks the same question the delivery filter asks in the negative:
/// is a cell registered here, or a hive scope? A subtree instantiation registers
/// its hive scopes in the apply sequence and several rejects return *after* that
/// point — so without the rollback a rejected mutation leaves a marker nothing
/// stands behind, the queue is released into it, and every held message dies as
/// `hive_no_route`. The declaration promised to hold them until something is
/// really there.
///
/// The reject has to be an APPLY-stage one, because the validate stage returns
/// before a single scope is registered and would prove nothing. So the same diff
/// that instantiates the subtree at the slot also wires `/hr`'s declared port
/// without the drain it owes (GH #147) — a refusal the apply sequence reaches
/// only after step 9c has put the new scope in the table.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rejected_mutation_does_not_release_the_queue() {
    let (td, h) = live_colony().await;
    rescan_templates(&h, td.path().join("templates")).await;

    send_to_with_ttl(&h, "/caller_p", 21).await;
    send_to_with_ttl(&h, "/caller_p", 22).await;
    wait_for_cell_db_value(&td.path().join("main/sink_p"), "counter", "2", BARRIER).await;

    // The subtree lands at `/hp/gen`, the address the queue is waiting on; the
    // undrained ingress onto `/hp/hr/gen` kills the mutation afterwards.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/hp","diff":{
            "add_nodes":[{"name":"gen","template":"park_hive"}],
            "add_edges":[{"from":"./probe","to":"./hr/gen"}]
        }}),
    )
    .await;
    let MutationOutcome::Rejected { error_code, .. } = &outcome else {
        panic!("the undrained ingress must take the whole mutation down; got {outcome:?}");
    };
    assert_eq!(
        error_code, "required_drain_missing",
        "the reject must be the APPLY-stage one — a validate-stage reject never \
         registers a scope and would prove nothing; got {outcome:?}"
    );

    // A barrier past the loop head: this delivery is handled in a LATER event
    // than the reject, so the release (if it were going to happen) has happened.
    send_to(&h, "/sink_p").await;
    wait_for_cell_db_value(&td.path().join("main/sink_p"), "counter", "3", BARRIER).await;

    let dls = h.drain_dead_letters().await;
    assert!(
        dls.is_empty(),
        "nothing was bound, so nothing may be released — and nothing may die; got {:?}",
        dls.iter()
            .map(|d| (d.resolved_target.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
    assert!(
        logged_deliveries(&h.tempdir_path(), PARK_SLOT).is_empty(),
        "a rolled-back mutation leaves no address for the queue to be released into"
    );

    // And the queue is still whole: the binding that really succeeds gets both,
    // in order.
    let outcome = fill_slot(&h, "/hp").await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{outcome:?}"
    );
    wait_for_cell_db_value(&td.path().join("main/hp/gen"), "counter", "2", BARRIER).await;
    let ttls = wait_for_deliveries(&h.tempdir_path(), PARK_SLOT, 2, BARRIER).await;
    assert!(
        ttls.len() == 2 && ttls[0] < ttls[1],
        "the queue survived the reject intact and in order; got {ttls:?}"
    );

    h.shutdown().await;
}

/// A message that meets the slot AFTER the binding, but inside the SAME colony
/// event, does not overtake the queue that was waiting for that binding.
///
/// One probe transits `/hx` onto `/colony/mutations`, carrying both the diff
/// that installs the occupant and a `reply_to` pointing at `/hx/gen`. The
/// dispatcher's reply is pushed onto the SAME work drain, so it meets an address
/// that became bound one item earlier — while the parked messages, older than
/// both, would otherwise wait for the top of the loop.
///
/// The TTLs say who arrived when: the two held ones went out with 21/22 and are
/// down to the twenties by the time they are logged, while the reply is a fresh
/// message carrying the colony's default TTL. So the logged deliveries must read
/// "the two old ones, then the new one".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_binding_traffic_does_not_overtake_the_queue_it_shares_an_event_with() {
    let (td, h) = live_colony().await;
    rescan_templates(&h, td.path().join("templates")).await;

    send_to_with_ttl(&h, "/caller_x", 21).await;
    send_to_with_ttl(&h, "/caller_x", 22).await;
    wait_for_cell_db_value(&td.path().join("main/sink_x"), "counter", "2", BARRIER).await;
    assert!(
        logged_deliveries(&h.tempdir_path(), OVERTAKE_SLOT).is_empty(),
        "both are held while the slot stands empty"
    );

    // ONE message, ONE event: it commits the binding, and its own reply is the
    // traffic that must not overtake what the binding released.
    let msg = MessageBuilder::new(Path::new("/hx"))
        .body(Body::Inline(
            json!({"scope":"/hx","diff":{"add_nodes":[{"name":"gen","template":"persist_mock"}]}}),
        ))
        .reply_to(Path::new("/hx/gen"))
        .ttl(41)
        .build();
    h.send(msg).await;

    wait_for_cell_db_value(&td.path().join("main/hx/gen"), "counter", "3", BARRIER).await;
    let ttls = wait_for_deliveries(&h.tempdir_path(), OVERTAKE_SLOT, 3, BARRIER).await;
    assert_eq!(
        ttls.len(),
        3,
        "two held plus the one that bound; got {ttls:?}"
    );
    assert!(
        ttls[0] < 30 && ttls[1] < 30,
        "the two HELD messages come first — a queue is not a side channel; got {ttls:?}"
    );
    assert!(
        ttls[0] < ttls[1],
        "and they keep their own order; got {ttls:?}"
    );
    assert!(
        ttls[2] > 30,
        "the reply that rode the binding event arrives LAST; got {ttls:?}"
    );

    h.shutdown().await;
}

/// A slot can be filled by a HIVE, and the queue is released into it too.
///
/// "Bound" means a registered cell **or** a hive scope — the same question the
/// delivery filter asks in the negative. A subtree instantiation therefore ends
/// the wait just as a cell does, and because the release goes through the
/// ordinary router the messages become that hive's own out-edge decisions and
/// reach its members. Nothing in the release distinguishes the two kinds of
/// occupant; this pins that the undistinguished case really works.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queue_released_into_a_hive_reaches_the_hives_members() {
    let (td, h) = live_colony().await;
    rescan_templates(&h, td.path().join("templates")).await;

    send_to_with_ttl(&h, "/caller_p", 21).await;
    send_to_with_ttl(&h, "/caller_p", 22).await;
    wait_for_cell_db_value(&td.path().join("main/sink_p"), "counter", "2", BARRIER).await;

    // `park_hive` is a HIVE: the address becomes a scope, not a cell, and its
    // only inward edge leads to `keeper`.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/hp","diff":{"add_nodes":[{"name":"gen","template":"park_hive"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{outcome:?}"
    );

    // The receipt is the HIVE MEMBER's own counter — a hive has no `cell.db` to
    // ask, which is the whole reason this case needed its own test.
    wait_for_cell_db_value(
        &td.path().join("main/hp/gen/keeper"),
        "counter",
        "2",
        BARRIER,
    )
    .await;
    let ttls = wait_for_deliveries(&h.tempdir_path(), "/hp/gen/keeper", 2, BARRIER).await;
    assert!(
        ttls.len() == 2 && ttls[0] < ttls[1],
        "the queue reached the member in order; got {ttls:?}"
    );

    h.shutdown().await;
}
