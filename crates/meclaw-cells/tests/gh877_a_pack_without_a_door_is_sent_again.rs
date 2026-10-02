//! GH #877 — an identity pack counts as delivered only when its receipt comes
//! back clean.
//!
//! WHAT WAS BROKEN
//! ===============
//! `affinity/push` wrote `subscribers.pack_hash` and `sent_at` in the same
//! breath as it SENT the pack. Measured in the Gate wave (F10): a tick that
//! fired before the generation had its identity door emitted a pack that met no
//! edge, the row recorded it as delivered, and every later tick computed the
//! same hash over the same record and stayed silent -- the brain never received
//! its identity. The receipt (`pack_ack`) existed and was consumed by nobody:
//! the member let it leave the level, and affinity had no lane that took one.
//!
//! WHAT IS PINNED HERE
//! ===================
//! * the push leaves the row untouched and names it on the pack instead
//!   (`pack_sub`, `pack_hash`), the delivering v-lane promotes both into
//!   context, the curator hands the context back on its receipt, and the
//!   member's `./assistants -> ./affinity` edge brings it home as
//!   `in_pack_ack` -- only a CLEAN receipt writes the hash;
//! * a pack sent before the door exists is sent again after the door is drawn,
//!   exactly once (the lock), and `talky-chat` gets its own pack;
//! * a refused receipt books nothing and parks its hash, an anonymous one is
//!   dropped; a pack nobody confirms goes again on the next tick, then after a
//!   doubling wait under a six-hour cap -- at most five packs in twenty ticks
//!   (review I-1);
//! * no receipt leaves the member: every one, named or not, takes the same
//!   edge into `./affinity`, and neither `member` nor `org` nor `meclaw-os`
//!   emits `pack_ack` or carries it out any more (OR-KX-66);
//! * the seeded row carries the id the gate derives, and a second subscribe
//!   leaves one ACTIVE row under it, the first superseded by status (F19);
//! * the builder announces the three curator summarizers with their need
//!   (OR-KX-P5).
//!
//! Messart: at the receiver -- the store's own `cell.db`, the curator's ledger,
//! the colony's `message_log` -- never "the push emitted".
//!
//! R2b / GH #49: a tree that does not carry the templates skips.

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::config::HiveParams;
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, RespawnFn, SpawnedCellKind,
    WakeFn, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Headers, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::code_wire::{code_stdin, emit_all, run_shipped_script, shipped_script};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

// ══════════════════════════════════════════════════════════ the shipped tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn templates_root() -> std::path::PathBuf {
    repo("templates")
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

const RECIPES: &str = "templates/builder/recipes/config.json";
const ASSISTANT_EXAMPLE: &str = "examples/organism/grow-assistant.json";
const PUSH: &str = "templates/affinity/push/config.json";
const GATE: &str = "templates/affinity/gate/config.json";
const MEMBER: &str = "templates/member/config.json";
const SEED: &str = "templates/affinity/store/seed/subscribers.jsonl";

/// Where the level is rendered and under which name: the member the recipe
/// grows into, and the generation. The colony below stands in for that member,
/// so every rendered edge is rebased onto it.
const SCOPE: &str = "/os/orgs/acme/members/alex";
const NAME: &str = "scribe";
/// The container scope the registry pushes into, as `meclaw-os` sets it.
const REGISTRY_SCOPE: &str = "/os/orgs";

const AFFINITY_FILES: &[&str] = &[
    "config.json",
    "store/config.json",
    "brief/config.json",
    "gate/config.json",
    "push/config.json",
    "clock/config.json",
    "store/seed/entities.jsonl",
    "store/seed/relations.jsonl",
    "store/seed/trust.jsonl",
    "store/seed/disclosure.jsonl",
    "store/seed/subscribers.jsonl",
];

fn shipped(name: &str, files: &[&str]) -> Option<std::path::PathBuf> {
    let root = templates_root().join(name);
    files
        .iter()
        .all(|rel| root.join(rel).exists())
        .then_some(root)
}

fn affinity_shipped() -> Option<std::path::PathBuf> {
    shipped("affinity", AFFINITY_FILES)
}

/// The generation, and the curator its rims stand in front of (`curator@1.0.0`,
/// GH #888). Without both the lock measures a fixture, not the lane.
fn generation_shipped() -> Option<std::path::PathBuf> {
    [MEMBER, RECIPES, ASSISTANT_EXAMPLE]
        .iter()
        .all(|f| repo(f).is_file())
        .then_some(())?;
    shipped("curator", &["config.json"])?;
    shipped(
        "assistant",
        &["config.json", "talky/config.json", "cogny/config.json"],
    )
}

/// The id `./gate` derives for a subscription (gate:
/// `"sub:" + subscriber + "|" + subject`). Written once here because F19 is
/// exactly the claim that the seed and the gate spell it the same way.
fn derived_id(subscriber: &str, subject: &str) -> String {
    format!("sub:{subscriber}|{subject}")
}

/// The shipped template, copied the way instantiation copies it: `config.json`
/// files, the seed tables next to them, and a `ref` resolved to the tree it
/// names (GH #277).
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    let src = &resolve_template_ref(src);
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_cells(&from, &dst.join(name));
        } else if name == "config.json"
            || src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl")
        {
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

fn resolve_template_ref(dir: &std::path::Path) -> std::path::PathBuf {
    let mut dir = dir.to_path_buf();
    for _ in 0..8 {
        let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) else {
            return dir;
        };
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            return dir;
        };
        if v["cell"]["type"] != "ref" {
            return dir;
        }
        let reference = v["cell"]["template"]
            .as_str()
            .expect("a ref cell names a template");
        dir = templates_root().join(reference.split('@').next().unwrap_or_default());
    }
    panic!("template ref chain does not terminate at {}", dir.display());
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v = read_json(&p);
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// The member's own edge that takes a receipt home (GH #877), read off the
/// shipped template rather than written here: the lanes below measure THAT
/// edge, and a copy of it would measure the copy. It is picked by its LANE as
/// well as its endpoints: `./assistants -> ./affinity` also carries a
/// generation's `brief` (GH #834), and that edge stands first.
fn member_ack_edge() -> Value {
    let member = read_json(&repo(MEMBER));
    member["params"]["graph"]["edges"]
        .as_array()
        .expect("the member draws edges")
        .iter()
        .find(|e| {
            e["from"] == "./assistants"
                && e["to"] == "./affinity"
                && e["condition"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("hop.route == 'pack_ack'")
        })
        .cloned()
        .expect(
            "the member carries no `./assistants -> ./affinity` edge: a receipt has no way \
             home, and every pack stays unconfirmed (GH #877)",
        )
}

// ═════════════════════════════════════════ (a) the push script, on its stdin

fn run_push(flat: &Value) -> std::process::Output {
    run_shipped_script(
        &shipped_script(repo(PUSH).to_str().expect("utf-8")),
        &code_stdin(flat).to_string(),
    )
}

fn ack_doc(context: Value, error_code: &str) -> Value {
    json!({
        "target": "/affinity/push",
        "header": {"hop": {"route": "in_pack_ack", "error_code": error_code,
                           "pack_owner": "/affinity/brief", "pack_slots": "identity",
                           "pack_unknown": ""},
                   "context": context},
        "ttl": 64,
        "messages": [],
    })
}

fn store_args(m: &Value) -> Value {
    let text = m["messages"][0]["text"].as_str().unwrap_or("{}");
    meclaw_core::serde_json::from_str(text).unwrap_or(Value::Null)
}

/// One `entities` round of the shipped push over ONE subscriber row, exactly
/// as the store's answer brings it back (`json` columns arrive as TEXT); the
/// entity's `recorded_at` is the knob that moves the pack's hash.
fn entities_round(retry: &str, recorded_at: &str) -> Vec<Value> {
    let sub = json!({"id": "sub:/g|entity:alex", "cell_path": "/g", "subject": "entity:alex",
                     "audience": "member:alex", "channel": "*", "slots": "[\"identity\"]",
                     "pack_hash": "", "retry": retry});
    emit_all(
        &shipped_script(repo(PUSH).to_str().expect("utf-8")),
        &json!({
            "target": "/affinity/push",
            "header": {"hop": {"route": "astore", "operation": "select"},
                       "context": {"aff_phase": "entities",
                                   "aff_carry": json!({"subs": [sub]}).to_string()}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!([{"entity_id": "entity:alex", "aieos": "{}",
                                          "mx": "{}", "recorded_at": recorded_at}])
                                  .to_string()}],
        }),
    )
}

