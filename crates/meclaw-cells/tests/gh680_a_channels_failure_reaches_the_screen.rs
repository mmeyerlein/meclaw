//! GH #680 — a grown screen hears its channels' failures as system notices.
//!
//! A screen was grown with two edges: what it produces (`event`, `receipt`)
//! goes up, and a `view` addressed to it comes down re-stamped `in_view`. The
//! `error` a channel emits left the container upwards for the operator and
//! nothing else — the person in front of the screen saw a microphone that fell
//! silent and no word about it.
//!
//! Since `display@2.2.0` the screen takes `in_notice` and translates a bare
//! `hop.error_code` into a sentence (ADR-0039). The edge that carries a
//! channel's `error` to the screen lives in the **builder's recipe** and not in
//! the `member` template: the `channels` container knows its screen only through
//! the mutation that grows it, so the mutation is what can draw `. -> ./<screen>`.
//! `examples/organism/grow-screen.json` is the byte truth of that edge, and this
//! file holds the recipe and the example to the same three edges — the same
//! discipline `gh466_grow_level_renders_the_level.rs` runs every level under.
//!
//! Why no `channel_node` guard on the third edge: an error carries no context
//! of the screen it belongs to — it belongs to the member — and every channel
//! failure in the container is a notice for that member's screen. The screen
//! itself never emits `error` (`templates/display/config.json`, `emits`), so
//! the edge cannot loop.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};
use std::path::PathBuf;

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(rel)
}

/// The edge, as the plan writes it and the example carries it.
fn the_error_edge() -> Value {
    json!({
        "from": ".",
        "to": "./display",
        "condition": "has(hop.route) && hop.route == 'error'",
        "modifier": {"set_hop": {"route": "'in_notice'"}}
    })
}

/// The screen's declaration out of the shipped example, or `None` in a tree
/// the examples did not travel into (GH #49).
fn example_screen() -> Option<Value> {
    let raw = std::fs::read_to_string(repo("examples/organism/grow-screen.json")).ok()?;
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("the example is json");
    v["manifest"]
        .as_array()
        .expect("a manifest")
        .iter()
        .find(|d| {
            d["scope"]
                .as_str()
                .is_some_and(|s| s.ends_with("/channels"))
        })
        .cloned()
}

/// The screen level, rendered through the shipped recipe (form `gh466::grow`).
fn rendered_screen(template: &Value) -> Value {
    let wish = json!({"recipe": "grow_level", "request": "…",
        "params": {"scope": "/os/orgs/acme/members/alex", "level": "screen",
                   "name": "display", "template": template,
                   "override_params": {"web": {"mount": "alex-display"}}}});
    let all = emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": wish.to_string()}],
        }),
    );
    assert_eq!(
        all.len(),
        1,
        "a screen wish renders exactly one manifest: {all:?}"
    );
    let out = all.into_iter().next().expect("one emission");
    let decls = out["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest: {out}"));
    assert_eq!(decls.len(), 1, "a screen is one declaration: {decls:?}");
    decls[0].clone()
}

#[test]
fn a_channels_failure_reaches_the_screen() {
    let Some(want) = example_screen() else {
        return; // a tree without the examples cannot make this assertion
    };
    // The example: three edges, and the third is the error wire in.
    let edges = want["diff"]["add_edges"]
        .as_array()
        .expect("the screen declaration carries add_edges");
    assert_eq!(
        edges.len(),
        3,
        "a screen costs three edges since builder@1.10.0 — up, down, and the \
         channels' failures in: {edges:?}"
    );
    assert_eq!(
        edges[2]["condition"],
        json!("has(hop.route) && hop.route == 'error'"),
        "the third edge carries a channel's `error` and nothing else"
    );
    assert_eq!(
        edges[2]["modifier"]["set_hop"]["route"],
        json!("'in_notice'"),
        "the failure arrives on the screen's own notice lane"
    );
    assert_eq!(
        &edges[2],
        &the_error_edge(),
        "the third edge, byte for byte"
    );
    assert!(
        edges[2]["condition"]
            .as_str()
            .is_some_and(|c| !c.contains("channel_node")),
        "an error carries no context of the screen — every channel failure in \
         the container is a notice for the member's screen, so no guard"
    );

    // The recipe renders the same three, in the same order.
    let got = rendered_screen(&want["diff"]["add_nodes"][0]["template"]);
    assert_eq!(
        got["diff"]["add_edges"], want["diff"]["add_edges"],
        "the shipped recipe and examples/organism/grow-screen.json disagree on \
         the screen's edges"
    );
    assert_eq!(got["scope"], want["scope"], "scope");
}

#[test]
fn the_screen_itself_never_emits_error_so_the_wire_cannot_loop() {
    let p = repo("templates/display/config.json");
    if !p.is_file() {
        return; // GH #49
    }
    let raw = std::fs::read_to_string(p).expect("readable");
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    let emits = cfg["params"]["contract"]["emits"]
        .as_array()
        .expect("the display declares what it emits");
    assert!(
        !emits.iter().any(|e| e["route"] == json!("error")),
        "the screen declares an `error` emission — the third edge would feed \
         the screen its own failure: {emits:?}"
    );
    let accepts = cfg["params"]["contract"]["accepts"]
        .as_array()
        .expect("the display declares what it accepts");
    assert!(
        accepts.iter().any(|a| a["route"] == json!("in_notice")),
        "the screen takes no `in_notice` — the edge would dead-letter: {accepts:?}"
    );
}
