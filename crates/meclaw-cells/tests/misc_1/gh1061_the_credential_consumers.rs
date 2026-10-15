//! GH #1061 (#801) — every cell that spends a provider credential is granted
//! one, and the builder's table of such cells cannot drift from the templates.
//!
//! Until 0.62 the builder recipe granted exactly two cells of a member (the
//! talky and cogny brains, `CREDENTIAL_ASKERS`) and one of an app (the
//! presenter's decider); every other provider call — the memory hive's four
//! models and its embedder, the file space, the curators, the screen's judge —
//! read `${OPENROUTER_API_KEY}` out of the colony's `.env`. #801 retires that
//! road, so the table `CREDENTIAL_CONSUMERS` names every consumer, and these
//! locks keep it honest in both directions:
//!
//! | lock | claim |
//! |---|---|
//! | `gh1061_every_credential_consumer_of_a_member_is_granted` | a member grown with a credential, its assistant and its presenter app render ONE grant and TWO v-lanes per consumer, every handle distinct |
//! | `gh1061_the_recipe_table_matches_the_templates` | every cell whose contract emits `credential_request` under a table template is in the table, and every table row is such a cell |
//! | `gh1061_every_colony_and_standalone_consumer_is_granted` | the colony shell's and every standalone template's consumers carry a grant their own seed holds, with both edges |
//! | `gh1061_a_recipe_grown_voice_channel_is_granted` | a channel grown by the recipe that spends a key (voice, Telegram) carries its grants, both v-lanes and its handles in the same diff |

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{emit_all, shipped_script};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

fn templates() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn shipped() -> bool {
    Path::new(RECIPES).is_file() && templates().join("member/template.json").is_file()
}

fn read(p: &Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn read_jsonl(p: &Path) -> Vec<Value> {
    std::fs::read_to_string(p)
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| meclaw_core::serde_json::from_str(l).expect("a jsonl row"))
        .filter(|v: &Value| v.get("schema").is_none())
        .collect()
}

/// The table, read out of the recipe's source without running it: the literal
/// is JSON between `CREDENTIAL_CONSUMERS = ` and its end marker.
fn table() -> BTreeMap<String, Vec<(String, String)>> {
    let script = read(Path::new(RECIPES))["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let start = script
        .find("CREDENTIAL_CONSUMERS = {")
        .expect("the recipe declares CREDENTIAL_CONSUMERS")
        + "CREDENTIAL_CONSUMERS = ".len();
    let end = script[start..]
        .find("}  # end CREDENTIAL_CONSUMERS")
        .expect("the table's end marker")
        + start
        + 1;
    let v: Value = meclaw_core::serde_json::from_str(&script[start..end])
        .unwrap_or_else(|e| panic!("CREDENTIAL_CONSUMERS is not JSON: {e}"));
    v.as_object()
        .expect("a map")
        .iter()
        .map(|(t, rows)| {
            let rows = rows
                .as_array()
                .expect("rows")
                .iter()
                .map(|r| {
                    (
                        r[0].as_str().expect("path").to_string(),
                        r[1].as_str().expect("kind").to_string(),
                    )
                })
                .collect();
            (t.clone(), rows)
        })
        .collect()
}

fn emits_credential_request(cfg: &Value) -> bool {
    cfg.pointer("/contract/emits/hop/route/values")
        .and_then(Value::as_array)
        .is_some_and(|vs| vs.iter().any(|v| v == "credential_request"))
}

/// Every consumer cell of the template tree at `dir`, refs followed by name,
/// as `(path relative to the template root, config)`.
fn consumers(dir: &Path, rel: &str, out: &mut Vec<(String, Value)>) {
    let cfg_path = dir.join("config.json");
    if !cfg_path.is_file() {
        return;
    }
    let cfg = read(&cfg_path);
    if cfg["cell"]["type"] == "ref" {
        let name = cfg["cell"]["template"]
            .as_str()
            .expect("a ref names its template")
            .split('@')
            .next()
            .unwrap_or_default()
            .to_string();
        consumers(&templates().join(name), rel, out);
        return;
    }
    if emits_credential_request(&cfg) {
        out.push((rel.to_string(), cfg.clone()));
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut kids: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    kids.sort();
    for k in kids {
        let name = k
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if name == "seed" {
            continue;
        }
        let child = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        consumers(&k, &child, out);
    }
}

/// Every non-empty `*grant_id` string in a cell's params, nested slots included.
fn grant_params(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                match x {
                    Value::String(s) if k.ends_with("grant_id") && !s.is_empty() => {
                        out.push(s.clone())
                    }
                    _ => grant_params(x, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| grant_params(x, out)),
        _ => {}
    }
}

fn render(recipe: &str, params: Value) -> Vec<Value> {
    let all = emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": recipe, "request": "…",
                                         "params": params}).to_string()}],
        }),
    );
    let first = all
        .into_iter()
        .find(|m| m["header"]["operation"] == "recipe")
        .expect("the recipe answers with a manifest emission");
    assert!(
        first["header"]["error_code"].is_null(),
        "the recipe refused a wish it is supposed to render: {first}"
    );
    first["manifest"].as_array().expect("a manifest").clone()
}

