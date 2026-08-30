//! GH #267 (ruling Q14, 2026-08-21 — `/colony/ledger`, aggregates-only): the
//! acceptance definition of the colony's aggregate reader.
//!
//! The argus scripts must stop opening `colony.db` behind the substrate's
//! back (AGENTS § Datenbank-Isolation, GH #160: foreign `colony.db` is taboo,
//! **also for reads**). Q14 sanctioned exactly one replacement: a read that
//! answers **counts and sums**, never rows. This file is that ruling written
//! as a test rather than as an intention.
//!
//! **The contract under test** —
//! `colony_dispatch::handle_read_ledger(db_path: &Path, query: api_dto::LedgerQuery)
//! -> api_dto::ReadLedgerReply`.
//!
//! **Two failure shapes, never confused** (Orchestrator-Ruling 2026-08-23,
//! Meta-Plan § 7):
//!
//! * `unavailable` means exactly one thing — *the read could not happen*.
//!   The reply still carries the resolved `query`, and **no** counts.
//! * A filter that cannot be read is **not** `unavailable`. It is the ordinary
//!   `invalid_query` refusal every `/colony/*` read has carried since GH #341 /
//!   GH #359, taken at **parse time**, before `handle_read_ledger` is called at
//!   all. A refusal carries no `query` — which is how a caller tells the two
//!   apart without reading a string. That property belongs to the endpoint, not
//!   to this helper, and is pinned in Task 11 Step 7 (in this same file, once
//!   the endpoint exists).
//!
//! This file is written **before** the implementation and does not compile
//! until `handle_read_ledger`, `LedgerQuery` and `ReadLedgerReply` exist. That
//! red state is the deliverable of Task 10.

use meclaw_colony::ColonyDb;
use meclaw_colony::api_dto::LedgerQuery;
use meclaw_colony::colony_dispatch::handle_read_ledger;
use meclaw_colony::persist::writer::{ColonyWriteOp, MessageLogRow};

/// Window start (inclusive) used by every test here.
const SINCE: i64 = 1_000;
/// Window end (exclusive) used by every test here.
const UNTIL: i64 = 2_000;
/// The cell whose traffic the prefix/cycle counters ask about.
const TARGET: &str = "/main/talky/brain";

/// Wraps a `hop` object into the header envelope the colony actually persists
/// (`{"context": {…}, "hop": {…}}`) — the aggregate reader extracts out of
/// `$.hop`, so the nesting is load-bearing, not decoration.
fn headers(hop: serde_json::Value) -> String {
    serde_json::json!({ "context": {}, "hop": hop }).to_string()
}

