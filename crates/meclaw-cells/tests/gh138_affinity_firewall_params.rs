//! GH #138 -- the record hive's and the screen hive's knobs are params.
//!
//! Until `affinity@3.2.0` and `firewall@2.2.0` ten behaviour knobs of these two
//! templates were `${AFFINITY_*}` / `${FIREWALL_*}` substitution tokens:
//! colony-global by construction, so two members of one colony could not have
//! their record cadence or their screening arithmetic tuned apart, and every
//! knob NAME was a public configuration contract that could only be renamed by
//! breaking a running colony. Ruling R-0904-6 moves them onto the `params`
//! surface of the cells that read them, defaults bit-identical.
//!
//! `crates/meclaw-cells/tests/w13_collector_params.rs` is the pattern (GH #136,
//! `collector@1.2.0`) and `gh138_memory_hive_params.rs` is the big precedent;
//! this file is the same three claims for the two hives that stand around a
//! member -- the one that says who somebody is and the one that decides whether
//! a stranger reaches them at all:
//!
//! 1. **THE ENVIRONMENT ROUTE IS GONE.** Not "deprecated", not "fallback":
//!    after this migration there is no provider lane left in either subtree, so
//!    the allowed set of `${...}` tokens is EMPTY and the sweep below says so.
//!    A clean cut is only clean if nothing reads the old surface.
//! 2. **ONE VALUE, THREE PLACES, NO DRIFT.** A knob's default exists as a
//!    literal in the script, as a value under `params`, and as
//!    `contract.settings.<knob>.default`; all three are compared per knob.
//! 3. **TUNED APART.** The same shipped script, two `params` objects, two
//!    behaviours -- asserted per knob, once negatively (nothing configured
//!    gives the shipped value) and once positively (a value handed down is the
//!    value the cell runs on). That is the property the environment form could
//!    not have, and it is what an `override_params` entry buys (GH #294: only a
//!    key the cell carries under `params` may be named at all).
//!
//! The push tick is in here too: `affinity/clock` has no script to read a
//! literal out of, so its schedule is pinned through the REAL timer parser --
//! fifteen test files used to push that schedule out of a run's way with an
//! `AFFINITY_PUSH_CRON` line in a `.env` (three more wrote a `FIREWALL_*` line
//! for the screen's arithmetic), and such a line would now be read by nothing at
//! all. A silently dead `.env` line shows up as a flake, never as a
//! red assert, which is why the positive half below exists.
//!
//! The end-to-end behaviour of the screen knobs is proved where it always was:
//! `f3_firewall_template.rs` runs a real turn over the size cap and the rate
//! window, `gh449_the_hardline_is_not_a_row.rs` runs one over the hardline
//! ceiling and `gh450_a_turn_can_wait_for_a_person.rs` over the hold pile --
//! all three hand the knob down as `params` since this migration.

use meclaw_cells::timer::params::TimerParams;
use meclaw_cells::timer::schedule::ScheduleKind;
use meclaw_core::serde_json::{Map, Value};
use std::io::Write;
use std::process::{Command, Stdio};

fn template_root(template: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates")
        .join(template)
}

fn config(template: &str, cell: &str) -> Value {
    let path = template_root(template).join(cell).join("config.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// ───────────────────────────────────────────────────────────── the inventory

/// One scripted cell's knobs: the accessor its script reads each one through
/// and the module global the value lands in. Restated here on purpose -- this
/// is the inventory the migration claims to be complete, and a knob that
/// quietly leaves a config fails here rather than in production.
struct Scripted {
    template: &'static str,
    cell: &'static str,
    /// `(knob, accessor, the python global it is assigned to)`
    knobs: &'static [(&'static str, &'static str, &'static str)],
}

