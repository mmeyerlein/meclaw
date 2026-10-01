//! GH #929 -- every budget segment between two seams fits its reserve.
//!
//! `modifier.restore_ttl` is a contract at named seams (`docs/meclaw-overview.md`
//! § Edge model, the seam table): a fresh root (source), a peer boundary, a
//! door into the hive that owns the unit of work, the curator entry and a round
//! that loops back through routing. Between two seams a message spends its
//! budget one routing decision at a time; this file measures the longest
//! segments of the shipped roads and holds each one to
//! `MESSAGE_DEFAULT_TTL - RESERVE`: the door to the curator entry (S1), the
//! curator entry to the brain (S2), a history call (S3), a consult into the
//! core (S4) and every segment of a file turn, a retried one included (S5).
//! Not measured one by one, only guarded by "no ttl death" on the roads
//! above: the consult's way back (`answer`/`ask` to the talky's curator
//! entry), the tail from the brain to the way out, the door of `talky-chat`
//! (the same road as `talky`'s) and the builder's round.
//!
//! The reserve (16) is room for nesting (a generation placed deeper under its
//! member) and for stages still to come. A segment that no longer fits is not
//! a reason to lift the default: first a seam is sought, and only with the
//! numbers from this file is the default raised.
//!
//! Measured at the receiver, never at the edge: the ttl a capture cell holds
//! when the message arrives, or the ttl of the delivery row in the test
//! colony's own `message_log`. Every case prints one line
//! `gh929 <segment>: start=<ttl> end=<ttl> used=<n>`.
//!
//! Free of a real provider by construction: every `llm` cell talks to a local
//! stub. Guarded like every template-reading test (GH #49).

#[path = "support/gh929_file_road.rs"]
mod file_road;
#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/gh929_member_road.rs"]
mod road;
#[path = "support/gh929_tool_roads.rs"]
mod tool_roads;

use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MESSAGE_DEFAULT_TTL, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use road::{Delivery, GENERATION, Road, Run, Segment, person, plain_brain, segment_before};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);

/// What a segment keeps back from the colony budget (OR-BD-11): nesting and
/// stages still to come.
const RESERVE: u32 = 16;

/// The most routing decisions one segment may spend.
const SEGMENT_MAX: i64 = (MESSAGE_DEFAULT_TTL - RESERVE) as i64;

/// What a restoring seam hands the delivery right behind it: the colony
/// budget less the one routing decision that crossed the seam.
const BEHIND_A_SEAM: i64 = MESSAGE_DEFAULT_TTL as i64 - 1;

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

fn as_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().expect("a JSON object")
}

/// A seam proof prints what was sent and what arrived.
fn say_arrival(case: &str, sent: u32, arrived: u32) {
    eprintln!("gh929 {case}: sent={sent} arrived={arrived}");
}

/// Hold a measured segment to the reserve, with the chain in the message.
fn assert_fits(name: &str, seg: &Segment, run: &Run, closing: &Delivery) {
    eprintln!("{}", seg.say(name));
    assert!(
        seg.used() <= SEGMENT_MAX,
        "{name} spends {} of {} routing decisions (reserve {RESERVE}): {}\nchain:\n{}",
        seg.used(),
        SEGMENT_MAX,
        seg.say(name),
        road::chain_of(&run.log, closing, 80).join("\n")
    );
}

// ─────────────────────────────────────────────── S0: a hive's own out-edge

