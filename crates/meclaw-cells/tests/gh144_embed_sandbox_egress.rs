//! GH #144 -- the memory-hive's embed cell declares the egress it lives on.
//!
//! # Why a template needs a `sandbox` block at all
//!
//! GH #85 injects `{"trust": "restricted", "network": "deny", "filesystem":
//! {"runtime": true}}` into every template-sourced `code` cell that declares
//! none. That default is right for the fourteen cells of this hive that only
//! shuffle JSON -- and fatal for the ONE whose entire job is an HTTPS call. A
//! freshly instantiated hive could therefore never build its semantic leg:
//! 953/953 embedding calls answered "endpoint unreachable" at `exit_code: 0`,
//! the retrieval fan silently degraded to three legs, and only long-running
//! colonies whose trees predate the cut kept working -- which is exactly why
//! production never showed it.
//!
//! The fix is a declaration, not an escape hatch: `trust` stays `restricted`,
//! the filesystem view stays the bare runtime set, and only `network` opens.
//! `trust: "trusted"` would have worked too and would have switched off
//! Landlock, the cgroup caps and the seccomp filter for a cell that runs a
//! shipped script against a remote endpoint -- the widest possible answer to
//! the narrowest possible need.
//!
//! # What is proven here
//!
//! 1. the shipped `config.json` carries the block, so the GH #85 injection
//!    finds the hole already filled (`a_declared_profile_is_never_overwritten`
//!    in `meclaw-colony/tests/gh85_template_default_deny.rs` is the other half);
//! 2. the REAL shipped script, under the REAL shipped profile, reaches a local
//!    HTTP endpoint and comes back undegraded. Local, never the paid endpoint:
//!    the property under test is "the boundary lets the call out", and that is
//!    the same syscall sequence whoever answers.

use meclaw_colony::StatelessCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use tokio::sync::mpsc;

const EMBED_CONFIG: &str = "../../templates/memory-hive/embed/config.json";
const GRAPH_INDEX_CONFIG: &str = "../../templates/graph-space/index/config.json";

fn embed_config() -> Value {
    let raw = std::fs::read_to_string(EMBED_CONFIG).expect("embed config");
    meclaw_core::serde_json::from_str(&raw).expect("embed config json")
}

/// The shipped script with `MEMORY_EMBED_ENDPOINT` bound to `endpoint`; every
/// other `${VAR:-default}` collapses to its default and every bare `${VAR}` to
/// the empty string -- the substitution the colony performs at instantiation.
/// (Same helper as `embed_token_accounting.rs`: the point of both tests is that
/// the SHIPPED script runs, never a copy of it.)
fn embed_script(endpoint: &str) -> String {
    let cfg = embed_config();
    let script = cfg["params"]["script_inline"].as_str().expect("script");
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        let (name, default) = match tail[..end].split_once(":-") {
            Some((n, d)) => (n, d),
            None => (&tail[..end], ""),
        };
        if name == "MEMORY_EMBED_ENDPOINT" {
            out.push_str(endpoint);
        } else {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// True when this kernel can enforce a filesystem allow-list. Without it the
/// `restricted` profile fails the spawn by design (fail-closed), so the live
/// half of this file has nothing to measure and says so.
fn have_landlock(test: &str) -> bool {
    match meclaw_cells::sandbox::landlock_abi() {
        Some(abi) => {
            eprintln!("[{test}] landlock abi {abi}");
            true
        }
        None => {
            eprintln!("[{test}] SKIPPED: no Landlock on this kernel (needs Linux 5.13+)");
            false
        }
    }
}

fn sink_for(otx: mpsc::Sender<CellEmission>, path: &str) -> OutputSink {
    OutputSink::new(
        otx,
        Path::new(path),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    )
}

/// Run the embed cell over `args` as a tool call and return its single
/// emission. `multi_send_capable` is `true` because the shipped contract
/// declares it.
async fn run_embed(script: &str, sandbox: Value, args: Value) -> CellEmission {
    let params = json!({
        "runner": "python3",
        "script_inline": script,
        "external_timeout_ms": 20000,
        "sandbox": sandbox,
    });
    let parsed = meclaw_cells::code::CodeParams::parse(&params).expect("params parse");
    let cell = meclaw_cells::code::CodeCell::new(parsed, true, None, false);
    let (otx, mut orx) = mpsc::channel(8);
    let sink = sink_for(otx, "/memory/embed");
    let msg = MessageBuilder::new(Path::new("/memory/embed"))
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant", "type": "tool_call", "id": "e-in",
            "text": args.to_string()
        }]})))
        .reply_to(Path::new("/memory/recall"))
        .build();
    cell.handle(msg, &sink).await;
    drop(sink);
    orx.recv().await.expect("the read lane always answers once")
}

