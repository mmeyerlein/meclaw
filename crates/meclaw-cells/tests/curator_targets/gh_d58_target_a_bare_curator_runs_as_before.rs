//! D58 target (GH #1483, contract points 1 and 5) -- a bare curator runs as
//! before.
//!
//! WHY this file exists: a curator in front of cells that never report and
//! with a composite that names no target (a colony without a registry, a
//! hand-built tree) must keep working: it cuts on the values of its
//! instance, exactly the window it cut before the strand, and only marks
//! that it had no target. The window of before is recorded as a fixed
//! expectation under `tests/fixtures/d58_target/bare.json`; the fixed
//! conversation on the corpus clock must give the same system part,
//! messages and tool scope byte for byte, and each of its calls one mark
//! `target_unknown`.
//!
//! Recording: under `MECLAW_D38_C0_BLESS=1` a missing fixture is written from
//! the hive as it stands; an existing one is never overwritten.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`) on the corpus
//! clock; no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{self as sj, json};

const FIXTURE: &str = "crates/meclaw-cells/tests/fixtures/d58_target/bare.json";
const BLESS: &str = "MECLAW_D38_C0_BLESS";

#[test]
fn gh_d58_target_a_bare_curator_runs_as_before() {
    if !shipped() {
        return;
    }
    let path = repo(FIXTURE);
    if std::env::var(BLESS).as_deref() == Ok("1") {
        if !path.is_file() {
            let w = fixed_window(&[], None, |_| {});
            std::fs::create_dir_all(path.parent().expect("a dir")).expect("fixture dir");
            std::fs::write(&path, sj::to_string_pretty(&w).expect("json") + "\n")
                .expect("the fixture is written");
        }
        return;
    }
    assert!(
        path.is_file(),
        "no fixture at {} -- recorded once with {BLESS}=1 before the strand",
        path.display()
    );
    let want = read_json(&path);
    let mut marks = 0;
    let got = fixed_window(&[], None, |_| {});
    for part in ["system", "messages", "tool_scope"] {
        assert_eq!(
            canonical(&want[part]),
            canonical(&got[part]),
            "the bare window's `{part}` is the one of before"
        );
    }
    // The marks: one per call of the conversation (four turns and the fifth
    // words), each saying the target was missing.
    let mut h = Hive::with_prelude(&[], PRELUDE);
    h.set_clock(T0_US);
    for (i, (words, reply)) in TALK.iter().enumerate() {
        let c = say(&mut h, None, &format!("t{i}"), words);
        answer(&mut h, &c, "stop", warm_usage(), json!([said(reply)]));
        marks += 1;
    }
    let found = marks_of_kind(&h, TARGET_MARK);
    assert_eq!(
        found.len(),
        marks,
        "one mark per call: {found:?} {:?}",
        h.stderr
    );
    assert!(
        found.iter().all(|m| m["missing"] == json!("target")),
        "{found:?}"
    );
    assert!(
        h.routed("catalog_ask").is_empty(),
        "no target named, nobody to ask: {:?}",
        h.out
    );
}
