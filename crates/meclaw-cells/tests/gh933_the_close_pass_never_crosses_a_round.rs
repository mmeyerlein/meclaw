//! GH #933 -- the close pass never crosses a round.
//!
//! WHY this file exists: the close pass reads a session WHOLE, and a session is
//! not one round. A member who spoke with one agent and then with another left
//! turns under two different `audience_set`s in one session, and a turn the
//! writer could not attribute at all may sit beside them. Until this issue the
//! pass rendered all of them into ONE prompt, offered every open fact of the
//! session next to them, and wrote whatever came back under the round of the
//! REQUEST. Three leaks follow from that one shape, and each is a way the pass
//! can look finished and be wrong:
//!
//! * a fact learned in round `{e,b}` is written with the audience `{e,a}`, so
//!   `a` later reads what only `b` was told;
//! * a fact the model files under the wrong turn (the newest, the first) takes
//!   that turn's audience instead of the audience of the words it came from;
//! * a correction proposed while reading round `{e,b}` replaces a fact of round
//!   `{e,a}` -- the record of one round rewritten by what another round said.
//!
//! The lock is therefore stated at the RECEIVER, never at a script: a colony
//! of the shipped memory hive is booted once, every `llm` cell on a local stub,
//! and the store is seeded directly (an episode without an audience is refused
//! by the writer since GH #244, so the only way such a row exists is the way it
//! exists in an old store -- already there). Per case one session: two turns
//! `EA1`/`EA2` of round `{e,a}` (the second spoken by `agent:a`), two turns
//! `EB1`/`EB2` of round `{e,b}` (the second spoken by `agent:b`), one turn `EN`
//! without a round, one open fact `F-EA` on `EA1` and one open topic per round.
//! Two turns per round are what makes the filing policies differ: the stub
//! closer answers every prompt with an `add` per marked turn, filed under the
//! source turn, the newest turn of the prompt, the first turn of the prompt --
//! three different turns of ONE round, so a fact misfiled inside its round
//! keeps that round -- or the matching turn of the OTHER round, which the pass
//! must refuse. Every prompt also carries a correction of `F-EA`, a closure of
//! the other round's topic, and (policy "source") a closure of its own topic,
//! (policy "foreign") a closure of its own topic at the other round's turn.
//! The matrix is 6 episode orders x 4 filing policies x 2 request rounds.
//!
//! What must hold in every case, measured in the `facts` and `topics` tables,
//! at the stub and in the colony's own `message_log`:
//!
//! (a) every fact a marked turn produced is readable by exactly the rounds that
//!     could read that turn (the set rule of `templates/affinity/README.md`,
//!     section "An audience is a SET, not a name"), over the whole power set of
//!     `{e,a,b}`; a fact filed under the other round's turn is never written;
//! (b) the turn without a round reaches no prompt and no fact;
//! (c) `F-EA` is offered only to its own round, and nothing from round `{e,b}`
//!     replaces it;
//! (d) one closer call per round of the session -- two, one per round;
//! (e) the report says so: `groups == 2`, `unaudienced == 1`, and every
//!     reference across a round is counted in `unseen_refs`;
//! (f) the lane fits its budget: the routing decisions one pass spends stay
//!     within `MESSAGE_DEFAULT_TTL - 16` (the reserve of
//!     `gh929_every_budget_segment_fits_its_reserve.rs`), and every case prints
//!     `gh933 close lane: start=<ttl> end=<ttl> used=<n> groups=<g>`;
//! (g) a prompt holds ONE round: no text, no episode id, no speaker, no topic
//!     name or id of the other round, measured on the prompt the stub recorded;
//! (h) a topic is closed only by its own round, at a turn of its own round.
//!
//! Free of a real provider by construction: every `llm` cell of the hive and
//! its embedder talk to one local stub. Guarded like every template-reading
//! test (GH #49): a tree without the hive is skipped, never judged.

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MESSAGE_DEFAULT_TTL, Message, MessageBuilder, Path};
use meclaw_testing::mock_http::{
    CapturedRequest, MockResponse, RequestValidator, start_mock_server_capturing_with_validator,
};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);

/// One wave of passes. `close-glue` runs ONE invocation at a time
/// (`max_concurrency: 1`), so the passes of a wave queue behind each other;
/// the bound is a failure marker for a wave that never reports, not a timing.
const WAVE_DEADLINE: Duration = Duration::from_secs(180);

/// What a segment keeps back from the colony budget (OR-BD-11), the number of
/// `gh929_every_budget_segment_fits_its_reserve.rs`.
const RESERVE: u32 = 16;

/// The most routing decisions one close pass may spend.
const LANE_MAX: i64 = (MESSAGE_DEFAULT_TTL - RESERVE) as i64;

/// The budget a pass enters the hive with: the colony default, the way a fresh
/// root carries it.
const START_TTL: u32 = MESSAGE_DEFAULT_TTL;

const CLOSER_MODEL: &str = "closer-stub";
const BACKGROUND_MODEL: &str = "background-stub";
const BACKGROUND_REPLY: &str = "A short account of what was said.";

const HIVE: &str = "/memory-hive";
const CHANNEL: &str = "c-933";
const PREDICATE: &str = "has_code";

/// The three participants, in the affinity vocabulary.
const PARTICIPANTS: [&str; 3] = ["member:e", "agent:a", "agent:b"];

/// The two rounds a pass can be requested under.
const ROUND_EA: &str = r#"["agent:a","member:e"]"#;
const ROUND_EB: &str = r#"["agent:b","member:e"]"#;

