//! GH #957 -- the model of a decisions cell changes at run time, its
//! provider never does.
//!
//! The push is the registry's own form (`{system: {}, params: {model,
//! $reset: [...]}}`, the whole model package with every other package key
//! reset): it is answered with silence and no request, and the NEXT call
//! carries the new model. A push naming `provider` is refused on the error
//! path and changes nothing -- the call after it still runs on the old model.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::{LlmParams, MODEL_PACKAGE_KEYS};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::MockResponse;
use mock_openai::MockOpenAI;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn mk_sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/decide"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

fn msg(body: Value) -> Message {
    MessageBuilder::new(Path::new("/decide"))
        .reply_to(Path::new("/caller"))
        .body(Body::Inline(body))
        .build()
}

fn decide() -> Value {
    json!({"messages": [], "decide": {"state": "s", "questions": {
        "wants": {"kind": "yes_no", "instructions": "Show it?"}}}})
}

fn yes() -> MockResponse {
    MockResponse::ok_json(br#"{"answers": {"wants": {"type": "noul", "noul": 0.9}}}"#)
}

fn drain(rx: &mut mpsc::Receiver<CellEmission>) -> Vec<CellEmission> {
    let mut out = Vec::new();
    while let Ok(em) = rx.try_recv() {
        out.push(em);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pushed_model_is_the_model_of_the_next_call_and_the_provider_stays() {
    let mock = MockOpenAI::start(vec![yes(), yes()]).await;
    let raw = json!({"provider": "decisions", "model": "vendor/decider-a", "api_key": "k",
                     "base_url": format!("{}/api", mock.base_url)});
    let mut cell = LlmCell::new(
        LlmParams::parse(&raw).unwrap(),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (sink, mut rx) = mk_sink();

    // The registry's push: the package, and every other package key reset.
    let reset: Vec<&str> = MODEL_PACKAGE_KEYS
        .iter()
        .copied()
        .filter(|k| *k != "model")
        .collect();
    cell.handle(
        msg(json!({"system": {}, "params": {"model": "vendor/decider-b", "$reset": reset}})),
        &sink,
        &mut db,
    )
    .await;
    assert!(drain(&mut rx).is_empty(), "a push is answered with silence");
    assert!(
        mock.recorded_requests().await.is_empty(),
        "a push asks nobody"
    );

    cell.handle(msg(decide()), &sink, &mut db).await;
    let ems = drain(&mut rx);
    assert_eq!(ems.len(), 1);
    assert!(
        ems[0].content.get("decision").is_some(),
        "{}",
        ems[0].content
    );
    assert_eq!(
        mock.recorded_requests().await[0].body["model"],
        "vendor/decider-b",
        "the next call carries the pushed model"
    );

    // A push naming the provider is refused, and nothing moves.
    cell.handle(
        msg(json!({"system": {}, "params": {"provider": "openai"}})),
        &sink,
        &mut db,
    )
    .await;
    let refused = drain(&mut rx);
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].content["header"]["error_code"], "invalid_input");
    assert_eq!(cell.params.provider, "decisions");

    cell.handle(msg(decide()), &sink, &mut db).await;
    assert_eq!(drain(&mut rx).len(), 1);
    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 2);
    assert_eq!(snaps[1].body["model"], "vendor/decider-b");
    assert_eq!(snaps[1].path, "/api/alpha/decisions");
}

/// Review M-4: the registry's push carries the row's `base_url` too. A
/// decisions cell follows it only to an origin in its `base_url_allow`
/// (GH #853) -- with the origin listed, the next call goes to the new
/// endpoint; without it, the push is refused and the old endpoint stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pushed_base_url_needs_its_origin_in_the_allow_list() {
    let old = MockOpenAI::start(vec![yes()]).await;
    let new = MockOpenAI::start(vec![yes()]).await;
    let reset: Vec<&str> = MODEL_PACKAGE_KEYS
        .iter()
        .copied()
        .filter(|k| *k != "model" && *k != "base_url")
        .collect();
    let push = json!({"system": {}, "params": {"model": "vendor/decider-b",
        "base_url": format!("{}/api", new.base_url), "$reset": reset}});

    for allowed in [false, true] {
        let mut raw = json!({"provider": "decisions", "model": "vendor/decider-a",
            "api_key": "k", "base_url": format!("{}/api", old.base_url)});
        if allowed {
            raw["base_url_allow"] = json!([new.base_url.clone()]);
        }
        let mut cell = LlmCell::new(
            LlmParams::parse(&raw).unwrap(),
            reqwest::Client::builder().build().unwrap(),
        );
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = DbConn::wrap(conn, None);
        let (sink, mut rx) = mk_sink();

        cell.handle(msg(push.clone()), &sink, &mut db).await;
        let answered = drain(&mut rx);
        if allowed {
            assert!(
                answered.is_empty(),
                "an allowed push is silence: {answered:?}"
            );
        } else {
            assert_eq!(answered.len(), 1, "a refused push is one error");
            assert_eq!(answered[0].content["header"]["error_code"], "invalid_input");
        }

        cell.handle(msg(decide()), &sink, &mut db).await;
        let ems = drain(&mut rx);
        assert_eq!(ems.len(), 1);
        assert!(
            ems[0].content.get("decision").is_some(),
            "{}",
            ems[0].content
        );
    }
    let at_old = old.recorded_requests().await;
    let at_new = new.recorded_requests().await;
    assert_eq!(
        at_old.len(),
        1,
        "the refused push left the cell on its endpoint"
    );
    assert_eq!(at_old[0].body["model"], "vendor/decider-a");
    assert_eq!(at_new.len(), 1, "the allowed push moved the next call");
    assert_eq!(at_new[0].body["model"], "vendor/decider-b");
    assert_eq!(at_new[0].path, "/api/alpha/decisions");
}
