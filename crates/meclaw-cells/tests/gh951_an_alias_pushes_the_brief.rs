//! GH #951 -- an alias pushes the brief.
//!
//! The acceptance of the objects hive across the three strands that carry it
//! (K: the curator's sidecar section `things` and its `in_candidate` lane;
//! M: memory's `in_alias` and `in_query {subject}`; O: the objects hive and
//! the member's wiring), on a real colony. The road of
//! `support/gh929_member_road.rs`, widened by the two holders this lock is
//! about: a generation of the SHIPPED assistant under its container (grown the
//! way `examples/organism/grow-assistant.json` grows it), the member's memory,
//! its objects and its graph space, the member's own edges between the four
//! drawn as they stand in `templates/member/config.json`, everything else that
//! leaves them drained. Every `llm` cell talks to a local stub -- the typed
//! surface's brain to a scripted one that answers each turn by the marker in
//! the person's words, every other to the background stub -- and the memory's
//! embedder to a local embeddings stub. No paid provider is reachable by
//! construction.
//!
//! One conversation, measured at the receivers:
//!
//! 1. **Three turns make a row.** The brain names the same thing in its
//!    sidecar section `things` on three turns of one round; the objects store
//!    holds an `active` row `ob-...` learned in that round (a row in its
//!    `cell.db`, not an emission), and the graph space indexed it under the
//!    same round.
//! 2. **Memory files a new fact under the object.** The row's aliases reached
//!    memory (`subject_aliases` -> `ob-...`); a fact about the thing said on a
//!    LATER turn stands in the memory store with `canonical_subject` `ob-...`.
//! 3. **The alias pushes the brief.** A later turn of the same round whose
//!    words carry the alias reaches the brain with the object's brief in the
//!    curator's push part `[...]` -- the request at the stub, not a log line.
//! 4. **A round that is not covered gets nothing.** A turn on another channel,
//!    in a round with one more person, carries the same alias; its request
//!    has no brief.
//! 5. **The owner confirms through the tools, nobody else** (review O I-4).
//!    On a turn of the bound channel the typed surface's brain calls
//!    `object_confirm` on the object: the call leaves the generation, crosses
//!    the member's door `./assistants -> ./objects` with the turn's round as
//!    `audience_now`, `tool_caller` 'talky-chat' and the bound channel's
//!    `context.speaker`, and the brain's next request carries `ok`. The same
//!    call on a turn without a speaker comes back `owner_only` -- fail-closed.
//! 6. **Clean.** No `ttl_expired`, no dead letter, no refusal of the objects'
//!    traffic (no memory refusal under `recall_caller` 'objects' at the
//!    assistants, no refused candidate or alias, no tool refusal but the one
//!    `owner_only` of point 5), and every delivery out of a code cell of the
//!    objects hive -- the tools included -- carries a route its
//!    `contract.emits` declares. The door `./assistants -> ./objects` (`thing_seen`) restores
//!    no budget: every run prints what is left of it at the hive
//!    (`gh951 door thing_seen: ...`), and a `ttl_expired` fails at once with
//!    that rest in the message.
//!
//! Every wait is on a signal (a row in a store, a request at a stub, an answer
//! at the sink) bounded by a failure deadline, never a timing window. Written
//! against the contracts of K and M as built (`K.<n>` / `M.<n> as built`):
//! every key this lock reads off another strand names its seam.
//!
//! Guarded like every template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/gh929_member_road.rs"]
mod road;
#[path = "support/graph_space_colony.rs"]
mod space;

use meclaw_core::serde_json::{self as sj, Value, json};
use meclaw_core::{Body, MESSAGE_DEFAULT_TTL, Message, MessageBuilder, Path};
use meclaw_testing::mock_http::{
    CapturedRequest, MockResponse, RequestValidator, start_mock_server_capturing_with_validator,
};
use meclaw_testing::override_params_on_disk;
use mock_openai::{MockOpenAI, canned_chat_completion, canned_tool_calls};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

// GH #1061: the road support already loads it (`duplicate_mod`).
use road::organism_assistant;

/// The failure-marker convention of this repo, not a timing discriminator --
/// generous, because the lock runs on build lanes slower than a desk.
const DEADLINE: Duration = Duration::from_secs(120);

/// The scripted brain: the typed surface of the generation (path under `main/`).
const BRAIN: &str = "assistants/scribe/talky-chat/brain";
/// The typed surface's curator store.
const LEDGER: &str = "main/assistants/scribe/talky-chat/curator/ledger/cell.db";
const OBJECTS_DB: &str = "main/objects/store/cell.db";
const MEMORY_DB: &str = "main/memory-hive/store/cell.db";

/// The thing the brain keeps naming, and how a person says it later.
const THING: &str = "Blue Kettle";
const ALIAS_SAID: &str = "blue kettle";
/// The claim of the fact filed about it on a later turn; nothing else says it.
const PLACE: &str = "hallway cupboard";

