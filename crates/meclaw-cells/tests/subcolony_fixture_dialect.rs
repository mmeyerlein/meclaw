//! P9 block B — the protocol fixture speaks wire v1, including its failure modes.
//!
//! The fixture exists because the interesting cases of a sub-colony connection
//! cannot be produced with a real `meclaw` child: a child announcing a foreign
//! protocol version, a child that never says hello, a child that dies mid-request.
//! A real binary is correct by construction and therefore useless as a negative
//! control.
//!
//! These tests pin the fixture itself. Everything downstream trusts it, so a
//! silently broken fixture would turn into silently passing facade tests.

use std::io::{BufRead as _, Write as _};
use std::process::{Command, Stdio};
use std::time::Duration;

const FIXTURE: &str = env!("CARGO_BIN_EXE_subcolony_protocol_fixture");

/// Run the fixture with `args`, feed it `input`, collect up to `expect` lines.
fn drive(args: &[&str], input: &str, expect: usize) -> Vec<String> {
    let mut child = Command::new(FIXTURE)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn fixture");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    stdin.write_all(input.as_bytes()).expect("write");
    stdin.flush().expect("flush");

    let reader = std::thread::spawn(move || {
        std::io::BufReader::new(stdout)
            .lines()
            .take(expect)
            .map_while(Result::ok)
            .collect::<Vec<_>>()
    });
    // Failure-marker timeout, generous per convention.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !reader.is_finished() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("fixture produced no {expect} line(s) within 30s");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let lines = reader.join().expect("reader thread");
    drop(stdin);
    let _ = child.wait();
    lines
}

fn json(line: &str) -> serde_json::Value {
    serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON: {e}; line: {line}"))
}

fn request(turn_id: &str, text: &str) -> String {
    format!(
        "{}\n",
        serde_json::json!({
            "v": 1, "type": "message",
            "context": {"turn_id": turn_id},
            "body": {"messages": [{"origin": "user", "type": "text", "text": text}]}
        })
    )
}

#[test]
fn it_announces_itself_and_echoes_a_request() {
    let out = drive(&[], &request("k1", "ping"), 2);
    let ready = json(&out[0]);
    assert_eq!(ready["type"], "ready");
    assert_eq!(ready["v"], 1);

    let reply = json(&out[1]);
    assert_eq!(reply["type"], "message");
    assert_eq!(reply["context"]["turn_id"], "k1", "correlation key echoed");
    assert_eq!(reply["body"]["messages"][0]["text"], "echo:ping");
}

#[test]
fn it_carries_the_trace_id_back() {
    let req = serde_json::json!({
        "v": 1, "type": "message", "trace_id": "018f0000-0000-7000-8000-0000000000ff",
        "context": {"turn_id": "k1"}, "body": {"messages": []}
    });
    let out = drive(&[], &format!("{req}\n"), 2);
    assert_eq!(
        json(&out[1])["trace_id"],
        "018f0000-0000-7000-8000-0000000000ff"
    );
}

#[test]
fn it_reports_the_ttl_it_was_given() {
    // The fixture echoes the received TTL so a facade test can prove the
    // decrement actually crossed the boundary rather than being asserted
    // parent-side only.
    let req = serde_json::json!({
        "v": 1, "type": "message", "ttl": 7,
        "context": {"turn_id": "k1"}, "body": {"messages": []}
    });
    let out = drive(&[], &format!("{req}\n"), 2);
    assert_eq!(json(&out[1])["body"]["header"]["received_ttl"], 7);
}

#[test]
fn it_reports_the_context_it_was_given() {
    // Lets a facade test prove exactly WHICH context keys crossed — the header
    // crossing rule is "nothing by default", and only an observation on the
    // child side can prove a key did not cross.
    let req = serde_json::json!({
        "v": 1, "type": "message",
        "context": {"turn_id": "k1", "user_id": "u-7"}, "body": {"messages": []}
    });
    let out = drive(&[], &format!("{req}\n"), 2);
    let seen = &json(&out[1])["body"]["header"]["received_context"];
    assert_eq!(seen["user_id"], "u-7");
    assert_eq!(seen["turn_id"], "k1");
}

#[test]
fn a_foreign_protocol_version_can_be_announced() {
    // The negative control for the strict version assert (D4).
    let out = drive(&["--protocol", "2"], "", 1);
    let ready = json(&out[0]);
    assert_eq!(ready["type"], "ready");
    assert_eq!(ready["v"], 2, "the fixture must be able to lie about v");
}

#[test]
fn it_can_refuse_to_say_hello() {
    // The negative control for the boot timeout. Nothing on stdout, ever.
    let mut child = Command::new(FIXTURE)
        .args(["--no-ready"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let stdout = child.stdout.take().expect("stdout");
    let reader =
        std::thread::spawn(move || std::io::BufReader::new(stdout).lines().next().is_some());
    // Semantic timing discriminator, deliberately short: the fixture writes its
    // ready frame as its very first action, so 2s of silence is decisive.
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        !reader.is_finished(),
        "--no-ready must stay silent, but a line arrived"
    );
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn it_can_die_after_a_given_number_of_requests() {
    // The negative control for "the child died mid-conversation".
    let input = format!("{}{}", request("k1", "one"), request("k2", "two"));
    let out = drive(&["--die-after", "1"], &input, 3);
    assert_eq!(
        out.len(),
        2,
        "ready + exactly one reply, then death; got {out:?}"
    );
    assert_eq!(json(&out[1])["context"]["turn_id"], "k1");
}

#[test]
fn it_can_emit_a_frame_nobody_asked_for() {
    // The negative control for unsolicited child egress (D6): a frame with no
    // correlation key at all.
    let out = drive(&["--unsolicited"], "", 2);
    assert_eq!(json(&out[0])["type"], "ready");
    let spontaneous = json(&out[1]);
    assert_eq!(spontaneous["type"], "message");
    assert!(
        spontaneous["context"].get("turn_id").is_none(),
        "an unsolicited frame carries no correlation key: {spontaneous}"
    );
}
