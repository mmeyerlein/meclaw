//! GH #1374 -- **a failed duplex session is not silence on the line.**
//!
//! Measured on a phone call against a live colony: when the duplex session of
//! a call fails, the media half emits `error` with `hop.error_code`
//! `duplex_failed` and closes its socket with 1011. In duplex mode the media
//! half has no recogniser and no synthesiser of its own, the hive's
//! `./voice -> .` edge carries the error only to the rim, and the dialplan
//! parks the leg after the stream started -- so the caller heard nothing,
//! for as long as they stayed on the line.
//!
//! The repair lives in the signalling half and its hive, not in the media
//! cell: a `./voice -> ./signal` edge for exactly that error, and a route in
//! `signal` that plays `params.failure_media` into the call it names and then
//! puts the leg down. Empty `failure_media` -- the default -- does nothing.
//!
//! The edge is walked with the colony's own CEL (`meclaw_colony::cel_eval`),
//! the script is driven straight through stdin. No colony, no switch.
//!
//! Guarded like every template-reading test (GH #49).

use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const HIVE: &str = "templates/freeswitch/config.json";
const SIGNAL: &str = "templates/freeswitch/signal/config.json";
const README: &str = "templates/freeswitch/README.md";

/// Synthetic on purpose: no real call reaches a lock.
const A_CALL: &str = "call-gh1374";
const MEDIA: &str = "file_string://sounds/sorry.wav!sounds/goodbye.wav";

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read(rel: &str) -> Option<Value> {
    let raw = std::fs::read_to_string(repo(rel)).ok()?;
    Some(sj::from_str(&raw).unwrap_or_else(|e| panic!("{rel} parses: {e}")))
}

fn skip() -> bool {
    if read(HIVE).is_none() || read(SIGNAL).is_none() {
        eprintln!("templates/freeswitch did not travel into this tree -- skipped (GH #49)");
        return true;
    }
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("no python3 -- skipped");
        return true;
    }
    false
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// The hop of the media half's error lane as `voice` stamps it
/// (`emit_error`: route, code, `msg_type`, the call under both names, the
/// engine).
fn voice_error(code: &str) -> Map<String, Value> {
    obj(
        json!({"route": "error", "error_code": code, "msg_type": "voice_error",
               "session_id": A_CALL, "call_id": A_CALL, "engine": "duplex"}),
    )
}

/// How many shipped edges `from -> to` of the hive carry `hop`, modifier
/// applied.
fn carried(from: &str, to: &str, hop: &Map<String, Value>) -> usize {
    let hive = read(HIVE).expect("checked by skip()");
    let edges = hive["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let context = Map::new();
    let mut n = 0;
    for e in edges {
        if e["from"] != from || e["to"] != to {
            continue;
        }
        if let Some(c) = e["condition"].as_str() {
            let cond = parse_condition(c).unwrap_or_else(|x| panic!("{c}: {x}"));
            if !matches!(evaluate_condition(&cond, &context, hop), Ok(true)) {
                continue;
            }
        }
        if e["modifier"].is_object() {
            let spec: ModifierSpec = sj::from_value(e["modifier"].clone()).expect("a modifier");
            let m = parse_modifier(&spec).expect("a modifier parses");
            let h = meclaw_core::Headers::from_parts(context.clone(), hop.clone());
            if apply_modifier(&m, &h).is_err() {
                continue;
            }
        }
        n += 1;
    }
    n
}

/// One pass of the shipped script with `hop` and `failure_media` as given
/// (`None` = the key absent). Returns the emissions it printed.
fn run(hop: Map<String, Value>, failure_media: Option<&str>) -> Vec<Value> {
    run_full(hop, Map::new(), "", failure_media).0
}

/// [`run`] with a context and a message text, and the script's stderr.
fn run_full(
    hop: Map<String, Value>,
    context: Map<String, Value>,
    said: &str,
    failure_media: Option<&str>,
) -> (Vec<Value>, String) {
    let cfg = read(SIGNAL).expect("checked by skip()");
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let mut params = cfg["params"].clone();
    let p = params.as_object_mut().expect("params is an object");
    p.remove("script_inline");
    p.remove("sandbox");
    p.insert("fs_api_base_url".into(), json!("http://127.0.0.1:8080"));
    match failure_media {
        Some(m) => {
            p.insert("failure_media".into(), json!(m));
        }
        None => {
            p.remove("failure_media");
        }
    }
    let doc = json!({
        "envelope": {"header": {"hop": hop, "context": context}},
        "body": {"messages": [{"origin": "tool", "type": "text", "text": said}]},
        "params": params,
    });
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3 runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(doc.to_string().as_bytes())
        .expect("the document goes in");
    let done = child.wait_with_output().expect("the script ends");
    assert!(
        done.status.success(),
        "the pass exits 0: {}",
        String::from_utf8_lossy(&done.stderr)
    );
    let emitted = sj::from_slice::<Vec<Value>>(&done.stdout).unwrap_or_else(|e| {
        panic!(
            "the script prints a list of emissions ({e}): {}",
            String::from_utf8_lossy(&done.stdout)
        )
    });
    (emitted, String::from_utf8_lossy(&done.stderr).into_owned())
}

/// `%XX` back to bytes -- the script encodes every query value.
fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).expect("utf-8")
}

