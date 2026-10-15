//! GH #425 — R6's fast lane: predefined parameterised recipes, rendered without
//! a model and without a network, sub-second. Sub-second is the development
//! target of the perf stage; what this file proves is its cause -- the render
//! reaches out to nothing, neither a socket nor a child process.
//!
//! What is under test is not speed for its own sake. It is that the trivial
//! class of structural wish — rewire an edge, hang a drain, grow a cell and
//! wire it in — never reaches an inference at all, and that the ORDER of the
//! declarations it produces is semantics rather than taste: a manifest rolls
//! forward and stops at the first refusal, with NO ROLLBACK (R5).

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{code_stdin, emit_all, run_shipped_script, shipped_script};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

/// The MANIFEST emission of a recipe.
///
/// A finished recipe emits a second message whenever the context names a caller:
/// the pair of rows the fast lane parks so that a refusal coming back on the
/// submitter's foreign chain can find the build's door again (GH #480). It is a
/// store bundle on route `bind` and has no business in this file; the contexts
/// here name nobody, so it does not arise.
fn run_recipes(payload: Value) -> Value {
    let all = emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": payload.to_string()}],
        }),
    );
    all.into_iter()
        .find(|m| m["header"]["operation"] == "recipe")
        .expect("the recipe answers with a manifest emission")
}

/// The digest, computed INDEPENDENTLY of the script under test: the same
/// canonical bytes, hashed by a python that never saw the shipped source. A
/// helper that drifted would agree with itself; this does not.
fn sha256_of_canonical(manifest: &Value) -> String {
    let program = concat!(
        "import sys, json, hashlib\n",
        "d = json.load(sys.stdin)\n",
        "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
        "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
    );
    let out = run_shipped_script(program, &manifest.to_string());
    assert!(out.status.success(), "digest helper failed");
    String::from_utf8(out.stdout).expect("hex")
}

#[test]
fn rewire_edge_removes_the_old_edge_before_it_draws_the_new_one() {
    let out = run_recipes(json!({"recipe": "rewire_edge", "request": "…",
        "params": {"scope": "/os", "from": "./a", "to": "./b", "old_to": "./c",
                   "condition": "has(hop.route) && hop.route == 'note'"}}));
    let decls = out["manifest"]
        .as_array()
        .expect("the manifest is an array");
    assert_eq!(
        decls.len(),
        2,
        "remove first, add second — a manifest has no rollback, so an add that \
         lands before the remove leaves BOTH edges live if the remove is refused"
    );
    assert!(decls[0]["diff"]["remove_edges"].is_array());
    assert!(decls[1]["diff"]["add_edges"].is_array());
    assert_eq!(
        decls[0]["diff"]["remove_edges"][0]["match"]["to"],
        json!("./c"),
        "remove_edges takes a `match` pattern, not a bare edge (docs/rewiring.md)"
    );
    assert_eq!(
        decls[1]["diff"]["add_edges"][0]["condition"],
        json!("has(hop.route) && hop.route == 'note'")
    );
    assert_eq!(out["header"]["manifest_class"], json!("fast"));
    assert_eq!(out["header"]["declaration_count"], json!(2));
}

#[test]
fn the_digest_is_over_the_canonical_bytes_of_the_manifest() {
    let out = run_recipes(json!({"recipe": "attach_drain", "request": "…",
        "params": {"scope": "/os", "from": "./x", "to": "./y", "route": "error"}}));
    let want = sha256_of_canonical(&out["manifest"]);
    assert_eq!(out["header"]["manifest_sha256"], json!(want));
}

#[test]
fn a_grown_cell_is_wired_in_by_the_same_recipe_that_grew_it() {
    let out = run_recipes(json!({"recipe": "add_node", "request": "…",
        "params": {"scope": "/os", "name": "digest", "template": "summarizer",
                   "target": "./notes", "route": "tick"}}));
    let decls = out["manifest"].as_array().expect("array");
    assert_eq!(
        decls.len(),
        2,
        "a node nobody can reach is not a growth, it is a directory"
    );
    assert_eq!(
        decls[0]["diff"]["add_nodes"][0]["template"],
        json!("summarizer")
    );
    assert_eq!(decls[1]["diff"]["add_edges"][0]["from"], json!("./digest"));
}