/// One `message_log` row through the real writer op — no hand-rolled SQL, so
/// the fixture cannot drift away from the DDL.
fn msg(n: u32, from: &str, to: &str, created_at: i64, hop: serde_json::Value) -> MessageLogRow {
    MessageLogRow {
        id: format!("019ebb7e-0000-7000-8000-{n:012}"),
        trace_id: "019ebb7e-0000-7000-8000-000000000abc".into(),
        parent_message_id: None,
        correlation_id: None,
        ttl: 10,
        from_path: from.into(),
        to_path: to.into(),
        reply_to: None,
        headers_json: headers(hop),
        body_kind: "inline".into(),
        body_payload: Some(r#"{"messages":[]}"#.into()),
        created_at,
    }
}

/// A fully-specified query. Every field is named here on purpose: the test file
/// is the place where the shape of `LedgerQuery` is fixed for the implementer.
fn query(since: i64, until: i64) -> LedgerQuery {
    LedgerQuery {
        since,
        until,
        path_prefix: None,
        cycle_id: None,
        group_by: None,
        tag: None,
        scan_budget: 50_000,
    }
}

/// Seeds one colony.db with the whole fixture and returns `(tempdir, db_path)`.
/// The `TempDir` must stay alive for the caller's lifetime.
///
/// **In-window messages (`created_at` in `[1000, 2000)`) — six rows:**
///
/// | # | from | to | hop |
/// |---|---|---|---|
/// | 1–3 | `/other/*` | `/other/*` | `model: m/a`, 100 prompt / 20 completion / 40 cached, `cost: 0.002`, `latency_ms: 120` |
/// | 4 | `/other/x` | `/other/y` | `finish_reason: "error"`, `error_code: "rate_limit"`, `latency_ms: 90`, **no model** |
/// | 5 | `/main/talky/ears` | **`/main/talky/brain`** | `cycle_id: "cycle:1"`, `duration_ms: 25` |
/// | 6 | **`/main/talky/brain`** | `/main/talky/out` | `cycle_id: "cycle:1"`, `duration_ms: 5` |
///
/// Rows 5 and 6 are the whole point of the cycle counter: both touch the target
/// and both belong to `cycle:1`, but only row 5 is the update **reaching** the
/// cell. Row 6 is the cell *answering* — a different fact, and not what the
/// argus's `params_update_seen` question asks.
///
/// **Out-of-window messages — two rows** (`created_at` 500 and 5000), each
/// deliberately built so that it would contaminate *every* counter at once if
/// the window were ignored: model `m/a`, `finish_reason: "error"`,
/// `to_path == TARGET`, `cycle_id: "cycle:1"`.
///
/// **Dead letters:** two in-window, one out-of-window.
///
/// **Mutations:** two in-window `MutationLogInsert` + `MutationLogUpdate` pairs
/// (`committed` / `rejected`) and one out-of-window pair.
async fn seed() -> (tempfile::TempDir, std::path::PathBuf) {
    let td = tempfile::TempDir::new().expect("tempdir");
    let db_path = td.path().join("colony.db");
    let db = ColonyDb::open(&db_path).expect("open colony.db");

    let m_a = serde_json::json!({
        "model": "m/a",
        "tokens_prompt": 100,
        "tokens_completion": 20,
        // GH #463: the rest of the usage block the `llm` cell writes.
        "tokens_cached": 40,
        "cost": 0.002,
        "latency_ms": 120
    });
    let contaminant = serde_json::json!({
        "model": "m/a",
        "tokens_prompt": 100,
        "tokens_completion": 20,
        "finish_reason": "error",
        "cycle_id": "cycle:1"
    });

    let rows = vec![
        // 1-3: the per-model aggregate.
        msg(1, "/other/a", "/other/b", 1_100, m_a.clone()),
        msg(2, "/other/a", "/other/c", 1_200, m_a.clone()),
        msg(3, "/other/a", "/other/d", 1_300, m_a.clone()),
        // 4: the one error, and it has no model at all. It carries a typed
        //    error_code and a latency, because a failed call took time too —
        //    that is what `group_by=error_code` groups (GH #463).
        msg(
            4,
            "/other/x",
            "/other/y",
            1_400,
            serde_json::json!({
                "finish_reason": "error",
                "error_code": "rate_limit",
                "latency_ms": 90
            }),
        ),
        // 5: cycle:1 ARRIVING at the target (to_path). `duration_ms` is the
        //    tool-side twin of `latency_ms` — a hop with no model and no cost
        //    that a per-path grouping still has to sum (GH #463).
        msg(
            5,
            "/main/talky/ears",
            TARGET,
            1_500,
            serde_json::json!({ "cycle_id": "cycle:1", "duration_ms": 25 }),
        ),
        // 6: cycle:1 LEAVING the target (from_path) — prefix traffic, but not
        //    an arrival, and therefore not part of the cycle counter.
        msg(
            6,
            TARGET,
            "/main/talky/out",
            1_600,
            serde_json::json!({ "cycle_id": "cycle:1", "duration_ms": 5 }),
        ),
        // Out of window on both sides.
        msg(7, "/main/talky/ears", TARGET, 500, contaminant.clone()),
        msg(8, "/main/talky/ears", TARGET, 5_000, contaminant.clone()),
    ];
    for row in rows {
        db.send_op(ColonyWriteOp::InsertMessageLog(row)).await;
    }

    for (n, created_at) in [(1u32, 1_100i64), (2, 1_200), (3, 500)] {
        db.send_op(ColonyWriteOp::InsertDeadLetter {
            sender_path: "/other/a".into(),
            original_target: "/gone".into(),
            resolved_target: "/gone".into(),
            error_code: "no_route".into(),
            trace_id: "019ebb7e-0000-7000-8000-000000000abc".into(),
            created_at,
            message_json: format!(r#"{{"id":"dl-{n}"}}"#),
        })
        .await;
    }

    // The status string is what the counter groups by; writing it through the
    // real update op keeps the fixture on the same column the audit uses.
    for (n, created_at, status) in [
        (1u32, 1_100i64, "committed"),
        (2, 1_200, "rejected"),
        (3, 500, "committed"),
    ] {
        let id = format!("019ebb7e-0000-7000-8000-0000000{n:05}");
        db.send_op(ColonyWriteOp::MutationLogInsert {
            id: id.clone(),
            scope: "/main".into(),
            payload_json: r#"{"add_nodes":[]}"#.into(),
            created_at,
            ack: None,
        })
        .await;
        db.send_op(ColonyWriteOp::MutationLogUpdate {
            id,
            status: status.into(),
            committed_at: created_at + 1,
            failure_reason: None,
            ack: None,
        })
        .await;
    }

    db.shutdown_async().await;
    (td, db_path)
}

/// Step 2: the ledger's primary job — how many messages, how many of them
/// failed, and what each model cost. Sums, never rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ledger_read_counts_messages_and_sums_tokens_per_model() {
    let (_td, db_path) = seed().await;

    let reply = handle_read_ledger(&db_path, query(SINCE, UNTIL)).await;

    assert!(
        reply.unavailable.is_none(),
        "a readable database is not unavailable"
    );
    let m = reply.messages.as_ref().expect("messages aggregate present");

    assert_eq!(m.total, 6, "the six in-window rows, and only those");
    assert_eq!(
        m.errors, 1,
        "exactly one in-window hop carries finish_reason=error"
    );

    let a = m.by_model.get("m/a").expect("model m/a is grouped");
    assert_eq!(a.calls, 3, "three in-window hops name m/a");
    assert_eq!(a.tokens_prompt, 300, "3 x 100 prompt tokens");
    assert_eq!(a.tokens_completion, 60, "3 x 20 completion tokens");

    // The two out-of-window rows carry model m/a, finish_reason=error and the
    // target path all at once — if the window leaked, at least one of the
    // numbers above would be wrong. Assert the leak explicitly as well, so a
    // future off-by-one in the window bound names itself.
    assert_eq!(
        m.by_model.len(),
        1,
        "no group appears that only out-of-window rows could have created"
    );
    assert!(
        !reply.scan_truncated,
        "the default budget is nowhere near exhausted by six rows"
    );
}

/// Step 3: the other two windows of the same read — the DLQ and the mutation
/// audit — answered as counts beside the message aggregate, in one round trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ledger_answers_dead_letter_and_mutation_counts() {
    let (_td, db_path) = seed().await;

    let reply = handle_read_ledger(&db_path, query(SINCE, UNTIL)).await;

    let dl = reply
        .dead_letters
        .as_ref()
        .expect("dead-letter count present");
    assert_eq!(dl.total, 2, "the third dead letter is out of window");

    let mu = reply.mutations.as_ref().expect("mutation counts present");
    assert_eq!(mu.total, 2, "the third mutation is out of window");
    assert_eq!(
        mu.by_status.get("committed").copied(),
        Some(1),
        "one committed mutation in window"
    );
    assert_eq!(
        mu.by_status.get("rejected").copied(),
        Some(1),
        "one rejected mutation in window"
    );
    assert_eq!(
        mu.by_status.len(),
        2,
        "no status group leaks in from outside the window"
    );
}