fn credential() -> Value {
    json!({"cred_ref": "cred:openrouter", "subject": "member:alex",
           "expires_at": "2099-01-01T00:00:00.000000Z"})
}

#[test]
fn gh1061_every_credential_consumer_of_a_member_is_granted() {
    if !shipped() {
        return; // GH #49
    }
    let mut decls = render(
        "grow_level",
        json!({"scope": "/os/orgs/acme", "level": "member", "name": "alex",
               "template": "member", "credential": credential()}),
    );
    decls.extend(render(
        "grow_level",
        json!({"scope": "/os/orgs/acme/members/alex", "level": "assistant",
               "name": "scribe", "template": "assistant", "credential": credential()}),
    ));
    decls.extend(render(
        "install_app",
        json!({"scope": "/os/orgs/acme/members/alex", "app": "presenter",
               "template": "presenter", "screen": "display", "generation": "scribe",
               "declaration": {"listens": ["turn"]}, "ctx": {"member_person": "alex"},
               "credential": credential()}),
    ));

    // Every grant row, every v-lane pair, every override, across the manifests.
    let mut grants: Vec<Value> = Vec::new();
    let mut asks: BTreeMap<String, String> = BTreeMap::new(); // consumer -> requester
    let mut answers: BTreeMap<String, String> = BTreeMap::new(); // consumer -> guard
    let mut named: BTreeSet<String> = BTreeSet::new(); // handles set as overrides
    for d in &decls {
        let scope = d["scope"]
            .as_str()
            .expect("scope")
            .trim_end_matches('/')
            .to_string();
        let abs = |p: &str| format!("{scope}/{}", p.trim_start_matches("./"));
        for row in d["diff"]["seed_rows"].as_array().into_iter().flatten() {
            if row["table"] == "grants" {
                grants.extend(row["rows"].as_array().expect("rows").iter().cloned());
            }
        }
        for e in d["diff"]["add_edges"].as_array().into_iter().flatten() {
            let (from, to) = (
                e["from"].as_str().unwrap_or(""),
                e["to"].as_str().unwrap_or(""),
            );
            if e["lane"] == "credential_request" {
                let req = e["modifier"]["set_context"]["requester"]
                    .as_str()
                    .unwrap_or("");
                asks.insert(abs(from), req.trim_matches('\'').to_string());
            }
            if e["lane"] == "in_sealed" {
                answers.insert(abs(to), e["condition"].as_str().unwrap_or("").to_string());
            }
        }
        for n in d["diff"]["add_nodes"].as_array().into_iter().flatten() {
            let mut hs = Vec::new();
            grant_params(&n["override_params"], &mut hs);
            named.extend(hs);
        }
    }

    let member = "/os/orgs/acme/members/alex";
    let want: Vec<String> = [
        "memory-hive/closer",
        "memory-hive/dialectic",
        "memory-hive/dreamer",
        "memory-hive/judge",
        "memory-hive/embed",
        "file-space/summarizer",
        "file-space/embed",
        "channels/display/judge",
        "assistants/scribe/talky/brain",
        "assistants/scribe/talky/curator/summarizer",
        "assistants/scribe/talky-chat/brain",
        "assistants/scribe/talky-chat/curator/summarizer",
        "assistants/scribe/cogny/brain",
        "assistants/scribe/cogny/curator/summarizer",
        "apps/presenter/decide",
    ]
    .iter()
    .map(|p| format!("{member}/{p}"))
    .collect();
    for c in &want {
        assert!(
            asks.contains_key(c),
            "no credential_request v-lane from {c}: {asks:?}"
        );
        assert!(
            answers.contains_key(c),
            "no in_sealed v-lane to {c}: {answers:?}"
        );
    }
    assert_eq!(
        grants.len(),
        want.len(),
        "ONE grant per consumer, no more: {:?}",
        grants.iter().map(|g| &g["grant_id"]).collect::<Vec<_>>()
    );
    let handles: BTreeSet<&str> = grants
        .iter()
        .filter_map(|g| g["grant_id"].as_str())
        .collect();
    assert_eq!(
        handles.len(),
        grants.len(),
        "two consumers share a handle: {handles:?}"
    );
    for g in &grants {
        let h = g["grant_id"].as_str().expect("a handle");
        assert!(
            named.contains(h),
            "grant {h} is seeded but no consumer names it: {named:?}"
        );
        assert!(
            answers
                .values()
                .any(|guard| guard.contains(&format!("hop.grant_id == '{h}'"))),
            "no answer edge is addressed by {h}"
        );
        let req = g["requester"].as_str().expect("requester");
        assert!(
            asks.values().any(|r| r == req),
            "no ask edge stamps requester {req}"
        );
        assert_eq!(g["cred_ref"], json!("cred:openrouter"), "{g}");
    }
}

