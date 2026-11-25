//! GH #1353 -- the whole road of a params-only update, from the shell's rim to
//! a cell behind a seal and back as a receipt.
//!
//! The tree is the shipped shell `meclaw-os` with a shipped `talky` under an
//! open organisation, member and generation. The edges between the two are not
//! written here: they are the ones the shipped builder recipe renders for that
//! generation. What is measured is the receipt at the shell's rim, the row in
//! the cell's own `cell.db` and the colony's dead letters.
//!
//! Free of a paid call by construction: the only provider is an in-process stub
//! and an update never reaches it. R2b / GH #49: a tree without the templates
//! skips.

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
use meclaw_colony::api_dto::{MessageLogFilter, ReadGraphReply};
use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::serde_json::{self, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::code_wire::{code_stdin, run_shipped_script, shipped_script};
use meclaw_testing::{ColonyHandle, wait};
use mock_openai::MockOpenAI;
use std::os::unix::fs::MetadataExt;
use tokio::sync::oneshot;

const SCOPE: &str = "/os/orgs";
const MEMBER: &str = "/os/orgs/acme/members/alex";
const GEN: &str = "/os/orgs/acme/members/alex/assistants/scribe";
const TALKY: &str = "/os/orgs/acme/members/alex/assistants/scribe/talky";
const RECIPES: &str = "templates/builder/recipes/config.json";

fn at(rest: &str) -> String {
    format!("{TALKY}/{rest}")
}

fn travelled() -> bool {
    shipped()
        && repo(RECIPES).is_file()
        && repo("templates/meclaw-os/config.json").is_file()
        && ["operator", "assistant"]
            .iter()
            .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
}

/// The edges the shipped recipe draws at the shell's container for the talky of
/// this generation: the registry's road, which ends on the generation's own
/// door (GH #1616), and the road of this file, which ends on the talky's.
fn the_recipes_edges() -> Vec<Value> {
    let mine = recipe_edges(
        json!({"scope": MEMBER, "level": "assistant", "name": "scribe",
               "template": "assistant",
               "ctx": {"model": "m", "model_fast": "f", "model_surface": "s"}}),
        &[GEN.to_string(), TALKY.to_string()],
    );
    for lane in [
        "'in_params'",
        "'params_applied'",
        "'in_model'",
        "'model_refused'",
    ] {
        assert!(
            mine.iter().any(|e| e.to_string().contains(lane)),
            "the recipe draws {lane}: {mine:#?}"
        );
    }
    mine
}

/// What the recipe draws at the container for a wish, on the paths `ends`.
fn recipe_edges(wish: Value, ends: &[String]) -> Vec<Value> {
    let doc = code_stdin(&json!({
        "target": "/os/builder/recipes",
        "header": {"hop": {"route": "recipe"}, "context": {}},
        "ttl": 64,
        "params": {"model_registry_scope": SCOPE},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "", "text": json!({
            "recipe": "grow_level", "request": "grow it", "params": wish
        }).to_string()}],
    }));
    let script = shipped_script(repo(RECIPES).to_str().expect("utf-8 path"));
    let out = run_shipped_script(&script, &doc.to_string());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let all: Value = serde_json::from_slice(&out.stdout).expect("json");
    let all = all.as_array().cloned().unwrap_or_else(|| vec![all.clone()]);
    let manifest = all
        .iter()
        .find(|m| m["header"]["operation"] == "recipe")
        .expect("an answer")["manifest"]
        .as_array()
        .expect("a manifest")
        .clone();
    let rel = |p: &str| format!(".{}", p.strip_prefix(SCOPE).expect("under it"));
    let ends: Vec<String> = ends.iter().map(|p| rel(p)).collect();
    manifest
        .iter()
        .filter(|d| d["scope"] == SCOPE)
        .flat_map(|d| {
            d["diff"]["add_edges"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|e| {
            ends.iter()
                .any(|p| e["from"] == p.as_str() || e["to"] == p.as_str())
        })
        .collect()
}

fn hive(edges: Vec<Value>) -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

struct Road {
    td: tempfile::TempDir,
    h: ColonyHandle,
    ports: Ports,
    _stub: MockOpenAI,
}

async fn grown() -> Road {
    grown_with(&[]).await
}

/// The tree with some of the member's own door hives beside its generation.
async fn grown_with(member_hives: &[&str]) -> Road {
    let stub = MockOpenAI::start(vec![]).await;
    let url = format!("{}/v1", stub.base_url);
    let td = tempfile::TempDir::new().expect("tempdir");
    let main = td.path().join("main");
    copy_resolved(&repo("templates/meclaw-os"), &main.join("os"), 0);
    copy_resolved(
        &repo("templates/talky"),
        &main.join(TALKY.trim_start_matches('/')),
        0,
    );
    for open in [
        "acme",
        "acme/members",
        "acme/members/alex",
        "acme/members/alex/assistants",
    ] {
        write_json(
            &main.join("os/orgs").join(open).join("config.json"),
            &hive(vec![]),
        );
    }
    // The generation's own edges between its rim and its talky, as shipped.
    let own: Vec<Value> =
        read_json(&repo("templates/assistant/config.json"))["params"]["graph"]["edges"]
            .as_array()
            .expect("edges")
            .iter()
            .filter(|e| {
                ["from", "to"]
                    .iter()
                    .all(|k| e[k] == "." || e[k] == "./talky")
            })
            .cloned()
            .collect();
    write_json(
        &main.join(GEN.trim_start_matches('/')).join("config.json"),
        &hive(own),
    );
    let mut road = the_recipes_edges();
    for name in member_hives {
        let path = format!("{MEMBER}/{name}");
        copy_resolved(
            &repo(&format!("templates/{name}")),
            &main.join(path.trim_start_matches('/')),
            0,
        );
        road.extend(recipe_edges(
            json!({"scope": "/os/orgs/acme", "level": "member", "name": "alex",
                   "template": "member", "ctx": {}}),
            &[path],
        ));
    }
    write_json(&main.join("os/orgs/config.json"), &hive(road));
    write_json(
        &main.join("config.json"),
        &hive(vec![json!({"from": "./os", "to": "/sink",
            "condition": "has(hop.route) && hop.route == 'params_receipt'"})]),
    );
    quiet_timers(&main);
    point_llms_at_stubs(&main, "", &url, &url, &json!({}));
    // The shell's own judges ask the broker for their credential; here the key
    // is in the config and nothing asks.
    let mut files = Vec::new();
    configs_under(&main, &mut files);
    for f in files {
        let mut cfg = read_json(&f);
        match cfg["cell"]["type"].as_str().expect("a type") {
            "llm" if cfg["params"]["credential_grant_id"] != "" => {
                cfg["params"]["credential_grant_id"] = json!("");
                write_json(&f, &cfg);
            }
            // The broker's vault is a process of its own and no part of the road.
            "vault" => write_json(
                &f,
                &json!({"cell": {"type": "code"},
                        "params": {"runner": "python3", "script_inline": "print('[]')"},
                        "contract": {"version": "1.0.0", "settings": {}, "consumes": {},
                                     "emits": {}, "capabilities": ["shell:exec"]}}),
            ),
            _ => {}
        }
    }
    write_env(td.path(), &main, &url);
    let (h, ports) = boot(&td).await;
    h.quiesce().await;
    dead_letters(&h).await;
    Road {
        td,
        h,
        ports,
        _stub: stub,
    }
}

impl Road {
    /// One params-only message at `door`, from outside the colony.
    async fn push(&self, door: &str, lane: &str, subscriber: &str, params: Value) {
        self.push_body(
            door,
            lane,
            subscriber,
            json!({"system": {}, "params": params}),
        )
        .await;
    }

    async fn push_body(&self, door: &str, lane: &str, subscriber: &str, body: Value) {
        self.h
            .send(
                MessageBuilder::new(Path::new(door))
                    .hop(map(json!({"route": lane, "subscriber": subscriber})))
                    .body(Body::Inline(body))
                    .ttl(64)
                    .build(),
            )
            .await;
    }

    async fn receipt(&mut self, what: &str) -> Message {
        answer_or_explain(&mut self.ports.sink, &self.h, what).await
    }

    /// An update at the shell's rim, and the one receipt it is answered with.
    async fn update(&mut self, subscriber: &str, params: Value) -> Message {
        self.update_body(subscriber, json!({"system": {}, "params": params}))
            .await
    }

    async fn update_body(&mut self, subscriber: &str, body: Value) -> Message {
        self.push_body("/os", "in_params", subscriber, body).await;
        let got = self.receipt(subscriber).await;
        self.h.quiesce().await;
        assert!(
            self.ports.sink.try_recv().is_err(),
            "{subscriber}: one update, one receipt"
        );
        got
    }

    fn db(&self, cell: &str) -> std::path::PathBuf {
        self.td
            .path()
            .join("main")
            .join(cell.trim_start_matches('/'))
            .join("cell.db")
    }

    /// The value the cell keeps for `key` in its own overlay.
    fn kept(&self, cell: &str, key: &str) -> Option<String> {
        rows(
            &self.db(cell),
            &format!("SELECT value FROM params WHERE key = '{key}'"),
        )
        .first()
        .and_then(|r| r.first().cloned())
    }

    async fn graph(&self) -> ReadGraphReply {
        let (ack, rx) = oneshot::channel();
        self.h
            .inbox_tx
            .send(ColonyMsg::ReadGraph {
                scope: Path::new("/"),
                ack,
            })
            .await
            .expect("send");
        rx.await.expect("graph")
    }

    async fn mutate(&self, payload: Value) -> MutationOutcome {
        let (ack, rx) = oneshot::channel();
        self.h
            .inbox_tx
            .send(ColonyMsg::Mutation {
                payload,
                reply_to: None,
                trace_id: Uuid::now_v7(),
                parent_message_id: Uuid::now_v7(),
                ack,
            })
            .await
            .expect("send");
        rx.await.expect("outcome")
    }

    async fn parked(&self) -> Vec<String> {
        self.graph()
            .await
            .nodes
            .into_iter()
            .filter(|n| n.parked || n.path.contains('~'))
            .map(|n| n.path)
            .collect()
    }
}

fn applied(m: &Message, cell: &str) {
    let hop = &m.headers.hop;
    assert_eq!(hop.get("route"), Some(&json!("params_receipt")), "{hop:?}");
    assert_eq!(
        hop.get("params_answer"),
        Some(&json!("params_applied")),
        "{hop:?}"
    );
    assert_eq!(hop.get("applied_subscriber"), Some(&json!(cell)), "{hop:?}");
    assert!(hop.get("error_code").is_none(), "{hop:?}");
}

/// OR-w2.tuer.2: the shell carries an update into `./orgs` and the answer of
/// the cell comes back at its rim, a refusal with the cell's code and detail.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_an_update_from_the_shell_reaches_orgs_and_comes_back_as_a_receipt() {
    if !travelled() {
        return;
    }
    let mut r = grown().await;
    let brain = at("brain");
    let got = r.update(&brain, json!({"tool_call_limit": 7})).await;
    applied(&got, &brain);
    assert_eq!(r.kept(&brain, "tool_call_limit").as_deref(), Some("7"));

    let got = r.update(&brain, json!({"no_such_key": 1})).await;
    let hop = &got.headers.hop;
    assert_eq!(hop.get("route"), Some(&json!("params_receipt")), "{hop:?}");
    assert_eq!(
        hop.get("params_answer"),
        Some(&json!("params_refused")),
        "{hop:?}"
    );
    assert_eq!(
        hop.get("refused_subscriber"),
        Some(&json!(brain)),
        "{hop:?}"
    );
    assert_eq!(
        hop.get("error_code"),
        Some(&json!("invalid_input")),
        "{hop:?}"
    );
    assert!(
        body_of(&got)["meta"]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("no_such_key")),
        "the cell's own detail: {}",
        body_of(&got)
    );
    assert_eq!(
        dead_letters(&r.h).await,
        vec![],
        "nothing of it dead-letters"
    );
    r.h.shutdown().await;
}

