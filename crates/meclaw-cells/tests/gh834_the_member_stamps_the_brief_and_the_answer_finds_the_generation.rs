//! GH #834, lock 2 -- the member stamps the brief, and the answer finds the
//! generation that asked. On a BOOTED colony, because a wiring defect is exactly
//! what no script pin can see: an edge that is missing, guarded on the wrong key
//! or firing twice leaves every script-level assertion green.
//!
//! The road under test, every hop of it shipped:
//!
//! ```text
//! <gen>/talky/collector --brief--> <gen>/talky (rim)
//!   --v-lane brief--> assistants (container)                recipe
//!   --edge A: in_brief, turn_id/asker/brief_caller--> affinity   member
//!   affinity answers (or fails) under the call id
//!   --edge B: in_briefing (answer || error, brief_caller inside)--> assistants
//!   --v-lane in_briefing, guarded on context.assistant--> <gen>/talky
//!   --> <gen>/talky/collector: the leg parks, the turn opens, the brain is called
//! ```
//!
//! Measured before it (member@1.9.3): nothing in a turn asked affinity at all, and
//! the member's two exits `./affinity -> .` (`answer && hop.subscriber == ''`,
//! `error`) would have carried EVERY internal brief answer up to the OS root,
//! where it dead-letters as `hive_no_route` -- the leak `no_brief_answer_reaches_
//! the_os_level` pins shut (OR-AG-24).
//!
//! What is booted: the SHIPPED `member`, `affinity` (with its store and seeds),
//! `talky` and `collector` hives, two generations of the shipped `assistant`, and
//! the container edges `examples/organism/grow-assistant.json` carries (the
//! recipe's golden, gh466). BOTH surfaces of a generation are the real talky with
//! the real collector -- `./talky` and `./talky-chat` -- because the way home
//! differs between them: the answer to the chat surface is told apart from the
//! spoken one by a reply-to token (`context.brief_surface`, stamped on the
//! surface's `brief` v-lane, moved onto the hop by the member's edge B, read by
//! the one regular `in_briefing` door beside talky's `default`; OR-AG.M3.4). A
//! broken link in that chain sends the chat's brief into the spoken surface and
//! leaves the chat turn waiting without a deadline (OR-AG-12) -- measurable only
//! with both surfaces standing. Doubled are only the cells this road never
//! reaches (and the `ref`s a filesystem boot cannot resolve), plus the brains: a
//! `code` cell that reports what the prompt carried, so the lock reads the prompt
//! AT THE BRAIN rather than trusting that the collector emitted it.
//!
//! Guarded like every template-reading test (GH #49).

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const ALPHA: &str = "alpha";
const BETA: &str = "beta";
const COUNTERPART: &str = "peer:north";
/// The two surfaces of a generation (`templates/assistant/config.json`), both of
/// which carry the knob (`both_surfaces_of_the_assistant_set_brief_slots`).
const TALKY: &str = "talky";
const TALKY_CHAT: &str = "talky-chat";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Every template this file boots, or `None` in a tree that did not carry them.
fn shipped() -> bool {
    ["member", "assistant", "talky", "collector", "affinity"]
        .iter()
        .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
        && repo("examples/organism/grow-assistant.json").is_file()
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v = read_json(&p);
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// The template's cells: every `config.json`, and the seed files of a store.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
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
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

fn append_rows(root: &std::path::Path, rel: &str, rows: &[Value]) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(root.join(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"));
    for r in rows {
        writeln!(f, "{r}").expect("append a seed row");
    }
}

// ══════════════════════════════════════════════════════════════ the doubles

const INERT: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// Emits exactly what the test hands it: `{"header": ..., "messages": ...}` in
/// the text of the one turn. The CONTEXT is the injected message's own and rides
/// on (context is persistent along a trace), so the test sets the stamps a
/// channel and a session keeper would have set by the time a turn reaches the
/// collector.
const DRIVER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
sys.stdout.write(json.dumps(json.loads(d["messages"][0]["text"])))
"#;

/// The brain, doubled. It reports what the prompt carried -- the `affinity_brief`
/// pair, the `system` families, the context it was called under -- and ends the
/// round on `finish_reason == 'length'`, the one exit of the shipped brain that
/// goes straight back into the collector as an answer.
const BRAIN: &str = r#"
import sys, json
doc = json.load(sys.stdin)
ctx = (doc["envelope"].get("header") or {}).get("context") or {}
body = doc["body"]
msgs = body.get("messages") or []
pair = None
for i, m in enumerate(msgs):
    if m.get("type") != "tool_call":
        continue
    try:
        fn = json.loads(m.get("text") or "{}")
    except Exception:
        fn = {}
    if not isinstance(fn, dict) or fn.get("name") != "affinity_brief":
        continue
    res = [x for x in msgs[i + 1:]
           if x.get("type") == "tool_result" and x.get("id") == m.get("id")]
    pair = {"id": m.get("id"), "args": json.loads(fn.get("arguments") or "{}"),
            "result": (res[0].get("text") if res else None)}
who = str((doc.get("params") or {}).get("who") or "")
surface = str((doc.get("params") or {}).get("surface") or "")
summary = {"who": who, "surface": surface, "pair": pair,
           "system": sorted((body.get("system") or {}).keys()),
           "turn_id": str(ctx.get("turn_id") or ""),
           "brief_caller": str(ctx.get("brief_caller") or ""),
           "asker": str(ctx.get("asker") or "")}
sys.stdout.write(json.dumps({
    "header": {"finish_reason": "length"},
    "messages": [{"origin": "assistant", "type": "text",
                  "text": json.dumps(summary)}]}))
"#;

fn double(script: &str, params: Value, purpose: &str) -> Value {
    let mut p = json!({"runner": "python3", "script_inline": script,
                       "external_timeout_ms": 15000});
    if let Value::Object(extra) = params {
        for (k, v) in extra {
            p[k] = v;
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": p,
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {"purpose": purpose, "use_when": "Test fixture only.",
                        "not_in_scope": "Not a template."}
    })
}

// ═══════════════════════════════════════════════════════════════ the tree

/// The root graph: the driver's doors into the tree, and ONE drain for
/// everything that leaves the member. The drain is the witness of the leak pin:
/// whatever of the brief road reaches the level above the member arrives here.
fn main_config() -> Value {
    let mut edges = vec![
        json!({"from": "./driver", "to": "./person",
               "condition": "has(hop.route) && hop.route == 'in_brief'"}),
        json!({"from": "./person", "to": "/sink",
               "condition": "!has(hop.route) || !hop.route.startsWith('in_')"}),
    ];
    for g in [ALPHA, BETA] {
        for s in [TALKY, TALKY_CHAT] {
            let collector = format!("./person/assistants/{g}/{s}/collector");
            // A turn as the surface's session keeper hands it on
            // (`./session-keeper -> ./collector`, set_hop in_turn): the context
            // was stamped upstream.
            edges.push(json!({
                "from": "./driver", "to": collector,
                "condition": format!("has(hop.route) && hop.route == 'turn_{g}_{s}'"),
                "modifier": {"set_hop": {"route": "'in_turn'"}}}));
            // The probe into the surface's own round table -- the one read past
            // the doors this file keeps: "the answer did NOT land in the other
            // generation / the other surface" is only measurable where it would
            // have landed.
            edges.push(json!({
                "from": "./driver", "to": format!("{collector}/window"),
                "condition": format!("has(hop.route) && hop.route == 'probe_{g}_{s}'"),
                "modifier": {"set_context": {"store_origin": "'probe'"}}}));
            edges.push(json!({
                "from": format!("{collector}/window"), "to": "/sink",
                "condition": "has(context.store_origin) && context.store_origin == 'probe'"}));
        }
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

/// The container edges the recipe renders for one generation, out of the
/// shipped example (byte-pinned to the recipe by gh466).
fn container_edges(who: &str) -> Vec<Value> {
    let ex = read_json(&repo("examples/organism/grow-assistant.json"));
    let raw = meclaw_core::serde_json::to_string(&ex["diff"]["add_edges"]).expect("edges");
    let renamed = raw.replace("scribe", who);
    meclaw_core::serde_json::from_str::<Vec<Value>>(&renamed).expect("edges parse")
}

/// A generation: the shipped assistant, BOTH surfaces (`./talky`,
/// `./talky-chat`) expanded to the shipped talky hive with the shipped collector
/// hive inside, each brain doubled and naming its surface.
fn stage_generation(root: &std::path::Path, who: &str) {
    let base = format!("main/person/assistants/{who}");
    copy_cells(&repo("templates/assistant"), &root.join(&base));
    let mut inerts = Vec::new();
    for surface in [TALKY, TALKY_CHAT] {
        let talky = format!("{base}/{surface}");
        std::fs::remove_dir_all(root.join(&talky)).expect("drop the ref marker");
        copy_cells(&repo("templates/talky"), &root.join(&talky));
        let collector = format!("{talky}/collector");
        std::fs::remove_dir_all(root.join(&collector)).expect("drop the ref marker");
        copy_cells(&repo("templates/collector"), &root.join(&collector));
        // What the assistant's ref marker sets on the surface's collector -- the
        // same slots on both (`both_surfaces_of_the_assistant_set_brief_slots`) --
        // minus the knobs that would send this turn anywhere else (no memory
        // leg, no per-turn episode): the brief leg is the only leg besides the
        // window.
        patch(root, &format!("{collector}/assemble/config.json"), |v| {
            v["params"]["brief_slots"] = json!(["peer", "channel"]);
            v["params"]["memory_tier"] = json!("");
            v["params"]["turn_write"] = json!("0");
        });
        write(
            root,
            &format!("{talky}/brain/config.json"),
            &double(
                BRAIN,
                json!({"who": who, "surface": surface}),
                "The brain, doubled: it reports its prompt.",
            ),
        );
        inerts.push(format!("{talky}/session-keeper"));
        inerts.push(format!("{talky}/dispatcher"));
    }
    inerts.push(format!("{base}/cogny"));
    inerts.push(format!("{base}/tools"));
    for inert in inerts {
        let _ = std::fs::remove_dir_all(root.join(&inert));
        write(
            root,
            &format!("{inert}/config.json"),
            &double(
                INERT,
                json!({}),
                "Inert double for a node this road never reaches.",
            ),
        );
    }
}

/// The counterpart and what the member released about it, to each generation in
/// a round that holds the counterpart itself (OR-AG-13, R-AF-3).
fn seed_counterpart(root: &std::path::Path, with_trust: bool) {
    let seed = "main/person/affinity/store/seed";
    append_rows(
        root,
        &format!("{seed}/entities.jsonl"),
        &[
            json!({"entity_id": COUNTERPART, "kind": "peer", "display_name": "North",
                 "canonical_name": "north", "owner_member": "member:alex",
                 "aieos": {"identity": {"names": {"first": "North"}}},
                 "aieos_version": "1.1.0",
                 "mx": {"peer": {"name": "north", "url": "https://north.example/peer"}},
                 "status": "active", "supersedes": "", "source": "curated",
                 "confidence": 90, "recorded_at": "2026-01-01T00:00:00Z"}),
        ],
    );
    let mut disclosure = Vec::new();
    for g in [ALPHA, BETA] {
        for (i, field) in ["aieos.identity.names", "mx.peer"].iter().enumerate() {
            disclosure.push(json!({
                "audience": format!("agent:{g}"),
                "audience_set": [format!("agent:{g}"), COUNTERPART],
                "decided_at": "2026-01-01T00:00:00Z", "entity_id": COUNTERPART,
                "field_path": field, "id": format!("disc:north-{g}-{i}"), "mode": "share"}));
        }
    }
    append_rows(root, &format!("{seed}/disclosure.jsonl"), &disclosure);
    if with_trust {
        append_rows(
            root,
            &format!("{seed}/trust.jsonl"),
            &[
                json!({"audience": format!("agent:{ALPHA}"), "decided_at": "2026-01-01T00:00:00Z",
                     "decided_by": "member:alex", "entity_id": COUNTERPART,
                     "id": "trust:north-alpha", "level": "known"}),
            ],
        );
    } else {
        // THE ERROR CASE: a store without the table the brief reads first. The
        // brief's own store-error path answers `error` -- the one exit besides
        // `answer`, and the one a caller must not wait for forever.
        std::fs::remove_file(root.join(format!("{seed}/trust.jsonl"))).expect("drop the seed");
        patch(root, "main/person/affinity/store/config.json", |v| {
            v["params"]["schema"]
                .as_object_mut()
                .expect("schema")
                .remove("trust");
        });
    }
}

fn build_tree(td: &tempfile::TempDir, with_trust: bool) {
    let root = td.path();
    // The test writes its own empty environment file at run time; no fixture
    // ships one.
    std::fs::write(root.join(".env"), "").expect("write an empty env file");
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/driver/config.json",
        &double(
            DRIVER,
            json!({}),
            "Test driver: emits what the test hands it.",
        ),
    );

    copy_cells(&repo("templates/member"), &root.join("main/person"));
    for holder in ["access", "memory-hive", "firewall"] {
        write(
            root,
            &format!("main/person/{holder}/config.json"),
            &double(
                INERT,
                json!({}),
                "Inert double for a holder this road never reaches.",
            ),
        );
    }
    std::fs::remove_dir_all(root.join("main/person/affinity")).expect("drop the ref marker");
    copy_cells(
        &repo("templates/affinity"),
        &root.join("main/person/affinity"),
    );
    // `${uuid7:…}` is minted by the mutation path; a filesystem boot is handed a
    // literal, and the push clock a cron that never fires during a test.
    patch(root, "main/person/affinity/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!("01916f00-0000-7000-8000-000000000834");
        v["params"]["schedules"][0]["cron"] = json!("0 0 4 * * *");
    });
    seed_counterpart(root, with_trust);

    for g in [ALPHA, BETA] {
        stage_generation(root, g);
    }
    patch(root, "main/person/assistants/config.json", |v| {
        let mut edges = Vec::new();
        for g in [ALPHA, BETA] {
            edges.extend(container_edges(g));
        }
        v["params"]["graph"] = json!({"edges": edges});
    });
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (tx, rx) = mpsc::channel::<Message>(128);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped member, affinity, talky and collector must boot");
    (h, rx)
}

// ═══════════════════════════════════════════════════════════════ the drive

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn text_of(m: &Message) -> String {
    body_of(m)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Something the driver emits, with the context it is emitted under.
fn drive(ctx: Value, header: Value, messages: Value) -> Message {
    let spec = json!({"header": header, "messages": messages});
    let ctx: Map<String, Value> = ctx.as_object().cloned().expect("ctx object");
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": spec.to_string()}]})))
        .context(ctx)
        .ttl(400)
        .build()
}

/// A turn of `gen` on its spoken surface, as it stands at the collector's door:
/// the channel's stamps (`counterpart` beside `channel`, the round with the
/// counterpart in it), the member's (`assistant`), the session keeper's
/// (`session_id`, the turn).
fn turn(who: &str, turn_id: &str) -> Message {
    turn_at(who, TALKY, turn_id, Some(COUNTERPART))
}

/// A turn of `gen` at `surface`. `counterpart: None` is a channel that stamps
/// none at all; `Some("")` is what the member's turn doors promote for such a
/// channel (`has(context.counterpart) ? context.counterpart : ''`, OR-AG.M3.5)
/// -- the Telegram turn of a generation whose knob is on.
fn turn_at(who: &str, surface: &str, turn_id: &str, counterpart: Option<&str>) -> Message {
    let mut ctx = json!({"session_id": format!("s-{who}-{surface}"), "turn_id": turn_id,
                         "assistant": who, "channel": "peer-channel:north",
                         "audience_set": format!("[\"agent:{who}\",\"{COUNTERPART}\"]")});
    if let Some(c) = counterpart {
        ctx["counterpart"] = json!(c);
    }
    drive(
        ctx,
        json!({"route": format!("turn_{who}_{surface}"), "turn_id": turn_id}),
        json!([{"origin": "user", "type": "text", "text": "hello from the north"}]),
    )
}

/// A read of the round table of `gen`'s `surface`.
fn probe_at(who: &str, surface: &str, turn_id: &str) -> Message {
    let op = json!({"operation": "select", "table": "round",
                    "columns": ["turn_id", "role", "turn"],
                    "where": {"turn_id": turn_id, "role": "leg-brief"}});
    drive(
        json!({}),
        json!({"route": format!("probe_{who}_{surface}")}),
        json!([{"origin": "assistant", "type": "tool_call", "id": "p1",
                "text": op.to_string()}]),
    )
}

async fn recv(rx: &mut mpsc::Receiver<Message>, secs: u64) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(secs), rx.recv())
        .await
        .ok()
        .flatten()
}

/// The brain's report of a turn, as it left the member on `answer`.
fn report_of(m: &Message) -> Option<Value> {
    if hop_of(m, "route") != "answer" {
        return None;
    }
    let v: Value = meclaw_core::serde_json::from_str(&text_of(m)).ok()?;
    v.get("who").is_some().then_some(v)
}

/// Whatever the sink collects until `want` brain reports have arrived. Every
/// other message is kept: the leak pin reads them.
async fn until_reports(
    rx: &mut mpsc::Receiver<Message>,
    want: usize,
) -> (Vec<Value>, Vec<Message>) {
    let (mut reports, mut others) = (Vec::new(), Vec::new());
    while reports.len() < want {
        let m = recv(rx, 60).await.unwrap_or_else(|| {
            panic!(
                "a turn never opened -- the brief leg was never parked, so the fan-in \
                 waits for ever. Reports so far: {reports:?}; also seen: {:?}",
                others
                    .iter()
                    .map(|m: &Message| format!("{:?} {}", m.headers.hop, text_of(m)))
                    .collect::<Vec<_>>()
            )
        });
        match report_of(&m) {
            Some(r) => reports.push(r),
            None => others.push(m),
        }
    }
    (reports, others)
}

/// The rows a probe of `gen`'s round table read.
async fn probed(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    who: &str,
    turn_id: &str,
) -> Vec<Value> {
    probed_at(h, rx, who, TALKY, turn_id).await
}

/// The rows a probe of the round table of `gen`'s `surface` read.
async fn probed_at(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    who: &str,
    surface: &str,
    turn_id: &str,
) -> Vec<Value> {
    h.send(probe_at(who, surface, turn_id)).await;
    for _ in 0..16 {
        let m = recv(rx, 30).await.expect("the probe answers");
        if m.headers.hop.get("operation").is_some() {
            let rows: Value =
                meclaw_core::serde_json::from_str(&text_of(&m)).unwrap_or(Value::Null);
            return rows.as_array().cloned().unwrap_or_default();
        }
    }
    panic!("the probe of {who}/{surface} never answered");
}

/// `(to_path, headers)` of every message the colony logged for a cell inside the
/// member's `affinity` -- read out of the message log AFTER the fact, the one
/// place where "nothing was sent there" is a row count rather than a silence
/// (the 709 locks read the same log the same way).
fn affinity_log(td: &tempfile::TempDir) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT to_path, headers FROM message_log")
        .expect("message_log");
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("query")
        .filter_map(Result::ok)
        .filter(|(to, _)| to.starts_with("/person/affinity"))
        .collect()
}