/// The round the object is learned in (the member and its assistant), and a
/// round it does not cover (one more person).
const ROUND_A: &str = road::AUDIENCE;
const ROUND_B: &str = r#"["member:owner","agent:scribe","member:guest"]"#;
const CHANNEL_A: &str = "talky:951";
const CHANNEL_B: &str = "talky:951-group";

/// The marker every person turn ends with; the scripted brain answers by it.
const MARK: &str = "(t951-";

/// The turns on which the brain calls `object_confirm`: the first on the
/// bound channel (a speaker), the second without one.
const BY_THE_OWNER: u32 = 7;
const BY_NOBODY: u32 = 8;

/// The object the brain's tool calls name, known once the row is active.
static OB: OnceLock<String> = OnceLock::new();

fn call_id(step: u32) -> String {
    format!("call-951-{step}")
}

/// The routes of the seams this lock crosses, for a failure's diagnosis.
const SEAM_ROUTES: [&str; 13] = [
    "thing_seen",
    "candidate",
    "in_candidate",
    "candidate_ack",
    "alias",
    "in_alias",
    "alias_ack",
    "facts",
    "in_query",
    "in_facts",
    "source_changed",
    "pull",
    "in_read",
];

// ─────────────────────────────────────────────────────────────── the tree

/// The member's own edges between the four holders this road boots, as they
/// stand in the shipped member.
fn member_edges() -> Vec<Value> {
    const HOLDERS: [&str; 4] = [
        "./assistants",
        "./memory-hive",
        "./objects",
        "./graph-space",
    ];
    let member = road::read_json(&road::repo("templates/member/config.json"));
    let held = |v: &Value| v.as_str().is_some_and(|p| HOLDERS.contains(&p));
    let edges: Vec<Value> = member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| held(&e["from"]) && held(&e["to"]))
        .collect();
    let draws = |from: &str, to: &str, route: &str| {
        edges.iter().any(|e| {
            e["from"] == from
                && e["to"] == to
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("'{route}'")))
        })
    };
    assert!(
        draws("./assistants", "./objects", "thing_seen")
            && draws("./objects", "./assistants", "candidate")
            && draws("./objects", "./memory-hive", "alias")
            && draws("./objects", "./graph-space", "source_changed"),
        "the member does not wire its objects (GH #951 O.6): sightings in, briefs to the \
         assistants, aliases to memory, versions to the graph space: {edges:#?}"
    );
    edges
}

/// K.3 / K.4 as built (OR-BC.O.1): the container lets `thing_seen` out of the
/// generation and `in_candidate` into it. The recipe is the builder's
/// `_assistant_level` (K's), which this road grows the generation with.
fn the_generation_lets_sightings_out_and_briefs_in() {
    let grown = organism_assistant::at_the_container(&road::read_json(&road::repo(
        "examples/organism/grow-assistant.json",
    )));
    let edges = grown["diff"]["add_edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let names = |e: &Value, route: &str| {
        e["condition"]
            .as_str()
            .is_some_and(|c| c.contains(&format!("'{route}'")))
    };
    assert!(
        edges
            .iter()
            .any(|e| e["from"] == "./scribe" && e["to"] == "." && names(e, "thing_seen")),
        "the grown container lets no `thing_seen` out of the generation (seam K.3, \
         OR-BC.O.1): the objects never hear a sighting"
    );
    assert!(
        edges.iter().any(|e| e["from"] == "."
            && e["to"].as_str().is_some_and(|t| t.starts_with("./scribe"))
            && names(e, "in_candidate")),
        "the grown container lets no `in_candidate` into the generation (seam K.4, \
         OR-BC.O.1): no brief reaches a curator"
    );
}

fn drain(from: &str, lane: &str) -> Value {
    json!({"from": from, "to": "/park",
           "condition": format!("has(hop.route) && hop.route == '{lane}'")})
}

