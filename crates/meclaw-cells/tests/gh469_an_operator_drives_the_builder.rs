//! GH #469 — stage two, from the outside: an operator drives the builder, and
//! the draft lands at the one submission front door.
//!
//! WHAT WAS MEASURED, AND WHY IT NEEDED A LANE
//! ===========================================
//! A stage-one colony (`examples/meclaw-os/seed-ref`) grows the shell and
//! nothing else — `/os/orgs` is the address an organisation is instantiated AT,
//! and it is empty. Until GH #469 the baumeister was reachable on exactly one
//! road: `./orgs -> ./builder`, an assistant inside an organisation raising
//! `build` with `build_op == 'draft'`. So the FIRST build of a fresh colony had
//! no caller: the only sender the edge was written for does not exist yet.
//!
//! An operator can address a hive path and seed `hop.route` (GH #175), and the
//! builder did draft — `classify`, the `grow_level` recipe, `recipes`, a
//! `manifest` with `manifest_sha256` stamped. The draft then died at
//! `./builder -> ./orgs`, in an empty container, as `hive_no_route`.
//!
//! WHAT THIS FILE PROVES, POSITIVELY
//! =================================
//! Two arrivals, each read off a cell that actually received the message —
//! never off the dead-letter queue, which is anti-correlated with correctness
//! and cannot tell "arrived somewhere else" from "arrived nowhere".
//!
//! 1. **The wish gets in.** A message addressed to `/os` — the rim, not the
//!    builder's own hive path — on `hop.route == 'in_build'` reaches the
//!    baumeister's first cell, carrying `context.build_caller == 'operator'`
//!    that the level's own door stamped. The caller never says which door it
//!    came in at; the level does.
//! 2. **The draft gets out, into the submission front door.** A `manifest` the
//!    builder raises in that round reaches `/os/operator/intake`, with the
//!    manifest and the digest the front door reads. That address is the cell
//!    that was called `/os/operator/submit` through operator@1.0.0 — since GH
//!    #556 the name `submit` belongs to the SUBMITTER hive, which moved in
//!    beside it and is one interior edge further on. R-Zielfluss (a): the
//!    operator hive is the ONE submission front door, and an operator-initiated
//!    draft takes the same road an assistant's does. The LANE it arrives on is
//!    `in_draft` since GH #474 and was `in_submit` through 1.5.0 — the address
//!    is this file's claim, what the front door then does with the draft is
//!    `gh474_a_draft_waits_for_a_yes.rs`.
//!
//! And the counterpart, because an edge that survives a re-hang delivers twice:
//! the same draft does NOT also go to `./orgs`, while an agent-initiated one
//! still does and only does that.
//!
//! WHAT IT IS NOT
//! ==============
//! It is not a measurement of whether the BROKER lets such a submission
//! through. That is the policy's decision, not the wiring's: the shipped
//! `colony.mutate.default` row scopes an agent's mutations to `/os/orgs`, so a
//! first organisation — whose scope is necessarily `/os` — comes back as a
//! NAMED refusal (`requester_not_permitted`) rather than as a silent dead
//! letter. Turning a `hive_no_route` into an answer is exactly what this lane
//! buys; which answers the policy gives is a different file.
//!
//! It also runs no model. Every cell of the grown shell is a TAP here: the
//! question is which address a message reaches, and a cell that computed
//! something would only add a second variable. The device is `gh465`'s inert
//! factory with a channel bolted on.
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
/// The baumeister's first cell — where `in_build` has to arrive.
const BUILDER_ENTRY: &str = "/os/builder/classify";
/// The cell that raises a recipe-drawn manifest.
const BUILDER_RECIPES: &str = "/os/builder/recipes";
/// The occupant of the one submission front door that takes a request in and
/// gives it a sender (R-Zielfluss (a)). Since GH #556 its sibling
/// `/os/operator/submit` is the submitter hive itself, reached over the front
/// door's own `./intake -> ./submit` edge and never from outside.
const FRONT_DOOR_INTAKE: &str = "/os/operator/intake";
/// The container an organisation is instantiated into — empty in a fresh
/// colony, which is the whole of the defect.
const CONTAINER: &str = "/os/orgs";
/// The marker the level's own door stamps on an operator-initiated round.
const CALLER_KEY: &str = "build_caller";
const CALLER_OPERATOR: &str = "operator";
/// A digest shape, not a digest: what is measured is that the pin SURVIVES the
/// re-stamp, and the bytes it was drawn over are the baumeister's business.
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

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

/// Did the seed and the shell travel with this tree (GH #49)?
fn shipped() -> bool {
    repo(&format!("{SEED}/main/os/config.json")).is_file()
        && repo(&format!("templates/{SHELL}/template.json")).is_file()
}