/// Put in front of a script: every way out of the process -- a socket that
/// connects or resolves, a child process -- writes [`REACHED_OUT`] to stderr and
/// fails. A model call is one of those ways, so a render that passes under it
/// never reached an inference.
const NO_WAY_OUT: &str = concat!(
    "import socket as _so, subprocess as _sp, os as _os, sys as _sy\n",
    "def _nope(what):\n",
    "    def f(*a, **k):\n",
    "        _sy.stderr.write('REACHED-OUT: %s\\n' % what)\n",
    "        _sy.stderr.flush()\n",
    "        raise PermissionError('the fast lane reached out: ' + what)\n",
    "    return f\n",
    "for _n in ('connect', 'connect_ex', 'sendto', 'sendmsg'):\n",
    "    setattr(_so.socket, _n, _nope('socket.' + _n))\n",
    "for _n in ('create_connection', 'getaddrinfo', 'gethostbyname'):\n",
    "    setattr(_so, _n, _nope('socket.' + _n))\n",
    "_sp.Popen.__init__ = _nope('subprocess')\n",
    "for _n in ('system', 'popen', 'fork', 'forkpty', 'posix_spawn', 'posix_spawnp',\n",
    "           'execv', 'execve', 'execvp', 'execvpe', 'spawnv', 'spawnve'):\n",
    "    if hasattr(_os, _n):\n",
    "        setattr(_os, _n, _nope('os.' + _n))\n",
);

/// The stderr mark of a way out [`NO_WAY_OUT`] caught.
const REACHED_OUT: &str = "REACHED-OUT:";

/// `script` behind [`NO_WAY_OUT`] over the recipe payload: exit status, the
/// emissions, stderr.
fn run_sealed(script: &str, payload: Value) -> (bool, Vec<Value>, String) {
    let doc = code_stdin(&json!({
        "target": "/os/builder/recipes",
        "header": {"hop": {"route": "recipe"}, "context": {}},
        "ttl": 64,
        "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                      "text": payload.to_string()}],
    }));
    let out = run_shipped_script(&format!("{NO_WAY_OUT}{script}"), &doc.to_string());
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let emitted = match meclaw_core::serde_json::from_slice::<Value>(&out.stdout) {
        Ok(Value::Array(a)) => a,
        Ok(other) => vec![other],
        Err(_) => Vec::new(),
    };
    (out.status.success(), emitted, stderr)
}

/// R6's sub-second claim rests on this: the fast lane renders with every way
/// out of the process shut -- no network, no child, so no model -- and still
/// answers its manifest. The duration itself is a development target of the
/// perf stage, never a pass/fail here (development-rules § 11).
#[test]
fn the_fast_lane_answers_in_well_under_a_second() {
    let (ok, emitted, stderr) = run_sealed(
        &shipped_script(RECIPES),
        json!({"recipe": "attach_drain", "request": "…",
               "params": {"scope": "/os", "from": "./x", "to": "./y", "route": "error"}}),
    );
    assert!(
        !stderr.contains(REACHED_OUT),
        "the fast lane reached out of its process -- a network or a model on the \
         lane R6 keeps free of both: {stderr}"
    );
    assert!(ok, "the sealed render exited non-zero: {stderr}");
    let out = emitted
        .iter()
        .find(|m| m["header"]["operation"] == "recipe")
        .unwrap_or_else(|| panic!("the recipe answers with a manifest emission: {emitted:?}"));
    assert_eq!(out["header"]["manifest_class"], json!("fast"));
    assert!(out["manifest"].is_array(), "{out}");
    assert!(
        out["header"].get("tokens_prompt").is_none(),
        "no model, so no tokens on the hop: {out}"
    );
}

/// The seal of [`NO_WAY_OUT`] holds: a script that does reach out -- an HTTP
/// call, a child -- is caught and named, so a silent fast lane is proof.
#[test]
fn the_seal_catches_a_call_out_and_a_child() {
    for (what, script) in [
        (
            "socket",
            "import urllib.request\nurllib.request.urlopen('http://127.0.0.1:9/', timeout=60)\n",
        ),
        (
            "subprocess",
            "import subprocess\nsubprocess.run(['true'])\n",
        ),
    ] {
        let (ok, _, stderr) = run_sealed(script, json!({}));
        assert!(
            stderr.contains(REACHED_OUT) && stderr.contains(what),
            "{what}: the seal let it through: {stderr}"
        );
        assert!(!ok, "{what}: a caught way out fails the script");
    }
}

#[test]
fn a_recipe_this_cell_cannot_render_is_named_rather_than_guessed() {
    // Unreachable behind `classify`, and here anyway: a cell knows no topology
    // and must not build on who stands in front of it.
    let out = run_recipes(json!({"recipe": "teleport", "params": {}}));
    assert_eq!(out["header"]["error_code"], json!("recipe_unknown"));
    assert!(
        out.get("manifest").is_none(),
        "a refusal ships no manifest — an empty one is a failure wearing the face \
         of an honest answer (GH #308)"
    );
}