const RECORDED: &str = "2026-09-01T00:00:00Z";

/// The `subscribers` writes among one round's messages.
fn subscriber_writes(out: &[Value]) -> Vec<Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "astore")
        .map(store_args)
        .filter(|a| a["table"] == "subscribers")
        .collect()
}

/// A send is not a delivery: the tick that finds a changed subscriber renders
/// its pack and names the row and the hash ON the pack -- and books no
/// delivery. Before GH #877 the same output wrote `pack_hash`/`sent_at`; since
/// the fix round it writes the TRY (`retry`), which is what bounds a resend.
#[test]
fn a_send_books_no_delivery_and_names_the_row_it_serves() {
    if affinity_shipped().is_none() {
        return;
    }
    let out = entities_round("", RECORDED);
    let briefs: Vec<&Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == "brief")
        .collect();
    assert_eq!(briefs.len(), 1, "one changed subscriber, one pack: {out:?}");
    assert_eq!(
        briefs[0]["header"]["pack_sub"], "sub:/g|entity:alex",
        "the pack names the row it serves, so its receipt can find the row again: {:?}",
        briefs[0]["header"]
    );
    let hash = briefs[0]["header"]["pack_hash"]
        .as_str()
        .unwrap_or_default();
    assert!(
        hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()),
        "and the hash of what it sends: {:?}",
        briefs[0]["header"]
    );
    let writes = subscriber_writes(&out);
    assert_eq!(writes.len(), 1, "one send, one note of the try: {writes:?}");
    let set = writes[0]["set"].as_object().expect("an update with a set");
    assert!(
        !set.contains_key("pack_hash") && !set.contains_key("sent_at"),
        "the send must not book a delivery -- a pack that meets no door would be recorded \
         as delivered for ever (Gate F10): {writes:?}"
    );
    let retry = &writes[0]["set"]["retry"];
    assert_eq!(retry["hash"], hash, "the try names what was sent: {retry}");
    assert_eq!(retry["tries"], 1, "a new hash is the first try: {retry}");
    assert_eq!(
        writes[0]["where"],
        json!({"id": "sub:/g|entity:alex", "status": "active"}),
        "{writes:?}"
    );
}

/// The wall clock, in the seconds the push script stamps `retry.at` with.
fn epoch_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_secs_f64()
}

