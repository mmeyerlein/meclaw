//! The scenarios of the display hive against the real curator (display-hive.md § 11).
//!
//! The driver is Python and runs `compose.py`; these tests run the driver, so that the
//! pins stand in the nextest filter and every diff under `templates/display` pulls them
//! (`docs/development-rules.md` § 10). One test per scenario id, seven numbers each:
//! `MODEL` is the document against itself (`run_model.py` against `pass.py`), `PURE`
//! the scenario against the verbatim copy of the pass inside the cell, `CURATOR` the
//! scenario through the cell's real entry -- two of them are colony-only and the
//! driver says so itself with a SKIP and a `0/0` -- and four lines about the memory of
//! the resident cell (GH #809): a killed cell rebuilds the same screen after a stroke
//! (`REBUILD`) and after an app's write (`REBUILD-WRITE`), keeps the mirror of what
//! `web` holds (`SNAPSHOT`) and sends one patch per pass with small headers (`HOPS`).
//! Three more tests run the stages that are no scenarios: the boot (`BOOT`), the
//! repair of a refused patch (`REPAIR`) and the silence while nothing is written
//! (`IDLE`).
//!
//! Why one test per id (GH #1046): the driver once ran all 116 scenarios in one test,
//! 213-239 s, and at 24 test threads it hit nextest's four-minute kill. Split, each
//! test takes about 0.7 s and nextest spreads them over the cores;
//! `the_scenario_list_is_the_document` keeps the list of tests equal to the document.
//!
//! The tests SKIP rather than fail where their material is legitimately absent: the
//! scenario tests when the library does not travel (R2b, the guard every tool-bound
//! test in this tree uses), `the_copy_is_the_document` when the private description is
//! not there at all -- a foreign clone and ci have the copies but not their source.

use std::process::Command;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

const SCENARIOS: &str = "templates/display/compose/scenarios";
const DRIVER: &str = "templates/display/compose/scenarios/run_display_scenarios.py";

fn library_ships() -> bool {
    repo(DRIVER).exists()
}

/// The stages one scenario runs through. `BOOT`, `REPAIR` and `IDLE` are no
/// scenarios -- they are cases of their own and have a test each, below.
const SCENARIO_STAGES: [&str; 7] = [
    "MODEL",
    "PURE",
    "CURATOR",
    "REBUILD",
    "REBUILD-WRITE",
    "SNAPSHOT",
    "HOPS",
];

/// One driver run; red when the driver says so. Returns its stdout.
fn drive(args: &[&str]) -> String {
    let out = Command::new("python3")
        .arg(repo(DRIVER))
        .args(args)
        .output()
        .expect("the driver runs");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "the driver failed ({args:?}):\n{stdout}\n{stderr}"
    );
    stdout
}

/// The counter lines of a run: `PURE 1/1` -> `("PURE", 1, 1, skipped)`. The driver
/// writes one counter per stage on a line of its own, counter first; `skipped` says
/// whether a `SKIP` line stood between the previous counter and this one -- the
/// driver's own word that the stage left this scenario out (colony-only, or a stage
/// that does not apply), the one reason a `0/0` may stand.
fn counters(stdout: &str, stages: &[&str]) -> Vec<(String, u32, u32, bool)> {
    let mut out = Vec::new();
    let mut skipped = false;
    for l in stdout.lines() {
        if l.starts_with("SKIP ") {
            skipped = true;
            continue;
        }
        let Some((label, rest)) = l.split_once(' ') else {
            continue;
        };
        if !stages.contains(&label) {
            continue;
        }
        let Some((num, den)) = rest.split_once('/') else {
            // `HOPS largest internal header ...`, `REBUILD <id> equal after ...`,
            // or `MODEL FAIL`: no counter. A missing counter is caught below.
            continue;
        };
        let (Ok(num), Ok(den)) = (num.parse(), den.parse()) else {
            continue;
        };
        out.push((label.to_string(), num, den, skipped));
        skipped = false;
    }
    out
}

/// Every stage printed its counter, full, and a `0/0` only where the driver said
/// SKIP for it. A line that is missing is as red as a line that is not full.
fn full(stdout: &str, stages: &[&str]) {
    let seen = counters(stdout, stages);
    for stage in stages {
        let Some((_, passed, total, skipped)) = seen.iter().find(|c| c.0 == *stage) else {
            panic!("the driver printed no `{stage}` counter:\n{stdout}");
        };
        assert!(
            *total > 0 || *skipped,
            "{stage} ran nothing and the driver named no SKIP:\n{stdout}"
        );
        assert_eq!(passed, total, "{stage} is not full:\n{stdout}");
    }
}

