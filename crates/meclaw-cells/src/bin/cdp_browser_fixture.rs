//! Test fixture, not a product surface: a browser double that speaks CDP.
//!
//! It is started exactly the way a real browser is — through the shell shim
//! that puts the pipes on fd 3 and fd 4 — and the first thing it does is check
//! that they ARE the pipes, because a shim that silently put fd 4 on stderr
//! would let every test below pass while the real browser answered into a log
//! file. The verdict travels twice: as a line on stderr (`fixtureFds 3=pipe
//! 4=pipe`) for whoever reads the run, and inside `Browser.getVersion` for the
//! test that asserts it.
//!
//! Usage: `cdp_browser_fixture --fixture-mode=<mode> [any browser flags]`.
//! Every flag it does not know is ignored, because it is started with the full
//! browser command line.
//!
//! Flags (any mode):
//! - `--fixture-pid-file=<path>` — write this process's pid there, so a test
//!   can prove it really is gone afterwards without a name pattern.
//! - `--fixture-frame-ms=<n>` — how fast a started screencast produces frames.
//!   A frame still only leaves while fewer than [`IN_FLIGHT`] of them are
//!   unacknowledged, because that is the whole flow control of a real
//!   screencast (wave G, g7): measured 2026-09-20 against Chromium
//!   153.0.8010.36, a cast that is never acknowledged sends THREE frames and
//!   then stands for good, and `Page.stopScreencast` + `Page.startScreencast`
//!   is what revives it. A fixture that sends on a timer alone lets a cell
//!   that never acknowledges anything past a green suite.
//!   The default is fast enough that a test does not wait for its picture.
//! - `--fixture-still` — the page stands still, the way most pages do: a
//!   started screencast produces exactly ONE frame and then nothing until it
//!   is stopped and started again. Measured against Chromium 153 on
//!   2026-09-19; the default (a frame every `--fixture-frame-ms`) models a
//!   page that animates, and modelling only that is what let finding B-G3
//!   past a green suite.
//!
//! Modes:
//! - `ok` — answers everything. The happy path.
//! - `mute` — reads and answers nothing, ever. The A-timeout's subject.
//! - `die-after-first` — answers one call, then exits 0 with a call still in
//!   flight.
//! - `stillborn` — never answers anything: one line on stderr and exit 133,
//!   the shape a packaged browser takes where its own sandbox cannot run.
//! - `crash-renderer` — answers, and emits `Inspector.targetCrashed` after the
//!   first `Page.navigate`.
//! - `same-namespace` — its child STAYS in this user namespace: what a browser
//!   whose sandbox does not hold looks like from outside.
//! - `own-namespace` — nothing beyond the default. Every other mode forks a
//!   child into a user namespace of its own, which is what a working browser
//!   looks like, so the cell's post-spawn check passes in all of them.
//!
//! Deliberately synchronous and dependency-free apart from `serde_json`: a
//! subtle bug in here would look like a bug in the code under test.

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::fs::FileTypeExt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args
        .iter()
        .find_map(|a| a.strip_prefix("--fixture-mode="))
        .unwrap_or("ok")
        .to_string();

    if let Some(path) = args
        .iter()
        .find_map(|a| a.strip_prefix("--fixture-pid-file="))
    {
        let _ = std::fs::write(path, std::process::id().to_string());
    }

    // SAFETY: fd 3 and fd 4 are handed over by the shell shim the cell starts
    // this process through, and nothing else in this process owns them. They
    // are taken exactly once, here.
    let mut inbound = unsafe { std::fs::File::from_raw_fd(3) };
    let outbound = unsafe { std::fs::File::from_raw_fd(4) };

    let fds = format!("3={} 4={}", kind_of(&inbound), kind_of(&outbound));
    eprintln!("fixtureFds {fds}");
    // Shared, because a running screencast writes frames from a thread of its
    // own while the main loop is still answering calls.
    let outbound = Arc::new(Mutex::new(outbound));
    let frame_ms: u64 = args
        .iter()
        .find_map(|a| a.strip_prefix("--fixture-frame-ms="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    // A page that stands still: one frame per start, then silence. That is
    // what a real screencast does, and modelling only the animating page is
    // what let B-G3 past a green suite.
    let still = args.iter().any(|a| a == "--fixture-still");
    let casting: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    // What a browser will send before it waits for an acknowledgement.
    let in_flight: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));

    // Every mode forks a child, because the cell looks at the child chain
    // before it hands out a page (R-G13). Only `same-namespace` forks one that
    // stays here, which is what a browser whose sandbox did not hold looks like.
    spawn_probe_child(mode != "same-namespace");

    if mode == "stillborn" {
        // What a packaged browser does where its own sandbox cannot run: it
        // never answers a single call, writes one line and leaves with 133.
        // The cell has to read that as a spawn that failed and not as a
        // browser that crashed -- an operator sent after `browser_crashed`
        // goes looking for a page that never existed.
        eprintln!("Failed to move to new namespace: No usable sandbox!");
        std::process::exit(133);
    }

    if mode == "mute" {
        // Read and never answer. The process stays alive so the caller's
        // A-timeout is what ends the call, not the death of the child.
        let mut sink = Vec::new();
        let _ = inbound.read_to_end(&mut sink);
        return;
    }

    let mute_eval = args.iter().any(|a| a == "--fixture-mute-eval");
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    let mut answered = 0usize;
    loop {
        buf.clear();
        loop {
            match inbound.read(&mut byte) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if byte[0] == 0 {
                break;
            }
            buf.push(byte[0]);
        }
        let Ok(call) = serde_json::from_slice::<Value>(&buf) else {
            continue;
        };
        let id = call.get("id").and_then(Value::as_u64).unwrap_or(0);
        let method = call
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let session = call
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string);

        let params = call.get("params").cloned().unwrap_or(json!({}));
        if mute_eval && method == "Runtime.evaluate" {
            // Never answered, so the caller's A-timeout is what ends the call.
            continue;
        }
        let result = answer(&method, &params, &fds);
        write_frame(&outbound, &reply(id, result, session.as_deref()));
        answered += 1;

        // Every input and every emulation call is repeated as an event, so a
        // test can read what the cell sent without asking a browser to behave.
        if method.starts_with("Input.") || method.starts_with("Emulation.") {
            write_frame(
                &outbound,
                &json!({
                    "method": "Fixture.input",
                    "sessionId": session,
                    "params": {"method": method, "params": params}
                }),
            );
        }
        // A navigation is followed by what a browser says about it: the target's
        // full address (fragment included) and the load that finished.
        if method == "Page.navigate"
            && let Some(url) = params.get("url").and_then(Value::as_str)
        {
            set_last_url(url);
            write_frame(
                &outbound,
                &json!({
                    "method": "Target.targetInfoChanged",
                    "params": {"targetInfo": {
                        "targetId": target_of(session.as_deref()),
                        "type": "page",
                        "url": url,
                        "title": "A fixture page",
                        "attached": true,
                    }}
                }),
            );
            write_frame(
                &outbound,
                &json!({
                    "method": "Page.loadEventFired",
                    "sessionId": session,
                    "params": {"timestamp": 1.0}
                }),
            );
        }

        match method.as_str() {
            "Page.startScreencast" => {
                if !casting.swap(true, Ordering::SeqCst) {
                    // A start clears the window, which is what makes
                    // stop-and-start the way back out of a stalled cast.
                    in_flight.store(0, Ordering::SeqCst);
                    cast(
                        Arc::clone(&outbound),
                        Arc::clone(&casting),
                        Arc::clone(&in_flight),
                        session.clone(),
                        frame_ms,
                        still,
                    );
                }
            }
            "Page.stopScreencast" => {
                casting.store(false, Ordering::SeqCst);
            }
            "Page.screencastFrameAck" => {
                let _ = in_flight.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    Some(n.saturating_sub(1))
                });
            }
            _ => {}
        }

        if mode == "crash-renderer" && method == "Page.navigate" {
            write_frame(
                &outbound,
                &json!({
                    "method": "Inspector.targetCrashed",
                    "sessionId": session,
                    "params": {}
                }),
            );
        }
        if mode == "die-after-first" && answered == 1 {
            // Out, with whatever the caller sends next unanswered.
            return;
        }
    }
}