/// The hash the push computes for the stand-in row, read off its own pack.
fn fresh_hash(recorded_at: &str) -> String {
    let out = entities_round("", recorded_at);
    let brief = out
        .iter()
        .find(|m| m["header"]["route"] == "brief")
        .expect("a row that was never served is sent");
    brief["header"]["pack_hash"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `Some(new retry)` when the round sent a pack, `None` when it stayed silent
/// -- and silent means silent: no pack, no write, no `audit` row.
fn round_with(retry: Value) -> Option<Value> {
    let out = entities_round(&retry.to_string(), RECORDED);
    let briefs = out
        .iter()
        .filter(|m| m["header"]["route"] == "brief")
        .count();
    if briefs == 0 {
        assert!(
            out.is_empty(),
            "a round that sends nothing writes nothing: {out:?}"
        );
        return None;
    }
    assert_eq!(briefs, 1, "{out:?}");
    let writes = subscriber_writes(&out);
    assert_eq!(writes.len(), 1, "{out:?}");
    Some(writes[0]["set"]["retry"].clone())
}

fn secs(v: &Value) -> f64 {
    v.as_f64().unwrap_or(-1.0)
}

/// Review I-1: a pack nobody confirms goes again on the NEXT tick, and after
/// that each wait is twice the one before, up to the cap; a new hash starts
/// over, and a refused hash is parked until it moves. Probed one state at a
/// time, with `retry.at` set against the wall clock the script reads.
#[test]
fn an_unconfirmed_pack_is_retried_with_a_doubling_gap_under_a_cap() {
    if affinity_shipped().is_none() {
        return;
    }
    const CAP: f64 = 6.0 * 3600.0;
    let hash = fresh_hash(RECORDED);

    // The first resend goes on the next tick, whenever that is, and the
    // distance it measured becomes the unit: the next wait is twice it.
    let t = epoch_now();
    let second = round_with(json!({"hash": hash, "tries": 1, "at": t - 300.0, "gap": 0}))
        .expect("the first resend goes on the next tick");
    assert_eq!(second["tries"], 2, "{second}");
    assert!(
        (secs(&second["gap"]) - 600.0).abs() < 30.0,
        "the next wait is twice the tick the first resend measured: {second}"
    );

    // Not yet due: silent. Due: sent, and the wait doubles.
    let t = epoch_now();
    assert!(
        round_with(json!({"hash": hash, "tries": 2, "at": t - 300.0, "gap": 600.0})).is_none(),
        "a try inside its wait must stay silent"
    );
    let t = epoch_now();
    let third = round_with(json!({"hash": hash, "tries": 2, "at": t - 601.0, "gap": 600.0}))
        .expect("a try whose wait is over goes again");
    assert_eq!(third["tries"], 3, "{third}");
    assert_eq!(secs(&third["gap"]), 1200.0, "each wait doubles: {third}");

    // The cap: a wait never grows past it, and a capped try still goes.
    let t = epoch_now();
    let capped = round_with(json!({"hash": hash, "tries": 9, "at": t - CAP - 1.0, "gap": CAP}))
        .expect("a capped try still goes once its wait is over");
    assert_eq!(
        secs(&capped["gap"]),
        CAP,
        "the wait stops at the cap: {capped}"
    );

    // A new hash starts over: sent at once, first try, no wait.
    let t = epoch_now();
    let other = "cd".repeat(32);
    let reset = round_with(json!({"hash": other, "tries": 7, "at": t, "gap": CAP}))
        .expect("a changed pack goes at once, whatever the old one's wait");
    assert_eq!(reset["hash"], hash, "{reset}");
    assert_eq!(reset["tries"], 1, "a new hash resets the count: {reset}");

    // Parked by a refusal: silent while the hash stands, sent once it moves.
    assert!(
        round_with(json!({"hash": hash, "parked": "slot_unknown", "at": t})).is_none(),
        "a refused pack is parked until it changes"
    );
    assert!(
        round_with(json!({"hash": other, "parked": "slot_unknown", "at": t})).is_some(),
        "a parked hash that is no longer the pack's hash parks nothing"
    );
}

/// The lock of review I-1 on the script alone: twenty ticks, five minutes
/// apart, over a subscriber whose receipt never comes -- at most five packs,
/// and the second on the very next tick. Virtual time: each round gets its
/// `retry.at` shifted against the wall clock by the virtual age of the try.
#[test]
fn twenty_ticks_without_a_receipt_send_at_most_five_packs() {
    if affinity_shipped().is_none() {
        return;
    }
    const TICK: f64 = 300.0;
    let mut state = Value::String(String::new());
    let mut sent_at_v: f64 = 0.0;
    let mut sends: Vec<usize> = Vec::new();
    for k in 0..20usize {
        let tv = k as f64 * TICK;
        let wall = epoch_now();
        let retry = match state.as_object() {
            Some(o) => {
                let mut o = o.clone();
                o.insert("at".into(), json!(wall - (tv - sent_at_v)));
                Value::Object(o).to_string()
            }
            None => String::new(),
        };
        let out = entities_round(&retry, RECORDED);
        let writes = subscriber_writes(&out);
        if let Some(w) = writes.first() {
            sends.push(k);
            let next = w["set"]["retry"].clone();
            sent_at_v = tv + (secs(&next["at"]) - wall);
            state = next;
        }
    }
    assert!(
        sends.len() <= 5,
        "a subscriber that never confirms got {} packs in 20 ticks (ticks {sends:?})",
        sends.len()
    );
    assert_eq!(
        &sends[..2],
        &[0, 1],
        "the first pack goes at once and the first resend on the next tick: {sends:?}"
    );
}

/// The clean receipt writes the hash, and only onto ITS row, only while that
/// row is active and does not hold this hash already (review focus (1)-(3)).
#[test]
fn a_clean_ack_writes_only_its_own_active_row() {
    if affinity_shipped().is_none() {
        return;
    }
    let out = run_push(&ack_doc(
        json!({"pack_sub": "sub:/g|entity:alex", "pack_hash": "ab".repeat(32)}),
        "",
    ));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let msgs: Vec<Value> = meclaw_core::serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(msgs.len(), 1, "one receipt, one write: {msgs:?}");
    let args = store_args(&msgs[0]);
    assert_eq!(args["operation"], "update", "{args}");
    assert_eq!(args["table"], "subscribers", "{args}");
    assert_eq!(args["set"]["pack_hash"], "ab".repeat(32), "{args}");
    assert!(
        args["set"]["sent_at"]
            .as_str()
            .is_some_and(|s| s.ends_with('Z') && s.len() >= 20),
        "and when the receipt came: {args}"
    );
    assert_eq!(
        args["where"]["id"], "sub:/g|entity:alex",
        "only the row the receipt names -- two subscribers of one subject are two rows: {args}"
    );
    assert_eq!(
        args["where"]["status"], "active",
        "a receipt for a cancelled subscription revives nothing: {args}"
    );
    assert_eq!(
        args["where"]["pack_hash"],
        json!({"or_null": {"neq": "ab".repeat(32)}}),
        "and a repeated receipt moves nothing -- the pack fans out to every rim of a \
         generation, and each rim answers: {args}"
    );
}

/// A refused receipt (`slot_unknown`, `pack_empty`) books no delivery -- it
/// PARKS the hash it refused (review I-1): the same pack would meet the same
/// refusal on every tick, so nothing goes again until the pack changes.
#[test]
fn a_refused_ack_parks_its_hash() {
    if affinity_shipped().is_none() {
        return;
    }
    for code in ["slot_unknown", "pack_empty"] {
        let out = run_push(&ack_doc(
            json!({"pack_sub": "sub:/g|entity:alex", "pack_hash": "ab".repeat(32)}),
            code,
        ));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let msgs: Vec<Value> = meclaw_core::serde_json::from_slice(&out.stdout).expect("json");
        let writes = subscriber_writes(&msgs);
        assert_eq!(writes.len(), 1, "one refusal, one park: {msgs:?}");
        let set = writes[0]["set"].as_object().expect("an update with a set");
        assert!(
            !set.contains_key("pack_hash") && !set.contains_key("sent_at"),
            "a receipt with error_code {code} must book no delivery: {writes:?}"
        );
        assert_eq!(set["retry"]["hash"], "ab".repeat(32), "{writes:?}");
        assert_eq!(
            set["retry"]["parked"], code,
            "the park names the refusal: {writes:?}"
        );
        assert_eq!(
            writes[0]["where"],
            json!({"id": "sub:/g|entity:alex", "status": "active"}),
            "only the row the receipt names, and never a cancelled one: {writes:?}"
        );
    }
}

/// A receipt that names no row is not one this hive can book: dropped, and
/// said on stderr rather than in silence.
#[test]
fn an_ack_without_pack_sub_is_dropped() {
    if affinity_shipped().is_none() {
        return;
    }
    for context in [
        json!({}),
        json!({"pack_hash": "ab".repeat(32)}),
        json!({"pack_sub": "sub:/g|entity:alex"}),
    ] {
        let out = run_push(&ack_doc(context.clone(), ""));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "[]",
            "an anonymous receipt must write nothing: {context}"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("pack_ack dropped"),
            "and the drop is said, not swallowed: {context} -> {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

// ═══════════════════════════════════════════ (b) the member's receipt edge

/// The member graph as the router reads it, modifiers included.
fn member_table() -> EdgeTable {
    const HIVE: &str = "/m";
    let member = read_json(&repo(MEMBER));
    let hp: HiveParams =
        meclaw_core::serde_json::from_value(member["params"].clone()).expect("member params");
    let abs = |ep: &str| -> String {
        match ep {
            "." => HIVE.to_string(),
            other => format!("{HIVE}/{}", other.trim_start_matches("./")),
        }
    };
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(&spec.from)),
            to: Path::new(&abs(&spec.to)),
            condition: spec.condition.as_ref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("condition {src:?}: {e}"))
            }),
            modifier: spec.modifier.as_ref().map(|m| {
                meclaw_colony::cel_eval::parse_modifier(m)
                    .unwrap_or_else(|e| panic!("modifier {m:?}: {e:?}"))
            }),
            is_default: spec.is_default,
            lane: None,
            tap: false,
        });
    }
    t
}

fn pack_ack_headers(context: Map<String, Value>) -> Headers {
    let mut hop = Map::new();
    for (k, v) in [
        ("route", "pack_ack"),
        ("error_code", ""),
        ("pack_owner", "/m/affinity/brief"),
        ("pack_slots", "identity"),
        ("pack_unknown", ""),
    ] {
        hop.insert(k.to_string(), json!(v));
    }
    Headers::from_parts(context, hop)
}

/// Where one receipt goes from the member's `./assistants`, and what it
/// carries when it arrives.
fn route_receipt(table: &EdgeTable, context: Map<String, Value>) -> Vec<(String, Headers)> {
    apply_edges(
        table,
        &Path::new("/m/assistants"),
        &pack_ack_headers(context),
    )
    .into_iter()
    .map(|d| (d.target.as_str().to_string(), d.headers_out))
    .collect()
}

/// OR-KX-P1, OR-KX-66: every receipt goes home to `./affinity` as
/// `in_pack_ack` and nowhere else -- above the member nobody consumes one. A
/// receipt of this member's own push carries its row (`pack_sub`) and hash
/// across the edge; a receipt that names no row arrives with an EMPTY one, and
/// `./push` drops it with a line on stderr (`an_ack_without_pack_sub_is_dropped`)
/// rather than letting it leave the level.
#[test]
fn pack_ack_reaches_affinity_and_nothing_above() {
    if !repo(MEMBER).is_file() {
        return;
    }
    let table = member_table();

    let mut ctx = Map::new();
    ctx.insert(
        "pack_sub".into(),
        json!("sub:./assistants/scribe|entity:alex"),
    );
    ctx.insert("pack_hash".into(), json!("ab".repeat(32)));
    let named = route_receipt(&table, ctx);
    let targets: Vec<&str> = named.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        targets,
        vec!["/m/affinity"],
        "a receipt of this member's own push goes to its affinity and to nothing above \
         (OR-KX-P1): {targets:?}"
    );
    let out = &named[0].1;
    assert_eq!(
        out.hop.get("route"),
        Some(&json!("in_pack_ack")),
        "restamped onto affinity's receipt lane: {:?}",
        out.hop
    );
    assert_eq!(
        out.context.get("pack_sub"),
        Some(&json!("sub:./assistants/scribe|entity:alex")),
        "the row the receipt answers for survives the edge: {:?}",
        out.context
    );
    assert_eq!(
        out.context.get("pack_hash"),
        Some(&json!("ab".repeat(32))),
        "and the hash that was sent: {:?}",
        out.context
    );

    // The receipt that names no row. Until OR-KX-66 it took a complement edge
    // out of the level; now it takes the SAME edge home, and nothing reaches the
    // member's own path.
    let anonymous = route_receipt(&table, Map::new());
    let targets: Vec<&str> = anonymous.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        targets,
        vec!["/m/affinity"],
        "a receipt that names no row goes to the member's affinity too, and never out \
         at `/m` (GH #877, OR-KX-66): {targets:?}"
    );
    let out = &anonymous[0].1;
    assert_eq!(
        out.hop.get("route"),
        Some(&json!("in_pack_ack")),
        "on the same lane as a named one: {:?}",
        out.hop
    );
    assert_eq!(
        out.context.get("pack_sub"),
        Some(&json!("")),
        "and with an EMPTY row, which is what `./push` recognises and drops: {:?}",
        out.context
    );
    assert_eq!(
        out.context.get("pack_hash"),
        Some(&json!("")),
        "and an empty hash: {:?}",
        out.context
    );

    // An empty row is the same case as a missing one.
    let mut empty = Map::new();
    empty.insert("pack_sub".into(), json!(""));
    let targets: Vec<String> = route_receipt(&table, empty)
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert_eq!(
        targets,
        vec!["/m/affinity".to_string()],
        "a receipt with an empty row goes home as well: {targets:?}"
    );
}

