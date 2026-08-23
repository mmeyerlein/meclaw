//! A store refusal is not a result — the small-cell sweep (GitHub #343, part 2).
//!
//! Part 1 (`gh343_a_store_refusal_is_not_a_result.rs`) took the four lanes with
//! double-digit dispatch sites. This file takes the rest of the issue's table:
//! the lanes that read `hop.operation` from a store reply in one, two or four
//! places. The exposure is the same and it is older than #331 —
//! `store/output.rs::build_tool_result` has stamped `operation` and
//! `error_code` together on every SQL-level failure since the store existed.
//!
//! Every case below was MEASURED against the shipped `script_inline` before
//! anything was edited:
//!
//! | lane | phase / op | what a refusal did |
//! |---|---|---|
//! | `memory-drain/drain` | `park` / `insert` | probed a ledger the park never reached; the probe then finds no batch and the whole day is dropped in silence |
//! | `memory-drain/drain` | `probe` / `select` | parked the day it had just stored, and said nothing |
//! | `session-keeper/stamp` | `look` / `select` | OPENED A SECOND GENERATION for a channel that already had one open — two sessions, two close batches, one split history |
//! | `session-keeper/close` | `sweep` / `select` | swept nothing and reported nothing; the night looks like a night with no idle channel |
//! | `session-keeper/close` | `seal` / `update` | `rows_affected` is 0 on a refusal, which reads exactly like a lost guard race |
//! | `firewall/screen` | `rules` / `select` | walked on with an EMPTY rule set: no blocklist, no allowlist, no pattern rule — a firewall that FAILS OPEN on a store timeout |
//! | `firewall/screen` | `rate` / `select` | counted zero arrivals and emitted `pass` |
//! | `receptionist/greet` | `look` / `select` | read "no row" as "new channel" and emitted a MUTATION that grows a second agent for a channel that already has one |
//!
//! The fix is the shape #308 established and part 1 reused: `operation` says
//! WHICH op answered, `error_code` says WHETHER it worked, and both are read.
//! Success branches gain `and not err`; a terminal branch ahead of the dispatch
//! chain reports `hop.reject_reason == 'store_refused'` with the store's own
//! code in the free string `hop.store_error` and the refused op in
//! `hop.store_operation`.
//!
//! Every test here drives the SHIPPED `script_inline` of the real template.
//!
//! **Split under GitHub #362.** The issue's table also covered the three small
//! collector copies (`research-assistant`, `coder-pipeline`, `llm-unit`) and
//! `coder-pipeline/taskarchive`. None of those templates is in the export
//! allow-list, so their six cases would be red in the published clone and made
//! this whole file unpublishable. They now live in
//! `gh343_the_private_collectors_read_the_error_code_too.rs`, which stays on the
//! export BLOCKLIST — the `gh235` precedent: split rather than guard, because a
//! skipping proof proves nothing (GitHub #234). Every template read below is a
//! published one, so these twenty-six cases travel.
//!
//! The helper block below is duplicated in the twin — by choice, not by
//! necessity: a shared home exists (`meclaw-testing`, a dev-dependency both
//! halves already import for `code_stdin`), and the duplication follows the
//! sanctioned `gh235` shape instead. Moving the helpers there is a refactor of
//! its own and needs its own sanction.

use std::io::Write;
use std::process::{Command, Stdio};

const DRAIN: &str = "../../templates/memory-drain/drain/config.json";
const KEEPER_STAMP: &str = "../../templates/session-keeper/stamp/config.json";
const KEEPER_CLOSE: &str = "../../templates/session-keeper/close/config.json";
const SCREEN: &str = "../../templates/firewall/screen/config.json";
const GREET: &str = "../../templates/receptionist/greet/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string — the substitution the colony performs at instantiation.
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

