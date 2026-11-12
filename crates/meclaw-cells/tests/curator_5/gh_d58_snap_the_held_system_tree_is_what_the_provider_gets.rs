//! GH #1468 (D58 snap) -- the held system tree is what the provider gets.
//!
//! WHY this file exists: a later call of a session carries only the families
//! of the system part that moved, and the model's cell holds the rest from the
//! calls before. The `sent` list of the snapshot row names the whole tree all
//! the same and marks each leaf `carried` or not. The curator harness has no
//! model cell, so its pin (`gh_d58_snap_a_snapshot_rebuilds_the_sent_bytes`)
//! compares only the carried leaves with the message to the model's cell;
//! that all leaves together are the system part the provider reads was the
//! curator's own bookkeeping and nothing more. Counts that are read from
//! `sent` stand on exactly that statement.
//!
//! Measured AT THE RECEIVER: a shipped role composite is booted as a colony
//! with a stub provider behind every `llm` cell (`support/composite_colony.rs`,
//! the start of the GH #892 colony case), `snapshot_window` on. One session,
//! three calls: between the first and the second exactly one family of the
//! system part moves (a pack through the door in the wall), between the second
//! and the third none. For every call the system part put together from ALL
//! leaves of `sent` and the blocks of the ledger is byte for byte the system
//! part of the request the stub recorded; for the call without a move the
//! request's system part is also byte for byte the one of the call before.
//!
//! A second case in the same test (`several_families_case`): the first one's
//! system part is one or two families and 30 to 70 bytes, which says nothing
//! about the order of several families or about the tool part -- and a
//! provider's cache prefix is decided exactly there. So the menu is asked at
//! boot and answered before the first call, the system part has four families
//! with a text and three tools, and over five calls nothing moves, then a
//! family between two others, then the tool part, then nothing. `sent` lists
//! the system part by path, the provider gets it in the order of the model's
//! cell (GH #1468, the reader orders): so this case too puts the entries
//! together by the rule of that cell, text and tools, and compares them with
//! the request byte for byte -- and it shows that the order is the
//! `system_order` of the cell in the composite and not one this file brings:
//! the same entries without that parameter are NOT the provider's text.
//!
//! Guarded like every template-reading test (GH #49).

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

// harness_fold: #[path = "support/composite_colony.rs"]
#[allow(unused_imports)]
use crate::composite_colony; // harness_fold: mod composite_colony;

use composite_colony::*;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::{ColonyHandle, override_params_on_disk, wait};
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion};
use std::collections::{BTreeMap, BTreeSet};
use tokio::sync::mpsc;

const COMPOSITE: &str = "talky";
const CHANNEL: &str = "chat:d58-snap";
const AUDIENCE: &str = r#"["member:owner","agent:voice"]"#;

const IDENTITY: &str = "You are the house's companion.";
const PERSONA: &str = "Keeps a small garden behind the house.";