/// GH #877, OR-KX-66: `pack_ack` ends inside the member, so no level above the
/// generation declares it or carries it out -- not the member, not the org
/// between member and colony, not the colony shell. Each of the three used to
/// emit it and draw an edge to its own path on it.
#[test]
fn no_container_level_emits_pack_ack_or_carries_it_out() {
    for tpl in ["member", "org", "meclaw-os"] {
        let path = repo(&format!("templates/{tpl}/config.json"));
        if !path.is_file() {
            return;
        }
        let v = read_json(&path);
        let emits = v["params"]["contract"]["emits"]
            .as_array()
            .unwrap_or_else(|| panic!("`{tpl}` declares what it emits"));
        assert!(
            !emits.iter().any(|l| l["route"] == "pack_ack"),
            "`{tpl}` still emits `pack_ack`: the receipt is booked at the member's \
             `./affinity` and nobody above consumes one (GH #877, OR-KX-66)"
        );
        let edges = v["params"]["graph"]["edges"]
            .as_array()
            .unwrap_or_else(|| panic!("`{tpl}` draws edges"));
        let out: Vec<&Value> = edges
            .iter()
            .filter(|e| e["to"] == ".")
            .filter(|e| {
                e["condition"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("hop.route == 'pack_ack'")
            })
            .collect();
        assert!(
            out.is_empty(),
            "`{tpl}` still carries `pack_ack` out to its own path: {out:?}"
        );
    }
}

// ═══════════════════════════════════════════════ (c) the seed and the gate

fn seed_rows() -> Vec<Value> {
    std::fs::read_to_string(repo(SEED))
        .expect("the subscribers seed")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| meclaw_core::serde_json::from_str::<Value>(l).expect("a seed line is JSON"))
        .filter(|v| v.get("schema").is_none())
        .collect()
}

/// F19: the seeded row carries the id `./gate` derives for the same subscriber
/// and subject, so a `subscribe` for it replaces the row instead of standing
/// beside it under a second id.
#[test]
fn the_seeded_row_has_the_derived_id() {
    if affinity_shipped().is_none() {
        return;
    }
    let rows = seed_rows();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    let cell_path = row["cell_path"].as_str().expect("a cell_path");
    let subject = row["subject"].as_str().expect("a subject");
    assert_eq!(
        row["id"].as_str(),
        Some(derived_id(cell_path, subject).as_str()),
        "the seed must spell the id the gate derives: {row}"
    );

    // And the gate, asked to subscribe that very subscriber to that subject,
    // writes under that id.
    let out = emit_all(
        &shipped_script(repo(GATE).to_str().expect("utf-8")),
        &json!({
            "target": "/affinity/gate",
            "header": {"hop": {"route": "in_propose"},
                       "context": {"actor": row["audience"], "subscriber": cell_path}},
            "ttl": 64,
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "s1",
                          "text": json!({"op": "subscribe", "subject": subject,
                                         "slots": ["identity"]}).to_string()}],
        }),
    );
    let inserted: Vec<Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == "astore")
        .map(store_args)
        .filter(|a| a["operation"] == "insert" && a["table"] == "subscribers")
        .collect();
    assert_eq!(inserted.len(), 1, "{out:?}");
    assert_eq!(inserted[0]["row"]["id"], row["id"], "{inserted:?}");
}

// ═════════════════════ (d) affinity alone: the receipt lane, driven by hand

const TICKER: &str = r#"
import sys, json
sys.stdout.write(json.dumps({
    "header": {"route": "ptick"},
    "messages": [{"origin": "user", "type": "text", "id": "t877", "text": "tick"}]}))
"#;

/// The subscribing side: the actor and the SUBSCRIBER ride on the hop so the
/// port edge can promote them into context (GH #288).
const WRITER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
try:
    a = json.loads(raw or "{}")
except Exception:
    a = {}
if not isinstance(a, dict):
    a = {}
sys.stdout.write(json.dumps({
    "header": {"route": "propose", "actor": str(a.get("actor") or "member:alex"),
               "subscriber": str(a.get("subscriber") or "")},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "w877",
                  "text": raw}]}))
"#;

/// A stand-in door: it answers every pack with a receipt whose `error_code` is
/// its own param -- the curator's receipt shape (K § 1), minus the curator.
const ACKER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
code = str((doc.get("params") or {}).get("error_code") or "")
sys.stdout.write(json.dumps({
    "header": {"route": "pack_ack", "error_code": code, "pack_owner": "",
               "pack_slots": "identity", "pack_unknown": ""},
    "messages": []}))
"#;