/// An OpenAI-compatible embeddings response with one vector.
fn one_vector_response() -> MockResponse {
    MockResponse::ok_json(
        json!({"object": "list", "model": "mock-embed",
               "data": [{"object": "embedding", "index": 0,
                         "embedding": [0.5, -0.5, 0.5, -0.5]}],
               "usage": {"prompt_tokens": 7, "total_tokens": 7}})
        .to_string()
        .as_bytes(),
    )
}

// ---- the declaration ------------------------------------------------------

#[test]
fn the_embed_cell_declares_the_narrowest_profile_that_can_call_out() {
    let cfg = embed_config();
    let sb = &cfg["params"]["sandbox"];
    assert_eq!(
        sb,
        &json!({"trust": "restricted", "network": "allow", "filesystem": {"runtime": true}}),
        "the one cell of the hive whose job is an HTTPS call must say so, and must say no \
         more than that: restricted trust, the bare runtime set, egress open"
    );
    assert_ne!(
        sb["trust"], "trusted",
        "trusted would switch off Landlock, the caps and the seccomp filter for a cell that \
         needs exactly one of the four axes opened"
    );
    assert!(
        sb["filesystem"].get("read").is_none() && sb["filesystem"].get("write").is_none(),
        "a template that names a host path bakes somebody else's machine into every \
         instantiated tree (GH #20), got {sb}"
    );
}

#[test]
fn every_shipped_cell_that_speaks_the_network_declares_a_profile() {
    // The sweep of GH #144, kept as a test rather than as a paragraph in a
    // receipt: a template cell that performs network I/O and does not open
    // `network` inherits `"deny"` at instantiation and is dead on arrival.
    // Today the two embed cells are the only ones -- the assertion is that the
    // next one does not slip in silently.
    // Assembled from the manifest dir rather than written as one literal: the
    // walk covers whatever library the checkout carries (the public clone
    // carries the exported subset), and the export's R2b check reads path
    // literals to decide whether a test reaches for a template that does not
    // travel. This one reaches for the directory, not for a template.
    let library = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("templates");
    let speakers = silent_speakers(&library);
    assert!(
        speakers.is_empty(),
        "these shipped cells perform network I/O without `network: \"allow\"` (or \
         `trust: \"trusted\"`) and are cut off at instantiation (GH #144): {speakers:?}"
    );
}

/// What a script calls to reach the network, as whole words: the stdlib and
/// the common client libraries, raw sockets, and the two shell clients. A
/// module that only parses (`urllib.parse`) is no call -- the sweep counted
/// every `urllib` once, and a cell that parses URLs passed only because its
/// sandbox block happened to be there (C1 finding, review of GH #944).
const NETWORK_CALLS: &[&str] = &[
    "urlopen",
    "urllib.request",
    "urllib3",
    "http.client",
    "HTTPConnection",
    "HTTPSConnection",
    "import requests",
    "from requests",
    "requests.get",
    "requests.post",
    "requests.put",
    "requests.patch",
    "requests.delete",
    "requests.head",
    "requests.request",
    "requests.Session",
    "httpx",
    "aiohttp",
    "import socket",
    "socket.socket",
    "create_connection",
    "curl",
    "wget",
];

/// The code of a script with its comments cut. `#` opens a comment where it
/// opens a word (line start, or after a blank) outside a quote -- the rule
/// Python and the shell share. Each line on its own: a quote left open (a
/// triple quote, a shell oddity) keeps the rest of its line, so the cut can
/// only ever keep too much, never hide a call.
fn without_comments(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    for line in script.lines() {
        let mut quote: Option<char> = None;
        let mut escaped = false;
        let mut prev: Option<char> = None;
        let mut cut = line.len();
        for (i, c) in line.char_indices() {
            if let Some(q) = quote {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            } else if c == '\'' || c == '"' {
                quote = Some(c);
            } else if c == '#' && matches!(prev, None | Some(' ') | Some('\t')) {
                cut = i;
                break;
            }
            prev = Some(c);
        }
        out.push_str(&line[..cut]);
        out.push('\n');
    }
    out
}