fn turn(text: &str) -> Message {
    MessageBuilder::new(Path::new(&format!("/{COMPOSITE}")))
        .hop(map(json!({"route": "in_turn"})))
        .context(map(json!({"channel": CHANNEL, "audience_set": AUDIENCE})))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

/// A pack through the door in the wall (`in_pack`): durable `system.*` slots
/// for the composite's window. The event is the receipt `pack_ack` at the rim;
/// the hang mark only aborts.
async fn pack(h: &ColonyHandle, park: &mut mpsc::Receiver<Message>, system: Value) {
    h.send(
        MessageBuilder::new(Path::new(&format!("/{COMPOSITE}")))
            .hop(map(json!({"route": "in_pack"})))
            .body(Body::Inline(json!({ "system": system })))
            .ttl(400)
            .build(),
    )
    .await;
    let ack = wait::until_some_with(
        "the pack is answered with its receipt",
        || {
            let mut got = None;
            while let Ok(m) = park.try_recv() {
                if m.headers.hop.get("route").and_then(Value::as_str) == Some("pack_ack") {
                    got = Some(m);
                    break;
                }
            }
            async move { got }
        },
        || async move { format!("dead letters: {:#?}", dead_letters(h).await) },
    )
    .await;
    assert_eq!(
        ack.headers.hop.get("error_code").and_then(Value::as_str),
        Some(""),
        "the pack is accepted: {:?}",
        ack.headers.hop
    );
}

/// The system part of a request as the provider read it: the content of the
/// leading `system` message, empty when the request has none.
fn provider_system(req: &OpenAiRequestSnapshot) -> String {
    let wire = messages(req);
    match wire.first() {
        Some(m) if m["role"] == "system" => m["content"]
            .as_str()
            .unwrap_or_else(|| panic!("the system part is one string: {m}"))
            .to_string(),
        _ => String::new(),
    }
}

/// The leaves of the system part a `sent` list names, read from the list and
/// the blocks of the ledger alone.
struct Listed {
    /// Every leaf, `(path, text)`, in the order of the list.
    leaves: Vec<(String, String)>,
    /// How many of them the call did not carry (`carried` false): the leaves
    /// the model's cell held from a call before.
    held: usize,
}

fn listed(ledger: &std::path::Path, sent: &str) -> Listed {
    let list: Vec<Value> = sj::from_str(sent).unwrap_or_else(|e| panic!("`sent` is a list: {e}"));
    let mut out = Listed {
        leaves: Vec::new(),
        held: 0,
    };
    for e in &list {
        let hash = e["hash"].as_str().unwrap_or_default();
        assert!(
            hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()),
            "an entry names a block: {e}"
        );
        let found = rows(
            ledger,
            &format!("SELECT body FROM blocks WHERE hash = '{hash}'"),
        );
        let body = found
            .first()
            .and_then(|r| r.first())
            .unwrap_or_else(|| panic!("the ledger holds the block of {e}"));
        assert_eq!(
            curator_hive::sha256_hex(body),
            hash,
            "the block is content-addressed: {e}"
        );
        let el: Value = sj::from_str(body).unwrap_or_else(|x| panic!("{e}: {x}"));
        if el.is_array() || el.get("origin").is_some() || el.get("path").is_none() {
            continue;
        }
        let carried = e["carried"]
            .as_bool()
            .unwrap_or_else(|| panic!("a leaf of the system part says `carried`: {e}"));
        if !carried {
            out.held += 1;
        }
        out.leaves.push((
            el["path"].as_str().unwrap_or_default().to_string(),
            el["text"].as_str().unwrap_or_default().to_string(),
        ));
    }
    out
}

/// The system prompt of a tree of leaves, by the rule of the `llm` cell
/// (`docs/cell-types.md`, `system_order`): the families the cell's
/// `system_order` names first and in that order, the others alphabetically,
/// `tools` never; below a family depth first in alphabetical order, a node
/// with a `text` is a leaf; an empty text adds nothing; the texts joined by
/// one blank line.
fn system_prompt(leaves: &[(String, String)], order: &[String]) -> String {
    let mut tree = Map::new();
    for (path, text) in leaves {
        let mut node = &mut tree;
        for key in path.split('.') {
            node = node
                .entry(key.to_string())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .expect("a node of the tree");
        }
        node.insert("text".to_string(), json!(text));
    }
    fn walk(node: &Value, out: &mut Vec<String>) {
        let Some(obj) = node.as_object() else { return };
        if let Some(text) = obj.get("text") {
            if let Some(t) = text.as_str().filter(|t| !t.is_empty()) {
                out.push(t.to_string());
            }
            return;
        }
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        for k in keys {
            walk(&obj[k], out);
        }
    }
    let mut rest: Vec<String> = tree.keys().filter(|k| *k != "tools").cloned().collect();
    let mut families = Vec::new();
    for k in order {
        if let Some(pos) = rest.iter().position(|x| x == k) {
            families.push(rest.remove(pos));
        }
    }
    rest.sort();
    families.extend(rest);
    let mut parts = Vec::new();
    for k in &families {
        walk(&tree[k], &mut parts);
    }
    parts.join("\n\n")
}