/// One seeded episode: tag, marker in its text, audience column (`None` is a
/// row the writer would refuse today and an old store still holds), who spoke.
struct Seeded {
    tag: &'static str,
    marker: &'static str,
    audience: Option<&'static str>,
    sender: &'static str,
    speaker: &'static str,
}

const EPISODES: [Seeded; 5] = [
    Seeded {
        tag: "ea1",
        marker: "SECRET-EA1",
        audience: Some(r#"["agent:a", "member:e"]"#),
        sender: "user",
        speaker: "member:e",
    },
    Seeded {
        tag: "ea2",
        marker: "SECRET-EA2",
        audience: Some(r#"["agent:a", "member:e"]"#),
        sender: "assistant",
        speaker: "agent:a",
    },
    Seeded {
        tag: "eb1",
        marker: "SECRET-EB1",
        audience: Some(r#"["agent:b", "member:e"]"#),
        sender: "user",
        speaker: "member:e",
    },
    Seeded {
        tag: "eb2",
        marker: "SECRET-EB2",
        audience: Some(r#"["agent:b", "member:e"]"#),
        sender: "assistant",
        speaker: "agent:b",
    },
    Seeded {
        tag: "en",
        marker: "SECRET-EN",
        audience: None,
        sender: "user",
        speaker: "member:e",
    },
];

/// The markers of the audienced turns; none is a prefix of another.
const MARKERS: [&str; 5] = [
    "SECRET-EA1",
    "SECRET-EA2",
    "SECRET-EB1",
    "SECRET-EB2",
    "SECRET-EN",
];

/// The two rounds of a session by tag: (round, its turns, its agent).
const ROUNDS: [(&str, [&str; 2], &str); 2] = [
    ("ea", ["ea1", "ea2"], "agent:a"),
    ("eb", ["eb1", "eb2"], "agent:b"),
];

/// The other round of a session.
fn other(round: &str) -> &'static str {
    if round == "ea" { "eb" } else { "ea" }
}

/// The round an episode tag or a marker belongs to (`ea1` and `SECRET-EA1`
/// both say `ea`); `None` for the turn without one.
fn round_of(tag_or_marker: &str) -> Option<&'static str> {
    let t = tag_or_marker
        .trim_start_matches("SECRET-")
        .to_ascii_lowercase();
    ROUNDS
        .iter()
        .find(|(r, _, _)| t.starts_with(r))
        .map(|(r, _, _)| *r)
}

/// Every order of the five episodes in time that the matrix walks:
/// `ORDERS[p][k]` is the rank of `EPISODES[k]`. Rounds in a row, rounds
/// interleaved, the round-less turn first, in the middle and last, and orders
/// where the FIRST turn of a round is not its oldest-numbered one.
const ORDERS: [[u32; 5]; 6] = [
    [0, 1, 2, 3, 4],
    [2, 3, 0, 1, 4],
    [0, 2, 1, 3, 4],
    [1, 2, 3, 4, 0],
    [0, 3, 1, 4, 2],
    [4, 1, 3, 0, 2],
];

/// Which turn the stub closer files an `add` under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Policy {
    /// The turn the marker stands in.
    Source,
    /// The newest turn of the prompt.
    Newest,
    /// The first turn the prompt names.
    First,
    /// The matching turn of the OTHER round (`EA1` for `EB1`): an id the
    /// prompt never showed.
    Foreign,
}

const POLICIES: [Policy; 4] = [
    Policy::Source,
    Policy::Newest,
    Policy::First,
    Policy::Foreign,
];

/// One session of the matrix.
#[derive(Clone, Debug)]
struct Case {
    sid: String,
    order: usize,
    policy: Policy,
    round: &'static str,
}

impl Case {
    fn episode_id(&self, tag: &str) -> String {
        format!("ep-{}-{tag}", self.sid)
    }

    /// The episode tag of an episode id of this case.
    fn tag_of<'a>(&self, id: &'a str) -> Option<&'a str> {
        id.strip_prefix(&format!("ep-{}-", self.sid))
    }

    fn topic_id(&self, round: &str) -> String {
        format!("t-{}-{round}", self.sid)
    }

    fn topic_name(&self, round: &str) -> String {
        format!("TOPIC-{}-{}", round.to_ascii_uppercase(), self.sid)
    }

    fn fea(&self) -> String {
        format!("f-{}-ea", self.sid)
    }

    fn fea_subject(&self) -> String {
        format!("fea item {}", self.sid)
    }

    fn correction(&self) -> String {
        format!("corrected {}", self.sid)
    }

    fn happened_at(&self, k: usize) -> String {
        format!("2026-10-01T10:00:0{}.000Z", ORDERS[self.order][k])
    }
}

