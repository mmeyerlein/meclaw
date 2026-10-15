//! GH #452 — `examples/vault-pilot`: one manifest, and a model that holds no key.
//!
//! WHAT THIS FILE IS
//! =================
//! The example is the first shipped artefact that wires the capability broker to
//! a consumer. It is checked in as three things a template library cannot decide
//! for you — an empty root hive, the broker's `store` with the rows that say
//! WHICH credential this colony holds and WHICH grant hangs on it, and the one
//! `llm` cell that names that grant — plus `grow.json`, which grows the other
//! five broker cells and the drain out of `templates/access`.
//!
//! The four claims, one test each:
//!
//! | claim | test |
//! |---|---|
//! | one manifest grows the tree, and the seeded grant is there before anything boots | [`a_one_manifest_grows_the_broker_and_the_grant_is_on_disk_before_the_boot`] |
//! | the model authenticates from the vault, with no `api_key` anywhere | [`b_the_pilot_model_answers_with_a_credential_it_never_had_in_its_config`] |
//! | without the grant row the same tree calls no provider at all | [`c_without_the_grant_row_the_model_stays_pending_and_never_calls_out`] |
//! | the grant survives a restart, the credential does not, and the turn does | [`d_the_grant_outlives_the_colony_and_the_credential_does_not`] |
//!
//! WHY THE STORE IS CHECKED IN
//! ===========================
//! `params.credential_grant_id` is IMMUTABLE (`crates/meclaw-cells/src/llm/params.rs`,
//! `IMMUTABLE_PARAM_KEYS`): no message may repoint it, so no message can mint it
//! either, and the `grants` row it names has to exist before the `llm` cell ever
//! boots. Nothing in a manifest can write that row — the diff vocabulary is seven
//! topology operations (`crates/meclaw-colony/src/mutation/validate.rs`,
//! `DIFF_OPERATIONS`) and not one of them writes to a store. What CAN put rows in
//! is a `seed/<table>.jsonl`, and it lands exactly once, on a FRESH `cell.db`.
//!
//! So the example checks the broker's `store` in with its seed and lets the
//! instantiation MERGE around it: `classify_subtree_nodes` partitions a subtree
//! template into cells that are already on disk (left untouched) and cells that
//! are missing (staged from the library), which is what makes one `add_nodes`
//! grow five cells around a sixth it did not write. That merge is the mechanism
//! this file measures first, because everything below it depends on the grant row
//! being real.
//!
//! Guarded like every other template-reading test (GH #49): the public export
//! ships a subset of the library, and material that did not travel is skipped
//! rather than judged.

use meclaw_cells::{
    LlmCellFactory, TimerCellFactory, code::CodeCellFactory, store::StoreCellFactory,
};
use meclaw_colony::api_dto::{MessageLogDto, MessageLogFilter};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, from_str, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use meclaw_testing::{ColonyHandle, LogMark};
use std::sync::Arc;
use tokio::sync::oneshot;

/// The credential value the vault holds. It is a fixture, not a key — the shape
/// is deliberately obvious so a hit in the log would be unmistakable.
const SECRET: &str = "sk-test-not-a-key-vault-pilot";
/// The vault passphrase. Named by `params.unlock_env` in the example's manifest.
const PASSPHRASE: &str = "a passphrase nobody guesses";
/// The environment variable the example's `grow.json` names in `unlock_env`.
const UNLOCK_ENV: &str = "VAULT_PILOT_PASSPHRASE";
/// The credential's catalogue name, as `seed/cred_refs.jsonl` and the grant say.
const CRED_REF: &str = "cred:example-provider:primary";
/// The grant handle, derived by the rule the example's README states:
/// `grant:` + the cred_ref's tail + `@` + the subject, each with `:` → `-`.
const GRANT_ID: &str = "grant:example-provider-primary@member-example";