/// The leaves of each family (the first segment of the path), sorted.
fn by_family(leaves: &[(String, String)]) -> BTreeMap<String, Vec<(String, String)>> {
    let mut out: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (path, text) in leaves {
        let family = path.split('.').next().unwrap_or_default().to_string();
        out.entry(family)
            .or_default()
            .push((path.clone(), text.clone()));
    }
    for v in out.values_mut() {
        v.sort();
    }
    out
}

/// The families whose leaves differ between two trees.
fn families_moved(before: &[(String, String)], after: &[(String, String)]) -> BTreeSet<String> {
    let (a, b) = (by_family(before), by_family(after));
    a.keys()
        .chain(b.keys())
        .filter(|k| a.get(*k) != b.get(*k))
        .cloned()
        .collect()
}

/// Where two texts part, for a message a reader can act on: the byte offset
/// of the first difference and both texts around it.
fn first_difference(left: (&str, &str), right: (&str, &str)) -> String {
    let at = left
        .1
        .bytes()
        .zip(right.1.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| left.1.len().min(right.1.len()));
    let around = |s: &str| -> String {
        let mut from = at.saturating_sub(120);
        while !s.is_char_boundary(from) {
            from -= 1;
        }
        let mut to = (at + 120).min(s.len());
        while !s.is_char_boundary(to) {
            to += 1;
        }
        s[from..to].to_string()
    };
    format!(
        "first difference at byte {at}\n{} ({} bytes): {:?}\n{} ({} bytes): {:?}",
        left.0,
        left.1.len(),
        around(left.1),
        right.0,
        right.1.len(),
        around(right.1)
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d58_snap_the_held_system_tree_is_what_the_provider_gets() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    // A day that turns during the run would move the day tiers of the window:
    // the curator tests' one midnight guard (GH #1033/#1062), a wait for the
    // clock, not for a result.
    curator_hive::clear_of_midnight();
    one_family_case().await;
    several_families_case().await;
}

/// The first case: one family held, one carried, no menu.
async fn one_family_case() {
    let brain = MockOpenAI::start(vec![
        canned_chat_completion("Good morning.", "stop"),
        canned_chat_completion("That sounds lovely.", "stop"),
        canned_chat_completion("Nothing else is new.", "stop"),
    ])
    .await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion("A short account.", "stop")]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    build(
        &td,
        COMPOSITE,
        &brain.base_url,
        &background.base_url,
        json!({}),
    );
    let main = td.path().join("main").join(COMPOSITE);
    override_params_on_disk(
        &main.join("curator/policy"),
        &json!({"snapshot_window": "1"}),
    );
    let order: Vec<String> = read_json(&main.join("brain/config.json"))["params"]["system_order"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|k| k.as_str().map(str::to_string))
        .collect();
    let (h, mut ports) = boot(&td).await;
    let ledger = main.join("curator/ledger/cell.db");

    // Before the first call: a family that then stands still for the session.
    pack(&h, &mut ports.park, json!({"identity": {"text": IDENTITY}})).await;
    h.send(turn("good morning")).await;
    answer_or_explain(&mut ports.sink, &h, "turn 1").await;
    // Between the first and the second call exactly one family moves.
    pack(
        &h,
        &mut ports.park,
        json!({"identity": {"text": IDENTITY}, "persona": {"self": {"text": PERSONA}}}),
    )
    .await;
    h.send(turn("I was out in the garden")).await;
    answer_or_explain(&mut ports.sink, &h, "turn 2").await;
    // Between the second and the third call none moves.
    h.send(turn("what else is new?")).await;
    answer_or_explain(&mut ports.sink, &h, "turn 3").await;

    // Read once the colony settled (TV-A2): every row that follows from the
    // three turns is written, and no fourth request is on its way.
    h.quiesce().await;
    let reqs = provider_requests(&brain, &h, 3).await;
    let dead = dead_letters(&h).await;
    while ports.park.try_recv().is_ok() {}
    h.shutdown().await;
    assert_eq!(reqs.len(), 3, "three turns are three calls of the provider");
    // The snapshot rows of the session, oldest first. The ledger knows a call
    // by its id, not by the cell that answered it: that the session has as
    // many rows as the scripted stub has requests is what ties row n to
    // request n -- a call another model answered would be a row too many.
    let calls = rows(
        &ledger,
        "SELECT call_id, session_id, sent FROM snapshots ORDER BY seq",
    );
    let overview: Vec<(&str, &str, usize)> = calls
        .iter()
        .map(|r| (r[0].as_str(), r[1].as_str(), r[2].len()))
        .collect();
    assert_eq!(
        calls.len(),
        3,
        "one snapshot row per call of the provider, (call, session, bytes of `sent`): \
         {overview:#?}"
    );
    let sessions: BTreeSet<&str> = calls.iter().map(|r| r[1].as_str()).collect();
    assert_eq!(sessions.len(), 1, "one session: {overview:#?}");

    let at_provider: Vec<String> = reqs.iter().map(provider_system).collect();
    let from_sent: Vec<Listed> = calls.iter().map(|r| listed(&ledger, &r[2])).collect();
    assert!(
        !at_provider[0].is_empty() && at_provider[0].contains(IDENTITY),
        "the first call has a system part: {:?}",
        at_provider[0]
    );
    // THE STATEMENT: for every call, all leaves of `sent` together are the
    // system part the provider read, byte for byte.
    for (n, (tree, wire)) in from_sent.iter().zip(&at_provider).enumerate() {
        let rebuilt = system_prompt(&tree.leaves, &order);
        assert!(
            rebuilt == *wire,
            "call {} (`{}`): the system part put together from `sent` is not the one the \
             provider got -- {}",
            n + 1,
            calls[n][0],
            first_difference(("from `sent`", &rebuilt), ("at the provider", wire))
        );
    }
    // The run was the one this file describes: one family moved before the
    // second call, none before the third, and the later calls list leaves
    // they did not carry -- the held tree, the part the statement is about.
    assert_eq!(
        families_moved(&from_sent[0].leaves, &from_sent[1].leaves),
        BTreeSet::from(["persona".to_string()]),
        "exactly one family moved between the first and the second call"
    );
    assert!(
        at_provider[1].contains(PERSONA) && !at_provider[0].contains(PERSONA),
        "the provider read the moved family from the second call on: {:?}",
        at_provider[1]
    );
    assert_eq!(
        families_moved(&from_sent[1].leaves, &from_sent[2].leaves),
        BTreeSet::new(),
        "no family moved between the second and the third call"
    );
    assert!(
        at_provider[2] == at_provider[1],
        "without a move the provider reads the system part of the call before -- {}",
        first_difference(
            ("second call", &at_provider[1]),
            ("third call", &at_provider[2])
        )
    );
    for n in [1, 2] {
        assert!(
            from_sent[n].held > 0,
            "call {} lists leaves the model's cell held and the call did not carry: {:?}",
            n + 1,
            calls[n][2]
        );
    }
    assert!(dead.is_empty(), "no dead letter: {dead:#?}");
}