/// S0 -- the door seam sits on a hive out-edge (`. -> ./x`), so the restore
/// has to happen in the hive transit, not only at a cell's emission. The
/// record disagreed with itself: one wave report said the substrate restores
/// only at cell emissions, the transit path declares that a hive out-edge may
/// restore too. A message with ttl 10 enters hive `/h`; the lane `in_turn`
/// leaves on the restoring edge, the lane `in_other` on a plain one. The
/// numbers are taken at the two capture cells.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s0_a_hive_out_edge_restores_in_transit() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let main = td.path().join("main");
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}}),
    );
    write_json(
        &main.join("h/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": ".", "to": "./x",
             "condition": "has(hop.route) && hop.route == 'in_turn'",
             "modifier": {"restore_ttl": true}},
            {"from": ".", "to": "./y",
             "condition": "has(hop.route) && hop.route == 'in_other'"}
        ]}}}),
    );
    let h = ColonyHandle::new_with_factories_at(&td, Vec::new());
    let (x_tx, mut x_rx) = mpsc::channel::<Message>(8);
    let (y_tx, mut y_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/h/x"), move || CaptureCell::new(x_tx.clone()))
        .await;
    h.spawn(Path::new("/h/y"), move || CaptureCell::new(y_tx.clone()))
        .await;
    bootstrap_from_filesystem(td.path(), &CellFactoryRegistry::new(), &h.runtime())
        .await
        .expect("a hive with two out-edges boots");

    const START: u32 = 10;
    let probe = |route: &str| {
        MessageBuilder::new(Path::new("/h"))
            .hop(as_map(&json!({"route": route})))
            .body(Body::Inline(json!({"messages": []})))
            .ttl(START)
            .build()
    };
    h.send(probe("in_turn")).await;
    h.send(probe("in_other")).await;
    let x = tokio::time::timeout(DEADLINE, x_rx.recv())
        .await
        .ok()
        .flatten()
        .expect("the restoring out-edge delivers");
    let y = tokio::time::timeout(DEADLINE, y_rx.recv())
        .await
        .ok()
        .flatten()
        .expect("the plain out-edge delivers");
    h.shutdown().await;

    say_arrival("S0 hive transit, restoring edge", START, x.ttl);
    say_arrival("S0 hive transit, plain edge", START, y.ttl);
    // Two routing decisions: into the hive, out of it. The plain edge carries
    // what is left; the restoring one arrives with the colony budget less the
    // one decision after the seam.
    assert_eq!(
        y.ttl,
        START - 2,
        "the plain out-edge carries the budget through: arrived with {}",
        y.ttl
    );
    assert_eq!(
        x.ttl,
        MESSAGE_DEFAULT_TTL - 1,
        "a restoring hive out-edge restores in the transit: arrived with {} (plain edge: {})",
        x.ttl,
        y.ttl
    );
}

// ─────────────────────────────────────── S1: the door to the curator entry

fn surface_path(surface: &str) -> String {
    format!("{GENERATION}/{surface}")
}

/// A talky turn that names its audience, from the generation's door to the
/// delivery that crosses the curator entry: the collector, the ambient recall
/// leg through the member's memory and back, and the curator's own
/// preparation of the round.
async fn door_to_curator_entry(deeper: u32) -> (Segment, Run) {
    let surface = "talky";
    let run = road::run(Road {
        scripted: vec![(format!("assistants/scribe/{surface}/brain"), plain_brain())],
        turn: person(surface, "Remember the probe for me.", true, deeper),
        answers: 1,
    })
    .await;
    run.trace(&format!("S1 deeper={deeper}"));
    let door = surface_path(surface);
    let curator = format!("{door}/curator");
    let closing = run
        .nth(0, |d| d.to == curator && d.route == "in_curate")
        .unwrap_or_else(|| {
            panic!(
                "the round reached the curator entry; ttl_expired: {:?}",
                run.expired()
            )
        })
        .clone();
    let seg = segment_before(&run.log, &closing, |d| d.to == door && d.route == "in_turn")
        .unwrap_or_else(|| {
            panic!(
                "the parent chain of the curator entry reaches the door:\n{}",
                road::chain_of(&run.log, &closing, 80).join("\n")
            )
        });
    assert_fits(
        &format!("S1 door -> curator entry (deeper {deeper})"),
        &seg,
        &run,
        &closing,
    );
    (seg, run)
}

/// S1 -- the door decouples the depth: the same turn at the container and two
/// routing decisions deeper arrives behind the generation's door with the same
/// budget, and spends the same on its way to the curator entry. Before the
/// `in_turn` door restored, the turn carried whatever the road above had left
/// (the OR-OS-67 start numbers: 61 at the container, the recall leg alone 26).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s1_the_door_hands_a_turn_the_full_budget_at_any_depth() {
    if !road::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let (top, top_run) = door_to_curator_entry(0).await;
    let (deep, deep_run) = door_to_curator_entry(2).await;
    for (seg, run, depth) in [(&top, &top_run, 0), (&deep, &deep_run, 2)] {
        assert!(
            run.expired().is_empty() && !run.answers.is_empty(),
            "the turn (deeper {depth}) is answered without a ttl death: {:?}",
            run.expired()
        );
        assert_eq!(
            seg.start,
            BEHIND_A_SEAM,
            "the door hands the turn (deeper {depth}) the colony budget: {}",
            seg.say("S1")
        );
    }
    assert_eq!(
        (top.start, top.end),
        (deep.start, deep.end),
        "two routing decisions more above the generation change nothing behind its door"
    );
}

// ──────────────────────────────────────── S2: the curator entry to the brain