/// The shell's own hives are on the same road, each cell answering once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_the_shells_own_cells_answer_on_the_same_road() {
    if !travelled() {
        return;
    }
    let mut r = grown().await;
    for (cell, key) in [
        ("/os/access/store", "query_timeout_ms"),
        ("/os/access/clock", "query_timeout_ms"),
        ("/os/argus/judge", "tool_call_limit"),
        ("/os/builder/compose", "tool_call_limit"),
        ("/os/builder/builder-librarian/store", "query_timeout_ms"),
        ("/os/llm-registry/translate", "tool_call_limit"),
        ("/os/llm-registry/store", "query_timeout_ms"),
        ("/os/operator/drafts", "query_timeout_ms"),
        ("/os/operator/submit/store", "query_timeout_ms"),
    ] {
        // One form for every type: no `messages`, which a store's contract
        // owes on its work and an update does not.
        let got = r.update_body(cell, json!({"params": {key: 4321}})).await;
        applied(&got, cell);
        assert_eq!(r.kept(cell, key).as_deref(), Some("4321"), "{cell}");
    }
    assert_eq!(
        dead_letters(&r.h).await,
        vec![],
        "nothing of it dead-letters"
    );
    r.h.shutdown().await;
}

/// OR-w2.tuer.3: the shipped store owes `messages` on its work, and an update
/// without the slot reaches it all the same -- only on this lane, and only with
/// a `params` object. Anything else still falls at the store's own contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_store_takes_an_update_without_the_messages_its_work_owes() {
    if !travelled() {
        return;
    }
    let mut r = grown().await;
    let store = "/os/access/store";
    let owed = &read_json(&r.db(store).with_file_name("config.json"))["contract"]["consumes"]["body"]
        ["messages"];
    assert!(
        owed.is_object() && owed["required"] != false,
        "the shipped contract owes the slot: {owed}"
    );
    let got = r
        .update_body(store, json!({"params": {"query_timeout_ms": 4321}}))
        .await;
    applied(&got, store);
    assert_eq!(r.kept(store, "query_timeout_ms").as_deref(), Some("4321"));
    assert_eq!(dead_letters(&r.h).await, vec![]);

    r.push_body("/os", "in_params", store, json!({"params": [1]}))
        .await;
    r.h.quiesce().await;
    let got = dead_letters(&r.h).await;
    assert!(
        got.iter()
            .any(|(code, _, target, _)| code == "consumes_violation" && target == store),
        "{got:?}"
    );
    assert!(r.ports.sink.try_recv().is_err(), "and gives no receipt");
    r.h.shutdown().await;
}

