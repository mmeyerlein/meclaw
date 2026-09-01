//! GH #556 — the submitter moved into the front door, and a submission still
//! runs the whole road.
//!
//! THE RULING
//! ==========
//! `submit` stopped being a hive of the OS shell and became an occupant of
//! `operator`: `/os/operator/submit`, gate and store. One front, one place a
//! submission lives, and a road that is readable off the graph — through
//! `meclaw-os@1.6.1` a submission crossed `./operator` and then `./submit`, two
//! shell stations, for what is one job.
//!
//! ADR-0015's guardrail survives the move rather than being broken by it, and
//! the amendment of 2026-08-31 says which half of it was ever load-bearing: the
//! protection is the **edge that is missing between the drafter and the
//! submitter**, and a missing edge is missing at every address. That claim is
//! held by `gh302`'s builder assertions and by the ADR anchor; this file holds
//! the other half — that the road the ruling rearranged still runs, hop by hop,
//! and that each hop arrives where the new topology says it does.
//!
//! WHAT THIS FILE PROVES, POSITIVELY
//! =================================
//! Every leg is read off a cell that actually received the message, or off a
//! row the mutation door actually wrote. Never off an empty dead-letter queue,
//! which cannot tell "arrived somewhere else" from "arrived nowhere".
//!
//! 1. **The shape.** The shell holds no `submit` occupant; the front door holds
//!    one, and it is a `ref` onto the same template the shell used to name. The
//!    cell that lends a submission its sender is `intake` — a ref inside a
//!    template is named after the template it references
//!    (`docs/development-rules.md` § 8a, R1), so the name `submit` belongs to
//!    the hive that moved in.
//! 2. **A submission gets in.** `in_submit` posted at `/os` reaches
//!    `/os/operator/intake`.
//! 3. **`apply` crosses no rim at all any more.** What `intake` raises reaches
//!    `/os/operator/submit/gate` on `in_apply`, over an edge of the front door's
//!    own graph. Through `operator@1.0.0` that was two shell edges.
//! 4. **The gate's question leaves the front door and reaches the broker**, on
//!    `in_request`, carrying `context.requester == /os/operator/submit` — the
//!    OCCUPANT whose reach the rule is about, not the hive around it (R-AC-1).
//! 5. **The verdict comes back in** and reaches the gate on `in_verdict`.
//! 6. **The privileged lane still reaches the mutation door.** `mutate` raised
//!    at the gate travels four hive transits and the colony writes a
//!    `mutation_log` row — the positive receipt that the lane arrived. Whether
//!    that row commits is the manifest's business and not this file's.
//! 7. **A submission is answered once.** An ordinary committed receipt reaches
//!    `intake` and stays inside the hive; a refusal and a class registration
//!    also leave, on the front door's own `sub_receipt` lane, and reach the
//!    baumeister. One lane per fact, so a caller subscribed to `receipt` cannot
//!    be handed two answers to one submission.
//!
//! WHAT IT IS NOT
//! ==============
//! It runs no model and no real cell: every cell of the grown shell is a TAP,
//! the device `gh465` and `gh469` use. The question is which ADDRESS a message
//! reaches, and a cell that computed something would only add a variable. It
//! therefore says nothing about what the broker decides — that is
//! `gh514_the_shell_is_a_scope_the_policy_can_reach.rs` — nor about what the
//! gate derives from a diff, which is `gh446` and `gh504`.
//!
//! Guarded like every template-reading test (GH #49): a tree that did not ship
//! the example or the library is SKIPPED, never judged.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, RespawnFn, SpawnedCellKind, WakeFn,
    bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellEmission, Headers, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// The seed under test, and the shell it declares.
