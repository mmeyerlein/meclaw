//! GH #709 — a sentence typed on a member's screen is a turn of its OWN channel.
//!
//! `display-hive.md` § 8.2: the chat app passes the event of its input line as a turn to
//! `channels/chat`, and from there it goes the way of every turn — firewall, session,
//! talky. Until this template there was no cell for that way. The chat app pushed its
//! input line straight at the member's firewall and had to invent three things it does
//! not own on the route: the identity, the assistant and the channel name, as literals
//! on an edge an installer wrote. R-24-4 rules that out — a channel is a transport form,
//! never a chat of its own — and § 8.7 says where the identity comes from instead: the
//! member's channel `chat`, never an installer literal.
//!
//! # What is measured here
//!
//! The SHIPPED script, run as the cell runs it: the document on stdin, the content JSON
//! on stdout. Four behaviours and one shape.
//!
//! 1. An accepted sentence leaves as `turn`, carrying a `turn_id` minted HERE (§ 8.2:
//!    the answer and every window built from it carry it on), the member, the channel
//!    name and the moment.
//! 2. The id is fresh per turn. A constant would make every answer of the day look like
//!    the answer to the first sentence.
//! 3. A `hop.user_id` the event names wins over the installed one (§ 8.7).
//! 4. An empty line is refused here, with `empty_turn`, rather than screened away later:
//!    a turn with no text costs a session, a model call and an answer about nothing.
//! 5. `in_answer` produces NOTHING. The channel `chat` has no loudspeaker (§ 8.3); the
//!    person reads the answer in the chat app, which hears it on the member's own lane.
//!
//! The drift lock at the end is the same one every `code` template owes: `channel.py` is
//! what a person reads and `params.script_inline` is what runs, and
//! `scripts/chat_channel_sync.py` is what keeps them one file.
//!
//! Guarded like every other template-reading test (GH #49): the public export ships a
//! subset of the library, and a template that did not travel is skipped rather than
//! judged.

use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// `templates/chat-channel`, or `None` when this tree did not ship it (GH #49).
fn shipped() -> Option<std::path::PathBuf> {
    let p = repo("templates/chat-channel");
    p.join("config.json").is_file().then_some(p)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn have_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok()
}

/// Hand the script to the runner on STDIN instead of in argv (GH #349), exactly as the
/// `code` cell does for an inline script over the `MAX_ARG_STRLEN` line.
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        meclaw_core::serde_json::to_string(script).expect("serialise the script"),
        meclaw_core::serde_json::to_string(stdin_doc).expect("serialise the document"),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// The emissions of one run of the SHIPPED cell: what the substrate would turn into