/// The door carries an update and nothing else: an ordinary op, one with an
/// update beside it and a turn are refused by the cell they name, with the
/// receipt of a refused update, and leave nothing behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_work_through_the_door_is_refused_and_not_run() {
    if !travelled() {
        return;
    }
    let mut r = grown().await;
    let store = "/os/access/store";
    let quick = at("quick");
    let policy = |r: &Road| rows(&r.db(store), "SELECT rule_id FROM policy ORDER BY rule_id");
    // The control, and the store's first wake (its seed rows): on the same road
    // an update alone holds.
    let got = r
        .update_body(store, json!({"params": {"query_timeout_ms": 1111}}))
        .await;
    applied(&got, store);
    let stood = policy(&r);
    assert!(!stood.is_empty(), "the store holds its seed");
    let op = json!([{"origin": "assistant", "type": "tool_call", "id": "c1", "text": json!({
        "operation": "insert", "table": "policy",
        "row": {"rule_id": "gh1353", "requester": "*", "capability": "*", "verdict": "allow"}
    }).to_string()}]);
    let turn = json!([{"origin": "user", "type": "text", "text": "hello"}]);
    for (cell, body) in [
        (store, json!({"messages": op})),
        (
            store,
            json!({"messages": op, "params": {"query_timeout_ms": 4321}}),
        ),
        (
            quick.as_str(),
            json!({"messages": turn, "params": {"tool_call_limit": 3}}),
        ),
    ] {
        let got = r.update_body(cell, body.clone()).await;
        let hop = &got.headers.hop;
        assert_eq!(hop.get("route"), Some(&json!("params_receipt")), "{hop:?}");
        assert_eq!(
            hop.get("params_answer"),
            Some(&json!("params_refused")),
            "{cell}: {hop:?}"
        );
        assert_eq!(hop.get("refused_subscriber"), Some(&json!(cell)), "{hop:?}");
        assert_eq!(
            hop.get("error_code"),
            Some(&json!("invalid_input")),
            "{hop:?}"
        );
        assert!(
            body_of(&got)["meta"]["detail"]
                .as_str()
                .is_some_and(|d| d.contains("params-only")),
            "{cell}: the reason: {}",
            body_of(&got)
        );
    }
    assert_eq!(policy(&r), stood, "the op ran on nothing");
    assert_eq!(r.kept(store, "query_timeout_ms").as_deref(), Some("1111"));
    assert_eq!(r.kept(&quick, "tool_call_limit"), None);
    assert_eq!(dead_letters(&r.h).await, vec![]);
    r.h.shutdown().await;
}