// ──────────────────────────────────────────────────────────── the shipped material

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The example plus the two templates it names, or `None` where any of it did
/// not travel with the export.
fn shipped() -> Option<()> {
    for rel in [
        "examples/vault-pilot/grow.json",
        "examples/vault-pilot/seed/main/access/store/seed/grants.jsonl",
        "templates/access/template.json",
        "templates/terminal/template.json",
    ] {
        if !repo(rel).exists() {
            return None;
        }
    }
    Some(())
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// The colony root: the example's own seed, the two templates the example
/// names (`access` and `terminal` — both shipped, both behind the presence
/// guard above, so a partial tree skips instead of failing on a missing
/// directory), and an `.env` that carries a base URL and nothing else.
/// No provider key anywhere: the brain's `api_key` is the empty literal (GH
/// #1061) — that is what makes the sealed lane the only way this model can
/// authenticate, and therefore what makes the assertion below a proof.
fn build_root(root: &std::path::Path, base_url: &str) {
    copy_tree(&repo("examples/vault-pilot/seed"), root);
    for name in ["access", "terminal"] {
        copy_tree(
            &repo("templates").join(name),
            &root.join("templates").join(name),
        );
    }
    std::fs::write(
        root.join(".env"),
        format!("EXAMPLE_PROVIDER_BASE_URL={base_url}\n"),
    )
    .unwrap();
}

/// The brain's credential round outlasts every hang mark of this file.
///
/// GH #1216 (tv-br M10k): the example ships the default `credential_wait_ms`
/// (10 s), a product deadline on the wall clock. Under a CPU quota the sealed
/// round took 21-22 s (build04, 80 %, 44 tests beside it), the round expired,
/// the parked turn got its `credential_pending` receipt, and the late box
/// opened with no turn left to answer -- the provider call the test awaits
/// never came, and the runner killed it at 240 s. A test that waits for the
/// round's success sets the round's deadline above its own hang mark (TV-Z9:
/// SUT time by configuration); test `c` keeps the shipped deadline, whose
/// expiry is its receipt. The deadline is `deadline::product_wait_ms` (OR-T12-13,
/// tv-br M10m: from the one time factor, not a literal), so the test's own
/// mark always fires first and names the wait that hung.
fn the_round_outwaits_the_hang_mark(root: &std::path::Path) {
    let path = root.join("main/brain/config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["params"]["credential_wait_ms"] = json!(meclaw_testing::deadline::product_wait_ms());
    std::fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
}

/// The example's manifest, as the ordered list of mutation bodies it is.
///
/// `--apply` hands the whole `{"manifest": [ … ]}` file to the mutation door;
/// the door's own message form is ONE body, so a test that speaks to it directly
/// unwraps the list and applies the entries in order — which is the same
/// roll-forward semantics, entry by entry.
fn manifest() -> Vec<Value> {
    let file: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("examples/vault-pilot/grow.json")).unwrap(),
    )
    .unwrap();
    file["manifest"]
        .as_array()
        .expect("grow.json carries a `manifest` list")
        .clone()
}

// ─────────────────────────────────────────────────────────────────── the colony

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
        (
            "vault".to_string(),
            Arc::new(meclaw_cells::vault::VaultCellFactory),
        ),
    ]
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let fs = factories();
    let h = ColonyHandle::new_with_factories_at(td, fs.clone());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the pilot's seed must boot");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    // Re-check R K9: an ack that never comes is a named `HANG:` marker, not
    // a runner kill.
    meclaw_testing::deadline::hang("the colony acks the template rescan", ack_rx)
        .await
        .expect("rescan ack")
        .expect("rescan aborted");
    h
}

/// Apply every entry of the example's manifest, in order, and assert each one
/// committed. A refusal here is the example being wrong, not the test.
async fn apply_manifest(h: &ColonyHandle) {
    for (i, entry) in manifest().into_iter().enumerate() {
        let outcome = apply(h, entry).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "grow.json entry {i} was refused: {outcome:?}"
        );
    }
}

async fn apply(h: &ColonyHandle, payload: Value) -> MutationOutcome {
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
    meclaw_testing::deadline::hang("the colony acks the mutation", ack_rx)
        .await
        .expect("mutation ack")
}

/// Fill the vault the way `meclaw --vault-add` does: straight into its own
/// `cell.db`, with no colony running. A message would put the value into the very
/// log this example exists to keep it out of.
/// Re-check M10c M1 / OR-T10-27: the product function behind the flag
/// (`user_channel::add`, off the worker like the CLI's `run_off_worker`), not a
/// copy of its steps.
///
/// Re-check m10f H1: the helper takes the running colony BY VALUE and stops it
/// first (`ColonyHandle::shutdown(self)`), so a seed beside a running colony
/// -- what `--vault-add` refuses while a daemon holds the root -- cannot be
/// written with it.
async fn seed_vault_secret(h: ColonyHandle, root: &std::path::Path) {
    h.shutdown().await;
    let root = root.to_path_buf();
    let (name, value, passphrase) = (
        CRED_REF.to_string(),
        SECRET.to_string(),
        PASSPHRASE.to_string(),
    );
    let stored = tokio::task::spawn_blocking(move || {
        meclaw_cells::vault::user_channel::add(
            &root,
            "/main/access/vault",
            &name,
            value.as_bytes(),
            passphrase.as_bytes(),
        )
    })
    .await
    .expect("the vault-add task");
    assert_eq!(
        stored,
        Ok(1),
        "`--vault-add` did not store the credential as its first version"
    );
}

