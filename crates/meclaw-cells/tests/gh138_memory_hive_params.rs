//! GH #138 -- the memory hive's knobs are params, not environment.
//!
//! Until `memory-hive@3.1.0` forty-nine behaviour knobs of this hive were
//! `${MEMORY_*}` substitution tokens: colony-global by construction, so two
//! members of one colony could not have their memories tuned apart, and every
//! knob NAME was a public configuration contract that could only be renamed by
//! breaking a running colony. Ruling R-0904-6 moves them onto the `params`
//! surface of the cells that read them, defaults bit-identical.
//!
//! `crates/meclaw-cells/tests/w13_collector_params.rs` is the pattern (GH #136,
//! `collector@1.2.0`); this file is the same three claims for the biggest
//! template in the tree:
//!
//! 1. **THE ENVIRONMENT ROUTE IS GONE FOR BEHAVIOUR.** Not "deprecated", not
//!    "fallback": the only `${...}` tokens left anywhere under
//!    `templates/memory-hive/` are the PROVIDER LANE, and they are named here
//!    one by one. A clean cut is only clean if nothing reads the old surface.
//! 2. **ONE VALUE, THREE PLACES, NO DRIFT.** A knob's default exists as a
//!    literal in the script, as a value under `params`, and as
//!    `contract.settings.<knob>.default`; all three are compared per knob.
//! 3. **TUNED APART.** The same shipped script, two `params` objects, two
//!    behaviours. That is the property the environment form could not have,
//!    and it is what an `override_params` entry buys (GH #294: only a key the
//!    cell carries under `params` may be named at all).
//!
//! The tick is in here too, because it moved in the same commit: the timer is
//! called `clock` (GH #551, ruling R-0904-5) and its schedule is a literal the
//! `params` carry rather than a token the environment fills. Six tests used to
//! push that schedule out of a run's way with a `MEMORY_DREAM_CRON` line in a
//! `.env`; they say it with `override_params` now, and
//! [`the_clock_ticks_on_the_schedule_its_params_carry`] is the proof that the
//! new form is what the timer actually plans on -- a `.env` line that silently
//! stopped meaning anything would have left those runs with a nightly job
//! firing into them and nobody saying so.

use meclaw_cells::timer::params::TimerParams;
use meclaw_cells::timer::schedule::ScheduleKind;
use meclaw_core::serde_json::{Map, Value};
use std::io::Write;
use std::process::{Command, Stdio};

fn hive_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/memory-hive")
}

