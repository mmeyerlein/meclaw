//! GH #1260 (OR-WG-13, B-L6) -- the bridge never denies its own lookup.
//!
//! WHY this file exists: in the wave's lab (r026e, turn 9, "will it rain in
//! Hamburg tomorrow?") the bridge of the tool round said "I have no access to
//! current weather data -- which day do you mean?" while the required
//! `weather` call ran beside it; the person heard a false denial and a
//! question, then the answer. The bridge did not know a tool was working for
//! it. Its instruction now names the lookup that runs (the round's tool
//! family, in words) and forbids talk of missing access, abilities or limits
//! and any question -- on the consult and research bridges too, whose answer
//! is still coming; there only a question about the request itself is out,
//! since those turns keep the conversation going (R-TR-27). Its window stays
//! free of tool results.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The tool round of a simple turn stamped with `families`: (bridge, call).
fn tool_round(h: &mut Hive, turn: &str, families: &str, ask: &str) -> (Msg, Msg) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "simple", "families": families, "needs": "tool"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(ask)]}));
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        2,
        "bridge and call: {:?} {:?}",
        h.out,
        h.stderr
    );
    (quick[0].clone(), quick[1].clone())
}

fn bridge_text(m: &Msg) -> String {
    m.body["system"]["instructions"]["bridge"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase()
}

fn forbids_denials(way: &str, text: &str) {
    assert!(
        text.contains("never speak of missing access, missing abilities or limits"),
        "{way}: the bridge is told to keep quiet about access and limits -- {text}"
    );
    // "never say you have no limit" says the opposite of the contract.
    assert!(!text.contains("no limit"), "{way}: {text}");
}

#[test]
fn the_tool_rounds_bridge_names_the_running_lookup_and_forbids_denials() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    // The quick class's tools (`web`, `weather`) run in every tool round,
    // whatever the stamp says -- the lab stamped its weather turns `web`.
    for (turn, families, ask) in [
        ("t1", "weather", "will it rain in Hamburg tomorrow?"),
        ("t2", "web,needs", "when does the museum open today?"),
    ] {
        let (bridge, call) = tool_round(&mut h, turn, families, ask);
        let text = bridge_text(&bridge);
        assert!(
            text.contains("already running right now, on the web and at the weather service"),
            "{families}: the running lookup and where it looks -- {text}"
        );
        // A family at the hint stage is named to the model, not callable:
        // the bridge does not claim it as a running lookup.
        assert!(!text.contains("needs"), "{families}: {text}");
        // Named in words, and no call written out: told "with your tool
        // `weather`", the fast model wrote the call as JSON (probe BL6 arm A).
        assert!(!text.contains('`'), "{families}: no code names -- {text}");
        assert!(
            text.contains("write no tool call, no json"),
            "{families}: {text}"
        );
        forbids_denials(families, &text);
        assert!(
            text.contains("ask the person nothing"),
            "{families}: {text}"
        );
        // No content either (B-L3 stays).
        for word in ["no facts", "no numbers", "no results"] {
            assert!(text.contains(word), "{families}: {word} -- {text}");
        }
        // The window is unchanged: no tool result, the person's turn is there.
        let types: Vec<String> = bridge.body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| m["type"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            !types.iter().any(|t| t == "tool_call" || t == "tool_result"),
            "{families}: {types:?}"
        );
        assert!(types.iter().any(|t| t == "text"), "{families}: {types:?}");
        assert_eq!(bridge.body["tool_scope"], json!({"allow": []}));
        // The required call carries no bridge leaf.
        assert_eq!(
            call.body["system"]["instructions"]["bridge"],
            Value::Null,
            "{families}: {:?}",
            call.body["system"]
        );
    }
}

