//! GH #982 (OR-NL-159 (1)) -- the presenter's decider runs on a grant: its key lives in the
//! member's own `access`, and no installation pulls it from `.env`.
//!
//! WHAT THIS FILE IS
//! =================
//! Every cell of the library that spends a provider key could take the grant way since
//! GH #560 -- except the presenter's `decide`. Its params carried no `credential_grant_id`,
//! so an `override_params` that set one was refused by the param-existence check
//! (`check_override_params`, `crates/meclaw-colony/src/mutation/subtree.rs`), its rim named
//! no connect point for the credential lane, and the builder's `install_app` had no road to
//! draw. Now `decide` carries the param (shipped empty), the presenter's rim names `./decide`
//! as the connect point of `credential_request` and `in_sealed`, and `install_app` renders
//! the whole road from one `credential` object in the wish.
//!
//! | claim | test |
//! |---|---|
//! | `install_app` with a grant renders the empty `api_key`, `credential_grant_id`, the grant and both access v-lanes for `apps/presenter/decide`; another app's grant is refused by name | [`a_install_app_renders_the_deciders_grant`] |
//! | a presenter whose decider holds a grant and no key anywhere asks the member's broker, and the decide request reaches the provider with the vault's bearer, which is on no record | [`b_the_decider_calls_with_the_granted_bearer`] |
//!
//! (a) runs the SHIPPED recipe and needs the recipe change of the strand
//! (`recipe_patch_grant.py`, applied to `templates/builder/recipes/config.json` at the merge);
//! it is written against that state. (b) does not read the recipe: it draws the two v-lanes
//! and the grant by hand, in exactly the form (a) pins the recipe to, so the substrate and
//! the presenter template are proven on their own.
//!
//! WHAT (b) BOOTS
//! ==============
//! A shell with one member `/alex` holding the SHIPPED presenter under `./apps/presenter`
//! (its `decide` pointed at a loopback provider, `api_key` EMPTY, `credential_grant_id`
//! set) and the SHIPPED `access` under `./access` (its vault unlocked by an environment
//! variable and filled the way `meclaw --vault-add` fills it). The v-lanes and the grant
//! arrive through the mutation door at `/alex`, so Stage 6 judges the presenter's connect
//! point. The `.env` names no decider key. One turn enters the presenter; the measure is at
//! the RECEIVER: the `authorization` header of the decide request the provider got.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not carry the library
//! is skipped, never judged.

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::vault::VaultCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationDoorOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, from_str, json, to_string_pretty};
use meclaw_core::{Body, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// The credential the member's vault holds. A fixture, not a key.
const SECRET: &str = "sk-test-not-a-key-gh976-decider";
const PASSPHRASE: &str = "a passphrase nobody guesses gh976";
const UNLOCK_ENV: &str = "GH976_PRESENTER_VAULT_PASSPHRASE";
const CRED_REF: &str = "cred:example-decider:primary";
const SUBJECT: &str = "member:alex";
const EXPIRES: &str = "2099-01-01T00:00:00.000000Z";
/// The handle the recipe builds: `grant:<cred tail>@<subject>/<app>-<cell>`.
const GRANT: &str = "grant:example-decider-primary@member-alex/presenter-decide";
const REQUESTER: &str = "app:presenter/decide";
const DECIDE: &str = "/alex/apps/presenter/decide";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/presenter/config.json",
        "templates/presenter/decide/config.json",
        "templates/access/config.json",
        "templates/access/vault/config.json",
        "templates/builder/recipes/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write_json(path: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, to_string_pretty(v).unwrap()).unwrap();
}

fn patch_json(path: &std::path::Path, f: impl FnOnce(&mut Value)) {
    let mut v = read_json(path);
    f(&mut v);
    write_json(path, &v);
}

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