const SCRIPTED: &[Scripted] = &[
    Scripted {
        template: "affinity",
        cell: "brief",
        knobs: &[
            ("disclosure_rows", "_int", "DISCLOSURE_ROWS"),
            ("traverse_depth", "_int", "TRAVERSE_DEPTH"),
            ("traverse_nodes", "_int", "TRAVERSE_NODES"),
        ],
    },
    Scripted {
        template: "affinity",
        cell: "push",
        knobs: &[("subscriber_rows", "_int", "SUBSCRIBER_ROWS")],
    },
    Scripted {
        template: "firewall",
        cell: "screen",
        knobs: &[
            ("firewall_max_chars", "_int", "MAX_CHARS"),
            ("firewall_rate_max", "_int", "RATE_MAX"),
            ("firewall_rate_window_ms", "_int", "RATE_WINDOW_MS"),
        ],
    },
    Scripted {
        template: "firewall",
        cell: "warden",
        knobs: &[
            ("firewall_hold_ttl_ms", "_int", "HOLD_TTL_MS"),
            ("firewall_hold_max", "_int", "HOLD_MAX"),
        ],
    },
];

/// The `code` cell's own param surface (`CodeParams::parse`), kept complete on
/// purpose: a substrate param this list forgets shows up below as a phantom
/// "undeclared knob", which is a false accusation against the template.
const SUBSTRATE_CODE_PARAMS: &[&str] = &[
    "runner",
    "script_path",
    "script_inline",
    "external_timeout_ms",
    "max_concurrency",
    "sandbox",
    "runner_mode",
];

// ─────────────────────────────────────────────────────────────── the harness

/// Hand a program to python3 **on stdin**, never in argv: a single argv string
/// is capped at 128 KiB (`MAX_ARG_STRLEN`) and both screen scripts are within
/// reach of it (GH #279, GH #349).
fn run_python(src: &str) -> std::process::Output {
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run one cell's shipped script over a stdin document whose `params` object is
/// `params`, then print `probe` evaluated against the module globals.
fn probe_with_params(template: &str, cell: &str, params: Value, probe: &str) -> String {
    let script = config(template, cell)["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let doc = meclaw_core::serde_json::json!({
        "envelope": {"header": {"context": {}, "hop": {}}},
        "body": {},
        "params": params,
    });
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}"
        ),
        meclaw_core::serde_json::to_string(&script).unwrap(),
        meclaw_core::serde_json::to_string(&doc.to_string()).unwrap(),
        probe
    );
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "{template}/{cell} exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Every `${NAME}` / `${NAME:-default}` a value carries, `contract` skipped --
/// a settings description may still NAME a variable in prose.
fn substitutions(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            let mut rest = s.as_str();
            while let Some(start) = rest.find("${") {
                let tail = &rest[start + 2..];
                let Some(end) = tail.find('}') else { return };
                let inner = &tail[..end];
                if !inner.starts_with("uuid7:") && !inner.starts_with("ctx.") {
                    out.push(inner.split(":-").next().unwrap_or(inner).to_string());
                }
                rest = &tail[end + 1..];
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                if key == "contract" {
                    continue;
                }
                substitutions(item, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                substitutions(item, out);
            }
        }
        _ => {}
    }
}

fn configs_under(template: &str) -> Vec<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            if p.is_dir() {
                walk(&p, out);
            } else if p.file_name().is_some_and(|n| n == "config.json") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(&template_root(template), &mut files);
    files
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// Claim 1. The old surface is not deprecated, it is absent -- and for these two
/// templates the allowed remainder is EMPTY, because neither of them ever asked
/// a provider anything: no model, no endpoint, no key. Both are deterministic
/// hives, so a single surviving `${...}` token is a knob somebody forgot.
#[test]
fn nothing_in_the_two_shipped_hives_reads_a_knob_out_of_the_environment() {
    for template in ["affinity", "firewall"] {
        let root = template_root(template);
        let files = configs_under(template);
        assert!(
            files.len() >= 4,
            "the sweep found {} config(s) under templates/{template} -- it swept \
             almost nothing and would have passed for a hive with a token in \
             every cell",
            files.len()
        );

        let mut strays: Vec<String> = Vec::new();
        for p in &files {
            let raw = std::fs::read_to_string(p).expect("config");
            let val: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
            let mut names = Vec::new();
            substitutions(&val, &mut names);
            for name in names {
                let rel = p.strip_prefix(&root).unwrap_or(p).display();
                strays.push(format!("  {template}/{rel}: ${{{name}}}"));
            }
        }
        assert!(
            strays.is_empty(),
            "{} knob(s) of templates/{template} still read out of the \
             environment (GH #138); this hive has no provider lane, so the \
             allowed set is empty:\n{}",
            strays.len(),
            strays.join("\n")
        );
    }
}

