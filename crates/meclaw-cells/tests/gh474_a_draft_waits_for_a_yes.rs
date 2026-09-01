//! GH #474 — an operator's draft is a PROPOSAL again: it is parked, answered,
//! and applied only when somebody quotes its digest back.
//!
//! WHAT WAS MEASURED
//! =================
//! GH #469 gave the rim an `in_build` lane and routed the resulting draft into
//! the one submission front door. Building a whole colony out of wishes with it
//! works — and it removed a step the baumeister's own contract promises:
//!
//! > the draft — an ordered list of mutation declarations plus the sha256
//! > digest over it and a sentence a human can read before saying yes. It is a
//! > PROPOSAL: nothing here has applied it and nothing here can
//!
//! With that edge the sentence a human can read before saying yes travelled
//! straight past the human. Measured on a real rebuild: a wish for a connector
//! channel produced a correct three-edge declaration **and a running `proxy`
//! cell** in one round, and putting that connector back to sleep took a second
//! act — where growing it asleep would have been one.
//!
//! WHAT THIS FILE PROVES, POSITIVELY
//! =================================
//! Two halves, and neither is an absence.
//!
//! **The front door, on its own wire.** The shipped `operator/intake` script is
//! run through `python3` exactly as the substrate would run it — the cell that
//! was called `operator/submit` through operator@1.0.0 and is named for what it
//! does since GH #556, because the name `submit` now belongs to the submitter
//! hive that moved in beside it:
//!
//! 1. `in_draft` with a manifest parks it and answers — a `dstore` insert whose
//!    row carries the digest and the declarations, and a `receipt` marked
//!    `draft_state == 'draft_ready'` that names the digest and the place it
//!    waits. Nothing leaves on `apply`.
//! 2. `in_submit` carrying a digest and NO manifest asks the store for that one
//!    row, under that one digest.
//! 3. the row coming back un-parks into exactly one `apply`, with the parked
//!    bytes verbatim.
//! 4. a digest nothing is parked under answers `digest_mismatch` — and emits no
//!    `apply` at all. The submission was never made rather than made and
//!    refused, and those are different facts.
//!
//! **The shell, on a booted colony.** The stage-one seed is grown for real and
//! every cell is a tap, so what is measured is which ADDRESS a message reaches:
//!
//! 5. a wish at the rim with no `auto_submit` reaches the baumeister carrying
//!    `context.build_caller == 'operator'` and NO `build_auto_submit`; a wish
//!    with `auto_submit: true` carries both.
//! 6. the draft of the first round lands at `/os/operator/intake` on `in_draft`
//!    — and never on `in_submit`, which is the lane that would have applied it.
//! 7. the draft of the second round lands on `in_submit`, the 1.5.0 road,
//!    unchanged for the caller that asks for it.
//!
//! WHAT IT IS NOT
//! ==============
//! It runs no model and it applies no mutation. The question is which road a
//! draft takes and what the front door does with it; a cell that computed, or a
//! mutation door that committed, would only add variables. Whether the BROKER
//! then permits the submission is the policy's decision and a different file.
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
use meclaw_testing::code_wire::{emit_all, shipped_script};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const INTAKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/operator/intake/config.json"
);
const DRAFTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/operator/drafts/config.json"
);

/// The seed under test, and the shell it declares.
const SEED: &str = "examples/meclaw-os/seed-ref";
const SHELL: &str = "meclaw-os";
const OS: &str = "/os";
/// The baumeister's first cell — where a wish has to arrive.
const BUILDER_ENTRY: &str = "/os/builder/classify";
/// The cell that raises a recipe-drawn manifest.
const BUILDER_RECIPES: &str = "/os/builder/recipes";
/// The occupant of the one submission front door that takes a request in and
/// gives it a sender (R-Zielfluss (a)). Since GH #556 the sibling
/// `/os/operator/submit` is the SUBMITTER hive, and a draft never goes there:
/// it reaches it, if at all, over the hive's own `./intake -> ./submit` edge.
const FRONT_DOOR_INTAKE: &str = "/os/operator/intake";
/// The marker the level's own door stamps on an operator-initiated round.
const CALLER_KEY: &str = "build_caller";
const CALLER_OPERATOR: &str = "operator";
/// The word that turns the halt OFF, promoted by the rim door.
const AUTO_KEY: &str = "build_auto_submit";
const AUTO_YES: &str = "yes";