/// The two credential v-lanes of the decider, at the member's scope -- the form the recipe
/// renders (pinned in (a)) and (b) draws.
fn v_lane_edges() -> Vec<Value> {
    vec![
        json!({
            "from": "./apps/presenter/decide", "to": "./access",
            "lane": "credential_request",
            "condition": "has(hop.route) && hop.route == 'credential_request'",
            "modifier": {"set_hop": {"route": "'in_invoke'"},
                         "set_context": {"requester": format!("'{REQUESTER}'")}}
        }),
        json!({
            "from": "./access", "to": "./apps/presenter/decide",
            "lane": "in_sealed",
            "condition": format!(
                "has(hop.route) && hop.route == 'ack' && has(hop.operation) && \
                 hop.operation == 'vault.deliver' && has(hop.grant_id) && \
                 hop.grant_id == '{GRANT}'"),
            "modifier": {"set_hop": {"route": "'in_sealed'"}}
        }),
    ]
}

/// The recipe's answer to one `install_app` wish for the presenter.
fn render(app: &str, credential: Value) -> Value {
    render_from(app, &format!("{app}@1.1.0"), credential)
}

/// `install_app` for `app` grown from `template`.
fn render_from(app: &str, template: &str, credential: Value) -> Value {
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(
            repo("templates/builder/recipes/config.json")
                .to_str()
                .expect("a utf-8 path"),
        ),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": {"scope": "/alex", "app": app,
                                                    "template": template,
                                                    "screen": "display",
                                                    "generation": "sam",
                                                    "declaration": {"listens": ["turn"]},
                                                    "ctx": {"member_person": "alex"},
                                                    "credential": credential}})
                                      .to_string()}],
        }),
    );
    out.into_iter().next().expect("the recipe emitted nothing")
}

fn credential() -> Value {
    json!({"cred_ref": CRED_REF, "subject": SUBJECT, "expires_at": EXPIRES})
}

/// The recipe half, and the template half it relies on.
#[test]
fn a_install_app_renders_the_deciders_grant() {
    if !shipped() {
        return;
    }
    // The template: the param exists (so `override_params` naming it is not refused),
    // ships empty, is a declared setting, and the rim names `./decide` for both lanes.
    let decide = read_json(&repo("templates/presenter/decide/config.json"));
    assert_eq!(decide["params"]["credential_grant_id"], json!(""));
    assert!(decide["params"]["credential_wait_ms"].is_number());
    assert_eq!(
        decide["contract"]["settings"]["credential_grant_id"]["type"],
        json!("string")
    );
    let hive = read_json(&repo("templates/presenter/config.json"));
    let at_decide = |list: &str, route: &str| {
        hive["params"]["contract"][list]
            .as_array()
            .into_iter()
            .flatten()
            .any(|e| e["route"] == json!(route) && e["at"] == json!(["./decide"]))
    };
    assert!(
        at_decide("emits", "credential_request"),
        "no connect point for the ask"
    );
    assert!(
        at_decide("accepts", "in_sealed"),
        "no connect point for the box"
    );

    // The recipe.
    let first = render("presenter", credential());
    assert!(
        first["header"]["error_code"].is_null(),
        "the recipe refused the presenter's grant: {first}"
    );
    let diff = &first["manifest"][0]["diff"];
    assert_eq!(first["manifest"][0]["scope"], json!("/alex"));
    assert_eq!(
        diff["add_nodes"][0]["override_params"]["decide"],
        json!({"api_key": "", "credential_grant_id": GRANT}),
        "the empty key is the switch, the grant is the road: {diff}"
    );
    let edges = diff["add_edges"].as_array().expect("edges");
    for want in v_lane_edges() {
        assert!(
            edges.contains(&want),
            "the v-lane {want} is not rendered: {edges:?}"
        );
    }
    let rows = diff["seed_rows"].as_array().expect("seed_rows");
    let grants = rows
        .iter()
        .find(|r| r["table"] == json!("grants"))
        .expect("a grants seed");
    assert_eq!(grants["target"], json!("./access/store"));
    let g = &grants["rows"][0];
    assert_eq!(g["grant_id"], json!(GRANT));
    assert_eq!(g["requester"], json!(REQUESTER));
    assert_eq!(g["cred_ref"], json!(CRED_REF));
    assert_eq!(g["subject"], json!(SUBJECT));
    assert_eq!(g["expires_at"], json!(EXPIRES));
    assert_eq!(g["scope"], json!({"actions": ["vault.deliver"]}));
    let events = rows
        .iter()
        .find(|r| r["table"] == json!("grant_events"))
        .expect("a grant_events seed");
    assert_eq!(events["rows"][0]["grant_id"], json!(GRANT));
    assert_eq!(events["rows"][0]["event"], json!("granted"));

    // An app with no cell that spends a grant is refused by name, never rendered without.
    let refused = render("colony-view", credential());
    assert_eq!(
        refused["header"]["error_code"],
        json!("app_declaration_invalid"),
        "a grant for an app with no such cell must be named: {refused}"
    );
}