/// S2 -- from the curator entry to the delivery into the brain (the round's
/// restoring edge): the curator's intake, policy and handover with their
/// ledger round trips.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s2_the_curator_prepares_a_round_within_its_reserve() {
    if !road::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let surface = "talky";
    let run = road::run(Road {
        scripted: vec![(format!("assistants/scribe/{surface}/brain"), plain_brain())],
        turn: person(surface, "Remember the probe for me.", true, 0),
        answers: 1,
    })
    .await;
    run.trace("S2");
    let door = surface_path(surface);
    let curator = format!("{door}/curator");
    let brain = format!("{door}/brain");
    let closing = run
        .nth(0, |d| d.to == brain)
        .unwrap_or_else(|| {
            panic!(
                "the round reached the brain; ttl_expired: {:?}",
                run.expired()
            )
        })
        .clone();
    let seg = segment_before(&run.log, &closing, |d| {
        d.to == curator && d.route == "in_curate"
    })
    .unwrap_or_else(|| {
        panic!(
            "the parent chain of the brain delivery reaches the curator entry:\n{}",
            road::chain_of(&run.log, &closing, 80).join("\n")
        )
    });
    assert_fits("S2 curator entry -> brain", &seg, &run, &closing);
    assert_eq!(
        seg.start,
        BEHIND_A_SEAM,
        "the curator entry restores: {}",
        seg.say("S2")
    );
    assert!(
        run.expired().is_empty() && !run.answers.is_empty(),
        "the round is answered: {:?}",
        run.expired()
    );
}

// ─────────────────────────────────────────── S3: a history call of the brain

/// S3 -- the brain asks its own history (`history_search`, with the round's
/// audience): from the delivery into the brain over the dispatcher, the
/// curator's `history` with its ledger round trips, the `tool_result` and the
/// collector back to the curator entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s3_a_history_call_returns_within_its_reserve() {
    if !road::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let (seg, run, closing) = tool_roads::s3_history_call().await;
    assert_fits("S3 brain -> history -> curator entry", &seg, &run, &closing);
    assert!(
        run.expired().is_empty(),
        "no ttl death on the history call: {:?}",
        run.expired()
    );
}

// ─────────────────────────────────────── S4: a consult between the two brains

/// S4 -- the talky consults its core (`consult_cogny`): from the consult door
/// into the core's hive to the core's curator entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s4_a_consult_reaches_the_core_within_its_reserve() {
    if !road::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let (seg, run, closing) = tool_roads::s4_consult().await;
    assert_fits(
        "S4 consult door -> core curator entry",
        &seg,
        &run,
        &closing,
    );
    assert_eq!(
        seg.start,
        BEHIND_A_SEAM,
        "the consult door restores: {}",
        seg.say("S4")
    );
    assert!(
        run.expired().is_empty(),
        "no ttl death on the consult: {:?}",
        run.expired()
    );
}

// ─────────────────────────────────────────────── S5: a file turn (OR-BD-12)

/// Print every segment of a file turn and return the most one of them spent;
/// a chain that died of its budget spent more than any segment may.
fn say_file_turn(variant: file_road::B2Variant, raced: usize, turn: &file_road::FileTurn) -> i64 {
    let run = format!("{variant:?}, raced {raced}");
    if turn.died {
        eprintln!(
            "gh929 S5 [{run}]: died of its budget, extracts={} ttl_expired={:?}",
            turn.extracts,
            turn.expired()
        );
        return i64::MAX;
    }
    let mut worst = 0;
    for (name, seg) in &turn.segments {
        eprintln!("{} [{run}]", seg.say(name));
        worst = worst.max(seg.used());
    }
    eprintln!(
        "gh929 S5 [{run}]: worst segment used={worst} arrived={:?} extracts={} ttl_expired={:?}",
        turn.arrived,
        turn.extracts,
        turn.expired()
    );
    worst
}

/// S5 -- a document sent through a connector is stored, extracted and derived
/// in the member's file space and reaches the generation as one turn. Every
/// segment of that road, from the fresh root at the connector to the
/// generation's door, fits its reserve with the file space as shipped: once
/// on the straight road, once with the document's path taken twice under it
/// (`path_taken`, the longest road on which the document is still stored --
/// `./ingest` tries at most three times). The finished turn leaves the space
/// without a restore: the segment from the last try to the generation's
/// `in_turn` door fits on its own (OR-BD-65).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s5_a_file_turn_fits_its_reserve() {
    if !file_road::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let variant = file_road::B2Variant::Shipped;
    for raced in [0, 2] {
        let turn = file_road::s5_file_turn(variant, raced).await;
        let worst = say_file_turn(variant, raced, &turn);
        assert!(
            !turn.died && turn.expired().is_empty() && !turn.arrived.is_empty(),
            "the file turn (raced {raced}) reaches the generation: {:?}",
            turn.expired()
        );
        assert_eq!(
            turn.extracts,
            1 + raced,
            "the document's leg ran once per try (raced {raced})"
        );
        assert!(
            worst <= SEGMENT_MAX,
            "a segment of the file road (raced {raced}) spends {worst} of {SEGMENT_MAX} routing \
             decisions"
        );
    }
}