fn code_cell(script: &str, routes: &[&str], extra_hop: Value, extra_params: Value) -> Value {
    let mut hop = json!({"route": {"type": "string", "values": routes, "required": false}});
    if let Some(extra) = extra_hop.as_object() {
        for (k, v) in extra {
            hop[k] = v.clone();
        }
    }
    let mut params = json!({"runner": "python3", "script_inline": script,
                            "external_timeout_ms": 15000});
    if let Some(extra) = extra_params.as_object() {
        for (k, v) in extra {
            params[k] = v.clone();
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": params,
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the shipped affinity hive.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

const CLOCK_ID: &str = "01916f00-0000-7000-8000-000000000877";
const NEVER: &str = "0 0 0 1 1 *";
const SUBJECT: &str = "entity:alex";
const ACKER_SUB: &str = "/acker";

/// The harness edges every colony here shares: the write port, the manual
/// tick, the ack/error drain and a TAP on every answer (an edge table fans
/// out, so the tap sees each push without taking it from anybody).
fn harness_edges() -> Vec<Value> {
    vec![
        json!({"from": "./writer", "to": "./affinity",
               "condition": "has(hop.route) && hop.route == 'propose'",
               "modifier": {"set_hop": {"route": "'in_propose'"},
                            "set_context": {
                                "actor": "hop.actor",
                                "subscriber": "has(hop.subscriber) ? hop.subscriber : ''"}}}),
        json!({"from": "./ticker", "to": "./affinity/push",
               "condition": "has(hop.route) && hop.route == 'ptick'",
               "modifier": {"set_context": {"affinity_origin": "'push'",
                                            "aff_phase": "''", "aff_carry": "''"}}}),
        json!({"from": "./affinity", "to": "/sink",
               "condition": "has(hop.route) && (hop.route == 'ack' || hop.route == 'error')"}),
        json!({"from": "./affinity", "to": "/pushsink",
               "condition": "has(hop.route) && hop.route == 'answer' && hop.subscriber != ''"}),
    ]
}

/// The affinity hive with a stand-in door behind it: the pack is delivered to
/// `./acker` in the form the builder's v-lane delivers it (restamped
/// `in_pack`, `pack_sub`/`pack_hash` promoted into context), and the receipt
/// goes home over the member's own edge.
fn build_affinity_tree(td: &tempfile::TempDir, affinity: &std::path::Path, ack_code: &str) {
    let root = td.path();
    std::fs::write(root.join(".env"), "").unwrap();
    let mut edges = harness_edges();
    edges.push(json!({
        "from": "./affinity", "to": "./acker",
        "condition": format!(
            "has(hop.route) && hop.route == 'answer' && hop.subscriber == '{ACKER_SUB}'"),
        "modifier": {"set_hop": {"route": "'in_pack'"},
                     "set_context": {
                         "pack_sub": "has(hop.pack_sub) ? hop.pack_sub : ''",
                         "pack_hash": "has(hop.pack_hash) ? hop.pack_hash : ''"}}}));
    let mut home = member_ack_edge();
    home["from"] = json!("./acker");
    edges.push(home);
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    write(
        root,
        "main/writer/config.json",
        &code_cell(
            WRITER,
            &["propose"],
            json!({"actor": {"type": "string", "required": false},
                   "subscriber": {"type": "string", "required": false}}),
            json!({}),
        ),
    );
    write(
        root,
        "main/ticker/config.json",
        &code_cell(TICKER, &["ptick"], json!({}), json!({})),
    );
    write(
        root,
        "main/acker/config.json",
        &code_cell(
            ACKER,
            &["pack_ack"],
            json!({"error_code": {"type": "string", "required": false},
                   "pack_owner": {"type": "string", "required": false},
                   "pack_slots": {"type": "string", "required": false},
                   "pack_unknown": {"type": "string", "required": false}}),
            json!({"error_code": ack_code}),
        ),
    );
    copy_cells(affinity, &root.join("main/affinity"));
    patch(root, "main/affinity/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(CLOCK_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
}

struct Sinks {
    sink: mpsc::Receiver<Message>,
    push: mpsc::Receiver<Message>,
}

async fn boot_with(
    td: &tempfile::TempDir,
    factories: Vec<(String, Arc<dyn CellFactory>)>,
) -> (ColonyHandle, Sinks) {
    let h = ColonyHandle::new_with_factories_at(td, factories.clone());
    let (sink_tx, sink) = mpsc::channel::<Message>(256);
    let (push_tx, push) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/pushsink"), move || {
        CaptureCell::new(push_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, Sinks { sink, push })
}

fn base_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
    ]
}

fn to(target: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// The next message on `rx` whose `hop.route` matches; 30 s is the failure
/// marker convention.
async fn recv_route(rx: &mut mpsc::Receiver<Message>, route: &str) -> Message {
    let mut seen: Vec<String> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Ok(Some(m)) = tokio::time::timeout(left, rx.recv()).await else {
            panic!("no `{route}` arrived within 30s; saw {seen:?}");
        };
        if hop_of(&m, "route") == route {
            return m;
        }
        seen.push(hop_of(&m, "route"));
    }
}

async fn subscribe(h: &ColonyHandle, sink: &mut mpsc::Receiver<Message>, op: Value) {
    h.send(to(
        "/writer",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;
    let ack = recv_route(sink, "ack").await;
    let text = match &ack.body {
        Body::Inline(v) => v["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Body::Blob(_) => String::new(),
    };
    assert!(
        text.contains("accepted"),
        "the subscription has to exist before a tick can serve it: {text}"
    );
}

/// Rows of one SQL statement against a `cell.db`, every column as text.
fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    if !db.exists() {
        return Vec::new();
    }
    let Ok(conn) = rusqlite::Connection::open(db) else {
        return Vec::new();
    };
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| {
                r.get::<_, Option<String>>(i)
                    .unwrap_or_default()
                    .unwrap_or_default()
            })
            .collect::<Vec<String>>())
    })
    .map(|it| it.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// `(id, status, pack_hash, sent_at, slots)` of every subscriber row -- read in
/// the affinity store's own `cell.db`, the receiver of every write.
fn subscriber_rows(td: &tempfile::TempDir) -> Vec<Vec<String>> {
    rows(
        &td.path().join("main/affinity/store/cell.db"),
        "SELECT id, status, pack_hash, sent_at, slots FROM subscribers ORDER BY id",
    )
}

fn row_of(td: &tempfile::TempDir, id: &str) -> Option<Vec<String>> {
    subscriber_rows(td)
        .into_iter()
        .find(|r| r[0] == id && r[1] == "active")
}

/// `(from_path, to_path, headers)` of every logged message.
fn log_rows(td: &tempfile::TempDir) -> Vec<(String, String, Value)> {
    rows(
        &td.path().join("colony.db"),
        "SELECT from_path, to_path, headers FROM message_log ORDER BY created_at",
    )
    .into_iter()
    .map(|r| {
        let headers = meclaw_core::serde_json::from_str::<Value>(&r[2]).unwrap_or(Value::Null);
        (r[0].clone(), r[1].clone(), headers)
    })
    .collect()
}

fn logged(td: &tempfile::TempDir, to_path: &str, route: &str) -> Vec<Value> {
    log_rows(td)
        .into_iter()
        .filter(|(_, to, h)| to == to_path && h["hop"]["route"] == route)
        .map(|(_, _, h)| h)
        .collect()
}

/// Poll `probe` until it yields, or fail after 30 s naming `what`.
async fn eventually<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(v) = probe() {
            return v;
        }
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Hold `probe` true for `window`, failing the moment it is not.
async fn stays(what: &str, window: Duration, mut probe: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + window;
    while tokio::time::Instant::now() < deadline {
        assert!(probe(), "{what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The `retry` note of the ACTIVE row under `id`, parsed (`Null` while none).
fn retry_of(td: &tempfile::TempDir, id: &str) -> Value {
    rows(
        &td.path().join("main/affinity/store/cell.db"),
        &format!("SELECT retry FROM subscribers WHERE id = '{id}' AND status = 'active'"),
    )
    .first()
    .and_then(|r| meclaw_core::serde_json::from_str::<Value>(&r[0]).ok())
    .unwrap_or(Value::Null)
}

/// One manual tick, returned only once it has RUN -- its entities read came
/// back to `./push`, which is where the push decides.
async fn tick_and_wait(h: &ColonyHandle, td: &tempfile::TempDir) {
    let before = logged_entities_echoes(td);
    h.send(to("/ticker", "tick")).await;
    eventually("a tick never ran", || {
        (logged_entities_echoes(td) > before).then_some(())
    })
    .await;
}

/// Task 2, since review I-1: a refused receipt leaves the hash empty -- no
/// delivery is booked -- and PARKS the refused hash, so the next tick over the
/// same record is silent instead of meeting the same refusal again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_errored_ack_parks_the_pack_until_it_changes() {
    let Some(affinity) = affinity_shipped() else {
        return;
    };
    if !repo(MEMBER).is_file() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_affinity_tree(&td, &affinity, "slot_unknown");
    let (h, mut s) = boot_with(&td, base_factories()).await;
    let id = derived_id(ACKER_SUB, SUBJECT);

    subscribe(
        &h,
        &mut s.sink,
        json!({"op": "subscribe", "subject": SUBJECT, "slots": ["identity"],
               "subscriber": ACKER_SUB}),
    )
    .await;

    h.send(to("/ticker", "tick")).await;
    let first = recv_route(&mut s.push, "answer").await;
    assert_eq!(hop_of(&first, "pack_sub"), id, "{:?}", first.headers.hop);
    // The refused receipt reached the push cell and parked its hash on the
    // row -- read in the store's own `cell.db`, the receiver of the write.
    let parked = eventually("the refused receipt never parked the row", || {
        let r = retry_of(&td, &id);
        (r["parked"] == "slot_unknown").then_some(r)
    })
    .await;
    assert_eq!(
        parked["hash"].as_str(),
        Some(hop_of(&first, "pack_hash").as_str()),
        "the park names the hash that was refused: {parked}"
    );
    assert!(
        row_of(&td, &id).is_some_and(|r| r[2].is_empty() && r[3].is_empty()),
        "a refused receipt booked a delivery: {:?}",
        row_of(&td, &id)
    );

    // The next tick runs over the same record and sends nothing.
    tick_and_wait(&h, &td).await;
    let quiet = tokio::time::timeout(Duration::from_secs(2), s.push.recv()).await;
    assert!(
        quiet.is_err(),
        "a parked pack went again: {:?}",
        quiet.map(|m| m.map(|m| m.headers.hop.clone()))
    );

    h.shutdown().await;
}

/// The lock of review I-1, at the receiver: a subscriber whose receipt never
/// comes (no door behind it -- the tap is the only place its pack lands) gets
/// at most five packs in twenty ticks, and the second on the very next tick.
/// Before the fix round every tick sent the same pack again, without end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subscriber_that_never_confirms_gets_at_most_five_packs_in_twenty_ticks() {
    let Some(affinity) = affinity_shipped() else {
        return;
    };
    if !repo(MEMBER).is_file() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_affinity_tree(&td, &affinity, "");
    let (h, mut s) = boot_with(&td, base_factories()).await;
    let nobody = "/nobody";

    subscribe(
        &h,
        &mut s.sink,
        json!({"op": "subscribe", "subject": SUBJECT, "slots": ["identity"],
               "subscriber": nobody}),
    )
    .await;

    // Tick 0 sends, and tick 1 -- the next one -- sends the first resend.
    let mut packs = 0usize;
    for _ in 0..2 {
        tick_and_wait(&h, &td).await;
        let m = recv_route(&mut s.push, "answer").await;
        assert_eq!(hop_of(&m, "subscriber"), nobody, "{:?}", m.headers.hop);
        packs += 1;
    }
    // Eighteen more ticks at a steady spacing -- the tick the wait doubles
    // from -- then whatever is still under way reaches the tap.
    for _ in 2..20usize {
        tick_and_wait(&h, &td).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    while let Ok(Some(m)) = tokio::time::timeout(Duration::from_secs(2), s.push.recv()).await {
        if hop_of(&m, "route") == "answer" {
            assert_eq!(hop_of(&m, "subscriber"), nobody, "{:?}", m.headers.hop);
            packs += 1;
        }
    }
    assert!(
        packs <= 5,
        "a subscriber that never confirms got {packs} packs in 20 ticks"
    );

    h.shutdown().await;
}

/// Task 4 (F19): a second `subscribe` of the same subscriber to the same
/// subject supersedes the first BY STATUS -- the hive never deletes (README,
/// No-Delete) -- so there is exactly one ACTIVE row under the id, carrying what
/// the second request asked for, and the first stands beside it inactive. The
/// receipt books only the active one (`a_clean_ack_writes_only_its_own_active_row`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_subscribe_keeps_one_active_row() {
    let Some(affinity) = affinity_shipped() else {
        return;
    };
    if !repo(MEMBER).is_file() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_affinity_tree(&td, &affinity, "");
    let (h, mut s) = boot_with(&td, base_factories()).await;
    let id = derived_id(ACKER_SUB, SUBJECT);

    for slots in [json!(["identity"]), json!(["identity", "channel"])] {
        subscribe(
            &h,
            &mut s.sink,
            json!({"op": "subscribe", "subject": SUBJECT, "slots": slots,
                   "subscriber": ACKER_SUB}),
        )
        .await;
    }
    let mine = eventually("the second subscribe never landed", || {
        let mine: Vec<Vec<String>> = subscriber_rows(&td)
            .into_iter()
            .filter(|r| r[0] == id)
            .collect();
        (mine.len() == 2).then_some(mine)
    })
    .await;
    let active: Vec<&Vec<String>> = mine.iter().filter(|r| r[1] == "active").collect();
    assert_eq!(
        active.len(),
        1,
        "one subscription is one ACTIVE row under its id, however often it was subscribed: \
         {mine:?}"
    );
    assert!(
        active[0][4].contains("channel"),
        "and it is the one the second request asked for: {mine:?}"
    );
    assert!(
        mine.iter()
            .any(|r| r[1] == "inactive" && !r[4].contains("channel")),
        "the first stays beside it, superseded by status and not deleted: {mine:?}"
    );

    h.shutdown().await;
}

// ═════════════════════ (e) the lock: a whole generation behind the member

/// The level `grow_level assistant` renders at the member, with or without the
/// identity door -- the recipe's own output, never a copy of it.
fn rendered_declaration(subscribe: bool) -> Value {
    let reference = read_json(&repo(ASSISTANT_EXAMPLE));
    let mut params = json!({
        "scope": SCOPE, "level": "assistant", "name": NAME,
        "template": reference["diff"]["add_nodes"][0]["template"].clone(),
        "ctx": reference["ctx"].clone(),
    });
    if subscribe {
        params["subscribe"] = json!(true);
    }
    let out = meclaw_testing::emit_one(
        &shipped_script(repo(RECIPES).to_str().expect("utf-8")),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                "text": json!({"recipe": "grow_level", "request": "…",
                               "params": params}).to_string()}],
        }),
    );
    assert!(out["header"]["error_code"].is_null(), "{out}");
    let decls = out["manifest"].as_array().expect("a manifest").clone();
    assert_eq!(decls.len(), 1, "a level is ONE declaration");
    decls[0].clone()
}

