//! GH #138 -- the knobs of `argus`, `access` and `receptionist` are params.
//!
//! Until `argus@1.0.0`, `access@2.4.3` and `receptionist@2.0.5` twenty-two
//! behaviour knobs of these three templates were `${...}` substitution tokens.
//! A token is colony-global by construction: two brokers in one colony could
//! not be given different sweep windows, two receptions could not build
//! different composites, and every knob NAME was a public configuration
//! contract that could only be renamed by breaking a running colony. Ruling
//! R-0904-6 moves them onto the `params` surface of the cells that read them,
//! defaults bit-identical.
//!
//! `crates/meclaw-cells/tests/w13_collector_params.rs` is the pattern (GH #136,
//! `collector@1.2.0`) and `gh138_memory_hive_params.rs` is the same claim for
//! the biggest template; this file is that claim for these three:
//!
//! 1. **THE ENVIRONMENT ROUTE IS GONE FOR BEHAVIOUR.** Not "deprecated", not
//!    "fallback": the only `${...}` tokens left anywhere under the three
//!    subtrees are the PROVIDER LANE, and they are named here one by one. A
//!    clean cut is only clean if nothing reads the old surface.
//! 2. **ONE VALUE, THREE PLACES, NO DRIFT.** A knob's default exists as a
//!    literal in the script, as a value under `params`, and as
//!    `contract.settings.<knob>.default`; all three are compared per knob.
//! 3. **TUNED APART.** The same shipped script, two `params` objects, two
//!    behaviours -- once per accessor class (`_int`, `_str`, `_list`), because
//!    the three read a blank differently and a proof over one of them would
//!    say nothing about the other two. That is the property the environment
//!    form could not have, and it is what an `override_params` entry buys
//!    (GH #294: only a key the cell already carries under `params` may be
//!    named at all).
//!
//! The two clocks were in here for the same reason `memory-hive/clock` was: a
//! timer has no top-level `cron` param to migrate onto, so each tick was a
//! literal inside `params.schedules[0].cron`. Both ticks are gone since
//! R-AG-1 (GH #1096, GH #1229): each clock ships no schedule, and the cell
//! beside it arms a one-shot watchdog when something becomes due -- the
//! argus's cadence is the meter's `cycle_ms` param now, one knob among the
//! scripted ones. The clock tests below pin both halves.