/// S5, the verdict on the file space's restoring edges (OR-BD-12, OR-BD-65),
/// each on the road it has to carry:
///
/// - without any of them the road from the connector to the generation's door
///   is one segment and does not fit;
/// - with only the space's own door (`. -> ./ingest`) restoring, the straight
///   road fits, a document whose path is taken twice under it does not: every
///   try runs the whole leg again on the budget the door handed out.
///   `./ingest -> ./extract` and `./ingest -> ./write` restore once per try
///   instead, and the script bounds the tries (a door row of a file job).
///
/// Should the file road ever shorten until a variant that does not fit
/// today fits, this lock says so, and the edges are to be read against the
/// seam table again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s5_without_its_restoring_edges_a_file_turn_does_not_fit() {
    if !file_road::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    use file_road::B2Variant::{DoorOnly, NoRestores};
    let mut worst = std::collections::BTreeMap::new();
    for (variant, raced) in [(NoRestores, 0), (DoorOnly, 0), (DoorOnly, 2)] {
        let turn = file_road::s5_file_turn(variant, raced).await;
        worst.insert(
            format!("{variant:?}/{raced}"),
            say_file_turn(variant, raced, &turn),
        );
    }
    let bare = worst["NoRestores/0"];
    assert!(
        bare > SEGMENT_MAX,
        "without its restoring edges the file road fits its reserve ({bare} <= {SEGMENT_MAX}): \
         the edges can fall (OR-BD-12)"
    );
    let door_raced = worst["DoorOnly/2"];
    assert!(
        door_raced > SEGMENT_MAX,
        "with only the space's door restoring, a document raced twice fits its reserve \
         ({door_raced} <= {SEGMENT_MAX}): `./ingest -> ./extract` and `./ingest -> ./write` \
         can give way to the door (OR-BD-12)"
    );
}

// ─────────────────────────────────────────────── S6: a timer strike is a source

fn timer_order(args: Value, call_id: &str, ttl: u32) -> Message {
    MessageBuilder::new(Path::new("/timer"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant", "type": "tool_call",
            "text": args.to_string(), "id": call_id
        }]})))
        .ttl(ttl)
        .build()
}

/// S6 -- an order that arrives at a timer nearly spent (ttl 5) still strikes
/// with the colony budget: the strike leaves the timer's own I/O side as a
/// fresh root (source), it does not inherit the order's rest. The order's
/// acknowledgement, an answer to the order, carries the order's rest.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s6_a_timer_strike_starts_on_the_full_budget() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("timer");
    std::fs::create_dir_all(&cell_dir).expect("mkdir");
    let h = ColonyHandle::new();
    let (tx, mut rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(tx.clone()))
        .await;
    let spawned = Arc::new(TimerCellFactory)
        .spawn_cell(
            Path::new("/timer"),
            json!({}),
            h.runtime().outputs_tx,
            cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("spawn timer");
    h.register_spawned(Path::new("/timer"), spawned).await;
    h.add_edge(Uuid::now_v7(), Path::new("/timer"), Path::new("/sink"))
        .await;

    const SENT: u32 = 5;
    let at = chrono::Utc::now() + chrono::Duration::seconds(2);
    h.send(timer_order(
        json!({"op": "add", "schedule_id": Uuid::now_v7().to_string(),
               "schedule_name": "gh929-strike",
               "at": at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
               "emit_to": "/sink", "emit_body": {"messages": []},
               "emit_headers": {"msg_type": "gh929_strike"}}),
        "call-strike",
        SENT,
    ))
    .await;
    let mut ack = None;
    let mut strike = None;
    while strike.is_none() {
        let m = tokio::time::timeout(DEADLINE, rx.recv())
            .await
            .ok()
            .flatten()
            .expect("the timer answers the order and strikes");
        match m.headers.hop.get("msg_type").and_then(Value::as_str) {
            Some("timer_op_ack") => ack = Some(m),
            Some("gh929_strike") => strike = Some(m),
            other => panic!("unexpected message at the sink: {other:?}"),
        }
    }
    h.shutdown().await;
    let ack = ack.expect("the order was acknowledged before the strike");
    let strike = strike.expect("the strike");
    say_arrival("S6 timer order acknowledgement", SENT, ack.ttl);
    say_arrival("S6 timer strike (source)", SENT, strike.ttl);
    assert!(
        ack.ttl < SENT,
        "the acknowledgement answers the order and carries its rest: {}",
        ack.ttl
    );
    assert_eq!(
        i64::from(strike.ttl),
        BEHIND_A_SEAM,
        "the strike is a fresh root: it arrives with the colony budget less its one decision"
    );
}