/// Claim 2. Every knob exists in all three places, with the same value.
///
/// The script literal is read out of the source text rather than exercised,
/// because that literal IS the fallback: `_int("firewall_rate_max", 30)` is the
/// value a cell uses when its config says nothing, and comparing the text is
/// the complete check over all nine scripted knobs.
#[test]
fn every_knob_is_a_param_a_setting_and_a_script_literal_with_one_value() {
    let mut total = 0usize;
    for group in SCRIPTED {
        let cfg = config(group.template, group.cell);
        let params = cfg["params"].as_object().expect("params");
        let settings = cfg["contract"]["settings"]
            .as_object()
            .expect("contract.settings");
        let src = cfg["params"]["script_inline"]
            .as_str()
            .expect("script_inline");
        let cell = format!("{}/{}", group.template, group.cell);

        for (knob, kind, _) in group.knobs {
            total += 1;
            let param = params
                .get(*knob)
                .unwrap_or_else(|| panic!("{cell}: params.{knob} is missing"));
            let default = settings
                .get(*knob)
                .unwrap_or_else(|| panic!("{cell}: contract.settings.{knob} is missing"))
                .get("default")
                .unwrap_or_else(|| panic!("{cell}: contract.settings.{knob}.default is missing"));
            assert_eq!(
                param, default,
                "{cell}: params.{knob} and contract.settings.{knob}.default disagree"
            );

            // `NAME = _int("firewall_rate_max", 30)` -- the literal after the
            // comma, which survives the clamps the warden wraps around two of
            // its own reads.
            let needle = format!("{kind}(\"{knob}\", ");
            let at = src
                .find(&needle)
                .unwrap_or_else(|| panic!("{cell}: the script does not read {knob} with {kind}"));
            let rest = &src[at + needle.len()..];
            let lit = &rest[..rest.find(')').expect("closing paren")];
            let lit: Value = meclaw_core::serde_json::from_str(lit).unwrap_or_else(|e| {
                panic!("{cell}/{knob}: script literal {lit:?} is not json ({e})")
            });
            assert_eq!(
                lit, *param,
                "{cell}: the script's own fallback for {knob} drifted from the shipped param"
            );
        }

        // No knob may hide: every non-substrate param is one of the declared ones.
        for key in params.keys() {
            assert!(
                SUBSTRATE_CODE_PARAMS.contains(&key.as_str())
                    || group.knobs.iter().any(|(k, _, _)| k == key),
                "{cell}: params.{key} is neither a substrate param nor a declared knob"
            );
        }
        assert_eq!(
            settings.len(),
            group.knobs.len(),
            "{cell}: contract.settings and the knob inventory disagree in size"
        );
    }
    assert_eq!(
        total, 9,
        "the scripted half of this migration is nine knobs; the tenth is the \
         push tick, which has no script"
    );
}