#[test]
fn gh1061_the_recipe_table_matches_the_templates() {
    if !shipped() {
        return; // GH #49
    }
    for (template, rows) in table() {
        let mut found = Vec::new();
        consumers(&templates().join(&template), "", &mut found);
        let found: BTreeSet<String> = found.into_iter().map(|(p, _)| p).collect();
        let listed: BTreeSet<String> = rows.into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            listed, found,
            "CREDENTIAL_CONSUMERS[{template:?}] (left) and the cells of templates/{template} \
             whose contract emits `credential_request` (right) differ: a consumer the recipe \
             does not grant reads no key at all since #801, and a row the template lacks \
             draws an edge to nothing"
        );
    }
}

/// Every granted consumer of the template at `dir` that `pick` selects has its
/// grant in the template's own broker seed and both v-lanes to that broker.
/// Returns how many grants it checked.
fn assert_granted(dir: &Path, pick: impl Fn(&str, &Value) -> bool) -> usize {
    let mut checked = 0;
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let grants = read_jsonl(&dir.join("access/store/seed/grants.jsonl"));
    let edges = read(&dir.join("config.json"))["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut found = Vec::new();
    consumers(dir, "", &mut found);
    for (path, cfg) in &found {
        if !pick(path, cfg) {
            continue;
        }
        let mut hs = Vec::new();
        grant_params(&cfg["params"], &mut hs);
        // The ref marker may set the grant for a referenced template's cell.
        let parent = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        let leaf = path.rsplit('/').next().unwrap_or("");
        if !parent.is_empty() {
            let marker = read(&dir.join(parent).join("config.json"));
            grant_params(&marker["override_params"][leaf], &mut hs);
        }
        hs.sort();
        hs.dedup();
        if hs.is_empty() {
            continue; // anonymous by default (OR-VG-5): web_search, a keyless local model
        }
        for h in &hs {
            let g = grants
                .iter()
                .find(|g| g["grant_id"] == json!(h))
                .unwrap_or_else(|| {
                    panic!("templates/{name}: {path} names {h}, its seed has no such grant")
                });
            let at = format!("./{path}");
            let req = g["requester"].as_str().expect("requester");
            assert!(
                edges.iter().any(|e| e["from"] == json!(at)
                    && e["to"] == json!("./access")
                    && e["modifier"]["set_context"]["requester"] == json!(format!("'{req}'"))),
                "templates/{name}: no ask edge {at} -> ./access stamping {req}"
            );
            assert!(
                edges.iter().any(|e| e["from"] == json!("./access")
                    && e["to"] == json!(at)
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains(&format!("hop.grant_id == '{h}'")))),
                "templates/{name}: no answer edge ./access -> {at} addressed by {h}"
            );
            checked += 1;
        }
    }
    checked
}