fn build(td: &tempfile::TempDir, stubs: &road::Stubs, embed: &str) {
    let root = td.path();
    let main = root.join("main");
    road::copy_resolved(
        &road::repo("templates/assistant"),
        &main.join("assistants/scribe"),
        0,
    );
    road::copy_resolved(
        &road::repo("templates/memory-hive"),
        &main.join("memory-hive"),
        0,
    );
    road::copy_resolved(
        &road::repo("templates/member/objects"),
        &main.join("objects"),
        0,
    );
    road::copy_resolved(
        &road::repo("templates/member/graph-space"),
        &main.join("graph-space"),
        0,
    );
    let grown = organism_assistant::at_the_container(&road::read_json(&road::repo(
        "examples/organism/grow-assistant.json",
    )));
    road::write_json(
        &main.join("assistants/config.json"),
        &json!({"cell": {"type": "hive"},
                "params": {"graph": {"edges": grown["diff"]["add_edges"].clone()}}}),
    );
    // K.3 as built: a curator offers the section `things` (`./schemas`) and
    // hands it on as `thing_seen` (`./intake`) only with the param
    // `things_section` "1" on both cells; every shipped curator says "0". Set
    // on every curator of the generation.
    for surface in ["talky", "talky-chat", "cogny"] {
        for cell in ["schemas", "intake"] {
            let at = main.join(format!("assistants/scribe/{surface}/curator/{cell}"));
            if at.join("config.json").is_file() {
                override_params_on_disk(&at, &json!({"things_section": "1"}));
            }
        }
    }
    let mut edges = member_edges();
    edges.push(json!({"from": "./assistants", "to": "/sink",
                      "condition": "has(hop.route) && hop.route == 'answer'"}));
    edges.push(json!({"from": "./assistants", "to": "/park",
                      "condition": "has(hop.route)", "default": true}));
    // M.1 as built: `alias_ack` is a required drain of memory; it leaves on the
    // member's own edge into the objects, beside the bundle, the tool lanes and
    // the refusal.
    let taken = [
        "bundle",
        "tool_result",
        "tool_schemas",
        "reject",
        "alias_ack",
    ];
    for lane in road::rim_emits("memory-hive") {
        if !taken.contains(&lane.as_str()) {
            edges.push(drain("./memory-hive", &lane));
        }
    }
    edges.push(json!({"from": "./memory-hive", "to": "/park",
                      "condition": "has(hop.route) && hop.route == 'reject' && \
                                    (!has(hop.recall_caller) || hop.recall_caller == 'outside')"}));
    // The graph space's `pull` is the member's (an `ob-` source to the
    // objects); every other lane of it is drained. The objects get no drain:
    // each of their lanes has a member edge, and one without is a dead letter
    // this lock must see.
    for lane in road::rim_emits("graph-space") {
        if lane != "pull" {
            edges.push(drain("./graph-space", &lane));
        }
    }
    road::write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    road::quiet_timers(&main);
    let pointed = road::point_llms_at_stubs(&main, stubs);
    assert!(
        pointed.iter().any(|p| p == BRAIN),
        "{BRAIN} is not an llm cell of the road: {pointed:?}"
    );
    road::write_env(root, &main, &stubs.background);
    // The memory's embedder talks to the embeddings stub, not to the chat stub.
    let env = root.join(".env");
    let raw = std::fs::read_to_string(&env).expect("the run's env file");
    let lines: String = raw
        .lines()
        .map(|l| {
            if l.starts_with("MEMORY_EMBED_ENDPOINT=") {
                format!("MEMORY_EMBED_ENDPOINT={embed}\n")
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    std::fs::write(env, lines).expect("rewrite the run's env file");
}

// ─────────────────────────────────────────────────────────────── the brain

fn content_of(m: &Value) -> String {
    match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The turn a brain request answers: the marker of the newest person turn on
/// its wire.
fn step_of(body: &Value) -> Option<u32> {
    let text = body["messages"]
        .as_array()?
        .iter()
        .rev()
        .filter(|m| m["role"] == "user")
        .map(content_of)
        .find(|t| t.contains(MARK))?;
    let at = text.rfind(MARK)? + MARK.len();
    text[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

fn with_block(text: &str, block: Value) -> String {
    format!("{text}\n\n```sidecar\n{block}\n```")
}

/// What the brain says on each turn: the thing in its section `things` on
/// turns 1-3, a fact about it in its section `memory` on turn 4, plain words
/// after. Each answer names its turn, so the sink can tell them apart.
fn reply_for(step: Option<u32>) -> String {
    match step {
        // K.3 as built: the section `things` is an object whose `items` list
        // {name, kind?, note?} -- the splitter drops a bare list under a key;
        // beside it the memory's nothing form, as gh949's owner answers.
        Some(n @ 1..=3) => with_block(
            &format!("Noted. (r951-{n})"),
            json!({"things": {"items": [
                       {"name": THING, "kind": "thing", "note": "the one on the stove"}]},
                   "memory": {"nothing_new": true, "facts": [],
                              "topic": {"movement": "continue"}}}),
        ),
        Some(4) => with_block(
            "Noted. (r951-4)",
            json!({"memory": {
                "facts": [{"subject": THING, "predicate": "located_in", "claim": PLACE,
                           "fact_kind": "world", "valid_from": null}],
                "topic": {"movement": "start", "name": "the kettle"}}}),
        ),
        Some(n) => format!("Noted. (r951-{n})"),
        None => road::REPLY.to_string(),
    }
}

/// Whether the request already carries the result of THIS turn's own call
/// `id` -- the second request of a tool round. Any tool message after the
/// person's turn is not enough: every turn opens with the memory's recall pair
/// (`assistant` + `tool`), so the first request of turn 7 already carried one,
/// the brain answered at once and `object_confirm` was never called (Fix-Runde 2).
fn tool_round_closed(body: &Value, id: &str) -> bool {
    let msgs = body["messages"].as_array().cloned().unwrap_or_default();
    msgs.iter()
        .any(|m| m["role"] == "tool" && m["tool_call_id"] == id)
}

fn brain_answer(req: &CapturedRequest) -> Option<MockResponse> {
    let body: Value = sj::from_slice(&req.body).ok()?;
    let step = step_of(&body);
    // Turns 7 and 8: first `object_confirm` on the object, then -- with the
    // result on the wire -- the turn's answer.
    let tool_turn = matches!(step, Some(BY_THE_OWNER | BY_NOBODY));
    let closed = step.is_some_and(|n| tool_round_closed(&body, &call_id(n)));
    if tool_turn && !closed {
        let n = step.expect("a tool turn has its marker");
        let ob = OB.get().expect("the object is known before its tool turns");
        let (id, args) = (call_id(n), json!({"id": ob}).to_string());
        return Some(canned_tool_calls(vec![(
            id.as_str(),
            "object_confirm",
            args.as_str(),
        )]));
    }
    Some(canned_chat_completion(&reply_for(step), "stop"))
}

/// The tool result under `id` in one brain request, read as the JSON the
/// objects' tools answer with.
fn tool_result_in(wire: &Value, id: &str) -> Value {
    let content = wire["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
        .map(content_of)
        .unwrap_or_else(|| {
            panic!(
                "the brain's request carries no tool result under `{id}` -- the call did not \
                 come back through the objects:\n{}",
                sketch(wire)
            )
        });
    // The brain sees every block with its id in front (`[#<12 hex>] `), a tool
    // result too -- measured in Fix-Runde 2: `[#980f00f1bcd7] {"id": ..., "ok": true}`.
    // The JSON the objects answer with is what follows the marker.
    let json = content
        .strip_prefix("[#")
        .and_then(|r| r.split_once("] "))
        .map_or(content.as_str(), |(_, rest)| rest);
    sj::from_str(json).unwrap_or_else(|_| Value::String(content.clone()))
}

struct Brain {
    url: String,
    captured: Arc<tokio::sync::Mutex<Vec<CapturedRequest>>>,
}

impl Brain {
    async fn start() -> Self {
        let answer: RequestValidator = Arc::new(brain_answer);
        let (addr, _join, captured) = start_mock_server_capturing_with_validator(
            vec![canned_chat_completion(road::REPLY, "stop")],
            Some(answer),
        )
        .await;
        Brain {
            url: format!("http://{addr}"),
            captured,
        }
    }

    /// The newest request the brain got for turn `step`.
    async fn request_of(&self, step: u32) -> Value {
        let all: Vec<Value> = self
            .captured
            .lock()
            .await
            .iter()
            .filter_map(|c| sj::from_slice::<Value>(&c.body).ok())
            .collect();
        all.into_iter()
            .rev()
            .find(|b| step_of(b) == Some(step))
            .unwrap_or_else(|| panic!("turn {step} never reached the brain"))
    }
}

/// A text on one line: every run of whitespace one space. The curator shows a
/// candidate on one line (`[thing: ... what: ...]`); a brief is written in lines.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `(index, role)` of every message of a request whose content carries
/// `needle`, both read on one line.
fn parts_with(wire: &Value, needle: &str) -> Vec<(usize, String)> {
    let needle = one_line(needle);
    wire["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter(|(_, m)| one_line(&content_of(m)).contains(&needle))
        .map(|(i, m)| (i, m["role"].as_str().unwrap_or_default().to_string()))
        .collect()
}

/// A request in short, for a failure message.
fn sketch(wire: &Value) -> String {
    wire["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let text: String = content_of(m).chars().take(400).collect();
            format!("{i:2} {}: {text}", m["role"].as_str().unwrap_or_default())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ─────────────────────────────────────────────────────────────── the turns

/// A person's words at the container's door, with the budget the ingress
/// leaves after the member's own two routing decisions.
fn person(step: u32, text: &str, round: &str, channel: &str, bound: bool) -> Message {
    // K.3 as built: the intake's `thing_seen` carries the turn's id
    // (`context.turn_id`); the objects count a turn once by it.
    let mut ctx = json!({"assistant": "scribe", "channel": channel, "channel_node": "chat",
                         "audience_set": round, "turn_id": format!("t951-{step}")});
    if bound {
        // K.5 as built: the turn edge of a channel bound to the member's own
        // chat stamps `context.speaker = 'member:<member_person>'`; this road
        // enters below the channel, so it stamps what that edge would. A group
        // channel stamps none.
        ctx["speaker"] = json!("member:owner");
    }
    MessageBuilder::new(Path::new("/assistants"))
        .hop(road::as_map(&json!({"route": "in_turn"})))
        .context(road::as_map(&ctx))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": format!("{text} {MARK}{step})")}
        ]})))
        .ttl(MESSAGE_DEFAULT_TTL - road::ABOVE_THE_CONTAINER)
        .build()
}

fn said(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => v["messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|t| t["text"].as_str().map(str::to_string))
            .collect::<Vec<_>>()
            .join("\n"),
        Body::Blob(_) => String::new(),
    }
}

async fn answered(sink: &mut mpsc::Receiver<Message>, root: &std::path::Path, step: u32) {
    let needle = format!("(r951-{step})");
    let deadline = Instant::now() + DEADLINE;
    let mut other = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, sink.recv()).await {
            Ok(Some(m)) if said(&m).contains(&needle) => return,
            Ok(Some(m)) => other.push(said(&m)),
            _ => panic!(
                "turn {step}: no answer left the generation within {DEADLINE:?}; other \
                 answers: {other:?}\n{}",
                diagnosis(root)
            ),
        }
    }
}

// ─────────────────────────────────────────────────────────────── the log

/// One delivery the colony logged, with its body when it travelled inline.
#[derive(Clone, Debug)]
struct Logged {
    id: String,
    parent: Option<String>,
    from: String,
    to: String,
    hop: Value,
    context: Value,
    ttl: i64,
    body: Value,
}

impl Logged {
    fn route(&self) -> &str {
        self.hop["route"].as_str().unwrap_or_default()
    }

    fn say(&self) -> String {
        format!(
            "ttl={:3} {} -> {} [{}]",
            self.ttl,
            self.from,
            self.to,
            self.route()
        )
    }
}

fn logged(root: &std::path::Path) -> Vec<Logged> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare(
            "SELECT id, parent_message_id, from_path, to_path, headers, ttl, body_kind, \
             body_payload FROM message_log ORDER BY rowid",
        )
        .expect("message_log");
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            r.get::<_, i64>(5)?,
            r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            r.get::<_, Option<String>>(7)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(id, parent, from, to, headers, ttl, kind, payload)| {
        let h: Value = sj::from_str(&headers).unwrap_or(Value::Null);
        let body = match (kind.as_str(), payload) {
            ("inline", Some(p)) => sj::from_str(&p).unwrap_or(Value::Null),
            _ => Value::Null,
        };
        Logged {
            id,
            parent,
            from,
            to,
            hop: h["hop"].clone(),
            context: h["context"].clone(),
            ttl,
            body,
        }
    })
    .collect()
}