// ───────────────────────────────────────────────────────────────── reading back

/// The broker's two cells as the message log names them: the root directory
/// `main` is stripped from logical paths (`path_truth.rs`).
const INVOKE: &str = "/access/invoke";
const STORE: &str = "/access/store";

/// One message-log row as one string, header and body.
fn row_text(r: &MessageLogDto) -> String {
    format!(
        "{} {}",
        r.headers_json,
        r.body_payload.as_deref().unwrap_or("")
    )
}

/// The turns of a row's inline body (`messages[]`), empty for anything else.
fn turns_of(r: &MessageLogDto) -> Vec<Value> {
    r.body_payload
        .as_deref()
        .and_then(|b| from_str::<Value>(b).ok())
        .and_then(|v| v.get("messages").and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

/// The first value under `key` anywhere in `v`.
fn find_key<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(m) => m
            .get(key)
            .or_else(|| m.values().find_map(|x| find_key(x, key))),
        Value::Array(a) => a.iter().find_map(|x| find_key(x, key)),
        _ => None,
    }
}

/// One call `./access/invoke` made on the broker's store and the store's
/// answer to that very call id, as the message log shows them.
///
/// M10 (OR-T10-20, GH #160): a test is not a cell, and `./access/store` is an
/// INTERIOR cell of a sealed hive. Its rows are read off the traffic the
/// colony journalled, through the colony — never off its `cell.db`.
#[derive(Debug)]
struct StoreCall {
    args: Value,
    rows_affected: Option<i64>,
    error_code: Option<String>,
    result: String,
}

/// Every answered call on `table`.
fn store_calls(log: &[MessageLogDto], table: &str) -> Vec<StoreCall> {
    let asked: Vec<(String, Value)> = log
        .iter()
        .filter(|r| r.from_path == INVOKE && r.to_path == STORE)
        .flat_map(turns_of)
        .filter_map(|t| {
            if t.get("type")?.as_str()? != "tool_call" {
                return None;
            }
            let args: Value = from_str(t.get("text")?.as_str()?).ok()?;
            let id = t.get("id")?.as_str()?.to_string();
            (args["table"] == table).then_some((id, args))
        })
        .collect();
    asked
        .into_iter()
        .filter_map(|(id, args)| {
            log.iter()
                .filter(|r| r.from_path == STORE && r.to_path == INVOKE)
                .find_map(|r| {
                    let answer = turns_of(r)
                        .into_iter()
                        .find(|t| t["type"] == "tool_result" && t["id"] == id.as_str())?;
                    let header: Value = from_str(&r.headers_json).ok()?;
                    Some(StoreCall {
                        args: args.clone(),
                        rows_affected: find_key(&header, "rows_affected").and_then(Value::as_i64),
                        error_code: find_key(&header, "error_code")
                            .and_then(Value::as_str)
                            .filter(|c| !c.is_empty())
                            .map(str::to_string),
                        result: answer["text"].as_str().unwrap_or_default().to_string(),
                    })
                })
        })
        .collect()
}