#[test]
fn the_consult_and_research_bridges_forbid_denials_too() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    for (turn, path, way) in [("t1", "complex", "consult"), ("t2", "research", "research")] {
        h.out.clear();
        let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                         "path": path, "families": "web"});
        let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": path});
        h.lane(
            "in_curate",
            ctx,
            hop,
            json!({"messages": [user("plan my week around the move")]}),
        );
        let quick = h.routed("quick");
        assert_eq!(quick.len(), 1, "{way}: {:?} {:?}", h.out, h.stderr);
        let text = bridge_text(&quick[0]);
        forbids_denials(way, &text);
        // Only a question about the request itself is out: the conversation
        // goes on while the answer is coming (R-TR-27).
        assert!(!text.contains("ask the person nothing"), "{way}: {text}");
        let scoped = match way {
            "consult" => "ask nothing about this request itself",
            _ => "keep the conversation going on another subject",
        };
        assert!(text.contains(scoped), "{way}: {scoped} -- {text}");
    }
    let research = bridge_text(&h.routed("quick")[0]);
    assert!(
        research.contains("never on this request: ask nothing about it"),
        "research: {research}"
    );
}

#[test]
fn the_consult_and_tool_bridges_see_only_the_persons_turn() {
    if !shipped() {
        return;
    }
    // An earlier answer in the window is a pattern the fast model follows:
    // it answered a consult in the bridge itself (a plan, a table) and said
    // tonight's degrees on a tool round (probe BL6 arm C). The research
    // bridge keeps the window, it goes on talking.
    for (turn, path, needs, way, window) in [
        ("t1", "complex", "", "consult", 1),
        ("t2", "simple", "tool", "tool", 1),
        ("t3", "research", "", "research", 3),
    ] {
        let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
        let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                         "path": path, "families": "weather", "needs": needs});
        let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": path});
        let earlier = json!({"origin": "assistant", "type": "text",
                             "text": "Here is a long plan with a table."});
        h.lane(
            "in_curate",
            ctx,
            hop,
            json!({"messages": [user("I moved last month"), earlier,
                                user("how cold is it tonight?")]}),
        );
        let quick = h.routed("quick");
        let bridge = quick
            .iter()
            .find(|m| m.body["system"]["instructions"]["bridge"].is_object())
            .unwrap_or_else(|| panic!("{way}: no bridge in {:?} {:?}", h.out, h.stderr));
        let texts: Vec<String> = bridge.body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| m["text"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(texts.len(), window, "{way}: {texts:?}");
        assert!(
            texts
                .last()
                .is_some_and(|t| t.contains("how cold is it tonight?")),
            "{way}: the person's turn -- {texts:?}"
        );
    }
}

#[test]
fn a_pin_is_never_the_persons_turn_and_a_split_turn_stays_whole() {
    if !shipped() {
        return;
    }
    // A pin (`broadcast_mode` tail, the default) stands in the window as
    // `{origin: user}` where it arrived; one that arrived after the person's
    // turn was walled stands behind it, and the bridge took it for the
    // person's turn and lost the request (fix-round review I1). A turn sent
    // as two messages keeps both: the bridge sees the whole request, not its
    // last fragment (review M1).
    const PIN: &str = "The owner is travelling until Friday.";
    for (turn, path, needs, way) in [
        ("t1", "complex", "", "consult"),
        ("t2", "simple", "tool", "tool"),
        ("t3", "research", "", "research"),
    ] {
        let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
        h.lane(
            "in_pin",
            json!({}),
            json!({}),
            json!({"pins": [{"text": PIN, "source": "orga"}]}),
        );
        // It arrived after the turn's rows: its place is behind them.
        h.db.execute("UPDATE pins SET at = '2099-01-01T00:00:00.000000Z'", [])
            .unwrap();
        h.out.clear();
        let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                         "path": path, "families": "weather", "needs": needs});
        let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": path});
        let earlier = json!({"origin": "assistant", "type": "text",
                             "text": "Here is a long plan with a table."});
        h.lane(
            "in_curate",
            ctx,
            hop,
            json!({"messages": [user("I moved last month"), earlier,
                                user("how cold is it tonight?"), user("in Berlin")]}),
        );
        let quick = h.routed("quick");
        let bridge = quick
            .iter()
            .find(|m| m.body["system"]["instructions"]["bridge"].is_object())
            .unwrap_or_else(|| panic!("{way}: no bridge in {:?} {:?}", h.out, h.stderr));
        let texts: Vec<String> = bridge.body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| m["text"].as_str().unwrap_or_default().to_string())
            .collect();
        if way == "research" {
            // The research bridge keeps the window: the pin stands last there,
            // so the case the other two guard against is real.
            assert!(
                texts
                    .last()
                    .is_some_and(|t| t.contains(&format!("[pinned by orga] {PIN}"))),
                "{way}: the pin stands behind the turn -- {texts:?}"
            );
            continue;
        }
        assert!(
            !texts
                .iter()
                .any(|t| t.contains(PIN) || t.contains("pinned by")),
            "{way}: no pin in the bridge -- {texts:?}"
        );
        assert_eq!(texts.len(), 2, "{way}: the whole turn -- {texts:?}");
        assert!(
            texts[0].contains("how cold is it tonight?") && texts[1].contains("in Berlin"),
            "{way}: the person's turn in order -- {texts:?}"
        );
    }
}

