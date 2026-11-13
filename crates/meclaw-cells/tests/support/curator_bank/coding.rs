//! The wall `coding` of the bank of constructed windows (GH #1465,
//! `support/curator_bank.rs`): one long task of an agent that reads files,
//! runs tests and looks at diffs, in three parts.
//!
//! What it counts: every step of the task is a round of its own with one
//! large tool result. Two parts are closed and one is running; `topic`
//! marks say so (`start` on the first round of a part, `end` on the last of
//! a closed one), the way `./intake` writes them. The closed parts lie
//! outside the rounds a rebuild keeps raw at least and go to the summary;
//! the running part stays raw: it is not summed up. Raw as a part is not raw
//! result by result (OR-58-68): in the running part every result outside
//! `keep_rounds` is garbage of the class `shrunk_tools`, but one, the
//! command the shell tool gave up on. The person's instructions are spread
//! over all three parts, and one file is read six times.
//!
//! A FAILED TEST RUN IS A RESULT, NO FAILURE OF THE TOOL (OR-58-66). The
//! tool worked and says what the command came to. The wall holds two, each
//! as the shell tool of the set writes a command that ended non-zero
//! (`crates/meclaw-cells/src/bash.rs`): its stdout, then its stderr between
//! the two sentinel lines; the exit code rides in the header, and no
//! `error_code` does. One stands in the running part within `keep_rounds`
//! (`FAILED`) and is raw because the last rounds are; outside `keep_rounds`
//! it would be a stub with its id like any other result, in the running
//! part too (OR-58-68). The other stands in a closed part outside
//! `keep_rounds` (`FAILED_CLOSED`).
//!
//! A TOOL THAT DID NOT WORK says so in the header of its result
//! (`error_code`). The wall holds one (`TIMEOUT`): the shell tool killed the
//! command at its operation timeout -- `bash.rs`, the arm
//! `Err(KillingTimeoutErr::Elapsed)` of `handle`: the text `"command timed
//! out after {:?}"` of `external_timeout`, the header `error_code:
//! "timeout"` (`ERR_TIMEOUT`, through `build_error_body`);
//! `templates/tools/bash/config.json` sets `external_timeout_ms` 600000, and
//! a `Duration` of it prints as `600s`. The header never reaches a turn, so
//! the round travels the lanes as a live one does
//! (`WallBuilder::lane_round`), its code in the slot `tool_errors`.

use super::{Bank, Run, WallBuilder, What, day, filler};
use crate::curator_hive::{Hive, canonical};
use chrono::{DateTime, Duration, Utc};
use meclaw_core::serde_json::json;

pub const SESSION: &str = "task-2026-03-02T08:00";

/// The rounds of the three parts: `(first, last)`, the third is running.
pub const PARTS: [(usize, usize); 3] = [(1, 6), (7, 12), (13, 34)];

/// The names the `topic` marks give the parts.
const NAMES: [&str; 3] = [
    "the tide table parser",
    "the fare rounding",
    "the pier closure report",
];

/// The file that is read six times.
pub const FILE: &str = "src/tide_table.rs";

/// The round of the running part whose test run fails: one of the last
/// rounds, the ones `keep_rounds` of the role keeps whole (OR-58-68).
pub const FAILED: usize = 33;

/// The round of a closed part whose test run fails.
pub const FAILED_CLOSED: usize = 8;

/// The round of the running part, outside `keep_rounds`, in which the shell
/// tool gives up on the command at its timeout.
pub const TIMEOUT: usize = 14;

/// The `error_code` in the header of the result of `TIMEOUT`.
pub const TIMEOUT_CODE: &str = "timeout";

/// The text of the result of `TIMEOUT`, as the shell tool writes it.
pub const TIMEOUT_TEXT: &str = "command timed out after 600s";

/// The rounds in which the person gives an instruction.
pub const INSTRUCTIONS: [usize; 9] = [1, 4, 7, 10, 13, 17, 22, 28, 33];

/// The turn of round `n`.
pub fn turn(n: usize) -> String {
    format!("c{n:02}")
}

fn at_round(n: usize) -> DateTime<Utc> {
    day(0, 8, 0) + Duration::minutes(4 * n as i64)
}

/// The part the round `n` belongs to.
pub fn part_of(n: usize) -> usize {
    PARTS
        .iter()
        .position(|(a, b)| (*a..=*b).contains(&n))
        .expect("a round of the wall")
}

/// The `topic` marks of the three parts.
fn topics(h: &mut Hive) {
    for (i, (first, last)) in PARTS.iter().enumerate() {
        let start = canonical(&json!({"movement": "start", "name": NAMES[i]}));
        h.mark(at_round(*first), SESSION, &turn(*first), "topic", &start);
        if i + 1 < PARTS.len() {
            let end = canonical(&json!({"movement": "end", "name": NAMES[i]}));
            h.mark(
                at_round(*last) + Duration::seconds(30),
                SESSION,
                &turn(*last),
                "topic",
                &end,
            );
        }
    }
}

