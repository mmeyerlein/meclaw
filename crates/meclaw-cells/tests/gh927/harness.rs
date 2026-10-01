//! GH #927 — the two ways the table tests of this lock run a shipped argus script.
//!
//! [`call`] loads the script's function definitions and calls ONE of them with
//! JSON arguments, which is how a pure function of a `script_inline` is tested
//! without a colony: the script is executed over a document that makes it exit
//! at its first early return, and every `def` above the main flow is in the
//! namespace by then. That is a rule the two scripts keep on purpose — every
//! pure function is defined before `d = doc["body"]`.
//!
//! [`run`] is the whole script over one document, the way `gh155_argus_loop.rs`
//! runs it: what it prints is what the cell would have emitted.
//!
//! Both hand the program to python3 on stdin, never in argv: a single argv
//! string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the scripts are near it
//! (GH #279).

#![allow(dead_code)]

use meclaw_core::serde_json::{self, Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

pub const METER: &str = "../../templates/argus/meter/config.json";
pub const MUTATOR: &str = "../../templates/argus/mutator/config.json";
pub const HIVE: &str = "../../templates/argus/config.json";

/// The template ships in this checkout (GH #49 / R2b), and python3 spawns.
pub fn can_run() -> bool {
    if !std::path::Path::new(HIVE).is_file() {
        eprintln!("argus did not travel into this tree -- skipped (GH #49)");
        return false;
    }
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("no python3 -- skipped");
        return false;
    }
    true
}

pub fn config(path: &str) -> Value {
    let raw = std::fs::read_to_string(path).expect("template config");
    serde_json::from_str(&raw).expect("config json")
}

/// The shipped script with its `${…}` tokens resolved the way the colony
/// resolves them at instantiation.
pub fn script(path: &str) -> String {
    meclaw_testing::resolve_script_vars(
        config(path)["params"]["script_inline"]
            .as_str()
            .expect("a script_inline"),
    )
}

/// The cell's shipped params without the script — what the substrate hands
/// the script as `doc["params"]`.
pub fn shipped_params(path: &str) -> Value {
    let mut p = config(path)["params"].clone();
    if let Some(o) = p.as_object_mut() {
        o.remove("script_inline");
    }
    p
}

fn python(src: &str) -> Vec<u8> {
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "python exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// A document that sends either script out at its first early return: a store
/// echo in phase `written`, which both scripts answer with `[]`.
fn quiet_doc(params: &Value) -> Value {
    json!({
        "envelope": {"header": {"hop": {"operation": "insert"},
                                "context": {"ar_phase": "written"}}},
        "body": {"messages": []},
        "params": params
    })
}

/// Call `func(*args)` of the shipped script at `path` and return its result as
/// JSON. A Python tuple comes back as an array.
pub fn call(path: &str, func: &str, args: Value) -> Value {
    call_with_params(path, &shipped_params(path), func, args)
}

/// The same, with the cell's params replaced — for a knob under test.
pub fn call_with_params(path: &str, params: &Value, func: &str, args: Value) -> Value {
    let src = format!(
        concat!(
            "import sys, io, json\n",
            "_script = {script}\n",
            "_doc = json.loads({doc})\n",
            "_args = json.loads({args})\n",
            "_g = {{'__name__': '__main__'}}\n",
            "_real = sys.stdout\n",
            "sys.stdin = io.StringIO(json.dumps(_doc))\n",
            "sys.stdout = io.StringIO()\n",
            "try:\n",
            "    exec(compile(_script, 'cell', 'exec'), _g)\n",
            "except SystemExit:\n",
            "    pass\n",
            "sys.stdout = _real\n",
            "_r = _g[{func}](*_args)\n",
            "sys.stdout.write(json.dumps(_r))\n"
        ),
        script = serde_json::to_string(&script(path)).unwrap(),
        doc = serde_json::to_string(&quiet_doc(params).to_string()).unwrap(),
        args = serde_json::to_string(&args.to_string()).unwrap(),
        func = serde_json::to_string(func).unwrap(),
    );
    let out = python(&src);
    serde_json::from_slice(&out)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out)))
}

/// Run the whole shipped script over one flat document (`header`, body slots,
/// optionally `params`) and collect what it emitted.
pub fn run(path: &str, flat: Value) -> Vec<Value> {
    let mut flat = flat;
    if flat.get("params").is_none() {
        flat["params"] = shipped_params(path);
    }
    let stdin_doc = meclaw_testing::code_stdin(&flat).to_string();
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(&script(path)).unwrap(),
        serde_json::to_string(&stdin_doc).unwrap(),
    );
    let out = python(&src);
    serde_json::from_slice(&out)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out)))
}

/// The emissions on one route.
pub fn on_route<'a>(out: &'a [Value], route: &str) -> Vec<&'a Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == route)
        .collect()
}

/// The store call (`rstore`/`cstore` tool_call arguments) of that operation on
/// that table, if the script wrote one.
pub fn store_call(out: &[Value], operation: &str, table: &str) -> Option<Value> {
    out.iter().find_map(|m| {
        let text = m["messages"][0]["text"].as_str()?;
        let args: Value = serde_json::from_str(text).ok()?;
        (args["operation"] == operation && args["table"] == table).then_some(args)
    })
}

/// Every store call of that operation on that table.
pub fn store_calls(out: &[Value], operation: &str, table: &str) -> Vec<Value> {
    out.iter()
        .filter_map(|m| {
            let text = m["messages"][0]["text"].as_str()?;
            let args: Value = serde_json::from_str(text).ok()?;
            (args["operation"] == operation && args["table"] == table).then_some(args)
        })
        .collect()
}

/// A store answer carrying `rows`, resumed in `phase` with `carry` as the
/// context carry — the shape `./receipts` or `./charter` sends back.
pub fn store_answer(rows: Value, phase: &str, carry: Value) -> Value {
    json!({
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c1",
                      "text": rows.to_string()}],
        "header": {"hop": {"operation": "select"},
                   "context": {"ar_phase": phase, "ar_carry": carry.to_string()}}
    })
}

/// Every emission passes the cell's own declared `emits` contract — a message
/// the declaration refuses never leaves the cell.
pub fn assert_the_declaration_admits(path: &str, out: &[Value]) {
    let cfg = config(path);
    let block: meclaw_colony::config::ContractBlock =
        serde_json::from_value(cfg["contract"].clone()).expect("the contract block parses");
    let compiled =
        meclaw_core::CompiledEmits::compile(&block.emits).expect("the emits schemas compile");
    assert!(!out.is_empty(), "nothing was emitted at all");
    for m in out {
        meclaw_core::validate_emits(m, &compiled)
            .unwrap_or_else(|e| panic!("the cell's own contract refuses what it emits: {e} — {m}"));
    }
}