const SEED: &str = "examples/meclaw-os/seed-ref";
const SHELL: &str = "meclaw-os";
/// Where the shell stands once the marker is fulfilled.
const OS: &str = "/os";
/// The front door, and the two occupants this ruling is about.
const FRONT_DOOR: &str = "/os/operator";
/// The cell that turns a request into a message with a sender. Called `submit`
/// through `operator@1.0.0`; the name moved to the hive that came to live here.
const INTAKE: &str = "/os/operator/intake";
/// The submitter's gate — one hive deeper than it stood through `meclaw-os@1.6.1`.
const GATE: &str = "/os/operator/submit/gate";
/// The identity the shell's own edge promotes when the gate asks (R-AC-1). It
/// names the OCCUPANT: the reach the rule is about is the submitter's, and
/// `/os/operator` would grant an export cell and a lifecycle composer the same.
const ASKER: &str = "/os/operator/submit";
/// The broker's first cell — where `in_request` has to arrive.
const POLICY: &str = "/os/access/policy";
/// The baumeister's two doors for a submitter's receipt: the repair lane of
/// GH #425 and the corpus nudge of GH #504.
const BUILDER_WEAVE: &str = "/os/builder/weave";
const BUILDER_LIBRARIAN: &str = "/os/builder/builder-librarian/catalogue";

/// A digest shape, not a digest: what is measured is that the pin survives the
/// hops, and the bytes it was drawn over are the submitter's business.
const DIGEST: &str = "1111111111111111111111111111111111111111111111111111111111111111";

