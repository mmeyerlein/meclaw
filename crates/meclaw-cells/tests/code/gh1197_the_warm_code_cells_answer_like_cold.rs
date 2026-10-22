//! GH #1197 -- the file-space and projection cells whose script holds nothing
//! between messages run `warm` (R-DT-13: the runner mode is decided per cell).
//!
//! **Warm answers like cold** -- the shipped script of every warm cell below,
//! spawned on the production path (`CodeCellFactory`) once `warm` with ONE
//! child and once `cold`, gets the same two messages one after the other, and
//! warm's answers equal cold's. The engine lives in
//! `support/warm_equivalence.rs`; GH #1276 runs it over every other warm cell
//! from data (`gh1276_every_warm_code_cell_answers_like_cold`).
//!
//! The pin of the shipped modes that stood here (the exact warm set of
//! file-space and projection, every other cell cold) is gone with GH #1276:
//! warm is the default now, and the mode of every code cell is locked once,
//! for all templates, by the inventory (`templates/RUNNER-MODES.md`,
//! `gh1276_every_code_cell_has_its_runner_mode`).

// harness_fold: #[path = "support/warm_equivalence.rs"]
#[allow(unused_imports)]
use crate::warm_equivalence; // harness_fold: mod warm_equivalence;

use meclaw_core::serde_json::{Value, json};
use warm_equivalence::{Probe, answers, no_overlay, probe};

/// The cells GH #1197 made warm. Every one is reached several times per turn
/// or per file operation, reads its input from the message and keeps no
/// process state between messages; the report of the strand gives the line
/// per cell. They are the part of `WARM_BEFORE_1276` whose probes are written
/// here by hand.
const WARM: [&str; 7] = [
    "file-space/guard",
    "file-space/ingest",
    "file-space/read",
    "file-space/tools",
    "file-space/write",
    "file-space/ws",
    "projection/mat",
];

/// The acceptance of GH #1197 for one cell: two messages, warm on one child
/// against cold, the same answers.
async fn warm_answers_like_cold(cell: &str, overlay: impl Fn(&str) -> Value, probes: [Probe; 2]) {
    assert!(
        WARM.contains(&cell),
        "{cell}: only a warm cell needs the pin"
    );
    warm_equivalence::warm_answers_like_cold(cell, overlay, &probes).await;
}