/// Step 4a: `path_prefix` is a **second counter**, not a filter over the
/// answer. Asking about one cell must not silently shrink every other number
/// in the reply — an argus that reads `total` after setting a prefix would
/// otherwise be reading a different question than the one it thinks it asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_path_prefix_counts_only_that_cells_traffic() {
    let (_td, db_path) = seed().await;

    let mut q = query(SINCE, UNTIL);
    q.path_prefix = Some(TARGET.to_string());
    let reply = handle_read_ledger(&db_path, q).await;

    let m = reply.messages.as_ref().expect("messages aggregate present");
    assert_eq!(
        m.path_prefix_total, 2,
        "one row arrives at the target, one leaves it"
    );
    assert_eq!(
        m.total, 6,
        "the full window count is untouched by the prefix — a counter, not a filter"
    );
    assert_eq!(m.errors, 1, "the error count is untouched by the prefix");
    assert_eq!(
        m.by_model.get("m/a").expect("m/a still grouped").calls,
        3,
        "the per-model sums are untouched by the prefix"
    );
}

/// Step 4b: two properties in one test, because they only mean something
/// together.
///
/// 1. **The cycle counter is scoped to `to_path` alone.** A reply *from* the
///    target is not the update *reaching* it. Row 6 of the fixture is exactly
///    that decoy: same cycle, same cell, wrong direction.
/// 2. **It is a third counter**, beside `total` and `path_prefix_total` — never
///    a filter over them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cycle_id_counts_what_of_that_cycle_arrived_there() {
    let (_td, db_path) = seed().await;

    let mut q = query(SINCE, UNTIL);
    q.path_prefix = Some(TARGET.to_string());
    q.cycle_id = Some("cycle:1".to_string());
    let hit = handle_read_ledger(&db_path, q).await;

    let m = hit.messages.as_ref().expect("messages aggregate present");
    assert_eq!(
        m.path_prefix_cycle_total, 1,
        "only the arrival counts; the reply leaving the cell in the same cycle does not"
    );
    assert_eq!(m.total, 6, "total is untouched by the cycle scope");
    assert_eq!(
        m.path_prefix_total, 2,
        "the prefix counter is untouched by the cycle scope"
    );
    assert_eq!(
        m.errors, 1,
        "the error count is untouched by the cycle scope"
    );

    // A cycle nobody ran answers zero — and answers it as a number, not as an
    // absent aggregate, an error, or an `unavailable`.
    let mut q2 = query(SINCE, UNTIL);
    q2.path_prefix = Some(TARGET.to_string());
    q2.cycle_id = Some("cycle:2".to_string());
    let miss = handle_read_ledger(&db_path, q2).await;

    assert!(
        miss.unavailable.is_none(),
        "an empty answer is not a failure"
    );
    let m2 = miss.messages.as_ref().expect("messages aggregate present");
    assert_eq!(m2.path_prefix_cycle_total, 0, "no row belongs to cycle:2");
    assert_eq!(m2.total, 6, "every other number is left exactly as it was");
    assert_eq!(m2.path_prefix_total, 2, "including the prefix counter");
    assert_eq!(m2.errors, 1, "and the error count");
}