/// Generous, per the 30s failure-marker convention: what is timed here is a
/// handful of routing hops, and a tight bound would only measure cargo load.
const ARRIVAL: Duration = Duration::from_secs(30);

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Did the seed and the library travel with this tree (GH #49)?
fn shipped() -> bool {
    repo(&format!("{SEED}/main/os/config.json")).is_file()
        && repo(&format!("templates/{SHELL}/template.json")).is_file()
        && repo("templates/operator/config.json").is_file()
        && repo("templates/submit/template.json").is_file()
}

// ──────────────────────────────────────────────────────────────────────────────
// 1 — the shape, off the shipped files
// ──────────────────────────────────────────────────────────────────────────────

/// The shell lost an occupant and the front door gained one — and the one it
/// gained is a `ref` onto the very template the shell used to name.
#[test]
fn the_submitter_is_an_occupant_of_the_front_door_and_not_of_the_shell() {
    if !shipped() {
        eprintln!("skipped: the library did not ship (GH #49)");
        return;
    }
    assert!(
        !repo(&format!("templates/{SHELL}/submit")).exists(),
        "the shell still ships a `submit` occupant — the whole of #556 is that it does not"
    );

    let marker = repo("templates/operator/submit/config.json");
    assert!(
        marker.is_file(),
        "the front door ships no `submit` occupant; the submitter has no home"
    );
    let cell = read_json(&marker);
    assert_eq!(
        cell["cell"]["type"], "ref",
        "`operator/submit` must be a REF onto the shipped template — a copy would be a \
         second submitter, and a second submitter is a second audit trail"
    );
    let want = read_json(&repo("templates/submit/template.json"))["version"]
        .as_str()
        .expect("the submit template names a version")
        .to_string();
    assert_eq!(
        cell["cell"]["template"],
        Value::String(format!("submit@{want}")),
        "the ref must pin the exact version the library ships — a bare name adopts \
         whatever is newest on disk"
    );

    // The cell that lends the sender kept its job and changed its name, because
    // the name it had is the address the ref now owns (§ 8a, R1).
    let intake = read_json(&repo("templates/operator/intake/config.json"));
    assert_eq!(intake["cell"]["type"], "code");
    assert!(
        intake["params"]["script_inline"].is_string(),
        "`operator/intake` is the code cell that draws the digest and gives a \
         submission a sender"
    );
}

/// The two edges that make the road out of the colony, read off the two files
/// that draw them. Neither can be added by a mutation on any scope.
#[test]
fn the_privileged_lane_leaves_the_front_door_and_then_the_colony() {
    if !shipped() {
        eprintln!("skipped: the library did not ship (GH #49)");
        return;
    }
    let operator = read_json(&repo("templates/operator/config.json"));
    let has = |cfg: &Value, from: &str, to: &str, needle: &str| -> bool {
        cfg["params"]["graph"]["edges"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|e| {
                e["from"] == from
                    && e["to"] == to
                    && e["condition"].as_str().unwrap_or_default().contains(needle)
            })
    };
    assert!(
        has(&operator, "./submit", ".", "'mutate'"),
        "the front door must let the submitter's `mutate` out — the submitter lives \
         inside it now, and a lane with no exit is a manifest that dies at a rim"
    );
    let shell = read_json(&repo(&format!("templates/{SHELL}/config.json")));
    assert!(
        has(&shell, "./operator", ".", "'mutate'"),
        "the shell must carry `mutate` from the front door to its own rim"
    );
    let root = read_json(&repo(&format!("{SEED}/main/config.json")));
    assert!(
        has(&root, "./os", "/colony/mutations", "'mutate'"),
        "the birth topology must draw the one edge no mutation can draw — \
         `/colony/mutations` is not an endpoint a mutation may name at any scope"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// the tap factory — gh465's inert cell with a channel on its mailbox
// ──────────────────────────────────────────────────────────────────────────────

/// One `(path, message)` pair per message a cell of the grown shell received.
type Arrival = (String, Message);

/// Every cell of the tree, replaced by something that only says "I got this".
struct TapCellFactory {
    tx: mpsc::Sender<Arrival>,
}

impl CellFactory for TapCellFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn is_lazy(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        _outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let capacity = mailbox_capacity.max(1);
        let (sender, receiver) = mpsc::channel::<Message>(capacity);
        let at = path.as_str().to_string();

        let tap = self.tx.clone();
        let wake_at = at.clone();
        let wake: WakeFn = Box::new(move |mut rx: mpsc::Receiver<Message>| {
            let tap = tap.clone();
            let at = wake_at.clone();
            tokio::spawn(async move {
                while let Some(m) = rx.recv().await {
                    let _ = tap.send((at.clone(), m)).await;
                }
            });
            let (stop_tx, _stop_rx) = oneshot::channel::<()>();
            let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
            (stop_tx, death_ack_rx)
        });

        let respawn_tap = self.tx.clone();
        let respawn: RespawnFn = Box::new(move || {
            let (tx, mut rx) = mpsc::channel::<Message>(capacity);
            let (peace_tx, peace_rx) = oneshot::channel::<()>();
            let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
            let tap = respawn_tap.clone();
            let at = at.clone();
            let join = tokio::spawn(async move {
                let _peace_keep = peace_tx;
                while let Some(m) = rx.recv().await {
                    let _ = tap.send((at.clone(), m)).await;
                }
            });
            (tx, join, peace_rx, backstop_rx)
        });

        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        Ok(SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            stop_tx,
            death_ack_rx,
            respawn,
        })
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// the colony under test — the shipped seed, grown for real
// ──────────────────────────────────────────────────────────────────────────────

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn cell_types_in(root: &std::path::Path) -> BTreeSet<String> {
    fn walk(dir: &std::path::Path, out: &mut BTreeSet<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.file_name().and_then(|n| n.to_str()) == Some("config.json")
                && let Ok(raw) = std::fs::read_to_string(&p)
                && let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw)
                && let Some(t) = v["cell"]["type"].as_str()
            {
                out.insert(t.to_string());
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(root, &mut out);
    out.remove("hive");
    out.remove("ref");
    out
}

/// A `.env` that holds every key the shell declares as required. The keys are
/// NAMES; no value of any of them travels in this tree.
fn complete_env() -> String {
    let declared = read_json(&repo(&format!("templates/{SHELL}/template.json")));
    let env = declared["requires"]["env"]
        .as_object()
        .expect("the shell declares `requires.env`");
    let mut out = String::new();
    for (key, decl) in env {
        if decl["required"].as_bool().unwrap_or(true) {
            out.push_str(&format!("{key}=placeholder-for-a-test\n"));
        }
    }
    out
}

/// The grown colony, plus the tap every cell in it writes to.
struct Grown {
    handle: ColonyHandle,
    arrivals: mpsc::Receiver<Arrival>,
    root: std::path::PathBuf,
    _td: tempfile::TempDir,
}

async fn grow() -> Grown {
    let td = tempfile::TempDir::new().unwrap();
    copy_tree(&repo(SEED), td.path());
    copy_tree(&repo("templates"), &td.path().join("templates"));
    std::fs::write(td.path().join(".env"), complete_env()).unwrap();

    let (tx, arrivals) = mpsc::channel::<Arrival>(4096);
    let factories: Vec<(String, Arc<dyn CellFactory>)> =
        cell_types_in(&td.path().join("templates"))
            .into_iter()
            .map(|t| {
                (
                    t,
                    Arc::new(TapCellFactory { tx: tx.clone() }) as Arc<dyn CellFactory>,
                )
            })
            .collect();

    let handle = ColonyHandle::new_with_factories_at(&td, factories.clone());
    let (ack_tx, ack_rx) = oneshot::channel();
    handle
        .inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx.await.expect("rescan ack").expect("rescan aborted");

    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &handle.runtime())
        .await
        .expect("stage one must grow the shell");

    let root = td.path().to_path_buf();
    Grown {
        handle,
        arrivals,
        root,
        _td: td,
    }
}

/// Wait until a message arrives at `at`, or give up loudly.
///
/// Every arrival that is not the awaited one is collected and reported with the
/// failure — "it went somewhere else" and "it went nowhere" are two different
/// defects and a bare timeout tells them apart for nobody.
async fn arrival_at(rx: &mut mpsc::Receiver<Arrival>, at: &str) -> Message {
    let mut seen: Vec<String> = Vec::new();
    let deadline = tokio::time::Instant::now() + ARRIVAL;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some((path, msg))) if path == at => return msg,
            Ok(Some((path, _))) => seen.push(path),
            Ok(None) => panic!("the colony closed its taps; nothing reached `{at}`, saw {seen:?}"),
            Err(_) => panic!("nothing reached `{at}` within {ARRIVAL:?}; saw {seen:?}"),
        }
    }
}