/// messages. An object is one message, an array is what it says, and an empty array is
/// the silence this cell answers an absorbed lane with.
fn channel(hop: Value, body: Value, params: Value) -> Vec<Value> {
    let cfg = read_json(&repo("templates/chat-channel/config.json"));
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("chat-channel declares script_inline");
    let doc = json!({
        "envelope": {
            "header": {"hop": hop, "context": {}},
            "target": "/person/channels/chat",
            "trace_id": "00000000-0000-0000-0000-000000000000",
            "ttl": 64,
            "reply_to": "/person/apps/chat",
        },
        "body": body,
        "params": params,
    });
    let out = run_script_on_stdin(script, &doc.to_string());
    assert!(
        out.status.success(),
        "the channel exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("chat-channel stdout is not JSON: {e}"));
    match v {
        Value::Array(a) => a,
        other => vec![other],
    }
}

fn typed(text: &str) -> Value {
    json!({"messages": [{"origin": "user", "type": "text", "text": text}]})
}

// ═══════════════════════════════════════════════════ 1. the accepted sentence

#[test]
fn a_typed_sentence_leaves_as_a_turn_with_an_id_of_its_own() {
    if shipped().is_none() || !have_python() {
        eprintln!("chat-channel did not travel into this tree, or no python3 -- skipped");
        return;
    }
    let out = channel(
        json!({"route": "in_typed"}),
        typed("how will the weather be"),
        json!({"user_id": "4711"}),
    );
    assert_eq!(out.len(), 1, "one sentence is one turn: {out:?}");
    let m = &out[0];
    assert_eq!(m["header"]["route"], "turn");
    assert_eq!(m["header"]["channel"], "chat", "the channel names itself");
    assert_eq!(
        m["header"]["user_id"], "4711",
        "§ 8.7: the identity is the CHANNEL's, and here it is the installed one"
    );
    let id = m["header"]["turn_id"]
        .as_str()
        .expect("a turn_id, as a string");
    assert!(
        !id.is_empty(),
        "§ 8.2: the turn_id is minted on acceptance, here, and carried on by the answer"
    );
    let at = m["header"]["happened_at"]
        .as_i64()
        .expect("happened_at is a number");
    assert!(
        at > 1_600_000_000_000,
        "happened_at is epoch MILLISECONDS, and {at} is not"
    );
    assert_eq!(
        m["messages"],
        json!([{"origin": "user", "type": "text", "text": "how will the weather be"}]),
        "the sentence travels as the member said it, and the channel adds no turn of \
         its own"
    );
}

#[test]
fn every_turn_gets_its_own_id() {
    if shipped().is_none() || !have_python() {
        return;
    }
    let one = channel(
        json!({"route": "in_typed"}),
        typed("the first one"),
        json!({}),
    );
    let two = channel(
        json!({"route": "in_typed"}),
        typed("the second one"),
        json!({}),
    );
    assert_ne!(
        one[0]["header"]["turn_id"], two[0]["header"]["turn_id"],
        "two sentences are two turns: a constant id would make every answer of the day \
         look like the answer to the first of them"
    );
}

#[test]
fn the_event_may_name_the_member_and_then_it_wins() {
    if shipped().is_none() || !have_python() {
        return;
    }
    let out = channel(
        json!({"route": "in_typed", "user_id": "99"}),
        typed("ich bin es"),
        json!({"user_id": "4711"}),
    );
    assert_eq!(
        out[0]["header"]["user_id"], "99",
        "§ 8.7: the input line knows who is sitting in front of it, so the event wins \
         over the value the channel was installed with"
    );
}

// ═══════════════════════════════════════════════════════ 2. what is refused

#[test]
fn an_empty_line_is_not_a_turn() {
    if shipped().is_none() || !have_python() {
        return;
    }
    for body in [typed("   "), json!({"messages": []})] {
        let out = channel(json!({"route": "in_typed"}), body.clone(), json!({}));
        assert_eq!(out.len(), 1, "a refusal is one message: {out:?}");
        assert_eq!(out[0]["header"]["route"], "error");
        assert_eq!(
            out[0]["header"]["error_code"], "empty_turn",
            "refused HERE rather than screened away later: an empty turn costs a \
             session, a model call and an answer about nothing ({body})"
        );
    }
}

#[test]
fn the_answer_ends_here_and_is_not_spoken() {
    if shipped().is_none() || !have_python() {
        return;
    }
    let answer = json!({"messages": [{"origin": "assistant", "type": "text", "text": "sonnig"}]});
    let out = channel(json!({"route": "in_answer"}), answer, json!({}));
    assert!(
        out.is_empty(),
        "§ 8.3: the channel `chat` has no loudspeaker. The answer stops here and the \
         person reads it in the chat app, which hears it on the member's own lane: {out:?}"
    );
    // Any other lane is nothing this channel opened, and silence is what it owes.
    let stray = channel(json!({"route": "in_speak"}), typed("nein"), json!({}));
    assert!(
        stray.is_empty(),
        "an unknown lane is answered with silence: {stray:?}"
    );
}

// ═══════════════════════════════════════════════════════════ 3. the declared shape

#[test]
fn the_template_declares_one_code_cell_and_the_four_lanes_it_really_has() {
    let Some(root) = shipped() else { return };
    let cfg = read_json(&root.join("config.json"));
    assert_eq!(
        cfg["cell"]["type"], "code",
        "the channel is one `code` cell"
    );
    let contract = &cfg["contract"];
    assert_eq!(
        contract["emits"]["hop"]["route"]["values"],
        json!(["turn", "error"]),
        "the two lanes that LEAVE. `code` validates every emission against this list \
         unconditionally, so a lane missing here is a contract_violation at runtime"
    );
    assert_eq!(
        contract["multi_send_capable"], false,
        "one message in, at most one message out. The empty list this cell writes on an \
         absorbed lane is zero messages and needs no multi-send: only a length above one \
         does (docs/cell-types.md § code)"
    );
    assert_eq!(
        contract["consumes"]["body"]["messages"]["required"], true,
        "a turn is its messages"
    );
    assert!(
        contract["settings"]["user_id"].is_object(),
        "the member is a declared SETTING of the channel, which is what makes it \
         addressable by override_params instead of by an edge literal (§ 8.7)"
    );
    let meta = read_json(&root.join("template.json"));
    assert_eq!(meta["name"], "chat-channel");
    assert_eq!(meta["version"], "1.0.0");
}

#[test]
fn the_script_a_person_reads_is_the_script_that_runs() {
    let Some(root) = shipped() else { return };
    let source = std::fs::read_to_string(root.join("channel.py"));
    let Ok(source) = source else {
        // The `.py` beside the config is a development-tree file; the config is what
        // ships. Nothing to compare in a tree that carries only the latter.
        return;
    };
    let cfg = read_json(&root.join("config.json"));
    assert_eq!(
        cfg["params"]["script_inline"].as_str().unwrap_or_default(),
        source,
        "channel.py and params.script_inline have drifted apart. The `.py` is the \
         source; run `python3 scripts/chat_channel_sync.py`"
    );
}

#[test]
fn the_catalogue_names_it() {
    let Some(_) = shipped() else { return };
    let table = std::fs::read_to_string(repo("templates/README.md")).expect("the catalogue");
    assert!(
        table.contains("[`chat-channel`](chat-channel/) | 1.0.0 |"),
        "every template the export ships has a row on the door sign (GH #235)"
    );
}