/// Y fix round 1, M-1: the positive list is the TEMPLATE's. An app called `presenter`
/// grown from another template gets no grant, and the presenter under another app name
/// gets none either (its cell paths and requester would name a cell the list does not).
#[test]
fn a_grant_follows_the_template_not_the_app_name() {
    if !shipped() {
        return;
    }
    for (app, template) in [("presenter", "objects@1.0.0"), ("deck", "presenter@1.2.2")] {
        let refused = render_from(app, template, credential());
        assert_eq!(
            refused["header"]["error_code"],
            json!("app_declaration_invalid"),
            "a grant for {app} from {template} must be refused by name: {refused}"
        );
        assert!(
            refused["manifest"].is_null(),
            "nothing is rendered for a refused grant: {refused}"
        );
    }
}

/// Y fix round 1, M-2: `finish_reason` stays required on every decision; only the
/// credential request (`hop.route == "credential_request"`, the llm cell's ask for its
/// sealed key, before any model call) may leave it out.
#[test]
fn the_decider_owes_a_finish_reason_except_on_its_credential_request() {
    if !shipped() {
        return;
    }
    let decide = read_json(&repo("templates/presenter/decide/config.json"));
    let emits: meclaw_core::EmitsBlock =
        meclaw_core::serde_json::from_value(decide["contract"]["emits"].clone())
            .expect("the decider's emits parse");
    let compiled = meclaw_core::CompiledEmits::compile(&emits).expect("the emits compile");
    let ask = json!({
        "header": {"route": "credential_request", "grant_id": GRANT},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "x", "text": "{}"}],
    });
    meclaw_core::validate_emits(&ask, &compiled)
        .unwrap_or_else(|e| panic!("the credential request passes the contract: {e}"));
    let decision_without = json!({
        "header": {"model": "m", "latency_ms": 3},
        "messages": [{"origin": "assistant", "type": "text", "text": "{}"}],
    });
    let err = meclaw_core::validate_emits(&decision_without, &compiled)
        .expect_err("a decision without finish_reason breaks the contract");
    assert!(err.contains("finish_reason"), "{err}");
    let decision_with = json!({
        "header": {"finish_reason": "stop", "model": "m", "latency_ms": 3},
        "messages": [{"origin": "assistant", "type": "text", "text": "{}"}],
    });
    meclaw_core::validate_emits(&decision_with, &compiled)
        .unwrap_or_else(|e| panic!("a decision with finish_reason passes: {e}"));
}

// ─────────────────────────────────────────────────────────────── the colony

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("vault".to_string(), Arc::new(VaultCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
    ]
}

/// Fill the member's vault the way `meclaw --vault-add` does: straight into its own
/// `cell.db`, with no colony running.
fn seed_vault_secret(root: &std::path::Path) {
    use meclaw_cells::vault::crypto::MasterKey;
    use meclaw_cells::vault::store as vs;
    let dir = root.join("main/alex/access/vault");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&dir.join("cell.db")).unwrap();
    vs::apply_ddl(&conn).unwrap();
    let salt = vs::salt_or_create(&conn).unwrap();
    let key = MasterKey::derive(PASSPHRASE.as_bytes(), &salt).unwrap();
    let (nonce, ct) = key.seal(SECRET.as_bytes()).unwrap();
    vs::put(&conn, CRED_REF, &nonce, &ct, &vs::now_iso()).unwrap();
}