/// Whether `code` holds `word` with no identifier character on either side.
fn has_word(code: &str, word: &str) -> bool {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    code.match_indices(word).any(|(i, _)| {
        !code[..i].chars().next_back().is_some_and(ident)
            && !code[i + word.len()..].chars().next().is_some_and(ident)
    })
}

/// The first network call the code of `script` makes, if any. Besides the
/// words of [`NETWORK_CALLS`], a `from urllib import` line that names
/// `request` counts: its calls then read `request.Request(…)` or
/// `request.build_opener().open(…)` and name no listed word (the old rule
/// caught that form through the bare `urllib`, review of the C1 finding I-2).
fn network_call(script: &str) -> Option<&'static str> {
    let code = without_comments(script);
    if let Some(w) = NETWORK_CALLS.iter().copied().find(|w| has_word(&code, w)) {
        return Some(w);
    }
    code.lines()
        .filter_map(|l| l.trim_start().strip_prefix("from urllib import"))
        .any(|names| has_word(names, "request"))
        .then_some("from urllib import request")
}

/// Whether a cell's sandbox block leaves egress open: `restricted` with
/// `network: "allow"`, or `trusted` -- the escape hatch without enforcement,
/// under which the parser forbids a `network` key at all
/// (`sandbox/profile.rs`, `mutation/stage.rs` `default_sandbox_block`).
fn opens_network(sandbox: &Value) -> bool {
    sandbox["network"] == "allow" || sandbox["trust"] == "trusted"
}

/// Every shipped cell beneath `root` that calls the network but does not
/// open it ([`opens_network`]), as `<path> (<call>)`. A sandbox block that
/// keeps `deny` is no excuse: under it the call fails like under the injected
/// default.
fn silent_speakers(root: &std::path::Path) -> Vec<String> {
    let mut speakers = Vec::new();
    for entry in walk(root) {
        let raw = match std::fs::read_to_string(&entry) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let cfg: Value = match meclaw_core::serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !matches!(
            cfg["cell"]["type"].as_str(),
            Some("code") | Some("bash") | Some("harness")
        ) {
            continue;
        }
        let script = [
            cfg["params"]["script_inline"].as_str(),
            cfg["params"]["command"].as_str(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
        if let Some(call) = network_call(&script)
            && !opens_network(&cfg["params"]["sandbox"])
        {
            speakers.push(format!("{} ({call})", entry.display()));
        }
    }
    speakers.sort();
    speakers
}

#[test]
fn the_sweep_counts_calls_not_words() {
    let cases: &[(&str, Option<&str>)] = &[
        // parsing is not calling, and prose is not code
        ("import urllib.parse\nq = urllib.parse.quote(x)\n", None),
        ("# urlopen would be the wrong tool here\nx = 1\n", None),
        ("x = 1  # never requests.post from here\n", None),
        ("echo done # curl comes later\n", None),
        ("def my_urlopen_helper():\n    return xcurl\n", None),
        // the calls themselves
        (
            "from urllib.request import urlopen\nurlopen(u)\n",
            Some("urlopen"),
        ),
        ("import urllib.request\n", Some("urllib.request")),
        ("r = requests.post(u, json=b)\n", Some("requests.post")),
        ("import socket\n", Some("import socket")),
        (
            "conn = http.client.HTTPSConnection(h)\n",
            Some("http.client"),
        ),
        ("curl -s https://example.invalid/\n", Some("curl")),
        // the module imported by name, its calls then read `request.…`
        (
            "from urllib import request\nreq = request.Request(u)\n\
             request.build_opener().open(req)\n",
            Some("from urllib import request"),
        ),
        (
            "from urllib import parse, request as rq\n",
            Some("from urllib import request"),
        ),
        ("from urllib import parse\nparse.quote(x)\n", None),
        // a `#` inside a quote opens no comment
        ("s = 'a # b'; urlopen(s)\n", Some("urlopen")),
        ("u = \"x#y\" ; requests.get(u)\n", Some("requests.get")),
    ];
    for (script, want) in cases {
        assert_eq!(network_call(script), *want, "{script:?}");
    }
}

#[test]
fn a_cell_that_only_parses_urls_needs_no_excuse() {
    // The graph index imports `urllib.parse` and nothing that reaches out: it
    // is no speaker, whatever its sandbox says. Before the sweep was sharpened
    // it passed only because it carries a sandbox block (with `network:
    // "deny"`); the rule now asks for `allow`, so the block is no excuse.
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(GRAPH_INDEX_CONFIG).expect("graph index config"),
    )
    .expect("graph index config json");
    let script = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        script.contains("urllib.parse"),
        "the case this lock is about"
    );
    assert_eq!(network_call(script), None);
}