/// The rows of `affinity_log` that belong to one turn (the member's edge A
/// promotes the turn onto context, and context rides along the whole trace).
fn affinity_rows_of(td: &tempfile::TempDir, turn_id: &str) -> Vec<(String, String)> {
    let needle = format!("\"turn_id\":\"{turn_id}\"");
    affinity_log(td)
        .into_iter()
        .filter(|(_, headers)| headers.replace(' ', "").contains(&needle))
        .collect()
}

/// A message on the brief road that reached the level above the member: every
/// emission of `affinity/brief` carries `hop.subscriber` (empty on the tool lane),
/// and nothing a generation says does.
fn is_brief_road(m: &Message) -> bool {
    m.headers.hop.contains_key("subscriber")
}

fn pair_of(r: &Value) -> &Value {
    &r["pair"]
}

// ══════════════════════════════════════════════════════════════ the locks

/// The member hop is what makes the brief answerable: without `turn_id` the answer
/// parks under no turn, without `asker` affinity refuses (`no_audience`), without
/// `brief_caller == 'inside'` the answer leaves the member instead of coming home.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_member_stamps_turn_id_asker_and_brief_caller() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, true);
    let (h, mut rx) = boot(&td).await;
    h.send(turn(ALPHA, "peer-channel#a1")).await;
    let (reports, _) = until_reports(&mut rx, 1).await;
    let r = &reports[0];
    assert_eq!(r["who"], ALPHA, "{r}");
    let pair = pair_of(r);
    assert!(
        pair.is_object(),
        "the prompt carries the affinity_brief pair -- the brief left, was answered and \
         came home: {r}"
    );
    assert_eq!(pair["args"]["subject"], COUNTERPART, "{r}");
    let result = pair["result"].as_str().unwrap_or_default();
    assert!(
        result.starts_with(&format!(
            "affinity brief on North (peer) for agent:{ALPHA}:"
        )),
        "asker = 'agent:' + context.assistant, stamped by the member (the receipt line \
         names the audience affinity served): {result}"
    );
    assert!(
        result.contains("trust known"),
        "the brief read the trust row of this pair: {result}"
    );
    assert!(
        !r["system"]
            .as_array()
            .expect("system list")
            .iter()
            .any(|s| s == "peer" || s == "channel"),
        "no brief slot lands in system.*: {r}"
    );
    assert_eq!(
        r["turn_id"], "peer-channel#a1",
        "the turn the round belongs to: {r}"
    );
    assert_eq!(
        r["brief_caller"], "",
        "the member's reply-to token is its own: edge B clears it on the way home: {r}"
    );
    assert_eq!(
        r["asker"], "",
        "and the asker it stamped for this round trip: {r}"
    );

    // turn_id at the receiver: the leg is filed under the turn that asked.
    let rows = probed(&h, &mut rx, ALPHA, "peer-channel#a1").await;
    assert_eq!(rows.len(), 1, "one brief leg under the turn: {rows:?}");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_answer_and_the_error_find_the_asking_generation_and_not_the_other() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    // The answer: two generations brief at once, each gets its own.
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, true);
    let (h, mut rx) = boot(&td).await;
    h.send(turn(ALPHA, "peer-channel#a2")).await;
    h.send(turn(BETA, "peer-channel#b2")).await;
    let (reports, _) = until_reports(&mut rx, 2).await;
    for g in [ALPHA, BETA] {
        let r = reports
            .iter()
            .find(|r| r["who"] == g)
            .unwrap_or_else(|| panic!("no report from {g}: {reports:?}"));
        let result = pair_of(r)["result"].as_str().unwrap_or_default();
        assert!(
            result.contains(&format!("for agent:{g}:")),
            "{g} got the brief affinity served to agent:{g}: {r}"
        );
    }
    for (asked, other, tid) in [
        (ALPHA, BETA, "peer-channel#a2"),
        (BETA, ALPHA, "peer-channel#b2"),
    ] {
        assert_eq!(
            probed(&h, &mut rx, asked, tid).await.len(),
            1,
            "the leg of {tid} is in {asked}'s round table"
        );
        assert!(
            probed(&h, &mut rx, other, tid).await.is_empty(),
            "and NOT in {other}'s: the container door is guarded on context.assistant"
        );
    }
    h.shutdown().await;

    // The error: a store that cannot read. The leg is empty, the turn opens.
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, false);
    let (h, mut rx) = boot(&td).await;
    h.send(turn(ALPHA, "peer-channel#a3")).await;
    let (reports, others) = until_reports(&mut rx, 1).await;
    let r = &reports[0];
    assert_eq!(
        r["who"], ALPHA,
        "the error came home to the generation that asked: {r}"
    );
    assert!(
        pair_of(r).is_null(),
        "an error is an empty leg -- no pair: {r}"
    );
    assert!(
        !others.iter().any(is_brief_road),
        "the error did not leave the member: {:?}",
        others.iter().map(|m| &m.headers.hop).collect::<Vec<_>>()
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_in_brief_from_outside_still_leaves_through_the_door() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, true);
    let (h, mut rx) = boot(&td).await;
    let ask = json!({"subject": COUNTERPART, "slots": ["peer"]});
    h.send(drive(
        json!({}),
        json!({"route": "in_brief", "audience": format!("agent:{ALPHA}"),
               "audience_set": format!("[\"agent:{ALPHA}\",\"{COUNTERPART}\"]")}),
        json!([{"origin": "assistant", "type": "tool_call", "id": "out-1",
                "text": ask.to_string()}]),
    ))
    .await;
    let m = recv(&mut rx, 60)
        .await
        .expect("the outside asker is answered at the member's rim");
    assert_eq!(hop_of(&m, "route"), "answer", "{:?}", m.headers.hop);
    assert_eq!(hop_of(&m, "subscriber"), "", "the tool lane, not a push");
    assert!(
        text_of(&m).starts_with(&format!(
            "affinity brief on North (peer) for agent:{ALPHA}:"
        )),
        "{}",
        text_of(&m)
    );
    // Nothing of it went into a generation: no turn was opened by it.
    assert!(
        recv(&mut rx, 3).await.is_none(),
        "an outside brief is answered once, at the rim, and nowhere else"
    );
    h.shutdown().await;
}