#[test]
fn a_duplex_failure_reaches_the_signalling_half_and_still_the_rim() {
    if skip() {
        return;
    }
    let hop = voice_error("duplex_failed");
    assert_eq!(
        carried("./voice", "./signal", &hop),
        1,
        "exactly one edge hands `error duplex_failed` to the signalling half"
    );
    assert_eq!(
        carried("./voice", ".", &hop),
        1,
        "the rim still hears the error, as before"
    );
    for other in ["unknown_session", "bad_audio_frame", "credential_timeout"] {
        assert_eq!(
            carried("./voice", "./signal", &voice_error(other)),
            0,
            "`{other}` is not the call being over and stays at the rim alone"
        );
    }
}

#[test]
fn a_set_failure_media_is_played_into_that_call_and_the_leg_goes_down() {
    if skip() {
        return;
    }
    let out = run(voice_error("duplex_failed"), Some(MEDIA));
    assert_eq!(out.len(), 1, "one command at the switch: {out:#?}");
    let m = &out[0];
    assert_eq!(m["header"]["route"], "switch", "{m:#}");
    assert_eq!(m["header"]["operation"], "failure_media", "{m:#}");
    assert_eq!(m["header"]["call_uuid"], A_CALL, "{m:#}");
    let args: Value =
        sj::from_str(m["messages"][0]["text"].as_str().expect("a tool call")).expect("json");
    let url = args["url"].as_str().expect("a url");
    let (path, query) = url.split_once('?').expect("a query");
    assert_eq!(path, "http://127.0.0.1:8080/webapi/uuid_transfer", "{url}");
    assert_eq!(
        unquote(query),
        format!("{A_CALL} playback:{MEDIA},hangup:NORMAL_CLEARING inline"),
        "the media is played into THIS call and the leg is put down after it, \
         in one command the channel runs in order: {url}"
    );
}

#[test]
fn an_empty_failure_media_does_nothing() {
    if skip() {
        return;
    }
    for media in [None, Some(""), Some("  ")] {
        let out = run(voice_error("duplex_failed"), media);
        assert!(
            out.is_empty(),
            "failure_media {media:?}: the default is today's behaviour: {out:#?}"
        );
    }
}

#[test]
fn another_error_code_does_nothing() {
    if skip() {
        return;
    }
    let out = run(voice_error("unknown_session"), Some(MEDIA));
    assert!(out.is_empty(), "{out:#?}");
    let mut nameless = voice_error("duplex_failed");
    nameless.remove("session_id");
    nameless.remove("call_id");
    let out = run(nameless, Some(MEDIA));
    assert!(out.is_empty(), "no call named, no command: {out:#?}");
}

/// A `failure_media` the inline dialplan cannot carry, or one that never
/// ends, is refused: no command at the switch, and stderr names the reason --
/// a refused setting must not be a second silence (review M1).
#[test]
fn a_failure_media_that_would_break_the_line_is_refused_aloud() {
    if skip() {
        return;
    }
    for bad in [
        "file_string://a.wav b.wav",
        "sounds/sorry.wav,hangup",
        "local_stream://moh",
    ] {
        let (out, err) = run_full(voice_error("duplex_failed"), Map::new(), "", Some(bad));
        assert!(out.is_empty(), "failure_media {bad:?}: {out:#?}");
        assert!(
            err.contains("failure_media"),
            "failure_media {bad:?}: stderr names the setting: {err:?}"
        );
    }
}

/// The call id comes from the media connection's `?session=`, a string the
/// WS client chose: only `[0-9A-Za-z-]` goes into an API argv (review M2).
/// Without `call_id` the session id names the call.
#[test]
fn only_a_plain_call_id_reaches_the_switch() {
    if skip() {
        return;
    }
    let mut odd = voice_error("duplex_failed");
    odd.insert("call_id".into(), json!("abc def"));
    odd.insert("session_id".into(), json!("abc def"));
    let (out, err) = run_full(odd, Map::new(), "", Some(MEDIA));
    assert!(out.is_empty(), "{out:#?}");
    assert!(
        err.contains("abc def") || err.contains("call id"),
        "{err:?}"
    );

    let mut by_session = voice_error("duplex_failed");
    by_session.remove("call_id");
    let out = run(by_session, Some(MEDIA));
    assert_eq!(out.len(), 1, "{out:#?}");
    assert_eq!(out[0]["header"]["call_uuid"], A_CALL, "{:#}", out[0]);
}

/// The switch refusing the transfer is not swallowed: no turn, no rim, one
/// stderr line (review M1).
#[test]
fn a_refused_transfer_is_said_on_stderr() {
    if skip() {
        return;
    }
    let ctx = obj(
        json!({"phone_origin": "switch", "phone_op": "failure_media",
                         "phone_call": A_CALL}),
    );
    let (out, err) = run_full(
        obj(json!({"route": "result"})),
        ctx,
        "-ERR No such channel!",
        Some(MEDIA),
    );
    assert!(out.is_empty(), "{out:#?}");
    assert!(
        err.contains("failure_media") && err.contains("-ERR"),
        "{err:?}"
    );
}

#[test]
fn the_setting_is_declared_where_an_operator_reads_it() {
    if skip() {
        return;
    }
    let cfg = read(SIGNAL).expect("checked");
    let setting = &cfg["contract"]["settings"]["failure_media"];
    assert_eq!(setting["default"], "", "{setting:#}");
    assert_eq!(cfg["params"]["failure_media"], "", "shipped empty");
    let readme = std::fs::read_to_string(repo(README)).expect("README");
    assert!(
        readme.contains("`failure_media`") && readme.contains("GH #1374"),
        "the README says what a failed duplex session sounds like"
    );
}
