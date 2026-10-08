//! `meclaw ask` — send one turn to a running colony and print the answer (GH #623).
//!
//! This is the one client command in the binary. It runs no colony: no root
//! lease, no `colony.db`, no tracing subscriber, no filesystem write. It speaks
//! the two HTTP routes the quickstart already used — `POST /messages` and
//! `GET /colony/trace` — so it adds no contract surface, and a topology that
//! answers a `curl` answers this.
//!
//! The flow is the `jq` pipeline the README carried, in one command:
//!
//! 1. `POST /messages` with the turn; the colony acknowledges 202 `{message_id}`.
//! 2. `GET /colony/trace?trace_id=<message_id>` until a hop of that trace travels
//!    on route `answer` or `error`.
//! 3. Print that hop's first turn, and exit on its route.
//!
//! GH #1099: step 2 does not poll. Each read after the first passes the
//! `log_seq` of the previous reply (`after_seq` + `wait_ms`), and the colony
//! holds it until its log commits past that point -- so the answer is read
//! the moment it is written, not on the next tick of a 500 ms clock (which
//! added up to half a second to every CLI turn and every latency measured
//! through it). Only against a colony too old to answer with `log_seq` does
//! the loop fall back to `LEGACY_POLL_INTERVAL`.

use anyhow::{Context, anyhow};
use serde_json::{Value, json};
use std::time::Duration;

/// Exit code of an answer that arrived on route `answer`.
pub const EXIT_ANSWER: i32 = 0;
/// Exit code of an answer that arrived on route `error`.
pub const EXIT_ERROR: i32 = 1;
/// Exit code of a turn that was not answered before `--timeout`.
pub const EXIT_TIMEOUT: i32 = 2;

/// How often the trace is asked of a colony that does not hold a read for the
/// next commit (no `log_seq` in its reply: a binary before GH #1099), and
/// after a failed read. The pre-#1099 cadence, kept for exactly that case.
const LEGACY_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Deadline of a single HTTP request. `--timeout` bounds the wait for an
/// ANSWER; without this, one silent socket bounds nothing at all and the
/// command outlives its own budget (measured in review: `--timeout 3` hung for
/// 40 s). Ten seconds is generous for a loopback read and short enough that the
/// loop below notices its deadline.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// GH #1099: how long one read may be held by the colony while it waits for
/// its log to move. Derived from [`HTTP_TIMEOUT`]: two seconds under it, so the
/// held read is answered (and the next one asked) before the request's own
/// deadline cuts it.
const TRACE_WAIT: Duration = HTTP_TIMEOUT.saturating_sub(Duration::from_secs(2));

/// Arguments of `meclaw ask`.
#[derive(Debug, Clone, clap::Args)]
pub struct AskArgs {
    /// Address of the running colony's HTTP API, as `host:port`. Mandatory:
    /// a default would be a promise about a topology the substrate never made.
    #[arg(long, value_name = "HOST:PORT")]
    pub api: String,

    /// Colony path of the cell the turn is addressed to, e.g. `/door`.
    /// Mandatory, for the same reason as `--api`.
    #[arg(long, value_name = "CELL_PATH")]
    pub target: String,

    /// The turn itself: one line of text, sent as a `user` turn.
    #[arg(value_name = "TEXT")]
    pub text: String,

    /// Channel this turn belongs to. Default: a fresh `ask-<uuid>`, so two
    /// calls are two conversations unless you say otherwise.
    #[arg(long, value_name = "ID")]
    pub channel: Option<String>,

    /// Seconds to wait for the answer before giving up (exit 2).
    #[arg(long, value_name = "SECS", default_value_t = 120)]
    pub timeout: u64,

    /// Print the answering trace row as JSON instead of its text.
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

/// The two routes an answer travels on, and what each one exits with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// `hop.route == "answer"`.
    Answer,
    /// `hop.route == "error"`.
    Error,
}

impl Verdict {
    /// The exit code this verdict carries.
    pub fn exit_code(self) -> i32 {
        match self {
            Verdict::Answer => EXIT_ANSWER,
            Verdict::Error => EXIT_ERROR,
        }
    }

    /// Reads a verdict off a route name; every other route is not an answer.
    fn from_route(route: &str) -> Option<Self> {
        match route {
            "answer" => Some(Verdict::Answer),
            "error" => Some(Verdict::Error),
            _ => None,
        }
    }
}

