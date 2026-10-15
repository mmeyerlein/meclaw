//! The colony lock's verdict over one driver report (`support/colony_line.rs`, the
//! function 710 runs on every line): a compositor that never drew is red, a shot that
//! did not come back judges nothing (TV-B4; its annotation is the driver's receipt,
//! pinned in `scripts/tests/test_display_lab.py`).
//!
//! A unit of its own rather than a seventh test in the lock
//! `710_the_colony_holds_in_both_engines_browser` (`scripts/tests/test_nextest_groups.py`
//! holds that binary at its six browser lines) or a test in 708: 708 reads the browser
//! driver's text, and a `#[path]` module beside a driver name makes `gate_plan.py` take
//! the unit for a browser starter (fail-closed), which moved 708 into the browser
//! station (re-check tv-bn-i M1, fix round I2). Its stem starts with the issue number
//! like its neighbours, so `harness_fold.py` keeps it a binary of its own.
//!
//! The same reason puts the Rust marker over a driver's child here
//! (`support/browser_run.rs`, orchestrator decision tv-bn-lb.2, re-check M1-R/M6-N):
//! its pins start real children, and in 708 they would make it a browser starter.

#[path = "support/browser_run.rs"]
mod browser_run;
#[path = "support/colony_line.rs"]
mod colony_line;

use colony_line::{Line, failures_of};
use meclaw_core::serde_json::json;

/// TV-B4, declared in re-check tv-bn-drv2-fix1 M (it replaced post-xhigh-8 M2): the
/// colony lock's verdict over one report (`support/colony_line.rs`, used by 710) lets a
/// missing evidence shot judge nothing and reads two frames that never came as a red
/// line. Pinned on its behaviour with synthetic reports, because a pin on 710's
/// source let three mutations through that turned a stalled compositor green (re-check
/// tv-bn-i M1: `frames_failed` read as an array, `frames.push` moved into the shot
/// loop, `frames.clear()` before the verdict).
#[test]
fn the_colony_lock_annotates_a_missing_shot_and_reds_a_stalled_compositor() {
    let line = Line {
        run: "L1",
        engine: "chromium",
        exit: "monitor",
    };
    let green = json!({"ok": true, "value": {}});
    // The failing names of a verdict: between "): " and " -- " (`failures_of`'s form).
    let names = |verdict: &str| -> String {
        let after = verdict
            .split_once("): ")
            .expect("a verdict names its run")
            .1;
        after
            .split_once(" -- ")
            .expect("a verdict lists its names")
            .0
            .to_string()
    };

    let stalled = json!({
        "viewport": "1920x1080",
        "checks": {"B-06": green.clone()},
        "frames_failed": {"L1-01": "no frame within 30000 ms"},
    });
    let verdict =
        failures_of(&line, &stalled).expect("a compositor that never drew leaves the line green");
    assert_eq!("frames L1-01", names(&verdict), "{verdict}");

    let lost = json!({
        "viewport": "1920x1080",
        "checks": {"B-06": green.clone()},
        "shots_not_taken": {"L1-01": "no picture within 30000 ms"},
    });
    assert_eq!(
        None,
        failures_of(&line, &lost),
        "a missing evidence shot judges the line (TV-B4: an annotation)"
    );

    let both = json!({
        "viewport": "1920x1080",
        "checks": {"B-06": green.clone(), "B-07": {"ok": false, "value": {}}},
        "shots_not_taken": {"L1-02": "no picture within 30000 ms"},
    });
    let verdict = failures_of(&line, &both).expect("a red proof leaves the line green");
    assert_eq!(
        "B-07",
        names(&verdict),
        "a missing shot stands among the failures: {verdict}"
    );
}