/// Both endpoints of every edge resolved against the declaration's scope.
fn resolved_edges(subscribe: bool) -> Vec<Value> {
    let decl = rendered_declaration(subscribe);
    let root = meclaw_core::Path::new(decl["scope"].as_str().expect("a scope"));
    decl["diff"]["add_edges"]
        .as_array()
        .expect("add_edges")
        .iter()
        .map(|e| {
            let mut e = e.clone();
            for side in ["from", "to"] {
                let raw = e[side].as_str().expect("an endpoint").to_string();
                e[side] = json!(meclaw_core::Path::resolve(&root, &raw).as_str());
            }
            e
        })
        .collect()
}

/// The identity door, re-spelled relative to the member this colony stands in
/// for: whatever `subscribe` adds to the level and nothing else.
fn identity_door() -> Vec<Value> {
    let base = resolved_edges(false);
    resolved_edges(true)
        .into_iter()
        .filter(|e| !base.contains(e))
        .map(|mut e| {
            for side in ["from", "to"] {
                let abs = e[side].as_str().expect("an endpoint").to_string();
                e[side] = json!(
                    abs.strip_prefix(&format!("{SCOPE}/"))
                        .map(|rest| format!("./{rest}"))
                        .unwrap_or_else(|| ".".to_string())
                );
            }
            e
        })
        .collect()
}

/// The subscriber the door's guard names -- the generation -- read out of the
/// rendered condition, so the row this file writes names exactly that string.
fn subscriber_literal() -> String {
    let door = identity_door();
    let cond = door
        .iter()
        .find_map(|e| {
            e["condition"]
                .as_str()
                .filter(|c| c.contains("hop.route == 'answer'"))
        })
        .expect("the door has a push edge")
        .to_string();
    let key = "hop.subscriber == '";
    let at = cond.find(key).expect("the push edge names its subscriber") + key.len();
    cond[at..cond[at..].find('\'').map(|n| at + n).expect("a literal")].to_string()
}

/// The inert factory of `gh302`/`gh473`: the tool surface of a real generation
/// would reach outward the moment it was spawned, and no pack touches it.
struct InertCellFactory;

