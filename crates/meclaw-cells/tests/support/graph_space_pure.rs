//! The pure half of the graph space's `index` script, for the locks of GH
//! #945 that need no colony: the loader of `support/file_space_hive.rs`
//! (imports, defs and upper-case constants of the shipped `script_inline`, in
//! file order, under the cell's own params), then a python expression over
//! that scope with `ARGS` the test's argument. `SIM` adds a table-level
//! stand-in of the store -- select, insert, update, delete with eq / neq / in,
//! applied in bundle order -- so that a lock can run phase 1 and phase 2 of
//! two sources in any interleaving it names.
#![allow(dead_code)]

use meclaw_core::serde_json::{self as sj, Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

pub fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// R2b / GH #49: a tree without the template skips.
pub fn shipped() -> bool {
    repo("templates/graph-space/index/config.json").is_file()
}

/// The store stand-in, python, loaded beside the pure half.
pub const SIM: &str = r#"
# A table-level stand-in of the store for the interleaving lock: select,
# insert, update, delete with eq / neq / in, applied in bundle order. It is
# loaded beside the pure half of `index` (gh945_a_stale_pull_never_overwrites).
def _match(row, where):
    for k, v in (where or {}).items():
        x = row.get(k)
        if isinstance(v, dict):
            (op, arg), = v.items()
            if op == "eq" and x != arg: return False
            if op == "neq" and x == arg: return False
            if op == "in" and x not in arg: return False
        elif x != v:
            return False
    return True


def run_bundle(db, ops):
    out = {}
    for cid, op in ops:
        t = db.setdefault(op["table"], [])
        o = op["operation"]
        if o == "select":
            out[cid] = [{c: r.get(c) for c in op["columns"]} for r in t if _match(r, op.get("where"))][:op["limit"]]
        elif o == "insert":
            t.append(dict(op["row"]))
            out[cid] = []
        elif o == "update":
            for r in t:
                if _match(r, op.get("where")):
                    r.update(op["set"])
            out[cid] = []
        elif o == "delete":
            db[op["table"]] = [r for r in t if not _match(r, op.get("where"))]
            out[cid] = []
    return out


def p1_rows(out):
    keys = ("r-prev", "r-src", "r-own", "r-nodes", "r-into", "r-open", "r-anch")
    rows = {}
    for k in keys:
        rows[k] = []
        for cid in sorted(out):
            if cid == k or (cid.startswith(k) and cid[len(k):].isdigit()):
                rows[k].extend(out[cid])
    return rows


def source_row(S, path, lang, module, version):
    return {"source": S, "path": path, "lang": lang, "module": module, "version": "",
            "announced": version, "indexed_at": "", "tomb": ""}


def interleave(db, specs, order, stamp="T"):
    """`specs`: name -> (src, nodes, links); `order`: e.g. ['A1', 'B1', 'B2', 'A2'].
    Phase 1 runs `phase1_ops` against the db, phase 2 runs `phase2_ops` on what
    THAT phase 1 read. The clock of a run is `<stamp>1` / `<stamp>2`."""
    read = {}
    for step in order:
        name, ph = step[:-1], step[-1]
        src, nodes, links = specs[name]
        if ph == "1":
            read[name] = p1_rows(run_bundle(db, phase1_ops(src, clean_nodes(nodes), clean_links(links), [], stamp + "1")))
        else:
            rows = read[name]
            anchors = [r["anchor"] for r in rows["r-anch"]]
            run_bundle(db, phase2_ops(src["source"], anchors, rows, stamp + "2"))
    return db
"#;

const LOADER: &str = r#"
import ast, io, json, sys
inp = json.load(sys.stdin)
sys.stdin = io.StringIO("")
src, params = inp["src"], inp["params"]
keep = [n for n in ast.parse(src).body
        if isinstance(n, (ast.Import, ast.ImportFrom, ast.FunctionDef))
        or (isinstance(n, ast.Assign)
            and all(isinstance(t, ast.Name) and t.id.isupper() for t in n.targets))]
scope = {"P": params, "doc": {"params": params, "body": {}, "envelope": {}}}
for n in keep:
    exec(compile(ast.Module(body=[n], type_ignores=[]), "cell", "exec"), scope)
exec(compile(inp["extra"], "extra", "exec"), scope)
scope["ARGS"] = inp.get("args")
print(json.dumps(eval(inp["probe"], scope)))
"#;

/// The pure half of the shipped script of `cell` plus `extra` (python
/// statements, e.g. `SIM` and helper defs), and `probe` evaluated in it.
pub fn pure_with(cell: &str, extra: &str, probe: &str, args: Value) -> Value {
    pure_at(
        &format!("templates/graph-space/{cell}/config.json"),
        extra,
        probe,
        args,
    )
}

/// The same loader over any shipped code cell, by its `config.json` path
/// under the repo root -- for a lock that holds the graph against the rule
/// of its source (the file space's heading slug, OR-BC-55 (3)).
pub fn pure_at(rel: &str, extra: &str, probe: &str, args: Value) -> Value {
    let cell = rel;
    let p = repo(rel);
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let cfg: Value = sj::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let mut params = cfg["params"].as_object().cloned().unwrap_or_default();
    let src = params
        .remove("script_inline")
        .and_then(|v| v.as_str().map(str::to_string))
        .expect("script_inline");
    let doc = json!({"src": src, "params": params, "extra": extra, "probe": probe, "args": args});
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(LOADER)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(doc.to_string().as_bytes())
        .expect("write the document");
    drop(sink);
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "{cell}: the pure half does not load or the probe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    sj::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{cell}: not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

pub fn pure(cell: &str, probe: &str, args: Value) -> Value {
    pure_with(cell, "", probe, args)
}