/// welle-testing tv-bn-layout (TV-K5), carried into the shared verdict by the bundle
/// merge tv-bn M: a run the driver ended itself -- its run marker `RUN_HANG_MS`, a
/// `HANG:` naming the proof it hung in -- says so in `failed`, and that sentence is the
/// red, even when the run never reached a single proof (an empty `checks`). Before the
/// merge moved the verdict into `support/colony_line.rs`, 710 carried this branch in its
/// own copy; without it the verdict dies on "measured nothing at all" and the `HANG:`
/// sentence that names the proof is lost.
#[test]
fn the_colony_lock_reds_a_run_the_driver_ended_itself_by_its_own_sentence() {
    let line = Line {
        run: "L2",
        engine: "webkit",
        exit: "phone",
    };
    let hung = json!({
        "viewport": "390x844",
        "checks": {},
        "failed": "HANG: run marker 330000 ms in B-07",
    });
    let verdict = failures_of(&line, &hung).expect("a run that hung leaves the line green");
    assert!(
        verdict.starts_with("L2 (webkit): HANG: run marker 330000 ms in B-07 -- "),
        "the run's own sentence is not the red: {verdict}"
    );
}

/// orchestrator decision tv-bn-lb.2 (re-check M1-R): a driver child past its marker gets
/// SIGTERM first, SIGKILL for it and everything below it a HANG mark later, and the test
/// reports `HANG:` with the last state line and the stderr tail -- never left to the
/// harness kill, which prints nothing. The child says where it stands, hears the
/// SIGTERM, and keeps hanging; the marker is the floor of every hang mark.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_driver_child_past_its_marker_is_stopped_and_named() {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(
        "trap 'echo got-term >&2' TERM; echo \"run-ms $MECLAW_WEBKIT_RUN_MS\" >&2; \
         echo 'stuck in B-36' >&2; echo 'LOG {\"proof\": \"B-36\"}'; \
         tail -f /dev/null & wait; wait",
    );
    let hung = browser_run::run_marked(
        &mut cmd,
        "a driver stuck in a proof",
        meclaw_testing::deadline::FLOOR,
        std::time::Duration::ZERO,
    )
    .await
    .expect_err("a child that never ends comes back as HANG, not as output");
    let line = hung.to_string();
    assert!(
        line.starts_with("HANG: a driver stuck in a proof ran past 10 s"),
        "{line}"
    );
    assert!(
        line.contains("run-ms 10000"),
        "the run marker reaches the driver: {line}"
    );
    assert!(line.contains("stuck in B-36"), "the stderr tail: {line}");
    assert!(
        line.contains("got-term"),
        "SIGTERM comes before SIGKILL: {line}"
    );
    assert_eq!(hung.last_state(), "LOG {\"proof\": \"B-36\"}", "{line}");
    assert!(hung.ended_on_term.is_none(), "{line}");
    assert!(
        hung.killed.len() >= 2,
        "the shell AND the process below it get SIGKILL: {:?}",
        hung.killed
    );
    // re-check B fix2, M3: the list is not the death. Every process of the tree is gone
    // after the SIGKILL -- the `tail` below the shell above all, which a SIGKILL to the
    // shell alone leaves running as an orphan -- and no `kill` failed.
    assert!(
        hung.survivors.is_empty(),
        "processes of the tree outlived the SIGKILL: {:?}\n{line}",
        hung.survivors
    );
    assert!(hung.signal_errors.is_empty(), "{line}");
    assert!(line.contains("alive after it: []"), "{line}");
}

/// Re-check tv-bn-layout B fix3 H1 (bundle N): a process that left the child's parent
/// chain -- a `setsid` daemon, or a grandchild whose parent ended and that now hangs off
/// init -- still holds the child's streams, and the parent-pid tree alone finds only
/// the zombie: `alive after it: []` was an all-clear while the cause of the hang ran
/// on. The marker tags the child's environment and finds every process that carries
/// the tag; the line says that the child itself had ended. The daemon is a finite
/// `sleep` so that a red run leaves no process behind for long.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_process_outside_the_childs_tree_is_found_by_its_tag() {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg("setsid sleep 300 & echo \"LOG daemon $!\"; exit 0");
    let hung = browser_run::run_marked(
        &mut cmd,
        "a driver whose daemon holds its streams",
        meclaw_testing::deadline::FLOOR,
        std::time::Duration::ZERO,
    )
    .await
    .expect_err("the daemon holds stdout open, so the run never reaches its end");
    let line = hung.to_string();
    let daemon: u32 = hung
        .last_state()
        .strip_prefix("LOG daemon ")
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or_else(|| panic!("the child names its daemon: {line}"));
    assert!(
        hung.killed.contains(&daemon),
        "the daemon outside the parent chain gets SIGKILL too ({daemon}): {line}"
    );
    assert!(
        hung.survivors.is_empty(),
        "processes outlived the SIGKILL: {:?}\n{line}",
        hung.survivors
    );
    assert!(
        line.contains("the child had ended; a process outside its tree holds its streams"),
        "{line}"
    );
}