/// The parent chain of `d`, newest first.
fn chain(log: &[Logged], d: &Logged, max: usize) -> Vec<String> {
    let by_id: BTreeMap<&str, &Logged> = log.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut out = vec![d.say()];
    let mut at = d;
    while let Some(p) = at.parent.as_deref().and_then(|p| by_id.get(p)).copied() {
        out.push(p.say());
        at = p;
        if out.len() >= max {
            break;
        }
    }
    out
}

/// The newest brief the objects pushed for `ob` (the push cell's `candidate`).
fn pushed_brief(log: &[Logged], ob: &str) -> Option<String> {
    log.iter()
        .rev()
        .filter(|d| d.from == "/objects/push" && d.route() == "candidate")
        .find_map(|d| {
            d.body["candidates"]
                .as_array()?
                .iter()
                .find(|c| c["id"] == ob)?["text"]
                .as_str()
                .map(str::to_string)
        })
}

/// The ttl left at each door of the objects' road.
fn rests(log: &[Logged], pred: impl Fn(&Logged) -> bool) -> Vec<i64> {
    log.iter().filter(|&d| pred(d)).map(|d| d.ttl).collect()
}

fn door_rest(log: &[Logged]) -> Vec<i64> {
    rests(log, |d| d.to == "/objects" && d.route() == "thing_seen")
}