/// Step 5: the reply echoes the **resolved** query, so a caller can always see
/// which question was actually answered — including the clamps that were
/// applied to it.
///
/// Two opposite treatments are pinned here, and the asymmetry is the point:
///
/// * **`tag` is truncated, never refused.** It is a correlation token: it never
///   touches the data, so shortening it cannot change the answer. An unbounded
///   one is a header-growth hazard (cf. GH #141).
/// * **`cycle_id` is never truncated.** It *filters*, so shortening it would
///   silently change the question. An over-long one is refused instead — but
///   that refusal is a **parse-time** property of the endpoint (`invalid_query`
///   under the `ledger` slot) and is asserted in Task 11 Step 7. What this test
///   pins is the helper side of that contract: a `cycle_id` at the 64-character
///   bound arrives here **verbatim**, so a truncated one can never be mistaken
///   for a legitimate query that got this far.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reply_echoes_the_query_including_the_caller_tag() {
    let (_td, db_path) = seed().await;

    let mut q = query(SINCE, UNTIL);
    q.path_prefix = Some(TARGET.to_string());
    q.group_by = Some("model".to_string());
    q.tag = Some("wait:9f3c".to_string());
    let reply = handle_read_ledger(&db_path, q).await;

    assert_eq!(reply.query.since, SINCE);
    assert_eq!(reply.query.until, UNTIL);
    assert_eq!(reply.query.path_prefix.as_deref(), Some(TARGET));
    assert_eq!(reply.query.group_by.as_deref(), Some("model"));
    assert_eq!(reply.query.cycle_id, None);
    assert_eq!(reply.query.scan_budget, 50_000);
    assert_eq!(
        reply.query.tag.as_deref(),
        Some("wait:9f3c"),
        "the caller's correlation token comes back verbatim"
    );

    // 65 characters: one over the bound.
    let long_tag = "t".repeat(65);
    let mut q2 = query(SINCE, UNTIL);
    q2.tag = Some(long_tag);
    let truncated = handle_read_ledger(&db_path, q2).await;

    let echoed = truncated
        .query
        .tag
        .as_deref()
        .expect("tag echoed, not dropped");
    assert_eq!(
        echoed.chars().count(),
        64,
        "an over-long tag is truncated to 64 characters, not rejected"
    );
    assert!(
        truncated.unavailable.is_none(),
        "a clamped tag is not a failed read"
    );
    assert!(
        truncated.messages.is_some(),
        "a clamped tag still answers the question"
    );

    // The cycle_id at the bound is a legal query and must survive untouched.
    let bounded_cycle = "c".repeat(64);
    let mut q3 = query(SINCE, UNTIL);
    q3.cycle_id = Some(bounded_cycle.clone());
    let echoed_cycle = handle_read_ledger(&db_path, q3).await;
    assert_eq!(
        echoed_cycle.query.cycle_id.as_deref(),
        Some(bounded_cycle.as_str()),
        "a filtering value is echoed verbatim — the helper never truncates a cycle_id"
    );
}

/// Recursively collects every object **key** in a JSON tree.
fn collect_keys(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, child) in map {
                out.push(k.clone());
                collect_keys(child, out);
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|i| collect_keys(i, out)),
        _ => {}
    }
}

/// Recursively collects every string **value** in a JSON tree.
fn collect_strings(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(map) => map.values().for_each(|c| collect_strings(c, out)),
        serde_json::Value::Array(items) => items.iter().for_each(|i| collect_strings(i, out)),
        serde_json::Value::String(s) => out.push(s.clone()),
        _ => {}
    }
}

/// Step 6: the aggregates-only ruling, expressed as a test.
///
/// Not one row, not one envelope, not one header value leaves this endpoint.
/// The only strings in the whole reply that *originated* in a header are the
/// `by_model` group keys — everything else the caller either sent itself or is
/// a number.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_raw_row_and_no_header_content_leaves_the_endpoint() {
    let (_td, db_path) = seed().await;

    let mut q = query(SINCE, UNTIL);
    q.path_prefix = Some(TARGET.to_string());
    q.cycle_id = Some("cycle:1".to_string());
    q.group_by = Some("model".to_string());
    q.tag = Some("wait:9f3c".to_string());
    let reply = handle_read_ledger(&db_path, q).await;

    let json = serde_json::to_value(&reply).expect("the reply serialises");

    let mut keys = Vec::new();
    collect_keys(&json, &mut keys);
    for forbidden in [
        "headers",
        "body_payload",
        "message_json",
        "payload_json",
        "id",
        "trace_id",
        "to_path",
        "from_path",
    ] {
        assert!(
            !keys.iter().any(|k| k == forbidden),
            "key `{forbidden}` must not occur anywhere in the ledger reply — \
             the ledger answers aggregates, never rows (GH #267, ruling Q14)"
        );
    }

    // The caller's own strings are allowed back out — it sent them.
    let caller_strings = ["/main/talky/brain", "cycle:1", "model", "wait:9f3c"];
    let model_keys: Vec<String> = reply
        .messages
        .as_ref()
        .expect("messages aggregate present")
        .by_model
        .keys()
        .cloned()
        .collect();
    assert_eq!(model_keys, vec!["m/a".to_string()], "the one grouped model");

    let mut strings = Vec::new();
    collect_strings(&json, &mut strings);
    for s in &strings {
        assert!(
            caller_strings.contains(&s.as_str()) || model_keys.contains(s),
            "string `{s}` leaves the endpoint although the caller never sent it \
             and it is not a by_model group key — header content is escaping"
        );
    }
}