// ──────────────────────────────────────────────────────────────────────────────
// the tap factory — gh465's inert cell with a channel on its mailbox
// ──────────────────────────────────────────────────────────────────────────────

/// One `(path, message)` pair per message a cell of the grown shell received.
type Arrival = (String, Message);

/// Every cell of the tree, replaced by something that only says "I got this".
///
/// The same reasoning `gh465`'s `InertCellFactory` carries: what is measured is
/// the topology, and a cell that computed would be a second variable. This one
/// adds the half `InertCellFactory` throws away — WHICH address received it.
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
    _root: tempfile::TempDir,
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

    Grown {
        handle,
        arrivals,
        _root: td,
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

/// Everything that has arrived so far, drained without waiting.
fn drained(rx: &mut mpsc::Receiver<Arrival>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok((path, _)) = rx.try_recv() {
        out.push(path);
    }
    out
}

fn hop(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

fn context(pairs: &[(&str, Value)]) -> Map<String, Value> {
    hop(pairs)
}

/// One emission, the way a cell of the tree makes one.
///
/// The production path and not the ingress one: a cell's output is routed over
/// the SENDER's out-edges and the `target` on it is a diagnostic, while
/// `ColonyMsg::Route` — what `ColonyHandle::send` builds — is addressed at its
/// target. Injecting the draft as a Route would measure the wrong road
/// entirely; it was measured, and it dies at `/os` with no lane on it.
fn emission(
    from: &str,
    hop_header: Map<String, Value>,
    ctx: Map<String, Value>,
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

/// The one declaration an operator's first wish would draw — a fantasy name,
/// because a test that names a real colony is a test that leaked one.
fn a_manifest() -> Value {
    json!([
        {"scope": "/os/orgs/acme",
         "diff": {"add_nodes": [{"name": "scribe", "template": "member@1.5.0"}]}}
    ])
}

// ──────────────────────────────────────────────────────────────────────────────
// 1 — the wish gets in, at the rim
// ──────────────────────────────────────────────────────────────────────────────

/// A build wish posted at `/os` reaches the baumeister, and the LEVEL says who
/// asked.
///
/// Addressed at the rim on purpose. Before GH #469 the only way in was the
/// builder's own hive path, which works (GH #175) and is not an interface: a
/// lane the level does not declare is a lane no caller can be told about, and
/// nothing promises it will still be there next version.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operators_wish_reaches_the_baumeister_through_the_rim() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let mut grown = grow().await;

    grown
        .handle
        .send(
            MessageBuilder::new(Path::new(OS))
                .hop(hop(&[("route", json!("in_build"))]))
                .ttl(16)
                .body(Body::Inline(json!({"messages": [{
                    "origin": "user", "type": "text", "id": "",
                    "text": "{\"request\":\"grow a member named scribe from member@1.5.0 \
                             under /os/orgs/acme\",\"scope\":\"/os/orgs/acme\"}"
                }]})))
                .build(),
        )
        .await;

    let got = arrival_at(&mut grown.arrivals, BUILDER_ENTRY).await;
    assert_eq!(
        got.headers.hop.get("route"),
        Some(&json!("in_build")),
        "the lane the level accepts is the lane the baumeister is handed"
    );
    assert_eq!(
        got.headers.context.get(CALLER_KEY),
        Some(&json!(CALLER_OPERATOR)),
        "the level's own door must stamp `context.{CALLER_KEY}` — which door a request came in \
         at is a fact of this level and never a caller's claim, and the return leg of the draft \
         is decided by it"
    );

    let dead = grown.handle.drain_dead_letters().await;
    assert!(
        dead.is_empty(),
        "a wish on a declared lane must not be refused anywhere on the way: {dead:?}"
    );
    grown.handle.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────────
// 2 — the draft gets out, into the one submission front door
// ──────────────────────────────────────────────────────────────────────────────

/// The manifest an operator-initiated round produced reaches
/// `/os/operator/intake`, carrying what the front door reads.
///
/// **RETRACTED, GH #474: the lane is `in_draft` and no longer `in_submit`.**
/// What GH #469 bought is that the draft reaches the one submission front door
/// instead of dying as `hive_no_route` in an empty container, and that claim is
/// untouched. What it also did, unintentionally, was submit the draft in the
/// same round — the baumeister's contract calls what it emits a PROPOSAL, and
/// nobody saw the digest before it ran. Since GH #474 the draft is parked and
/// answered at that same address, and `hop.auto_submit: true` on the wish is
/// what asks for the older one-act road. Both lanes are pinned by
/// `gh474_a_draft_waits_for_a_yes.rs`; this test keeps the ADDRESS claim.
///
/// The emission is injected AT the cell that raises it in production
/// (`./recipes`, `hop.operation == 'recipe'` with no `error_code`), so the
/// builder's own outward edge is exercised rather than assumed. What is not
/// exercised is the model — see the header.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operator_initiated_draft_lands_at_the_submission_front_door() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let mut grown = grow().await;
    let manifest = a_manifest();

    grown
        .handle
        .outputs_sender()
        .send(emission(
            BUILDER_RECIPES,
            hop(&[
                ("operation", json!("recipe")),
                ("manifest_sha256", json!(DIGEST)),
                ("declaration_count", json!(1)),
            ]),
            context(&[(CALLER_KEY, json!(CALLER_OPERATOR))]),
            json!({"messages": [], "manifest": manifest.clone()}),
        ))
        .await
        .expect("the outputs channel is the production emission path");

    let got = arrival_at(&mut grown.arrivals, FRONT_DOOR_INTAKE).await;
    assert_eq!(
        got.headers.hop.get("route"),
        Some(&json!("in_draft")),
        "the front door recognises what to do by the lane and nothing else — and since GH \
         #474 the default lane for an operator-asked draft is the one that PARKS it"
    );
    assert_eq!(
        got.headers.hop.get("manifest_sha256"),
        Some(&json!(DIGEST)),
        "the digest the baumeister stamped must survive the re-stamp — the front door honours a \
         pin rather than three hops later"
    );
    match &got.body {
        Body::Inline(v) => assert_eq!(
            v.get("manifest"),
            Some(&manifest),
            "the front door needs the manifest in the BODY, on either lane: `in_draft` parks \
             these bytes and `in_submit` forwards them"
        ),
        other => panic!("the draft arrived with a body Phase A cannot read: {other:?}"),
    }
    assert!(
        got.headers.context.get("operator_caller").is_none(),
        "an operator is not an agent: the marker the `./orgs -> ./operator` edge stamps must \
         stay off this road, or a person's receipt is sent back down into an empty container"
    );

    // And it went to ONE place. `./orgs` is empty in a fresh colony, which is
    // where the draft used to die.
    let dead = grown.handle.drain_dead_letters().await;
    assert!(
        dead.is_empty(),
        "the draft was still refused somewhere — this is the `hive_no_route` GH #469 removed: \
         {dead:?}"
    );
    let elsewhere = drained(&mut grown.arrivals);
    assert!(
        !elsewhere.iter().any(|p| p.starts_with(CONTAINER)),
        "the same draft was delivered into the container as well — an edge that survives a \
         re-hang fans out and submits twice: {elsewhere:?}"
    );
    grown.handle.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────────
// 3 — and the road an assistant takes is untouched
// ──────────────────────────────────────────────────────────────────────────────

/// The counter-guard, measured rather than read: a draft with no
/// `context.build_caller` still goes home to the organisation that asked, and
/// not to the front door.
///
/// This is the half that makes the guard on the new edge a guard and not a
/// re-route. It runs against a colony with an organisation in the container, so
/// "it arrived at `./orgs`" is an arrival rather than the absence of one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_initiated_draft_still_goes_home_and_not_to_the_front_door() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let mut grown = grow().await;

    grown
        .handle
        .outputs_sender()
        .send(emission(
            BUILDER_RECIPES,
            hop(&[("operation", json!("recipe"))]),
            Map::new(),
            json!({"messages": [], "manifest": a_manifest()}),
        ))
        .await
        .expect("the outputs channel is the production emission path");

    // The container is empty in a stage-one colony, so the agent's road ends in
    // the dead-letter queue HERE — and that is the positive signal available:
    // the target it was refused at is `./orgs`, which is exactly where an
    // agent-initiated draft belongs and where an operator's must not go.
    let mut refused = Vec::new();
    let deadline = tokio::time::Instant::now() + ARRIVAL;
    while tokio::time::Instant::now() < deadline && refused.is_empty() {
        refused = grown.handle.drain_dead_letters().await;
        if refused.is_empty() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    let at: Vec<&str> = refused.iter().map(|d| d.resolved_target.as_str()).collect();
    assert!(
        at.contains(&CONTAINER),
        "an agent-initiated draft must still be routed at `{CONTAINER}` — the counter-guard on \
         the old edge has re-routed a road it was only meant to narrow. It was refused at {at:?}"
    );
    let elsewhere = drained(&mut grown.arrivals);
    assert!(
        !elsewhere.iter().any(|p| p == FRONT_DOOR_INTAKE),
        "an agent's draft reached the operator's front door — the guard on the new edge is not \
         guarding: {elsewhere:?}"
    );
    grown.handle.shutdown().await;
}