fn config(cell: &str) -> Value {
    let path = hive_root().join(cell).join("config.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// ───────────────────────────────────────────────────────────── the inventory

/// Every knob of one scripted cell, with the accessor its script reads it
/// through. Restated here on purpose: this is the inventory the migration
/// claims to be complete, and a knob that quietly leaves a config fails here.
struct Scripted {
    cell: &'static str,
    knobs: &'static [(&'static str, &'static str)],
    /// Declared settings that are NOT params of this cell -- the provider lane
    /// it documents beside its own knobs. Counted so that
    /// `contract.settings.len()` can still be pinned exactly.
    documented: &'static [&'static str],
}

const SCRIPTED: &[Scripted] = &[
    Scripted {
        cell: "recall",
        knobs: &[
            ("tier0_max_episodes", "_int"),
            ("tier0_max_beliefs", "_int"),
            ("tier0_max_foresight", "_int"),
            ("tier0_episode_chars", "_int"),
            ("tier0_tokens", "_int"),
            ("tier1_leg_limit", "_int"),
            ("tier1_axis_limit", "_int"),
            ("tier1_graph_depth", "_int"),
            ("tier1_graph_nodes", "_int"),
            ("tier1_graph_fact_nodes", "_int"),
            ("tier1_graph_fact_limit", "_int"),
            ("tier1_self_limit", "_int"),
            ("tier1_self_budget", "_int"),
            ("self_legacy_subject", "_str"),
            ("tier1_topk", "_int"),
            ("sem_max_distance", "_float"),
            ("kw_min_score_ratio", "_float"),
            ("bundle_episode_budget", "_int"),
            ("tier1_tokens", "_int"),
            ("tier1_item_chars", "_int"),
            ("rrf_k", "_int"),
            ("rrf_w_keyword", "_float"),
            ("rrf_w_semantic", "_float"),
            ("rrf_w_graph", "_float"),
            ("rrf_w_temporal", "_float"),
            ("rrf_w_self", "_float"),
            ("rrf_w_temporal_point", "_float"),
            ("rrf_agreement", "_float"),
            ("query_safe_chars", "_int"),
            ("query_max_chars", "_int"),
            ("query_tokens", "_int"),
        ],
        documented: &[],
    },
    Scripted {
        cell: "dream-glue",
        knobs: &[
            ("dream_axis_limit", "_int"),
            ("canon_judge", "_str"),
            ("canon_max_predicates", "_int"),
            ("canon_max_pairs", "_int"),
            ("canon_max_axes", "_int"),
            ("canon_max_paged_axes", "_int"),
            ("canon_max_card", "_int"),
            ("canon_extract_lookback_days", "_int"),
            ("canon_closed_rows", "_int"),
            ("canon_max_closed_axes", "_int"),
            ("scratch_ttl_days", "_int"),
        ],
        documented: &[],
    },
    Scripted {
        cell: "close-glue",
        knobs: &[("close_turn_rows", "_int"), ("close_fact_rows", "_int")],
        documented: &[],
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

/// The provider lane, name by name -- the ONLY `${...}` tokens the hive may
/// still carry after the migration (ruling R-0904-6; the gate reads the same
/// classes off the name in `scripts/check_tree_rules.py` § R6).
const ENV_LANE: &[&str] = &[
    "MEMORY_LLM_BASE_URL",
    "MEMORY_EMBED_ENDPOINT",
    "MEMORY_EMBED_MODEL",
    "MEMORY_EMBED_API_KEY",
    "MEMORY_EMBED_DIM",
    "MODEL_CLOSER",
    "MODEL_DREAMER",
    "MODEL_DIALECTIC",
    "MODEL_JUDGE",
    "OPENROUTER_API_KEY",
    "OPENROUTER_HTTP_REFERER",
    "OPENROUTER_X_TITLE",
];

// ─────────────────────────────────────────────────────────────── the harness

/// Hand a program to python3 **on stdin**, never in argv: a single argv string
/// is capped at 128 KiB (`MAX_ARG_STRLEN`) and the recall script is far past
/// it (GH #279, GH #349).
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
fn probe_with_params(cell: &str, params: Value, probe: &str) -> String {
    let script = config(cell)["params"]["script_inline"]
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
        "{cell} exited non-zero: {}",
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
/// in the hive is one of the twelve provider-lane names, and the sweep is over
/// the whole subtree so a cell nobody was thinking about cannot keep one.
#[test]
fn nothing_in_the_shipped_hive_reads_a_behaviour_knob_out_of_the_environment() {
    let root = hive_root();
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
    walk(&root, &mut files);
    assert!(
        files.len() > 10,
        "the sweep found {} config(s) under templates/memory-hive -- it swept \
         nothing and would have passed for a hive with a token in every cell",
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
                strays.push(format!("  memory-hive/{rel}: ${{{name}}}"));
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
/// because that literal IS the fallback: `_int("tier1_topk", 20)` is the value
/// a cell uses when its config says nothing, and comparing the text is the
/// complete check over all forty-four scripted knobs.
#[test]
fn every_knob_is_a_param_a_setting_and_a_script_literal_with_one_value() {
    let mut total = 0usize;
    for group in SCRIPTED {
        let cfg = config(group.cell);
        let params = cfg["params"].as_object().expect("params");
        let settings = cfg["contract"]["settings"]
            .as_object()
            .expect("contract.settings");
        let src = cfg["params"]["script_inline"]
            .as_str()
            .expect("script_inline");

        for (knob, kind) in group.knobs {
            total += 1;
            let cell = group.cell;
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

            // `NAME = _int("tier1_topk", 20)` -- the literal after the comma.
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
                    || group.knobs.iter().any(|(k, _)| k == key),
                "{}: params.{key} is neither a substrate param nor a declared knob",
                group.cell
            );
        }
        assert_eq!(
            settings.len(),
            group.knobs.len() + group.documented.len(),
            "{}: contract.settings and the knob inventory disagree in size",
            group.cell
        );
    }
    assert_eq!(
        total, 44,
        "the scripted half of the migration is forty-four knobs"
    );
}

/// The four model cells carry their one knob inside `params.provider_extra`,
/// which is a param already -- the migration there is the TOKEN leaving, not a
/// key arriving. Pinned separately because the value is an object and there is
/// no script to read a literal out of.
#[test]
fn every_model_cell_carries_its_deliberation_budget_as_a_literal() {
    for (cell, effort) in [
        ("closer", "medium"),
        ("dreamer", "medium"),
        ("dialectic", "medium"),
        ("judge", "high"),
    ] {
        let cfg = config(cell);
        let shipped = &cfg["params"]["provider_extra"];
        assert_eq!(
            shipped["reasoning"]["effort"].as_str(),
            Some(effort),
            "{cell}: params.provider_extra.reasoning.effort is not the shipped literal"
        );
        assert_eq!(
            &cfg["contract"]["settings"]["provider_extra"]["default"], shipped,
            "{cell}: the declared provider_extra and the shipped one disagree"
        );
    }
}

/// Claim 3 -- the point of GH #138. One shipped script, two `params` objects,
/// two behaviours, in the cell that carries thirty-one of the forty-nine knobs.
/// Under the environment form both instances read the same variable and this
/// test could not be written at all.
#[test]
fn two_instances_of_the_same_recall_script_are_tuned_apart() {
    let shipped = probe_with_params(
        "recall",
        Value::Object(Map::new()),
        "_real.write(str(TOPK))",
    );
    assert_eq!(shipped, "20", "the shipped tier-1 top-k is still twenty");
    let terse = probe_with_params(
        "recall",
        meclaw_core::serde_json::json!({"tier1_topk": 3}),
        "_real.write(str(TOPK))",
    );
    assert_eq!(terse, "3", "an instance tuned to three did not get three");
}

/// A knob blanked by an operator means "not configured", not a dead cell -- and
/// a number may arrive as a string, which is what an operator typing a config
/// line writes.
///
/// The two accessor kinds part company on the BLANK string, deliberately, and
/// the split is pinned here because the README states it: `_int`/`_float` fall
/// back (there is no number in a blank string), `_str` keeps it, because for a
/// name knob the empty string is a VALUE. `self_legacy_subject` is exactly that
/// case -- blanking it is the documented way to switch the legacy subject off
/// for a hive that never had one, and a "helpful" fallback would make that
/// impossible to say.
#[test]
fn a_blank_knob_falls_back_and_a_string_number_is_read() {
    for blank in ["null", "\"\"", "\"   \""] {
        let params: Value =
            meclaw_core::serde_json::from_str(&format!("{{\"tier1_topk\": {blank}}}")).unwrap();
        assert_eq!(
            probe_with_params("recall", params, "_real.write(str(TOPK))"),
            "20",
            "a tier1_topk of {blank} must fall back to the shipped default"
        );
    }
    assert_eq!(
        probe_with_params(
            "recall",
            meclaw_core::serde_json::json!({"tier1_topk": "7"}),
            "_real.write(str(TOPK))"
        ),
        "7"
    );

    // The `_str` half: null is still "not configured", a blank string is not.
    assert_eq!(
        probe_with_params(
            "recall",
            meclaw_core::serde_json::json!({"self_legacy_subject": null}),
            "_real.write(SELF_LEGACY_SUBJECT)"
        ),
        "user",
        "a null name knob must fall back to the shipped default"
    );
    assert_eq!(
        probe_with_params(
            "recall",
            meclaw_core::serde_json::json!({"self_legacy_subject": ""}),
            "_real.write(\"[\" + SELF_LEGACY_SUBJECT + \"]\")"
        ),
        "[]",
        "a BLANKED name knob keeps the empty string -- that is how the legacy \
         subject is switched off, and a fallback here would make it unsayable"
    );
}

/// GH #551 -- the tick is called `clock`, and it plans on the schedule its own
/// `params` carry.
///
/// The negative half first: with no override the shipped literal is what the
/// REAL timer parser plans on. Then the positive half, which is the one that
/// matters: the schedule a mutation pushes down through `override_params` is
/// the one the timer plans on, so the five test setups that used to write a
/// `MEMORY_DREAM_CRON` line into a `.env` still move the nightly run out of
/// their way. A `.env` line that stopped meaning anything would have been
/// silent -- green tests with a nightly job firing into them.
#[test]
fn the_clock_ticks_on_the_schedule_its_params_carry() {
    let dir = hive_root().join("clock");
    assert!(
        dir.is_dir(),
        "templates/memory-hive/clock is missing -- the tick is called `clock` \
         since GH #551 (ruling R-0904-5)"
    );
    let mut cfg = config("clock");
    // Read before the mutable borrow below: the declared half of this knob's
    // pair. A timer has no script, so the triple the nine scripted knobs get is
    // a PAIR here -- the declaration and the literal inside the schedule -- and
    // nothing compared the two until now.
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
    assert_eq!(shipped.schedules.len(), 1, "the hive ticks once a night");
    let ScheduleKind::Cron(shipped_cron) = &shipped.schedules[0].kind else {
        panic!("the nightly schedule is a repeating cron, not a one-shot `at`")
    };
    assert_eq!(
        shipped_cron, "0 0 3 * * *",
        "the shipped nightly schedule is a literal, not a token"
    );
    assert_eq!(
        &declared, shipped_cron,
        "contract.settings.cron.default and params.schedules[0].cron disagree -- a \
         reader of the contract and the timer would be told different nights"
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
