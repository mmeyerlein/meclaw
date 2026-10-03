//! The seed's decisions slug has the form the registry hand accepts (GH #976,
//! PE-DP-9 / D1 review E M-5).
//!
//! `templates/llm-registry/hand` refuses a model id that is not one token of a
//! provider's list (`MODEL_ID_MAX_CHARS`, `MODEL_ID_FORM`, `model_id_ok`). The
//! seed `store/seed/models.jsonl` ships a decisions row (`wire_dialect`
//! `decisions`); the D1 review found it pinned to a floating alias (`~...`),
//! which the hand would refuse the moment the row is pushed through it, and
//! which names no dated model either. This lock runs the hand's OWN check --
//! extracted from the shipped `script_inline` by python's `ast`, never copied --
//! over every `model_id` of the seed, and proves the check bites on the
//! floating form.
//!
//! The slug `typesafe/jev-1.13` was checked on 2026-10-03 against the
//! provider's free, keyless model listing: `GET /api/v1/models/<slug>/endpoints`
//! answers 200 with the dated endpoint `typesafe/jev-1.13-20260917` and a
//! context of 32000 tokens. The chat list `GET /api/v1/models` does not list
//! decisions models, so it is no evidence either way.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const HAND: &str = "templates/llm-registry/hand/config.json";
const SEED: &str = "templates/llm-registry/store/seed/models.jsonl";

/// The floating form the pin replaced; the hand must refuse it.
const FLOATING: &str = "~typesafe/jev-latest";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Pulls `MODEL_ID_MAX_CHARS`, `MODEL_ID_FORM` and `model_id_ok` out of the
/// hand's script with `ast` (plus `import re`), runs them alone, and answers
/// the verdict for every seed row and every probe.
const DRIVER: &str = r#"
import ast, json, sys
doc = json.load(sys.stdin)
tree = ast.parse(doc["script"])
keep = []
for node in tree.body:
    if isinstance(node, ast.Assign) and any(
            isinstance(t, ast.Name) and t.id in ("MODEL_ID_MAX_CHARS", "MODEL_ID_FORM")
            for t in node.targets):
        keep.append(node)
    elif isinstance(node, ast.FunctionDef) and node.name == "model_id_ok":
        keep.append(node)
names = sorted({t.id for n in keep if isinstance(n, ast.Assign) for t in n.targets}
               | {n.name for n in keep if isinstance(n, ast.FunctionDef)})
ns = {}
exec(compile(ast.Module(body=ast.parse("import re").body + keep, type_ignores=[]), "hand", "exec"), ns)
rows = []
for i, line in enumerate(doc["seed"].splitlines()):
    if not line.strip():
        continue
    r = json.loads(line)
    if "schema" in r:
        continue
    mid = r.get("model_id")
    rows.append({"model_id": mid, "wire_dialect": r.get("wire_dialect"),
                 "ok": isinstance(mid, str) and bool(ns["model_id_ok"](mid))})
sys.stdout.write(json.dumps({"found": names, "max": ns["MODEL_ID_MAX_CHARS"], "rows": rows,
                             "probes": {p: bool(ns["model_id_ok"](p)) for p in doc["probes"]}}))
"#;

fn have_python() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn drive(script: &str, seed: &str, probes: &[&str]) -> Value {
    let doc = json!({"script": script, "seed": seed, "probes": probes});
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(DRIVER)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3 runs");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(doc.to_string().as_bytes())
        .expect("write the document");
    drop(sink);
    let out = child.wait_with_output().expect("wait for python3");
    assert!(
        out.status.success(),
        "the driver failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("the driver answers JSON")
}

#[test]
fn seed_slug_has_the_model_id_form() {
    let (Ok(hand), Ok(seed)) = (
        std::fs::read_to_string(repo(HAND)),
        std::fs::read_to_string(repo(SEED)),
    ) else {
        // The template library does not travel in the published tree (R2b).
        return;
    };
    if !have_python() {
        return;
    }
    let v: Value = serde_json::from_str(&hand).expect("hand config is JSON");
    let script = meclaw_testing::resolve_script_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("hand params.script_inline"),
    );
    let got = drive(&script, &seed, &[FLOATING]);

    assert_eq!(
        got["found"],
        json!(["MODEL_ID_FORM", "MODEL_ID_MAX_CHARS", "model_id_ok"]),
        "the hand still defines its model-id check under these names: {got}"
    );
    let rows = got["rows"].as_array().expect("rows");
    assert!(!rows.is_empty(), "the seed carries model rows: {got}");
    for r in rows {
        assert_eq!(
            r["ok"], true,
            "every seeded model_id has the form the hand accepts: {r}"
        );
    }

    let decisions: Vec<&Value> = rows
        .iter()
        .filter(|r| r["wire_dialect"] == "decisions")
        .collect();
    assert_eq!(decisions.len(), 1, "exactly one decisions row: {got}");
    let slug = decisions[0]["model_id"].as_str().expect("a model_id");
    assert!(
        !slug.contains('~'),
        "the decisions row is a listed slug, not a floating alias: {slug}"
    );
    assert_eq!(slug, "typesafe/jev-1.13", "the slug checked on 2026-10-03");

    // The pin bites: the floating form is refused by the same function.
    assert_eq!(
        got["probes"][FLOATING], false,
        "the hand refuses the floating form {FLOATING}: {got}"
    );
}