fn grow_tree(root: &std::path::Path, base_url: &str) {
    copy_tree(&repo("templates"), &root.join("templates"));
    write_json(
        &root.join("main/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./alex", "to": "."}]}}}),
    );
    write_json(
        &root.join("main/alex/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./apps", "to": "."}, {"from": "./access", "to": "."}]}}}),
    );
    write_json(
        &root.join("main/alex/apps/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./presenter", "to": "."}]}}}),
    );
    let presenter = root.join("main/alex/apps/presenter");
    copy_tree(&repo("templates/presenter"), &presenter);
    patch_json(&presenter.join("decide/config.json"), |v| {
        v["params"]["model"] = json!("mock-decider");
        v["params"]["base_url"] = json!(base_url);
        // No bearer of its own (an empty string is not a bearer, GH #271), a grant instead.
        v["params"]["api_key"] = json!("");
        v["params"]["credential_grant_id"] = json!(GRANT);
        v["params"]["external_timeout_ms"] = json!(10_000);
        v["cell"]["message_timeout"] = json!(40_000);
    });
    copy_tree(&repo("templates/access"), &root.join("main/alex/access"));
    patch_json(&root.join("main/alex/access/vault/config.json"), |v| {
        v["params"]["unlock_env"] = json!(UNLOCK_ENV);
    });
    // The access clock carries `${uuid7:...}`, which only the mutation door
    // resolves; a copied tree boots with a literal id, and the sweep never
    // fires inside the run.
    patch_json(&root.join("main/alex/access/clock/config.json"), |v| {
        for s in v["params"]["schedules"].as_array_mut().unwrap() {
            s["schedule_id"] = json!("01900000-0000-7000-8000-000000000982");
            s["cron"] = json!(meclaw_testing::NEVER_CRON);
        }
    });
    // The decider's key is in NO `.env`: the file names nothing a cell could spend.
    std::fs::write(root.join(".env"), "GH976_UNRELATED=1\n").unwrap();
    seed_vault_secret(root);
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let root = td.path();
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx.await.expect("rescan ack").expect("rescan aborted");
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the shell must boot");
    h
}

async fn apply(h: &ColonyHandle, payload: Value) -> MutationDoorOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::MutationDoor {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send manifest");
    ack_rx.await.expect("manifest ack")
}

/// The road and the grant, through the mutation door at the member -- the lowest common
/// ancestor of the decider and the broker.
fn road_manifest() -> Value {
    json!({"manifest": [{
        "scope": "/alex",
        "diff": {
            "add_edges": v_lane_edges(),
            "seed_rows": [
                {"target": "./access/store", "table": "grants", "rows": [{
                    "grant_id": GRANT, "requester": REQUESTER,
                    "capability": "credential.read", "subject": SUBJECT,
                    "scope": {"actions": ["vault.deliver"]}, "cred_ref": CRED_REF,
                    "purpose": "authenticate the presenter app's decide cell against the provider",
                    "issued_at": "2026-01-01T00:00:00.000000Z", "expires_at": EXPIRES,
                    "rule_id": "credential-read", "constraints": {"rate_per_min": 60}}]},
                {"target": "./access/store", "table": "grant_events", "rows": [{
                    "id": "ev-gh976-0000000001", "grant_id": GRANT, "event": "granted",
                    "at": "2026-01-01T00:00:00.000000Z", "actor": "operator",
                    "reason_code": "", "detail": {"why": "seeded with the app"}}]},
            ],
        }
    }]})
}

