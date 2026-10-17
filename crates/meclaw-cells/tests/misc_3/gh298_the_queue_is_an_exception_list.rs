//! Wave 5 -- `pending_extraction` is an exception list, not a work list (GitHub #298).
//!
//! The queue was built as a work list: the writer enqueued every turn, the store
//! echoed the insert back, and that echo walked straight into the batch gate --
//! so a row's normal fate was to be counted, claimed and paid for by a model
//! call. Per-turn extraction inverts that. The front model annotates the turn it
//! just answered, and the annotation covers the row (`inline` when it carried
//! content, `nothing` when the verdict was an honest empty one). What is left in
//! `pending` afterwards is not work waiting to be scheduled -- it is a turn whose
//! annotation NEVER ARRIVED, which is a fault worth looking at.
//!
//! This file measures the seam where that inversion begins: the `enqueue` phase
//! still inserts the row, with the provenance the writer handed over, but its
//! store echo is a DEAD END. It no longer names the gate's phase, so an insert
//! cannot start a batch, and the echo it does name parks.
//!
//! Everything here runs the REAL `params.script_inline` of `extract-glue`
//! against injected replies, so nothing costs anything.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string --
/// the same substitution the colony performs when it instantiates the template.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn glue_script() -> String {
    let raw = std::fs::read_to_string(GLUE_CONFIG).expect("extract-glue config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv (GH #279: a single argv string is
/// capped at 128 KiB and the shipped scripts have grown to within a few KB of
/// that line).
fn run_script_on_stdin(script: &str, stdin_doc: &[u8]) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(&String::from_utf8_lossy(stdin_doc).to_string()).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run the real script with a real stdin document and return the emitted messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(&glue_script(), &meclaw_testing::code_stdin_bytes(&doc));
    assert!(
        out.status.success(),
        "extract-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The conversation the enqueued turn belongs to.
const SESSION: &str = "s-gh298q";

/// The room it was spoken in.
const CHANNEL: &str = "c-gh298q";

/// Who was present. Not `["*"]` -- a universal set would let a queue row with no
/// cast at all pass this file (#244).
const AUDIENCE: &str = r#"["member:user","agent:assistant"]"#;

/// What the turn said.
const CONTENT: &str = "my favourite colour is green now";

/// One queue item exactly as the writer hands it over: the turn, plus the
/// provenance it was written with.
fn queue_item() -> String {
    serde_json::json!({
        "episode_id": "ep-q1",
        "session_id": SESSION,
        "channel": CHANNEL,
        "audience_set": AUDIENCE,
        "sender": "user",
        "content": CONTENT,
        "token_est": 9
    })
    .to_string()
}

/// The writer's hand-over as the queue edge delivers it.
fn enqueue(item: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "enqueue", "session_id": SESSION,
                        "audience_set": AUDIENCE, "channel": CHANNEL},
            "hop": {}
        },
        "messages": [{"origin": "tool", "type": "text", "text": item}]
    })
}

/// The store's echo of a write this lane emitted, in the phase that write named.
fn echo(phase: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "extract", "mem_phase": phase, "batch_id": "",
                        "session_id": SESSION,
                        "audience_set": AUDIENCE, "channel": CHANNEL},
            "hop": {"operation": "insert", "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": "[]"}]
    })
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

#[test]
fn the_enqueue_writes_the_row_and_nothing_else() {
    // One op leaves the enqueue, and it is the insert. Anything beyond it would
    // mean the write lane pays for something on the answer path -- the queue is
    // fire-and-forget by design.
    let msgs = emit(enqueue(&queue_item()));
    assert_eq!(msgs.len(), 1, "the enqueue emits one op: {msgs:?}");
    assert_eq!(msgs[0]["header"]["route"], "xstore");
    let args = args_of(&msgs[0]);
    assert_eq!(args["operation"], "insert");
    assert_eq!(args["table"], "pending_extraction");
}

#[test]
fn the_enqueue_echo_does_not_name_the_batch_gate() {
    // The inversion itself. The phase this insert names is the phase its echo
    // will come back in, and naming `gate` there is what made an enqueue START A
    // BATCH: every written turn walked into the gate's queue read and, often
    // enough, into a model call. A row is now an exception marker, so the echo of
    // writing one leads nowhere.
    let msgs = emit(enqueue(&queue_item()));
    let phase = msgs[0]["header"]["phase"]
        .as_str()
        .expect("every store op of this lane names its phase");
    assert_ne!(
        phase, "gate",
        "an enqueued turn is not a batch waiting to be paid for: {msgs:?}"
    );
    assert_eq!(
        phase, "queued",
        "and the echo it does name is the queue's own dead end: {msgs:?}"
    );
}

#[test]
fn the_queued_row_carries_the_provenance_the_writer_handed_over() {
    // The row has to say who was present without a join back to its episode:
    // a turn that sits in `pending` is one nobody annotated, and the first thing
    // anybody looking at it needs is the cast and the room -- reading them off
    // the episode means the exception list cannot be read on its own.
    let msgs = emit(enqueue(&queue_item()));
    let row = args_of(&msgs[0])["row"].clone();
    assert_eq!(
        row["status"], "pending",
        "a fresh row is the one status that means `nobody has answered for this turn`: {row}"
    );
    assert_eq!(row["episode_id"], "ep-q1");
    assert_eq!(row["session_id"], SESSION);
    assert_eq!(row["channel"], CHANNEL);
    assert_eq!(row["audience_set"], AUDIENCE);
    assert_eq!(row["sender"], "user");
    assert_eq!(row["content"], CONTENT);
}

#[test]
fn the_queued_echo_is_a_dead_end() {
    // The other half of the same statement, measured where it would otherwise
    // leak: the phase the insert names must PARK when it comes back. A phase the
    // script does not know falls through every branch and emits nothing either --
    // which looks identical from here -- so this is asserted next to the branch
    // that makes it deliberate, and the enqueue test above pins the name.
    let msgs = emit(echo("queued"));
    assert!(
        msgs.is_empty(),
        "the store echo of a queue insert leads nowhere: {msgs:?}"
    );
}