#[test]
fn a_real_call_without_allow_stays_red() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cell = |name: &str, sandbox: Value| {
        let d = dir.path().join(name);
        std::fs::create_dir_all(&d).expect("cell dir");
        let mut params = json!({
            "script_inline": "# fetch it\nfrom urllib.request import urlopen\n\
                              print(urlopen('http://127.0.0.1:9/').read())\n"
        });
        if !sandbox.is_null() {
            params["sandbox"] = sandbox;
        }
        std::fs::write(
            d.join("config.json"),
            json!({"cell": {"type": "code"}, "params": params}).to_string(),
        )
        .expect("config");
    };
    cell("bare", Value::Null);
    cell("denied", json!({"trust": "restricted", "network": "deny"}));
    // `trusted` is the escape hatch without enforcement: nothing is cut off,
    // and the parser forbids a `network` key under it.
    cell("trusted", json!({"trust": "trusted"}));
    cell(
        "allowed",
        json!({"trust": "restricted", "network": "allow"}),
    );
    let found = silent_speakers(dir.path());
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found
            .iter()
            .any(|f| f.contains("bare") && f.ends_with("(urlopen)"))
    );
    assert!(found.iter().any(|f| f.contains("denied")));
    assert!(!found.iter().any(|f| f.contains("allowed")));
    assert!(!found.iter().any(|f| f.contains("trusted")));
}

/// Every `config.json` beneath `root`.
fn walk(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().and_then(|n| n.to_str()) == Some("config.json") {
                out.push(p);
            }
        }
    }
    out
}

// ---- the live proof -------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_script_reaches_an_endpoint_under_the_shipped_profile() {
    const T: &str = "the_shipped_script_reaches_an_endpoint_under_the_shipped_profile";
    if !have_landlock(T) {
        return;
    }
    let (addr, _join, _cap) = start_mock_server_capturing(vec![one_vector_response()]).await;
    let script = embed_script(&format!("http://{addr}/v1/embeddings"));
    let sandbox = embed_config()["params"]["sandbox"].clone();

    let em = run_embed(
        &script,
        sandbox,
        json!({"query": {"text": "what does the user eat", "recall_id": "r1"}}),
    )
    .await;

    assert_eq!(em.content["header"]["route"], "equery");
    let body: Value =
        meclaw_core::serde_json::from_str(em.content["messages"][0]["text"].as_str().unwrap())
            .expect("query body json");
    assert_eq!(
        body["degraded"], false,
        "the call must leave the sandbox; a degraded answer here is the GH #144 defect \
         (error {:?})",
        body["error"]
    );
    assert!(
        body["vector"].is_string(),
        "and it must come back with a vector, got {body}"
    );
}

/// The counter-proof, so the test above cannot pass for the wrong reason: the
/// SAME script and the SAME endpoint under the profile GH #85 would have
/// injected. Degraded, every time -- which is precisely what a fresh hive did.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_injected_default_would_still_strangle_it() {
    const T: &str = "the_injected_default_would_still_strangle_it";
    if !have_landlock(T) || !meclaw_cells::sandbox::network_isolation_supported() {
        eprintln!("[{T}] SKIPPED: this host cannot enforce network \"deny\"");
        return;
    }
    let (addr, _join, _cap) = start_mock_server_capturing(vec![one_vector_response()]).await;
    let script = embed_script(&format!("http://{addr}/v1/embeddings"));

    let em = run_embed(
        &script,
        json!({"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}}),
        json!({"query": {"text": "what does the user eat", "recall_id": "r1"}}),
    )
    .await;

    let body: Value =
        meclaw_core::serde_json::from_str(em.content["messages"][0]["text"].as_str().unwrap())
            .expect("query body json");
    assert_eq!(
        body["degraded"], true,
        "without the declaration the cell is in a fresh netns and cannot reach anything"
    );
    assert_eq!(
        em.content["header"]["exit_code"], 0,
        "and it fails QUIETLY at exit 0 -- the reason this went unnoticed for a whole \
         benchmark run"
    );
}
