//! GH #1292 (OR-SA-35) -- an episode written while the embedder's credential
//! is still on its way gets its vector once the credential is there.
//!
//! `./embed` spends a credential grant. At a boot the first episodes arrive
//! while the vault is still waking: the embedder's round runs past
//! `credential_wait_ms` and the call comes back as `credential_pending`. Before
//! this lock nothing in the hive took that receipt -- it died as `no_route`
//! (`./embed -> ./writer`), the episode kept a `queued` row and stayed out of
//! the semantic leg until a night's backfill. Now the receipt goes to the
//! writer, which sends the item ONCE more; that call opens a new round and runs
//! as soon as the box arrives. A second receipt is not answered (no loop,
//! R-AG-1) -- the row then waits for the backfill as before.
//!
//! Two locks: the booted hive (shipped files, a real store, a real `code`
//! embedder on a grant, a loopback provider, the vault played by the test) and
//! the writer's one-retry rule run bare on stdin.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{self, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use support::*;

const AUDIENCE: &str = r#"["agent:a","member:e"]"#;
const EMBED: &str = "/memory-hive/embed";

fn embedding_reply() -> MockResponse {
    let v: Vec<f64> = (0..1024)
        .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
        .collect();
    MockResponse::ok_json(
        json!({"data": [{"index": 0, "embedding": v}], "usage": {"prompt_tokens": 1}})
            .to_string()
            .as_bytes(),
    )
}

fn episode(text: &str) -> Message {
    let ctx = json!({"audience_set": AUDIENCE, "channel": "direct:e",
                     "session_id": "s-1", "turn_id": "s-1#1", "agent_id": "agent:a",
                     "happened_at": "2026-10-21T20:00:00Z"});
    MessageBuilder::new(Path::new(HIVE))
        .hop(as_map(&json!({"route": "in_episode"})))
        .context(as_map(&ctx))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": text}]})))
        .build()
}

/// The box the vault delivers to the round `request` opened, on the hive's
/// `in_sealed` lane (the one address a sealed hive opens at `./embed`).
fn sealed_box(request: &Message, secret: &str) -> Message {
    let text = body_of(request)["messages"][0]["text"]
        .as_str()
        .expect("the request carries its args as text");
    let args: Value = serde_json::from_str(text).expect("args are JSON");
    let recipient = args["payload"]["recipient_key"]
        .as_str()
        .expect("payload.recipient_key");
    let sealed = meclaw_cells::sealed::seal_to(recipient, secret.as_bytes()).expect("seal the box");
    MessageBuilder::new(Path::new(EMBED))
        // the lane the hive opens at `./embed` for a box (`accepts` `in_sealed`)
        .hop(as_map(&json!({"route": "in_sealed"})))
        .body(Body::Inline(json!({"sealed": sealed.to_json()})))
        .build()
}