fn matrix() -> Vec<Case> {
    let mut out = Vec::new();
    for (r, round) in [ROUND_EA, ROUND_EB].into_iter().enumerate() {
        for (p, policy) in POLICIES.into_iter().enumerate() {
            for order in 0..ORDERS.len() {
                out.push(Case {
                    sid: format!("s933-o{order}-p{p}-r{r}"),
                    order,
                    policy,
                    round,
                });
            }
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────── the rule

/// The participant set of an `audience_set` value, or `None` where the value
/// is no set at all (NULL, `""`, `[]`, not JSON, not a list of strings).
fn audience(raw: &str) -> Option<BTreeSet<String>> {
    let v: Value = meclaw_core::serde_json::from_str(raw).ok()?;
    let set: BTreeSet<String> = v
        .as_array()?
        .iter()
        .filter_map(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    (!set.is_empty()).then_some(set)
}

/// Whether the round `round` may read a row whose audience column is `raw`.
///
/// The set rule of `templates/affinity/README.md` ("An audience is a SET, not
/// a name", GH #154): a row is readable iff it is addressed to `*` or the
/// CURRENT round is a subset of its audience. Fail-closed: a row without a set
/// is read by nobody.
fn reads(round: &BTreeSet<String>, raw: &str) -> bool {
    match audience(raw) {
        Some(set) => set.contains("*") || round.is_subset(&set),
        None => false,
    }
}

/// Every non-empty round over the three participants.
fn every_round() -> Vec<BTreeSet<String>> {
    (1u32..(1 << PARTICIPANTS.len()))
        .map(|mask| {
            PARTICIPANTS
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, p)| p.to_string())
                .collect()
        })
        .collect()
}

fn marker_in(text: &str) -> Option<&'static str> {
    MARKERS.into_iter().find(|m| text.contains(m))
}

// ─────────────────────────────────────────────────────────────── the tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/memory-hive/config.json",
        "templates/memory-hive/close-glue/config.json",
        "templates/memory-hive/closer/config.json",
        "templates/memory-hive/extract-glue/config.json",
        "templates/memory-hive/store/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

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

/// The shipped template, copied the way instantiation lays it out (the
/// `copy_resolved` of `gh895`): a `cell.type: "ref"` directory is replaced by
/// the referenced template and its `override_params` applied (GH #140, #277).
fn copy_resolved(src: &std::path::Path, dst: &std::path::Path, depth: usize) {
    assert!(
        depth < 8,
        "template ref chain does not terminate at {}",
        src.display()
    );
    let marker = src.join("config.json");
    if marker.is_file() {
        let cfg = read_json(&marker);
        if cfg["cell"]["type"] == "ref" {
            let reference = cfg["cell"]["template"]
                .as_str()
                .unwrap_or_else(|| panic!("{}: a ref names a template", marker.display()));
            let name = reference.split('@').next().unwrap_or_default();
            copy_resolved(&repo("templates").join(name), dst, depth + 1);
            if let Some(over) = cfg["override_params"].as_object() {
                for (cell, params) in over {
                    override_params_on_disk(&dst.join(cell), params);
                }
            }
            return;
        }
    }
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_resolved(&from, &dst.join(name), depth);
        } else if name == "config.json"
            || (src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl"))
        {
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

fn configs_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "seed") {
                configs_under(&p, out);
            }
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// Every `llm` cell of the tree on the one stub: the closer under its own
/// model name (that is how the stub tells its calls apart), all others under
/// the background one.
fn point_llms_at_stub(main: &std::path::Path, stub: &str) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "llm" {
            continue;
        }
        let closer = f
            .parent()
            .is_some_and(|d| d.ends_with("memory-hive/closer"));
        cfg["params"]["base_url"] = json!(stub);
        cfg["params"]["model"] = json!(if closer {
            CLOSER_MODEL
        } else {
            BACKGROUND_MODEL
        });
        cfg["params"]["api_key"] = json!("sk-test");
        write_json(&f, &cfg);
    }
}

/// Every timer of the tree, out of the run's way (the sweep of `gh895`).
fn quiet_timers(main: &std::path::Path) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut n: u64 = 0;
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "timer" {
            continue;
        }
        let Some(schedules) = cfg["params"]["schedules"].as_array_mut() else {
            continue;
        };
        for s in schedules.iter_mut() {
            n += 1;
            if s["schedule_id"]
                .as_str()
                .is_some_and(|id| id.contains("${"))
            {
                s["schedule_id"] =
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0933_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// The run's own environment file: every `${VAR}` bound to a dummy, the
/// memory's model and embedder endpoints to the stub.
fn write_env(root: &std::path::Path, main: &std::path::Path, stub: &str) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for f in files {
        let raw = std::fs::read_to_string(&f).unwrap_or_default();
        let mut rest = raw.as_str();
        while let Some(start) = rest.find("${") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            let name = &rest[..end];
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                vars.insert(name.to_string(), format!("dummy-{name}"));
            }
            rest = &rest[end + 1..];
        }
    }
    vars.insert("OPENROUTER_API_KEY".into(), "test-key".into());
    vars.insert("MODEL_CLOSER".into(), CLOSER_MODEL.into());
    vars.insert("MEMORY_LLM_BASE_URL".into(), stub.to_string());
    vars.insert(
        "MEMORY_EMBED_ENDPOINT".into(),
        format!("{stub}/v1/embeddings"),
    );
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// The lanes a template declares at its own path (an `at` lane docks
/// elsewhere, ADR-0020).
fn rim_emits(template: &str) -> Vec<String> {
    read_json(&repo(&format!("templates/{template}/config.json")))["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|l| match &l["at"] {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            _ => false,
        })
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect()
}

