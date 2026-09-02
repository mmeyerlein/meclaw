//! GH #563 -- the export sink's completeness markers land whole, and a marker
//! that cannot be read is named instead of dropped.
//!
//! `templates/member/export-sink` has filed every table part through a
//! neighbour file and a rename since it was written, and wrote both
//! completeness markers -- `<hive>/seed/export_final.json` and the member-level
//! `export_final.json` -- with a plain `open(path, "w")`. That truncates the
//! name a reader watches to zero bytes before it is filled, so a reader
//! arriving in between gets an empty file where a JSON document is promised.
//! Measured rather than suspected: with a sleep between the truncate and the
//! write, a reader of the member-level marker gets `EOF while parsing a value`
//! every time, which is the panic that turned CI run 33494173120 red.
//!
//! The reader side was hardened first (commit 1662924c -- the tests wait on the
//! document they assert on). This file pins the writer side, and the second,
//! quieter half of the same defect: the sink's own `hive_marker()` swallowed
//! `ValueError` and answered `None`, so a peer hive's marker caught mid-write
//! dropped out of `hives[]` without a word and the member-level marker claimed
//! an incompleteness the directory did not have.
//!
//! Three claims:
//!
//! 1. **THE SHIPPED SCRIPT WRITES EVERY DOCUMENT BY RENAME** -- a drift lock on
//!    the script's own shape, so a later edit cannot quietly reintroduce a
//!    truncating write.
//! 2. **A FINISHED WALK LEAVES NO TEMPORARY BEHIND AND TWO PARSEABLE MARKERS**
//!    -- the property a rename buys, measured on disk.
//! 3. **A CORRUPT PEER MARKER IS NAMED, NOT DROPPED** -- it leaves the readable
//!    `hives[]` alone and appears under `unreadable`, with a line on stderr.
//!
//! The shipped `params.script_inline` is run directly through `python3`, the
//! way `meclaw_testing::code_wire` runs every shipped script: the sink's
//! `sandbox.trust` is `restricted` and a restricted profile is fail-closed
//! against the host, so a colony test would measure the machine instead of the
//! cell. The colony half of this sink is
//! `gh447_an_export_lands_as_a_seed_set.rs`, and the boundary itself is pinned
//! in `gh447_the_member_fires_the_close_pass.rs`.

use meclaw_core::serde_json::{Value, from_slice, from_str, json};
use meclaw_testing::code_wire::{code_stdin, run_shipped_script, shipped_script};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SINK: &str = "templates/member/export-sink/config.json";

fn sink_script() -> String {
    shipped_script(repo(SINK).to_str().expect("a utf-8 repo path"))
}

/// One `dump` part as the porter stamps it, ready to be handed to the sink.
fn part(hive: &str, table: &str, final_: bool) -> Value {
    json!({
        "format": "meclaw-memory-export/1",
        "hive_template": hive,
        "export_id": "exp-563",
        "exported_at": "2026-09-02T00:00:00Z",
        "table": table,
        "part": 1,
        "of": 1,
        "final": final_,
        "absent": false,
        "key": ["id"],
        "schema": {"id": "text"},
        "rows": [{"id": "r1"}],
    })
}

/// The flat spelling of the message the drain edge delivers.
fn dump_message(export_dir: &std::path::Path, part: &Value) -> Value {
    json!({
        "header": {"hop": {"route": "dump", "dump_kind": "export_part",
                           "export_final": if part["final"] == json!(true) { "1" } else { "0" }}},
        "messages": [{"origin": "assistant", "type": "text", "text": part.to_string()}],
        "params": {"export_dir": export_dir.to_str().expect("a utf-8 temp path")}
    })
}

fn run(export_dir: &std::path::Path, part: &Value) -> std::process::Output {
    let out = run_shipped_script(
        &sink_script(),
        &code_stdin(&dump_message(export_dir, part)).to_string(),
    );
    assert!(
        out.status.success(),
        "the sink exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn read_json(p: &std::path::Path) -> Value {
    from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
        .unwrap_or_else(|e| panic!("{} is not json: {e}", p.display()))
}

/// Every regular file under `root`, relative paths, sorted.
fn tree(root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable directory") {
            let p = entry.expect("dir entry").path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(
                    p.strip_prefix(root)
                        .expect("under root")
                        .display()
                        .to_string(),
                );
            }
        }
    }
    out.sort();
    out
}

// ------------------------------------------------------------- the drift lock

