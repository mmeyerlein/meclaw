//! GH #405 — a bundle whose creates are all refused still applies its deletes.
//!
//! Split out of GH #402 as its option 3. #402 itself is fixed — `canvy` defines
//! its components before it writes, so nothing shipped reaches this path any
//! more — and what remained was the general property of the `web` cell.
//!
//! WHAT IS BEING PINNED, AND WHY IT IS A PIN AND NOT A FIX
//! ======================================================
//! The behaviour is not a regression. "A bundle is not a transaction" was ruled
//! for the `store` in W4 (GH #295), inherited by the `web` cell in W8 (task 7,
//! step 3), and documented in both language versions of `docs/cell-types` ever
//! since. Nothing here changes it. The ruling taken for this issue is **option
//! 3**: a bundle stays a sequence of independent ops, and the honest thing to
//! do is to say so where the damage actually shows — plus give the caller the
//! recipe that avoids it. A bundle-level `atomic` / `stop_on_error` flag
//! (option 4) is a contract addition on a public surface and is deliberately
//! not taken here.
//!
//! The awkward shape is worth naming precisely, because it is not partiality in
//! general. `object.delete` needs no component; `object.create` looks one up.
//! So `unknown_component` is structurally a filter that removes exactly the
//! constructive legs of a bundle and lets the destructive ones through: a patch
//! that failed to write anything still destroyed what was there. The sender's
//! intent was "make the tree look like this"; what it achieved was "remove what
//! does not belong in a tree I could not build".
//!
//! Three tests, and the third is the half that makes this a drift lock rather
//! than a string bet: the documented recipe is executed, not quoted.

use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, Path};
use meclaw_testing::free_port;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;

/// A page with a stack root and two leaf children — two things a delete can
/// take away, because `object.delete` refuses a node that still has children.
fn seed(cell_dir: &std::path::Path) {
    let seed = cell_dir.join("seed");
    std::fs::create_dir_all(&seed).expect("seed dir");
    std::fs::write(
        seed.join("components.jsonl"),
        concat!(
            r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
            "\n",
            r#"{"name":"stack","template":"<main>{{children}}</main>","prop_schema":"{}","editable":"[]","layer":"content"}"#,
            "\n",
            r#"{"name":"text","template":"<p>{{body}}</p>","prop_schema":"{\"body\":\"text\"}","editable":"[]","layer":"content"}"#,
            "\n"
        ),
    )
    .expect("components");
    std::fs::write(
        seed.join("objects.jsonl"),
        concat!(
            r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
            "\n",
            r#"{"id":"root","parent":null,"component":"stack","ord":0,"props":"{}"}"#,
            "\n",
            r#"{"id":"a","parent":"root","component":"text","ord":0,"props":"{\"body\":\"first\"}"}"#,
            "\n",
            r#"{"id":"b","parent":"root","component":"text","ord":1,"props":"{\"body\":\"second\"}"}"#,
            "\n"
        ),
    )
    .expect("objects");
    std::fs::write(
        seed.join("pages.jsonl"),
        concat!(
            r#"{"schema":{"route":"text","root":"text","title":"text"}}"#,
            "\n",
            r#"{"route":"/","root":"root","title":"Home"}"#,
            "\n"
        ),
    )
    .expect("pages");
}

