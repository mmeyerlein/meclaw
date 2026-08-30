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
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use std::sync::Arc;
use std::time::Duration;
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
/// `EXAMPLE_PROVIDER_KEY` stays UNSET on purpose — that is what makes the
/// sealed lane the only way this model can authenticate, and therefore what
/// makes the assertion below a proof.
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
    ack_rx.await.expect("rescan ack").expect("rescan aborted");
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
    ack_rx.await.expect("mutation ack")
}

/// Fill the vault the way `meclaw --vault-add` does: straight into its own
/// `cell.db`, with no colony running. A message would put the value into the very
/// log this example exists to keep it out of.
fn seed_vault_secret(root: &std::path::Path) {
    use meclaw_cells::vault::crypto::MasterKey;
    use meclaw_cells::vault::store as vs;
    let dir = root.join("main/access/vault");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&dir.join("cell.db")).unwrap();
    vs::apply_ddl(&conn).unwrap();
    let salt = vs::salt_or_create(&conn).unwrap();
    let key = MasterKey::derive(PASSPHRASE.as_bytes(), &salt).unwrap();
    let (nonce, ct) = key.seal(SECRET.as_bytes()).unwrap();
    vs::put(&conn, CRED_REF, &nonce, &ct, &vs::now_iso()).unwrap();
}

// ───────────────────────────────────────────────────────────────── reading back