/// Re-check tv-bn M2, M1 (bundle N M3): a scan that cannot look is no all-clear. A
/// daemon that made itself non-dumpable (`PR_SET_DUMPABLE` 0, as a browser's sandbox
/// helpers do) has an `environ` the own user can not read (EACCES), so the tag scan
/// skipped it as "not tagged" and the line said `alive after it: []` while it held
/// the streams. Such a process of the own user, started after the child, is named as
/// `unscanned`; it is not killed (nothing proves it is the run's). Finite, and killed
/// by its pid at the end, so a red run leaves nothing behind for long.
///
/// Re-check tv-bn M3 M1: the pinned case must have HAPPENED. The daemon checks both
/// `prctl` answers (`PR_SET_DUMPABLE` 0 returns 0, `PR_GET_DUMPABLE` says 0) and only
/// then answers `ready` on its own FIFO; the shell ends only after that answer -- an
/// event, not the distance between the FLOOR marker and python's start. A daemon that
/// could not make itself non-dumpable ends the shell with `LOG not non-dumpable`, so
/// the run names no daemon and the test is red. Only `named` counts (the old code
/// killed a READABLE daemon by its tag, which is the other pin), and the line must say
/// that the child had ended while a process outside its tree held its streams.
/// python3 is a premise of this suite (some 200 test files of meclaw-cells start it):
/// without it the test is red, never a silent SKIP.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_daemon_whose_environ_is_unreadable_is_named_unscanned() {
    let python = std::process::Command::new("python3")
        .arg("-c")
        .arg("pass")
        .status();
    assert!(
        python.as_ref().map(|s| s.success()).unwrap_or(false),
        "python3 is a premise of this suite (the non-dumpable daemon): {python:?}"
    );
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(
        r#"f=$(mktemp -u) && mkfifo "$f" || exit 9
setsid python3 -c '
import ctypes, sys, time
libc = ctypes.CDLL(None, use_errno=True)
ok = libc.prctl(4, 0, 0, 0, 0) == 0 and libc.prctl(3, 0, 0, 0, 0) == 0
with open(sys.argv[1], "w") as fifo:
    fifo.write("ready\n" if ok else "failed %d\n" % ctypes.get_errno())
if not ok:
    sys.exit(3)
time.sleep(120)
' "$f" &
pid=$!
read -r answer <"$f"
rm -f "$f"
if [ "$answer" = ready ]; then echo "LOG daemon $pid"; else echo "LOG not non-dumpable: $answer"; fi
exit 0"#,
    );
    let hung = browser_run::run_marked(
        &mut cmd,
        "a driver whose non-dumpable daemon holds its streams",
        meclaw_testing::deadline::FLOOR,
        std::time::Duration::ZERO,
    )
    .await
    .expect_err("the daemon holds stdout open, so the run never reaches its end");
    let line = hung.to_string();
    let daemon: u32 = hung
        .last_state()
        .strip_prefix("LOG daemon ")
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or_else(|| panic!("the child names its non-dumpable daemon: {line}"));
    let _ = std::process::Command::new("kill")
        .arg("-KILL")
        .arg(daemon.to_string())
        .status();
    let named = line
        .split("unscanned (environ unreadable): [")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .map(|list| list.split(", ").any(|p| p == daemon.to_string()))
        .unwrap_or(false);
    assert!(
        named,
        "a daemon whose environ the scan could not read is not named unscanned \
         ({daemon}): {line}"
    );
    assert!(
        line.contains("the child had ended; a process outside its tree holds its streams"),
        "{line}"
    );
}

