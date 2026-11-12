//! D58 moveout (GH #1476, plan `u-moveout.md` contract point 1, `fill`; plan
//! review B1) -- the policy fills in the tools the cut left open.
//!
//! WHY this file exists: the tool round's bridge names where the running
//! lookup looks ("on the web and at the weather service"), from the tools
//! the cut leaves callable for the target -- not from the stamp (the lab
//! stamped its weather turns `web`, r026e) and not from the hint stage. The
//! lead cannot write that phrase: its question goes out before the cut and
//! carries no tool leaves. So the entry carries a text with a gap and a table
//! tool -> words plus a rest word (`LOOKUP_PLACES` today, now the lead's), and
//! the policy fills the gap by today's rule (`running_tools`,
//! `lookup_phrase`: the family name before the tool name, the hint stage not
//! counted, an unnamed family adds "in other places").

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_moveout.rs"]
#[allow(unused_imports)]
use crate::curator_moveout; // harness_fold: mod curator_moveout;

use curator_hive::*;
use curator_moveout::*;
use meclaw_core::serde_json::{Value, json};

fn side_of_round(h: &mut Hive, turn: &str, families: &str, scope: Value) -> String {
    let mut body = json!({"messages": [user("will it rain in Hamburg tomorrow?")]});
    if !scope.is_null() {
        body["tool_scope"] = scope;
    }
    seam(h, turn, "0", tool_stamp(families), body);
    let side = side_calls(h);
    assert_eq!(side.len(), 1, "{turn}: {:?} {:?}", h.out, h.stderr);
    side_text(&side[0])
}

#[test]
fn gh_d58_moveout_the_policy_fills_the_tools_the_cut_left_open() {
    if !shipped() {
        return;
    }
    // The lead writes the gap and the table, never the filled phrase.
    let v = verdict(
        lead_knobs(json!({})),
        "simple",
        &["tool"],
        &["weather"],
        LIVE_H,
    );
    let must = sides_of(&v, MUST_CALL);
    assert_eq!(must.len(), 1, "{v}");
    let text = must[0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains(TOOL_WORDS),
        "today's sentence, from the lead: {text}"
    );
    assert!(
        !text.contains("weather service") && !text.contains("on the web"),
        "the places are not in the lead's text -- the cut decides them: {text}"
    );
    let fill = must[0]["fill"].to_string();
    assert!(
        fill.contains("at the weather service")
            && fill.contains("on the web")
            && fill.contains("in other places"),
        "the table and the rest word ride the entry: {fill}"
    );

    let mut h = moveout_hive(&[]);
    // The class's tools are open whatever the stamp says.
    let t = side_of_round(&mut h, "t1", "weather", Value::Null);
    assert!(
        t.contains("already running right now, on the web and at the weather service;"),
        "the open tools, by family: {t}"
    );
    // A family at the hint stage is named to the model, not callable.
    let t = side_of_round(&mut h, "t2", "weather,needs", Value::Null);
    assert!(
        t.contains("already running right now, on the web and at the weather service;"),
        "the hint stage is not counted: {t}"
    );
    // The channel leaves only the weather tool open: only it is named.
    let t = side_of_round(&mut h, "t3", "weather", json!({"allow": ["weather"]}));
    assert!(
        t.contains("already running right now, at the weather service;"),
        "only what the cut left open: {t}"
    );

    let policy = script_at("curator/policy");
    for word in ["at the weather service", "LOOKUP_PLACES", "in other places"] {
        assert!(
            !policy.contains(word),
            "curator/policy holds no words of a lookup ({word})"
        );
    }
}
