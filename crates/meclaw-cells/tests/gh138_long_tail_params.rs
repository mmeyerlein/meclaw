//! GH #138 -- the long tail: the nine PUBLISHED roots, and not a behaviour knob
//! left in the environment.
//!
//! **Split under GH #584.** The strand covered eleven roots, two of which --
//! `llm-registry` and `coder-pipeline` -- are not in the export allow-list, so
//! the whole file was on the export BLOCKLIST and the published tree lost the
//! nine it could have carried. The two private roots now live in
//! `gh138_private_tail_params.rs`, which keeps the blocklist entry; this half
//! reads published templates only and travels. Splitting rather than guarding
//! is the `gh235`/`gh343` precedent: an `exists()` skip would ship a test that
//! asserts nothing, which is the defect GH #234 was about.
//!
//! This is the last strand of ruling R-0904-6. The three before it moved the
//! templates that carry a knob budget worth a table of its own -- the memory
//! hive's forty-nine, the keeper/summarizer/dispatcher trio, argus/access/
//! receptionist, affinity/firewall. What was left is the TAIL: templates with
//! one, two or four knobs each, plus the grey zone the ruling settled by hand
//! (a grant id and a sandbox root are REFERENCES, not material, so they are
//! behaviour and they are params; a bearer token, an endpoint and a model id
//! are the provider lane and they stay in `.env`).
//!
//! `crates/meclaw-cells/tests/w13_collector_params.rs` is the pattern (GH #136,
//! `collector@1.2.0`) and `gh138_memory_hive_params.rs` is the big worked
//! example. This file makes the same claims for nine templates at once, and
//! it makes them in the THREE shapes the tail actually has -- because a
//! migration that only knew the scripted shape would have quietly left the
//! other two behind:
//!
//! 1. **SCRIPTED** (`code` cells): the knob is a `params` key, a
//!    `contract.settings` entry and a literal in the script, read through
//!    `_int` / `_str` off the stdin document. Three copies, one value.
//! 2. **TYPED** (`file`, `edit`, `vault`, `llm` cells): the knob was ALREADY a
//!    params key with a `${...}` token inside it. The migration there is the
//!    TOKEN leaving, not a key arriving -- the key was addressable by
//!    `override_params` (GH #294) all along, it just could not hold a value of
//!    its own on disk.
//! 3. **SCHEDULED** (`timer` cells): the knob lives inside `params.schedules`,
//!    which is one key holding a list. Same story as the typed shape, and the
//!    proof is the REAL `TimerParams::parse` planning on it.
//!
//! Every claim below is made twice where a claim can be: negatively (the
//! shipped literal is what the cell reads with nothing overridden) and
//! positively (a value pushed down through `override_params` is what it reads
//! instead). The negative half alone would pass for a template that ignored its
//! params entirely.

use meclaw_cells::llm::LlmParams;
use meclaw_cells::timer::params::TimerParams;
use meclaw_cells::timer::schedule::ScheduleKind;
use meclaw_cells::vault::VaultParams;
use meclaw_cells::{EditCellFactory, FileCellFactory};
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::{Map, Value};
use std::io::Write;
use std::process::{Command, Stdio};

/// The nine PUBLISHED roots of the eleven this strand owns; `llm-registry` and
/// `coder-pipeline` are the twin's (GH #584). `steward` is deliberately absent
/// from both: it has been deprecated since GH #462, ships one more release and
/// takes no further work, so its seven knobs stay where they are and its row
/// stays in the gate's `TRANSITIONAL` table.
const TEMPLATES: &[&str] = &[
    "builder-librarian",
    "archive-bridge",
    "daily-digest",
    "canvy",
    "clock",
    "tools",
    "vault",
    "talky",
    "cogny",
];

/// The provider lane of these nine, name by name -- the ONLY `${...}` tokens
/// they may still carry. Every one of them is a credential or an endpoint: a
/// secret in a `config.json` is a secret in the repository, which is the one
/// thing ruling R-0904-6 does NOT move. The gate reads the same classes off the
/// name in `scripts/check_tree_rules.py` § R6.
const ENV_LANE: &[&str] = &[
    "OPENROUTER_API_KEY",
    "SEARCH_API_KEY",
    "SEARCH_ENDPOINT",
    "TELEGRAM_BOT_TOKEN",
];

