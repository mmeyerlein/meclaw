//! The wall `errors` of the bank of constructed windows (GH #1465,
//! `support/curator_bank.rs`): one written conversation in which every tool
//! the shipped templates answer failed once.
//!
//! What it counts: per tool of the inventory one failed result, written to
//! the byte as the tool writes it, in a round of its own that lies outside
//! `keep_rounds` when the rebuild plans. A failed result is never shortened
//! (GH #1430): the model keeps seeing that a tool failed, in the words of
//! the failure. Behind them stand three old results that did not fail
//! (`shrunk_tools`), so the rebuild has something to collect.
//!
//! THE WAY IN. A shipped tool says that it failed in the header of its
//! result (`error_code`); the text is an error object or plain prose. The
//! header never reaches a turn: the collector names the code per call id in
//! the slot `tool_errors` of the curate (GH #1279). So no failed result of
//! this wall is written into the ledger by the bank. Each travels the lanes
//! as a live round does (`WallBuilder::lane_round`): the call on the tap,
//! the result on `in_curate` with its header code in `tool_errors`, and
//! `./intake` writes the rows. What the rebuild then knows of the failure
//! is what the hive kept of it.
//!
//! THE INVENTORY is the list below, one entry per tool, each with the place
//! its text and its code come from: the code of the cell that answers the
//! tool. No text is shaped after what the policy recognizes as a failure;
//! where the hive loses a failure on the way, the test that reads this wall
//! is red and names the tool.
//!
//! THE COUNTER-PROBE (`UNMARKED`) is the named limit of that promise: a
//! result that tells of a failure in prose alone -- no header code, no
//! error object -- is a result like any other, and an old one is shortened
//! like any other. No list of words makes it a failure.

use super::{Bank, Run, WallBuilder, What, day, filler};
use meclaw_core::serde_json::{Value, json};

const SESSION: &str = "chat-2026-03-02T13:00";

/// One failed result of the inventory.
pub struct Failure {
    /// The tool family this failure belongs to.
    pub family: &'static str,
    /// The name the model calls the tool by.
    pub tool: &'static str,
    /// The arguments of the call that failed.
    pub arguments: Value,
    /// The `error_code` in the header of the result.
    pub code: &'static str,
    /// The text of the result, as the tool wrote it.
    pub text: &'static str,
    /// Where text and code come from.
    pub source: &'static str,
}

/// The tools the shipped templates answer (plan part `core-bank`, addendum
/// F2): the web search, the weather, the three history tools of the
/// curator, the memory's recall, and a file tool.
pub const FAMILIES: [&str; 7] = [
    "web",
    "weather",
    "history_search",
    "history_read",
    "history_outline",
    "memory_recall",
    "file",
];

/// The turn of the counter-probe.
pub const UNMARKED: &str = "e15";

/// The turn of the old result that did not fail and stands next to the
/// counter-probe: what "a result like any other" looks like on a run.
pub const PLAIN: &str = "e20";

/// The inventory: one failed result per tool family. A family without an
/// entry is a gap of the inventory, named by the test.
pub fn inventory() -> Vec<Failure> {
    vec![
        Failure {
            family: "web",
            tool: "web",
            arguments: json!({"op": "search", "query": "ferry strike north pier"}),
            // The search cell answers a failure with `build_error_body`: the
            // code rides in the header, the text is the sentence alone.
            // `external_timeout_ms` is 15000 in the shipped cell, a
            // `Duration` prints as `15s`.
            code: "timeout",
            text: "web_search timed out after 15s",
            source: "crates/meclaw-cells/src/web_search.rs (the timeout arm of `handle`, \
                     `emit_error`), crates/meclaw-cells/src/tool.rs `build_error_body`, \
                     templates/tools/web_search/config.json `external_timeout_ms`",
        },
        Failure {
            family: "weather",
            tool: "weather",
            arguments: json!({"place": "Harbour Town"}),
            // `refuse(code, message)` of the weather cell:
            // `emit(json.dumps({"error": message, "error_code": code}), code)`.
            code: "upstream_error",
            text: "{\"error\": \"the weather service answered HTTP 503\", \
                   \"error_code\": \"upstream_error\"}",
            source: "templates/tools/weather/config.json `script_inline`, `refuse`, `emit` and \
                     `Upstream(\"upstream_error\", \"the weather service answered HTTP %s\")`",
        },
        Failure {
            family: "history_search",
            tool: "history_search",
            arguments: json!({"query": "the blue folder"}),
            // `answer(call_id, name, payload, code)` of the history cell:
            // the payload with `error` and `tool`, keys sorted, no spaces;
            // the same code as `error_code` in the header.
            code: "bad_arguments",
            text: "{\"detail\":\"the arguments are not one JSON object\",\
                   \"error\":\"bad_arguments\",\"tool\":\"history_search\"}",
            source: "templates/curator/history `script_inline`, `refuse(call_id, name, \
                     \"bad_arguments\", ...)` through `answer`",
        },
        Failure {
            family: "history_read",
            tool: "history_read",
            arguments: json!({"id": "#0123456789ab"}),
            code: "not_found",
            text: "{\"detail\":\"no block of this wall has the id #0123456789ab\",\
                   \"error\":\"not_found\",\"tool\":\"history_read\"}",
            source: "templates/curator/history `script_inline`, `refuse(call_id, name, \
                     \"not_found\", \"no block of this wall has the id #\" + h[:12])`",
        },
        Failure {
            family: "history_outline",
            tool: "history_outline",
            arguments: json!({"since": "2026-02-01"}),
            code: "ledger_error",
            text: "{\"detail\":\"the ledger refused the read; try again later\",\
                   \"error\":\"ledger_error\",\"tool\":\"history_outline\"}",
            source: "templates/curator/history `script_inline`, `refuse(call_id, name, \
                     \"ledger_error\", \"the ledger refused the read; try again later\")`",
        },
        Failure {
            family: "memory_recall",
            tool: "memory_recall",
            arguments: json!({"query": "the spare key of the shed"}),
            // The recall cell refuses with `reject_message(reason, detail)`:
            // `hop.reject_reason` and the text `"recall rejected: %s%s"`. The
            // memory's tool cell answers the call with that text under the
            // same reason as `error_code` (`result(call_id, said, reason)`).
            code: "missing_audience",
            text: "recall rejected: missing_audience",
            source: "templates/memory-hive/recall `script_inline` `reject_message`, \
                     templates/memory-hive/tool `script_inline` (the `reject` arm, `result`)",
        },
        Failure {
            family: "file",
            tool: "file_read",
            arguments: json!({"path": "notes/harbour-plan.md"}),
            // The read cell: `fail(S, "not_found", "no such file")` ->
            // `{"ok": false, "error": {...}}` with the `op` of the request;
            // the tools cell of the space writes it as the result's text
            // (`result_text`: keys sorted, the correlation removed) and puts
            // the code of the error into the header (`tool_result`).
            code: "not_found",
            text: "{\"error\": {\"code\": \"not_found\", \"message\": \"no such file\"}, \
                   \"ok\": false, \"op\": \"read\"}",
            source: "templates/file-space/read `script_inline` (`fail`, `finish`, `on_file`), \
                     templates/file-space/tools `script_inline` (`result_text`, `tool_result`)",
        },
    ]
}