struct Live {
    port: u16,
    mailbox: mpsc::Sender<meclaw_core::Message>,
    out_rx: mpsc::Receiver<CellEmission>,
    _stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

async fn start(cell_dir: &std::path::Path) -> Live {
    let port = free_port();
    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(64);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory)
        .spawn_cell(
            Path::new("/web"),
            json!({ "port": port }),
            out_tx,
            cell_dir.to_path_buf(),
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("spawn");
    let SpawnedCellKind::Active {
        join,
        sender,
        stop_tx,
        ..
    } = spawned
    else {
        panic!("Active");
    };

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(r) = reqwest::get(format!("http://127.0.0.1:{port}/")).await
            && r.status().is_success()
        {
            break;
        }
        assert!(Instant::now() < deadline, "the cell never served its page");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    Live {
        port,
        mailbox: sender,
        out_rx,
        _stop: stop_tx,
        join,
    }
}

/// Send a body carrying the given tool calls, and read the one reply.
async fn call(live: &mut Live, calls: &[(&str, Value)]) -> Value {
    let turns: Vec<Value> = calls
        .iter()
        .map(|(id, args)| {
            json!({"origin": "tool", "type": "tool_call", "text": args.to_string(), "id": id})
        })
        .collect();
    let msg = MessageBuilder::new(Path::new("/web"))
        .body(Body::Inline(json!({ "messages": turns })))
        .reply_to(Path::new("/caller"))
        .build();
    live.mailbox.send(msg).await.expect("mailbox");

    let emission = tokio::time::timeout(Duration::from_secs(30), live.out_rx.recv())
        .await
        .expect("the cell must answer a tool call")
        .expect("an emission");
    emission.content
}

async fn page(live: &Live) -> String {
    reqwest::get(format!("http://127.0.0.1:{}/", live.port))
        .await
        .expect("get")
        .text()
        .await
        .expect("text")
}

/// The legs a caller sends when it means "make the tree look like this" and its
/// components are not defined: every create is refused, every delete is not.
fn rebuild_bundle() -> Vec<(&'static str, Value)> {
    vec![
        ("del-a", json!({"op": "object.delete", "id": "a"})),
        ("del-b", json!({"op": "object.delete", "id": "b"})),
        (
            "new-1",
            json!({"op": "object.create", "id": "n1", "parent": "root", "component": "ghost", "props": {}}),
        ),
        (
            "new-2",
            json!({"op": "object.create", "id": "n2", "parent": "root", "component": "ghost", "props": {}}),
        ),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bundle_whose_creates_are_all_refused_still_applies_its_deletes() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let before = page(&live).await;
    assert!(before.contains("first") && before.contains("second"));

    let reply = call(&mut live, &rebuild_bundle()).await;

    // Every create refused, by name.
    assert_eq!(
        reply["header"]["bundle_errors"],
        json!(2),
        "both creates were refused: {reply}"
    );
    for leg in [2usize, 3] {
        assert_eq!(
            reply["results"][leg]["error_code"],
            json!("unknown_component"),
            "leg {leg} names the reason: {reply}"
        );
    }
    // And the deletes landed anyway. THIS is the property of the issue: the
    // reply reports a patch that wrote nothing, and the tree it was patching is
    // smaller than it was.
    assert_eq!(
        reply["header"]["rows_affected"],
        json!(2),
        "the two deletes counted — a bundle is a sequence of independent ops: {reply}"
    );
    assert!(
        !reply["header"]
            .as_object()
            .expect("header is an object")
            .contains_key("error_code"),
        "the reply as a whole is not a refusal — `bundle_errors` is where a \
         caller learns this, and it is the only place: {reply}"
    );

    let after = page(&live).await;
    assert!(
        !after.contains("first") && !after.contains("second"),
        "the tree lost what the sender could not rebuild: {after}"
    );

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_documented_recipe_sends_two_bundles_and_keeps_the_tree() {
    // The other half of the ruling, executed rather than quoted: a destructive
    // bundle goes out as two, the first one's `bundle_errors` is read, and the
    // deletes are never sent when it is not zero. This is what makes the
    // documented advice a mechanism instead of a sentence.
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let all = rebuild_bundle();
    let creates: Vec<(&str, Value)> = all
        .iter()
        .filter(|(_, a)| a["op"] == json!("object.create"))
        .map(|(id, a)| (*id, a.clone()))
        .collect();
    let deletes: Vec<(&str, Value)> = all
        .iter()
        .filter(|(_, a)| a["op"] == json!("object.delete"))
        .map(|(id, a)| (*id, a.clone()))
        .collect();

    let reply = call(&mut live, &creates).await;
    let errors = reply["header"]["bundle_errors"]
        .as_i64()
        .expect("bundle_errors is stamped unconditionally, including as 0");
    if errors == 0 {
        call(&mut live, &deletes).await;
    }
    assert_eq!(
        errors, 2,
        "the creates were refused, so the deletes stayed home"
    );

    let after = page(&live).await;
    assert!(
        after.contains("first") && after.contains("second"),
        "nothing was destroyed by a patch that could not be built: {after}"
    );

    live.join.abort();
}

/// The drift-lock half: the sentence and the mechanism, in one file.
///
/// Both language versions must carry the statement, because the spec trias is a
/// pair and a promise a reader cannot read in their own language is not a
/// promise (`docs/development-rules.md` § 3).
///
/// WHY IT ASKS WHICH TREE IT IS IN
/// ==============================
/// The export publishes each `docs/X.en.md` under the plain name `docs/X.md`
/// and ships no German original (`development-rules.md` § 2a). So in the public
/// tree `cell-types.md` holds the ENGLISH text and `cell-types.en.md` does not
/// exist at all — a test that demanded the German sentence from `cell-types.md`
/// would be red there for being right here. The presence of the `.en.md` twin
/// is what tells the two trees apart, and the counter is what keeps an empty
/// run from looking like a successful one.
#[test]
fn both_language_versions_say_what_a_destructive_bundle_costs() {
    const DE: &str = "geloescht, was da war";
    const EN: &str = "still destroyed what was there";

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    let private = root.join("cell-types.en.md").exists();
    let expected: Vec<(&str, &str)> = if private {
        vec![("cell-types.md", DE), ("cell-types.en.md", EN)]
    } else {
        // Public tree: one file, and it carries the English half.
        vec![("cell-types.md", EN)]
    };

    let mut checked = 0usize;
    for (file, needle) in &expected {
        let path = root.join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} must be readable: {e}", path.display()));
        assert!(
            text.contains(needle),
            "{file} does not state what a bundle whose creates are all refused \
             does to its deletes (GH #405). The behaviour is pinned by \
             `a_bundle_whose_creates_are_all_refused_still_applies_its_deletes` \
             in this file; a mechanism nobody documented is the drift this lock \
             exists to catch."
        );
        checked += 1;
    }
    assert_eq!(
        checked,
        expected.len(),
        "every language version this tree carries was read — an empty run and a \
         forgotten call must never look alike"
    );
    assert!(
        checked >= 1,
        "the docs directory carried neither language version"
    );
}