/// THE LEAK PIN (OR-AG-24). Before the discriminator, every internal brief answer
/// ALSO left through `./affinity -> .` -- up to the OS root, dead-lettered there as
/// `hive_no_route`. Zero messages of the brief road reach the level above the
/// member, and nothing dead-letters.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_brief_answer_reaches_the_os_level() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, true);
    let (h, mut rx) = boot(&td).await;
    h.send(turn(ALPHA, "peer-channel#a4")).await;
    h.send(turn(BETA, "peer-channel#b4")).await;
    let (reports, mut others) = until_reports(&mut rx, 2).await;
    assert!(
        reports.iter().all(|r| pair_of(r).is_object()),
        "both briefs went out and came home: {reports:?}"
    );
    while let Some(m) = recv(&mut rx, 3).await {
        others.push(m);
    }
    let leaked: Vec<_> = others
        .iter()
        .filter(|m| is_brief_road(m))
        .map(|m| (m.headers.hop.clone(), text_of(m)))
        .collect();
    assert!(
        leaked.is_empty(),
        "0 messages of the brief road may reach the level above the member: {leaked:?}"
    );
    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "and nothing dead-letters: {dead:?}");
    h.shutdown().await;
}

/// THE CHAT HALF OF THE ROAD (OR-AG.M3.4). Both surfaces of one generation ask at
/// once. The spoken surface's `in_briefing` door is the `default`, the chat's is
/// the one regular door, guarded on `hop.brief_surface == 'talky-chat'` -- a token
/// the chat's own `brief` v-lane stamps on context and the member's edge B moves
/// onto the hop. Each surface gets its own brief at its own brain, and neither
/// round table holds the other's leg. A broken link anywhere in that chain puts
/// the chat's answer into the spoken surface (the default takes it), and the chat
/// turn waits without a deadline -- this lock then times out naming it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_typed_turn_is_briefed_at_the_chat_surface_and_the_answer_finds_that_surface() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, true);
    let (h, mut rx) = boot(&td).await;
    let (spoken, typed) = ("peer-channel#t5", "peer-channel#c5");
    h.send(turn_at(ALPHA, TALKY, spoken, Some(COUNTERPART)))
        .await;
    h.send(turn_at(ALPHA, TALKY_CHAT, typed, Some(COUNTERPART)))
        .await;
    let (reports, mut others) = until_reports(&mut rx, 2).await;
    for (surface, tid) in [(TALKY, spoken), (TALKY_CHAT, typed)] {
        let r = reports
            .iter()
            .find(|r| r["surface"] == surface)
            .unwrap_or_else(|| panic!("no report from the {surface} brain: {reports:?}"));
        assert_eq!(r["who"], ALPHA, "{r}");
        assert_eq!(
            r["turn_id"], tid,
            "the {surface} brain was called for its own turn: {r}"
        );
        let pair = pair_of(r);
        assert!(
            pair.is_object(),
            "the {surface} prompt carries the affinity_brief pair -- the brief left \
             that surface and came home to it: {r}"
        );
        assert_eq!(pair["args"]["subject"], COUNTERPART, "{r}");
        assert!(
            pair["result"]
                .as_str()
                .unwrap_or_default()
                .starts_with(&format!(
                    "affinity brief on North (peer) for agent:{ALPHA}:"
                )),
            "{r}"
        );
    }
    // At the receiver: each leg is filed in the round table of the surface that
    // asked, and NOT in the sibling's -- the default door did not also take it.
    for (asked, other, tid) in [(TALKY, TALKY_CHAT, spoken), (TALKY_CHAT, TALKY, typed)] {
        assert_eq!(
            probed_at(&h, &mut rx, ALPHA, asked, tid).await.len(),
            1,
            "the leg of {tid} is in {asked}'s round table"
        );
        assert!(
            probed_at(&h, &mut rx, ALPHA, other, tid).await.is_empty(),
            "and NOT in {other}'s: the in_briefing doors are told apart by \
             hop.brief_surface"
        );
    }
    // The instrument the next lock reads is proven here: the brief of each turn
    // IS in the message log, at affinity.
    for tid in [spoken, typed] {
        assert!(
            !affinity_rows_of(&td, tid).is_empty(),
            "the brief of {tid} reached affinity, and the log shows it"
        );
    }
    while let Some(m) = recv(&mut rx, 3).await {
        others.push(m);
    }
    assert!(
        !others.iter().any(is_brief_road),
        "no brief of either surface left the member"
    );
    h.shutdown().await;
}