/// The SIGTERM path: a driver that hears it ends on it (`stopped by SIGTERM`, its report
/// printed), and the HANG line says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_driver_child_that_ends_on_sigterm_is_named_so() {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(
        "trap 'echo \"{\\\"failed\\\": \\\"stopped by SIGTERM\\\"}\"; exit 143' TERM; \
         tail -f /dev/null & wait",
    );
    let hung = browser_run::run_marked(
        &mut cmd,
        "a driver that hears SIGTERM",
        meclaw_testing::deadline::FLOOR,
        std::time::Duration::ZERO,
    )
    .await
    .expect_err("past its marker");
    assert_eq!(
        hung.ended_on_term.and_then(|s| s.code()),
        Some(143),
        "{hung}"
    );
    assert!(hung.last_state().contains("stopped by SIGTERM"), "{hung}");
    assert!(hung.to_string().contains("it ended on SIGTERM"), "{hung}");
}

/// The other half: a child that ends hands back exactly what `.output()` would -- the
/// status, stdout and stderr, a failing exit included (exit 1 is a failing proof, its
/// report is on stdout either way).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_driver_child_that_ends_hands_back_its_output() {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg("echo \"report $MECLAW_WEBKIT_RUN_MS\"; echo note >&2; exit 1");
    let out = browser_run::run_marked(
        &mut cmd,
        "a driver that ends",
        meclaw_testing::deadline::scaled(meclaw_testing::deadline::HANG),
        std::time::Duration::ZERO,
    )
    .await
    .unwrap_or_else(|h| panic!("{h}"))
    .expect("sh starts");
    let run_ms = meclaw_testing::deadline::scaled(meclaw_testing::deadline::HANG).as_millis();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("report {run_ms}\n")
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "note\n");
}

/// `workshop/tools/wait.mjs` of this tree, from the crate root.
const WAIT: &str = "../../workshop/tools/wait.mjs";

/// re-check tv-bn-layout B fix1, M6-N (TV-Z3): the reading a HANG line carries is bounded
/// itself. A page whose renderer stands answers no `evaluate`, and an unbounded reading
/// hung the HANG path until the run's marker (17 min) without naming the wait. Run on
/// node: a `hang` whose reader never answers still rejects with `HANG:` and says the page
/// did not answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hang_whose_page_never_answers_still_names_the_wait() {
    let wait = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(WAIT);
    if !wait.is_file() {
        eprintln!("SKIP: {WAIT} is not in this tree");
        return;
    }
    let script = format!(
        "import {{ hang }} from {:?}; \
         try {{ await hang('a stalled page', new Promise(() => {{}}), \
           {{ deadline: 1, state: () => new Promise(() => {{}}) }}); \
           console.log('NO HANG'); }} \
         catch (e) {{ console.log(e.message); }}",
        wait.to_string_lossy()
    );
    let mut cmd = tokio::process::Command::new("node");
    cmd.arg("--input-type=module").arg("-e").arg(script);
    let out = match browser_run::run_marked(
        &mut cmd,
        "node over wait.mjs",
        meclaw_testing::deadline::scaled(meclaw_testing::deadline::HANG),
        std::time::Duration::ZERO,
    )
    .await
    .unwrap_or_else(|h| panic!("{h}"))
    {
        Ok(out) => out,
        Err(e) => {
            // A station that books the engines has node; there a missing one is red, not
            // a silent skip of this pin (re-check B fix2, H3).
            assert!(
                std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref() != Ok("1"),
                "no node on a lane that requires the browsers: {e}"
            );
            eprintln!("SKIP: no node on this host: {e}");
            return;
        }
    };
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.starts_with("HANG: a stalled page") && said.contains("the page did not answer within"),
        "a reading that never answers took the HANG line with it (exit {:?}): {said}\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}