/// Every document this cell writes is written into a neighbour and renamed over
/// the name a reader watches. One place in the script carries the rule, and
/// nothing writes a marker by truncating it.
#[test]
fn the_shipped_sink_writes_every_marker_through_a_rename() {
    let script = sink_script();

    assert_eq!(
        script.matches("write_whole(").count(),
        4,
        "one definition and three writers -- the table part and both \
         completeness markers. A writer that stopped going through the helper \
         is a writer that truncates a name a reader watches (GH #563)."
    );
    assert_eq!(
        script.matches("os.replace(").count(),
        1,
        "the rename lives in the helper and nowhere else, so there is exactly \
         one place where the rule can be read or broken"
    );
    assert_eq!(
        script.matches("\"w\") as fh").count(),
        1,
        "exactly one truncating open in the whole script, and it is the \
         helper's own temporary"
    );
    assert!(
        !script.contains(r#"MARKER), "w")"#),
        "a completeness marker is never opened for writing under the name a \
         reader watches -- that is the zero-byte window GH #563 measured"
    );
}

// ------------------------------------------------------------- the whole file

/// A finished walk leaves the seed file, both markers, and nothing else.
#[test]
fn a_final_part_leaves_no_temp_file_and_a_parseable_marker() {
    let td = tempfile::TempDir::new().unwrap();
    let export_dir = td.path().join("exports");
    std::fs::create_dir_all(&export_dir).unwrap();

    run(&export_dir, &part("memory-hive", "episodes", true));

    let files = tree(&export_dir);
    assert!(
        files.iter().all(|f| !f.ends_with(".part")),
        "a rename leaves no neighbour behind: {files:?}"
    );
    assert_eq!(
        files,
        vec![
            "export_final.json".to_string(),
            "memory-hive/seed/episodes.jsonl".to_string(),
            "memory-hive/seed/export_final.json".to_string(),
        ],
        "the walk writes exactly three files"
    );

    let hive = read_json(&export_dir.join("memory-hive/seed/export_final.json"));
    assert_eq!(hive["hive"], "memory-hive");
    assert_eq!(hive["export_id"], "exp-563");

    let member = read_json(&export_dir.join("export_final.json"));
    assert_eq!(member["format"], "meclaw-member-export/1");
    assert_eq!(member["hives"], json!(["memory-hive"]));
    assert_eq!(
        member["unreadable"],
        json!([]),
        "nothing was unreadable, and the key says so rather than being absent"
    );
}

// ----------------------------------------------------- the corrupt peer marker

/// A peer hive's marker that is not a JSON object at all is corruption -- every
/// marker is written by rename now, so it can no longer be a write caught
/// halfway. The member-level document names it instead of answering "that hive
/// is not finished", which is the incompleteness it must never claim.
#[test]
fn a_corrupt_hive_marker_is_named_not_dropped() {
    let td = tempfile::TempDir::new().unwrap();
    let export_dir = td.path().join("exports");
    std::fs::create_dir_all(export_dir.join("other/seed")).unwrap();
    std::fs::write(export_dir.join("other/seed/export_final.json"), "{not json").unwrap();

    let out = run(&export_dir, &part("mem", "episodes", true));

    let member = read_json(&export_dir.join("export_final.json"));
    assert_eq!(
        member["hives"],
        json!(["mem"]),
        "the readable list stays the readable ones"
    );
    assert_eq!(
        member["unreadable"],
        json!(["other"]),
        "and the directory whose marker could not be read is NAMED in the \
         document that is about it"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("other"),
        "the refusal is also said out loud, so an operator watching the run \
         sees it: {stderr}"
    );
    // A non-empty stderr on a successful run is exactly what makes the
    // substrate stamp `header.had_stderr = true` on the emission and copy the
    // text into `log.jsonl` at warn level (`meclaw_cells::code::cell`,
    // `cell-types.md` § code). This harness runs the script directly, so it
    // sees the stderr the substrate would read rather than the header the
    // substrate would add; that the line is THERE is the half the script owns.
    assert!(
        !out.stderr.is_empty(),
        "a refusal an operator can only find by diffing two markers is a \
         refusal nobody finds"
    );

    // The emission is unchanged: a corrupt peer is not this walk's failure.
    let emitted: Value = from_slice(&out.stdout).expect("one message on stdout");
    assert_eq!(emitted["header"]["route"], "export_done");
    assert_eq!(emitted["header"]["export_hive"], "mem");
}

/// The other half of the same split, and the one the first cut of this fix got
/// wrong: a marker that EXISTS and cannot be OPENED is not "that hive has not
/// finished" either. Only a file that is not there is that fact. Everything
/// else -- a directory standing where the marker belongs, a permission the
/// export process does not have, an I/O error on the block -- is a marker this
/// cell could not read, and saying "not finished" about it claims the same
/// incompleteness GH #563 is about.
///
/// A DIRECTORY is the deterministic version of that case: `open()` raises
/// `IsADirectoryError`, which is an `OSError` and not a `FileNotFoundError`,
/// on every platform this runs on and without touching a mode bit.
#[test]
fn a_marker_that_exists_and_cannot_be_opened_is_named_too() {
    let td = tempfile::TempDir::new().unwrap();
    let export_dir = td.path().join("exports");
    // `<other>/seed/export_final.json` is a DIRECTORY, not a file.
    std::fs::create_dir_all(export_dir.join("other/seed/export_final.json")).unwrap();

    let out = run(&export_dir, &part("mem", "episodes", true));

    let member = read_json(&export_dir.join("export_final.json"));
    assert_eq!(
        member["hives"],
        json!(["mem"]),
        "the readable list stays the readable ones"
    );
    assert_eq!(
        member["unreadable"],
        json!(["other"]),
        "a marker this cell could not open is NAMED. Answering `None` for it -- \
         the answer a missing file gets -- would let this document claim an \
         incompleteness the directory does not have, which is the whole of GH #563"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("other"),
        "and it is said out loud, the same way a corrupt one is"
    );
}