/// Step 7a: a bounded read that hit its bound says so. An aggregate that
/// silently stopped counting is worse than no aggregate: the argus would read
/// a smaller number as good news.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_exhausted_scan_budget_is_reported_rather_than_hidden() {
    let (_td, db_path) = seed().await;

    let mut q = query(SINCE, UNTIL);
    q.scan_budget = 2;
    let reply = handle_read_ledger(&db_path, q).await;

    assert!(
        reply.scan_truncated,
        "a budget of 2 over six in-window rows is exhausted, and the reply must say so"
    );
    assert!(
        reply.unavailable.is_none(),
        "an exhausted budget is a partial answer, not a failed read"
    );
    assert!(
        reply.messages.is_some(),
        "the partial counts are still delivered"
    );

    let generous = handle_read_ledger(&db_path, query(SINCE, UNTIL)).await;
    assert!(
        !generous.scan_truncated,
        "the default budget is not exhausted by six rows"
    );
}

/// Step 7b: the one and only meaning of `unavailable` — **the read could not
/// happen**.
///
/// This is the fail-closed signal the argus's verdict depends on: it must
/// never be able to mistake "I could not look" for "I looked and found
/// nothing". So the reply carries no counts at all — not zeroes — while still
/// echoing the resolved query, which is what distinguishes it from the
/// `invalid_query` refusal (that one carries no query; Task 11 Step 7).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ledger_read_that_cannot_look_says_so() {
    let td = tempfile::TempDir::new().expect("tempdir");
    // Never created — a read-only open of a nonexistent file fails rather than
    // conjuring an empty database, which is exactly the property under test.
    let missing = td.path().join("does-not-exist.db");

    let reply = handle_read_ledger(&missing, query(SINCE, UNTIL)).await;

    let why = reply
        .unavailable
        .as_deref()
        .expect("a read that could not happen reports why");
    assert!(!why.is_empty(), "the reason string is not empty");

    assert!(
        reply.messages.is_none(),
        "no message counts — an unavailable read reports nothing, not zero"
    );
    assert!(reply.dead_letters.is_none(), "no dead-letter count either");
    assert!(reply.mutations.is_none(), "no mutation counts either");

    // Still a fully resolved query echo: this is the structural difference
    // between `unavailable` and an `invalid_query` refusal.
    assert_eq!(reply.query.since, SINCE);
    assert_eq!(reply.query.until, UNTIL);

    let json = serde_json::to_value(&reply).expect("the reply serialises");
    for absent in ["messages", "dead_letters", "mutations"] {
        assert!(
            json.get(absent).is_none(),
            "`{absent}` must be skipped entirely, so no consumer can read a zero out of it"
        );
    }
    assert!(
        json.get("query").is_some(),
        "the query echo is what tells an unavailable answer apart from a refusal"
    );
}

// ---------------------------------------------------------------------------
// Task 11 Step 7 — the ENDPOINT, not the helper.
//
// Everything above pins `handle_read_ledger`. What follows pins the door in
// front of it: a cell emission reaches `/colony/ledger` and is answered at the
// sender, and a filter that cannot be read is refused at PARSE time — before
// `handle_read_ledger` is called at all, which is precisely why a refusal
// carries no `query` echo.
// ---------------------------------------------------------------------------

/// Boots a colony with a `CaptureCell` sink at `/ledgersink` and returns the
/// handle plus the sink's receiver.
async fn boot_ledger_probe() -> (
    meclaw_testing::ColonyHandle,
    tokio::sync::mpsc::Receiver<meclaw_core::Message>,
) {
    let colony = meclaw_testing::ColonyHandle::new();
    let (tx, rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(64);
    colony
        .spawn(meclaw_core::Path::new("/ledgersink"), move || {
            meclaw_testing::topologies::phase_3a::CaptureCell::new(tx.clone())
        })
        .await;
    (colony, rx)
}

/// Sends one `/colony/ledger` read carrying `query` and returns the `ledger`
/// slot of the reply that arrives at `/ledgersink`.
async fn ask_ledger(
    colony: &meclaw_testing::ColonyHandle,
    rx: &mut tokio::sync::mpsc::Receiver<meclaw_core::Message>,
    query: meclaw_core::serde_json::Value,
) -> meclaw_core::serde_json::Value {
    let msg = meclaw_core::MessageBuilder::new(meclaw_core::Path::new("/colony/ledger"))
        .body(meclaw_core::Body::Inline(
            meclaw_core::serde_json::json!({ "query": query, "messages": [] }),
        ))
        .reply_to(meclaw_core::Path::new("/ledgersink"))
        .build();
    colony.send(msg).await;

    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
        .expect("/colony/ledger must reply to /ledgersink");
    let meclaw_core::Body::Inline(body) = &reply.body else {
        panic!("inline ledger reply expected");
    };
    assert!(
        body.get("ledger").is_some(),
        "the answer lives under the endpoint's own top-level slot, found {body}"
    );
    body["ledger"].clone()
}

/// Step 7a: the wiring proof. A cell emission to `/colony/ledger` is dispatched,
/// answered under the `ledger` slot and cascaded back to the sender's
/// `reply_to` — with nothing landing in the dead-letter queue.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_emission_to_the_ledger_endpoint_is_answered_at_the_sender() {
    let (colony, mut rx) = boot_ledger_probe().await;

    let slot = ask_ledger(
        &colony,
        &mut rx,
        meclaw_core::serde_json::json!({"since": 0}),
    )
    .await;

    assert!(
        slot.get("query").is_some(),
        "an answer echoes the resolved query — that is what tells it from a refusal"
    );
    assert_eq!(
        slot["query"]["since"].as_i64(),
        Some(0),
        "the caller's window start survives the trip through the endpoint"
    );
    assert!(
        slot.get("messages").is_some(),
        "a readable colony.db answers counts, found {slot}"
    );
    assert!(
        slot.get("status").is_none(),
        "an answer is not discriminated by `status` — that shape belongs to the refusal"
    );

    let dlq = colony.drain_dead_letters().await;
    assert!(
        dlq.is_empty(),
        "a known /colony endpoint must not dead-letter, found {dlq:?}"
    );

    colony.shutdown().await;
}