const ANSWERER: &str = "tools";
const HOUSE: &str = "The house stands at the end of the lane.";
const PERSONA_LATER: &str = "Keeps a small garden and two hens behind the house.";
const SLOTS_OF_TOOLS: &str = "SELECT b.body FROM slots s JOIN blocks b ON b.hash = s.hash \
                              WHERE s.path LIKE 'tools.%'";

fn declaration(name: &str, description: &str) -> Value {
    json!({"name": name, "description": description,
           "parameters": {"type": "object", "properties": {}}})
}

/// A menu answer of the level above (`in_menu`), the declarations of the tools
/// the composite says it uses. The event is the row of the curator's ledger
/// that holds `mark` under `system.tools`; the hang mark only aborts.
async fn menu_answer(h: &ColonyHandle, ledger: &std::path::Path, schemas: Value, mark: &str) {
    h.send(
        MessageBuilder::new(Path::new(&format!("/{COMPOSITE}")))
            .hop(map(json!({"route": "in_menu"})))
            .context(map(json!({"tool_answerer": ANSWERER})))
            .body(Body::Inline(
                json!({"messages": [], "schemas": schemas, "unknown": []}),
            ))
            .ttl(400)
            .build(),
    )
    .await;
    until_row(
        ledger,
        SLOTS_OF_TOOLS,
        mark,
        "the menu answer reached the curator's tool slots",
    )
    .await;
}

