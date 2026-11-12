//! D58 live (GH #1482, plan `u-live.md` goal and contract point 4) -- no
//! script of the curator and none of the lead reads a call key or an engine.
//!
//! WHY this file exists: the curator held a running session by the voice's
//! keys -- `call_id` and `engine` read out of hop, context and the lead's
//! ask head in `./history`, `./handover`, `./policy`, `./push` and the
//! lead -- so every model with a session of its own had to look like the
//! voice. The keys are translated at the edges now (`live`); a script that
//! still reads one of them is a table of the voice in disguise.
//!
//! The lock reads the `script_inline` of every cell under
//! `templates/curator/` and under `templates/lead/` (the hive of the lead
//! since strand `lead`, GH #1474), and refuses every whole string literal `"call_id"` or `"engine"` of the code
//! (python's own tokenizer: a comment, a docstring line and a part of a
//! longer literal such as `"tool_call_id"` do not count; a docstring is one
//! literal and never equals a key). In `./history` `call_id` is also the
//! name of a tool call's id -- a variable, never a literal, so not counted.
//! What stays is the column `call_id` of the ledger tables `calls`,
//! `call_blocks` and `snapshots`: the id of a model call, not of a session.
//! Those sites stand in [`KEPT`] beside this test, each by its cell and a
//! piece of its line; every entry must still match (a stale entry is red
//! too), so the list cannot grow quietly.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The literal sites that stay: `(cell, literal, a piece of the line)`. All
/// are the column `call_id` of a ledger table (the id of one model call).
const KEPT: [(&str, &str, &str); 15] = [
    (
        "curator/policy",
        "call_id",
        "upd(\"calls\", a.get(\"use\") or {}, {\"call_id\": call_})",
    ),
    (
        "curator/policy",
        "call_id",
        "\"where\": {\"call_id\": {\"in\": ids}}}),",
    ),
    (
        "curator/policy",
        "call_id",
        "sel(\"calls\", [\"session_id\"], {\"call_id\": call})",
    ),
    (
        "curator/policy",
        "call_id",
        "sel(\"calls\", [\"call_id\"], rebuild_where(the_round)",
    ),
    (
        "curator/policy",
        "call_id",
        "sel(\"snapshots\", [\"seq\", \"call_id\", \"session_id\"",
    ),
    (
        "curator/policy",
        "call_id",
        "\"call_id\": call, \"session_id\": session, \"turn_id\": turn, \"iter\": it,",
    ),
    (
        "curator/policy",
        "call_id",
        "ins(\"call_blocks\", {\"call_id\": call, \"pos\": pos, \"hash\": h})",
    ),
    (
        "curator/policy",
        "call_id",
        "\"seq\": s_seq, \"call_id\": call,",
    ),
    (
        "curator/policy",
        "call_id",
        "\"call_id\": b_head[\"curator_call\"], \"session_id\": session",
    ),
    (
        "curator/policy",
        "call_id",
        // D58 moveout (GH #1476): the snapshot row of each side call, one seq
        // past the call's own per entry.
        "\"seq\": s_seq + 1 + s_i, \"call_id\": b_head[\"curator_call\"],",
    ),
    (
        "curator/policy",
        "call_id",
        "upd(\"calls\", fields, {\"call_id\": call})",
    ),
    (
        "curator/policy",
        "call_id",
        "(rows(lg, \"p-newest\") or [{}])[0].get(\"call_id\")",
    ),
    (
        "curator/policy",
        "call_id",
        "sel(\"call_blocks\", [\"pos\", \"hash\"], {\"call_id\": call},",
    ),
    (
        "curator/policy",
        "call_id",
        "sel(\"calls\", [\"turn_id\"], {\"call_id\": call})",
    ),
    (
        "curator/reader",
        "call_id",
        "SNAP_COLS = [\"seq\", \"call_id\", \"session_id\"",
    ),
];

/// The keys a script may not read: the voice's call and its engine.
const KEYS: [&str; 2] = ["call_id", "engine"];

/// Every whole string literal of `KEYS` in each source, by python's own
/// tokenizer: `[cell, line_no, literal, line]`.
const SCAN: &str = r#"
import ast, io, json, sys, tokenize
inp = json.load(sys.stdin)
out = []
for name, src in inp["sources"]:
    lines = src.splitlines()
    for tok in tokenize.generate_tokens(io.StringIO(src).readline):
        if tok.type != tokenize.STRING:
            continue
        try:
            v = ast.literal_eval(tok.string)
        except Exception:
            continue
        if v in inp["keys"]:
            out.append([name, tok.start[0], v, lines[tok.start[0] - 1].strip()])
print(json.dumps(out))
"#;

/// The scripts the lock reads: `(cell, script)` of every cell under
/// `templates/curator/` and `templates/lead/` that ships one.
fn scripts() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for root in ["curator", "lead"] {
        let dir = repo(&format!("templates/{root}"));
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut configs = vec![(root.to_string(), dir.join("config.json"))];
        for e in entries.flatten() {
            if e.path().is_dir() {
                let cell = e.file_name().to_string_lossy().to_string();
                configs.push((format!("{root}/{cell}"), e.path().join("config.json")));
            }
        }
        for (name, cfg) in configs {
            if !cfg.is_file() {
                continue;
            }
            if let Some(s) = read_json(&cfg)["params"]["script_inline"].as_str() {
                out.push((name, s.to_string()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn gh_d58_live_no_curator_and_no_lead_script_reads_a_call_key_or_an_engine() {
    if !shipped() {
        return;
    }
    let sources = scripts();
    let names: Vec<&str> = sources.iter().map(|(n, _)| n.as_str()).collect();
    for want in [
        "curator/history",
        "curator/handover",
        "curator/policy",
        "curator/push",
    ] {
        assert!(
            names.contains(&want),
            "check of the setup: {want} is read: {names:?}"
        );
    }
    assert!(
        names.contains(&"lead/lead"),
        "check of the setup: the lead's script under templates/lead/ is read: {names:?}"
    );
    let doc = json!({"sources": sources, "keys": KEYS});
    let run = run_python(SCAN, &doc.to_string());
    assert!(
        run.status.success(),
        "the scan does not run: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let found: Vec<Value> = sj::from_slice(&run.stdout).expect("the scan's findings are JSON");
    let mut used = vec![0usize; KEPT.len()];
    let mut refused = Vec::new();
    for f in &found {
        let (cell, line_no, lit, line) = (
            f[0].as_str().unwrap_or(""),
            f[1].as_i64().unwrap_or(0),
            f[2].as_str().unwrap_or(""),
            f[3].as_str().unwrap_or(""),
        );
        match KEPT
            .iter()
            .position(|(c, l, piece)| *c == cell && *l == lit && line.contains(piece))
        {
            Some(i) => used[i] += 1,
            None => refused.push(format!("{cell}:{line_no}: \"{lit}\": {line}")),
        }
    }
    let stale: Vec<String> = KEPT
        .iter()
        .zip(&used)
        .filter(|(_, n)| **n == 0)
        .map(|((c, l, piece), _)| format!("{c}: \"{l}\": {piece}"))
        .collect();
    assert!(
        refused.is_empty(),
        "{} read(s) of a call key or an engine in a curator or lead script:\n  {}",
        refused.len(),
        refused.join("\n  ")
    );
    assert!(
        stale.is_empty(),
        "kept site(s) no script has any more -- take them off the list:\n  {}",
        stale.join("\n  ")
    );
}
