//! D58 followup (GH #1479, contract point 1) -- the curator knows nothing of a
//! person's wellbeing.
//!
//! WHY this file exists: the curator keeps the window of every model and
//! knows no domain (R-58-9 no. 3). Until this strand `./policy` and
//! `./history` carried a rule about the person's wellbeing: the knobs
//! `wellbeing_line`, `wellbeing_min_confidence`, `wellbeing_rows`, a data line
//! `Wellbeing: {...}` at the head of the start window, an entry in the held
//! state `seed_facts:<live>`, a mark `wellbeing` that closed it, a day reader;
//! `./intake` put the class `wellbeing` on the fact mark by name. The rule
//! moved to the hive `assistant` (`support/assistant_followup.rs`). Here:
//! (1) no knob of any curator cell is named after it; (2) no code of a
//! curator script names it -- neither as a whole string literal (the lock of
//! strand target, `LOCKED_NAMES` carries `wellbeing` from this strand on) nor
//! as part of an identifier or a literal (a comment, a docstring and a bare
//! string statement are prose and do not count); (3) a classed fact of today
//! in the ledger puts no `Wellbeing:` line into a start window and books no
//! `wellbeing` mark.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{self as sj, json};

/// Every identifier, attribute, argument, function name and non-prose string
/// literal of the code that CONTAINS the word (case-insensitive).
const PART_READER: &str = r#"
import ast, json, sys
d = json.load(sys.stdin)
word = d["word"].lower()
out = []
for src in d["sources"]:
    tree = ast.parse(src["code"])
    prose = set(id(n.value) for n in ast.walk(tree) if isinstance(n, ast.Expr))
    for n in ast.walk(tree):
        hit = None
        if isinstance(n, ast.Constant) and isinstance(n.value, str) and id(n) not in prose:
            hit = n.value
        elif isinstance(n, ast.Name):
            hit = n.id
        elif isinstance(n, ast.Attribute):
            hit = n.attr
        elif isinstance(n, ast.arg):
            hit = n.arg
        elif isinstance(n, (ast.FunctionDef, ast.ClassDef)):
            hit = n.name
        if hit is not None and word in hit.lower():
            out.append("%s:%d: %r" % (src["where"], getattr(n, "lineno", 0), hit[:80]))
print(json.dumps(out))
"#;

#[test]
fn gh_d58_followup_the_curator_knows_no_wellbeing() {
    if !shipped() {
        return;
    }
    // (1) No knob of a curator cell.
    let mut knobs = Vec::new();
    for cell in CELLS {
        let cfg = cell_config(cell);
        for k in cfg["params"]
            .as_object()
            .map(|m| m.keys())
            .into_iter()
            .flatten()
        {
            if k.to_lowercase().contains(CLASS) {
                knobs.push(format!("{cell}.{k}"));
            }
        }
    }
    assert!(
        knobs.is_empty(),
        "knobs of the curator named after the wellbeing: {knobs:?}"
    );

    // (2) No name in code: the lock, then any part.
    let sources = curator_sources();
    assert!(
        sources
            .iter()
            .any(|(w, _)| w.ends_with("curator/history/config.json")),
        "check of the setup: the history's script is read"
    );
    let hive = read_json(&repo("templates/curator/config.json"));
    let locked = lock_findings(
        &[CLASS],
        &sources,
        &hive["params"]["graph"]["edges"],
        &hive["params"]["contract"],
    );
    assert!(
        locked.is_empty(),
        "{} whole name(s) in the curator:\n  {}",
        locked.len(),
        locked.join("\n  ")
    );
    assert!(
        LOCKED_NAMES.contains(&CLASS),
        "the strand enters `{CLASS}` into the name lock (u-followup.md, Abnahme)"
    );
    let doc = json!({"word": CLASS,
                     "sources": sources.iter().map(|(w, c)| json!({"where": w, "code": c}))
                                       .collect::<Vec<_>>()});
    let out = run_python(PART_READER, &sj::to_string(&doc).expect("json"));
    assert!(
        out.status.success(),
        "the reader runs: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parts: Vec<String> = sj::from_slice(&out.stdout).expect("a list");
    assert!(
        parts.is_empty(),
        "{} part(s) of code naming the wellbeing in the curator:\n  {}",
        parts.len(),
        parts.join("\n  ")
    );

    // (3) A classed fact of today opens nothing in the start window.
    clear_of_midnight();
    let mut h = Hive::new();
    let at = backdated(30);
    h.row(at, "text-1", "t1", "user", &user("I feel unwell"), 1);
    h.mark(
        at + chrono::Duration::seconds(1),
        "text-1",
        "t1",
        "fact",
        &canonical(&json!({"subject": "Ada", "predicate": "has_symptom",
                           "claim": "Ada feels unwell and slept badly.",
                           "source": "person", "stance": "asserted",
                           "confidence": 0.9, CLASS: true})),
    );
    let s = seed(&mut h, "call-2", "start", 0);
    assert_eq!(
        count_in(&s, "Wellbeing:"),
        0,
        "no wellbeing line: {:?}",
        s.messages()
    );
    let booked = h.rows("SELECT value FROM marks WHERE kind = 'wellbeing'");
    assert!(booked.is_empty(), "no wellbeing mark: {booked:?}");
}