/// What this fixture answers for one method.
fn answer(method: &str, params: &Value, fds: &str) -> Result<Value, String> {
    match method {
        "Browser.getVersion" => Ok(json!({
            "protocolVersion": "1.3",
            "product": "cdp_browser_fixture/1.0",
            "userAgent": "cdp_browser_fixture",
            // The measurement, where a test can assert it.
            "fixtureFds": fds,
        })),
        "Target.createBrowserContext" => Ok(json!({
            "browserContextId": format!("ctx-{}", std::process::id()),
        })),
        "Target.createTarget" => Ok(json!({"targetId": format!("tgt-{}", next_serial())})),
        "Target.attachToTarget" => Ok(json!({"sessionId": format!("sess-{}", next_serial())})),
        "Target.closeTarget" | "Target.disposeBrowserContext" => Ok(json!({"success": true})),
        // A real browser answers this, and the cell asks it after every load:
        // `Target.targetInfoChanged` does not carry the settled title
        // (measured against Chromium 153 on 2026-09-19, finding B-G1), so the
        // title is pulled rather than waited for.
        "Target.getTargetInfo" => Ok(json!({"targetInfo": {
            "targetId": params.get("targetId").cloned().unwrap_or_else(|| json!("tgt-0")),
            "type": "page",
            "url": last_url(),
            "title": "A fixture page",
            "attached": true,
        }})),
        "Page.navigate" => {
            let url = params.get("url").and_then(Value::as_str).unwrap_or("");
            if url.starts_with("chrome-error://") {
                Err("net::ERR_NAME_NOT_RESOLVED".to_string())
            } else {
                Ok(json!({"frameId": "frame-1", "loaderId": "loader-1"}))
            }
        }
        "Page.enable"
        | "Page.startScreencast"
        | "Page.stopScreencast"
        | "Page.screencastFrameAck"
        | "Runtime.enable"
        | "Emulation.setDeviceMetricsOverride"
        | "Target.setAutoAttach" => Ok(json!({})),
        "Runtime.evaluate" => Ok(json!({"result": {"type": "string", "value": "text/html"}})),
        // Every input call is echoed back, so a test can read what the cell
        // sent without guessing at a browser's behaviour.
        m if m.starts_with("Input.") => Ok(json!({"echo": {"method": m, "params": params}})),
        other => Err(format!("{other} is not implemented by this fixture")),
    }
}