/// Step 7b: the refusal is a **parse-time** property of the endpoint. Three
/// unreadable filters, one shape — the `invalid_query` refusal every
/// `/colony/*` read has carried since GH #341 / GH #359. No new error code, no
/// counts, and no `query` echo: that absence is how a caller tells a refusal
/// from an `unavailable` answer without reading a string.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreadable_ledger_filter_is_refused_like_every_other_colony_read() {
    let (colony, mut rx) = boot_ledger_probe().await;

    let unreadable = [
        // A grouping the ledger does not have. Answering the ungrouped holdings
        // would be the silent default K-1 refused.
        meclaw_core::serde_json::json!({"group_by": "cell_type"}),
        // 65 characters: one over the bound. A filtering value is never
        // truncated, because a shortened one asks a different question.
        meclaw_core::serde_json::json!({"cycle_id": "c".repeat(65)}),
        // A window bound that is not a number.
        meclaw_core::serde_json::json!({"since": "gestern"}),
    ];

    for query in unreadable {
        let slot = ask_ledger(&colony, &mut rx, query.clone()).await;

        assert_eq!(
            slot["status"].as_str(),
            Some("error"),
            "refused filter {query} must answer the error shape, found {slot}"
        );
        assert_eq!(
            slot["error_code"].as_str(),
            Some("invalid_query"),
            "the ledger is the fifth read under the ONE documented code, not a new one"
        );
        assert!(
            slot["details"].as_str().is_some_and(|d| !d.is_empty()),
            "a refusal names what it could not read"
        );

        assert!(
            slot.get("query").is_none(),
            "a refusal carries no query echo — that is the structural difference \
             to an `unavailable` answer, found {slot}"
        );
        assert!(
            slot.as_array().is_none(),
            "a refusal is never a result list"
        );
        for count in ["messages", "dead_letters", "mutations", "scan_truncated"] {
            assert!(
                slot.get(count).is_none(),
                "`{count}` must be absent from a refusal — the read never happened"
            );
        }
    }

    let dlq = colony.drain_dead_letters().await;
    assert!(
        dlq.is_empty(),
        "a refusal is an answer, not a dead letter, found {dlq:?}"
    );

    colony.shutdown().await;
}

/// Step 7c: an EMPTY window is refused, not answered with zeroes.
///
/// The counts of an inverted window are the one place where *"we did not
/// look"* and *"we looked and saw nothing"* read alike: a caller that swapped
/// its bounds by accident would take the second reading and conclude its colony
/// was quiet. So the endpoint refuses under the same `invalid_query` the other
/// unreadable filters carry — no new error code, and no counts to misread.
///
/// The test runs on the **resolved** window, which is the only place it can
/// run: both bounds have defaults (`until` = now, `since` = now - 3600), so
/// `until` alone is not enough to tell an empty window from a sensible one.
/// Both cases below are empty after resolution — the inverted pair, and the
/// degenerate one where the two bounds coincide.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_ledger_window_is_refused_rather_than_answered_with_zeroes() {
    let (colony, mut rx) = boot_ledger_probe().await;

    let empty = [
        // Inverted: the caller swapped `since` and `until`.
        meclaw_core::serde_json::json!({"since": 2_000, "until": 1_000}),
        // Degenerate: a window of zero width holds nothing by construction.
        meclaw_core::serde_json::json!({"since": 1_000, "until": 1_000}),
        // Only `until` given, and it lands before the default `since`
        // (`now - 3600`) — the resolved window is what is tested, not the
        // caller's field count.
        meclaw_core::serde_json::json!({"until": 1_000}),
    ];

    for query in empty {
        let slot = ask_ledger(&colony, &mut rx, query.clone()).await;

        assert_eq!(
            slot["status"].as_str(),
            Some("error"),
            "empty window {query} must answer the error shape, found {slot}"
        );
        assert_eq!(
            slot["error_code"].as_str(),
            Some("invalid_query"),
            "an empty window refuses under the ONE documented read code"
        );
        assert!(
            slot["details"]
                .as_str()
                .is_some_and(|d| d.contains("empty window: until <= since")),
            "the refusal names the window it could not read, found {slot}"
        );
        for count in ["messages", "dead_letters", "mutations", "scan_truncated"] {
            assert!(
                slot.get(count).is_none(),
                "`{count}` must be absent — a zero count is exactly the reading \
                 this refusal exists to prevent"
            );
        }
    }

    // The neighbouring case that must NOT be refused: a caller that sends
    // neither bound gets `now - 3600 … now`, which is a real window.
    let slot = ask_ledger(&colony, &mut rx, meclaw_core::serde_json::json!({})).await;
    assert!(
        slot.get("query").is_some() && slot.get("status").is_none(),
        "the defaults resolve to a non-empty window and must still answer, found {slot}"
    );

    let dlq = colony.drain_dead_letters().await;
    assert!(
        dlq.is_empty(),
        "a refusal is an answer, not a dead letter, found {dlq:?}"
    );

    colony.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// GH #463 — the usage block travels whole, and counts group by path and error