/// Every template that seeds its own broker, with the number of granted
/// consumers the walk checks in it at least -- the inventory AND the
/// anti-vacuum floor. Four of them (`coder-pipeline`, `egon`,
/// `research-assistant`, `slack-agent`) are private and do not travel with the
/// export: each row is guarded per template by `is_file()` on its seed, so the
/// public clone checks its own subset against the sum of the rows it has. A
/// count is the floor of that template, not an exact pin -- a template that
/// grows a consumer stays green; one whose walk breaks does not.
const BROKERED: &[(&str, usize)] = &[
    ("coder-pipeline", 3),
    ("daily-digest", 1),
    ("egon", 2),
    ("freeswitch", 2),
    ("meclaw-os", 2),
    ("research-assistant", 2),
    ("slack-agent", 3),
    ("steward", 1),
    ("summarizer", 1),
];

#[test]
fn gh1061_every_colony_and_standalone_consumer_is_granted() {
    if !shipped() {
        return; // GH #49
    }
    let mut checked = 0;
    let mut floor = 0;
    for dir in template_dirs() {
        if !dir.join("access/store/seed/grants.jsonl").is_file() {
            continue;
        }
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let Some(&(_, min)) = BROKERED.iter().find(|(t, _)| *t == name) else {
            panic!(
                "templates/{name} seeds a broker but is not in BROKERED -- add it with its count"
            );
        };
        let n = assert_granted(&dir, |_, _| true);
        assert!(
            n >= min,
            "templates/{name}: only {n} granted consumers checked, BROKERED expects {min}"
        );
        checked += n;
        floor += min;
    }
    assert!(
        floor > 0 && checked >= floor,
        "only {checked} granted consumers checked against a floor of {floor} -- the walk found nothing"
    );
}

/// The shipped template directories, sorted.
fn template_dirs() -> Vec<PathBuf> {
    let mut rd: Vec<PathBuf> = std::fs::read_dir(templates())
        .expect("templates/")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    rd.sort();
    rd
}

/// GH #1061: a `voice` cell refuses a session without its key, and with a
/// grant it waits for a box only a broker sends. A template that PLACES one
/// (`freeswitch/voice`, a ref to `voice@…` that inherits its two handles)
/// therefore ships the broker, the seeded grants and both edges -- or the
/// cell asks into the void and every call is refused after the wait. The
/// single-cell `voice` template itself cannot seed (its README shows the
/// manifest that brings the broker), so the root of a template is not a
/// placement.
#[test]
fn gh1061_no_voice_cell_ships_a_grant_without_a_broker_edge() {
    if !shipped() {
        return; // GH #49
    }
    let is_placed_voice =
        |path: &str, cfg: &Value| !path.is_empty() && cfg["cell"]["type"] == "voice";
    let mut placed = 0;
    for dir in template_dirs() {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let mut found = Vec::new();
        consumers(&dir, "", &mut found);
        for (path, cfg) in found.iter().filter(|(p, c)| is_placed_voice(p, c)) {
            let mut hs = Vec::new();
            grant_params(&cfg["params"], &mut hs);
            if hs.is_empty() {
                continue;
            }
            placed += 1;
            assert!(
                dir.join("access/store/seed/grants.jsonl").is_file(),
                "templates/{name}: ./{path} spends {hs:?}, but the template ships no broker \
                 (access/store/seed/grants.jsonl) -- the cell would ask into the void"
            );
        }
        if placed > 0 && dir.join("access/store/seed/grants.jsonl").is_file() {
            assert_granted(&dir, is_placed_voice);
        }
    }
    assert!(
        placed >= 1,
        "no placed voice cell with a grant found -- freeswitch/voice is one"
    );
}