/// What a failed run of the test `test` reads like, as the shell tool
/// writes it; the keyword of the row stands in the message of the assertion.
fn failed_run(test: &str, key: &str) -> String {
    format!(
        "running 42 tests\n{}\ntest {test} ... FAILED\n\n\
         failures:\n    {test}\n\n\
         test result: FAILED. 41 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n\
         \n##meclaw-stderr-start##\n\
         thread '{test}' panicked at src/pier.rs:88:9:\n\
         assertion `left == right` failed: case {key}\n  left: 6\n right: 7\n\
         error: test failed, to rerun pass `--lib`\n##meclaw-stderr-end##",
        (1..=41)
            .map(|i| format!("test pier::case_{i:02} ... ok"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// Registers the walls of this seat.
pub fn register(bank: &mut Bank) {
    let mut w = WallBuilder::new("coding", "brindle");
    let s = SESSION;
    let last = PARTS[2].1;
    // The role keeps the results of its last three tool rounds whole.
    let kept_whole = last - 2;
    assert!(
        FAILED >= kept_whole && part_of(FAILED) == PARTS.len() - 1,
        "the failed run of the running part lies in the rounds kept whole"
    );
    // The two other layers the statements (b) and (c) of the wall are made
    // for (review bank minor 2): held by an assert, not by the constants
    // alone, so a shift of the parts says the wall no longer tests them.
    assert!(
        part_of(FAILED_CLOSED) < PARTS.len() - 1 && FAILED_CLOSED < kept_whole,
        "the failed run of a closed part lies in a closed part, outside the rounds kept whole"
    );
    assert!(
        part_of(TIMEOUT) == PARTS.len() - 1 && TIMEOUT < kept_whole,
        "the timeout lies in the running part, outside the rounds kept whole"
    );
    for n in 1..=last {
        let t = turn(n);
        let part = part_of(n);
        w.at(at_round(n));
        if INSTRUCTIONS.contains(&n) {
            w.words(
                s,
                &t,
                &format!(
                    "Instruction {n} for {}, {{k}}: keep every public name as it is and \
                     touch nothing outside this part.",
                    NAMES[part]
                ),
            );
        }
        let seed = 300 + n as u64;
        let what = if part < 2 || n >= kept_whole {
            What::Other
        } else {
            What::Junk("shrunk_tools")
        };
        if n == TIMEOUT {
            // The tool itself failed: a round of its own through the lanes,
            // the code of its header in the slot `tool_errors`.
            w.lane_round(
                What::Error,
                s,
                &t,
                &format!("Run the tests of step {n} once more, {{k}}."),
                "bash",
                json!({"command": format!("cargo test step_{n}")}),
                TIMEOUT_CODE,
                |_| TIMEOUT_TEXT.to_string(),
                &format!(
                    "The run of step {n} of {} did not come back, {{k}}.",
                    NAMES[part]
                ),
            );
            continue;
        }
        if n == FAILED || n == FAILED_CLOSED {
            // A result like any other to the hive: no statement of a class.
            let test = if n == FAILED {
                "pier::closure_report_lists_every_pier"
            } else {
                "fare::rounds_half_up_at_the_cent"
            };
            w.tool(
                What::Other,
                s,
                &t,
                "bash",
                json!({"command": format!("cargo test {}", &test[..4])}),
                None,
                |k| failed_run(test, k),
            );
        } else if n % 6 == 1 {
            // The same file, read again: rounds 1, 7, 13, 19, 25, 31.
            w.tool(
                what,
                s,
                &t,
                "file",
                json!({"op": "read", "path": FILE}),
                None,
                |k| {
                    format!(
                        "// {FILE}, 412 lines\n{}\n// end of {FILE}, read under {k}",
                        filler(77, 120)
                    )
                },
            );
        } else if n % 3 == 0 {
            w.tool(
                what,
                s,
                &t,
                "bash",
                json!({"command": "git diff"}),
                None,
                |k| {
                    format!(
                        "diff --git a/{FILE} b/{FILE}\n{}\n+// step {n}, marked {k}",
                        filler(seed, 120)
                    )
                },
            );
        } else if n % 3 == 2 {
            w.tool(
                what,
                s,
                &t,
                "bash",
                json!({"command": format!("cargo test step_{n}")}),
                None,
                |k| {
                    format!(
                        "running 120 tests\n{}\ntest result: ok. 120 passed; 0 failed -- run {k}",
                        filler(seed, 120)
                    )
                },
            );
        } else {
            let path = format!("src/part_{part}/unit_{n}.rs");
            w.tool(
                what,
                s,
                &t,
                "file",
                json!({"op": "read", "path": path}),
                None,
                |k| {
                    format!(
                        "// {path}, 380 lines\n{}\n// read under {k}",
                        filler(seed, 120)
                    )
                },
            );
        }
        w.answer(
            s,
            &t,
            &format!(
                "Step {n} of {} is done, {{k}}. {}",
                NAMES[part],
                filler(seed + 1000, 3)
            ),
        );
    }

    bank.wall(
        w.build(day(0, 11, 0), SESSION)
            .target(40_000)
            .prepare(topics)
            .run(Run::of("coding"))
            .run(Run::of("coding").with("gc", json!("1")))
            .one_role(
                "a long task of file reads, test runs and diffs fits the role made for such a \
                 task and no other; the second run sets gc to 1, so the collection is shown \
                 for the role although its preset stays 0",
            ),
    );
}