// code.
//
// Before #463 the `llm` cell wrote four figures into the hop header and the
// ledger summed two of them. The cache-read count and the provider's own cost
// were parsed off the wire and dropped, `latency_ms` lived only in the body —
// which this endpoint never reads — and `group_by` was accepted, echoed and
// never looked at. A watcher asking "latency per cell" or "cost per model" had
// the numbers in the log and no way to sum them.
// ───────────────────────────────────────────────────────────────────────────

/// The whole usage block is summed, not just the two token counts.
///
/// `latency_samples` is the load-bearing half: three of the six in-window hops
/// carry a latency, so a mean computed over `calls` would answer a different,
/// smaller number and no reader could tell.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_figure_of_the_usage_block_is_summed_per_model() {
    let (_td, db_path) = seed().await;

    let reply = handle_read_ledger(&db_path, query(SINCE, UNTIL)).await;
    let m = reply.messages.as_ref().expect("messages aggregate present");
    let a = m.by_model.get("m/a").expect("model m/a is grouped");

    assert_eq!(a.calls, 3);
    assert_eq!(a.tokens_prompt, 300, "3 x 100 prompt tokens");
    assert_eq!(a.tokens_completion, 60, "3 x 20 completion tokens");
    assert_eq!(a.tokens_cached, 120, "3 x 40 cache-read tokens");
    assert!(
        (a.cost - 0.006).abs() < 1e-9,
        "3 x 0.002 of the provider's OWN cost figure, found {}",
        a.cost
    );
    assert_eq!(a.latency_ms, 360, "3 x 120 ms of provider wall time");
    assert_eq!(
        a.latency_samples, 3,
        "the divisor for a mean is the number of hops that CARRIED a latency, \
         never the call count — most hops in a window carry none"
    );
    assert_eq!(a.latency_ms / a.latency_samples as i64, 120, "mean latency");

    // The m/a hops are `llm` hops: they carry no tool-side duration at all, and
    // the sample count is what says so rather than a zero that reads as "fast".
    assert_eq!(a.duration_ms, 0);
    assert_eq!(a.duration_samples, 0, "no m/a hop carried a duration_ms");
}

/// `group_by=path` groups the same sums by the **receiving** path, and picks up
/// the `duration_ms` that tool cells write and no model ever carries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ledger_read_groups_by_receiving_path_when_asked() {
    let (_td, db_path) = seed().await;

    let mut q = query(SINCE, UNTIL);
    q.group_by = Some("path".to_string());
    let reply = handle_read_ledger(&db_path, q).await;
    let m = reply.messages.as_ref().expect("messages aggregate present");

    assert_eq!(
        m.by_path.len(),
        6,
        "six in-window rows, six distinct to_paths, found {:?}",
        m.by_path.keys().collect::<Vec<_>>()
    );

    // Row 5 ARRIVES at the target; row 6 LEAVES it. The grouping keys on
    // `to_path`, so the target's group is the arrival — the same direction the
    // cycle counter means.
    let arrived = m.by_path.get(TARGET).expect("the target is a group");
    assert_eq!(arrived.calls, 1, "one row arrived at the target");
    assert_eq!(arrived.duration_ms, 25);
    assert_eq!(arrived.duration_samples, 1);
    let left = m
        .by_path
        .get("/main/talky/out")
        .expect("the target's answer is grouped under ITS receiver");
    assert_eq!(left.duration_ms, 5, "row 6 counts at the cell it reached");

    assert_eq!(
        m.by_path
            .get("/other/b")
            .expect("an m/a receiver")
            .latency_ms,
        120,
        "an llm hop's latency is summed under the path it reached"
    );

    // A second map beside `by_model`, never a filter over it: asking for one
    // grouping must not shrink the answer the caller did not ask about.
    assert_eq!(m.total, 6);
    assert_eq!(m.by_model.get("m/a").expect("m/a still grouped").calls, 3);
    assert!(
        m.by_error_code.is_empty(),
        "only the requested second grouping is computed"
    );
}