/// One column of one table of the broker's store, read straight off disk.
///
/// A test is not a cell, and this is the only way in: `./access/store` is an
/// INTERIOR cell of a sealed hive, so no probe edge from outside can reach it —
/// which is exactly the property the template exists to have.
fn store_rows(root: &std::path::Path, sql: &str) -> Vec<String> {
    let db = root.join("main/access/store/cell.db");
    if !db.exists() {
        return Vec::new();
    }
    let Ok(conn) = rusqlite::Connection::open(&db) else {
        return Vec::new();
    };
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let Ok(rows) = st.query_map([], |r| r.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

/// Every `message_log` row of the colony, headers and body as one string each.
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

/// Poll until `f` answers `true`, or give up after ~15 s. Failure markers are
/// generous on purpose (CLAUDE.md § Coding-Standards): this waits on a cell
/// round trip under whatever load the rest of the suite is putting on the host.
async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..60 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("{what} never happened");
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

    // And the grant. A `store` spawns lazily, so its `cell.db` does not exist yet
    // and cannot: the row's pre-boot existence is a claim about the SEED FILE,
    // which is the only form a grant can take before anything runs. The merge left
    // it where the example put it, and the handle in it is the handle the model's
    // config names — two files agreeing on one literal is what closes the
    // chicken-and-egg, and the rows themselves are measured in the tests below.
    assert!(
        !td.path().join("main/access/store/cell.db").exists(),
        "the store woke without being addressed — this assertion measures the seed"
    );
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
    // An environment token survives in the file LITERALLY and is resolved on
    // every read, so what is pinned here is the FORM: a `${VAR:-}` whose default
    // is empty. With the variable unset that resolves to the empty string, which
    // is not a bearer (GH #271) — and a cell with no bearer is a cell that has to
    // ask. A literal key here would make the sealed lane unreachable.
    assert_eq!(
        brain["params"]["api_key"], "${EXAMPLE_PROVIDER_KEY:-}",
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
    let h = boot(&td).await;
    apply_manifest(&h).await;

    // The vault dir exists only after the manifest ran, so the credential goes in
    // here — the same write `meclaw --vault-add` performs, and the same one the
    // README tells an operator to perform before the first start.
    seed_vault_secret(td.path());
    h.shutdown().await;

    // Second boot: the vault has a secret, the store has the grant, and nothing
    // else has changed. This is the state an operator's first `meclaw` start is
    // in, and everything below happens without another gesture.
    let h = boot(&td).await;

    // GH #457 changed what the FIRST turn costs. The model holds nothing, so it
    // parks this turn, asks the vault once, and answers the very same turn when
    // the box comes back — one message in, one answer out, no second gesture.
    h.send(turn("ping")).await;

    // The sealed round runs beside us; its receipt is the booked spend, which
    // `./invoke` writes only after the box came back.
    until("the vault.deliver spend", || {
        store_rows(
            td.path(),
            "SELECT outcome FROM usage WHERE operation = 'vault.deliver'",
        )
        .contains(&"ok".to_string())
    })
    .await;

    until("the provider call for the very first turn", || {
        // `captured` is behind an async mutex; a blocking read here would be a
        // lock in a test loop, so the poll body only checks what it can cheaply.
        provider_was_called(&captured)
    })
    .await;
    // And it cost no refusal on the way. Under GH #421 this turn was spent on a
    // `credential_pending` nobody redelivered.
    assert!(
        !log_rows(td.path())
            .iter()
            .any(|r| r.contains("credential_pending")),
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
    let hits: Vec<String> = log_rows(td.path())
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
    seed_vault_secret(td.path());
    h.shutdown().await;

    let h = boot(&td).await;
    h.send(turn("ping")).await;
    until("the credential_pending refusal", || {
        log_rows(td.path())
            .iter()
            .any(|r| r.contains("credential_pending"))
    })
    .await;

    // Give the broker the same wall-clock budget the positive case needed, and
    // then measure what it did NOT do.
    tokio::time::sleep(Duration::from_secs(3)).await;
    h.send(turn("ping again")).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    assert!(
        store_rows(td.path(), "SELECT grant_id FROM grants").is_empty(),
        "the grant row came from somewhere — this counter-proof measures nothing"
    );
    assert!(
        store_rows(
            td.path(),
            "SELECT outcome FROM usage WHERE operation = 'vault.deliver'"
        )
        .iter()
        .all(|o| o != "ok"),
        "a delivery was booked for a grant that does not exist"
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
    let h = boot(&td).await;
    apply_manifest(&h).await;
    seed_vault_secret(td.path());
    h.shutdown().await;

    // Round one, on a fresh colony. One turn, one round, one answer.
    let h = boot(&td).await;
    h.send(turn("ping")).await;
    until("the first delivery", || {
        store_rows(
            td.path(),
            "SELECT outcome FROM usage WHERE operation = 'vault.deliver'",
        )
        .contains(&"ok".to_string())
    })
    .await;
    until("the first provider call", || provider_was_called(&captured)).await;
    let before = captured.lock().await.len();
    h.shutdown().await;

    // The colony is down. The grant is a row, so it is still there — nothing
    // re-seeded it, because a store seeds only a FRESH `cell.db`.
    assert!(
        store_rows(td.path(), "SELECT grant_id FROM grants").contains(&GRANT_ID.to_string()),
        "the grant did not survive the restart"
    );
    let deliveries_before = store_rows(
        td.path(),
        "SELECT outcome FROM usage WHERE operation = 'vault.deliver'",
    )
    .len();

    // Round two, same tree, second colony. ONE turn, and it has to carry both
    // claims by itself — that is the whole difference GH #457 made.
    let h = boot(&td).await;
    h.send(turn("ping after the restart")).await;
    until("the second delivery", || {
        store_rows(
            td.path(),
            "SELECT outcome FROM usage WHERE operation = 'vault.deliver'",
        )
        .len()
            > deliveries_before
    })
    .await;
    // Half one: a second delivery had to be asked for at all. A cell that still
    // held the credential would have had nothing to ask the vault about.
    //
    // Half two: and that same turn still reached the provider. No second gesture
    // from the user, no `credential_pending` in the log — the turn was parked for
    // the length of the round, not thrown away.
    until(
        "the provider call for the first turn after the wake",
        || provider_calls_exceed(&captured, before),
    )
    .await;
    assert!(
        !log_rows(td.path())
            .iter()
            .any(|r| r.contains("credential_pending")),
        "the first turn after the wake was refused instead of parked"
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────── polling

/// `true` once the mock server has seen at least one request.
///
/// The captured list is behind an async mutex and [`until`] takes a synchronous
/// predicate, so the read is a `try_lock`: a miss is indistinguishable from "not
/// yet" for this purpose, and the loop comes back in 250 ms either way.
fn provider_was_called(
    captured: &Arc<tokio::sync::Mutex<Vec<meclaw_testing::mock_http::CapturedRequest>>>,
) -> bool {
    captured.try_lock().map(|c| !c.is_empty()).unwrap_or(false)
}

/// `true` once the mock server has seen more than `n` requests.
fn provider_calls_exceed(
    captured: &Arc<tokio::sync::Mutex<Vec<meclaw_testing::mock_http::CapturedRequest>>>,
    n: usize,
) -> bool {
    captured.try_lock().map(|c| c.len() > n).unwrap_or(false)
}