/// One channel declaration, read as the broker sees it: the grant rows, the
/// ask edge's requester per asking cell, the answer guards per cell, and the
/// handles the node's `override_params` names. Endpoints come back absolute.
struct ChannelRoad {
    grants: Vec<Value>,
    asks: BTreeMap<String, String>,
    answers: BTreeMap<String, Vec<String>>,
    named: BTreeSet<String>,
    /// Every other edge out of a cell: (from, condition).
    outs: Vec<(String, String)>,
}

fn channel_road(decls: &[Value]) -> ChannelRoad {
    let mut road = ChannelRoad {
        grants: Vec::new(),
        asks: BTreeMap::new(),
        answers: BTreeMap::new(),
        named: BTreeSet::new(),
        outs: Vec::new(),
    };
    for d in decls {
        let scope = d["scope"]
            .as_str()
            .expect("scope")
            .trim_end_matches('/')
            .to_string();
        let abs = |p: &str| format!("{scope}/{}", p.trim_start_matches("./"));
        for row in d["diff"]["seed_rows"].as_array().into_iter().flatten() {
            assert_eq!(row["target"], json!("./access/store"), "{row}");
            if row["table"] == "grants" {
                road.grants
                    .extend(row["rows"].as_array().expect("rows").iter().cloned());
            }
        }
        for e in d["diff"]["add_edges"].as_array().into_iter().flatten() {
            let (from, to) = (
                e["from"].as_str().unwrap_or(""),
                e["to"].as_str().unwrap_or(""),
            );
            let cond = e["condition"].as_str().unwrap_or("");
            let broker = format!("{MEMBER}/access");
            // GH #1102: the channel road is plain deep edges -- a `lane` would
            // make them v-lanes, and `channels` declares no connect point.
            let is_ask = abs(to) == broker && cond.contains("hop.route == 'credential_request'");
            let is_answer =
                abs(from) == broker && cond.contains("hop.operation == 'vault.deliver'");
            if is_ask || is_answer {
                assert!(
                    e["lane"].is_null(),
                    "GH #1102: a channel credential edge carries no lane: {e}"
                );
            }
            if is_ask {
                let req = e["modifier"]["set_context"]["requester"]
                    .as_str()
                    .unwrap_or("");
                road.asks
                    .insert(abs(from), req.trim_matches('\'').to_string());
            }
            if !is_ask
                && e["lane"].is_null()
                && abs(from).starts_with(&format!("{MEMBER}/channels/"))
            {
                road.outs
                    .push((abs(from), e["condition"].as_str().unwrap_or("").to_string()));
            }
            if is_answer {
                road.answers
                    .entry(abs(to))
                    .or_default()
                    .push(e["condition"].as_str().unwrap_or("").to_string());
            }
        }
        for n in d["diff"]["add_nodes"].as_array().into_iter().flatten() {
            let mut hs = Vec::new();
            grant_params(&n["override_params"], &mut hs);
            road.named.extend(hs);
        }
    }
    road
}

const MEMBER: &str = "/os/orgs/acme/members/alex";

fn grow_channel(name: &str, template: &str, extra: Value) -> Vec<Value> {
    let mut p = json!({"scope": MEMBER, "level": "channel", "name": name,
                       "template": template, "assistant": "scribe",
                       "ctx": {"member_person": "alex"}});
    for (k, v) in extra.as_object().expect("extra params") {
        p[k] = v.clone();
    }
    render("grow_level", p)
}

/// Every grant of `road` is answered on its own handle at `cell`, asked for by
/// that cell with the grant's own requester, and spends `cred_ref`.
fn assert_channel_granted(road: &ChannelRoad, cell: &str, want: &[(&str, &str)]) {
    let req = road
        .asks
        .get(cell)
        .unwrap_or_else(|| panic!("no credential_request edge from {cell}: {:?}", road.asks));
    let guards = road.answers.get(cell).cloned().unwrap_or_default();
    assert_eq!(
        road.grants.len(),
        want.len(),
        "one grant per spent key: {:?}",
        road.grants
    );
    // The ask is not a turn: no other edge out of the cell may carry it on.
    for (from, cond) in road.outs.iter().filter(|(f, _)| f == cell) {
        assert!(
            cond.contains("!(has(hop.route) && hop.route == 'credential_request')")
                || cond.starts_with("has(hop.error_code)")
                || cond.contains("has(hop.chat_id)"),
            "{from}: an edge that would carry the cell's credential_request on as a turn: {cond}"
        );
    }
    for (handle, cred_ref) in want {
        let g = road
            .grants
            .iter()
            .find(|g| g["grant_id"] == json!(handle))
            .unwrap_or_else(|| panic!("no grant {handle} seeded: {:?}", road.grants));
        assert_eq!(g["cred_ref"], json!(cred_ref), "{g}");
        assert_eq!(g["requester"].as_str(), Some(req.as_str()), "{g}");
        assert!(
            guards
                .iter()
                .any(|c| c.contains(&format!("hop.grant_id == '{handle}'"))),
            "no answer edge to {cell} addressed by {handle}: {guards:?}"
        );
    }
}