use meclaw_cells::timer::op::TimerOp;
use meclaw_cells::timer::params::TimerParams;
use meclaw_cells::timer::schedule::ScheduleKind;
use meclaw_core::serde_json::{Map, Value};
use std::io::Write;
use std::process::{Command, Stdio};

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn config(template: &str, cell: &str) -> Value {
    let path = templates_root()
        .join(template)
        .join(cell)
        .join("config.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// ───────────────────────────────────────────────────────────── the inventory

/// Every knob of one scripted cell, with the accessor its script reads it
/// through. Restated here on purpose: this is the inventory the migration
/// claims to be complete, and a knob that quietly leaves a config fails here.
struct Scripted {
    template: &'static str,
    cell: &'static str,
    knobs: &'static [(&'static str, &'static str)],
    /// Declared settings that are NOT params of this cell -- the provider lane
    /// it documents beside its own knobs. Counted so that
    /// `contract.settings.len()` can still be pinned exactly.
    documented: &'static [&'static str],
}

const SCRIPTED: &[Scripted] = &[
    Scripted {
        template: "argus",
        cell: "meter",
        knobs: &[
            ("max_ledger_rows", "_int"),
            ("hint_limit", "_int"),
            ("cycle_ms", "_int"),
        ],
        documented: &[],
    },
    Scripted {
        template: "argus",
        cell: "mutator",
        knobs: &[
            ("max_numeric_step_pct", "_int"),
            ("numeric_param_keys", "_list"),
        ],
        documented: &[],
    },
    Scripted {
        template: "argus",
        cell: "probe",
        knobs: &[
            ("probe_window_sec", "_int"),
            ("probe_max_errors", "_int"),
            ("probe_ledger_tries", "_int"),
        ],
        documented: &[],
    },
    Scripted {
        template: "access",
        cell: "invoke",
        knobs: &[("usage_rows", "_int")],
        documented: &[],
    },
    Scripted {
        template: "access",
        cell: "policy",
        knobs: &[("policy_rows", "_int"), ("max_ttl_ms", "_int")],
        documented: &[],
    },
    Scripted {
        template: "access",
        cell: "sweep",
        knobs: &[("sweep_rows", "_int"), ("sweep_event_rows", "_int")],
        documented: &[],
    },
    Scripted {
        template: "receptionist",
        cell: "greet",
        knobs: &[
            ("template", "_str"),
            ("ingress", "_str"),
            ("reply_from", "_str"),
            ("error_from", "_str"),
            ("reply_to", "_str"),
            ("write_to", "_str"),
            ("error_to", "_str"),
        ],
        // `model` is the provider lane: a model id is the one thing in this
        // cell that belongs to the deployment rather than to the instance, so
        // it stays a `${RECEPTIONIST_MODEL:-...}` token and stays declared.
        documented: &["model"],
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

/// The provider lane, name by name -- the ONLY `${...}` tokens these three
/// templates may still carry after the migration (ruling R-0904-6; the gate
/// reads the same classes off the name in `scripts/check_tree_rules.py` § R6).
///
/// The two attribution names are the grey-zone call `scripts/check_tree_rules.py`
/// § R6 makes out loud (`ENV_LANE_EXACT`); argus' judge carries them since the
/// owner ruling of 2026-10-16 (every LLM request names its app), in the
/// memory-hive form.
const ENV_LANE: &[&str] = &[
    "OPENROUTER_HTTP_REFERER",
    "OPENROUTER_X_TITLE",
    "ARGUS_JUDGE_PROVIDER",
    "ARGUS_JUDGE_BASE_URL",
    "ARGUS_JUDGE_MODEL",
    "OPENROUTER_API_KEY",
    "RECEPTIONIST_MODEL",
];

// ─────────────────────────────────────────────────────────────── the harness

/// Hand a program to python3 **on stdin**, never in argv: a single argv string
/// is capped at 128 KiB (`MAX_ARG_STRLEN`) and these scripts are within a few
/// KB of that line (GH #279, GH #349).
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
///
/// The script's own output is swallowed: what these tests read is the value a
/// knob resolved to, not the emissions, and a cell that exits early on an empty
/// body has still bound its constants by then.
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

// ═══════════════════════════════════════════════════════════════════════ pins

/// Claim 1. The old surface is not deprecated, it is absent: every token left
/// in the three subtrees is one of the five provider-lane names, and the sweep
/// is over the whole subtree so a cell nobody was thinking about cannot keep
/// one.
#[test]
fn nothing_in_the_three_templates_reads_a_behaviour_knob_out_of_the_environment() {
    let mut files = Vec::new();
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
    let root = templates_root();
    for template in ["argus", "access", "receptionist"] {
        walk(&root.join(template), &mut files);
    }
    assert!(
        files.len() >= 15,
        "the sweep found {} config(s) over the three templates -- it swept \
         almost nothing and would have passed for a tree with a token in every \
         cell",
        files.len()
    );

    let mut strays: Vec<String> = Vec::new();
    for p in &files {
        let raw = std::fs::read_to_string(p).expect("config");
        let val: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
        let mut names = Vec::new();
        substitutions(&val, &mut names);
        for name in names {
            if !ENV_LANE.contains(&name.as_str()) {
                let rel = p.strip_prefix(&root).unwrap_or(p).display();
                strays.push(format!("  {rel}: ${{{name}}}"));
            }
        }
    }
    assert!(
        strays.is_empty(),
        "{} behaviour knob(s) still read out of the environment (GH #138):\n{}",
        strays.len(),
        strays.join("\n")
    );
}

/// Claim 2. Every knob exists in all three places, with the same value.
///
/// The script literal is read out of the source text rather than exercised,
/// because that literal IS the fallback: `_int("probe_window_sec", 120)` is the
/// value a cell uses when its config says nothing, and comparing the text is
/// the complete check over all nineteen scripted knobs.
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

        for (knob, kind) in group.knobs {
            total += 1;
            let (template, cell) = (group.template, group.cell);
            let param = params
                .get(*knob)
                .unwrap_or_else(|| panic!("{template}/{cell}: params.{knob} is missing"));
            let default = settings
                .get(*knob)
                .unwrap_or_else(|| panic!("{template}/{cell}: contract.settings.{knob} is missing"))
                .get("default")
                .unwrap_or_else(|| {
                    panic!("{template}/{cell}: contract.settings.{knob}.default is missing")
                });
            assert_eq!(
                param, default,
                "{template}/{cell}: params.{knob} and contract.settings.{knob}.default disagree"
            );

            // `NAME = _int("probe_window_sec", 120)` -- the literal after the
            // comma, up to the accessor's closing paren.
            let needle = format!("{kind}(\"{knob}\", ");
            let at = src.find(&needle).unwrap_or_else(|| {
                panic!("{template}/{cell}: the script does not read {knob} with {kind}")
            });
            let rest = &src[at + needle.len()..];
            let lit = &rest[..rest.find(')').expect("closing paren")];
            let lit: Value = meclaw_core::serde_json::from_str(lit).unwrap_or_else(|e| {
                panic!("{template}/{cell}/{knob}: script literal {lit:?} is not json ({e})")
            });
            assert_eq!(
                lit, *param,
                "{template}/{cell}: the script's own fallback for {knob} drifted from the \
                 shipped param"
            );
        }

        // No knob may hide: every non-substrate param is one of the declared ones.
        for key in params.keys() {
            assert!(
                SUBSTRATE_CODE_PARAMS.contains(&key.as_str())
                    || group.knobs.iter().any(|(k, _)| k == key),
                "{}/{}: params.{key} is neither a substrate param nor a declared knob",
                group.template,
                group.cell
            );
        }
        assert_eq!(
            settings.len(),
            group.knobs.len() + group.documented.len(),
            "{}/{}: contract.settings and the knob inventory disagree in size",
            group.template,
            group.cell
        );
    }
    // Nineteen since GH #927: the meter's `hint_limit` (how many open hints a
    // judge is shown) joined the inventory as a param from its first day.
    // Twenty since GH #1229: the meter's `cycle_ms` is the cadence the clock's
    // `cycle_cron` used to carry, a param of the cell that arms the watchdog.
    assert_eq!(
        total, 20,
        "the scripted half of the migration is twenty knobs"
    );
}

/// The vault's two knobs are params already -- the migration there is the TOKEN
/// leaving, not a key arriving. Pinned separately because this cell has no
/// script for a third copy: the value it runs on IS `params.key_source`, read
/// by the substrate's own `vault` cell type.
///
/// Both name a SOURCE rather than key material (`key_source` says where the
/// master key comes from, `credential_name` says which file to read under
/// `$CREDENTIALS_DIRECTORY`), which is why the ruling puts them on the params
/// surface and not in the provider lane beside the passphrase itself --
/// `params.unlock_env` still names an environment variable and still ships
/// `null`.
#[test]
fn the_vault_names_its_key_source_and_its_credential_as_literals() {
    let cfg = config("access", "vault");
    for (knob, shipped) in [("key_source", "auto"), ("credential_name", "vault_key")] {
        assert_eq!(
            cfg["params"][knob].as_str(),
            Some(shipped),
            "access/vault: params.{knob} is not the shipped literal"
        );
        assert_eq!(
            cfg["contract"]["settings"][knob]["default"], cfg["params"][knob],
            "access/vault: the declared default and the shipped param disagree for {knob}"
        );
    }
    assert!(
        cfg["params"]["unlock_env"].is_null(),
        "the passphrase is still DECLARED and UNSET -- a woken vault is locked"
    );
}

/// Claim 3, the `_int` half -- the point of GH #138. One shipped script, two
/// `params` objects, two behaviours. Under the environment form both instances
/// read the same variable and this test could not be written at all.
#[test]
fn two_instances_of_the_same_probe_script_are_tuned_apart() {
    let shipped = probe_with_params(
        "argus",
        "probe",
        Value::Object(Map::new()),
        "_real.write(str(WINDOW_SEC))",
    );
    assert_eq!(shipped, "120", "the shipped health-check window is 120s");
    let wider = probe_with_params(
        "argus",
        "probe",
        meclaw_core::serde_json::json!({"probe_window_sec": 900}),
        "_real.write(str(WINDOW_SEC))",
    );
    assert_eq!(wider, "900", "an instance tuned to 900s did not get 900s");
}

/// Claim 3, the `_str` half. Two receptions of one colony build different
/// composites and answer onto different lanes -- the thing an operator running
/// two channels most obviously wants, and the thing one `.env` could not say.
#[test]
fn two_receptions_of_one_colony_build_different_composites() {
    let shipped = probe_with_params(
        "receptionist",
        "greet",
        Value::Object(Map::new()),
        "_real.write(TEMPLATE + '|' + '[' + REPLY_TO + ']')",
    );
    assert_eq!(
        shipped, "talky|[]",
        "the shipped reception builds a `talky` and leaves the answer lane unwired"
    );
    let tuned = probe_with_params(
        "receptionist",
        "greet",
        meclaw_core::serde_json::json!({"template": "cogny", "reply_to": "./sink"}),
        "_real.write(TEMPLATE + '|' + '[' + REPLY_TO + ']')",
    );
    assert_eq!(
        tuned, "cogny|[./sink]",
        "a reception tuned to another composite did not get it"
    );
}

/// Claim 3, the `_list` half. The mutator's radius is a declared key set, and
/// widening it is exactly the operator decision the shipped comment describes
/// -- per instance now, not per colony.
#[test]
fn the_mutators_radius_is_widened_per_instance() {
    let shipped = probe_with_params(
        "argus",
        "mutator",
        Value::Object(Map::new()),
        "_real.write(','.join(NUMERIC_PARAM_KEYS))",
    );
    assert_eq!(
        shipped, "temperature,max_tokens,external_timeout_ms,attachment_timeout_ms",
        "the shipped radius is the llm cell's runtime-mutable numeric params"
    );
    let widened = probe_with_params(
        "argus",
        "mutator",
        meclaw_core::serde_json::json!({"numeric_param_keys": ["temperature", "top_p"]}),
        "_real.write(','.join(NUMERIC_PARAM_KEYS))",
    );
    assert_eq!(
        widened, "temperature,top_p",
        "the widened radius did not land"
    );
    // A comma string is read the same way: `override_params` reaches this knob
    // as a config line somebody types, and the shipped default was one.
    let typed = probe_with_params(
        "argus",
        "mutator",
        meclaw_core::serde_json::json!({"numeric_param_keys": "temperature, top_p"}),
        "_real.write(','.join(NUMERIC_PARAM_KEYS))",
    );
    assert_eq!(
        typed, "temperature,top_p",
        "a typed comma list was not read"
    );
}

/// A knob blanked by an operator means "not configured", not a dead cell -- and
/// a number may arrive as a string, which is what an operator typing a config
/// line writes.
///
/// The two accessor kinds part company on the BLANK string, deliberately:
/// `_int` falls back (there is no number in a blank string), `_str` keeps it,
/// because for an ADDRESS knob the empty string is a VALUE. `ingress` is
/// exactly that case -- empty means "the instance path itself", which is what a
/// sealed composite wants and what the shipped default already is, so a
/// "helpful" fallback here would make the default unsayable.
#[test]
fn a_blank_knob_falls_back_and_a_string_number_is_read() {
    for blank in ["null", "\"\"", "\"   \""] {
        let params: Value =
            meclaw_core::serde_json::from_str(&format!("{{\"probe_window_sec\": {blank}}}"))
                .unwrap();
        assert_eq!(
            probe_with_params("argus", "probe", params, "_real.write(str(WINDOW_SEC))"),
            "120",
            "a probe_window_sec of {blank} must fall back to the shipped default"
        );
    }
    assert_eq!(
        probe_with_params(
            "argus",
            "probe",
            meclaw_core::serde_json::json!({"probe_window_sec": "45"}),
            "_real.write(str(WINDOW_SEC))"
        ),
        "45",
        "a number typed as a string must be read as a number"
    );

    // The `_str` half: null is still "not configured", a blank string is not.
    assert_eq!(
        probe_with_params(
            "receptionist",
            "greet",
            meclaw_core::serde_json::json!({"template": null}),
            "_real.write(TEMPLATE)"
        ),
        "talky",
        "a null composite name must fall back to the shipped default"
    );
    assert_eq!(
        probe_with_params(
            "receptionist",
            "greet",
            meclaw_core::serde_json::json!({"ingress": ""}),
            "_real.write(\"[\" + INGRESS + \"]\")"
        ),
        "[]",
        "a BLANKED address knob keeps the empty string -- that is how a sealed \
         composite is addressed at its own root, and a fallback here would make \
         it unsayable"
    );
}

/// Neither clock ticks any more (GH #1096, GH #1229, R-AG-1).
///
/// Both used to plan on a cron literal inside `params.schedules[0]`, and this
/// test proved the real timer parser planned on the literal and on an
/// `override_params` replacement of it. Both ticks are gone: each clock ships
/// no schedule and declares no cadence, and the timer parser plans on nothing
/// until the cell beside it arms a watchdog -- `access/sweep` for the next
/// `expires_at`, `argus/meter` for the next cycle while a goal is enabled.
#[test]
fn both_ticks_plan_on_the_schedule_their_params_carry() {
    for (template, knob, why) in [
        ("access", "cron", "a tick over unchanged grants"),
        (
            "argus",
            "cycle_cron",
            "a tick over a charter nobody switched on",
        ),
    ] {
        let cfg = config(template, "clock");
        let shipped = TimerParams::parse(&cfg["params"])
            .unwrap_or_else(|e| panic!("{template}/clock params: {e}"));
        assert!(
            shipped.schedules.is_empty(),
            "{template}/clock ships a schedule again -- {why}"
        );
        assert!(
            cfg["contract"]["settings"].get(knob).is_none(),
            "{template}/clock declares a {knob} it no longer carries"
        );
    }
}

/// R-AG-1 (GH #1229): the argus clock is a watchdog, not a six-hourly tick.
///
/// The clock ships no schedule and declares no cadence; the cadence is the
/// meter's `cycle_ms`, and the meter spends it only in a `goals` step that
/// found an enabled goal. That step appends ONE one-shot order for the next
/// cycle on the `clock` lane, which the hive draws onto `./clock`; a charter
/// with nothing enabled writes its `idle` receipt and orders nothing, so an
/// inert argus rests until an event (`in_cycle`) arrives. The script runs
/// with its clock injected -- `datetime.now` frozen at one moment -- so the
/// moment of the order is compared exactly, and no wall clock judges anything.
#[test]
fn gh1229_argus_arms_its_next_cycle_only_while_a_goal_is_enabled() {
    let clock = config("argus", "clock");
    assert_eq!(
        clock["params"]["schedules"],
        meclaw_core::serde_json::json!([]),
        "argus/clock ships a schedule -- a tick over a charter nobody switched on"
    );
    assert!(
        clock["contract"]["settings"].get("cycle_cron").is_none(),
        "argus/clock declares a cadence of its own; the cadence is the meter's cycle_ms"
    );
    let parsed = TimerParams::parse(&clock["params"]).expect("argus/clock params");
    assert!(parsed.schedules.is_empty(), "the timer plans on nothing");

    let hive_path = templates_root().join("argus/config.json");
    let hive: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(&hive_path).expect("argus/config.json"),
    )
    .expect("argus/config.json is json");
    let edges = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("the hive's edges");
    assert!(
        edges.iter().any(|e| e["from"] == "./meter"
            && e["to"] == "./clock"
            && e["condition"] == "has(hop.route) && hop.route == 'clock'"),
        "no edge carries the meter's order onto ./clock"
    );

    let cycle_ms = meter_params()["cycle_ms"]
        .as_i64()
        .expect("the meter ships params.cycle_ms");
    let goals_step = |rows: Value| -> Vec<Value> {
        meter_echo(
            "goals",
            meclaw_core::serde_json::json!({"operation": "select"}),
            rows,
        )
    };
    let frozen_ms = METER_NOW_MS;

    // One enabled goal: the cycle goes on (the rules are read) and the next
    // cycle is ordered, once.
    let armed = goals_step(meclaw_core::serde_json::json!([{
        "id": "goal:llm-cost", "metric": "llm_cost", "source": "colony",
        "direction": "lower", "window_minutes": 60, "min_samples": 30,
        "min_delta_pct": 10, "quality_gate": "answer_quality", "enabled": 1}]));
    assert!(
        armed.iter().any(|m| m["header"]["route"] == "cstore"),
        "the cycle itself goes on and reads the rules: {armed:?}"
    );
    let orders: Vec<&Value> = armed
        .iter()
        .filter(|m| m["header"]["route"] == "clock")
        .collect();
    assert_eq!(
        orders.len(),
        1,
        "a tick with an enabled goal arms exactly one next cycle: {armed:?}"
    );
    let order = orders[0];
    assert_eq!(order["op"], "add", "{order}");
    assert_eq!(order["schedule_name"], "argus-cycle", "{order}");
    assert_eq!(order["emit_to"], "../meter", "{order}");
    assert_eq!(
        order["rearm"], true,
        "the order replaces the standing one: {order}"
    );
    let mut body = order.clone();
    body.as_object_mut().expect("an object").remove("header");
    match TimerOp::parse(&body).expect("the timer's own parser takes the order") {
        TimerOp::Add { row, rearm } => {
            assert!(rearm, "{order}");
            assert_eq!(row.schedule_name, "argus-cycle");
            let ScheduleKind::At(at) = row.kind else {
                panic!("a one-shot `at`, not a cron: {order}")
            };
            assert_eq!(
                at.timestamp_millis(),
                frozen_ms + cycle_ms,
                "the next cycle is cycle_ms after this tick: {order}"
            );
        }
        other => panic!("an add, not {other:?}"),
    }

    // Nothing enabled: the idle receipt, and no order -- the argus rests.
    let inert = goals_step(meclaw_core::serde_json::json!([]));
    assert!(
        !inert.iter().any(|m| m["header"]["route"] == "clock"),
        "an inert charter arms nothing: {inert:?}"
    );
}

