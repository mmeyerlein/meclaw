//! GH #1532 (wave d58-3, strand raten) -- the memory hive counts at the
//! model's catalog row, not at fixed figures.
//!
//! Before, `./recall`, `./tool` and `./writer` of the memory hive sized and
//! estimated with a fixed three characters a token (`content-budget v1`) and a
//! fixed 1024 tokens an inline image, whatever model the window belonged to.
//! The llm cell counts a request at the row's `chars_per_token` and
//! `tokens_per_image` (`window.rs` `estimate_prompt_tokens`), so a bundle of
//! 5 % of a window was weighed in other tokens than the window it fills. Now:
//!
//! 1. every budget takes the rate of the model whose window it sizes -- the
//!    request's `chars_per_token` beside its `input_soft` (`content-budget v2`);
//! 2. the token estimate counts at the same rate and an inline image at the
//!    row's `tokens_per_image`;
//! 3. a request that names neither counts exactly as before: three
//!    characters a token, 1024 an image.

use meclaw_core::serde_json::{self, Value, json};
use std::path::Path;

fn script(cell: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates")
        .join(cell)
        .join("config.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{cell}: {e}"));
    let v: Value = serde_json::from_str(&raw).expect("config json");
    v["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// Run the shipped `cell` on one request whose header carries `context` and
/// `hop` and an empty body, then evaluate `program` in the cell's globals.
fn probe(cell: &str, context: Value, hop: Value, program: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_real = {}\n",
            "_sink, _out = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_real, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _out\n",
            "{}"
        ),
        serde_json::to_string(&script(cell)).unwrap(),
        program
    );
    let stdin = json!({
        "envelope": {"header": {"context": context, "hop": hop}},
        "body": {},
        "params": {}
    })
    .to_string();
    let out = meclaw_testing::run_shipped_script(&src, &stdin);
    assert!(
        out.status.success(),
        "{cell} stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Eight bytes of text; the object `{'ab': 'xyzxyz'}` (eight bytes of key
/// and value); one inline image under a one-byte key.
const ESTIMATES: &str = "print([tokens_of_bytes(8), estimate_tokens({'ab': 'xyzxyz'}), \
     estimate_tokens({'u': 'data:image/png;base64,AAAA'})])";

const ESTIMATING: [&str; 2] = ["memory-hive/recall", "memory-hive/writer"];

#[test]
fn the_estimate_counts_at_the_rows_rate() {
    for cell in ESTIMATING {
        let got = probe(cell, json!({"chars_per_token": 4}), json!({}), ESTIMATES);
        // 8 bytes at 4 a token: 2; the image at the core's 1024 plus the key.
        assert_eq!(got, "[2, 2, 1025]", "{cell}");
        // A context value travels as text as often as as a number.
        let got = probe(cell, json!({"chars_per_token": "4"}), json!({}), ESTIMATES);
        assert_eq!(got, "[2, 2, 1025]", "{cell} (rate as text)");
    }
}

#[test]
fn an_image_counts_the_rows_tokens_per_image() {
    for cell in ESTIMATING {
        let got = probe(
            cell,
            json!({"tokens_per_image": 1600}),
            json!({}),
            ESTIMATES,
        );
        assert_eq!(got, "[3, 3, 1601]", "{cell}");
        let got = probe(
            cell,
            json!({"chars_per_token": 4, "tokens_per_image": "1600"}),
            json!({}),
            ESTIMATES,
        );
        assert_eq!(got, "[2, 2, 1601]", "{cell} (both)");
    }
}

#[test]
fn without_a_row_the_estimate_is_the_old_one() {
    for cell in ESTIMATING {
        for ctx in [
            json!({}),
            json!({"chars_per_token": null, "tokens_per_image": null}),
            json!({"chars_per_token": "", "tokens_per_image": ""}),
            json!({"chars_per_token": 0, "tokens_per_image": 0}),
            json!({"chars_per_token": "four", "tokens_per_image": "many"}),
            json!({"chars_per_token": true, "tokens_per_image": true}),
        ] {
            let got = probe(cell, ctx.clone(), json!({}), ESTIMATES);
            assert_eq!(got, "[3, 3, 1025]", "{cell} {ctx}");
        }
    }
}

#[test]
fn the_writer_takes_the_hops_stamp_before_the_context() {
    let got = probe(
        "memory-hive/writer",
        json!({"chars_per_token": 2, "tokens_per_image": 900}),
        json!({"chars_per_token": 4, "tokens_per_image": 1600}),
        ESTIMATES,
    );
    assert_eq!(got, "[2, 2, 1601]");
}

#[test]
fn the_tool_result_limit_takes_the_rate_beside_the_window() {
    let cell = "memory-hive/tool";
    // A tenth of a 1000-token window: 100 tokens, at 4 characters a token.
    let got = probe(
        cell,
        json!({"input_soft": 1000, "chars_per_token": 4}),
        json!({}),
        "print(LIMIT)",
    );
    assert_eq!(got, "400");
    let got = probe(
        cell,
        json!({"recall_input_soft": "1000", "chars_per_token": "4.5"}),
        json!({}),
        "print(LIMIT)",
    );
    assert_eq!(got, "450");
    // No rate: the core's three, the limit exactly as before.
    let got = probe(cell, json!({"input_soft": 1000}), json!({}), "print(LIMIT)");
    assert_eq!(got, "300");
    // No window: no limit, whatever the rate.
    let got = probe(
        cell,
        json!({"chars_per_token": 4}),
        json!({}),
        "print(LIMIT)",
    );
    assert_eq!(got, "None");
}

#[test]
fn every_budget_of_the_three_cells_names_its_rate() {
    // The v1 block had no rate to take; every call of the v2 block's
    // `budget_chars` passes the rate of the window it sizes.
    for cell in [
        "memory-hive/recall",
        "memory-hive/tool",
        "memory-hive/writer",
    ] {
        let s = script(cell);
        assert!(
            s.contains("# >>> content-budget v2") && !s.contains("# >>> content-budget v1"),
            "{cell}: carries the block that takes the target's rate"
        );
        for (n, line) in s.lines().enumerate() {
            let code = line.split('#').next().unwrap_or("");
            if code.contains("budget_chars(") && !code.trim_start().starts_with("def ") {
                // The request's rate: the context's `chars_per_token`, or
                // ASKED_RATE that `./recall` sets from it once per request.
                assert!(
                    code.contains("chars_per_token") || code.contains("ASKED_RATE"),
                    "{cell}:{}: a budget without the rate: {}",
                    n + 1,
                    line.trim()
                );
            }
        }
    }
}