/// The shipped hive with its embedder on a grant: the credential round leaves
/// on `/out` (the vault is the test), and the provider is a loopback stub.
fn on_a_grant(td: &tempfile::TempDir, endpoint: &str, wait_ms: u64) {
    build(td, &[]);
    let main = td.path().join("main");
    let parent = main.join("config.json");
    let mut cfg = read_json(&parent);
    cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("edges")
        .push(json!({"from": "./memory-hive/embed", "to": "/out",
                     "condition": "has(hop.route) && hop.route == 'credential_request'"}));
    std::fs::write(&parent, serde_json::to_string_pretty(&cfg).expect("json")).expect("write");
    let embed = main.join("memory-hive/embed/config.json");
    let mut cfg = read_json(&embed);
    cfg["params"]["credential_grant_id"] = json!("g-embed");
    cfg["params"]["credential_wait_ms"] = json!(wait_ms);
    std::fs::write(&embed, serde_json::to_string_pretty(&cfg).expect("json")).expect("write");
    let env = td.path().join(".env");
    let vars: String = std::fs::read_to_string(&env)
        .expect("the env file build wrote")
        .lines()
        .map(|l| {
            if l.starts_with("MEMORY_EMBED_ENDPOINT=") {
                format!("MEMORY_EMBED_ENDPOINT={endpoint}\n")
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    std::fs::write(&env, vars).expect("rewrite the env file");
}

/// Boot with a late credential: the first round runs out without a box, the
/// receipt reaches the writer (no `no_route`), the writer's one retry opens a
/// second round, the box of that round arrives -- and the episode's row is
/// `ready` with a vector the provider was paid for with the granted key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_episode_written_before_the_credential_is_embedded_once_it_arrives() {
    if !shipped() {
        eprintln!("gh1292: the memory hive is not in this tree, skipped (GH #49)");
        return;
    }
    let (addr, _server, capture) = start_mock_server_capturing(vec![embedding_reply()]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    // The first round must run out on its own (the vault is late); the second
    // is answered at once. The deadline is product configuration (TV-Z9) at
    // the floor of every positive wait, far under the test's own marks.
    let wait_ms = u64::try_from(
        meclaw_testing::deadline::scaled(std::time::Duration::from_secs(10)).as_millis(),
    )
    .expect("ms");
    on_a_grant(&td, &format!("http://{addr}/v1/embeddings"), wait_ms);
    let (h, mut rx) = boot(&td).await;
    let db = db(&td);
    let mut seen = Vec::new();

    h.send(episode("I planted the tomatoes by the south wall today."))
        .await;
    let first = next_on(&mut rx, &mut seen, "credential_request", |_| true).await;
    // No box for the first round: it runs out, the writer sends the item once
    // more, and that call asks again.
    let second = next_on(&mut rx, &mut seen, "credential_request", |m| {
        m.id != first.id
    })
    .await;
    h.send(sealed_box(&second, "stub-secret-1292")).await;

    let ready = meclaw_testing::wait::until_some("the episode's embedding row turns ready", || {
        let got = if db.exists() {
            rows(
                &db,
                "SELECT status, model_id FROM embeddings WHERE owner_table = 'episodes'",
            )
        } else {
            Vec::new()
        };
        std::future::ready(got.iter().any(|r| r[0] == "ready").then_some(got))
    })
    .await;
    let dead: Vec<String> = h
        .drain_dead_letters()
        .await
        .iter()
        .map(|d| {
            format!(
                "{:?} -> {:?} {} {:?}",
                d.sender_path,
                d.resolved_target,
                d.reason.as_code(),
                d.message.headers.hop.get("error_code")
            )
        })
        .collect();
    assert!(
        dead.iter().all(|d| !d.contains("credential_pending")),
        "the receipt of the late credential died: {dead:?}"
    );
    assert_eq!(
        ready,
        vec![vec![
            "ready".to_string(),
            "google/gemini-embedding-2".to_string()
        ]],
        "the episode's row never got its vector; dead letters: {dead:?}"
    );
    let auth: Vec<Option<String>> = capture
        .lock()
        .await
        .iter()
        .map(|r| r.headers.get("authorization").cloned())
        .collect();
    assert_eq!(
        auth,
        vec![Some("Bearer stub-secret-1292".to_string())],
        "one paid call, with the granted key"
    );
}

/// The writer's half, bare: the first receipt sends the item again on
/// `embed_again`, the receipt of that retry is not answered.
#[test]
fn the_writer_sends_a_pending_item_once_more_and_never_twice() {
    let item = json!({"items": [{"embedding_id": "emb-1", "text": "tomatoes"}]}).to_string();
    let script = meclaw_testing::shipped_script(WRITER);
    let receipt = |retry: &str| {
        json!({
            "header": {"hop": {"error_code": "credential_pending"},
                       "context": {"mem_phase": "embed_pending",
                                   "embed_item": item, "embed_episode": "ep-1",
                                   "embed_retry": retry}},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": "the bearer credential was requested"}]
        })
    };
    let first = meclaw_testing::emit_all(&script, &receipt(""));
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0]["header"]["route"], "embed_again");
    assert_eq!(first[0]["header"]["episode_id"], "ep-1");
    assert_eq!(first[0]["messages"][0]["text"], item.as_str());
    assert_the_declaration_admits(WRITER, &first);
    let second = meclaw_testing::emit_all(&script, &receipt("1"));
    assert!(
        second.is_empty(),
        "a second receipt is answered: {second:?}"
    );
}