fn decision() -> MockResponse {
    MockResponse::ok_json(
        json!({"answers": {}, "model": "mock-decider",
               "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0}})
        .to_string()
        .as_bytes(),
    )
}

fn log_rows(root: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT headers, COALESCE(body_payload, '') FROM message_log")
        .expect("message_log");
    st.query_map([], |r| {
        Ok(format!(
            "{} {}",
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .collect()
}

/// `from -> to route error_code` of every delivery, the trail a failure shows.
fn trail(root: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare(
            "SELECT from_path, to_path, headers, COALESCE(body_payload, '') FROM message_log \
             ORDER BY rowid",
        )
        .expect("message_log");
    st.query_map([], |r| {
        let h: Value =
            meclaw_core::serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or(Value::Null);
        Ok(format!(
            "{} -> {} route={} error={} {}",
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            h["hop"]["route"],
            h["hop"]["error_code"],
            if h["hop"]["error_code"].is_null() {
                String::new()
            } else {
                r.get::<_, String>(3)?.chars().take(400).collect()
            }
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .collect()
}

/// `set_var` is unsafe in edition 2024; sound here because the one test that writes it
/// writes it before the cell that reads it exists.
fn arm_passphrase() {
    unsafe { std::env::set_var(UNLOCK_ENV, PASSPHRASE) };
}

/// The claim at the receiver: the decide request carries the vault's bearer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b_the_decider_calls_with_the_granted_bearer() {
    if !shipped() {
        return;
    }
    arm_passphrase();
    let (addr, _server, captured) =
        start_mock_server_capturing(vec![decision(), decision(), decision()]).await;
    let td = tempfile::TempDir::new().unwrap();
    grow_tree(td.path(), &format!("http://{addr}"));
    let env = std::fs::read_to_string(td.path().join(".env")).unwrap();
    assert!(
        !env.contains("PRESENTER_DECIDE_API_KEY") && !env.contains(SECRET),
        "the decider's key must be in no .env"
    );

    let h = boot(&td).await;
    let outcome = apply(&h, road_manifest()).await;
    assert!(
        outcome.is_committed(),
        "the decider's credential v-lanes must commit -- the presenter names `./decide` \
         as their connect point; got {outcome:?}"
    );

    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("turn"));
    hop.insert("turn_id".to_string(), json!("t-gh976"));
    let mut ctx = Map::new();
    ctx.insert(
        "audience_set".to_string(),
        json!(r#"["agent:sam","member:alex"]"#),
    );
    h.send(
        MessageBuilder::new(Path::new("/alex/apps/presenter"))
            .hop(hop)
            .context(ctx)
            .body(Body::Inline(json!({"messages": [
                {"origin": "user", "type": "text", "text": "what is the weather today"}]})))
            .ttl(64)
            .build(),
    )
    .await;

    let mut seen = Vec::new();
    for _ in 0..240 {
        seen = captured.lock().await.clone();
        if !seen.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if seen.is_empty() {
        let dead: Vec<String> = h
            .drain_dead_letters()
            .await
            .iter()
            .map(|d| {
                format!(
                    "{} -> {} [{}] {:?}",
                    d.sender_path.as_str(),
                    d.resolved_target.as_str(),
                    d.reason.as_code(),
                    d.message.headers.hop
                )
            })
            .collect();
        let trail: Vec<String> = trail(td.path());
        panic!(
            "the decider never reached the provider -- the credential round did not close; \
             trail: {trail:#?}; dead letters: {dead:#?}"
        );
    }
    let first = &seen[0];
    assert_eq!(
        first.headers.get("authorization").map(String::as_str),
        Some(format!("Bearer {SECRET}").as_str()),
        "the decide request does not carry the member vault's bearer: {:?}",
        first.headers
    );
    h.shutdown().await;

    // The box is on record (the lane ran), the value is not (the lane did not leak).
    let log = log_rows(td.path());
    assert!(
        log.iter()
            // The decider's `credential_request` is journalled as the access hive
            // receives it: the v-lane edge hands it in on `in_invoke`.
            .any(|r| r.contains(r#""route":"in_invoke""#) && r.contains(GRANT)),
        "no credential_request from {DECIDE} was journalled: {:#?}",
        log.iter()
            .filter(|r| r.contains("credential") || r.contains("grant"))
            .map(|r| r.chars().take(500).collect::<String>())
            .collect::<Vec<_>>()
    );
    assert!(
        log.iter().any(|r| r.contains("\"epk\"")
            && r.contains("\"nonce\"")
            && r.contains("\"ciphertext\"")),
        "no sealed box was journalled -- the absence below would prove nothing"
    );
    let hits: Vec<&String> = log
        .iter()
        .filter(|r| r.contains(SECRET) || r.contains(PASSPHRASE))
        .collect();
    assert!(hits.is_empty(), "the credential is on record: {hits:?}");
}