impl CellFactory for InertCellFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn is_lazy(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        _path: Path,
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
        let wake: WakeFn = Box::new(|mut rx: mpsc::Receiver<Message>| {
            tokio::spawn(async move { while rx.recv().await.is_some() {} });
            let (stop_tx, _stop_rx) = oneshot::channel::<()>();
            let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
            (stop_tx, death_ack_rx)
        });
        let respawn: RespawnFn = Box::new(move || {
            let (tx, mut rx) = mpsc::channel::<Message>(capacity);
            let (peace_tx, peace_rx) = oneshot::channel::<()>();
            let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
            let join = tokio::spawn(async move {
                let _peace_keep = peace_tx;
                while rx.recv().await.is_some() {}
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
    out
}

/// The real cell types for everything a pack touches, an inert one for the
/// rest of the generation (its tool surface).
fn generation_factories(root: &std::path::Path) -> Vec<(String, Arc<dyn CellFactory>)> {
    let mut fs = base_factories();
    let known: BTreeSet<String> = fs.iter().map(|(n, _)| n.clone()).collect();
    for t in cell_types_in(root) {
        if !known.contains(&t) && t != "hive" && t != "ref" {
            fs.push((t, Arc::new(InertCellFactory) as Arc<dyn CellFactory>));
        }
    }
    fs
}

/// Every timer of the generation stopped, every `llm` cell pointed at an
/// address that refuses: no pack costs an inference, and nothing here may
/// reach a provider.
fn quiesce(dir: &std::path::Path, counter: &mut u32) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            quiesce(&p, counter);
            continue;
        }
        if p.file_name().and_then(|n| n.to_str()) != Some("config.json") {
            continue;
        }
        let mut v = read_json(&p);
        match v["cell"]["type"].as_str().unwrap_or_default() {
            "timer" => {
                let Some(schedules) = v["params"]["schedules"].as_array_mut() else {
                    continue;
                };
                for sched in schedules.iter_mut() {
                    *counter += 1;
                    sched["schedule_id"] = json!(format!(
                        "0190a3f2-0000-7000-8000-{:012}",
                        877_000 + *counter
                    ));
                    sched["cron"] = json!(NEVER);
                }
            }
            "llm" => {
                v["params"]["base_url"] = json!("http://127.0.0.1:1/v1");
                v["params"]["model"] = json!("gpt-4o-mock");
            }
            _ => continue,
        }
        std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
    }
}

/// The member stand-in: its affinity, its assistants container with ONE grown
/// generation in it, the member's own receipt edge -- and the identity door
/// only when `door_at_boot`. Returns the generation's path relative to `main/`.
fn build_generation_tree(
    td: &tempfile::TempDir,
    affinity: &std::path::Path,
    assistant: &std::path::Path,
    door_at_boot: bool,
) -> String {
    let root = td.path();
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").unwrap();
    let mut edges = harness_edges();
    edges.push(member_ack_edge());
    if door_at_boot {
        edges.extend(identity_door());
    }
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    write(
        root,
        "main/writer/config.json",
        &code_cell(
            WRITER,
            &["propose"],
            json!({"actor": {"type": "string", "required": false},
                   "subscriber": {"type": "string", "required": false}}),
            json!({}),
        ),
    );
    write(
        root,
        "main/ticker/config.json",
        &code_cell(TICKER, &["ptick"], json!({}), json!({})),
    );
    write(
        root,
        "main/assistants/config.json",
        &json!({"cell": {"type": "hive"}}),
    );
    copy_cells(affinity, &root.join("main/affinity"));
    patch(root, "main/affinity/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(CLOCK_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    let target = subscriber_literal();
    let rel = format!("main/{}", target.trim_start_matches("./"));
    copy_cells(assistant, &root.join(&rel));
    quiesce(&root.join(&rel), &mut 0);
    rel
}

/// One body at the mutation door, the one `POST /colony/mutations` hands it.
async fn mutate(h: &ColonyHandle, payload: Value) -> MutationOutcome {
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
        .expect("send mutation");
    ack_rx.await.expect("mutation ack")
}

/// The slot paths the curator in front of `rim` holds as `pack` -- its ledger's
/// `slots` table (K § 1), the receiver of every identity pack.
fn curator_pack_slots(td: &tempfile::TempDir, rel: &str, rim: &str) -> Vec<String> {
    rows(
        &td.path().join(rel).join(rim).join("curator/ledger/cell.db"),
        "SELECT path FROM slots WHERE owner = 'pack' ORDER BY path",
    )
    .into_iter()
    .map(|r| r[0].clone())
    .collect()
}

fn holds_identity(slots: &[String]) -> bool {
    slots
        .iter()
        .any(|p| p == "identity" || p.starts_with("identity."))
}

/// The absolute path of `rim`'s curator hive, as the colony logs it.
fn curator_path(rel: &str, rim: &str) -> String {
    format!("/{}/{rim}/curator", rel.trim_start_matches("main/"))
}

fn now_utc() -> String {
    // The same second-resolution RFC 3339 form `./push` stamps `sent_at` in,
    // so the two compare as strings.
    let out = std::process::Command::new("python3")
        .args([
            "-c",
            "import datetime;print(datetime.datetime.now(datetime.timezone.utc)\
             .strftime('%Y-%m-%dT%H:%M:%SZ'))",
        ])
        .output()
        .expect("python3");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// THE LOCK (GH #877, Variante A). A pack sent before the generation has its
/// door is not booked; drawing the door and ticking again delivers it EXACTLY
/// once, the clean receipt books it with a `sent_at` after the door, and a
/// third tick over the same record is silent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pack_without_a_door_is_sent_again() {
    let (Some(affinity), Some(assistant)) = (affinity_shipped(), generation_shipped()) else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    let rel = build_generation_tree(&td, &affinity, &assistant, false);
    let (h, mut s) = boot_with(&td, generation_factories(&td.path().join("main"))).await;
    let subscriber = subscriber_literal();
    let id = derived_id(&subscriber, SUBJECT);
    let talky = curator_path(&rel, "talky");

    subscribe(
        &h,
        &mut s.sink,
        json!({"op": "subscribe", "subject": SUBJECT, "slots": ["identity"],
               "subscriber": subscriber}),
    )
    .await;

    // 1. The tick before the door. The pack LEAVES affinity (caught on the
    //    tap), meets no edge into the generation, and the row stays unbooked.
    h.send(to("/ticker", "tick")).await;
    let early = recv_route(&mut s.push, "answer").await;
    assert_eq!(
        hop_of(&early, "subscriber"),
        subscriber,
        "{:?}",
        early.headers.hop
    );
    assert_eq!(hop_of(&early, "pack_sub"), id, "{:?}", early.headers.hop);
    stays(
        "a pack that met no door was booked as delivered (Gate F10)",
        Duration::from_secs(2),
        || row_of(&td, &id).is_some_and(|r| r[2].is_empty() && r[3].is_empty()),
    )
    .await;
    assert!(
        logged(&td, &talky, "in_pack").is_empty(),
        "no door, no delivery: the talky curator got a pack before it had a door"
    );

    // 2. The door. The whole second passes so that a `sent_at` stamped by the
    //    first tick could never pass for one stamped after the door.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let door_at = now_utc();
    let outcome = mutate(
        &h,
        json!({"scope": "/", "diff": {"add_edges": identity_door()}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the rendered identity door must commit: {outcome:?}"
    );

    // 3. The next tick finds the row unbooked -- a change -- and delivers.
    //    Its send is taken off the tap here, so that step 4 hears only what
    //    the third tick says.
    h.send(to("/ticker", "tick")).await;
    let resent = recv_route(&mut s.push, "answer").await;
    assert_eq!(hop_of(&resent, "pack_sub"), id, "{:?}", resent.headers.hop);
    let slots = eventually("the pack never reached the talky curator's ledger", || {
        let slots = curator_pack_slots(&td, &rel, "talky");
        holds_identity(&slots).then_some(slots)
    })
    .await;
    assert!(holds_identity(&slots), "{slots:?}");
    let booked = eventually("the clean receipt never booked the row", || {
        row_of(&td, &id).filter(|r| r[2].len() == 64)
    })
    .await;
    assert!(
        booked[3].as_str() >= door_at.as_str(),
        "sent_at {} must lie after the door ({door_at}) -- it is the moment the receipt \
         came back, not the moment of a send nobody received",
        booked[3]
    );
    let receipts = logged(&td, "/affinity", "in_pack_ack");
    assert!(
        receipts
            .iter()
            .any(|h| h["hop"]["error_code"] == "" && h["context"]["pack_sub"] == id.as_str()),
        "a clean receipt naming the row reached affinity: {receipts:?}"
    );
    assert!(
        receipts.iter().all(|h| h["hop"]["error_code"] == ""),
        "and no rim refused the pack: {receipts:?}"
    );
    assert_eq!(
        logged(&td, &talky, "in_pack").len(),
        1,
        "exactly one pack reached the talky curator"
    );

    // 4. A third tick over the same record: it runs (its entities read comes
    //    back to ./push) and sends nothing.
    let reads_before = logged_entities_echoes(&td);
    h.send(to("/ticker", "tick")).await;
    eventually("the third tick never ran", || {
        (logged_entities_echoes(&td) > reads_before).then_some(())
    })
    .await;
    let quiet = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let m = s.push.recv().await?;
            if hop_of(&m, "route") == "answer" {
                return Some(m);
            }
        }
    })
    .await;
    assert!(
        !matches!(quiet, Ok(Some(_))),
        "a tick over a booked record must send nothing: {:?}",
        quiet.ok().flatten().map(|m| m.headers.hop.clone())
    );
    assert_eq!(
        logged(&td, &talky, "in_pack").len(),
        1,
        "still exactly one pack at the talky curator after the third tick"
    );

    h.shutdown().await;
}

/// How many times the store's `entities` answer has come back to `./push` --
/// one per tick that found an active subscriber.
fn logged_entities_echoes(td: &tempfile::TempDir) -> usize {
    log_rows(td)
        .into_iter()
        .filter(|(from, to, h)| {
            from == "/affinity/store"
                && to == "/affinity/push"
                && h["context"]["aff_phase"] == "entities"
        })
        .count()
}

/// The talky-chat pack (Leser-2 finding): the builder draws the identity door
/// for EVERY rim the generation vouches for, so the typed channel's voice knows
/// who it is as well. One tick delivers to all three curators, each rim's
/// receipt comes home carrying the row, and the talky-chat curator got exactly
/// one pack.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn talky_chat_gets_its_identity_pack() {
    let (Some(affinity), Some(assistant)) = (affinity_shipped(), generation_shipped()) else {
        return;
    };
    let td = tempfile::TempDir::new().unwrap();
    let rel = build_generation_tree(&td, &affinity, &assistant, true);
    let (h, mut s) = boot_with(&td, generation_factories(&td.path().join("main"))).await;
    let subscriber = subscriber_literal();
    let id = derived_id(&subscriber, SUBJECT);

    subscribe(
        &h,
        &mut s.sink,
        json!({"op": "subscribe", "subject": SUBJECT, "slots": ["identity"],
               "subscriber": subscriber}),
    )
    .await;
    h.send(to("/ticker", "tick")).await;

    for rim in ["talky", "talky-chat", "cogny"] {
        let slots = eventually(
            &format!("the pack never reached the {rim} curator's ledger"),
            || {
                let slots = curator_pack_slots(&td, &rel, rim);
                holds_identity(&slots).then_some(slots)
            },
        )
        .await;
        assert!(holds_identity(&slots), "{rim}: {slots:?}");
    }
    let chat_rim = format!("/{}/talky-chat", rel.trim_start_matches("main/"));
    let home = eventually("the talky-chat receipt never reached the container", || {
        log_rows(&td)
            .into_iter()
            .find(|(from, to, h)| {
                from == &chat_rim && to == "/assistants" && h["hop"]["route"] == "pack_ack"
            })
            .map(|(_, _, h)| h)
    })
    .await;
    assert_eq!(home["hop"]["error_code"], "", "{home}");
    assert_eq!(
        home["context"]["pack_sub"],
        id.as_str(),
        "the row survives every station of the way back -- v-lane, curator, rim: {home}"
    );
    eventually("the receipts never booked the row", || {
        row_of(&td, &id).filter(|r| r[2].len() == 64)
    })
    .await;
    assert_eq!(
        logged(&td, &curator_path(&rel, "talky-chat"), "in_pack").len(),
        1,
        "exactly one pack reached the talky-chat curator"
    );

    h.shutdown().await;
}

