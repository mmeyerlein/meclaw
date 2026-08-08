//! Test fixture: a process that speaks the sub-colony wire (protocol v1)
//! without booting a real colony.
//!
//! Why this exists rather than driving a real `meclaw` child: the cases a
//! sub-colony facade has to survive are the ones a correct binary never
//! produces. A real child always announces protocol 1, always boots, and never
//! dies halfway through a request — which makes it useless as a negative
//! control. Every switch below buys exactly one such case.
//!
//! It also answers with observations (`received_ttl`, `received_context`) that a
//! real colony has no reason to send. That is what lets a facade test prove what
//! actually CROSSED the boundary: the header-crossing rule is "nothing unless
//! declared", and only the far side can testify that a key did not arrive.
//!
//! Test-only artefact — reached through `CARGO_BIN_EXE_subcolony_protocol_fixture`.

use std::io::{BufRead as _, Write as _};

/// The wire version this fixture claims unless told to lie.
const PROTOCOL: u64 = 1;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| -> Option<String> {
        let i = args.iter().position(|a| a == name)?;
        args.get(i + 1).cloned()
    };
    let num = |name: &str| -> Option<u64> { value(name).and_then(|v| v.parse().ok()) };

    let protocol = num("--protocol").unwrap_or(PROTOCOL);
    let die_after = num("--die-after");
    let delay_ms = num("--delay-ms").unwrap_or(0);

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    // The handshake, first action of all — so "--no-ready" is a decisive
    // silence rather than a race.
    if !has("--no-ready") {
        let ready = serde_json::json!({
            "v": protocol, "type": "ready", "version": "fixture"
        });
        let _ = writeln!(out, "{ready}");
        let _ = out.flush();
    }

    // A frame nobody asked for: no correlation key, sent before any request.
    if has("--unsolicited") {
        let frame = serde_json::json!({
            "v": PROTOCOL, "type": "message",
            "context": {},
            "body": {"messages": [{"origin": "assistant", "type": "text", "text": "nobody asked"}]}
        });
        let _ = writeln!(out, "{frame}");
        let _ = out.flush();
    }

    let mut answered: u64 = 0;
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<serde_json::Value>(&line) else {
            let err = serde_json::json!({
                "v": PROTOCOL, "type": "error",
                "error_code": "invalid_frame", "detail": "not JSON"
            });
            let _ = writeln!(out, "{err}");
            let _ = out.flush();
            continue;
        };

        // Die WITHOUT answering: the negative control for a child that goes
        // away while a caller is waiting on it.
        if has("--die-on-request") {
            std::process::exit(0);
        }

        if delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }

        let said = req["body"]["messages"]
            .as_array()
            .and_then(|turns| turns.iter().rev().find(|t| t["origin"] == "user"))
            .and_then(|t| t["text"].as_str())
            .unwrap_or("");

        let reply = serde_json::json!({
            "v": PROTOCOL,
            "type": "message",
            "trace_id": req.get("trace_id").cloned().unwrap_or(serde_json::Value::Null),
            "context": req.get("context").cloned().unwrap_or_else(|| serde_json::json!({})),
            "body": {
                // Observations a real colony would not send, carried in the
                // body's own header slot — see the module docs. They ride the
                // ordinary body path, so nothing on the parent side needs to
                // know this fixture exists.
                "header": {
                    "received_ttl": req.get("ttl").cloned().unwrap_or(serde_json::Value::Null),
                    "received_context": req.get("context").cloned()
                        .unwrap_or_else(|| serde_json::json!({}))
                },
                "messages": [
                    {"origin": "assistant", "type": "text", "text": format!("echo:{said}")}
                ]
            }
        });
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();

        answered += 1;
        if die_after.is_some_and(|n| answered >= n) {
            // Leave abruptly, the way a crashing child would.
            std::process::exit(0);
        }
    }
}
