//! GH #963 -- the presenter's own topics, as tables over the pure functions of `stage`.
//!
//! The driver is `templates/presenter/stage/check_stage.py` (the same one GH #959 runs);
//! this lock names the tables of presenter 1.1 so none of them can vanish silently:
//! OBSERVE (a tool call or result is data -- it opens no window and asks the decider
//! nothing; the result pairs with its observed call; a call belongs to the turn its
//! `turn_id` names or the newest open turn of its round; the work view keeps at most 50
//! rows per session), SEARCH (links only http(s); a sure verdict opens the window and the
//! result fills it; hits of another round never), WORK (steps, files and the test verdict,
//! never a file's content or a command's output), SOURCE (a declared source only in the
//! presenter's own topics, read after the verdict in the screen's round, none without
//! one), REGISTRY (the decider's announcement entry for the model registry, the lanes the
//! app block observes dock at `stage`, and the recipe renders them).
//!
//! SKIPs where the template does not travel (R2b, the guard of every tool-bound test).

use std::process::Command;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const DRIVER: &str = "templates/presenter/stage/check_stage.py";

#[test]
fn the_observed_topics_hold() {
    if !repo(DRIVER).exists() {
        println!("SKIP the_observed_topics_hold: templates/presenter does not travel");
        return;
    }
    let out = Command::new("python3")
        .arg(repo(DRIVER))
        .output()
        .expect("python3 runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the stage tables are not green:\n{stdout}\n{stderr}"
    );
    for table in ["OBSERVE", "SEARCH", "WORK", "SOURCE", "REGISTRY"] {
        let line = stdout
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{table} ")))
            .unwrap_or_else(|| panic!("the table {table} did not run:\n{stdout}"));
        let (ok, n) = line.split_once('/').expect("a counter");
        assert_eq!(ok, n, "{table} {line}");
        assert!(
            n.parse::<u32>().unwrap_or(0) > 5,
            "{table} ran almost nothing: {line}"
        );
    }
}