/// THE TELEGRAM TURN of a generation whose knob is on (OR-AG-10, OR-AG.M3.1): a
/// channel that stamps no counterpart. The leg is parked EMPTY at the turn's
/// opening, so the fan-in completes on the window alone and the brain is called
/// -- measured at the brain, not over stdin. And nothing asks affinity: zero rows
/// of either turn in the message log at affinity. Both forms of "none" -- the key
/// absent, and the empty string the member's turn doors promote -- on both
/// surfaces.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_without_a_counterpart_opens_at_the_brain_and_asks_affinity_nothing() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, true);
    let (h, mut rx) = boot(&td).await;
    let turns = [
        (TALKY, "telegram#t6", Some("")),
        (TALKY_CHAT, "telegram#c6", None),
    ];
    for (surface, tid, counterpart) in turns {
        h.send(turn_at(ALPHA, surface, tid, counterpart)).await;
    }
    let (reports, mut others) = until_reports(&mut rx, 2).await;
    for (surface, tid, _) in turns {
        let r = reports
            .iter()
            .find(|r| r["surface"] == surface)
            .unwrap_or_else(|| panic!("the {surface} turn never reached its brain: {reports:?}"));
        assert_eq!(r["turn_id"], tid, "{r}");
        assert!(
            pair_of(r).is_null(),
            "no counterpart, no brief in the prompt: {r}"
        );
        let rows = probed_at(&h, &mut rx, ALPHA, surface, tid).await;
        assert_eq!(
            rows.len(),
            1,
            "the leg of {tid} is parked, empty, at {surface}: {rows:?}"
        );
    }
    while let Some(m) = recv(&mut rx, 3).await {
        others.push(m);
    }
    for (_, tid, _) in turns {
        let asked = affinity_rows_of(&td, tid);
        assert!(
            asked.is_empty(),
            "a turn without a counterpart asks affinity nothing -- there is no \
             fallback subject: {asked:?}"
        );
    }
    assert!(!others.iter().any(is_brief_road), "{others:?}");
    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "and nothing dead-letters: {dead:?}");
    h.shutdown().await;
}