/// The first row of a trace that travels on route `answer` or `error`.
///
/// `/colony/trace` answers oldest first, so the first such row is the answer to
/// the turn that opened this trace, not a later one in the same conversation.
/// `headers_json` is a JSON **string** column; a row whose headers do not parse
/// carries no route and is skipped rather than failing the call.
pub fn first_terminal_row(trace: &Value) -> Option<(&Value, Verdict)> {
    trace.get("trace")?.as_array()?.iter().find_map(|row| {
        let headers: Value = serde_json::from_str(row.get("headers_json")?.as_str()?).ok()?;
        let route = headers.get("hop")?.get("route")?.as_str()?;
        Verdict::from_route(route).map(|v| (row, v))
    })
}

/// The text an answering row carries: `body_payload` parsed, first turn's `text`.
///
/// A row whose body is a blob pointer (`body_kind == "blob"`) or whose turn
/// carries no text yields `None` — the caller then says so instead of printing
/// an empty line that reads like an empty answer.
pub fn answer_text(row: &Value) -> Option<String> {
    if row.get("body_kind")?.as_str()? != "inline" {
        return None;
    }
    let body: Value = serde_json::from_str(row.get("body_payload")?.as_str()?).ok()?;
    Some(
        body.get("messages")?
            .as_array()?
            .first()?
            .get("text")?
            .as_str()?
            .to_string(),
    )
}

/// The body `POST /messages` takes for one user turn.
fn turn_body(target: &str, channel: &str, text: &str) -> Value {
    json!({
        "target": target,
        "headers": { "channel": channel },
        "body": { "messages": [{ "origin": "user", "type": "text", "text": text }] }
    })
}

/// What the wait ends with once the budget is spent.
///
/// The distinction is the reader's: a colony that answered nothing in time is a
/// timeout, and a colony that could not be read at all is a transport failure
/// wearing a timeout's clothes. `saw_trace` is what tells them apart -- one
/// successful read proves the address answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineVerdict {
    /// The trace was read and carried no answer yet.
    Timeout,
    /// Not one read succeeded, and the last one failed.
    Transport,
}

/// Which of the two a spent budget was.
pub fn deadline_verdict(saw_trace: bool, read_failed: bool) -> DeadlineVerdict {
    if !saw_trace && read_failed {
        DeadlineVerdict::Transport
    } else {
        DeadlineVerdict::Timeout
    }
}

/// The dead letter of the posted turn itself, as `(error_code, resolved_target)`.
///
/// A mistyped `--target` is accepted with a 202 and dies in the router, so the
/// turn is answered by nobody and the wait would run its full budget over a
/// typo. The dead-letter queue is where that shows, and it is read on every
/// poll rather than once at the end, because the point is not to wait at all.
///
/// The match is on `message_id`, not on `trace_id` (GH #640). A trace is the
/// whole conversation the turn set off, siblings included: "a cell emission
/// that matches no out-edge of its sender ... lands in the DLQ" (overview
/// § Routing errors and the dead-letter queue), so a memory write a lane makes
/// on the side and nobody consumes is an entry carrying this trace id while the
/// answer is still being worked on. Only the entry whose dead-lettered message
/// **is** the posted turn is a verdict on it, and the queue says which one that
/// is: "the entry carries the `trace_id` and the `message_id` of the message
/// that was posted".
///
/// The parent chain is not the test, although it looks like one. Every hop of
/// the trace descends from the posted turn, the side emission included, so a
/// chain that reaches the posted id separates nothing; and the messages it
/// would be walked over are exactly the ones that may have no `message_log` row
/// (a boundary refusal "lives in the dead-letter queue and never in
/// `/colony/trace`"). An entry without a `message_id` -- a row written before
/// the field existed -- is not attributable and is left to the wait.
pub fn dead_letter_of<'a>(letters: &'a Value, message_id: &str) -> Option<(&'a str, &'a str)> {
    letters
        .get("dead_letters")?
        .as_array()?
        .iter()
        .find(|d| d.get("message_id").and_then(Value::as_str) == Some(message_id))
        .map(|d| {
            (
                d.get("error_code").and_then(Value::as_str).unwrap_or("?"),
                d.get("resolved_target")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
            )
        })
}