/// `group_by=error_code` groups by the typed failure code. A hop that did not
/// fail carries none and creates no group — an invented `null` key would be a
/// bucket nobody can read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ledger_read_groups_by_error_code_when_asked() {
    let (_td, db_path) = seed().await;

    let mut q = query(SINCE, UNTIL);
    q.group_by = Some("error_code".to_string());
    let reply = handle_read_ledger(&db_path, q).await;
    let m = reply.messages.as_ref().expect("messages aggregate present");

    assert_eq!(
        m.by_error_code.len(),
        1,
        "one in-window hop failed, found {:?}",
        m.by_error_code.keys().collect::<Vec<_>>()
    );
    let rl = m
        .by_error_code
        .get("rate_limit")
        .expect("the typed code is the group key");
    assert_eq!(rl.calls, 1);
    assert_eq!(
        rl.latency_ms, 90,
        "a failed call took time too, and that time is summable"
    );
    assert_eq!(rl.latency_samples, 1);
    assert_eq!(rl.tokens_prompt, 0, "a call that failed reports no tokens");

    assert_eq!(m.errors, 1, "the window-wide error count is unchanged");
    assert!(
        m.by_path.is_empty(),
        "only the requested grouping is computed"
    );
}

/// The compatibility half of #463: a caller that asks for no grouping — the
/// only caller shape that existed before — gets the reply it has always got,
/// with the two new maps not merely empty but **absent** from the JSON.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ungrouped_read_answers_the_shape_it_always_answered() {
    let (_td, db_path) = seed().await;

    let reply = handle_read_ledger(&db_path, query(SINCE, UNTIL)).await;
    let json = serde_json::to_value(&reply).expect("the reply serialises");
    let messages = json["messages"].as_object().expect("messages object");

    assert!(
        messages.contains_key("by_model"),
        "by_model is answered whether or not a grouping was asked for"
    );
    for added in ["by_path", "by_error_code"] {
        assert!(
            !messages.contains_key(added),
            "`{added}` must not appear in an ungrouped reply — an empty map \
             would be a new field every existing reader has to learn to ignore"
        );
    }
}

/// The aggregates-only ruling under a grouping the caller chose (GH #267,
/// ruling Q14, widened by #463): a group KEY is the dimension the caller asked
/// to be grouped along. Nothing else of the row travels with it — no envelope,
/// no header value, no body.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grouped_read_reveals_its_group_keys_and_nothing_else() {
    let (_td, db_path) = seed().await;

    let mut q = query(SINCE, UNTIL);
    q.group_by = Some("path".to_string());
    let reply = handle_read_ledger(&db_path, q).await;
    let json = serde_json::to_value(&reply).expect("the reply serialises");

    let mut keys = Vec::new();
    collect_keys(&json, &mut keys);
    for forbidden in [
        "headers",
        "body_payload",
        "message_json",
        "payload_json",
        "id",
        "trace_id",
        "to_path",
        "from_path",
    ] {
        assert!(
            !keys.iter().any(|k| k == forbidden),
            "key `{forbidden}` must not occur anywhere in a grouped ledger reply"
        );
    }

    let m = reply.messages.as_ref().expect("messages aggregate present");
    let group_keys: Vec<String> = m.by_model.keys().chain(m.by_path.keys()).cloned().collect();
    let caller_strings = ["path"];
    let mut strings = Vec::new();
    collect_strings(&json, &mut strings);
    for s in &strings {
        assert!(
            caller_strings.contains(&s.as_str()) || group_keys.contains(s),
            "string `{s}` leaves the endpoint although it is neither the \
             caller's own word nor a group key of the grouping it asked for"
        );
    }
}

/// The grouping vocabulary is exactly three words, and the list is closed.
///
/// A caller word must never reach SQL: it selects one of three fixed
/// expressions or it is refused under the one documented `invalid_query`.
#[test]
fn the_grouping_vocabulary_is_three_words_and_closed() {
    use meclaw_colony::colony_dispatch::parse_read_query_ledger_filters;

    for accepted in ["model", "path", "error_code"] {
        let body = serde_json::json!({
            "query": {"since": SINCE, "until": UNTIL, "group_by": accepted}
        });
        let q = parse_read_query_ledger_filters(&body)
            .unwrap_or_else(|e| panic!("`{accepted}` must be accepted: {}", e.details));
        assert_eq!(
            q.group_by.as_deref(),
            Some(accepted),
            "the accepted grouping is echoed verbatim"
        );
    }

    for refused in ["cell_type", "to_path", "PATH", "", "model, path"] {
        let body = serde_json::json!({
            "query": {"since": SINCE, "until": UNTIL, "group_by": refused}
        });
        let err = parse_read_query_ledger_filters(&body)
            .expect_err("an unknown grouping is refused, never silently ignored");
        assert_eq!(err.key, "query.group_by");
        assert!(
            err.details.contains("error_code"),
            "the refusal names the vocabulary it has, found {}",
            err.details
        );
    }
}