/// Collect every address something reached within a short, fixed window.
///
/// Used where the claim is about a FAN-OUT — how many places one emission was
/// delivered to — which a single `arrival_at` cannot answer.
async fn arrivals_within(rx: &mut mpsc::Receiver<Arrival>, window: Duration) -> Vec<String> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return out;
        }
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some((path, _))) => out.push(path),
            Ok(None) | Err(_) => return out,
        }
    }
}

fn hop(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

/// One emission, the way a cell of the tree makes one — routed over the
/// SENDER's out-edges, which is the road under test. An ingress `Route` is
/// addressed at its target and would measure nothing about the topology.
fn emission(
    from: &str,
    ctx: Map<String, Value>,
    hop_header: Map<String, Value>,
    body: Value,
) -> CellEmission {
    let mut content = body;
    content["header"] = Value::Object(hop_header);
    CellEmission {
        sender_path: Path::new(from),
        parent_message_id: None,
        trace_id: Uuid::now_v7(),
        input_ttl: 16,
        input_headers: Headers::from_parts(ctx, Map::new()),
        input_reply_to: None,
        target: Path::new(from),
        content,
        direct_reply: false,
    }
}

/// Context, in the shape one hop would have promoted it.
fn context(pairs: &[(&str, Value)]) -> Map<String, Value> {
    hop(pairs)
}

/// A fantasy manifest — a test that names a real colony is a test that leaked
/// one. What matters is that it is an ordered list of declarations.
fn a_manifest() -> Value {
    json!([
        {"scope": "/os/orgs",
         "diff": {"add_nodes": [{"name": "acme", "template": "org@1.3.0"}]}}
    ])
}

fn count_mutation_rows(root: &std::path::Path) -> i64 {
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return 0;
    };
    conn.query_row("SELECT COUNT(*) FROM mutation_log", [], |r| r.get(0))
        .unwrap_or(0)
}

// ──────────────────────────────────────────────────────────────────────────────
// 2 — the road, hop by hop
// ──────────────────────────────────────────────────────────────────────────────

/// A manifest posted at the rim reaches the cell that gives it a sender.
///
/// The lane and the rim door are unchanged by #556; what changed is which
/// occupant is behind them, and the address is this test's claim.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_submission_posted_at_the_rim_reaches_the_intake() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let mut grown = grow().await;

    grown
        .handle
        .send(
            MessageBuilder::new(Path::new(OS))
                .hop(hop(&[("route", json!("in_submit"))]))
                .ttl(16)
                .body(Body::Inline(
                    json!({"messages": [], "manifest": a_manifest()}),
                ))
                .build(),
        )
        .await;

    let got = arrival_at(&mut grown.arrivals, INTAKE).await;
    assert_eq!(
        got.headers.hop.get("route").and_then(Value::as_str),
        Some("in_submit"),
        "the front door's own door routes `in_submit` to the intake unchanged"
    );
    grown.handle.shutdown().await;
}