/// Runs the command and returns the process exit code.
///
/// `Err` is the transport class (R-0908-3): the address does not answer the
/// `POST`, or the colony refuses the turn with a non-2xx. Those are errors, not
/// timeouts, and the caller reports them on stderr and exits 1. **Inside the
/// wait it is different**: the turn is already accepted, and a colony that
/// stutters for one poll (a restarting proxy in front of it, a 503) has not
/// failed the turn. Reads are retried until the budget is spent, and only a
/// wait in which not one read succeeded reports the transport failure.
pub async fn run(args: AskArgs) -> anyhow::Result<i32> {
    if args.api.contains("://") {
        return Err(anyhow!(
            "--api takes a host:port such as 127.0.0.1:7777, not a URL ({})",
            args.api
        ));
    }
    let base = format!("http://{}", args.api);
    let channel = args
        .channel
        .clone()
        .unwrap_or_else(|| format!("ask-{}", uuid::Uuid::now_v7()));
    let client = reqwest::Client::builder()
        .connect_timeout(HTTP_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .build()
        .context("build the HTTP client")?;

    let message_id = post_turn(&client, &base, &args, &channel).await?;

    // Two deadlines, and they are not redundant. The inner one is the budget
    // the reader asked for and the one that decides the verdict; the outer is
    // the backstop that guarantees the command ends even if a read is still in
    // flight when the budget runs out, which is why it is a whole HTTP timeout
    // longer.
    let budget = Duration::from_secs(args.timeout);
    let waiting = wait_for_answer(&client, &base, &message_id, &args, budget);
    match tokio::time::timeout(budget + HTTP_TIMEOUT, waiting).await {
        Ok(outcome) => outcome,
        Err(_) => {
            report_timeout(&base, args.timeout);
            Ok(EXIT_TIMEOUT)
        }
    }
}

/// Reads the trace and the dead letters until one of them speaks or `budget`
/// is spent -- each read after the first held by the colony until its log
/// moves (GH #1099).
async fn wait_for_answer(
    client: &reqwest::Client,
    base: &str,
    message_id: &str,
    args: &AskArgs,
    budget: Duration,
) -> anyhow::Result<i32> {
    let deadline = tokio::time::Instant::now() + budget;
    let mut saw_trace = false;
    let mut last_error: Option<anyhow::Error> = None;
    // The colony's commit counter as of the last reply; `None` = read now.
    let mut after_seq: Option<u64> = None;
    loop {
        let wait = after_seq.map(|seq| {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            (seq, left.min(TRACE_WAIT))
        });
        after_seq = None;
        match read_trace(client, base, message_id, wait).await {
            Ok(trace) => {
                // No `last_error = None` here: it is read only where no read
                // ever succeeded, and that branch cannot be reached once this
                // arm has run.
                saw_trace = true;
                after_seq = log_seq_of(&trace);
                if let Some((row, verdict)) = first_terminal_row(&trace) {
                    print_answer(row, args.json);
                    return Ok(verdict.exit_code());
                }
            }
            Err(e) => last_error = Some(e),
        }
        // A mistyped target is a 202 followed by silence; the queue says so.
        // Only for the posted turn itself, though -- a sibling hop of the same
        // trace that dies is not this turn's fate (GH #640).
        if let Ok(letters) = read_dead_letters(client, base).await
            && let Some((code, target)) = dead_letter_of(&letters, message_id)
        {
            eprintln!(
                "meclaw ask: the turn was dead-lettered as `{code}` at `{target}` -- \
                 check --target against {base}/colony/graph"
            );
            return Ok(EXIT_ERROR);
        }
        if tokio::time::Instant::now() >= deadline {
            return match deadline_verdict(saw_trace, last_error.is_some()) {
                DeadlineVerdict::Transport => {
                    Err(last_error
                        .unwrap_or_else(|| anyhow!("the trace could not be read even once")))
                }
                DeadlineVerdict::Timeout => {
                    report_timeout(base, args.timeout);
                    Ok(EXIT_TIMEOUT)
                }
            };
        }
        if after_seq.is_none() {
            // An old colony (no `log_seq`) or a failed read: nothing to wait
            // on, so the pre-#1099 cadence.
            tokio::time::sleep(LEGACY_POLL_INTERVAL).await;
        }
    }
}

/// GH #1099: the commit counter a reply carries, if the colony speaks it.
pub fn log_seq_of(trace: &Value) -> Option<u64> {
    trace.get("log_seq").and_then(Value::as_u64)
}

/// Prints what an answering row carries: its text, or the row itself.
fn print_answer(row: &Value, as_json: bool) {
    if as_json {
        println!("{row}");
        return;
    }
    match answer_text(row) {
        Some(text) => println!("{text}"),
        None => eprintln!(
            "meclaw ask: the answering hop carries no readable turn; \
             re-run with --json to see it"
        ),
    }
}

/// The one message a spent budget writes.
fn report_timeout(base: &str, secs: u64) {
    eprintln!(
        "meclaw ask: no answer within {secs}s -- the colony may still be working. \
         Watch it at {base}/ui/, or look for a refusal at {base}/colony/dead_letters"
    );
}

/// `POST /messages`, returning the `message_id` the colony acknowledged.
async fn post_turn(
    client: &reqwest::Client,
    base: &str,
    args: &AskArgs,
    channel: &str,
) -> anyhow::Result<String> {
    let url = format!("{base}/messages");
    let resp = client
        .post(&url)
        .json(&turn_body(&args.target, channel, &args.text))
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = resp.status();
    let payload = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("POST {url} answered {status}: {payload}"));
    }
    let accepted: Value = serde_json::from_str(&payload)
        .with_context(|| format!("POST {url} answered {status} with a body that is not JSON"))?;
    accepted
        .get("message_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("POST {url} answered {status} without a message_id: {payload}"))
}