/// The failure message of a `ttl_expired`: what was left at the door
/// `./assistants -> ./objects` and further on, and the shallowest chain.
fn ttl_report(log: &[Logged], dead: &[(String, String, String, String)]) -> String {
    let expired: Vec<_> = dead.iter().filter(|d| d.0 == "ttl_expired").collect();
    // K.4 as built: a brief enters a curator as `in_candidate`.
    let candidate = rests(log, |d| {
        d.route() == "in_candidate" && d.to.contains("/curator")
    });
    let facts = rests(log, |d| {
        d.to == "/memory-hive" && d.route() == "in_query" && d.hop["recall_caller"] == "objects"
    });
    let back = rests(log, |d| d.to == "/objects" && d.route() == "in_facts");
    let shallowest = log
        .iter()
        .min_by_key(|d| d.ttl)
        .map(|d| chain(log, d, 80).join("\n  "))
        .unwrap_or_default();
    format!(
        "ttl_expired in the run. The door `./assistants -> ./objects` on `thing_seen` \
         restores no budget (GH #951): `thing_seen` arrived at /objects with ttl rest {:?} \
         of {MESSAGE_DEFAULT_TTL}; further on, `in_candidate` at a curator {candidate:?}, \
         the objects' `facts` at memory {facts:?}, `in_facts` at /objects {back:?}.\n\
         Expired: {expired:#?}\nThe shallowest delivery and its chain:\n  {shallowest}",
        door_rest(log)
    )
}