/// The substrate answers a `transfer` slot above the cell, before its contract
/// and its own refusal. On the lane it does not: an import and an export through
/// a door reach the cell, which refuses them, and no row goes in or out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_transfer_through_the_door_is_refused_and_not_run() {
    let hives = ["firewall", "memory-hive"];
    if !travelled()
        || !hives
            .iter()
            .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
    {
        return;
    }
    let mut r = grown_with(&hives).await;
    let rules = format!("{MEMBER}/firewall/rules");
    let memory = format!("{MEMBER}/memory-hive/store");
    // The long-running half of the same seam.
    let clock = at("collector/clock");
    for cell in [&rules, &memory, &clock] {
        let cfg = read_json(&r.db(cell).with_file_name("config.json"));
        assert!(
            cfg["contract"]["transfer"].is_null(),
            "{cell} ships with a database that travels"
        );
    }
    // The control, and the first wake of each (a store's seed rows).
    for cell in [&rules, &clock] {
        let got = r.update(cell, json!({"query_timeout_ms": 1111})).await;
        applied(&got, cell);
    }
    let got = r.update(&memory, json!({"query_timeout_ms": 1111})).await;
    assert_eq!(
        got.headers.hop.get("error_code"),
        Some(&json!("write_denied"))
    );
    let held = |r: &Road| {
        [
            rows(&r.db(&rules), "SELECT rule_id FROM rules ORDER BY rule_id"),
            rows(&r.db(&memory), "SELECT model_id FROM emb_models"),
            rows(&r.db(&clock), "SELECT schedule_id FROM schedules"),
        ]
    };
    let stood = held(&r);
    assert!(
        !stood[0].is_empty() && !stood[1].is_empty(),
        "the stores hold their seed: {stood:?}"
    );
    dead_letters(&r.h).await;

    let import = json!({"operation": "import", "table": "rules", "key": ["rule_id"],
        "schema": {"rule_id": "text", "kind": "text", "field": "text", "value": "text",
                   "action": "text", "enabled": "int", "note": "text"},
        "rows": [{"rule_id": "gh1353", "kind": "substring", "field": "text", "value": "x",
                  "action": "pass", "enabled": 1, "note": ""}]});
    let schedule = json!({"operation": "import", "table": "schedules",
        "schema": {"schedule_id": "text", "schedule_name": "text", "kind": "text",
                   "at_utc": "text", "emit_to": "text", "emit_body_json": "text",
                   "emit_headers_json": "text", "status": "text", "created_at": "text"},
        "rows": [{"schedule_id": "gh1353", "schedule_name": "gh1353", "kind": "at",
                  "at_utc": "1970-01-01T00:00:00Z", "emit_to": "/sink",
                  "emit_body_json": "{}", "emit_headers_json": "{}", "status": "completed",
                  "created_at": "1970-01-01T00:00:00Z"}]});
    let export = |table: &str| json!({"operation": "export", "table": table});
    let params = json!({"query_timeout_ms": 4321});
    for (cell, body) in [
        (&rules, json!({"messages": [], "transfer": import})),
        (
            &rules,
            json!({"messages": [], "transfer": import, "params": params}),
        ),
        (&rules, json!({"messages": [], "transfer": export("rules")})),
        (
            &rules,
            json!({"messages": [],
                   "transfer": {"operation": "export", "table": "rules", "to": "gh1353.json"}}),
        ),
        (
            &memory,
            json!({"messages": [], "transfer": export("emb_models")}),
        ),
        (
            &memory,
            json!({"messages": [], "transfer": export("emb_models"), "params": params}),
        ),
        (&clock, json!({"transfer": schedule})),
        (&clock, json!({"transfer": schedule, "params": params})),
        (&clock, json!({"transfer": export("schedules")})),
    ] {
        r.push_body("/os", "in_params", cell, body.clone()).await;
        r.h.quiesce().await;
        assert_eq!(held(&r), stood, "{cell}: {body}");
        let Ok(got) = r.ports.sink.try_recv() else {
            panic!("{cell}: no receipt: {:?}", dead_letters(&r.h).await);
        };
        let hop = &got.headers.hop;
        assert_eq!(hop.get("route"), Some(&json!("params_receipt")), "{hop:?}");
        assert_eq!(
            hop.get("params_answer"),
            Some(&json!("params_refused")),
            "{cell}: {hop:?}"
        );
        assert_eq!(hop.get("refused_subscriber"), Some(&json!(cell)), "{hop:?}");
        assert_eq!(
            hop.get("error_code"),
            Some(&json!("invalid_input")),
            "{hop:?}"
        );
        assert!(
            body_of(&got)["meta"]["detail"]
                .as_str()
                .is_some_and(|d| d.contains("params-only")),
            "{cell}: the reason: {}",
            body_of(&got)
        );
        assert!(r.ports.sink.try_recv().is_err(), "{cell}: one receipt");
        assert_eq!(dead_letters(&r.h).await, vec![], "{cell}");
        assert_eq!(
            r.kept(cell, "query_timeout_ms").as_deref(),
            (cell != &memory).then_some("1111"),
            "{cell}"
        );
    }

    // A store's contract owes `messages` on whatever is no update, and it is
    // asked first: the same import without the slot ends there.
    r.push_body("/os", "in_params", &rules, json!({"transfer": import}))
        .await;
    r.h.quiesce().await;
    assert_eq!(held(&r), stood);
    let got = dead_letters(&r.h).await;
    assert!(
        got.iter()
            .any(|(code, _, target, _)| code == "consumes_violation" && *target == rules),
        "{got:?}"
    );
    assert!(r.ports.sink.try_recv().is_err(), "and gives no receipt");
    let log = r.h.read_log(MessageLogFilter::default()).await;
    assert!(
        !log.iter().any(|m| ["import", "export"]
            .iter()
            .any(|op| m.headers_json.contains(&format!("\"operation\":\"{op}\"")))),
        "no transfer was answered"
    );

    // The same import off the lane, from a sender inside the colony, runs.
    r.h.send(
        MessageBuilder::new(Path::new(&rules))
            .parent_message_id(Uuid::now_v7())
            .reply_to(Path::new("/sink"))
            .body(Body::Inline(json!({"transfer": import})))
            .ttl(64)
            .build(),
    )
    .await;
    let got = r.receipt("the transfer off the lane").await;
    assert_eq!(
        got.headers.hop.get("operation"),
        Some(&json!("import")),
        "{:?}",
        got.headers.hop
    );
    assert!(
        held(&r)[0].iter().any(|row| row[0] == "gh1353"),
        "the row stands: {:?}",
        held(&r)
    );
    r.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_tool_call_limit_reaches_quick_inside_sealed_talky() {
    if !travelled() {
        return;
    }
    let mut r = grown().await;
    let quick = at("quick");
    let got = r.update(&quick, json!({"temperature": 0.5})).await;
    applied(&got, &quick);
    let before = std::fs::metadata(r.db(&quick)).expect("cell.db").ino();

    let got = r.update(&quick, json!({"tool_call_limit": 3})).await;
    applied(&got, &quick);
    assert_eq!(r.kept(&quick, "tool_call_limit").as_deref(), Some("3"));
    assert_eq!(
        r.kept(&at("brain"), "tool_call_limit"),
        None,
        "the neighbour behind the same door moved nothing"
    );
    assert_eq!(
        r.kept(&quick, "temperature").as_deref(),
        Some("0.5"),
        "what the cell kept before is still there"
    );
    assert_eq!(
        std::fs::metadata(r.db(&quick)).expect("cell.db").ino(),
        before,
        "the store is the one the cell had"
    );
    assert_eq!(r.parked().await, Vec::<String>::new());
    r.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_timer_inside_a_nested_seal_is_reached() {
    if !travelled() {
        return;
    }
    for sealed in ["talky", "collector", "curator"] {
        assert!(
            read_json(&repo(&format!("templates/{sealed}/config.json")))["params"]["ports"]
                .is_array(),
            "{sealed} ships sealed"
        );
    }
    let mut r = grown().await;
    // Two seals deep, and a clock of the same name behind either of them.
    let clock = at("collector/clock");
    let got = r.update(&clock, json!({"query_timeout_ms": 4321})).await;
    applied(&got, &clock);
    assert_eq!(r.kept(&clock, "query_timeout_ms").as_deref(), Some("4321"));
    assert_eq!(r.kept(&at("curator/clock"), "query_timeout_ms"), None);
    r.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_an_ordinary_message_into_the_sealed_hive_is_still_refused() {
    if !travelled() {
        return;
    }
    let r = grown().await;
    let quick = at("quick");
    // The door has no `at`: not even its own lane opens the inside from outside.
    for lane in ["in_turn", "in_params"] {
        r.push(&quick, lane, &quick, json!({"tool_call_limit": 3}))
            .await;
        r.h.quiesce().await;
        let codes = dead_letters(&r.h).await;
        assert!(
            codes
                .iter()
                .any(|(code, _, target, _)| code == "hive_boundary" && *target == quick),
            "{lane}: {codes:?}"
        );
        assert_eq!(r.kept(&quick, "tool_call_limit"), None, "{lane}");
    }
    r.h.shutdown().await;
}

/// OR-W2-P-4: the door stands at the hive's own path, so it takes the lane from
/// outside as `in_model` always did.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_door_hive_takes_in_params_from_outside_like_in_model() {
    if !travelled() {
        return;
    }
    let mut r = grown().await;
    let quick = at("quick");
    r.push(TALKY, "in_model", &quick, json!({"model": "test/m2"}))
        .await;
    r.push(TALKY, "in_params", &quick, json!({"tool_call_limit": 3}))
        .await;
    let got = r.receipt("the answer climbs the same way back").await;
    applied(&got, &quick);
    assert_eq!(r.kept(&quick, "model").as_deref(), Some("\"test/m2\""));
    assert_eq!(r.kept(&quick, "tool_call_limit").as_deref(), Some("3"));
    r.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_a_target_without_a_door_ends_as_hive_no_route() {
    if !travelled() {
        return;
    }
    let mut r = grown().await;
    let code_cell = at("errors");
    for (subscriber, ends_at) in [
        ("/elsewhere/cell", "/os/operator"),
        ("/os/orgs/nobody/cell", "/os/orgs"),
        (code_cell.as_str(), TALKY),
    ] {
        r.push("/os", "in_params", subscriber, json!({"x": 1}))
            .await;
        r.h.quiesce().await;
        let got = dead_letters(&r.h).await;
        let here: Vec<_> = got
            .iter()
            .map(|(code, _, target, _)| (code.as_str(), target.as_str()))
            .collect();
        assert_eq!(
            here,
            vec![("hive_no_route", ends_at)],
            "{subscriber}: {got:?}"
        );
        assert!(r.ports.sink.try_recv().is_err(), "{subscriber}: no receipt");
    }
    r.h.shutdown().await;
}

/// The registry's push takes the road it always took: applied in silence, and a
/// refusal goes back to the registry and to nobody else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_the_registry_model_push_is_unchanged() {
    if !travelled() {
        return;
    }
    let mut r = grown().await;
    let brain = at("brain");
    r.push(
        "/os/llm-registry",
        "update",
        &brain,
        json!({"model": "test/m2"}),
    )
    .await;
    wait::until_some_with(
        "the package is kept",
        || {
            let kept = r.kept(&brain, "model");
            async move { (kept.as_deref() == Some("\"test/m2\"")).then_some(()) }
        },
        || async { format!("dead letters: {:#?}", dead_letters(&r.h).await) },
    )
    .await;
    r.push(
        "/os/llm-registry",
        "update",
        &brain,
        json!({"model": "test/m3", "base_url": "https://elsewhere.example/v1"}),
    )
    .await;
    r.h.quiesce().await;
    let log = r.h.read_log(MessageLogFilter::default()).await;
    assert!(
        log.iter().any(|m| m.to_path == "/os/llm-registry"
            && m.headers_json.contains("in_refused")
            && m.headers_json.contains("test/m3")),
        "the refusal reaches the registry"
    );
    assert!(
        !log.iter().any(|m| m.headers_json.contains("params_answer")),
        "a push on the registry's lane is answered on no params lane"
    );
    assert!(r.ports.sink.try_recv().is_err(), "and gives no receipt");
    assert_eq!(r.kept(&brain, "model").as_deref(), Some("\"test/m2\""));
    r.h.shutdown().await;
}

/// The second half of ruling C: both ends inside the seal, so the edge is an
/// ordinary edge, and the cells it joins stay the cells they were.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_an_edge_inside_a_sealed_hive_is_added_without_rebirth() {
    if !travelled() {
        return;
    }
    let mut r = grown().await;
    let quick = at("quick");
    let got = r.update(&quick, json!({"tool_call_limit": 3})).await;
    applied(&got, &quick);
    let before = std::fs::metadata(r.db(&quick)).expect("cell.db").ino();
    let nodes = |g: &ReadGraphReply| {
        let mut n: Vec<String> = g.nodes.iter().map(|n| n.path.clone()).collect();
        n.sort();
        n
    };
    let stood = nodes(&r.graph().await);

    let outcome = r
        .mutate(json!({"scope": TALKY, "diff": {"add_edges": [
            {"from": "./quick", "to": "./errors",
             "condition": "has(hop.route) && hop.route == 'gh1353_probe'"}]}}))
        .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{outcome:?}"
    );
    let graph = r.graph().await;
    assert!(
        graph.edges.iter().any(|e| e.from == quick
            && e.to == at("errors")
            && format!("{e:?}").contains("gh1353_probe")),
        "the edge stands in the edge table"
    );
    assert_eq!(nodes(&graph), stood, "no node came, went or was parked");
    assert_eq!(
        std::fs::metadata(r.db(&quick)).expect("cell.db").ino(),
        before
    );
    let got = r.update(&quick, json!({"temperature": 0.5})).await;
    applied(&got, &quick);
    assert_eq!(r.kept(&quick, "tool_call_limit").as_deref(), Some("3"));
    r.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1353_an_edge_across_the_seal_is_refused() {
    if !travelled() {
        return;
    }
    let r = grown().await;
    let deep = format!(".{}", at("quick").strip_prefix("/os").expect("under /os"));
    for edge in [
        json!({"from": "./operator", "to": deep,
               "condition": "has(hop.route) && hop.route == 'in_params'"}),
        // The lane has no connect point, so naming it opens nothing either.
        json!({"from": "./operator", "to": deep, "lane": "in_params",
               "condition": "has(hop.route) && hop.route == 'in_params'"}),
    ] {
        let outcome = r
            .mutate(json!({"scope": "/os", "diff": {"add_edges": [edge.clone()]}}))
            .await;
        match &outcome {
            MutationOutcome::Rejected { error_code, .. } => assert!(
                ["hive_port_boundary", "v_lane_no_connect_point"].contains(&error_code.as_str()),
                "{edge}: {outcome:?}"
            ),
            other => panic!("{edge}: {other:?}"),
        }
    }
    r.h.shutdown().await;
}