/// `SELECT outcome FROM usage WHERE operation = 'vault.deliver'`, off the log:
/// the outcome of every `vault.deliver` usage row the store wrote — its answer
/// to that very insert says one row affected and carries no error code.
fn deliveries(log: &[MessageLogDto]) -> Vec<String> {
    store_calls(log, "usage")
        .into_iter()
        .filter(|c| {
            c.args["operation"] == "insert"
                && c.args["row"]["operation"] == "vault.deliver"
                && c.rows_affected == Some(1)
                && c.error_code.is_none()
        })
        .map(|c| {
            c.args["row"]["outcome"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `grants` table as the broker saw it: per answered `grants` lookup, the
/// `grant_id`s the store returned.
fn grant_lookups(log: &[MessageLogDto]) -> Vec<Vec<String>> {
    store_calls(log, "grants")
        .into_iter()
        .filter(|c| c.args["operation"] == "select" && c.error_code.is_none())
        .map(|c| {
            from_str::<Value>(&c.result)
                .ok()
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|r| r["grant_id"].as_str().map(str::to_string))
                .collect()
        })
        .collect()
}

/// `true` once the model's answer to a turn is on record: the mock's `pong`
/// in a row `/brain` sent. Returned with the row count and the newest rows, so
/// a hang mark carries what the log held.
fn answered(log: &[MessageLogDto]) -> (bool, usize, Vec<(String, String)>) {
    let done = log.iter().any(|r| {
        r.from_path == "/brain"
            && r.body_payload
                .as_deref()
                .is_some_and(|b| b.contains("pong"))
    });
    let newest = log
        .iter()
        .take(5)
        .map(|r| (r.from_path.clone(), r.to_path.clone()))
        .collect();
    (done, log.len(), newest)
}

/// The message log, read through the colony (`ReadMessages`): all of it, or
/// only what was committed after `since`.
async fn read(h: &ColonyHandle, since: Option<&LogMark>) -> Vec<MessageLogDto> {
    match since {
        Some(mark) => h.log_since(mark, MessageLogFilter::default()).await,
        None => h.read_log(MessageLogFilter::default()).await,
    }
}

/// Wait until `done` holds for `view` of the log. On a hang the mark carries
/// the view of the last read, a state the log really was in.
async fn until_log<T: std::fmt::Debug + Default>(
    h: &ColonyHandle,
    what: &str,
    since: Option<&LogMark>,
    view: impl Fn(&[MessageLogDto]) -> T,
    done: impl Fn(&T) -> bool,
) {
    let last = std::sync::Mutex::new(T::default());
    meclaw_testing::wait::until_with(
        what,
        || async {
            let seen = view(&read(h, since).await);
            let ok = done(&seen);
            *last.lock().expect("log state") = seen;
            ok
        },
        || std::mem::take(&mut *last.lock().expect("log state")),
    )
    .await;
}

/// A user turn for the pilot model.
///
/// The address is `/brain`, not `/main/brain`: the root cell directory is
/// STRIPPED from logical paths (`crates/meclaw-colony/src/path_truth.rs`), which
/// is also why the example's manifest says `scope: "/"` and `./brain` while the
/// same cell lies at `main/brain/` on disk.
fn turn(text: &str) -> Message {
    MessageBuilder::new(Path::new("/brain"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

fn chat_answer() -> MockResponse {
    MockResponse::ok_json(
        json!({
            "id": "chatcmpl-1", "object": "chat.completion", "created": 1,
            "model": "gpt-4o-mini",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": "pong"}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string()
        .as_bytes(),
    )
}

/// Wait under the kit's hang mark until the mock provider has seen more than
/// `n` requests. M10d (TV-Z3): a hang reports the request count it saw last,
/// not a hand-rolled 15-s give-up.
async fn until_calls(
    what: &str,
    captured: &Arc<tokio::sync::Mutex<Vec<meclaw_testing::mock_http::CapturedRequest>>>,
    n: usize,
) {
    meclaw_testing::wait::until_with(
        what,
        || std::future::ready(provider_calls_exceed(captured, n)),
        || captured.try_lock().map(|c| c.len()),
    )
    .await;
}

/// The environment the example's `unlock_env` names. `set_var` is unsafe in
/// edition 2024 (a concurrent `getenv` would be a data race); sound here because
/// every test in this file writes the same value and the cell that reads it does
/// not exist yet.
fn arm_passphrase() {
    unsafe { std::env::set_var(UNLOCK_ENV, PASSPHRASE) };
}

// ═════════════════════════════════════════════════════════════════════════ pins

/// The merge, and the row that could not have got there any other way.
///
/// One manifest grows five broker cells and a drain around a `store` the example
/// checked in, and when it is done the grant the `llm` cell names is on disk —
/// before a single cell of this colony has answered a message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_one_manifest_grows_the_broker_and_the_grant_is_on_disk_before_the_boot() {
    let Some(()) = shipped() else {
        return;
    };
    arm_passphrase();
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path(), "http://127.0.0.1:1/v1");
    let h = boot(&td).await;

    apply_manifest(&h).await;

    // Five cells the manifest grew, one it left alone, and the consumer.
    for rel in [
        "main/access/policy/config.json",
        "main/access/invoke/config.json",
        "main/access/sweep/config.json",
        "main/access/clock/config.json",
        "main/access/vault/config.json",
        "main/access/store/config.json",
        "main/brain/config.json",
    ] {
        assert!(
            td.path().join(rel).exists(),
            "{rel} is missing after the manifest"
        );
    }

    // The merge left the checked-in store byte-identical: its seed is still the
    // example's, which is the whole reason it is checked in.
    assert_eq!(
        std::fs::read(td.path().join("main/access/store/config.json")).unwrap(),
        std::fs::read(repo("templates/access/store/config.json")).unwrap(),
        "the pilot's store config has drifted from the shipped one"
    );

    // `unlock_env` reached the vault through `override_params` — the one setting
    // every deployment has to make, made by the manifest and not by hand.
    let vault: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(td.path().join("main/access/vault/config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        vault["params"]["unlock_env"], UNLOCK_ENV,
        "the manifest could not set unlock_env: {vault}"
    );

    // And the grant. Before anything runs a grant can only take one form, the
    // SEED FILE: the row's pre-boot existence is a claim about that file. The
    // merge left it where the example put it, and the handle in it is the handle
    // the model's config names — two files agreeing on one literal is what closes
    // the chicken-and-egg, and the rows themselves are measured in the tests
    // below (`deliveries`, `grant_lookups`). M10e (OR-T10-27): no stat on the
    // store's `cell.db` any more — that is a foreign cell's database, and the
    // graph cannot say "not woken" (`GraphNodeDto` carries no wake state).
    let seed = |t: &str| {
        std::fs::read_to_string(td.path().join(format!("main/access/store/seed/{t}.jsonl")))
            .unwrap_or_else(|e| panic!("{t}.jsonl: {e}"))
    };
    assert!(seed("grants").contains(GRANT_ID), "{}", seed("grants"));
    assert!(
        seed("grant_events").contains(GRANT_ID),
        "a grant with no event has no effective state: {}",
        seed("grant_events")
    );
    assert!(
        seed("cred_refs").contains(CRED_REF),
        "{}",
        seed("cred_refs")
    );
    assert!(
        seed("policy").contains("\"enabled\": 1"),
        "the pilot's rule ships switched ON — a template's does not: {}",
        seed("policy")
    );
    let brain: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(td.path().join("main/brain/config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        brain["params"]["credential_grant_id"], GRANT_ID,
        "the model names a grant the seed does not carry: {brain}"
    );
    // Since GH #1061 (#801) the param is the empty literal: no `${…}` token of
    // the secret class stands in a shipped example. The empty string is not a
    // bearer (GH #271) — and a cell with no bearer is a cell that has to ask. A
    // literal key here would make the sealed lane unreachable.
    assert_eq!(
        brain["params"]["api_key"], "",
        "the pilot model must hold no bearer of its own: {brain}"
    );

    h.shutdown().await;
}

/// The whole point: a model with no key in its config answers, and the bearer
/// the provider sees is the value that was sealed in the vault.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b_the_pilot_model_answers_with_a_credential_it_never_had_in_its_config() {
    let Some(()) = shipped() else {
        return;
    };
    arm_passphrase();
    let (addr, _server, captured) =
        start_mock_server_capturing(vec![chat_answer(), chat_answer(), chat_answer()]).await;

    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path(), &format!("http://{addr}/v1"));
    the_round_outwaits_the_hang_mark(td.path());
    let h = boot(&td).await;
    apply_manifest(&h).await;

    // The vault dir exists only after the manifest ran, so the credential goes in
    // here — the same write `meclaw --vault-add` performs, and the same one the
    // README tells an operator to perform before the first start. M10e: after
    // the colony is down, as the flag demands (it refuses a running root).
    seed_vault_secret(h, td.path()).await;

    // Second boot: the vault has a secret, the store has the grant, and nothing
    // else has changed. This is the state an operator's first `meclaw` start is
    // in, and everything below happens without another gesture.
    let h = boot(&td).await;

    // GH #457 changed what the FIRST turn costs. The model holds nothing, so it
    // parks this turn, asks the vault once, and answers the very same turn when
    // the box comes back — one message in, one answer out, no second gesture.
    h.send(turn("ping")).await;

    // The sealed round runs beside us; its receipt is the booked spend, which
    // `./invoke` writes only after the box came back. M10 (OR-T10-20): read off
    // the message log through the colony, not off the store's `cell.db`.
    until_log(&h, "the vault.deliver spend", None, deliveries, |seen| {
        seen.iter().any(|o| o == "ok")
    })
    .await;

    until_calls("the provider call for the very first turn", &captured, 0).await;
    // The log is read while the colony runs, so the turn has to be over first:
    // its answer is on record (positive), and the colony is quiet behind it
    // (`Settle`: the writer committed everything that followed).
    until_log(&h, "the turn's answer is on record", None, answered, |a| {
        a.0
    })
    .await;
    h.quiesce().await;
    let rows = read(&h, None).await;
    // M10e: the absences below read THIS state of the log, so it has to hold
    // the turn's answer itself -- a read that came back empty would otherwise
    // pass every one of them.
    assert!(
        answered(&rows).0,
        "the log read behind the barrier does not hold the turn's answer: {:?}",
        answered(&rows)
    );
    let log: Vec<String> = rows.iter().map(row_text).collect();
    // And it cost no refusal on the way. Under GH #421 this turn was spent on a
    // `credential_pending` nobody redelivered.
    assert!(
        !log.iter().any(|r| r.contains("credential_pending")),
        "the parked turn was refused instead of answered"
    );

    let seen = captured.lock().await.clone();
    let last = seen.last().expect("the provider was called");
    assert_eq!(
        last.headers.get("authorization").map(String::as_str),
        Some(format!("Bearer {SECRET}").as_str()),
        "the bearer on the wire is not the vault's value: {:?}",
        last.headers
    );

    h.shutdown().await;

    // And nothing of it is on record. The example ships no `api_key`, so the only
    // copy of this value that ever existed outside the vault lived in one task's
    // RAM.
    let hits: Vec<String> = log
        .into_iter()
        .filter(|r| r.contains(SECRET) || r.contains(PASSPHRASE))
        .collect();
    assert!(hits.is_empty(), "the credential is on record: {hits:?}");
}

/// The counter-proof. Take the grant row out of the seed and change nothing else:
/// the same tree, the same vault, the same enabled rule — and the model never
/// gets past its own refusal, because `./invoke` reads `grants` and a grant that
/// was never minted cannot be spent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c_without_the_grant_row_the_model_stays_pending_and_never_calls_out() {
    let Some(()) = shipped() else {
        return;
    };
    arm_passphrase();
    let (addr, _server, captured) = start_mock_server_capturing(vec![chat_answer()]).await;

    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path(), &format!("http://{addr}/v1"));
    // The one difference: the header line stays (the table is still declared),
    // the row goes.
    let seed = td.path().join("main/access/store/seed/grants.jsonl");
    let head = std::fs::read_to_string(&seed).unwrap();
    std::fs::write(&seed, head.lines().next().unwrap().to_string() + "\n").unwrap();

    let h = boot(&td).await;
    apply_manifest(&h).await;
    seed_vault_secret(h, td.path()).await;

    let h = boot(&td).await;
    h.send(turn("ping")).await;
    until_log(
        &h,
        "the credential_pending refusal",
        None,
        |log| {
            log.iter()
                .map(row_text)
                .filter(|r| r.contains("credential_pending"))
                .count()
        },
        |pending| *pending > 0,
    )
    .await;

    // Then measure what the broker did NOT do. M10d (TV-A1): no wall-clock
    // budget -- the absence is read behind the quiescence barrier, after a
    // second turn, once the colony has settled everything both turns caused.
    // M10 (OR-T10-20): the store's rows are read off the log through the
    // colony.
    h.send(turn("ping again")).await;
    until_log(
        &h,
        "the second turn's credential_pending refusal",
        None,
        |log| {
            log.iter()
                .map(row_text)
                .filter(|r| r.contains("credential_pending"))
                .count()
        },
        |pending| *pending > 1,
    )
    .await;
    h.quiesce().await;
    let log = read(&h, None).await;

    // The broker DID look the grant up (non-vacuity), and the store answered
    // every lookup with no row.
    let lookups = grant_lookups(&log);
    assert!(
        !lookups.is_empty() && lookups.iter().all(Vec::is_empty),
        "the grant row came from somewhere — this counter-proof measures nothing: {lookups:?}"
    );
    let booked = deliveries(&log);
    assert!(
        booked.iter().all(|o| o != "ok"),
        "a delivery was booked for a grant that does not exist: {booked:?}"
    );
    assert!(
        captured.lock().await.is_empty(),
        "the model called the provider with no credential at all"
    );

    h.shutdown().await;
}

/// The grant is durable and the credential is not — and the turn survives both.
///
/// A `grants` row is a table row and outlives everything; the opened credential
/// lives in the `llm` cell's task and dies with it
/// (`crates/meclaw-cells/src/llm/cell.rs`, the `credential` field). So a restart
/// finds the grant intact and the pocket empty, and asks again.
///
/// GH #457 CHANGED WHAT THAT COSTS. This test used to pin the gap: the first turn
/// after every wake was spent on a `credential_pending` nobody redelivered, and
/// the assertion was that the provider had NOT been called. That is now the
/// opposite claim — the turn is parked for the length of the round and answered
/// when the box lands — so the two halves are measured separately: a SECOND
/// delivery proves the credential did not survive the sleep, and a provider call
/// that came after it proves the turn did.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d_the_grant_outlives_the_colony_and_the_credential_does_not() {
    let Some(()) = shipped() else {
        return;
    };
    arm_passphrase();
    let (addr, _server, captured) = start_mock_server_capturing(vec![
        chat_answer(),
        chat_answer(),
        chat_answer(),
        chat_answer(),
    ])
    .await;

    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path(), &format!("http://{addr}/v1"));
    the_round_outwaits_the_hang_mark(td.path());
    let h = boot(&td).await;
    apply_manifest(&h).await;
    seed_vault_secret(h, td.path()).await;

    // Round one, on a fresh colony. One turn, one round, one answer.
    let h = boot(&td).await;
    h.send(turn("ping")).await;
    until_log(&h, "the first delivery", None, deliveries, |seen| {
        seen.iter().any(|o| o == "ok")
    })
    .await;
    until_calls("the first provider call", &captured, 0).await;
    let before = captured.lock().await.len();
    // M10 (OR-T10-20): the delivery count is read off the log through the
    // colony before it goes down, once it is quiet behind the round.
    h.quiesce().await;
    let deliveries_before = deliveries(&read(&h, None).await).len();
    h.shutdown().await;

    // Round two, same tree, second colony. ONE turn, and it has to carry both
    // claims by itself — that is the whole difference GH #457 made.
    let h = boot(&td).await;
    let restarted = h.log_mark().await;
    h.send(turn("ping after the restart")).await;
    until_log(&h, "the second delivery", None, deliveries, |seen| {
        seen.len() > deliveries_before
    })
    .await;
    // The grant is a row, so the second colony's store still answers with it —
    // nothing re-seeded it, because a store seeds only a FRESH `cell.db`.
    let lookups = grant_lookups(&read(&h, Some(&restarted)).await);
    assert!(
        lookups.iter().flatten().any(|g| g == GRANT_ID),
        "the grant did not survive the restart: {lookups:?}"
    );
    // Half one: a second delivery had to be asked for at all. A cell that still
    // held the credential would have had nothing to ask the vault about.
    //
    // Half two: and that same turn still reached the provider. No second gesture
    // from the user, no `credential_pending` in the log — the turn was parked for
    // the length of the round, not thrown away.
    until_calls(
        "the provider call for the first turn after the wake",
        &captured,
        before,
    )
    .await;
    // Read while the colony runs: the turn's answer on record first, then quiet.
    until_log(
        &h,
        "the answer after the wake is on record",
        Some(&restarted),
        answered,
        |a| a.0,
    )
    .await;
    h.quiesce().await;
    // M10e: one read carries the positive mark and the absence -- both rounds'
    // answers must be in it, or an empty read would pass the absence.
    let rows = read(&h, None).await;
    let answers = rows
        .iter()
        .filter(|r| {
            r.from_path == "/brain"
                && r.body_payload
                    .as_deref()
                    .is_some_and(|b| b.contains("pong"))
        })
        .count();
    assert!(
        answers >= 2,
        "the log read behind the barrier does not hold both rounds' answers \
         ({answers}): {:?}",
        answered(&rows)
    );
    assert!(
        !rows
            .iter()
            .map(row_text)
            .any(|r| r.contains("credential_pending")),
        "the first turn after the wake was refused instead of parked"
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────── polling

/// `true` once the mock server has seen more than `n` requests.
fn provider_calls_exceed(
    captured: &Arc<tokio::sync::Mutex<Vec<meclaw_testing::mock_http::CapturedRequest>>>,
    n: usize,
) -> bool {
    captured.try_lock().map(|c| c.len() > n).unwrap_or(false)
}