/// The tool part a `sent` list names, by the rule of the `llm` cell: the leaves
/// under `tools` in alphabetical order of their key, each the tool object it
/// holds, one string per tool.
fn listed_tools(leaves: &[(String, String)]) -> Vec<String> {
    let mut under: Vec<&(String, String)> = leaves
        .iter()
        .filter(|(path, _)| path.starts_with("tools."))
        .collect();
    under.sort();
    under
        .iter()
        .map(|(path, text)| {
            let tool: Value =
                sj::from_str(text).unwrap_or_else(|e| panic!("`{path}` holds a tool object: {e}"));
            sj::to_string(&tool).expect("serialise")
        })
        .collect()
}

/// The tool part of a request as the provider read it, one string per tool.
fn provider_tools(req: &OpenAiRequestSnapshot) -> Vec<String> {
    req.tools()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| sj::to_string(t).expect("serialise"))
        .collect()
}

/// The families of a list in the order their first leaf stands in it.
fn families_in_order(leaves: &[(String, String)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (path, _) in leaves {
        let family = path.split('.').next().unwrap_or_default().to_string();
        if !out.contains(&family) {
            out.push(family);
        }
    }
    out
}

/// The second case (GH #1468): the menu is asked at boot, the system part has
/// several families and a tool part of several entries, and over five calls
/// nothing moves, then a family in the middle, then the tool part, then
/// nothing again. `sent` lists the system part by path; the reader orders the
/// entries by the rule of the model's cell, and the order it uses is the
/// `system_order` of that cell in the composite on disk, not a list of this file.
async fn several_families_case() {
    let brain = MockOpenAI::start(vec![
        canned_chat_completion("Good morning.", "stop"),
        canned_chat_completion("That sounds lovely.", "stop"),
        canned_chat_completion("Two hens, how nice.", "stop"),
        canned_chat_completion("I can look that up.", "stop"),
        canned_chat_completion("Nothing else is new.", "stop"),
    ])
    .await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion("A short account.", "stop")]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    build(
        &td,
        COMPOSITE,
        &brain.base_url,
        &background.base_url,
        json!({}),
    );
    ask_the_menu_at_boot(&td, COMPOSITE);
    let main = td.path().join("main").join(COMPOSITE);
    override_params_on_disk(
        &main.join("curator/policy"),
        &json!({"snapshot_window": "1"}),
    );
    let (h, mut ports) = boot(&td).await;
    let ledger = main.join("curator/ledger/cell.db");

    // The menu asked at boot is answered before the first call goes: the
    // curator's own answer is the row of the sidecar contract (the event the
    // GH #892 colony case waits for), the answer of the level above the rows
    // of the tool slots.
    until_row(
        &ledger,
        "SELECT b.body FROM slots s JOIN blocks b ON b.hash = s.hash \
         WHERE s.path = 'instructions.sidecar'",
        "[#0123456789ab]",
        "the curator's menu answer reached the sidecar contract",
    )
    .await;
    menu_answer(
        &h,
        &ledger,
        json!([
            declaration("weather", "Almanac one: the weather at a place."),
            declaration("web_search", "Searches the web."),
            declaration("web_fetch", "Fetches one page."),
        ]),
        "Almanac one",
    )
    .await;
    pack(
        &h,
        &mut ports.park,
        json!({"identity": {"text": IDENTITY}, "persona": {"self": {"text": PERSONA}},
               "handover": {"text": HOUSE}}),
    )
    .await;

    h.send(turn("good morning")).await;
    answer_or_explain(&mut ports.sink, &h, "turn 1").await;
    // Before the second call nothing moves.
    h.send(turn("I was out in the garden")).await;
    answer_or_explain(&mut ports.sink, &h, "turn 2").await;
    // Before the third call one family between two others moves.
    pack(
        &h,
        &mut ports.park,
        json!({"identity": {"text": IDENTITY}, "persona": {"self": {"text": PERSONA_LATER}},
               "handover": {"text": HOUSE}}),
    )
    .await;
    h.send(turn("we have two hens now")).await;
    answer_or_explain(&mut ports.sink, &h, "turn 3").await;
    // Before the fourth call the tool part moves.
    menu_answer(
        &h,
        &ledger,
        json!([
            declaration(
                "weather",
                "Almanac two: the weather and the wind at a place."
            ),
            declaration("web_search", "Searches the web."),
            declaration("web_fetch", "Fetches one page."),
        ]),
        "Almanac two",
    )
    .await;
    h.send(turn("will it rain tomorrow?")).await;
    answer_or_explain(&mut ports.sink, &h, "turn 4").await;
    // Before the fifth call nothing moves.
    h.send(turn("what else is new?")).await;
    answer_or_explain(&mut ports.sink, &h, "turn 5").await;

    h.quiesce().await;
    let reqs = provider_requests(&brain, &h, 5).await;
    let dead = dead_letters(&h).await;
    while ports.park.try_recv().is_ok() {}
    h.shutdown().await;
    assert_eq!(reqs.len(), 5, "five turns are five calls of the provider");
    let calls = rows(
        &ledger,
        "SELECT call_id, session_id, sent FROM snapshots ORDER BY seq",
    );
    let overview: Vec<(&str, &str, usize)> = calls
        .iter()
        .map(|r| (r[0].as_str(), r[1].as_str(), r[2].len()))
        .collect();
    assert_eq!(
        calls.len(),
        5,
        "one snapshot row per call of the provider, (call, session, bytes of `sent`): \
         {overview:#?}"
    );
    let sessions: BTreeSet<&str> = calls.iter().map(|r| r[1].as_str()).collect();
    assert_eq!(sessions.len(), 1, "one session: {overview:#?}");

    let from_sent: Vec<Listed> = calls.iter().map(|r| listed(&ledger, &r[2])).collect();
    let texts_at_provider: Vec<String> = reqs.iter().map(provider_system).collect();
    let tools_at_provider: Vec<Vec<String>> = reqs.iter().map(provider_tools).collect();

    // The case is the one this function describes: several families, a tool
    // part of several entries.
    let filled: Vec<(String, String)> = from_sent[0]
        .leaves
        .iter()
        .filter(|(_, text)| !text.is_empty())
        .cloned()
        .collect();
    let families = families_in_order(&filled);
    assert!(
        families.len() >= 3 && families.iter().any(|f| f == "tools"),
        "the system part has at least three families with a text, the tools among them: \
         {families:?}"
    );
    assert!(
        tools_at_provider[0].len() >= 2,
        "the provider is offered at least two tools: {:?}",
        tools_at_provider[0]
    );

    // What moved between the calls, read from `sent`.
    let moved: Vec<BTreeSet<String>> = (1..5)
        .map(|n| families_moved(&from_sent[n - 1].leaves, &from_sent[n].leaves))
        .collect();
    assert_eq!(
        moved,
        vec![
            BTreeSet::new(),
            BTreeSet::from(["persona".to_string()]),
            BTreeSet::from(["tools".to_string()]),
            BTreeSet::new(),
        ],
        "nothing moves, then one family, then the tool part, then nothing"
    );
    assert!(
        texts_at_provider.iter().all(|t| t.contains(IDENTITY)
            && t.contains(HOUSE)
            && !t.starts_with(PERSONA)
            && !t.starts_with(PERSONA_LATER)
            && !t.ends_with(PERSONA)
            && !t.ends_with(PERSONA_LATER)),
        "the family that moves stands between two others in what the provider read: {:?}",
        texts_at_provider[2]
    );
    assert!(
        texts_at_provider[2].contains(PERSONA_LATER)
            && !texts_at_provider[1].contains(PERSONA_LATER),
        "the provider read the moved family from the third call on: {:?}",
        texts_at_provider[2]
    );
    assert!(
        tools_at_provider[3] != tools_at_provider[2]
            && tools_at_provider[3]
                .iter()
                .any(|t| t.contains("Almanac two")),
        "the provider read the moved tool part from the fourth call on: {:?}",
        tools_at_provider[3]
    );
    // Without a move the provider reads what it read the call before.
    for n in [1, 4] {
        assert!(
            texts_at_provider[n] == texts_at_provider[n - 1],
            "call {}: without a move the provider reads the system part of the call before -- {}",
            n + 1,
            first_difference(
                ("the call before", &texts_at_provider[n - 1]),
                ("this call", &texts_at_provider[n])
            )
        );
        assert!(
            tools_at_provider[n] == tools_at_provider[n - 1],
            "call {}: without a move the provider reads the tools of the call before: {:?} \
             against {:?}",
            n + 1,
            tools_at_provider[n],
            tools_at_provider[n - 1]
        );
    }
    for n in 1..5 {
        assert!(
            from_sent[n].held > 0,
            "call {} lists leaves the model's cell held and the call did not carry: {:?}",
            n + 1,
            calls[n][2]
        );
    }
    assert!(dead.is_empty(), "no dead letter: {dead:#?}");

    // The order is the cell's own: `system_order` as the model's cell of the
    // composite on disk declares it. It names families, and it decides -- the
    // same entries put together without it (alphabetically throughout) are
    // not what the provider read, so a reader that brought no order or its
    // own would not pass below.
    let order: Vec<String> = read_json(&main.join("brain/config.json"))["params"]["system_order"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|k| k.as_str().map(str::to_string))
        .collect();
    assert!(
        families.iter().filter(|f| order.contains(f)).count() >= 2
            && families.iter().any(|f| f != "tools" && !order.contains(f)),
        "the cell's `system_order` {order:?} names at least two of the families and leaves \
         one out: {families:?}"
    );
    assert!(
        system_prompt(&from_sent[0].leaves, &[]) != texts_at_provider[0],
        "without the cell's `system_order` the entries are not the provider's text: {:?}",
        texts_at_provider[0]
    );

    // THE STATEMENT, last, so that a run that is not the described one never
    // reaches it: for every call, the entries of `sent`, ordered by the rule
    // of the model's cell with the cell's own `system_order`, are the system
    // part the provider read -- text and tools --, byte for byte. Every call
    // is looked at before the verdict.
    let mut findings = Vec::new();
    for (n, tree) in from_sent.iter().enumerate() {
        let text = system_prompt(&tree.leaves, &order);
        if text != texts_at_provider[n] {
            findings.push(format!(
                "call {} (`{}`): the system part put together from `sent` is not the one the \
                 provider got -- {}\nfamilies of `sent`: {:?}",
                n + 1,
                calls[n][0],
                first_difference(
                    ("from `sent`", &text),
                    ("at the provider", &texts_at_provider[n])
                ),
                families_in_order(&tree.leaves)
            ));
        }
        let (listed, wire) = (
            listed_tools(&tree.leaves).join("\n"),
            tools_at_provider[n].join("\n"),
        );
        if listed != wire {
            findings.push(format!(
                "call {} (`{}`): the tools `sent` lists are not the ones the provider got -- {}",
                n + 1,
                calls[n][0],
                first_difference(("from `sent`", &listed), ("at the provider", &wire))
            ));
        }
    }
    assert!(
        findings.is_empty(),
        "{} finding(s) over {} calls:\n{}",
        findings.len(),
        calls.len(),
        findings.join("\n\n")
    );
}