/// 2030-01-01T00:00:00Z: the moment the meter script is told it is.
const METER_NOW_MS: i64 = 1_893_456_000_000;

/// argus/meter's shipped params, `script_inline` taken out.
fn meter_params() -> Value {
    let mut params = config("argus", "meter")["params"].clone();
    params
        .as_object_mut()
        .expect("params")
        .remove("script_inline");
    params
}

/// Run argus/meter's shipped script over ONE store echo -- `hop` and the
/// phase as the hive edge hands them on, `rows` as the tool result -- with its
/// clock injected (`datetime.now` frozen at `METER_NOW_MS`), and check every
/// emission against the meter's own contract. The harness of the two R-AG-1
/// tests (GH #1229) on the meter's cycle order.
fn meter_echo(phase: &str, hop: Value, rows: Value) -> Vec<Value> {
    let meter = config("argus", "meter");
    let script = meclaw_testing::resolve_script_vars(
        meter["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    );
    let block: meclaw_colony::config::ContractBlock =
        meclaw_core::serde_json::from_value(meter["contract"].clone())
            .expect("the meter's contract parses");
    let declared =
        meclaw_core::CompiledEmits::compile(&block.emits).expect("the emits schemas compile");
    let flat = meclaw_core::serde_json::json!({
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c1",
                      "text": rows.to_string()}],
        "header": {"hop": hop, "context": {"ar_phase": phase, "ar_carry": "{}"}},
        "params": meter_params(),
    });
    let src = format!(
        concat!(
            "import sys, io, datetime\n",
            "class _Frozen(datetime.datetime):\n",
            "    @classmethod\n",
            "    def now(cls, tz=None):\n",
            "        return cls.fromtimestamp({} / 1000.0, tz)\n",
            "datetime.datetime = _Frozen\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), {{'__name__': '__main__'}})\n"
        ),
        METER_NOW_MS,
        meclaw_core::serde_json::to_string(&script).unwrap(),
        meclaw_core::serde_json::to_string(&meclaw_testing::code_stdin(&flat).to_string()).unwrap(),
    );
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "argus/meter exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let emitted: Vec<Value> = meclaw_core::serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)));
    for m in &emitted {
        meclaw_core::validate_emits(m, &declared)
            .unwrap_or_else(|e| panic!("the meter's own contract refuses {m}: {e}"));
    }
    emitted
}

