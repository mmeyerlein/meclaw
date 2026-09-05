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
//! The two ticks are in here for the same reason `memory-hive/clock` was: a
//! timer has no top-level `cron` param to migrate onto -- `TimerParams::parse`
//! reads `schedules` and `query_timeout_ms` and would ignore any other key in
//! silence -- so its knob is a literal inside `params.schedules[0].cron`, and
//! an override names `schedules`. Four test setups used to push those two ticks
//! out of a run's way with a `.env` line; they say it with the param now, and
//! the clock tests below are the proof that the new form is what the real timer
//! parser plans on. A `.env` line that silently stopped meaning anything would
//! not have failed an assert -- it would have left those runs with a sweep
//! firing into them and nobody saying so.

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
        knobs: &[("max_ledger_rows", "_int")],
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
const ENV_LANE: &[&str] = &[
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
/// the complete check over all eighteen scripted knobs.
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
    assert_eq!(
        total, 18,
        "the scripted half of the migration is eighteen knobs"
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

/// Both ticks plan on the schedule their own `params` carry (GH #138).
///
/// The negative half first: with no override the shipped literal is what the
/// REAL timer parser plans on. Then the positive half, which is the one that
/// matters: the schedule a mutation pushes down through `override_params` is
/// the one the timer plans on, so the four test setups that used to write an
/// `ARGUS_CYCLE_CRON` or `ACCESS_SWEEP_CRON` line into a `.env` still move the
/// periodic run out of their way. A `.env` line that stopped meaning anything
/// would have been silent -- green tests with a sweep firing into them.
#[test]
fn both_ticks_plan_on_the_schedule_their_params_carry() {
    for (template, knob, shipped_cron) in [
        ("argus", "cycle_cron", "0 0 */6 * * *"),
        ("access", "cron", "0 */5 * * * *"),
    ] {
        let mut cfg = config(template, "clock");
        // Read before the mutable borrow below: the declared half of this
        // knob's pair. A timer has no script, so the triple the scripted knobs
        // get is a PAIR here -- the declaration and the literal inside the
        // schedule -- and nothing compared the two until now. The two names are
        // not one name: `argus` declares `cycle_cron`, `access` declares `cron`,
        // and both mean `params.schedules[0].cron`.
        let declared = cfg["contract"]["settings"][knob]["default"]
            .as_str()
            .unwrap_or_else(|| panic!("{template}/clock: contract.settings.{knob}.default"))
            .to_string();
        let params = cfg["params"].as_object_mut().expect("params");
        // The one substitution staging performs on this file: `${uuid7:...}` is
        // not a UUID until the colony mints one.
        params["schedules"][0]["schedule_id"] =
            Value::String("0190a3f2-0000-7000-8000-000000000001".into());

        let shipped = TimerParams::parse(&Value::Object(params.clone())).expect("shipped params");
        assert_eq!(shipped.schedules.len(), 1, "{template} ticks once");
        let ScheduleKind::Cron(cron) = &shipped.schedules[0].kind else {
            panic!("{template}'s schedule is a repeating cron, not a one-shot `at`")
        };
        assert_eq!(
            cron, shipped_cron,
            "{template}: the shipped schedule is a literal, not a token"
        );
        assert_eq!(
            &declared, cron,
            "{template}/clock: contract.settings.{knob}.default and \
             params.schedules[0].cron disagree -- a reader of the contract and \
             the timer would be told different cadences"
        );

        // What a mutation writes: `override_params` replaces the whole
        // `schedules` key (last-write-wins,
        // `mutation::stage::patch_and_substitute_config`), and GH #294 lets it
        // name that key precisely because it EXISTS under `params`.
        let mut overridden = params.clone();
        overridden["schedules"][0]["cron"] = Value::String("0 0 0 1 1 *".into());
        let tuned = TimerParams::parse(&Value::Object(overridden)).expect("overridden params");
        let ScheduleKind::Cron(tuned_cron) = &tuned.schedules[0].kind else {
            panic!("{template}: the overridden schedule stopped being a cron")
        };
        assert_eq!(
            tuned_cron, "0 0 0 1 1 *",
            "{template}: the schedule an override names is not the one the timer plans on"
        );
    }
}