// ══════════════════════ (f) the registry announces the curator summarizers

/// OR-KX-P5: a generation grown in a tree with a registry announces its three
/// curator summarizers beside its brains -- each with the start value its
/// template cell is born on and a byte copy of that cell's need -- and the
/// registry's push reaches each of them through the rim it stands behind.
#[test]
fn the_curator_summarizers_are_announced_with_their_need() {
    if generation_shipped().is_none() || shipped("curator", &["summarizer/config.json"]).is_none() {
        return;
    }
    let reference = read_json(&repo(ASSISTANT_EXAMPLE));
    let ctx = reference["ctx"].clone();
    let wish = json!({"recipe": "grow_level", "request": "grow an assistant",
                      "params": {"scope": SCOPE, "level": "assistant", "name": NAME,
                                 "template": reference["diff"]["add_nodes"][0]["template"],
                                 "ctx": ctx}});
    let out = emit_all(
        &shipped_script(repo(RECIPES).to_str().expect("utf-8")),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "params": {"model_registry_scope": REGISTRY_SCOPE},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": wish.to_string()}],
        }),
    );
    let answer = out
        .iter()
        .find(|m| m["header"]["operation"] == "recipe")
        .expect("the recipe answered");
    let road = &answer["manifest"][1]["diff"]["add_edges"];
    let edges = road.as_array().expect("the registry road");
    let announce = edges
        .iter()
        .find(|e| {
            e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("'mutation_committed'"))
        })
        .expect("an announcement edge");
    let lit = announce["modifier"]["set_context"]["model_announced"]
        .as_str()
        .unwrap_or_default();
    let brains: Vec<Value> =
        meclaw_core::serde_json::from_str(lit.trim_matches('\'')).expect("a JSON literal");
    let need = read_json(&templates_root().join("curator/summarizer/config.json"))["params"]
        ["requirement"]
        .clone();
    assert!(
        need.as_str().is_some_and(|n| !n.trim().is_empty()),
        "the summarizer states its need (OR-KX-G11): {need}"
    );
    let generation = format!("{SCOPE}/assistants/{NAME}");
    for rim in ["talky", "talky-chat", "cogny"] {
        let path = format!("{generation}/{rim}/curator/summarizer");
        let entry = brains
            .iter()
            .find(|b| b["cell_path"] == path.as_str())
            .unwrap_or_else(|| panic!("{path} is not announced: {brains:?}"));
        assert_eq!(
            entry["start_model"], ctx["model"],
            "{path} is announced on the start value its template cell is born on \
             (`${{ctx.model}}`): {entry}"
        );
        assert_eq!(
            entry["requirement"], need,
            "and with a byte copy of its need, which the registry translates: {entry}"
        );
        let rel = format!("./acme/members/alex/assistants/{NAME}/{rim}");
        assert!(
            edges.iter().any(|e| e["from"] == "."
                && e["to"] == rel.as_str()
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("hop.subscriber == '{path}'")))),
            "the registry's push to {path} rides onto the rim it stands behind ({rel}), \
             whose `in_model` door hands it to the curator: {edges:?}"
        );
    }
}