/// `apply` never crosses a rim any more: one edge of the front door's own
/// graph carries it from the intake to the submitter's gate.
///
/// Through `operator@1.0.0` this leg was two shell edges — `./submit -> .` out
/// of the front door and `./operator -> ./submit` back into a sibling hive.
/// That is the station #556 removed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_intake_hands_the_manifest_to_the_gate_inside_the_same_hive() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let mut grown = grow().await;

    grown
        .handle
        .outputs_sender()
        .send(emission(
            INTAKE,
            context(&[]),
            hop(&[
                ("route", json!("apply")),
                ("operation", json!("operator_submit")),
                ("manifest_sha256", json!(DIGEST)),
            ]),
            json!({"messages": [], "manifest": a_manifest()}),
        ))
        .await
        .expect("the outputs channel is the production emission path");

    let got = arrival_at(&mut grown.arrivals, GATE).await;
    assert_eq!(
        got.headers.hop.get("route").and_then(Value::as_str),
        Some("in_apply"),
        "the front door re-stamps `apply` onto the lane the submitter accepts"
    );
    assert_eq!(
        got.headers
            .hop
            .get("manifest_sha256")
            .and_then(Value::as_str),
        Some(DIGEST),
        "the digest is what the gate checks the bytes against; a leg that dropped it \
         would make the check unperformable"
    );
    grown.handle.shutdown().await;
}

/// The capability question leaves the front door, and the shell says who is
/// asking — the occupant, not the hive around it (R-AC-1).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gates_question_reaches_the_broker_as_the_occupant_that_asked() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let mut grown = grow().await;

    grown
        .handle
        .outputs_sender()
        .send(emission(
            GATE,
            context(&[]),
            hop(&[("route", json!("ask")), ("manifest_sha256", json!(DIGEST))]),
            json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": "q1",
                                 "text": "{\"capability\":\"colony.mutate\"}"}]}),
        ))
        .await
        .expect("the outputs channel is the production emission path");

    let got = arrival_at(&mut grown.arrivals, POLICY).await;
    assert_eq!(
        got.headers.hop.get("route").and_then(Value::as_str),
        Some("in_request"),
        "the shell re-stamps `ask` onto the lane the broker accepts"
    );
    assert_eq!(
        got.headers.context.get("requester").and_then(Value::as_str),
        Some(ASKER),
        "the broker reads the requester off the EDGE and never out of a body (R-AC-1), \
         and the edge names the SUBMITTER — a rule naming `{FRONT_DOOR}` would permit an \
         export cell and a lifecycle composer the same thing"
    );
    assert_eq!(
        got.headers.context.get("sub_ask").and_then(Value::as_str),
        Some("1"),
        "the shell's own correlation marker, without which the verdict has no way back \
         into the front door"
    );
    assert_eq!(
        got.headers.context.get("sub_sha").and_then(Value::as_str),
        Some(DIGEST),
        "`hop.*` lives for one hop, so the digest has to be promoted or the answer \
         cannot be matched to the manifest it was asked about"
    );
    grown.handle.shutdown().await;
}

/// The verdict comes back into the front door and reaches the gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_verdict_comes_back_into_the_front_door() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let mut grown = grow().await;

    grown
        .handle
        .outputs_sender()
        .send(emission(
            POLICY,
            context(&[("sub_ask", json!("1")), ("sub_sha", json!(DIGEST))]),
            hop(&[("route", json!("grant"))]),
            json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "q1",
                                 "text": "{\"decision\":\"allowed\"}"}]}),
        ))
        .await
        .expect("the outputs channel is the production emission path");

    let got = arrival_at(&mut grown.arrivals, GATE).await;
    assert_eq!(
        got.headers.hop.get("route").and_then(Value::as_str),
        Some("in_verdict"),
        "the shell re-stamps the broker's `grant` onto the submitter's own lane, and the \
         front door carries it in — two rims where there used to be one"
    );
    grown.handle.shutdown().await;
}