/// Claim 3 -- the point of GH #138, per knob, both ways round.
///
/// The NEGATIVE half is that an instance which configures nothing gets exactly
/// the shipped value; the POSITIVE half is that an instance which names the
/// knob gets the value it named. Under the environment form the second half
/// could not be written at all: both instances read the same variable.
#[test]
fn every_knob_is_tuned_apart_by_its_own_params() {
    for group in SCRIPTED {
        let cfg = config(group.template, group.cell);
        for (knob, _, global) in group.knobs {
            let cell = format!("{}/{}", group.template, group.cell);
            let shipped = cfg["params"][*knob].as_i64().unwrap_or_else(|| {
                panic!("{cell}: params.{knob} is not the whole number the script reads")
            });
            let probe = format!("_real.write(str({global}))");

            assert_eq!(
                probe_with_params(
                    group.template,
                    group.cell,
                    Value::Object(Map::new()),
                    &probe
                ),
                shipped.to_string(),
                "{cell}: an instance that configures nothing must run on the shipped {knob}"
            );

            // A value no shipped default is, so the assertion cannot pass by
            // accident -- and one every knob here tolerates (the warden clamps
            // its two into [0, 1024] and [1, ..], and 7 survives both).
            let tuned = 7i64;
            assert_ne!(
                shipped, tuned,
                "{cell}: pick another probe value for {knob}"
            );
            assert_eq!(
                probe_with_params(
                    group.template,
                    group.cell,
                    meclaw_core::serde_json::json!({ *knob: tuned }),
                    &probe
                ),
                tuned.to_string(),
                "{cell}: an instance that names {knob} did not get the value it named"
            );
        }
    }
}

/// A knob blanked by an operator means "not configured", not a dead cell -- and
/// a number may arrive as a string, which is what an operator typing a config
/// line writes.
///
/// Every knob of these two hives is a NUMBER, so the split the README states
/// applies to all of them the same way: `_int` falls back on `null` and on a
/// blank string alike, because there is no number in either. (`_str`, which
/// KEEPS a blank because for a name knob the empty string is a value, has no
/// instance here -- neither hive has a name knob.)
///
/// `firewall_hold_ttl_ms` is in here for a reason of its own: it is the one knob
/// with a FLOOR under it, and a floor is easy to confuse with a fallback. They
/// are different answers to different questions. `0` is a value somebody meant,
/// and it is clamped to 1 ms because a hold without an expiry is a leak; a blank
/// is nobody having said anything, and it is the shipped hour. A test that only
/// checked the clamp would have let the prose claim a blank lands on the floor.
#[test]
fn a_blank_knob_falls_back_and_a_string_number_is_read() {
    for blank in ["null", "\"\"", "\"   \""] {
        let params: Value =
            meclaw_core::serde_json::from_str(&format!("{{\"firewall_rate_max\": {blank}}}"))
                .unwrap();
        assert_eq!(
            probe_with_params("firewall", "screen", params, "_real.write(str(RATE_MAX))"),
            "30",
            "a firewall_rate_max of {blank} must fall back to the shipped default"
        );
    }
    assert_eq!(
        probe_with_params(
            "firewall",
            "screen",
            meclaw_core::serde_json::json!({"firewall_rate_max": "2"}),
            "_real.write(str(RATE_MAX))"
        ),
        "2",
        "a number typed as a string is a number"
    );
    assert_eq!(
        probe_with_params(
            "affinity",
            "brief",
            meclaw_core::serde_json::json!({"traverse_depth": "4"}),
            "_real.write(str(TRAVERSE_DEPTH))"
        ),
        "4"
    );

    // The floored knob: a blank is the shipped hour, not the 1 ms floor.
    for blank in ["null", "\"\"", "\"   \""] {
        let params: Value =
            meclaw_core::serde_json::from_str(&format!("{{\"firewall_hold_ttl_ms\": {blank}}}"))
                .unwrap();
        assert_eq!(
            probe_with_params(
                "firewall",
                "warden",
                params,
                "_real.write(str(HOLD_TTL_MS))"
            ),
            "3600000",
            "a firewall_hold_ttl_ms of {blank} is \"not configured\" and must be the \
             shipped default -- the 1 ms floor is what a value of 0 gets, and \
             confusing the two would let the README claim a blank switches the \
             expiry down to nothing"
        );
    }
}

