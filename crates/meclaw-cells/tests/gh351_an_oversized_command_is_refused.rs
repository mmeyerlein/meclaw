//! GH #351 — a `bash` command at or above the per-argv-string cap is refused
//! before the spawn, loudly.
//!
//! Linux caps a **single** `argv` string at `MAX_ARG_STRLEN` = `32 * PAGE_SIZE`
//! = 131 072 bytes, independent of `ARG_MAX` and not raisable. `bash` hands the
//! whole command to the shell as exactly such a string (`/bin/sh -c <command>`),
//! so a command at or above that line died inside `spawn()` with
//! `Argument list too long (os error 7)` and surfaced as `error_code:
//! "io_error"` — a message that reads like an I/O fault of the child, while in
//! truth no child ever existed.
//!
//! Unlike `code`'s `script_inline`, a `bash` command is **not** template-authored:
//! it arrives per message as the `command` argument of a `tool_call`, so its size
//! is decided at runtime by whoever writes the call — typically a model. The
//! remedy #349 chose for `code` (write the oversized string to a per-spawn temp
//! file and point the runner at it) does not carry over: `sh <file>` is not
//! `sh -c <command>` — `$0` becomes the script path and `$1…` shift by one, so
//! any command reading positional parameters would change meaning. Ruling K-2:
//! refuse, do not materialise.
//!
//! What is pinned here:
//!
//! 1. a command of `MAX_ARG_STRLEN` bytes is answered with `invalid_input`;
//! 2. **no child ran** — the refused command would have created a marker file
//!    had the shell ever seen it, and the marker is absent;
//! 3. the message names the actual byte size AND the 131 072 limit — pinned by
//!    a command deliberately sized ABOVE the cap, so the two numbers differ and
//!    a message that dropped either one fails;
//! 4. a command one byte below the cap still spawns and answers normally, so
//!    the refusal is a cap and not a haircut.

use meclaw_cells::BashCell;
use meclaw_colony::StatelessCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

/// The Linux per-argv-string cap on the target platform: `32 * PAGE_SIZE` with
/// the usual 4 KiB page. Named here so the tests say WHY the commands have the
/// sizes they have.
const MAX_ARG_STRLEN: usize = 32 * 4096;

fn cell() -> BashCell {
    BashCell {
        external_timeout: std::time::Duration::from_secs(30),
        max_concurrency: 4,
        sandbox: None,
        max_bytes: 256 * 1024,
    }
}

/// Drive `handle` once through the production path and return the sole emission.
async fn run(command: &str) -> meclaw_core::CellEmission {
    let (otx, mut orx) = mpsc::channel(16);
    let sink = OutputSink::new(
        otx,
        Path::new("/bash"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new("/bash"))
        .reply_to(Path::new("/caller"))
        .body(Body::Inline(json!({
            "messages": [{
                "origin": "assistant",
                "type": "tool_call",
                "text": json!({"command": command}).to_string(),
                "id": "call-351",
            }]
        })))
        .build();
    cell().handle(msg, &sink).await;
    drop(sink);
    orx.recv().await.expect("the cell must answer")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_at_the_argv_cap_is_refused_before_the_spawn() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let marker = td.path().join("THE-SHELL-RAN");
    // Were the shell ever started, this command would touch the marker. The
    // padding is a shell comment, so the command stays valid and does exactly
    // one observable thing.
    let head = format!("touch {} #", marker.to_str().expect("utf-8 path"));
    let command = format!("{head}{}", "x".repeat(MAX_ARG_STRLEN - head.len()));
    assert_eq!(
        command.len(),
        MAX_ARG_STRLEN,
        "the pin is only a pin AT the cap"
    );

    let em = run(&command).await;
    let header = &em.content["header"];

    assert_eq!(em.target, Path::new("/caller"));
    assert_eq!(
        header["error_code"], "invalid_input",
        "an oversized command is a refused input, not an I/O fault: {header}"
    );
    assert_eq!(header["finish_reason"], "error");
    assert!(
        header.get("exit_code").is_none() || header["exit_code"].is_null(),
        "no child ran, so there is no exit code to report: {header}"
    );

    let text = em.content["messages"][0]["text"]
        .as_str()
        .expect("text")
        .to_string();
    assert!(
        text.contains("131072"),
        "the message must name the 131 072 byte limit: {text}"
    );

    assert!(
        !marker.exists(),
        "no child process may be spawned — the marker proves the shell ran"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_refusal_names_the_actual_size_next_to_the_limit() {
    // AT the cap the two numbers coincide, so a message that quietly dropped
    // the caller's own size would still read correctly. This command sits an
    // arbitrary 137 bytes ABOVE the cap, which forces them apart: 131 209 is
    // the size that arrived, 131 072 is the wall it hit. Both must be named —
    // "too long" without the numbers is the vagueness this issue is about.
    const OVER: usize = MAX_ARG_STRLEN + 137;
    let command = "x".repeat(OVER);
    assert_eq!(OVER, 131_209, "the two numbers must differ to be a pin");

    let em = run(&command).await;
    let text = em.content["messages"][0]["text"].as_str().expect("text");

    assert_eq!(em.content["header"]["error_code"], "invalid_input");
    assert!(
        text.contains("131209"),
        "the message must name the ACTUAL byte size, not just the wall: {text}"
    );
    assert!(
        text.contains("131072"),
        "the message must name the 131 072 byte limit: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_one_byte_below_the_cap_still_runs() {
    let head = "printf ok #";
    let command = format!("{head}{}", "x".repeat(MAX_ARG_STRLEN - 1 - head.len()));
    assert_eq!(
        command.len(),
        MAX_ARG_STRLEN - 1,
        "the largest command that still fits into one argv string"
    );

    let em = run(&command).await;
    let header = &em.content["header"];
    assert!(
        header.get("error_code").is_none() || header["error_code"].is_null(),
        "one byte below the cap the shell still starts: {header}"
    );
    assert_eq!(header["exit_code"], 0, "{header}");
    assert_eq!(em.content["messages"][0]["text"], "ok");
}