/// The target this fixture hands out for a session.
///
/// One target per session, and the mapping is the order they were made in:
/// `sess-<n>` was attached to `tgt-<n-1>`, because the cell creates the target
/// and then attaches to it.
fn target_of(session: Option<&str>) -> String {
    let n: u64 = session
        .and_then(|s| s.strip_prefix("sess-"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    format!("tgt-{}", n.saturating_sub(1))
}

/// A reply frame, with the session it belongs to when there is one.
fn reply(id: u64, result: Result<Value, String>, session: Option<&str>) -> Value {
    let mut frame = match result {
        Ok(v) => json!({"id": id, "result": v}),
        Err(e) => json!({"id": id, "error": {"code": -32000, "message": e}}),
    };
    if let Some(s) = session {
        frame["sessionId"] = Value::String(s.to_string());
    }
    frame
}

/// Write one NUL-terminated JSON frame on fd 4.
fn write_frame(out: &Arc<Mutex<std::fs::File>>, frame: &Value) {
    let mut bytes = frame.to_string().into_bytes();
    bytes.push(0);
    let Ok(mut file) = out.lock() else { return };
    let _ = file.write_all(&bytes);
    let _ = file.flush();
}

/// How many frames a cast sends before it waits to be acknowledged.
///
/// Three, measured 2026-09-20 at Chromium 153.0.8010.36 over this pipe: a cast
/// that is never acknowledged produces three frames and then nothing at all.
const IN_FLIGHT: usize = 3;

/// Emit `Page.screencastFrame` events until the cast is stopped.
///
/// The picture is four bytes of a JPEG's own magic, base64-encoded: enough for
/// a test to recognise what came out the other end, and nothing a decoder has
/// to work for.
fn cast(
    out: Arc<Mutex<std::fs::File>>,
    casting: Arc<AtomicBool>,
    in_flight: Arc<AtomicUsize>,
    session: Option<String>,
    frame_ms: u64,
    still: bool,
) {
    std::thread::spawn(move || {
        while casting.load(Ordering::SeqCst) {
            // The flow control, and it is the acknowledgement and nothing
            // else: a cast whose frames nobody answers stands after
            // `IN_FLIGHT` of them (wave G, g7, measured at Chromium 153).
            if in_flight.load(Ordering::SeqCst) >= IN_FLIGHT {
                std::thread::sleep(std::time::Duration::from_millis(2));
                continue;
            }
            in_flight.fetch_add(1, Ordering::SeqCst);
            let n = next_serial();
            write_frame(
                &out,
                &json!({
                    "method": "Page.screencastFrame",
                    "sessionId": session,
                    "params": {
                        // base64 of FF D8 FF E0 — a JPEG's first four bytes.
                        "data": "/9j/4A==",
                        "sessionId": n,
                        "metadata": {
                            "offsetTop": 0,
                            "pageScaleFactor": 1,
                            "deviceWidth": 960,
                            "deviceHeight": 600,
                            "scrollOffsetX": 0,
                            "scrollOffsetY": 120,
                            "timestamp": 1.0,
                        }
                    }
                }),
            );
            if still {
                // The cast stays ON — a second `Page.startScreencast` without
                // a stop is refused by a real browser and produces nothing —
                // but the page has nothing more to show.
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(frame_ms));
        }
    });
}

/// `pipe`, `file`, `tty` or `other` — what a descriptor actually is.
fn kind_of(f: &std::fs::File) -> &'static str {
    match f.metadata() {
        Err(_) => "closed",
        Ok(m) => {
            let t = m.file_type();
            if t.is_fifo() {
                "pipe"
            } else if t.is_char_device() {
                "tty"
            } else if t.is_file() {
                "file"
            } else {
                "other"
            }
        }
    }
}

/// A child that either shares this user namespace or has one of its own.
///
/// The two shapes the cell's post-spawn sandbox check has to tell apart: a
/// renderer in the SAME user namespace as the cell is a browser whose sandbox
/// did not hold, and one in its own is a browser whose sandbox did.
fn spawn_probe_child(own_namespace: bool) {
    // The CDP pipes are closed in the child before it execs. A helper that
    // kept the write end of fd 4 open would hold the pipe open after this
    // process exits, and the cell would wait out its A-timeout instead of
    // reading the browser's death.
    let mut cmd = std::process::Command::new("/bin/sh");
    if own_namespace {
        cmd.arg("-c")
            .arg("exec 3<&- 4>&-; exec unshare --user sleep 600");
    } else {
        cmd.arg("-c").arg("exec 3<&- 4>&-; exec sleep 600");
    }
    let _ = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Where the last `Page.navigate` pointed, so `Target.getTargetInfo` can say
/// it back the way a browser does.
fn url_slot() -> &'static Mutex<String> {
    static URL: std::sync::OnceLock<Mutex<String>> = std::sync::OnceLock::new();
    URL.get_or_init(|| Mutex::new("about:blank".to_string()))
}

/// Remember where the page was sent.
fn set_last_url(url: &str) {
    if let Ok(mut slot) = url_slot().lock() {
        *slot = url.to_string();
    }
}

/// Where the page was sent last.
fn last_url() -> String {
    url_slot()
        .lock()
        .map(|slot| slot.clone())
        .unwrap_or_else(|_| "about:blank".to_string())
}

/// A counter for the ids this fixture hands out.
fn next_serial() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