/// The warden's two knobs may only ever make the pile SAFER, and the clamp
/// survives the move onto params: the hardline ceiling of 1024 is a constant of
/// the template and no instance may lift the pile above it (GH #449).
#[test]
fn an_instance_may_lower_the_hold_pile_and_never_raise_it() {
    assert_eq!(
        probe_with_params(
            "firewall",
            "warden",
            meclaw_core::serde_json::json!({"firewall_hold_max": 1_000_000}),
            "_real.write(str(HOLD_MAX))"
        ),
        "1024",
        "an instance lifted the hold pile past the hardline ceiling"
    );
    assert_eq!(
        probe_with_params(
            "firewall",
            "warden",
            meclaw_core::serde_json::json!({"firewall_hold_ttl_ms": 0}),
            "_real.write(str(HOLD_TTL_MS))"
        ),
        "1",
        "a hold without an expiry is a leak -- there is no value that switches \
         the timeout off"
    );
}

/// The tenth knob. `affinity/clock` is a `timer`, so there is no script to read
/// a literal out of: its schedule is a value of `params.schedules`, a key the
/// cell already carried, and the migration there is the TOKEN leaving rather
/// than a key arriving. Pinned through the REAL parser, both ways round.
///
/// The positive half is the one that matters: fifteen test files used to push
/// this tick out of their way with an `AFFINITY_PUSH_CRON` line in a `.env`, and
/// they say it with `override_params` now. A `.env` line that stopped meaning
/// anything would have been silent -- green tests with a push lane firing into
/// them every five minutes.
#[test]
fn the_push_clock_ticks_on_the_schedule_its_params_carry() {
    let mut cfg = config("affinity", "clock");
    // Read before the mutable borrow below: this is the declared half of the
    // tenth knob's triple.
    let declared = cfg["contract"]["settings"]["cron"]["default"]
        .as_str()
        .expect("contract.settings.cron.default")
        .to_string();
    let params = cfg["params"].as_object_mut().expect("params");
    // The one substitution staging performs on this file: `${uuid7:...}` is not
    // a UUID until the colony mints one.
    params["schedules"][0]["schedule_id"] =
        Value::String("0190a3f2-0000-7000-8000-000000000001".into());

    let shipped = TimerParams::parse(&Value::Object(params.clone())).expect("shipped params");
    assert_eq!(shipped.schedules.len(), 1, "the push lane ticks once");
    let ScheduleKind::Cron(shipped_cron) = &shipped.schedules[0].kind else {
        panic!("the push schedule is a repeating cron, not a one-shot `at`")
    };
    assert_eq!(
        shipped_cron, "0 */5 * * * *",
        "the shipped push cadence is a literal, not a token"
    );
    // The tenth knob gets the same three-copies check as the nine scripted ones,
    // minus the copy it cannot have: there is no script, so the triple is the
    // parsed schedule, the raw param and the declaration beside it.
    assert_eq!(
        &declared, shipped_cron,
        "contract.settings.cron.default and params.schedules[0].cron disagree -- a \
         reader of the contract and the timer would be told different cadences"
    );

    // What a mutation writes: `override_params` replaces the whole `schedules`
    // key (last-write-wins, `mutation::stage::patch_and_substitute_config`), and
    // GH #294 lets it name that key precisely because it EXISTS under `params`.
    let mut overridden = params.clone();
    overridden["schedules"][0]["cron"] = Value::String("0 0 4 1 1 *".into());
    let tuned = TimerParams::parse(&Value::Object(overridden)).expect("overridden params");
    let ScheduleKind::Cron(tuned_cron) = &tuned.schedules[0].kind else {
        panic!("the overridden schedule stopped being a cron")
    };
    assert_eq!(
        tuned_cron, "0 0 4 1 1 *",
        "the schedule an override names is not the one the timer plans on"
    );
}