/// Registers the walls of this seat.
pub fn register(bank: &mut Bank) {
    let mut w = WallBuilder::new("errors", "vellum");
    let s = SESSION;

    w.at(day(0, 13, 0));
    w.words(s, "e00", "Hello, {k}. A few things to look up today.");
    w.answer(s, "e00", "Hello. Go ahead, {k}.");

    // One failed call per tool, each a round of its own, through the lanes.
    for (i, f) in inventory().into_iter().enumerate() {
        let turn = format!("e{:02}", i + 1);
        w.at(day(0, 13, 5) + chrono::Duration::minutes(3 * i as i64));
        let text = f.text;
        w.lane_round(
            What::Error,
            s,
            &turn,
            &format!("Next, {{k}}: please use {} for me.", f.tool),
            f.tool,
            f.arguments,
            f.code,
            |_| text.to_string(),
            &format!(
                "That did not work, {{k}}: {} gave me nothing I can use.",
                f.tool
            ),
        );
    }

    // The counter-probe: the same way in, a failure told in prose alone --
    // no code in the header, no error object. It is counted as what it is
    // to the hive: an old result.
    w.at(day(0, 13, 50));
    w.lane_round(
        What::Junk("shrunk_tools"),
        s,
        UNMARKED,
        "And the folder at the harbour office, {k}: can you look into it?",
        "web",
        json!({"op": "fetch", "url": "https://harbour-office.example/archive/folder-12"}),
        "",
        |k| {
            format!(
                "The archive of the harbour office could not be opened this afternoon because \
                 the reading room is closed for the yearly inventory and nobody is there to \
                 fetch a folder from the shelves. Reference {k}."
            )
        },
        "The archive is closed for its inventory today, {k}.",
    );

    // Three old results that did not fail.
    for i in 0..3_i64 {
        let turn = format!("e{:02}", 20 + i);
        w.at(day(0, 14, 0) + chrono::Duration::minutes(5 * i));
        w.words(s, &turn, &format!("Then the timetable of line {i}, {{k}}."));
        w.tool(
            What::Junk("shrunk_tools"),
            s,
            &turn,
            "web",
            json!({"op": "search", "query": format!("timetable line {i}")}),
            None,
            |k| {
                format!(
                    "Search result on the timetable of line {i}: {} Reference {k}.",
                    filler(60 + i as u64, 80)
                )
            },
        );
        w.answer(
            s,
            &turn,
            &format!("Line {i} runs every twenty minutes, {{k}}."),
        );
    }

    // The youngest tool round: within `keep_rounds`.
    w.at(day(0, 14, 30));
    w.words(s, "e30", "And today's weather, {k}?");
    w.tool(
        What::Other,
        s,
        "e30",
        "weather",
        json!({"place": "Harbour Town"}),
        None,
        |k| format!("Harbour Town, now: 4.0 degrees, light rain from the west. Reference {k}."),
    );
    w.answer(s, "e30", "Four degrees and light rain, {k}.");

    bank.wall(
        w.build(day(0, 15, 0), SESSION)
            .target(3600)
            .run(Run::of("talky"))
            .run(Run::of("quick"))
            .run(Run::of("quick").with("gc", json!("1"))),
    );
}