fn diagnosis(root: &std::path::Path) -> String {
    let log = logged(root);
    let seams: Vec<String> = log
        .iter()
        .filter(|d| SEAM_ROUTES.contains(&d.route()) || d.hop["section"] == "things")
        .map(Logged::say)
        .collect();
    format!(
        "Objects: {:#?}\nAliases: {:#?}\nFacts: {:#?}\nCandidates: {:#?}\nDead letters: \
         {:#?}\nSeam deliveries: {seams:#?}",
        space::rows(
            &root.join(OBJECTS_DB),
            "SELECT id, rev, state, seen, turns, audience_set, aliases FROM objects \
             ORDER BY id, rev"
        ),
        space::rows(
            &root.join(MEMORY_DB),
            "SELECT alias, canonical FROM subject_aliases"
        ),
        space::rows(
            &root.join(MEMORY_DB),
            &format!(
                "SELECT subject, canonical_subject, audience_set FROM facts \
                 WHERE claim LIKE '%{PLACE}%'"
            )
        ),
        space::rows(
            &root.join(LEDGER),
            "SELECT source, cand_id, audience_set FROM candidates"
        ),
        space::dead_letters(root),
    )
}

/// Wait until `probe` answers; a `ttl_expired` fails at once with the rest
/// at the doors, the deadline with what the stores and the log hold.
async fn until<T>(root: &std::path::Path, what: &str, probe: impl Fn() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(v) = probe() {
            return v;
        }
        let dead = space::dead_letters(root);
        if dead.iter().any(|d| d.0 == "ttl_expired") {
            panic!("{what}: {}", ttl_report(&logged(root), &dead));
        }
        assert!(
            Instant::now() < deadline,
            "{what}: not within {DEADLINE:?}.\n{}",
            diagnosis(root)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn canonical(round: &str) -> String {
    let mut v: Vec<String> = sj::from_str(round).expect("a round is a JSON list");
    v.sort();
    v.dedup();
    sj::to_string(&v).expect("serialise")
}

fn is_object_id(s: &str) -> bool {
    s.strip_prefix("ob-")
        .is_some_and(|h| h.len() == 12 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The two `object_confirm` calls at the member's door into the objects: each
/// with the round of its turn as `audience_now`, the typed surface as
/// `tool_caller`, and the bound channel's speaker on the owner's turn only;
/// both answered by the objects' tools, each result home at the typed surface.
fn the_tool_calls_crossed_the_member_door(log: &[Logged], round: &str) {
    let calls: Vec<&Logged> = log
        .iter()
        .filter(|d| d.to == "/objects" && d.route() == "in_tool")
        .collect();
    assert_eq!(calls.len(), 2, "two calls at the objects' door: {calls:#?}");
    for d in &calls {
        let id = d.hop["tool_call_id"].as_str().unwrap_or_default();
        assert_eq!(d.hop["tool_name"], json!("object_confirm"), "{d:?}");
        assert_eq!(
            d.context["tool_caller"],
            json!("talky-chat"),
            "the level stamps the surface: {d:?}"
        );
        let now = d.context["audience_now"].as_str().unwrap_or_default();
        assert_eq!(
            canonical(now),
            round,
            "the member's door stamps the turn's round as `audience_now`: {d:?}"
        );
        let speaker = d.context["speaker"].as_str().unwrap_or_default();
        if id == call_id(BY_THE_OWNER) {
            assert_eq!(
                speaker, "member:owner",
                "the bound channel's speaker reaches the objects through the assistant \
                 level: {d:?}"
            );
        } else {
            assert_eq!(id, call_id(BY_NOBODY), "{d:?}");
            assert_eq!(speaker, "", "a turn without a speaker carries none: {d:?}");
        }
    }
    let results: Vec<&str> = log
        .iter()
        .filter(|d| d.from == "/objects/tools" && d.route() == "tool_result")
        .filter_map(|d| d.hop["tool_call_id"].as_str())
        .collect();
    assert_eq!(
        results.len(),
        2,
        "the objects' tools answer both calls: {results:?}"
    );
    let home: Vec<&str> = log
        .iter()
        .filter(|d| d.to.ends_with("/talky-chat") && d.route() == "in_tool")
        .filter_map(|d| d.hop["tool_call_id"].as_str())
        .collect();
    for step in [BY_THE_OWNER, BY_NOBODY] {
        assert!(
            home.contains(&call_id(step).as_str()),
            "the result of {} comes home to the typed surface: {home:?}",
            call_id(step)
        );
    }
}

/// Every delivery out of a code cell of the objects hive carries a route its
/// contract declares (`contract.emits` in the run, not on paper).
fn objects_emit_within_contract(log: &[Logged]) {
    for cell in ["gate", "push", "brief", "source", "tools", "schemas"] {
        let cfg = road::read_json(&road::repo(&format!(
            "templates/objects/{cell}/config.json"
        )));
        let Some(values) = cfg["contract"]["emits"]["hop"]["route"]["values"].as_array() else {
            continue;
        };
        let declared: Vec<&str> = values.iter().filter_map(Value::as_str).collect();
        let from = format!("/objects/{cell}");
        for d in log.iter().filter(|d| d.from == from) {
            assert!(
                declared.contains(&d.route()),
                "{from} emitted `{}`, which its contract does not declare ({declared:?}): {}",
                d.route(),
                d.say()
            );
        }
    }
}

/// Deliveries that say the objects' traffic was refused somewhere.
fn refusals(log: &[Logged]) -> Vec<String> {
    let set = |v: &Value| v.as_str().is_some_and(|c| !c.is_empty());
    let listed = |v: &Value| v.as_array().is_some_and(|r| !r.is_empty());
    log.iter()
        .filter(|d| {
            let at_assistants = d.to.starts_with("/assistants");
            // A memory refusal of the objects' question must go home to the
            // objects, never to the assistants (member edge 6).
            let misrouted = at_assistants && d.hop["recall_caller"] == "objects";
            // The one expected refusal: `owner_only` for the call of a turn
            // without a speaker (point 5).
            let expected = d.route() == "in_tool"
                && d.hop["tool_call_id"] == call_id(BY_NOBODY).as_str()
                && d.hop["error_code"] == "owner_only";
            let objects_error = at_assistants
                && d.from.starts_with("/objects")
                && set(&d.hop["error_code"])
                && !expected;
            // K.1 as built: `candidate_ack` {source, stored, refused[]}.
            let brief_refused = d.route() == "candidate_ack" && listed(&d.body["refused"]);
            // M.1 as built (OR-BC-75): `alias_ack` {done, refused: [{alias,
            // error_code}]}, hop `error_code`.
            let alias_refused = d.route() == "alias_ack"
                && (set(&d.hop["error_code"]) || listed(&d.body["refused"]));
            // M.2 as built (OR-BC-66): recall refuses `in_query {subject}` on
            // the route `reject` with the code in `hop.reject_reason`; a store
            // refusal stamps `error_code` -- read both (OR-BC.O.2).
            let facts_refused = d.to == "/objects"
                && d.route() == "in_facts"
                && (set(&d.hop["error_code"]) || set(&d.hop["reject_reason"]));
            misrouted || objects_error || brief_refused || alias_refused || facts_refused
        })
        .map(|d| format!("{} hop={} body={}", d.say(), d.hop, d.body))
        .collect()
}

// ─────────────────────────────────────────────────────────────── the lock

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_alias_pushes_the_brief() {
    if !(road::shipped() && space::shipped()) {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    // Before GH #951 the member holds no objects: red here, not skipped.
    assert!(
        road::repo("templates/member/objects/config.json").is_file(),
        "the member carries no `./objects` holder (GH #951)"
    );
    the_generation_lets_sightings_out_and_briefs_in();

    let brain = Brain::start().await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(road::BACKGROUND_REPLY, "stop")]).await;
    let embed = space::embed_stub();
    let stubs = road::Stubs {
        scripted: HashMap::from([(BRAIN.to_string(), brain.url.clone())]),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    build(&td, &stubs, &embed.url);
    let (h, ports) = road::boot(&td).await;
    let road::Ports { mut sink, mut park } = ports;
    tokio::spawn(async move { while park.recv().await.is_some() {} });
    let round_a = canonical(ROUND_A);

    // 1. Three turns of one round name the thing; the third makes it active.
    for step in 1..=3 {
        h.send(person(
            step,
            "We have a new kettle in the kitchen.",
            ROUND_A,
            CHANNEL_A,
            true,
        ))
        .await;
        answered(&mut sink, root, step).await;
        until(
            root,
            &format!("turn {step}: the objects count the sighting"),
            || {
                let seen = space::rows(
                    &root.join(OBJECTS_DB),
                    &format!("SELECT max(seen) FROM objects WHERE aliases LIKE '%{THING}%'"),
                );
                seen.first()?
                    .first()?
                    .parse::<u32>()
                    .ok()
                    .filter(|n| *n >= step)
            },
        )
        .await;
    }
    let (ob, learned_in) = until(root, "three sightings make an active row", || {
        let r = space::rows(
            &root.join(OBJECTS_DB),
            &format!(
                "SELECT id, audience_set FROM objects WHERE state = 'active' \
                 AND aliases LIKE '%{THING}%' ORDER BY rev DESC LIMIT 1"
            ),
        );
        r.first().map(|r| (r[0].clone(), r[1].clone()))
    })
    .await;
    assert!(is_object_id(&ob), "an object's id is `ob-<12 hex>`: {ob}");
    OB.set(ob.clone()).expect("one object per run");
    assert_eq!(
        learned_in, round_a,
        "the row keeps the round it was learned in, canonical"
    );
    let indexed_in = until(root, "the graph space indexes the object", || {
        let r = space::rows(
            &space::graph_db(root),
            &format!(
                "SELECT audience_set FROM sources WHERE source = '{ob}' \
                 AND version != '' AND version = announced"
            ),
        );
        r.first().map(|r| r[0].clone())
    })
    .await;
    assert_eq!(
        indexed_in, round_a,
        "the graph space keeps the object's round on its source"
    );

    // 2. The aliases reach memory; the brief reaches the typed surface's curator.
    until(root, "the object's alias reaches memory", || {
        // M.1 as built: memory keeps an alias in `subject_aliases (alias, canonical)`.
        let r = space::rows(
            &root.join(MEMORY_DB),
            &format!("SELECT alias FROM subject_aliases WHERE canonical = '{ob}'"),
        );
        (!r.is_empty()).then_some(())
    })
    .await;
    until(root, "the objects push the brief", || {
        pushed_brief(&logged(root), &ob)
    })
    .await;
    until(root, "the typed surface's curator keeps the brief", || {
        // K.1 as built: the curator's ledger keeps a pushed candidate in
        // `candidates` (source, cand_id, ..., audience_set).
        let r = space::rows(
            &root.join(LEDGER),
            &format!("SELECT cand_id FROM candidates WHERE cand_id = '{ob}'"),
        );
        (!r.is_empty()).then_some(())
    })
    .await;

    // A fact said on a later turn is filed under the object.
    h.send(person(
        4,
        "I put it away after breakfast.",
        ROUND_A,
        CHANNEL_A,
        true,
    ))
    .await;
    answered(&mut sink, root, 4).await;
    until(
        root,
        "turn 4: the new fact is filed under the object",
        || {
            // M.1 as built: a fact written after the alias derives `canonical_subject`
            // through `subject_aliases` (the store owns the column), and the one
            // `canonicalize` of `in_alias` re-derives an older one.
            let r = space::rows(
                &root.join(MEMORY_DB),
                &format!("SELECT canonical_subject FROM facts WHERE claim LIKE '%{PLACE}%'"),
            );
            r.iter().any(|r| r[0] == ob).then_some(())
        },
    )
    .await;

    // 3. The alias in the person's words pushes the brief.
    let brief = pushed_brief(&logged(root), &ob).expect("the brief was pushed above");
    assert!(
        brief.starts_with(&format!("thing: {THING}")),
        "a brief opens with `<type>: <name>` (O.3): {brief:?}"
    );
    h.send(person(
        5,
        &format!("Where is the {ALIAS_SAID} now?"),
        ROUND_A,
        CHANNEL_A,
        true,
    ))
    .await;
    answered(&mut sink, root, 5).await;
    let wire = brain.request_of(5).await;
    // K.2 as built (OR-BC.K.2): the push is ONE part `[<text>; <text>]`, the
    // result of a `memory_recall` pair `./push` hands `./intake` on
    // `in_addendum` -- a tool message of the request.
    let part = format!("[{brief}");
    let carrying = parts_with(&wire, &part);
    assert!(
        !carrying.is_empty(),
        "turn 5 names `{ALIAS_SAID}` in the round the object was learned in, yet the brain's \
         request carries no push part `{part}...`. The request:\n{}",
        sketch(&wire)
    );
    eprintln!("gh951 brief {brief:?} pushed in turn 5 at {carrying:?}");

    // 4. A round the row does not cover gets no brief.
    h.send(person(
        6,
        &format!("Is the {ALIAS_SAID} still in the cupboard?"),
        ROUND_B,
        CHANNEL_B,
        false,
    ))
    .await;
    answered(&mut sink, root, 6).await;
    let wire = brain.request_of(6).await;
    let mut leaked = parts_with(&wire, &brief);
    leaked.extend(parts_with(&wire, &format!("thing: {THING}")));
    assert!(
        leaked.is_empty(),
        "turn 6 is in a round with one more person ({ROUND_B}), which the object's round \
         does not cover, yet its request carries the brief at {leaked:?}:\n{}",
        sketch(&wire)
    );

    // 5. The owner confirms through the tools; a turn without a speaker may not.
    for (step, bound) in [(BY_THE_OWNER, true), (BY_NOBODY, false)] {
        h.send(person(
            step,
            &format!("Please keep the {ALIAS_SAID} on your list."),
            ROUND_A,
            CHANNEL_A,
            bound,
        ))
        .await;
        answered(&mut sink, root, step).await;
        let wire = brain.request_of(step).await;
        let result = tool_result_in(&wire, &call_id(step));
        if bound {
            assert_eq!(
                result["ok"],
                json!(true),
                "turn {step}: the owner's `object_confirm` comes back ok: {result}"
            );
        } else {
            assert_eq!(
                result["error"]["code"],
                json!("owner_only"),
                "turn {step}: a turn without a speaker may not confirm: {result}"
            );
        }
    }

    // 6. Clean.
    space::quiet(root).await;
    h.shutdown().await;
    let log = logged(root);
    the_tool_calls_crossed_the_member_door(&log, &round_a);
    let dead = space::dead_letters(root);
    let door = door_rest(&log);
    eprintln!("gh951 door thing_seen: ttl at /objects = {door:?} (of {MESSAGE_DEFAULT_TTL})");
    assert!(
        dead.iter().all(|d| d.0 != "ttl_expired"),
        "{}",
        ttl_report(&log, &dead)
    );
    assert!(dead.is_empty(), "no dead letter in the run: {dead:#?}");
    let refused = refusals(&log);
    assert!(
        refused.is_empty(),
        "the objects' traffic was refused or misrouted: {refused:#?}"
    );
    objects_emit_within_contract(&log);
}