/// The control: the comparison sees a leak. A script that keeps a counter on
/// `sys` -- process state, which a fresh globals dict does not reset -- answers
/// `1` to both messages cold and `2` to message 2 warm, and the pin's very
/// comparison tells them apart. Without this, a comparator that masked too much
/// would pass every cell above.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_script_that_keeps_process_state_answers_differently_warm() {
    let leaky = json!({
        "runner": "python3",
        "external_timeout_ms": 30_000,
        "script_inline": "import sys, json\n\
                          json.load(sys.stdin)\n\
                          sys.meclaw_gh1197 = getattr(sys, 'meclaw_gh1197', 0) + 1\n\
                          sys.stdout.write(json.dumps({'messages': [], 'n': sys.meclaw_gh1197}))\n"
    });
    let two = || [probe(json!({}), json!({})), probe(json!({}), json!({}))];
    let cold = answers("control/leaky", &leaky, "cold", &json!({}), None, &two()).await;
    let warm = answers("control/leaky", &leaky, "warm", &json!({}), None, &two()).await;
    assert_eq!(warm[0], cold[0], "message 1 starts from the same state");
    assert_ne!(
        warm[1], cold[1],
        "message 2 warm sees what message 1 left on `sys`; the comparison must say so"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guard_answers_alike_warm_and_cold() {
    warm_answers_like_cold(
        "file-space/guard",
        no_overlay,
        [
            probe(
                json!({"route": "in_check", "op_id": "w-1"}),
                json!({"kind": "text", "path": "/a.toml", "text": "a = ["}),
            ),
            probe(
                json!({"route": "in_check", "op_id": "w-2"}),
                json!({"kind": "text", "path": "/b.json", "text": "{\"x\": 1}"}),
            ),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ingest_answers_alike_warm_and_cold() {
    warm_answers_like_cold(
        "file-space/ingest",
        no_overlay,
        [
            probe(
                json!({"route": "in_ingest"}),
                json!({
                    "messages": [{"origin": "user", "type": "text", "text": "see"}],
                    "attachments": [{"blob_id": "b1", "mime_type": "text/plain",
                                     "filename": "a.txt", "size_bytes": 5, "sha256": "00"}]
                }),
            ),
            probe(
                json!({"route": "in_ingest"}),
                json!({"messages": [{"origin": "user", "type": "text", "text": "no file"}]}),
            ),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_answers_alike_warm_and_cold() {
    warm_answers_like_cold(
        "file-space/read",
        no_overlay,
        [
            probe(
                json!({"route": "in_read", "op": "read", "op_id": "r-1", "caller": "tools"}),
                json!({"file": "/a.md", "args": {}}),
            ),
            probe(
                json!({"route": "in_read", "op": "list", "op_id": "r-2", "caller": "tools"}),
                json!({"args": {"dir": "/"}}),
            ),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tools_answers_alike_warm_and_cold() {
    let call = |id: &str, tool: &str, text: &str| Probe {
        hop: json!({"route": "in_tool", "tool_name": tool, "tool_call_id": id}),
        context: json!({"tool_caller": "/brain"}),
        body: json!({"messages": [{"id": id, "text": text}]}),
    };
    warm_answers_like_cold(
        "file-space/tools",
        no_overlay,
        [
            call("c1", "file_read", "{\"file\": \"/a.md\"}"),
            call("c2", "file_list", "{}"),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_answers_alike_warm_and_cold() {
    warm_answers_like_cold(
        "file-space/write",
        no_overlay,
        [
            probe(
                json!({"route": "in_write", "op": "create", "op_id": "tools:c1", "caller": "tools"}),
                json!({"args": {"path": "/a.md", "text": "hello\n"}}),
            ),
            probe(
                json!({"route": "in_write", "op": "overwrite", "op_id": "tools:c2", "caller": "tools"}),
                json!({"file": "/b.md", "args": {"text": "more\n"}}),
            ),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_answers_alike_warm_and_cold() {
    warm_answers_like_cold(
        "file-space/ws",
        no_overlay,
        [
            probe(
                json!({"route": "in_ws", "op": "ws_open", "op_id": "tools:c1", "caller": "tools",
                       "ws": "w1"}),
                json!({"args": {"name": "w1"}}),
            ),
            probe(
                json!({"route": "in_ws", "op": "ws_status", "op_id": "tools:c2", "caller": "tools",
                       "ws": "w2"}),
                json!({"args": {}}),
            ),
        ],
    )
    .await;
}

/// `projection/mat` writes below `base_path`, which the owner's override grants
/// to its sandbox (as `gh905_a_workspace_is_materialized_and_adopted` does);
/// each mode gets its own base, so neither run finds the other's layout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mat_answers_alike_warm_and_cold() {
    let owner = |base: &str| {
        json!({
            "base_path": base,
            "exec_allow": ["true"],
            "sandbox": {"trust": "restricted", "network": "deny",
                        "filesystem": {"runtime": true, "write": [base]}}
        })
    };
    warm_answers_like_cold(
        "projection/mat",
        owner,
        [
            probe(
                json!({"route": "in_proj", "op": "ws_materialize", "op_id": "m-1", "caller": "tools",
                       "ws": "w1"}),
                json!({"args": {}}),
            ),
            probe(
                json!({"route": "in_proj", "op": "ws_exec", "op_id": "m-2", "caller": "tools",
                       "ws": "w1"}),
                json!({"args": {"argv": ["true"]}}),
            ),
        ],
    )
    .await;
}