/// One scenario through the seven stages of GH #809. The driver once ran all 116
/// in one process: 213-239 s, the four-minute kill at 24 threads (GH #1046). One
/// test per id is ~0.7 s each and lets nextest spread them over the cores.
fn scenario(id: &str) {
    if !library_ships() {
        println!("SKIP {id}: the library does not travel");
        return;
    }
    let mut args = vec![id];
    args.extend([
        "--model-only",
        "--pure-only",
        "--curator-only",
        "--rebuild-only",
        "--rebuild-write-only",
        "--snapshot-only",
        "--hops-only",
    ]);
    let stdout = drive(&args);
    // The MODEL counter must count exactly this one scenario: an id the document
    // does not know would otherwise run nothing and still read full.
    let model = counters(&stdout, &["MODEL"]);
    assert!(
        matches!(model.first(), Some((_, 1, 1, _))),
        "MODEL did not run {id} exactly once:\n{stdout}"
    );
    full(&stdout, &SCENARIO_STAGES);
}

/// The cases of a stage that is no scenario (`--boot-only` and its two siblings).
fn single(flag: &str, stage: &str) {
    if !library_ships() {
        println!("SKIP {stage}: the library does not travel");
        return;
    }
    full(&drive(&[flag]), &[stage]);
}

#[test]
fn the_boot_stage() {
    single("--boot-only", "BOOT");
}

#[test]
fn the_repair_stage() {
    single("--repair-only", "REPAIR");
}

#[test]
fn the_idle_stage() {
    single("--idle-only", "IDLE");
}

/// One `#[test]` per scenario id, and the list of ids as `IDS` for the drift lock.
macro_rules! scenarios {
    ($($name:ident => $id:literal,)*) => {
        const IDS: &[&str] = &[$($id),*];
        $(
            #[test]
            fn $name() {
                scenario($id);
            }
        )*
    };
}

// The ids of `scenarios.json`, in its order. A new scenario there is red in
// `the_scenario_list_is_the_document` until it has its line here.
scenarios! {
    scenario_q_01 => "Q-01",
    scenario_q_02 => "Q-02",
    scenario_q_03 => "Q-03",
    scenario_q_04 => "Q-04",
    scenario_q_05 => "Q-05",
    scenario_q_06 => "Q-06",
    scenario_q_07 => "Q-07",
    scenario_q_08 => "Q-08",
    scenario_q_09 => "Q-09",
    scenario_q_10 => "Q-10",
    scenario_q_11 => "Q-11",
    scenario_q_12 => "Q-12",
    scenario_q_13 => "Q-13",
    scenario_q_14 => "Q-14",
    scenario_q_15 => "Q-15",
    scenario_q_16 => "Q-16",
    scenario_q_17 => "Q-17",
    scenario_q_18 => "Q-18",
    scenario_q_19 => "Q-19",
    scenario_q_20 => "Q-20",
    scenario_s_001 => "S-001",
    scenario_s_002 => "S-002",
    scenario_s_003 => "S-003",
    scenario_s_004 => "S-004",
    scenario_s_005 => "S-005",
    scenario_s_006 => "S-006",
    scenario_s_007 => "S-007",
    scenario_s_008 => "S-008",
    scenario_s_009 => "S-009",
    scenario_s_010 => "S-010",
    scenario_s_011 => "S-011",
    scenario_s_012 => "S-012",
    scenario_s_013 => "S-013",
    scenario_s_014 => "S-014",
    scenario_s_015 => "S-015",
    scenario_s_016 => "S-016",
    scenario_s_017 => "S-017",
    scenario_s_018 => "S-018",
    scenario_s_019 => "S-019",
    scenario_s_020 => "S-020",
    scenario_s_021 => "S-021",
    scenario_s_022 => "S-022",
    scenario_s_023 => "S-023",
    scenario_s_024 => "S-024",
    scenario_s_025 => "S-025",
    scenario_s_026 => "S-026",
    scenario_s_027 => "S-027",
    scenario_s_028 => "S-028",
    scenario_s_029 => "S-029",
    scenario_s_030 => "S-030",
    scenario_s_031 => "S-031",
    scenario_s_032 => "S-032",
    scenario_s_033 => "S-033",
    scenario_s_034 => "S-034",
    scenario_s_035 => "S-035",
    scenario_s_036 => "S-036",
    scenario_s_037 => "S-037",
    scenario_s_038 => "S-038",
    scenario_s_039 => "S-039",
    scenario_s_040 => "S-040",
    scenario_s_041 => "S-041",
    scenario_s_042 => "S-042",
    scenario_s_043 => "S-043",
    scenario_s_044 => "S-044",
    scenario_s_045 => "S-045",
    scenario_s_046 => "S-046",
    scenario_s_047 => "S-047",
    scenario_s_048 => "S-048",
    scenario_s_049 => "S-049",
    scenario_s_050 => "S-050",
    scenario_s_051 => "S-051",
    scenario_s_052 => "S-052",
    scenario_s_053 => "S-053",
    scenario_s_054 => "S-054",
    scenario_s_055 => "S-055",
    scenario_s_056 => "S-056",
    scenario_s_057 => "S-057",
    scenario_s_058 => "S-058",
    scenario_s_059 => "S-059",
    scenario_s_060 => "S-060",
    scenario_s_061 => "S-061",
    scenario_s_062 => "S-062",
    scenario_s_063 => "S-063",
    scenario_s_064 => "S-064",
    scenario_s_065 => "S-065",
    scenario_s_066 => "S-066",
    scenario_s_067 => "S-067",
    scenario_s_068 => "S-068",
    scenario_s_069 => "S-069",
    scenario_s_070 => "S-070",
    scenario_s_071 => "S-071",
    scenario_s_072 => "S-072",
    scenario_s_073 => "S-073",
    scenario_s_074 => "S-074",
    scenario_s_075 => "S-075",
    scenario_s_076 => "S-076",
    scenario_s_077 => "S-077",
    scenario_s_078 => "S-078",
    scenario_s_079 => "S-079",
    scenario_s_080 => "S-080",
    scenario_s_081 => "S-081",
    scenario_s_082 => "S-082",
    scenario_s_083 => "S-083",
    scenario_s_084 => "S-084",
    scenario_s_085 => "S-085",
    scenario_s_086 => "S-086",
    scenario_s_087 => "S-087",
    scenario_s_088 => "S-088",
    scenario_s_089 => "S-089",
    scenario_s_090 => "S-090",
    scenario_s_091 => "S-091",
    scenario_s_092 => "S-092",
    scenario_s_093 => "S-093",
    scenario_s_094 => "S-094",
    scenario_s_095 => "S-095",
    scenario_s_096 => "S-096",
}