/// The store's own seed files (`seed/<table>.jsonl`, loaded once on the fresh
/// `cell.db`). The header is the table's declared schema, read off the copied
/// store config, so a column the hive adds later is covered without an edit
/// here. This is the direct seeding the writer would refuse for `EN`.
fn seed_store(store: &std::path::Path, cases: &[Case]) {
    let schema = read_json(&store.join("config.json"))["params"]["schema"].clone();
    let mut episodes = vec![json!({"schema": schema["episodes"].clone()})];
    let mut facts = vec![json!({"schema": schema["facts"].clone()})];
    let mut topics = vec![json!({"schema": schema["topics"].clone()})];
    for case in cases {
        for (round, turns, _) in ROUNDS {
            let k = EPISODES
                .iter()
                .position(|e| e.tag == turns[0])
                .expect("a seeded turn");
            topics.push(json!({
                "id": case.topic_id(round),
                "session_id": case.sid,
                "channel": CHANNEL,
                "audience_set": EPISODES[k].audience,
                "name": case.topic_name(round),
                "opened_episode_id": case.episode_id(turns[0]),
                "closed_episode_id": "",
                "opened_at": case.happened_at(k),
                "closed_at": "",
                "closure_source": "",
            }));
        }
        for (k, ep) in EPISODES.iter().enumerate() {
            let at = case.happened_at(k);
            episodes.push(json!({
                "id": case.episode_id(ep.tag),
                "session_id": case.sid,
                "turn_id": format!("{}#{k}", case.sid),
                "sender": ep.sender,
                "speaker": ep.speaker,
                "channel": CHANNEL,
                "audience_set": ep.audience,
                "content": format!("{}: the code word of {} is said here.", ep.marker, case.sid),
                "happened_at": at,
                "recorded_at": at,
            }));
        }
        let ea_at = case.happened_at(0);
        let subject = case.fea_subject();
        let claim = format!("original {}", case.sid);
        facts.push(json!({
            "id": case.fea(),
            "episode_id": case.episode_id("ea1"),
            "session_id": case.sid,
            "channel": CHANNEL,
            "audience_set": EPISODES[0].audience,
            "subject": subject,
            "canonical_subject": subject,
            "predicate": PREDICATE,
            "canonical_predicate": PREDICATE,
            "claim": claim,
            "canonical_claim": claim,
            "claim_hash": "",
            "fact_kind": "world",
            "valid_from": ea_at,
            "valid_until": null,
            "recorded_at": ea_at,
            "expired_at": null,
            "superseded_by": null,
            "closure_source": "",
            "confidence": 70,
            "source": "",
        }));
    }
    let lines = |rows: &[Value]| -> String {
        rows.iter()
            .map(|r| meclaw_core::serde_json::to_string(r).expect("serialise") + "\n")
            .collect()
    };
    std::fs::create_dir_all(store.join("seed")).expect("mkdir seed");
    std::fs::write(
        store.join("seed/episodes.jsonl"),
        lines(episodes.as_slice()),
    )
    .expect("seed episodes");
    std::fs::write(store.join("seed/facts.jsonl"), lines(facts.as_slice())).expect("seed facts");
    std::fs::write(store.join("seed/topics.jsonl"), lines(topics.as_slice())).expect("seed topics");
}

/// The memory hive alone, its rim drained: the report to `/report`, every
/// other lane it may say to `/park`.
fn build(td: &tempfile::TempDir, stub: &str, cases: &[Case]) {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(&repo("templates/memory-hive"), &main.join("memory-hive"), 0);
    let mut edges = vec![json!({"from": "./memory-hive", "to": "/report",
                                "condition": "has(hop.route) && hop.route == 'close_report'"})];
    for lane in rim_emits("memory-hive") {
        if lane != "close_report" {
            edges.push(json!({"from": "./memory-hive", "to": "/park",
                              "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
        }
    }
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    quiet_timers(&main);
    point_llms_at_stub(&main, stub);
    write_env(root, &main, stub);
    seed_store(&main.join("memory-hive/store"), cases);
}

struct Ports {
    report: mpsc::Receiver<Message>,
    /// Held, not dropped: a capture whose receiver is gone turns every
    /// delivery into a send error.
    #[allow(dead_code)]
    park: mpsc::Receiver<Message>,
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, Ports) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (report_tx, report_rx) = mpsc::channel::<Message>(256);
    let (park_tx, park_rx) = mpsc::channel::<Message>(1024);
    h.spawn(Path::new("/report"), move || {
        CaptureCell::new(report_tx.clone())
    })
    .await;
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped memory hive must boot");
    (
        h,
        Ports {
            report: report_rx,
            park: park_rx,
        },
    )
}

/// The close pass of one session, requested under the case's round.
fn close_pass(case: &Case) -> Message {
    MessageBuilder::new(Path::new(HIVE))
        .hop(as_map(&json!({"route": "in_close_pass"})))
        .context(as_map(&json!({"session_id": case.sid, "channel": CHANNEL,
                                "audience_set": case.round})))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "id": "m1",
                                 "text": "close"}]}),
        ))
        .ttl(START_TTL)
        .build()
}

// ─────────────────────────────────────────────────────────────── the stub

/// One prompt the closer was handed.
#[derive(Clone, Debug)]
struct Prompt {
    session: String,
    text: String,
    turn_ids: Vec<String>,
}

