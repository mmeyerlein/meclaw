//! GH #342 — the steward judge's declared tool set and charter reach the model,
//! or the charter rule is unenforceable.
//!
//! `templates/steward/judge/config.json` shipped its charter as `params.system`
//! and its one tool as `params.tools`. `LlmParams` has **neither** field, and
//! `LlmParams::parse` is a plain `serde_json::from_value` without
//! `deny_unknown_fields`, so both keys were dropped in silence at spawn. The
//! same keys arriving later as a params-update message would be a loud
//! `invalid_input` against `KNOWN_PARAM_KEYS`; in `config.json` at birth they
//! were simply gone.
//!
//! The consequence is the charter's own closing sentence — *"Answer with
//! exactly one tool_call to `steward_change`, and nothing else"* — addressed to
//! a model that was never shown a tool called `steward_change`, and never shown
//! the charter either.
//!
//! The only spawn-time route into the persistent `system` tree is
//! `seed/system.jsonl` (`crates/meclaw-cells/src/llm/seed.rs`), which is where
//! every other tool-carrying brain in this tree gets its identity from. So the
//! fix is a seed, and this file is its pin.
//!
//! # Why this goes to the wire and reads the shipped bytes
//!
//! Asserting on the seed file's content would pass on a seed the loader never
//! reads, and asserting on a fixture would pass on a fixture nobody ships. The
//! chain here is the shipped one: the bytes of `templates/steward/judge/`
//! (config **and** seed), `substitute_env_only` — the colony's own late-binding
//! pass — and `LlmCellFactory::spawn_cell`, which loads the seed exactly the
//! way a boot loads it. The assertion is on what the provider recorded.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCellFactory;
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path};
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The repository root, from the crate this test lives in.
fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const JUDGE_DIR: &str = "templates/steward/judge";

/// The tool the charter tells the judge to answer with.
const TOOL: &str = "steward_change";

/// The charter's closing sentence, verbatim from the shipped `answer` block.
/// If the seed stops carrying it, the instruction that names the tool is gone
/// and the tool alone would not restore it.
const ANSWER_SENTENCE: &str =
    "Answer with exactly one tool_call to `steward_change`, and nothing else.";

/// The charter's opening words, verbatim from the shipped `role` block —
/// `system_order` has to put this first, or the argument arrives shuffled.
const ROLE_OPENING: &str = "You are the judge of a colony's control loop.";

/// Whether the steward ships with this checkout (the documented R2b exception
/// form): a public clone without the template skips instead of failing on a
/// dead `templates/` reference.
fn shipped() -> Option<std::path::PathBuf> {
    let path = core_root().join(JUDGE_DIR);
    path.join("config.json").is_file().then_some(path)
}

/// Copy `src` into `dst`, directories and all.
fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("target dir");
    for entry in std::fs::read_dir(src).expect("readable source") {
        let entry = entry.expect("dir entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy");
        }
    }
}

/// Materialise the shipped judge directory as a cell directory in `td`, with
/// the `${…}` params already resolved against `env` — the substitution the
/// colony performs at boot, run here so the mock's port can be reached without
/// inventing a second code path. Everything else (`seed/` above all) travels
/// verbatim.
fn install_resolved_judge(
    td: &TempDir,
    shipped_dir: &std::path::Path,
    env: &HashMap<String, String>,
) -> std::path::PathBuf {
    let cell_dir = td.path().join("judge");
    copy_tree(shipped_dir, &cell_dir);

    let raw = std::fs::read_to_string(shipped_dir.join("config.json"))
        .expect("the shipped judge config is on disk");
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("the judge config parses");
    let resolved = meclaw_colony::mutation::substitute::substitute_env_only(&cfg, env)
        .expect("the judge config substitutes");
    std::fs::write(
        cell_dir.join("config.json"),
        meclaw_core::serde_json::to_vec_pretty(&resolved).expect("serialise"),
    )
    .expect("resolved config");
    cell_dir
}

/// Spawn the cell through the real factory — the same call the colony makes, so
/// `seed/system.jsonl` is loaded by the code that loads it at boot.
#[allow(clippy::type_complexity)]
fn spawn(
    cell_dir: &std::path::Path,
    raw: Value,
) -> Result<
    (
        mpsc::Sender<Message>,
        mpsc::Receiver<Message>,
        meclaw_colony::WakeFn,
        mpsc::Receiver<CellEmission>,
    ),
    String,