/// R-AG-1 (GH #1229), re-check K-UHREN A I3: the cycle is armed in the
/// `goals` step, so a store reject on either read before it -- the deadline
/// read (`stale`) or the charter read (`goals`) -- used to end in the error
/// branch with a `store_error` receipt and NO order: the strike that started
/// the cycle was spent, and a loop with an enabled goal stood still until
/// somebody called `in_cycle`. The error branch now orders the next cycle
/// too; an inert charter still ends the chain at the next strike, which
/// reads it again.
#[test]
fn gh1229_a_store_reject_before_the_charter_arms_the_next_cycle() {
    let cycle_ms = meter_params()["cycle_ms"]
        .as_i64()
        .expect("the meter ships params.cycle_ms");
    for phase in ["stale", "goals"] {
        let out = meter_echo(
            phase,
            meclaw_core::serde_json::json!({"operation": "select",
                                            "error_code": "query_timeout"}),
            meclaw_core::serde_json::json!([]),
        );
        assert!(
            out.iter().any(|m| m["header"]["route"] == "error"),
            "a reject in `{phase}` still says so on the error lane: {out:?}"
        );
        let orders: Vec<&Value> = out
            .iter()
            .filter(|m| m["header"]["route"] == "clock")
            .collect();
        assert_eq!(
            orders.len(),
            1,
            "a reject in `{phase}` arms exactly one next cycle: {out:?}"
        );
        let mut body = orders[0].clone();
        body.as_object_mut().expect("an object").remove("header");
        match TimerOp::parse(&body).expect("the timer's own parser takes the order") {
            TimerOp::Add { row, rearm } => {
                assert!(rearm, "{body}");
                assert_eq!(row.schedule_name, "argus-cycle");
                let ScheduleKind::At(at) = row.kind else {
                    panic!("a one-shot `at`, not a cron: {body}")
                };
                assert_eq!(
                    at.timestamp_millis(),
                    METER_NOW_MS + cycle_ms,
                    "the next cycle is cycle_ms after the rejected step: {body}"
                );
            }
            other => panic!("an add, not {other:?}"),
        }
    }

    // A reject of the receipt itself (`serr`) and of a later step orders
    // nothing: the cycle was armed in `goals` already, or the chain is the
    // receipt's own.
    for phase in ["serr", "rules"] {
        let out = meter_echo(
            phase,
            meclaw_core::serde_json::json!({"operation": "select",
                                            "error_code": "query_timeout"}),
            meclaw_core::serde_json::json!([]),
        );
        assert!(
            !out.iter().any(|m| m["header"]["route"] == "clock"),
            "a reject in `{phase}` orders no cycle: {out:?}"
        );
    }
}