fn chat_completion(content: &str, model: &str) -> MockResponse {
    let body = json!({
        "id": "chatcmpl-gh933",
        "model": model,
        "choices": [{
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    });
    MockResponse::ok_json(body.to_string().as_bytes())
}

fn content_of(m: &Value) -> String {
    match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// The JSON document of the closer's user message (tolerant of text around
/// it).
fn prompt_doc(text: &str) -> Value {
    if let Ok(v) = meclaw_core::serde_json::from_str::<Value>(text) {
        return v;
    }
    match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if a < b => {
            meclaw_core::serde_json::from_str(&text[a..=b]).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    }
}

/// The verdict the stub closer gives one prompt: an `add` per marked turn,
/// filed under the turn the case's policy names, a correction of `F-EA`
/// whatever the prompt offered, a closure of the OTHER round's topic, and --
/// by policy -- a closure of its own topic at its own or at a foreign turn.
fn verdict(doc: &Value, case: &Case) -> Value {
    let turns = doc["turns"].as_array().cloned().unwrap_or_default();
    let id_of = |t: &Value| t["episode_id"].as_str().unwrap_or("").to_string();
    let Some(round) = turns.iter().find_map(|t| {
        case.tag_of(t["episode_id"].as_str().unwrap_or(""))
            .and_then(round_of)
    }) else {
        return json!({"nothing_to_add": true, "add": [], "sharpen": [], "correct": [],
                      "close_topics": []});
    };
    let foreign = other(round);
    let newest = turns
        .iter()
        .max_by(|x, y| {
            x["at"]
                .as_str()
                .unwrap_or("")
                .cmp(y["at"].as_str().unwrap_or(""))
        })
        .map(id_of)
        .unwrap_or_default();
    let first = turns.first().map(id_of).unwrap_or_default();
    let mut add = Vec::new();
    for t in &turns {
        let Some(marker) = marker_in(t["text"].as_str().unwrap_or("")) else {
            continue;
        };
        let source = id_of(t);
        let episode_id = match case.policy {
            Policy::Source => source.clone(),
            Policy::Newest => newest.clone(),
            Policy::First => first.clone(),
            Policy::Foreign => {
                let tag = case.tag_of(&source).unwrap_or("");
                case.episode_id(&format!("{foreign}{}", tag.get(2..).unwrap_or("")))
            }
        };
        add.push(json!({"episode_id": episode_id,
                        "subject": format!("{marker} item {}", case.sid),
                        "predicate": PREDICATE, "claim": marker,
                        "fact_kind": "world", "confidence": 80}));
    }
    let mut close_topics = vec![json!({"id": case.topic_id(foreign), "ended_episode_id": first})];
    match case.policy {
        Policy::Source => {
            close_topics.push(json!({"id": case.topic_id(round), "ended_episode_id": first}));
        }
        Policy::Foreign => {
            close_topics.push(json!({"id": case.topic_id(round),
                                     "ended_episode_id": case.episode_id(&format!("{foreign}1"))}));
        }
        Policy::Newest | Policy::First => {}
    }
    json!({
        "nothing_to_add": false,
        "add": add,
        "sharpen": [],
        "correct": [{"fact_id": case.fea(), "subject": case.fea_subject(),
                     "predicate": PREDICATE, "claim": case.correction(),
                     "why": "a later turn corrected it"}],
        "close_topics": close_topics
    })
}

/// The stub: the closer's calls are answered per prompt and recorded; every
/// other call (the embedder above all) gets the background reply.
async fn start_stub(cases: &[Case]) -> (String, Arc<Mutex<Vec<Prompt>>>) {
    let by_sid: Arc<HashMap<String, Case>> =
        Arc::new(cases.iter().map(|c| (c.sid.clone(), c.clone())).collect());
    let prompts: Arc<Mutex<Vec<Prompt>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = prompts.clone();
    let validator: RequestValidator = Arc::new(move |req: &CapturedRequest| {
        let body: Value = meclaw_core::serde_json::from_slice(&req.body).ok()?;
        if body["model"] != CLOSER_MODEL {
            return None;
        }
        let text = body["messages"]
            .as_array()
            .and_then(|ms| ms.iter().rev().find(|m| m["role"] == "user"))
            .map(content_of)
            .unwrap_or_default();
        let doc = prompt_doc(&text);
        let session = doc["session_id"].as_str().unwrap_or("").to_string();
        let turn_ids = doc["turns"]
            .as_array()
            .map(|ts| {
                ts.iter()
                    .filter_map(|t| t["episode_id"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        seen.lock().expect("prompt log").push(Prompt {
            session: session.clone(),
            text,
            turn_ids,
        });
        let answer = match by_sid.get(&session) {
            Some(case) => verdict(&doc, case).to_string(),
            None => json!({"nothing_to_add": true, "add": [], "sharpen": [],
                           "correct": [], "close_topics": []})
            .to_string(),
        };
        Some(chat_completion(&answer, CLOSER_MODEL))
    });
    let (addr, _join, _captured) = start_mock_server_capturing_with_validator(
        vec![chat_completion(BACKGROUND_REPLY, BACKGROUND_MODEL)],
        Some(validator),
    )
    .await;
    (format!("http://{addr}"), prompts)
}

// ─────────────────────────────────────────────────────────────── reading

fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    if !db.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| {
                r.get::<_, Option<String>>(i)
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            })
            .collect::<Vec<String>>())
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .unwrap_or_default()
}

/// Until `sql` returns at least `n` rows in `db`. The wait is on an EVENT,
/// not on the clock: every delivery the colony logs and every row the store
/// adds is progress and opens a fresh window, and only a colony that does
/// nothing at all for `DEADLINE` fails (GH #987). Measured on lane build01,
/// release gate of 0.60.0 (2026-10-03): under the full run (9992 tests) the
/// lock took 137.5 s and the first `SECRET-EA1` fact was not in the store
/// within a fixed 30 s window after the last report, while the colony was
/// still writing the 36 sessions' facts behind it; alone it was green
/// (20/20 with its neighbours in 78 s). A fixed window measured the host's
/// load, not the hive.
async fn until_rows(db: &std::path::Path, sql: &str, n: usize, what: &str) {
    let mut seen = progress(db);
    let mut window = Instant::now() + DEADLINE;
    while rows(db, sql).len() < n {
        let now = progress(db);
        if now != seen {
            seen = now;
            window = Instant::now() + DEADLINE;
        }
        assert!(
            Instant::now() < window,
            "{what}: fewer than {n} row(s) of `{sql}`, and the colony made no \
             progress for {DEADLINE:?} (last seen: {seen:?})"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// What counts as the colony moving: the deliveries `colony.db` has logged
/// and the rows of the memory store's own tables. Either changing is an event.
fn progress(db: &std::path::Path) -> (i64, usize, usize) {
    let logged = db
        .ancestors()
        .find(|p| p.join("colony.db").exists())
        .and_then(|root| rusqlite::Connection::open(root.join("colony.db")).ok())
        .and_then(|c| {
            c.query_row("SELECT COALESCE(MAX(rowid), 0) FROM message_log", [], |r| {
                r.get::<_, i64>(0)
            })
            .ok()
        })
        .unwrap_or(0);
    (
        logged,
        rows(db, "SELECT id FROM facts").len(),
        rows(db, "SELECT id FROM episodes").len(),
    )
}

/// Until the facts table stops growing: the ingress writes AFTER the report
/// left, so a late leak must have had its chance to land before it is judged.
async fn until_quiet(db: &std::path::Path) {
    let deadline = Instant::now() + DEADLINE;
    let mut last = usize::MAX;
    let mut calm = 0;
    while calm < 4 {
        let now = rows(db, "SELECT id FROM facts").len();
        if now == last {
            calm += 1;
        } else {
            calm = 0;
            last = now;
        }
        assert!(
            Instant::now() < deadline,
            "the facts table did not settle within {DEADLINE:?}"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// A number on a hop, whether the cell wrote it as a number or as a string.
fn num(v: Option<&Value>) -> Option<u64> {
    match v? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// One fact row as the store holds it.
#[derive(Clone, Debug)]
struct Fact {
    id: String,
    episode_id: String,
    audience_set: String,
    subject: String,
    claim: String,
    superseded_by: String,
}

fn facts_of(db: &std::path::Path, sid: &str) -> Vec<Fact> {
    rows(
        db,
        &format!(
            "SELECT id, episode_id, COALESCE(audience_set, ''), COALESCE(subject, ''), \
             COALESCE(claim, ''), COALESCE(superseded_by, '') FROM facts \
             WHERE session_id = '{sid}' ORDER BY id"
        ),
    )
    .into_iter()
    .map(|r| Fact {
        id: r[0].clone(),
        episode_id: r[1].clone(),
        audience_set: r[2].clone(),
        subject: r[3].clone(),
        claim: r[4].clone(),
        superseded_by: r[5].clone(),
    })
    .collect()
}

/// `id -> (content, audience_set)` of every episode the store holds.
fn episodes(db: &std::path::Path) -> HashMap<String, (String, String)> {
    rows(
        db,
        "SELECT id, COALESCE(content, ''), COALESCE(audience_set, '') FROM episodes",
    )
    .into_iter()
    .map(|r| (r[0].clone(), (r[1].clone(), r[2].clone())))
    .collect()
}

/// `id -> (closed_at, closed_episode_id)` of every topic the store holds.
fn topics(db: &std::path::Path) -> HashMap<String, (String, String)> {
    rows(
        db,
        "SELECT id, COALESCE(closed_at, ''), COALESCE(closed_episode_id, '') FROM topics",
    )
    .into_iter()
    .map(|r| (r[0].clone(), (r[1].clone(), r[2].clone())))
    .collect()
}

/// `(trace_id, ttl, session_id)` of every delivery the colony logged.
fn deliveries(root: &std::path::Path) -> Vec<(String, i64, String)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT trace_id, ttl, headers FROM message_log ORDER BY rowid")
        .expect("message_log");
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(trace, ttl, headers)| {
        let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
        let sid = h["context"]["session_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        (trace, ttl, sid)
    })
    .collect()
}

/// `(sender, target, error_code)` of every dead letter of the run.
fn dead_letters(root: &std::path::Path) -> Vec<(String, String, String)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT sender_path, original_target, error_code FROM dead_letters ORDER BY id")
        .expect("dead_letters");
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .filter_map(Result::ok)
        .collect()
}

// ═══════════════════════════════════════════════════════════════════ lock

/// The issue's trace as a matrix: 6 episode orders x 3 filing policies x 2
/// request rounds, one session each, one colony for all of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_close_pass_never_crosses_a_round() {
    if !shipped() {
        eprintln!("gh933: the memory hive is not in this tree, skipped (GH #49)");
        return;
    }
    let cases = matrix();
    let (stub, prompts) = start_stub(&cases).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, &stub, &cases);
    let (h, mut ports) = boot(&td).await;
    let db = td.path().join("main/memory-hive/store/cell.db");

    // The passes, a wave of six at a time; every wave waits for its reports.
    let mut reports: HashMap<String, Vec<Map<String, Value>>> = HashMap::new();
    for wave in cases.chunks(6) {
        for case in wave {
            h.send(close_pass(case)).await;
        }
        let deadline = Instant::now() + WAVE_DEADLINE;
        while wave.iter().any(|c| !reports.contains_key(&c.sid)) {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(Some(m)) = tokio::time::timeout(left, ports.report.recv()).await else {
                let missing: Vec<&str> = wave
                    .iter()
                    .filter(|c| !reports.contains_key(&c.sid))
                    .map(|c| c.sid.as_str())
                    .collect();
                panic!("no close_report within {WAVE_DEADLINE:?} for {missing:?}");
            };
            let sid = m
                .headers
                .context
                .get("session_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            reports.entry(sid).or_default().push(m.headers.hop.clone());
        }
    }

    // The writes of a pass leave AFTER its report. Every marked turn that a
    // round could read lands as a fact -- unless the closer filed it under the
    // other round's turn; then the table settles.
    for case in cases.iter().filter(|c| c.policy != Policy::Foreign) {
        for marker in ["SECRET-EA1", "SECRET-EA2", "SECRET-EB1", "SECRET-EB2"] {
            until_rows(
                &db,
                &format!(
                    "SELECT id FROM facts WHERE session_id = '{}' AND claim LIKE '%{marker}%'",
                    case.sid
                ),
                1,
                &format!("{}: the {marker} turn is written", case.sid),
            )
            .await;
        }
    }
    until_quiet(&db).await;
    // A late report (a second one for a session) is a finding, not noise.
    while let Ok(m) = ports.report.try_recv() {
        let sid = m
            .headers
            .context
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        reports.entry(sid).or_default().push(m.headers.hop.clone());
    }

    let held = episodes(&db);
    let topics = topics(&db);
    let all_facts: HashMap<String, Vec<Fact>> = cases
        .iter()
        .map(|c| (c.sid.clone(), facts_of(&db, &c.sid)))
        .collect();
    let by_id: HashMap<String, Fact> = all_facts
        .values()
        .flatten()
        .map(|f| (f.id.clone(), f.clone()))
        .collect();
    let prompts: Vec<Prompt> = prompts.lock().expect("prompt log").clone();
    h.shutdown().await;
    let log = deliveries(td.path());
    let dead = dead_letters(td.path());

    let rounds = every_round();
    let ea_set = audience(ROUND_EA).expect("a set");
    let eb_set = audience(ROUND_EB).expect("a set");
    let mut findings: Vec<String> = Vec::new();

    for case in &cases {
        let sid = &case.sid;
        let facts = &all_facts[sid];
        let mine: Vec<&Prompt> = prompts.iter().filter(|p| &p.session == sid).collect();
        let en_id = case.episode_id("en");
        let ea_ids = [case.episode_id("ea1"), case.episode_id("ea2")];
        let fea = case.fea();

        // (a) a fact is never readable by a round that could not read its turn,
        // and a fact the closer filed under the other round is not written.
        for f in facts.iter().filter(|f| marker_in(&f.claim).is_some()) {
            let marker = marker_in(&f.claim).expect("filtered");
            if case.policy == Policy::Foreign {
                findings.push(format!(
                    "{sid} (a): {marker} was filed under the other round's turn and \
                     written anyway: {f:?}"
                ));
            }
            let Some((content, ep_aud)) = held.get(&f.episode_id) else {
                findings.push(format!("{sid} (a): {} hangs on no episode: {f:?}", f.id));
                continue;
            };
            let filed_round = case.tag_of(&f.episode_id).and_then(round_of);
            if case.policy == Policy::Source && !content.contains(marker) {
                findings.push(format!(
                    "{sid} (a): {marker} is filed under {} whose text is {content:?}",
                    f.episode_id
                ));
            }
            if filed_round != round_of(marker) {
                findings.push(format!(
                    "{sid} (a): {marker} is filed under {} of round {filed_round:?}",
                    f.episode_id
                ));
            }
            for r in &rounds {
                if reads(r, &f.audience_set) && !reads(r, ep_aud) {
                    findings.push(format!(
                        "{sid} (a): round {r:?} reads {marker} (audience {}) but not its \
                         turn {} (audience {ep_aud:?})",
                        f.audience_set, f.episode_id
                    ));
                }
            }
        }

        // (b) the turn without a round reaches no prompt and no fact.
        for p in &mine {
            if p.text.contains("SECRET-EN") || p.text.contains(&en_id) {
                findings.push(format!("{sid} (b): the unaudienced turn is in a prompt"));
            }
        }
        for f in facts {
            if f.claim.contains("SECRET-EN") || f.subject.contains("SECRET-EN") {
                findings.push(format!("{sid} (b): a fact carries SECRET-EN: {f:?}"));
            }
        }

        // (c) F-EA is offered to its own round only, and round {e,b} never
        // replaces it.
        let offered: Vec<&&Prompt> = mine.iter().filter(|p| p.text.contains(&fea)).collect();
        if offered.len() > 1 {
            findings.push(format!(
                "{sid} (c): F-EA is offered in {} prompts",
                offered.len()
            ));
        }
        for p in &offered {
            if p.turn_ids.iter().any(|t| !ea_ids.contains(t)) {
                findings.push(format!(
                    "{sid} (c): F-EA is offered beside turns {:?}",
                    p.turn_ids
                ));
            }
        }
        match facts.iter().find(|f| f.id == fea) {
            None => findings.push(format!("{sid} (c): F-EA is gone from the store")),
            Some(f) if !f.superseded_by.is_empty() => {
                let by = by_id.get(&f.superseded_by);
                let ok = by.is_some_and(|n| audience(&n.audience_set).as_ref() == Some(&ea_set));
                if !ok {
                    findings.push(format!(
                        "{sid} (c): F-EA is superseded by {:?} ({by:?}), not by a fact of \
                         round {{e,a}}",
                        f.superseded_by
                    ));
                }
            }
            Some(_) => {}
        }
        for f in facts {
            if f.claim == case.correction() && audience(&f.audience_set).as_ref() == Some(&eb_set) {
                findings.push(format!(
                    "{sid} (c): the correction of F-EA stands in round {{e,b}}: {f:?}"
                ));
            }
        }

        // (d) one closer call per round of the session.
        if mine.len() != 2 {
            findings.push(format!(
                "{sid} (d): {} closer request(s), expected 2",
                mine.len()
            ));
        }

        // (g) a prompt holds ONE round and nothing of the other: no text, no
        // id, no speaker, no topic. Measured on what the stub was handed.
        let mut asked: Vec<&str> = Vec::new();
        for p in &mine {
            let of: BTreeSet<Option<&str>> = p
                .turn_ids
                .iter()
                .map(|id| case.tag_of(id).and_then(round_of))
                .collect();
            let Some(&Some(round)) = of.iter().next().filter(|_| of.len() == 1) else {
                findings.push(format!(
                    "{sid} (g): a prompt mixes rounds: {:?}",
                    p.turn_ids
                ));
                continue;
            };
            asked.push(round);
            let foreign = other(round);
            let (_, foreign_turns, foreign_agent) = ROUNDS
                .iter()
                .find(|(r, _, _)| *r == foreign)
                .expect("a round");
            let mut leaks: Vec<String> = vec![
                format!("SECRET-{}", foreign.to_ascii_uppercase()),
                (*foreign_agent).to_string(),
                case.topic_name(foreign),
                case.topic_id(foreign),
            ];
            leaks.extend(foreign_turns.iter().map(|t| case.episode_id(t)));
            for leak in leaks {
                if p.text.contains(&leak) {
                    findings.push(format!(
                        "{sid} (g): the prompt of round {round} carries {leak:?} of round \
                         {foreign}"
                    ));
                }
            }
            if !p.text.contains(&case.topic_name(round)) {
                findings.push(format!(
                    "{sid} (g): the prompt of round {round} does not offer its own topic"
                ));
            }
        }
        asked.sort_unstable();
        if asked != ["ea", "eb"] {
            findings.push(format!(
                "{sid} (d): prompts per round {asked:?}, expected one each"
            ));
        }

        // (h) a topic is closed by its own round only, at its own round's turn;
        // only the "source" policy closes at all.
        for (round, _, _) in ROUNDS {
            let id = case.topic_id(round);
            let Some((closed_at, ended)) = topics.get(&id) else {
                findings.push(format!("{sid} (h): topic {id} is gone"));
                continue;
            };
            let ended_round = case.tag_of(ended).and_then(round_of);
            match (case.policy == Policy::Source, closed_at.is_empty()) {
                (true, true) => {
                    findings.push(format!("{sid} (h): topic {id} was not closed by its round"));
                }
                (false, false) => findings.push(format!(
                    "{sid} (h): topic {id} was closed at {ended} -- by a verdict that named it \
                     across a round or at a foreign turn"
                )),
                _ => {}
            }
            if !closed_at.is_empty() && ended_round != Some(round) {
                findings.push(format!(
                    "{sid} (h): topic {id} of round {round} is closed at {ended}"
                ));
            }
        }

        // (e) the report says so.
        let said = reports.get(sid).cloned().unwrap_or_default();
        let groups = said.first().and_then(|hop| num(hop.get("groups")));
        if said.len() != 1 {
            findings.push(format!("{sid} (e): {} close_report(s)", said.len()));
        }
        // Every reference across a round is counted: per prompt the other
        // round's topic, in round {e,b} the correction of F-EA, and under the
        // "foreign" policy both adds and the own topic at a foreign turn.
        let (unseen, closed) = match case.policy {
            Policy::Foreign => (9, 0),
            Policy::Source => (3, 2),
            Policy::Newest | Policy::First => (3, 0),
        };
        if let Some(hop) = said.first()
            && (num(hop.get("unseen_refs")) != Some(unseen)
                || num(hop.get("closed")) != Some(closed))
        {
            findings.push(format!(
                "{sid} (e): unseen_refs={:?} closed={:?}, expected {unseen} and {closed}",
                hop.get("unseen_refs"),
                hop.get("closed")
            ));
        }
        if let Some(hop) = said.first()
            && (num(hop.get("groups")) != Some(2) || num(hop.get("unaudienced")) != Some(1))
        {
            findings.push(format!(
                "{sid} (e): groups={:?} unaudienced={:?}, expected 2 and 1: {hop:?}",
                hop.get("groups"),
                hop.get("unaudienced")
            ));
        }

        // (f) the lane fits its budget, measured in the colony's own log.
        let traces: BTreeSet<&str> = log
            .iter()
            .filter(|(_, _, s)| s == sid)
            .map(|(t, _, _)| t.as_str())
            .collect();
        let end = log
            .iter()
            .filter(|(t, _, _)| traces.contains(t.as_str()))
            .map(|(_, ttl, _)| *ttl)
            .min();
        match end {
            None => findings.push(format!("{sid} (f): no delivery of this session is logged")),
            Some(end) => {
                let used = i64::from(START_TTL) - end;
                eprintln!(
                    "gh933 close lane: start={START_TTL} end={end} used={used} groups={} \
                     case={sid}",
                    groups.map_or("?".to_string(), |g| g.to_string())
                );
                if used > LANE_MAX {
                    findings.push(format!(
                        "{sid} (f): the pass spends {used} of {LANE_MAX} routing decisions \
                         (reserve {RESERVE})"
                    ));
                }
            }
        }
    }

    let expired: Vec<&(String, String, String)> =
        dead.iter().filter(|(_, _, c)| c == "ttl_expired").collect();
    if !expired.is_empty() {
        findings.push(format!("(f) the run has ttl deaths: {expired:?}"));
    }

    assert!(
        findings.is_empty(),
        "{} finding(s) over {} case(s):\n{}",
        findings.len(),
        cases.len(),
        findings.join("\n")
    );
}