/// Generous, per the 30s failure-marker convention.
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
// the front door, on its own wire
// ──────────────────────────────────────────────────────────────────────────────

/// The one declaration a first wish would draw — a fantasy name, because a test
/// that names a real colony is a test that leaked one.
fn a_manifest() -> Value {
    json!([
        {"scope": "/os/orgs/acme", "ctx": {},
         "diff": {"add_nodes": [{"name": "scribe", "template": "member@1.4.0"}]}}
    ])
}

/// One request at the front door's `intake` occupant.
fn front_door(hop: Value, body: Value, context: Value) -> Vec<Value> {
    let mut flat = body;
    flat["target"] = json!(FRONT_DOOR_INTAKE);
    flat["header"] = json!({"hop": hop, "context": context});
    flat["ttl"] = json!(64);
    flat["params"] = json!({});
    emit_all(&shipped_script(INTAKE), &flat)
}

/// The `tool_call` args of a store operation the front door emitted.
fn store_args(msg: &Value) -> Value {
    meclaw_core::serde_json::from_str(
        msg["messages"][0]["text"]
            .as_str()
            .expect("a store operation travels as a tool_call turn"),
    )
    .expect("json")
}

#[test]
fn a_drawn_draft_is_parked_and_answered_and_nothing_is_applied() {
    let manifest = a_manifest();
    let out = front_door(
        json!({"route": "in_draft", "tool_call_id": "c1"}),
        json!({"manifest": manifest.clone(), "messages": []}),
        json!({}),
    );
    assert_eq!(
        out.len(),
        2,
        "park, then answer — never one and never three"
    );

    // Nothing may leave on the lane that reaches the submitter. This is the
    // whole defect, asserted as a lane rather than as an outcome.
    assert!(
        out.iter().all(|m| m["header"]["route"] != json!("apply")),
        "a draft that emits `apply` is the very thing GH #474 removes: {out:?}"
    );

    let park = store_args(&out[0]);
    assert_eq!(out[0]["header"]["route"], "dstore", "the interior lane");
    assert_eq!(park["operation"], "insert");
    assert_eq!(park["table"], "drafts");
    assert_eq!(
        park["row"]["manifest"], manifest,
        "the parked bytes are the drawn bytes, verbatim — a park that re-rendered them \
         would park a manifest nobody was shown"
    );
    let sha = park["row"]["manifest_sha256"]
        .as_str()
        .expect("a digest on the row");
    assert_eq!(sha.len(), 64, "a sha256 hex digest is 64 characters");

    let answer = &out[1];
    assert_eq!(answer["header"]["route"], "receipt", "the one lane out");
    assert_eq!(
        answer["header"]["draft_state"], "draft_ready",
        "the receipt has to SAY it is a proposal; a caller that has to infer it from the \
         absence of counts is a caller that will infer wrong"
    );
    assert_eq!(answer["header"]["manifest_sha256"], json!(sha));
    assert_eq!(answer["header"]["declaration_count"], 1);
    assert_eq!(
        answer["header"]["draft_path"], "/os/operator/drafts",
        "the place it waits, derived from the target rather than written down — this \
         template is instantiable anywhere"
    );
    assert_eq!(
        answer["manifest"], manifest,
        "a person cannot say yes to a digest they were not shown the bytes of"
    );
    let said = answer["messages"][0]["text"].as_str().expect("a sentence");
    assert!(said.contains(sha), "the sentence names the digest: {said}");
    assert!(
        said.contains("nothing has been applied"),
        "and says what did NOT happen: {said}"
    );
}

#[test]
fn a_quoted_digest_asks_the_store_for_that_one_row() {
    let out = front_door(
        json!({"route": "in_submit", "manifest_sha256": "deadbeef", "tool_call_id": "c2"}),
        json!({"messages": []}),
        json!({}),
    );
    assert_eq!(out.len(), 1, "one read, and no submission yet");
    assert_eq!(out[0]["header"]["route"], "dstore");
    let ask = store_args(&out[0]);
    assert_eq!(ask["operation"], "select");
    assert_eq!(ask["table"], "drafts");
    assert_eq!(
        ask["where"]["manifest_sha256"], "deadbeef",
        "the digest is the key: `submit <digest>` is a decision about the bytes somebody \
         was shown, not about the bytes the caller happens to hold"
    );
    assert!(
        ask["order_by"].is_array() && ask["limit"] == json!(1),
        "a `limit` without an `order_by` returns an unspecified row"
    );
}