/// `GET /colony/trace` for exactly the turn that was sent.
///
/// `wait` = `(after_seq, max)`: the colony holds the read until its log
/// commits past `after_seq` or `max` has passed (GH #1099).
async fn read_trace(
    client: &reqwest::Client,
    base: &str,
    message_id: &str,
    wait: Option<(u64, Duration)>,
) -> anyhow::Result<Value> {
    let url = format!("{base}/colony/trace");
    let mut query: Vec<(&str, String)> = vec![
        ("trace_id", message_id.to_string()),
        ("limit", "1000".to_string()),
    ];
    if let Some((after_seq, max)) = wait {
        query.push(("after_seq", after_seq.to_string()));
        query.push(("wait_ms", max.as_millis().to_string()));
    }
    let resp = client
        .get(&url)
        .query(&query)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let payload = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("GET {url} answered {status}: {payload}"));
    }
    serde_json::from_str(&payload)
        .with_context(|| format!("GET {url} answered {status} with a body that is not JSON"))
}

/// `GET /colony/dead_letters` — the newest thousand rows of the persisted
/// `dead_letters` table. No mark is sent, so the answer is newest first
/// (GH #794), which is where a turn posted a moment ago sits.
async fn read_dead_letters(client: &reqwest::Client, base: &str) -> anyhow::Result<Value> {
    let url = format!("{base}/colony/dead_letters");
    let resp = client
        .get(&url)
        .query(&[("limit", "1000")])
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let payload = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("GET {url} answered {status}: {payload}"));
    }
    serde_json::from_str(&payload)
        .with_context(|| format!("GET {url} answered {status} with a body that is not JSON"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(route: Option<&str>, text: &str) -> Value {
        let headers = match route {
            Some(r) => json!({ "hop": { "route": r }, "context": {} }),
            None => json!({ "hop": {}, "context": {} }),
        };
        json!({
            "headers_json": headers.to_string(),
            "body_kind": "inline",
            "body_payload": json!({
                "messages": [{ "origin": "assistant", "type": "text", "text": text }]
            }).to_string(),
        })
    }

    /// GH #1099: a reply's `log_seq` is what the next read waits past; a
    /// colony that does not send one gets the legacy cadence.
    #[test]
    fn the_next_read_waits_past_the_log_seq_of_the_last_reply() {
        assert_eq!(log_seq_of(&json!({"trace": [], "log_seq": 41})), Some(41));
        assert_eq!(log_seq_of(&json!({"trace": []})), None);
        assert!(TRACE_WAIT < HTTP_TIMEOUT);
    }

    #[test]
    fn a_trace_without_a_route_has_no_answer_yet() {
        let trace = json!({ "trace": [row(None, "on its way"), row(Some("tool"), "a call")] });
        assert!(first_terminal_row(&trace).is_none());
    }

    #[test]
    fn the_first_answering_row_wins_because_the_trace_is_oldest_first() {
        let trace = json!({
            "trace": [
                row(None, "ingress"),
                row(Some("answer"), "the answer"),
                row(Some("error"), "a later refusal"),
            ]
        });
        let (found, verdict) = first_terminal_row(&trace).expect("an answer");
        assert_eq!(verdict, Verdict::Answer);
        assert_eq!(answer_text(found).as_deref(), Some("the answer"));
        assert_eq!(verdict.exit_code(), 0);
    }

    #[test]
    fn an_error_route_is_an_answer_too_and_exits_one() {
        let trace = json!({ "trace": [row(Some("error"), "no")] });
        let (found, verdict) = first_terminal_row(&trace).expect("a refusal");
        assert_eq!(verdict, Verdict::Error);
        assert_eq!(verdict.exit_code(), 1);
        assert_eq!(answer_text(found).as_deref(), Some("no"));
    }

    #[test]
    fn unparseable_headers_are_skipped_not_fatal() {
        let mut broken = row(Some("answer"), "unreachable");
        broken["headers_json"] = json!("{not json");
        let trace = json!({ "trace": [broken, row(Some("answer"), "the readable one")] });
        let (found, _) = first_terminal_row(&trace).expect("the readable row");
        assert_eq!(answer_text(found).as_deref(), Some("the readable one"));
    }

    #[test]
    fn a_blob_body_has_no_text_to_print() {
        let mut blob = row(Some("answer"), "irrelevant");
        blob["body_kind"] = json!("blob");
        assert_eq!(answer_text(&blob), None);
    }

    #[test]
    fn a_spent_budget_is_a_timeout_once_a_single_read_has_worked() {
        assert_eq!(deadline_verdict(true, false), DeadlineVerdict::Timeout);
        assert_eq!(
            deadline_verdict(true, true),
            DeadlineVerdict::Timeout,
            "a read that failed at the end does not undo the ones that worked"
        );
    }

    #[test]
    fn a_wait_in_which_no_read_ever_worked_is_a_transport_failure() {
        assert_eq!(deadline_verdict(false, true), DeadlineVerdict::Transport);
    }

    #[test]
    fn a_dead_letter_is_found_by_the_message_it_killed() {
        let letters = json!({ "dead_letters": [
            {"trace_id": "other", "message_id": "other",
             "error_code": "unresolved_path", "resolved_target": "/nope"},
            {"trace_id": "mine", "message_id": "mine",
             "error_code": "unresolved_path", "resolved_target": "/dor"},
        ]});
        assert_eq!(
            dead_letter_of(&letters, "mine"),
            Some(("unresolved_path", "/dor"))
        );
        assert_eq!(dead_letter_of(&letters, "unknown"), None);
        assert_eq!(dead_letter_of(&json!({}), "mine"), None);
    }

    #[test]
    fn a_sibling_hop_of_the_same_trace_is_not_this_turn_s_verdict() {
        // GH #640: the trace is the whole conversation the turn set off. A
        // side emission that nobody consumes dies with this trace id on it
        // while the answer is still on its way.
        let letters = json!({ "dead_letters": [
            {"trace_id": "mine", "message_id": "a-later-hop",
             "error_code": "no_route", "resolved_target": "/door"},
        ]});
        assert_eq!(dead_letter_of(&letters, "mine"), None);
    }

    #[test]
    fn an_entry_without_a_message_id_is_not_attributable() {
        // A row written before the field existed. Reading it as this turn's
        // fate is the guess the trace-id match used to make.
        let letters = json!({ "dead_letters": [
            {"trace_id": "mine", "error_code": "no_route", "resolved_target": "/door"},
        ]});
        assert_eq!(dead_letter_of(&letters, "mine"), None);
    }

    #[test]
    fn the_posted_body_is_the_one_the_quickstart_posted() {
        let body = turn_body("/door", "ask-1", "hello");
        assert_eq!(body["target"], "/door");
        assert_eq!(body["headers"]["channel"], "ask-1");
        assert_eq!(body["body"]["messages"][0]["origin"], "user");
        assert_eq!(body["body"]["messages"][0]["type"], "text");
        assert_eq!(body["body"]["messages"][0]["text"], "hello");
    }
}