/// The privileged lane still reaches the mutation door, from one hive deeper.
///
/// The positive receipt is a `mutation_log` row: the door writes one for what
/// it was handed. Whether that row COMMITS is the manifest's business — this
/// file asserts arrival, not outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_manifest_still_reaches_the_mutation_door() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let grown = grow().await;
    let before = count_mutation_rows(&grown.root);

    grown
        .handle
        .outputs_sender()
        .send(emission(
            GATE,
            context(&[]),
            hop(&[("route", json!("mutate"))]),
            json!({"messages": [], "manifest": a_manifest()}),
        ))
        .await
        .expect("the outputs channel is the production emission path");

    let deadline = tokio::time::Instant::now() + ARRIVAL;
    loop {
        if count_mutation_rows(&grown.root) > before {
            grown.handle.shutdown().await;
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no `mutation_log` row appeared within {ARRIVAL:?}: the `mutate` lane did not \
             reach `/colony/mutations` from `{GATE}`. Four hive transits carry it now — \
             the submitter, the front door, the shell and the colony's root — and one \
             missing exit edge on any of them ends the road"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// 3 — one submission, one answer
// ──────────────────────────────────────────────────────────────────────────────

/// An ordinary committed receipt is answered INSIDE the front door and does not
/// leave it a second time.
///
/// This is the guard on `sub_receipt`, and it is the reason the lane exists at
/// all: `receipt` is what a caller subscribes to, and a submission answered on
/// it twice — once rendered by the intake, once raw from the gate — is a
/// submission whose caller cannot tell which answer is the outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ordinary_committed_receipt_is_answered_once() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let mut grown = grow().await;

    grown
        .handle
        .outputs_sender()
        .send(emission(
            GATE,
            context(&[]),
            hop(&[
                ("route", json!("receipt")),
                ("applied", json!(1)),
                ("manifest_sha256", json!(DIGEST)),
            ]),
            json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "r1",
                                 "text": "one declaration committed"}]}),
        ))
        .await
        .expect("the outputs channel is the production emission path");

    let seen = arrivals_within(&mut grown.arrivals, Duration::from_secs(2)).await;
    assert!(
        seen.contains(&INTAKE.to_string()),
        "the submitter's receipt must reach the occupant that asked; saw {seen:?}"
    );
    assert!(
        !seen.iter().any(|p| p.starts_with("/os/builder/")),
        "an ordinary committed receipt has nothing to say to the baumeister and must \
         not leave the front door at all; saw {seen:?}"
    );
    grown.handle.shutdown().await;
}

/// A refusal reaches BOTH: the occupant that asked, and the baumeister whose
/// draft it was (the repair lane of GH #425).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refusal_also_leaves_on_the_front_doors_own_lane() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let mut grown = grow().await;

    grown
        .handle
        .outputs_sender()
        .send(emission(
            GATE,
            context(&[]),
            hop(&[
                ("route", json!("receipt")),
                ("error_code", json!("requester_not_permitted")),
            ]),
            json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "r2",
                                 "text": "the broker refused this submission"}]}),
        ))
        .await
        .expect("the outputs channel is the production emission path");

    let seen = arrivals_within(&mut grown.arrivals, Duration::from_secs(3)).await;
    assert!(
        seen.contains(&INTAKE.to_string()),
        "the refusal must still reach the occupant that asked; saw {seen:?}"
    );
    assert!(
        seen.contains(&BUILDER_WEAVE.to_string()),
        "a refusal closes a drafted round, so it has to reach the baumeister — over \
         `sub_receipt` out of the front door and `./operator -> ./builder` at the shell \
         (GH #425); saw {seen:?}"
    );
    grown.handle.shutdown().await;
}

/// A committed submission whose diff registered a template class nudges the
/// corpus that stopped describing the library (GH #504).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_registered_class_nudges_the_corpus_over_the_new_lane() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let mut grown = grow().await;

    grown
        .handle
        .outputs_sender()
        .send(emission(
            GATE,
            context(&[]),
            hop(&[
                ("route", json!("receipt")),
                ("registers_class", json!(true)),
                ("applied", json!(2)),
            ]),
            json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "r3",
                                 "text": "two declarations committed"}]}),
        ))
        .await
        .expect("the outputs channel is the production emission path");

    let seen = arrivals_within(&mut grown.arrivals, Duration::from_secs(3)).await;
    assert!(
        seen.contains(&BUILDER_LIBRARIAN.to_string()),
        "the nudge must reach the librarian: `./submit -> .` re-stamps the receipt onto \
         `sub_receipt`, and `./operator -> ./builder` re-stamps that onto `in_ingest` \
         (GH #504); saw {seen:?}"
    );
    grown.handle.shutdown().await;
}