/// The store's answer, spelled the way the hive's own edge promotes it.
fn unparked(rows: Value, carry: Value) -> Vec<Value> {
    front_door(
        json!({"operation": "store_select"}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "",
                             "text": rows.to_string()}]}),
        json!({"op_origin": "drafts", "op_phase": "unpark", "op_carry": carry.to_string()}),
    )
}

#[test]
fn the_parked_row_comes_back_as_exactly_one_submission() {
    let manifest = a_manifest();
    // The digest the front door itself drew, taken off the park it emitted —
    // never a literal, so the two halves of this file cannot drift apart.
    let park = store_args(
        &front_door(
            json!({"route": "in_draft"}),
            json!({"manifest": manifest.clone(), "messages": []}),
            json!({}),
        )[0],
    );
    let sha = park["row"]["manifest_sha256"].as_str().expect("digest");

    let out = unparked(
        json!([{"id": "r1", "manifest": manifest, "manifest_sha256": sha,
                "tool_call_id": "c2"}]),
        json!({"digest": sha, "call_id": "op:c2", "agent": false}),
    );
    assert_eq!(out.len(), 1, "one apply, and no second park");
    assert_eq!(out[0]["header"]["route"], "apply");
    assert_eq!(out[0]["header"]["manifest_sha256"], json!(sha));
    assert_eq!(out[0]["header"]["declaration_count"], 1);
    assert_eq!(
        out[0]["manifest"],
        a_manifest(),
        "the bytes that travel are the bytes that were parked"
    );
    assert_eq!(
        out[0]["header"]["tool_call_id"], "op:c2",
        "the marked id rides through the store round trip in the carry — the store's \
         answer begins with no hop of this cell's own"
    );
}

#[test]
fn a_digest_nothing_is_parked_under_submits_nothing() {
    let out = unparked(
        json!([]),
        json!({"digest": "deadbeef", "call_id": "op:c2", "agent": false}),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "receipt");
    assert_eq!(
        out[0]["header"]["error_code"], "digest_mismatch",
        "a submission that was never made is not a submission that failed"
    );
    assert_eq!(out[0]["header"]["expected"], "deadbeef");
    assert!(
        out.iter().all(|m| m["header"]["route"] != json!("apply")),
        "nothing may reach the submitter under a digest nothing is parked under"
    );
}

#[test]
fn a_manifest_in_the_body_still_takes_the_old_road() {
    // The half that makes the two above a HALT rather than a re-route: an
    // operator who posts a manifest is still answered with one submission.
    let out = front_door(
        json!({"route": "in_submit"}),
        json!({"manifest": a_manifest(), "messages": []}),
        json!({}),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "apply");
    assert_eq!(out[0]["manifest"], a_manifest());
}

#[test]
fn the_drafts_store_is_reachable_only_from_inside_the_hive() {
    let store = read_json(&repo("templates/operator/drafts/config.json"));
    assert_eq!(store["cell"]["type"], "store");
    assert_eq!(
        store["contract"]["write_surface"], "internal",
        "the front door declares `params.ports: []`, so the hive path is the only address \
         and the only sender inside it is `./intake`"
    );
    assert!(
        store["params"]["schema"]["drafts"]["manifest_sha256"] == json!("text")
            && store["params"]["schema"]["drafts"]["manifest"] == json!("json"),
        "the digest is the key and the declarations are the value"
    );
    let _ = DRAFTS;
}

// ──────────────────────────────────────────────────────────────────────────────
// the tap factory — gh465's inert cell with a channel on its mailbox
// ──────────────────────────────────────────────────────────────────────────────

type Arrival = (String, Message);

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