/// GH #1061 (OR-VG.V4.5): a channel grown by the recipe that spends a key --
/// a `voice` cell's recogniser and synthesiser, a Telegram connector's bot
/// token -- asks the member's broker like a brain does. The level renders the
/// grant rows, the two v-lanes and the handles in the SAME diff, or the cell
/// asks into the void and every session is refused after the wait.
/// `override_params` replaces a block whole, so a voice wish that names no
/// `stt`/`tts` block keeps the template's shipped handles; a wish that names
/// the block gets a handle of its own, so two voice channels of one member
/// never share a sealed box.
#[test]
fn gh1061_a_recipe_grown_voice_channel_is_granted() {
    if !shipped() {
        return; // GH #49
    }
    let creds = json!({"stt_cred_ref": "cred:deepgram", "tts_cred_ref": "cred:cartesia",
                       "subject": "member:alex",
                       "expires_at": "2099-01-01T00:00:00.000000Z"});
    let voice = read(&templates().join("voice/config.json"));
    let shipped_stt = voice["params"]["stt"]["credential_grant_id"]
        .as_str()
        .expect("stt handle");
    let shipped_tts = voice["params"]["tts"]["credential_grant_id"]
        .as_str()
        .expect("tts handle");

    // The shipped blocks, so the shipped handles.
    let a = channel_road(&grow_channel(
        "voice",
        "voice",
        json!({"credential": creds}),
    ));
    assert_channel_granted(
        &a,
        &format!("{MEMBER}/channels/voice"),
        &[
            (shipped_stt, "cred:deepgram"),
            (shipped_tts, "cred:cartesia"),
        ],
    );

    // Blocks of its own: handles of its own, named where the cell reads them.
    let b = channel_road(&grow_channel(
        "voice-b",
        "voice",
        json!({"credential": creds,
               "override_params": {"stt": voice["params"]["stt"].clone(),
                                   "tts": voice["params"]["tts"].clone()}}),
    ));
    let own: Vec<String> = b.named.iter().cloned().collect();
    assert_eq!(
        own.len(),
        2,
        "both blocks name a handle of their own: {own:?}"
    );
    assert!(
        !own.iter().any(|h| h == shipped_stt || h == shipped_tts),
        "a second voice channel must not share the shipped handles: {own:?}"
    );
    let stt = own
        .iter()
        .find(|h| h.contains("deepgram"))
        .expect("an stt handle");
    let tts = own
        .iter()
        .find(|h| h.contains("cartesia"))
        .expect("a tts handle");
    assert_channel_granted(
        &b,
        &format!("{MEMBER}/channels/voice-b"),
        &[
            (stt.as_str(), "cred:deepgram"),
            (tts.as_str(), "cred:cartesia"),
        ],
    );

    // The Telegram connector: one flat handle, always its own.
    let t = channel_road(&grow_channel(
        "telegram",
        "telegram-connector",
        json!({"bind_chat": "12345",
               "credential": {"cred_ref": "cred:telegram-bot", "subject": "member:alex",
                              "expires_at": "2099-01-01T00:00:00.000000Z"}}),
    ));
    let handle = t
        .named
        .iter()
        .next()
        .expect("bot_token_grant_id is named")
        .clone();
    assert_eq!(t.named.len(), 1, "{:?}", t.named);
    assert_channel_granted(
        &t,
        &format!("{MEMBER}/channels/telegram"),
        &[(handle.as_str(), "cred:telegram-bot")],
    );
}