> {
    let (otx, orx) = mpsc::channel::<CellEmission>(8);
    let (itx, irx) = mpsc::channel(8);
    // The colony inbox receiver must outlive the cell task (the watcher sends
    // `CellDied` into it); leaking it keeps the channel open for the test.
    std::mem::forget(irx);
    let kind = Arc::new(LlmCellFactory).spawn_cell(
        Path::new("/steward/judge"),
        raw,
        otx,
        cell_dir.to_path_buf(),
        meclaw_colony::ContractView::default(),
        itx,
        None,
        32,
        None,
        None,
        16,
    )?;
    match kind {
        SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            ..
        } => Ok((sender, receiver, wake, orx)),
        SpawnedCellKind::Active { .. } => unreachable!("llm spawns Dormant"),
    }
}

/// Drive one inference through the shipped judge and hand back what the
/// provider recorded.
///
/// Nothing here re-states a shipped param: the env below binds only the four
/// settings `contract.settings` declares, exactly as an operator's `.env`
/// would. `STEWARD_JUDGE_PROVIDER` is bound to `openai` because that is what the
/// template now DEFAULTS to (GH #387): `LlmParams` accepts no other adapter
/// today, so the binding here restates the shipped default rather than
/// overriding it -- the mock speaks the same Chat-Completions wire.
async fn drive_the_shipped_judge(mock: &MockOpenAI) -> OpenAiRequestSnapshot {
    let shipped_dir = shipped().expect("caller checked");
    let td = TempDir::new().expect("tempdir");
    let env = HashMap::from([
        ("STEWARD_JUDGE_PROVIDER".to_string(), "openai".to_string()),
        (
            "STEWARD_JUDGE_BASE_URL".to_string(),
            format!("{}/v1", mock.base_url),
        ),
        ("STEWARD_JUDGE_MODEL".to_string(), "gpt-x".to_string()),
        ("OPENROUTER_API_KEY".to_string(), "sk-test".to_string()),
    ]);
    let cell_dir = install_resolved_judge(&td, &shipped_dir, &env);
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(cell_dir.join("config.json")).expect("resolved config readable"),
    )
    .expect("resolved config parses");
    let params = cfg
        .get("params")
        .cloned()
        .expect("the judge config carries params");

    let (sender, receiver, wake, mut orx) =
        spawn(&cell_dir, params).expect("the shipped judge must spawn");
    wake(receiver);
    sender
        .send(
            MessageBuilder::new(Path::new("/steward/judge"))
                .reply_to(Path::new("/steward/mutator"))
                .body(Body::Inline(json!({
                    "messages": [{
                        "origin": "user", "type": "text",
                        "text": "cycle gh342: cost_per_answer 0.0141 USD over 120 samples"
                    }]
                })))
                .build(),
        )
        .await
        .expect("the judge accepts the measurement");
    orx.recv().await.expect("the judge must emit something");

    let mut snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "exactly one provider call per inference");
    snaps.remove(0)
}

/// The defect. The charter orders exactly one `tool_call` to `steward_change`;
/// if the tool is not on the wire the model cannot obey, and the rule the whole
/// steward rests on — a change comes with its own pre-authored revert plan —
/// is unenforceable at the only place it could be enforced.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_judge_declares_steward_change_to_the_provider() {
    if shipped().is_none() {
        return;
    }
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let snap = drive_the_shipped_judge(&mock).await;

    let tools = snap
        .tools()
        .expect("the judge's request must carry tools[] — the charter demands a tool_call");
    assert_eq!(
        tools.len(),
        1,
        "the judge declares exactly one tool: {tools:?}"
    );
    assert_eq!(
        tools[0]["function"]["name"], TOOL,
        "the tool the charter names must be the tool the provider sees: {tools:?}"
    );
}

/// The sibling half. `params.system` was dead in the same file for the same
/// reason, so the judge would have arrived with a tool and no instructions —
/// and the tool's own description ("The only way to answer.") is not a charter.
/// `system_order` is pinned along with it: the charter is an ordered argument,
/// and the default would deliver it alphabetically, `answer` first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_judge_charter_reaches_the_system_message_in_order() {
    if shipped().is_none() {
        return;
    }
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let snap = drive_the_shipped_judge(&mock).await;

    let messages = snap.messages().expect("request must carry messages[]");
    assert_eq!(
        messages[0]["role"], "system",
        "the charter must lead the request: {messages:?}"
    );
    let system = messages[0]["content"]
        .as_str()
        .expect("the system message is a string");
    assert!(
        system.contains(ANSWER_SENTENCE),
        "the charter sentence that names the tool must reach the model: {system:?}"
    );
    assert!(
        system.starts_with(ROLE_OPENING),
        "system_order must put `role` first — an alphabetical charter opens with its own \
         closing instruction: {system:?}"
    );
    assert!(
        !system.contains("\"type\": \"function\"") && !system.contains("\"type\":\"function\""),
        "the tools subtree must stay out of the prompt string, as it always has: {system:?}"
    );
}