fn hop(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

/// One emission, the way a cell of the tree makes one.
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

/// A wish at the rim, optionally asking for the one-act road.
fn wish(auto: Option<bool>) -> Message {
    let mut h = hop(&[("route", json!("in_build"))]);
    if let Some(v) = auto {
        h.insert("auto_submit".into(), json!(v));
    }
    MessageBuilder::new(Path::new(OS))
        .hop(h)
        .ttl(16)
        .body(Body::Inline(json!({"messages": [{
            "origin": "user", "type": "text", "id": "",
            "text": "{\"request\":\"grow a member named scribe from member@1.4.0 \
                     under /os/orgs/acme\",\"scope\":\"/os/orgs/acme\"}"
        }]})))
        .build()
}

// ──────────────────────────────────────────────────────────────────────────────
// the rim door reads one more word off the wish
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rim_door_says_which_road_the_draft_will_take() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let mut grown = grow().await;

    grown.handle.send(wish(None)).await;
    let plain = arrival_at(&mut grown.arrivals, BUILDER_ENTRY).await;
    assert_eq!(plain.headers.hop.get("route"), Some(&json!("in_build")));
    assert_eq!(
        plain.headers.context.get(CALLER_KEY),
        Some(&json!(CALLER_OPERATOR)),
        "the level's own door stamps which door a round came in at"
    );
    assert!(
        plain.headers.context.get(AUTO_KEY).is_none(),
        "the DEFAULT is the halt: a wish that says nothing must not be promoted to the \
         one-act road, or a proposal applies itself by default"
    );

    grown.handle.send(wish(Some(true))).await;
    let auto = arrival_at(&mut grown.arrivals, BUILDER_ENTRY).await;
    assert_eq!(
        auto.headers.context.get(AUTO_KEY),
        Some(&json!(AUTO_YES)),
        "`hop.auto_submit: true` is the word that turns the halt off, and the level \
         promotes it because a hop lives for exactly one hop"
    );
    assert_eq!(
        auto.headers.context.get(CALLER_KEY),
        Some(&json!(CALLER_OPERATOR)),
        "both roads are still an operator's"
    );

    let dead = grown.handle.drain_dead_letters().await;
    assert!(
        dead.is_empty(),
        "a wish on a declared lane must not be refused anywhere on the way: {dead:?}"
    );
    grown.handle.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────────
// and the draft takes it
// ──────────────────────────────────────────────────────────────────────────────

/// Inject the draft at the cell that raises it in production and read which
/// lane it arrives on at the front door.
async fn lane_of_the_draft(auto: bool) -> Message {
    let mut grown = grow().await;
    let mut ctx = hop(&[(CALLER_KEY, json!(CALLER_OPERATOR))]);
    if auto {
        ctx.insert(AUTO_KEY.into(), json!(AUTO_YES));
    }
    grown
        .handle
        .outputs_sender()
        .send(emission(
            BUILDER_RECIPES,
            hop(&[
                ("operation", json!("recipe")),
                ("declaration_count", json!(1)),
            ]),
            ctx,
            json!({"messages": [], "manifest": a_manifest()}),
        ))
        .await
        .expect("the outputs channel is the production emission path");
    let got = arrival_at(&mut grown.arrivals, FRONT_DOOR_INTAKE).await;
    let dead = grown.handle.drain_dead_letters().await;
    assert!(
        dead.is_empty(),
        "the draft was refused on the way: {dead:?}"
    );
    grown.handle.shutdown().await;
    got
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operators_draft_stops_at_the_front_door_to_be_looked_at() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let got = lane_of_the_draft(false).await;
    assert_eq!(
        got.headers.hop.get("route"),
        Some(&json!("in_draft")),
        "the halt is a LANE and not a flag: `in_submit` is the lane that reaches the \
         submitter, and a draft nobody has seen must not arrive on it"
    );
    match &got.body {
        Body::Inline(v) => assert_eq!(
            v.get("manifest"),
            Some(&a_manifest()),
            "the front door parks the drawn bytes, so they have to travel with the draft"
        ),
        other => panic!("the draft arrived with a body the front door cannot read: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_one_act_road_is_still_there_for_the_caller_that_asks_for_it() {
    if !shipped() {
        eprintln!("skipped: {SEED} did not ship (GH #49)");
        return;
    }
    let got = lane_of_the_draft(true).await;
    assert_eq!(
        got.headers.hop.get("route"),
        Some(&json!("in_submit")),
        "`auto_submit: true` is the 1.5.0 road, unchanged — a rebuild script replaying \
         wishes somebody has already read is a real caller and not a loophole"
    );
}