/// Params the substrate itself reads off a scripted cell, so they are never a
/// knob of the template.
const SUBSTRATE_CODE_PARAMS: &[&str] = &[
    "runner",
    "script_path",
    "script_inline",
    "external_timeout_ms",
    "max_concurrency",
    "sandbox",
    "runner_mode",
];

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// One cell's `config.json`. `cell` is the path under `templates/`, so a
/// single-cell template is named by its root alone (`vault`, `clock`).
fn config(cell: &str) -> Value {
    let path = repo(&format!("templates/{cell}/config.json"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// ───────────────────────────────────────────────────────────── the inventory

/// One scripted cell, with the accessor its script reads each knob through.
/// Restated here on purpose: this is the inventory the migration claims to be
/// complete, and a knob that quietly leaves a config fails here.
struct Scripted {
    cell: &'static str,
    knobs: &'static [(&'static str, &'static str)],
}

const SCRIPTED: &[Scripted] = &[
    Scripted {
        cell: "builder-librarian/retrieve",
        knobs: &[
            ("row_chars", "_int"),
            ("catalogue_chars", "_int"),
            ("level_chars", "_int"),
            ("topk", "_int"),
        ],
    },
    Scripted {
        cell: "archive-bridge",
        knobs: &[("archive_table", "_str")],
    },
];

/// The typed half: a knob that was always a params key of a substrate cell
/// type, with a `${...}` token sitting inside it. Cell, key, and the literal it
/// ships.
const TYPED: &[(&str, &str, &str)] = &[
    ("tools/edit", "base_path", "/tmp"),
    ("tools/file", "base_path", "/tmp"),
    ("vault", "key_source", "auto"),
    ("vault", "credential_name", "vault_key"),
    ("talky/brain", "credential_grant_id", ""),
    ("cogny/brain", "credential_grant_id", ""),
];

// ─────────────────────────────────────────────────────────────── the harness

/// Hand a program to python3 **on stdin**, never in argv: a single argv string
/// is capped at 128 KiB (`MAX_ARG_STRLEN`) and these scripts are long enough
/// to make that a real bound (GH #279, GH #349).
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
        "body": {"messages": []},
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
/// a settings description may still NAME a variable in prose, and the gate
/// skips the same block for the same reason.
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

/// The shipped `params` of one timer, with the one substitution staging
/// performs on such a file resolved: `${uuid7:...}` is not a UUID until the
/// colony mints one.
fn timer_params(cell: &str) -> Map<String, Value> {
    let mut cfg = config(cell);
    let params = cfg["params"].as_object_mut().expect("params");
    let schedules = params["schedules"].as_array_mut().expect("schedules");
    for (n, row) in schedules.iter_mut().enumerate() {
        let id = row["schedule_id"].as_str().unwrap_or_default().to_string();
        if id.starts_with("${uuid7:") {
            row["schedule_id"] =
                Value::String(format!("0190a3f2-0000-7000-8000-00000000000{}", n + 1));
        }
    }
    params.clone()
}

fn cron_of(params: &Map<String, Value>, cell: &str) -> String {
    let parsed = TimerParams::parse(&Value::Object(params.clone()))
        .unwrap_or_else(|e| panic!("{cell}: the shipped params do not parse: {e}"));
    assert_eq!(parsed.schedules.len(), 1, "{cell} ticks on one schedule");
    match &parsed.schedules[0].kind {
        ScheduleKind::Cron(c) => c.clone(),
        _ => panic!("{cell}: the schedule is a repeating cron, not a one-shot `at`"),
    }
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// Claim 1. The old surface is not deprecated, it is absent. The sweep walks
/// every `config.json` under all nine published roots, so a cell nobody was
/// thinking about cannot keep a token. The two private roots are swept the same
/// way by the twin (GH #584), so no cell of the strand goes unswept.
#[test]
fn nothing_in_the_shipped_long_tail_reads_a_behaviour_knob_out_of_the_environment() {
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
    let root = repo("templates");
    for t in TEMPLATES {
        walk(&root.join(t), &mut files);
    }
    // Forty configs live under the nine roots today (re-measured for the split,
    // GH #584); the floor is half of that, which is low enough to survive a
    // template losing a cell and high enough that a sweep over a wrong or empty
    // root cannot pass.
    assert!(
        files.len() > 20,
        "the sweep found {} config(s) across the nine published templates -- it \
         swept almost nothing and would have passed for a tree with a token in \
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

/// Claim 2, scripted shape. Every knob exists in all three places, with the
/// same value. The script literal is read out of the source text rather than
/// exercised, because that literal IS the fallback: `_int("topk", 5)` is what
/// the cell uses when its config says nothing.
#[test]
fn every_scripted_knob_is_a_param_a_setting_and_a_script_literal_with_one_value() {
    let mut total = 0usize;
    for group in SCRIPTED {
        let cfg = config(group.cell);
        let params = cfg["params"]
            .as_object()
            .unwrap_or_else(|| panic!("{}: no params block", group.cell));
        let settings = cfg["contract"]["settings"]
            .as_object()
            .unwrap_or_else(|| panic!("{}: no contract.settings", group.cell));
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

            // `NAME = _int("topk", 5)` -- the literal after the comma.
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
            group.knobs.len(),
            "{}: contract.settings and the knob inventory disagree in size",
            group.cell
        );
    }
    assert_eq!(
        total, 5,
        "the published scripted half of this strand is five knobs across two \
         cells -- the other five are the twin's (GH #584)"
    );
}

/// Claim 2, typed shape. The key was always there; what it holds is now a
/// literal, and the declared default says the same thing.
#[test]
fn every_typed_knob_is_a_literal_its_contract_declares() {
    for (cell, knob, shipped) in TYPED {
        let cfg = config(cell);
        let param = cfg["params"]
            .get(knob)
            .unwrap_or_else(|| panic!("{cell}: params.{knob} is missing"));
        assert_eq!(
            param.as_str(),
            Some(*shipped),
            "{cell}: params.{knob} is not the shipped literal"
        );
        let default = cfg["contract"]["settings"]
            .get(knob)
            .unwrap_or_else(|| panic!("{cell}: contract.settings.{knob} is missing"))
            .get("default")
            .unwrap_or_else(|| panic!("{cell}: contract.settings.{knob}.default is missing"));
        assert_eq!(
            default.as_str(),
            Some(*shipped),
            "{cell}: the declared default and the shipped param disagree"
        );
    }
}

/// Claim 3, scripted. One shipped script, two `params` objects, two
/// behaviours -- the property the environment form could not have. The
/// negative half (nothing overridden -> the shipped five) is what makes the
/// positive half mean something.
#[test]
fn two_instances_of_the_same_retrieve_script_are_tuned_apart() {
    let cell = "builder-librarian/retrieve";
    assert_eq!(
        probe_with_params(cell, Value::Object(Map::new()), "_real.write(str(TOPK))"),
        "5",
        "the shipped top-k is still five"
    );
    assert_eq!(
        probe_with_params(
            cell,
            meclaw_core::serde_json::json!({"topk": 2}),
            "_real.write(str(TOPK))"
        ),
        "2",
        "an instance tuned to two did not get two"
    );
    // The other end of the same claim: a window knob, and a STRING one.
    assert_eq!(
        probe_with_params(
            cell,
            meclaw_core::serde_json::json!({"catalogue_chars": 99}),
            "_real.write(str(CATALOGUE_CHARS))"
        ),
        "99"
    );
    assert_eq!(
        probe_with_params(
            "archive-bridge",
            meclaw_core::serde_json::json!({"archive_table": "ledger"}),
            "_real.write(ARCHIVE_TABLE)"
        ),
        "ledger",
        "an archive bridge pointed at another table did not get it"
    );
    // The same claim over the two private roots -- two registries in one colony,
    // paged apart -- is the twin's (GH #584).
}

/// A knob blanked by an operator means "not configured", not a dead cell -- and
/// a number may arrive as a string, which is what an operator typing a config
/// line writes.
///
/// The two accessor kinds part company on the BLANK string, deliberately:
/// `_int` falls back (there is no number in a blank string), `_str` keeps it,
/// because for a NAME knob the empty string is a value. `credential_grant_id`
/// is the same rule one layer down and the reason it is stated here: unset it
/// resolves to the empty string, which means "no grant at all", and a helpful
/// fallback would make that impossible to say.
#[test]
fn a_blank_knob_falls_back_and_a_string_number_is_read() {
    let cell = "builder-librarian/retrieve";
    for blank in ["null", "\"\"", "\"   \""] {
        let params: Value =
            meclaw_core::serde_json::from_str(&format!("{{\"topk\": {blank}}}")).unwrap();
        assert_eq!(
            probe_with_params(cell, params, "_real.write(str(TOPK))"),
            "5",
            "a topk of {blank} must fall back to the shipped default"
        );
    }
    assert_eq!(
        probe_with_params(
            cell,
            meclaw_core::serde_json::json!({"topk": "3"}),
            "_real.write(str(TOPK))"
        ),
        "3",
        "a numeric knob typed as a string must still be read"
    );
    assert_eq!(
        probe_with_params(
            "archive-bridge",
            meclaw_core::serde_json::json!({"archive_table": null}),
            "_real.write(ARCHIVE_TABLE)"
        ),
        "archive",
        "a null name knob must fall back to the shipped default"
    );
    assert_eq!(
        probe_with_params(
            "archive-bridge",
            meclaw_core::serde_json::json!({"archive_table": ""}),
            "_real.write(\"[\" + ARCHIVE_TABLE + \"]\")"
        ),
        "[]",
        "a BLANKED name knob keeps the empty string -- an `_str` accessor that \
         fell back could not express `no table`"
    );
}

/// Claim 3, scheduled. Three timers, and the proof is the REAL parser: what it
/// plans on with nothing overridden is the shipped literal, and what it plans
/// on with an `override_params` schedule is that one instead. A `.env` line
/// that stopped meaning anything would have been silent.
#[test]
fn the_three_clocks_tick_on_the_schedules_their_params_carry() {
    for (cell, shipped) in [
        ("clock", "0 */5 * * * *"),
        ("canvy/clock", "0 * * * * *"),
        ("daily-digest/clock", "0 0 8 * * *"),
    ] {
        let params = timer_params(cell);
        assert_eq!(
            cron_of(&params, cell),
            shipped,
            "{cell}: the shipped schedule is a literal, not a token"
        );

        // What a mutation writes: `override_params` replaces the whole
        // `schedules` key (last-write-wins,
        // `mutation::stage::patch_and_substitute_config`), and GH #294 lets it
        // name that key precisely because it EXISTS under `params`.
        let mut tuned = params.clone();
        tuned["schedules"][0]["cron"] = Value::String("0 0 4 1 1 *".into());
        assert_eq!(
            cron_of(&tuned, cell),
            "0 0 4 1 1 *",
            "{cell}: the schedule an override names is not the one the timer plans on"
        );
    }
}

/// `clock`'s schedule KEY is the second half of that template's migration, and
/// it is the one knob in this strand whose value the parser validates: a
/// `schedule_id` is a UUID, so a literal that is not one does not reach a
/// running timer at all.
#[test]
fn the_clock_carries_its_schedule_key_as_a_literal_uuid() {
    let params = timer_params("clock");
    assert_eq!(
        params["schedules"][0]["schedule_id"].as_str(),
        Some("0190a3f2-0000-7000-8000-000000000484"),
        "the shipped schedule key is a literal UUID, not a token"
    );
    let parsed = TimerParams::parse(&Value::Object(params)).expect("shipped params");
    assert_eq!(
        parsed.schedules[0].schedule_id.to_string(),
        "0190a3f2-0000-7000-8000-000000000484"
    );
}

/// The digest's payload rides on the schedule, and both halves of it are
/// literals now: the URL inside the `tool_call` body and the chat id in
/// `emit_headers`. The chat id ships EMPTY on purpose -- see the template
/// README: the environment form refused to boot without one, and a params
/// default cannot refuse; what replaces the refusal is that the hive
/// instantiates inactive and an empty chat id delivers nowhere.
#[test]
fn the_digest_clock_carries_its_url_and_its_chat_id_on_the_schedule() {
    let params = timer_params("daily-digest/clock");
    let parsed = TimerParams::parse(&Value::Object(params.clone())).expect("shipped params");
    let row = &parsed.schedules[0];
    let call = row.emit_body["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        call, "{\"url\": \"https://example.com\"}",
        "the digest URL is a literal in the schedule's tool_call body"
    );
    assert_eq!(
        row.emit_headers.get("chat_id").and_then(Value::as_str),
        Some(""),
        "the shipped chat id is the empty string -- no chat, until a mutation names one"
    );

    let mut tuned = params;
    tuned["schedules"][0]["emit_headers"]["chat_id"] = Value::String("-100123".into());
    let parsed = TimerParams::parse(&Value::Object(tuned)).expect("overridden params");
    assert_eq!(
        parsed.schedules[0]
            .emit_headers
            .get("chat_id")
            .and_then(Value::as_str),
        Some("-100123"),
        "the chat id an override names is not the one the schedule stamps"
    );
}

/// The DRIFT-LOCK for the scheduled shape. Every other knob class in this file
/// is pinned three times against one value -- `params`, `contract.settings` and
/// the literal in the script -- but a `timer` has no script, and the value it
/// reads is not a top-level param at all: it sits inside `params.schedules[0]`.
/// So the declaration and the schedule are two copies that nothing compared,
/// and a retune that moved one of them would have been silent: a reader of the
/// contract and the timer itself would be told different things, and the
/// contract is what an `override_params` author reads before writing one.
///
/// This walks `contract.settings` rather than a hand-written list, so a NEW
/// declared setting fails here until its schedule-side home is named -- the
/// point being that a setting nobody can locate inside the schedule is a
/// setting that documents nothing.
#[test]
fn every_declared_setting_of_the_three_clocks_is_the_literal_inside_the_schedule() {
    for cell in ["clock", "canvy/clock", "daily-digest/clock"] {
        let cfg = config(cell);
        let settings = cfg["contract"]["settings"]
            .as_object()
            .unwrap_or_else(|| panic!("{cell}: contract.settings"));
        assert!(
            !settings.is_empty(),
            "{cell}: a timer that declares no setting declares nothing about its tick"
        );
        let row = &cfg["params"]["schedules"][0];
        for (knob, spec) in settings {
            let declared = spec
                .get("default")
                .unwrap_or_else(|| panic!("{cell}: contract.settings.{knob}.default is missing"));
            let carried = schedule_side(row, knob).unwrap_or_else(|| {
                panic!(
                    "{cell}: contract.settings.{knob} has no home inside \
                     params.schedules[0] -- name it in `schedule_side` or the \
                     declaration documents nothing"
                )
            });
            assert_eq!(
                declared, &carried,
                "{cell}: contract.settings.{knob}.default and the literal inside \
                 params.schedules[0] disagree -- a reader of the contract and the \
                 timer would be told different things"
            );
        }
    }
}

/// Where a declared setting of a `timer` lives inside its one schedule row.
/// `None` means the name has no home there, which is the failure the test
/// above reports rather than skipping.
fn schedule_side(row: &Value, knob: &str) -> Option<Value> {
    match knob {
        // `argus/clock` calls its cadence `cycle_cron`; both names mean the
        // schedule's own `cron`, and naming both here keeps this helper the one
        // place the mapping is written down.
        "cron" | "cycle_cron" => Some(row["cron"].clone()),
        "schedule_id" => Some(row["schedule_id"].clone()),
        // The digest's URL rides inside the `tool_call` the schedule emits, as
        // the `url` argument of its JSON body -- one hop deeper than a plain
        // schedule field, which is exactly why it drifts unwatched.
        "digest_url" => {
            let text = row["emit_body"]["messages"][0]["text"].as_str()?;
            let args: Value = meclaw_core::serde_json::from_str(text).ok()?;
            Some(args.get("url")?.clone())
        }
        // Header-stamped values: the schedule carries them for the out-edge to
        // promote into context.
        "chat_id" => Some(row["emit_headers"]["chat_id"].clone()),
        _ => None,
    }
}

/// The typed half, proved through the REAL parsers rather than through the
/// JSON alone: a vault reads its key source and its credential NAME off params,
/// and a brain reads the grant it presents off params.
#[test]
fn the_vault_and_the_two_brains_parse_their_literals_and_an_override() {
    let shipped = config("vault")["params"].clone();
    let vault = VaultParams::parse(&shipped).expect("shipped vault params");
    assert_eq!(vault.key_source.as_str(), "auto");
    assert_eq!(vault.credential_name, "vault_key");

    let mut tuned = shipped;
    tuned["key_source"] = Value::String("systemd-cred".into());
    tuned["credential_name"] = Value::String("alex_vault".into());
    let vault = VaultParams::parse(&tuned).expect("overridden vault params");
    assert_eq!(
        vault.key_source.as_str(),
        "systemd-cred",
        "the key SOURCE an override names is not the one the vault reads"
    );
    assert_eq!(vault.credential_name, "alex_vault");

    for cell in ["talky/brain", "cogny/brain"] {
        let mut shipped = config(cell)["params"].clone();
        // The provider lane the ruling leaves alone, resolved the way the
        // colony would resolve it at boot.
        shipped["api_key"] = Value::String("sk-test".into());
        shipped["model"] = Value::String("openai/gpt-4o-mini".into());
        let brain = LlmParams::parse(&shipped).unwrap_or_else(|e| panic!("{cell}: {e}"));
        // The shipped grant is the EMPTY string, and the substrate reads that
        // as no grant at all: `llm/cell.rs` parks a turn only for a grant that
        // `filter(|g| !g.is_empty())` survives, so an unconfigured brain spends
        // its `api_key` exactly as it did under the environment form.
        assert_eq!(
            brain.credential_grant_id.as_deref(),
            Some(""),
            "{cell}: the shipped grant id is the empty string -- no grant at all"
        );

        let mut tuned = shipped;
        tuned["credential_grant_id"] = Value::String("grant-7".into());
        let brain = LlmParams::parse(&tuned).unwrap_or_else(|e| panic!("{cell}: {e}"));
        assert_eq!(
            brain.credential_grant_id.as_deref(),
            Some("grant-7"),
            "{cell}: the grant an override names is not the one the brain presents"
        );
    }
}

/// The sandbox roots, proved through the factories that own them: the shipped
/// literal validates, a root a mutation points somewhere else validates as that
/// place, and a bad one is still refused. Under the environment form the value
/// was the same for every instance of the template in a colony. The third root
/// of the strand, `coder-pipeline/fs`, is the twin's (GH #584).
#[test]
fn the_file_roots_are_literals_a_mutation_can_repoint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let elsewhere = dir.path().display().to_string();

    for (cell, factory) in [("tools/file", 0usize), ("tools/edit", 1usize)] {
        let validate = |p: &Value| -> Result<(), String> {
            if factory == 0 {
                FileCellFactory.validate_params(p)
            } else {
                EditCellFactory.validate_params(p)
            }
        };
        let mut params = config(cell)["params"].clone();
        // The FILESYSTEM half of the check is made against a directory this test
        // owns; what is pinned here is that the value the factory reads comes
        // from `params`, not from an environment.
        params["base_path"] = Value::String(elsewhere.clone());
        validate(&params).unwrap_or_else(|e| panic!("{cell}: a repointed root was refused: {e}"));

        params["base_path"] = Value::String(format!("{elsewhere}/does-not-exist"));
        assert!(
            validate(&params).is_err(),
            "{cell}: a root that does not exist must still be refused -- the \
             boundary is checked, not merely copied"
        );
    }
}