fn script_of(config: &str) -> String {
    let raw = std::fs::read_to_string(config).unwrap_or_else(|e| panic!("{config}: {e}"));
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv (GH #279: a single argv string is
/// capped at 128 KiB).
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// The emission AND what the cell said on stderr. A `code` cell's stderr is not
/// swallowed by the substrate — it is recorded as `had_stderr` and persisted as
/// a warn line (GH #44) — so a lane whose only trace of a failure is that line
/// has a trace, and this is where it is checked.
fn emit_and_stderr(script: &str, doc: serde_json::Value) -> (Vec<serde_json::Value>, String) {
    let out = run_script_on_stdin(script, &meclaw_testing::code_stdin(&doc).to_string());
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "the script exited non-zero: {said}");
    let msgs = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (msgs, said)
}

fn emit(script: &str, doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(script, &meclaw_testing::code_stdin(&doc).to_string());
    assert!(
        out.status.success(),
        "the script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// One store reply as the return edge delivers it: the chain's phase in the
/// context, the op in the hop — and, when `error_code` is `Some`, the refusal
/// stamp beside it.
fn store_reply(
    context: serde_json::Value,
    operation: &str,
    error_code: Option<&str>,
    text: &str,
) -> serde_json::Value {
    let mut hop = serde_json::json!({"operation": operation, "rows_affected": 0});
    if let Some(code) = error_code {
        hop["error_code"] = serde_json::json!(code);
    }
    serde_json::json!({
        "header": {"context": context, "hop": hop},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "", "text": text}]
    })
}

fn routes(msgs: &[serde_json::Value]) -> Vec<String> {
    msgs.iter()
        .filter_map(|m| m["header"]["route"].as_str().map(str::to_string))
        .collect()
}

fn phases(msgs: &[serde_json::Value]) -> Vec<String> {
    msgs.iter()
        .filter_map(|m| m["header"]["phase"].as_str().map(str::to_string))
        .collect()
}

/// Everything the emission says, in one string — "the reply names the store's
/// error_code" has to hold over the whole message, not over a field we picked.
fn spoken(msgs: &[serde_json::Value]) -> String {
    serde_json::to_string(msgs).expect("serialise")
}

/// The three things every terminal branch owes: it stops (no further store op),
/// it speaks, and what it speaks names the store's own code.
fn asserts_the_refusal_is_reported(
    msgs: &[serde_json::Value],
    store_route: &str,
    code: &str,
    lane: &str,
) {
    assert!(
        !routes(msgs).iter().any(|r| r == store_route),
        "{lane}: the chain advanced on a refusal — it sent the store another \
         op instead of stopping: {}",
        spoken(msgs)
    );
    assert_eq!(
        routes(msgs),
        vec!["reject".to_string()],
        "{lane}: a refusal has to leave on the reject lane: {}",
        spoken(msgs)
    );
    assert!(
        spoken(msgs).contains(code),
        "{lane}: the reply does not name the store's error_code: {}",
        spoken(msgs)
    );
    assert!(
        spoken(msgs).contains("store_refused"),
        "{lane}: the refusal is not marked `store_refused`: {}",
        spoken(msgs)
    );
}

// ──────────────────────────────────────────────────────── memory-drain/drain

fn drain_context(phase: &str) -> serde_json::Value {
    serde_json::json!({"drain_phase": phase, "session_id": "s1"})
}

#[test]
fn the_drain_does_not_probe_a_ledger_the_park_never_reached() {
    let out = emit(
        &script_of(DRAIN),
        store_reply(
            drain_context("park"),
            "insert",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    asserts_the_refusal_is_reported(&out, "lstore", "query_timeout", "memory-drain/drain");
}

#[test]
fn the_drain_does_not_drop_the_day_on_a_refused_probe() {
    let out = emit(
        &script_of(DRAIN),
        store_reply(
            drain_context("probe"),
            "select",
            Some("invalid_input"),
            "no such column: drained_upto",
        ),
    );
    asserts_the_refusal_is_reported(&out, "lstore", "invalid_input", "memory-drain/drain");
}

#[test]
fn the_drain_still_probes_after_a_clean_park() {
    let out = emit(
        &script_of(DRAIN),
        store_reply(drain_context("park"), "insert", None, ""),
    );
    assert!(
        phases(&out).iter().any(|p| p == "probe"),
        "memory-drain/drain: a parked day must still be probed: {}",
        spoken(&out)
    );
}

// ─────────────────────────────────────────────────────── session-keeper/stamp

fn stamp_look_context() -> serde_json::Value {
    serde_json::json!({"ses_phase": "look", "channel": "tg:4711", "keeper_body": "{}"})
}

#[test]
fn the_stamp_does_not_open_a_second_generation_on_a_refused_lookup() {
    // The worst of this sweep: an empty row list reads as "no open session",
    // and the lazy-reopen branch then mints a SECOND generation for a channel
    // that already has one open. Two sessions, two close batches, one history
    // split down the middle.
    let out = emit(
        &script_of(KEEPER_STAMP),
        store_reply(
            stamp_look_context(),
            "select",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    assert!(
        !phases(&out).iter().any(|p| p == "open"),
        "session-keeper/stamp: it opened a new generation on a refused \
         lookup: {}",
        spoken(&out)
    );
    asserts_the_refusal_is_reported(&out, "kstore", "query_timeout", "session-keeper/stamp");
}

#[test]
fn the_stamp_still_touches_a_session_that_came_back() {
    let rows = serde_json::json!([{
        "channel": "tg:4711", "session_id": "tg:4711-1", "opened_at": "a",
        "last_seen": "a", "closed": 0, "closed_at": "", "audience_set": "[]"
    }])
    .to_string();
    let out = emit(
        &script_of(KEEPER_STAMP),
        store_reply(stamp_look_context(), "select", None, &rows),
    );
    assert!(
        phases(&out).iter().any(|p| p == "touch"),
        "session-keeper/stamp: an answered lookup must still restart the idle \
         clock: {}",
        spoken(&out)
    );
}

// ─────────────────────────────────────────────────────── session-keeper/close

fn close_context(phase: &str) -> serde_json::Value {
    serde_json::json!({
        "ses_phase": phase, "channel": "tg:4711", "keeper_session": "tg:4711-1",
        "keeper_audience": "[]"
    })
}

#[test]
fn the_night_does_not_look_idle_because_the_sweep_was_refused() {
    let out = emit(
        &script_of(KEEPER_CLOSE),
        store_reply(
            close_context("sweep"),
            "select",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    asserts_the_refusal_is_reported(&out, "kstore", "query_timeout", "session-keeper/close");
}

#[test]
fn the_close_does_not_read_a_refused_seal_as_a_lost_race() {
    let out = emit(
        &script_of(KEEPER_CLOSE),
        store_reply(
            close_context("seal"),
            "update",
            Some("invalid_input"),
            "no such column: closed_at",
        ),
    );
    asserts_the_refusal_is_reported(&out, "close", "invalid_input", "session-keeper/close");
}

#[test]
fn the_sweep_still_seals_the_channels_it_found() {
    let rows = serde_json::json!([{
        "channel": "tg:4711", "session_id": "tg:4711-1", "opened_at": "a",
        "last_seen": "a", "closed": 0, "closed_at": "", "audience_set": "[]"
    }])
    .to_string();
    let out = emit(
        &script_of(KEEPER_CLOSE),
        store_reply(close_context("sweep"), "select", None, &rows),
    );
    assert!(
        phases(&out).iter().any(|p| p == "seal"),
        "session-keeper/close: an answered sweep must still seal: {}",
        spoken(&out)
    );
}

// ────────────────────────────────────────────────────────────── firewall/screen

fn screen_context(phase: &str) -> serde_json::Value {
    let body = serde_json::json!({
        "messages": [{"origin": "user", "type": "text", "text": "hello"}]
    })
    .to_string();
    serde_json::json!({
        "fw_phase": phase, "channel": "tg:4711", "user_id": "u1", "fw_body": body,
        "fw_now": "2026-01-01T00:00:00.000000Z"
    })
}

#[test]
fn the_screen_does_not_run_on_an_empty_rule_set_it_never_read() {
    // The security-relevant one: an unanswered rule read left the screen with
    // no blocklist, no allowlist and no pattern rule, and it walked straight
    // on to the rate phase. A firewall that fails OPEN on a store timeout.
    let out = emit(
        &script_of(SCREEN),
        store_reply(
            screen_context("rules"),
            "select",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    assert!(
        !routes(&out).iter().any(|r| r == "pass"),
        "firewall/screen: it passed a turn it never screened: {}",
        spoken(&out)
    );
    asserts_the_refusal_is_reported(&out, "fwstore", "query_timeout", "firewall/screen");
}

#[test]
fn the_screen_does_not_pass_a_turn_whose_budget_it_never_counted() {
    let out = emit(
        &script_of(SCREEN),
        store_reply(
            screen_context("rate"),
            "select",
            Some("invalid_input"),
            "no such table: arrivals",
        ),
    );
    assert!(
        !routes(&out).iter().any(|r| r == "pass"),
        "firewall/screen: it passed a turn whose rate budget it never read: {}",
        spoken(&out)
    );
    asserts_the_refusal_is_reported(&out, "fwstore", "invalid_input", "firewall/screen");
}

#[test]
fn the_screen_still_passes_a_turn_inside_its_budget() {
    let out = emit(
        &script_of(SCREEN),
        store_reply(screen_context("rate"), "select", None, "[]"),
    );
    assert!(
        routes(&out).iter().any(|r| r == "pass"),
        "firewall/screen: an answered rate read must still pass the turn: {}",
        spoken(&out)
    );
}

// ──────────────────────────────────────────────────────── receptionist/greet

fn greet_look_context() -> serde_json::Value {
    serde_json::json!({
        "rec_phase": "look", "rec_channel": "tg:4711", "rec_aud": "[]",
        "rec_body": "{}"
    })
}

#[test]
fn the_reception_does_not_build_a_second_agent_on_a_refused_lookup() {
    // "No row" and "the store did not answer" are not the same sentence. Read
    // as the first, a refusal minted a MUTATION that grows another talky for a
    // channel that already has one.
    let out = emit(
        &script_of(GREET),
        store_reply(
            greet_look_context(),
            "select",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    assert!(
        !routes(&out).iter().any(|r| r == "mutate"),
        "receptionist/greet: it grew a second agent on a refused lookup: {}",
        spoken(&out)
    );
    asserts_the_refusal_is_reported(&out, "rstore", "query_timeout", "receptionist/greet");
}

#[test]
fn the_reception_still_hands_a_known_channel_through() {
    let rows = serde_json::json!([{
        "channel": "tg:4711", "talky_path": "/main/talky-tg_4711",
        "created_at": "2026-01-01T00:00:00.000000Z"
    }])
    .to_string();
    let out = emit(
        &script_of(GREET),
        store_reply(greet_look_context(), "select", None, &rows),
    );
    assert_eq!(
        routes(&out),
        vec!["turn".to_string()],
        "receptionist/greet: a known channel must still go straight through: {}",
        spoken(&out)
    );
}

// ══════════════════════════════════════════════ FIX ROUND 1 — the phase truth
//
// Review finding F1–F3, one class: the terminal branch fired on EVERY store
// reply, including the replies to **fire-and-forget** ops whose emission had
// already left the cell. Three of the four lanes therefore said something that
// was no longer true, and one of them said it on a lane that forbids a second
// message about one turn.
//
// A step is fire-and-forget when its store op rides in the SAME emission as the
// thing that step produced:
//
// | lane | fire-and-forget step | what had already left |
// |---|---|---|
// | `firewall/screen` | `mark` | the `pass` verdict |
// | `memory-drain/drain` | `mark` | the episodes of this delivery |
// | `session-keeper/stamp` | `touch`, `open` | the stamped turn |
// | `receptionist/greet` | `open` | the mutation AND the turn |
//
// `firewall` is the one where telling the truth is not enough — a second
// verdict about one turn contradicts the two-lane contract itself — so there
// the refusal is named to the phases that still owe a verdict. The other three
// keep reporting and say per step what is actually still true.

/// The refusal must not claim the opposite of what happened.
fn asserts_does_not_claim(msgs: &[serde_json::Value], claim: &str, lane: &str) {
    assert!(
        !spoken(msgs).contains(claim),
        "{lane}: the refusal still claims \"{claim}\" although that step had \
         already emitted: {}",
        spoken(msgs)
    );
}

// ─────────────────────────────────────────── F1: the screen's second verdict

#[test]
fn a_refused_arrival_mark_is_not_a_second_verdict() {
    // The `rate` branch emits the arrival insert and the `pass` verdict in ONE
    // multi-send. A refusal of that insert therefore arrives after the parent
    // has already been told what this turn was — and `pass`/`reject` is
    // documented as "exactly one of two lanes". So the refusal is NOT reported
    // here; the turn simply did not spend its rate slot.
    let (out, said) = emit_and_stderr(
        &script_of(SCREEN),
        store_reply(
            screen_context("mark"),
            "insert",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    assert!(
        out.is_empty(),
        "firewall/screen: a refused arrival mark produced a SECOND verdict for \
         a turn that was already passed: {}",
        spoken(&out)
    );
    // Terminal is not the same as invisible. Emitting nothing would otherwise
    // make this the one failure in the cell with no trace at all, and the cost
    // of it is a number an operator can check against.
    assert_eq!(
        said,
        concat!(
            "firewall/screen: arrival mark refused (query_timeout), ",
            "rate window undercounts by one\n"
        ),
        "firewall/screen: a refused arrival mark left no line on stderr"
    );
}

#[test]
fn a_booked_arrival_says_nothing_at_all() {
    // The control for the line above: the warn belongs to the refusal, not to
    // every mark reply. A cell that logs on the happy path teaches an operator
    // to ignore the log.
    let (out, said) = emit_and_stderr(
        &script_of(SCREEN),
        store_reply(screen_context("mark"), "insert", None, ""),
    );
    assert!(
        out.is_empty() && said.is_empty(),
        "firewall/screen: an answered arrival mark must be silent, got {} / {said:?}",
        spoken(&out)
    );
}

#[test]
fn an_answered_arrival_mark_is_still_terminal() {
    // The control for the branch above: the clean mark reply was terminal
    // before #343 and still is. A guard that only stops the refusal would be
    // indistinguishable from one that stops everything.
    let out = emit(
        &script_of(SCREEN),
        store_reply(screen_context("mark"), "insert", None, ""),
    );
    assert!(
        out.is_empty(),
        "firewall/screen: an answered arrival mark must stay terminal: {}",
        spoken(&out)
    );
}

// ──────────────────────────────────── F2: the drain's episodes already left

#[test]
fn a_refused_drain_mark_does_not_claim_the_day_is_undrained() {
    // The `mark` insert rides in the same emission as the episodes it covers.
    // A refusal of it means the turns ARE gone and only the high-water mark is
    // missing — the opposite of "nothing was parked and nothing was marked".
    let out = emit(
        &script_of(DRAIN),
        store_reply(
            drain_context("mark"),
            "insert",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    asserts_the_refusal_is_reported(&out, "lstore", "query_timeout", "memory-drain/drain");
    asserts_does_not_claim(&out, "Nothing was parked", "memory-drain/drain");
    asserts_does_not_claim(&out, "still drains whole", "memory-drain/drain");
    assert!(
        spoken(&out).contains("ALREADY left"),
        "memory-drain/drain: the refusal does not say the episodes had already \
         left: {}",
        spoken(&out)
    );
}

#[test]
fn a_refused_drain_park_still_says_the_batch_can_be_redelivered() {
    // The other half of the same discrimination: at `park` nothing HAS left,
    // and that sentence must survive the fix.
    let out = emit(
        &script_of(DRAIN),
        store_reply(
            drain_context("park"),
            "insert",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    assert!(
        spoken(&out).contains("still drains whole"),
        "memory-drain/drain: a refusal before anything left must still promise \
         the redelivery: {}",
        spoken(&out)
    );
}

// ──────────────────────────── F3: the keeper's and the reception's phase truth

#[test]
fn a_refused_session_write_does_not_claim_the_turn_was_never_stamped() {
    // `touch` and `open` ride in the same emission as the stamped turn. The
    // `open` case is the sharp one: the turn travels with a session id whose
    // row was never written, so the NEXT turn opens yet another generation.
    let script = script_of(KEEPER_STAMP);
    for phase in ["touch", "open"] {
        let out = emit(
            &script,
            store_reply(
                serde_json::json!({
                    "ses_phase": phase, "channel": "tg:4711",
                    "keeper_session": "tg:4711-1"
                }),
                if phase == "open" { "insert" } else { "update" },
                Some("query_timeout"),
                "query exceeded query_timeout_ms",
            ),
        );
        asserts_the_refusal_is_reported(&out, "kstore", "query_timeout", "session-keeper/stamp");
        asserts_does_not_claim(&out, "turn was NOT stamped", "session-keeper/stamp");
        assert!(
            spoken(&out).contains("ALREADY stamped"),
            "session-keeper/stamp: the `{phase}` refusal does not say the turn \
             had already left: {}",
            spoken(&out)
        );
    }
}

#[test]
fn a_refused_lookup_still_says_the_turn_was_not_stamped() {
    // The control: at `look` nothing has left, and that sentence is the true
    // one there.
    let out = emit(
        &script_of(KEEPER_STAMP),
        store_reply(
            stamp_look_context(),
            "select",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    assert!(
        spoken(&out).contains("turn was NOT stamped"),
        "session-keeper/stamp: a refusal before the turn left must still say \
         so: {}",
        spoken(&out)
    );
}

#[test]
fn a_refused_seal_does_not_claim_nothing_was_swept() {
    // The sweep DID find this channel — that is why a seal was attempted at
    // all. Only the seal did not take.
    let out = emit(
        &script_of(KEEPER_CLOSE),
        store_reply(
            close_context("seal"),
            "update",
            Some("invalid_input"),
            "no such column: closed_at",
        ),
    );
    asserts_does_not_claim(&out, "nothing was swept", "session-keeper/close");
    assert!(
        spoken(&out).contains("DID find this channel"),
        "session-keeper/close: the seal refusal does not say the sweep had \
         found the channel: {}",
        spoken(&out)
    );
}

#[test]
fn a_refused_channel_row_does_not_claim_no_agent_was_built() {
    // `greet`'s `open` insert rides in the same emission as the mutation AND
    // the turn. By the time its refusal arrives the agent exists.
    let out = emit(
        &script_of(GREET),
        store_reply(
            serde_json::json!({
                "rec_phase": "open", "rec_channel": "tg:4711", "rec_aud": "[]",
                "rec_body": "{}"
            }),
            "insert",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    asserts_the_refusal_is_reported(&out, "rstore", "query_timeout", "receptionist/greet");
    asserts_does_not_claim(&out, "No agent was built", "receptionist/greet");
    assert!(
        spoken(&out).contains("agent WAS built"),
        "receptionist/greet: the `open` refusal does not say the agent had \
         already been built: {}",
        spoken(&out)
    );
}

#[test]
fn a_refused_channel_lookup_still_says_nothing_was_built() {
    // The control, same shape as the stamp's.
    let out = emit(
        &script_of(GREET),
        store_reply(
            greet_look_context(),
            "select",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    assert!(
        spoken(&out).contains("no agent was built"),
        "receptionist/greet: a refusal before anything was built must still \
         say so: {}",
        spoken(&out)
    );
}

// ───────────────────────────── F4: talky drains the keeper's refusal (ruling)

const TALKY: &str = "../../templates/talky/config.json";
const TALKY_ERRORS: &str = "../../templates/talky/errors/config.json";

/// The composite's own `params.graph`, as the substrate reads it — asked
/// through the REAL router rather than compared as a condition string, the
/// discipline `gh202`/`gh173` established.
fn talky_edges() -> meclaw_colony::edge_table::EdgeTable {
    let raw = std::fs::read_to_string(TALKY).expect("talky config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    let mut table = meclaw_colony::edge_table::EdgeTable::new();
    for e in v["params"]["graph"]["edges"].as_array().expect("edges") {
        table.insert(meclaw_colony::edge_table::Edge {
            id: meclaw_core::Uuid::now_v7(),
            from: meclaw_core::Path::new(e["from"].as_str().expect("from")),
            to: meclaw_core::Path::new(e["to"].as_str().expect("to")),
            condition: e["condition"].as_str().map(|c| {
                meclaw_colony::cel_eval::parse_condition(c).expect("shipped condition parses")
            }),
            modifier: None,
            is_default: false,
        });
    }
    table
}

#[test]
fn talky_drains_the_keepers_refusal_into_its_own_error_lane() {
    // Controller ruling of this round: the keeper's new `reject` had no
    // subscriber inside `talky`, so a colony whose session store stopped
    // answering went quiet and the only trace was a dead letter. `./errors` is
    // talky's ONE error drain and its `error` exit is already declared, so this
    // adds a subscriber, not a lane.
    let mut headers = meclaw_core::Headers::new();
    headers
        .hop
        .insert("route".into(), serde_json::json!("reject"));
    let targets: Vec<String> = meclaw_colony::edge_table::apply_edges(
        &talky_edges(),
        &meclaw_core::Path::new("./session-keeper"),
        &headers,
    )
    .into_iter()
    .map(|d| d.target.as_str().to_string())
    .collect();
    assert_eq!(
        targets,
        vec!["./errors".to_string()],
        "talky: the session-keeper's reject reaches {targets:?} — it must reach \
         the composite's own error drain, or a store that stopped answering is \
         a silent room"
    );
}

#[test]
fn the_talky_error_drain_names_the_store_that_refused() {
    // The other half: `./errors` keys its report on `hop.error_code`, and a
    // keeper refusal carries none — the keeper did not fail, the store it spoke
    // to did. Without reading `hop.store_error` the row would say
    // `source=unknown code=error`, which is a drain that files everything under
    // nothing.
    let out = emit(
        &script_of(TALKY_ERRORS),
        serde_json::json!({
            "header": {"context": {"session_id": "s1"},
                       "hop": {"route": "reject", "reject_reason": "store_refused",
                               "store_error": "query_timeout",
                               "store_operation": "select"}},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "k-reject",
                          "text": "the session store refused the select"}]
        }),
    );
    let said = spoken(&out);
    assert!(
        said.contains("source=session-keeper"),
        "talky/errors: the report does not name the session-keeper: {said}"
    );
    assert!(
        said.contains("code=query_timeout"),
        "talky/errors: the report does not carry the STORE's code: {said}"
    );
}

#[test]
fn the_talky_error_drain_still_names_the_brain() {
    // The control. A source chain that answers every message the same way has
    // stopped discriminating.
    let out = emit(
        &script_of(TALKY_ERRORS),
        serde_json::json!({
            "header": {"context": {"session_id": "s1"},
                       "hop": {"finish_reason": "error", "error_code": "provider_error"}},
            "messages": [{"origin": "assistant", "type": "text", "text": "boom"}]
        }),
    );
    let said = spoken(&out);
    assert!(
        said.contains("source=brain") && said.contains("code=provider_error"),
        "talky/errors: a brain failure must still be filed as one: {said}"
    );
}