#[test]
fn a_window_without_the_persons_text_still_carries_no_pin() {
    if !shipped() {
        return;
    }
    // With no text of the person in the window (an empty window, only an
    // answer) the bridge kept the whole window, pins included, and a pin stood
    // there as the only `{origin: user}` element -- the one thing the bridge
    // window never carries (delta review 2 of U, M1).
    const PIN: &str = "The owner is travelling until Friday.";
    let earlier = json!({"origin": "assistant", "type": "text",
                         "text": "Here is a long plan with a table."});
    for (turn, path, needs, way) in [
        ("t1", "complex", "", "consult"),
        ("t2", "simple", "tool", "tool"),
    ] {
        for window in [json!([]), json!([earlier.clone()])] {
            let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
            h.lane(
                "in_pin",
                json!({}),
                json!({}),
                json!({"pins": [{"text": PIN, "source": "orga"}]}),
            );
            h.out.clear();
            let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                             "path": path, "families": "weather", "needs": needs});
            let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                             "channel": "test", "audience_set": ROUND_E, "path": path});
            h.lane("in_curate", ctx, hop, json!({"messages": window.clone()}));
            let quick = h.routed("quick");
            let bridge = quick
                .iter()
                .find(|m| m.body["system"]["instructions"]["bridge"].is_object())
                .unwrap_or_else(|| panic!("{way}: no bridge in {:?} {:?}", h.out, h.stderr));
            let texts: Vec<String> = bridge.body["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|m| m["text"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                !texts
                    .iter()
                    .any(|t| t.contains(PIN) || t.contains("pinned by")),
                "{way} {window}: no pin in the bridge -- {texts:?}"
            );
        }
    }
}

#[test]
fn a_family_without_a_place_is_never_named_by_its_code_name() {
    if !shipped() {
        return;
    }
    // Told a tool's code name, the fast model wrote the call out (probe BL6
    // arm A); a family the bridge has no words for stays unnamed.
    for (class, running) in [
        (
            "web,weather,calendar",
            "already running right now, on the web and at the weather service and in other places;",
        ),
        ("calendar", "already running right now; it was started"),
    ] {
        let mut h = Hive::with(&[
            ("policy", "role", json!("talky")),
            ("policy", "quick_tools", json!(class)),
        ]);
        let (bridge, _) = tool_round(&mut h, "t1", "calendar", "when is my next appointment?");
        let text = bridge_text(&bridge);
        assert!(text.contains(running), "{class}: {text}");
        assert!(
            !text.contains("calendar"),
            "{class}: no code name -- {text}"
        );
        assert!(!text.contains("with the tool"), "{class}: {text}");
    }
}