/// The drift lock of the split (GH #1046): the tests above are the scenarios of the
/// document, no more and no fewer. A scenario without a test would never run.
#[test]
fn the_scenario_list_is_the_document() {
    if !library_ships() {
        println!("SKIP the_scenario_list_is_the_document: the library does not travel");
        return;
    }
    let path = repo(&format!("{SCENARIOS}/scenarios.json"));
    let doc: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .expect("scenarios.json is JSON");
    let there: Vec<&str> = doc["scenarios"]
        .as_array()
        .expect("scenarios.json has a `scenarios` list")
        .iter()
        .map(|s| s["id"].as_str().expect("every scenario has an id"))
        .collect();
    let missing: Vec<&&str> = there.iter().filter(|id| !IDS.contains(id)).collect();
    let extra: Vec<&&str> = IDS.iter().filter(|id| !there.contains(id)).collect();
    assert!(
        missing.is_empty() && extra.is_empty() && there.len() == IDS.len(),
        "the scenario tests are not the document: missing {missing:?}, extra {extra:?} \
         ({} in scenarios.json, {} tests) -- add or remove the line in `scenarios!` of \
         crates/meclaw-cells/tests/710_the_scenarios_run_against_the_curator.rs",
        there.len(),
        IDS.len()
    );
}

/// The drift lock: the copy in the repo is byte for byte the description's model.
///
/// TWO STEPS, because the two readers ask different questions (GH #753).
///
/// A strand asks "is this commit's copy the commit it says it came from" and compares
/// against `SOURCE`, the mark beside the copies: `git show <sha>:./<file>` in the
/// living tree. That answer does not change while the gate runs. The integration and
/// release passes ask "is the copy the description as it stands today" and compare
/// against the working copy, as this lock always did -- that is the question a wave
/// has to answer before it ships, and it is the one the mark cannot answer.
///
/// Splitting them is paid for: on one evening three sessions moved the living
/// description while strand gates ran against it, and six full strand gates went red
/// on this one test -- 5589 gate seconds, none of them about the strand's own work
/// (`plans/welle-p-2026-09-19/befund/01-zeit.md` section 3.2).
///
/// The runner names the mode in `MECLAW_GATE_MODE`; nothing set means a strand, so a
/// test run by hand gets the deterministic half. Without the living tree (ci, a
/// foreign clone) both halves SKIP with their reason on stdout -- the same guard the
/// WebKit laboratory uses.
///
/// ONE more silence, and only one: a tree that does not know the mark's commit yet.
/// A missing or malformed mark is RED, with the same sentence a drift gets -- the
/// mark travels with the copies, so its absence is a defect here, and a SKIP would
/// make deleting it the way around the lock (wave P review M3). The three copies are compared as whole files: they are
/// taken over unchanged, `sync_pass.py` only carries `pass.py` on into `compose.py`
/// and never edits it, so nothing here may differ -- not even a docstring.
#[test]
fn the_copy_is_the_document() {
    let src = std::env::var("MECLAW_DISPLAY_HIVE")
        .unwrap_or_else(|_| format!("{}/projeks/MeClaw/meclaw-next/23-display", home()));
    if !std::path::Path::new(&src).is_dir() || !library_ships() {
        println!("SKIP the_copy_is_the_document: the description tree is not here");
        return;
    }
    let model = format!("{src}/model");
    let mode = std::env::var("MECLAW_GATE_MODE").unwrap_or_default();
    // `<rev>:./<file>` resolves against the working directory inside the repository,
    // so the model's own path never has to be spelled out here.
    let mark = match mode.is_empty() || mode == "strand" {
        false => None, // integration and release ask the working copy
        true => match mark_sha() {
            // A MISSING or unreadable mark is a defect of THIS repository, not a
            // clone that lags behind: the mark travels with the copies, and every
            // tree that has the one has the other. Skipping here would let a strand
            // that bends the copies and deletes the mark stay green in every strand
            // gate -- the SKIP would hide the very drift the lock exists for
            // (wave P review M3).
            None => panic!(
                "{SCENARIOS}/SOURCE names no commit (display-hive.md § 0.7); \
                 run `python3 scripts/display_sync.py` to renew copies and mark"
            ),
            // The one silence a clone may legitimately produce: it was fetched
            // before the description moved, so it cannot resolve the mark at all.
            Some(sha) if !commit_exists(&model, &sha) => {
                println!("SKIP the_copy_is_the_document: the description tree has no {sha} yet");
                return;
            }
            some => some,
        },
    };
    for name in ["scenarios.json", "pass.py", "run_model.py"] {
        // The driver's own name for the model's runner; the other two keep theirs.
        let source_name = if name == "run_model.py" {
            "run.py"
        } else {
            name
        };
        let (there, whence) = match mark.as_deref() {
            Some(sha) => (
                show(&model, sha, source_name),
                format!("the model at {sha} (SOURCE)"),
            ),
            None => (
                std::fs::read(format!("{model}/{source_name}"))
                    .unwrap_or_else(|e| panic!("the source {source_name}: {e}")),
                "the description's working copy".to_string(),
            ),
        };
        let here = std::fs::read(repo(&format!("{SCENARIOS}/{name}")))
            .unwrap_or_else(|e| panic!("the copy {name}: {e}"));
        // `assert!`, not `assert_eq!`: a diff of 169 kB of JSON is no message.
        assert!(
            here == there,
            "{name} differs from {whence} (display-hive.md § 0.7); \
             run `python3 scripts/display_sync.py` to renew copies and mark"
        );
    }
}

/// The commit named by the mark `SOURCE`: one line, `meclaw-next <sha> <date>`.
fn mark_sha() -> Option<String> {
    let text = std::fs::read_to_string(repo(&format!("{SCENARIOS}/SOURCE"))).ok()?;
    let sha = text.split_whitespace().nth(1)?.to_string();
    (sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit())).then_some(sha)
}

/// Whether the living tree already carries that commit. It may not: a clone fetched
/// before the description moved is behind, and being behind is not a drift.
fn commit_exists(model: &str, sha: &str) -> bool {
    Command::new("git")
        .args(["-C", model, "cat-file", "-e", &format!("{sha}^{{commit}}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// One file of the model as that commit holds it.
fn show(model: &str, sha: &str, name: &str) -> Vec<u8> {
    let out = Command::new("git")
        .args(["-C", model, "show", &format!("{sha}:./{name}")])
        .output()
        .unwrap_or_else(|e| panic!("git show {sha}:./{name}: {e}"));
    assert!(
        out.status.success(),
        "git show {sha}:./{name} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}
