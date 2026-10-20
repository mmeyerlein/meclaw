//! GH #1280 -- a pending consult takes the consult tool away.
//!
//! WHY this file exists: since `talk:brain` (GH #1260) a talk turn reaches
//! the brain with its whole menu. A `complex` turn had left for cogny under
//! the curator's own consult id, the next turn was stamped `talk`, and the
//! brain -- told in `system.consult.pending` "Do not consult again" -- called
//! `consult_cogny` once more, as a tool call without a word beside it
//! (production log of 2026-10-20: two consults 9 s apart, 38 s and 42 s each,
//! the second about 0.09 $). A prompt is a request; the menu is the rule. So
//! while the session's consult is pending, every speaking call carries
//! `tool_scope.deny: ["consult_cogny"]`, the pending note stays beside it as
//! information, and the mark ends with the consult's advice -- or, as a
//! backstop, at `consult_pending_ms`, now 120 s: the measured consults ran
//! 38-42 s, and the old 30 s let the mark lapse while the first still ran.
//! The backstop is checked off the mark's own `until`; no wall clock decides.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn curate_with(h: &mut Hive, turn_id: &str, path: &str, extra_ctx: Value, ask: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0", "phase": "",
                     "path": path});
    let mut ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": path});
    for (k, v) in extra_ctx.as_object().cloned().unwrap_or_default() {
        ctx[k] = v;
    }
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(ask)], "system": mode("Be brief.")}),
    );
}

fn pending(h: &Hive) -> Value {
    meclaw_core::serde_json::from_str(&h.state("consult_pending:s1")).unwrap_or(Value::Null)
}

/// `until - at` of the pending mark, in milliseconds.
fn span_ms(mark: &Value) -> i64 {
    let at = |k: &str| {
        chrono::DateTime::parse_from_rfc3339(mark[k].as_str().unwrap_or_default())
            .unwrap_or_else(|e| panic!("{k} of {mark}: {e}"))
    };
    (at("until") - at("at")).num_milliseconds()
}

/// The names the call's `tool_scope` denies.
fn denied(call: &Msg) -> Vec<String> {
    call.body
        .get("tool_scope")
        .and_then(|s| s.get("deny"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The one brain call of the last lane.
fn brain_call(h: &Hive) -> Msg {
    let brain = h.routed("brain");
    assert_eq!(brain.len(), 1, "{:?} {:?}", h.out, h.stderr);
    brain[0].clone()
}

#[test]
fn a_pending_consult_takes_the_consult_tool_away() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));

    // Before any consult the brain keeps its whole menu.
    curate_with(&mut h, "t1", "talk", json!({}), "how are you?");
    let before = brain_call(&h);
    assert!(
        !denied(&before).contains(&"consult_cogny".to_string()),
        "no consult runs, nothing is taken away: {:?}",
        before.body.get("tool_scope")
    );

    // A complex turn leaves for the backend: one consult, one mark.
    curate_with(&mut h, "t2", "complex", json!({}), "plan my week");
    let errand = h.routed("consult");
    assert_eq!(errand.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let cid = errand[0].hop["consult_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let mark = pending(&h);
    assert_eq!(mark["consult_id"], json!(cid), "{mark}");
    assert_eq!(
        span_ms(&mark),
        120_000,
        "the backstop stands at consult_pending_ms, above the measured 38-42 s: {mark}"
    );

    // The person's next turn is a talk turn: the brain answers it, without
    // the consult tool, and is told the consult runs.
    curate_with(&mut h, "t3", "talk", json!({}), "and what about tomorrow?");
    let during = brain_call(&h);
    assert!(
        h.routed("consult").is_empty(),
        "exactly one consult: {:?}",
        h.out
    );
    assert!(
        denied(&during).contains(&"consult_cogny".to_string()),
        "while the consult is pending the menu has no consult tool: {:?}",
        during.body.get("tool_scope")
    );
    assert!(
        during.body["system"]["consult"]["pending"]["text"]
            .as_str()
            .is_some_and(|t| t.contains(&cid)),
        "the pending note stays as information: {:?}",
        during.body["system"]
    );

    // The advice arrives: the mark ends with it, the tool comes back.
    curate_with(
        &mut h,
        "t4",
        "complex",
        json!({"consult_id": cid}),
        "the advice",
    );
    let advice = brain_call(&h);
    assert!(h.routed("consult").is_empty(), "never consulted again");
    assert_eq!(pending(&h), Value::Null, "the advice ends the mark");
    assert!(
        !denied(&advice).contains(&"consult_cogny".to_string()),
        "{:?}",
        advice.body.get("tool_scope")
    );
    curate_with(&mut h, "t5", "talk", json!({}), "thanks");
    let after = brain_call(&h);
    assert!(
        !denied(&after).contains(&"consult_cogny".to_string()),
        "after the advice the brain may consult again: {:?}",
        after.body.get("tool_scope")
    );

    // Backstop: an advice that never comes. A mark past its `until` goes at
    // the next call, and the consult tool with it.
    curate_with(&mut h, "t6", "complex", json!({}), "plan my month");
    assert_eq!(h.routed("consult").len(), 1, "{:?}", h.out);
    let mut lost = pending(&h);
    lost["until"] = json!("2000-01-01T00:00:00.000000Z");
    h.db.execute(
        "UPDATE state SET value = ?1 WHERE key = 'consult_pending:s1'",
        [canonical(&lost)],
    )
    .unwrap();
    curate_with(&mut h, "t7", "talk", json!({}), "still there?");
    let lapsed = brain_call(&h);
    assert_eq!(pending(&h), Value::Null, "the lapsed mark is gone");
    assert!(
        !denied(&lapsed).contains(&"consult_cogny".to_string()),
        "past the backstop the tool comes back: {:?}",
        lapsed.body.get("tool_scope")
    );
    assert!(
        lapsed.body["system"]["consult"].get("pending").is_none(),
        "{:?}",
        lapsed.body["system"]
    );
}

#[test]
fn a_channel_scope_keeps_its_own_deny_beside_the_consult_tool() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));
    curate_with(&mut h, "t1", "complex", json!({}), "plan my week");
    assert_eq!(h.routed("consult").len(), 1, "{:?}", h.out);

    // A channel that already denies a tool keeps it denied, and the consult
    // tool joins it once.
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "phase": "",
                     "path": "talk"});
    let ctx = json!({"session_id": "s1", "turn_id": "t2", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("and tomorrow?")], "system": mode("Be brief."),
               "tool_scope": {"deny": ["weather"]}}),
    );
    let during = brain_call(&h);
    assert_eq!(
        denied(&during),
        vec!["weather".to_string(), "consult_cogny".to_string()],
        "{:?}",
        during.body.get("tool_scope")
    );
}

/// A `consult_cogny` call the brain wrote itself, as the llm cell hands it on
/// the tap; `folded` writes it in the collector's folded form
/// (`consult{op: "cogny"}`).
fn consult_call(id: &str, folded: bool) -> Value {
    let (name, args) = if folded {
        ("consult", json!({"op": "cogny", "args": {"goal": "plan"}}))
    } else {
        ("consult_cogny", json!({"goal": "plan"}))
    };
    json!({"origin": "assistant", "type": "tool_call", "id": id,
           "text": json!({"name": name, "arguments": args.to_string()}).to_string()})
}

#[test]
fn a_consult_the_brain_starts_itself_takes_the_tool_away_too() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));

    // Turn N: a talk turn, and the brain consults on its own (GH #1280,
    // review I1: since `talk:brain` the brain holds the tool on talk turns).
    curate_with(&mut h, "t1", "talk", json!({}), "plan my week");
    let call = brain_call(&h);
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([consult_call("c1", false)]),
    );
    let mark = pending(&h);
    assert_eq!(mark["consult_id"], json!("c1"), "{mark} {:?}", h.stderr);
    assert_eq!(span_ms(&mark), 120_000, "{mark}");

    // Turn N+1: no speaking model has the consult tool on its menu.
    curate_with(&mut h, "t2", "talk", json!({}), "and tomorrow?");
    let during = brain_call(&h);
    assert!(
        denied(&during).contains(&"consult_cogny".to_string()),
        "the brain's own consult takes the tool away: {:?}",
        during.body.get("tool_scope")
    );
    assert!(
        during.body["system"]["consult"]["pending"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("c1")),
        "{:?}",
        during.body["system"]
    );

    // The advice under the call's id ends the mark, the tool comes back.
    curate_with(
        &mut h,
        "t3",
        "talk",
        json!({"consult_id": "c1"}),
        "the advice",
    );
    let advice = brain_call(&h);
    assert_eq!(pending(&h), Value::Null, "the advice ends the mark");
    assert!(
        !denied(&advice).contains(&"consult_cogny".to_string()),
        "{:?}",
        advice.body.get("tool_scope")
    );
}

#[test]
fn the_brain_s_consult_never_overwrites_a_standing_mark() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));
    curate_with(&mut h, "t1", "complex", json!({}), "plan my week");
    let errand = h.routed("consult");
    assert_eq!(errand.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let cid = errand[0].hop["consult_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let before = pending(&h);

    // A brain call that consults anyway (a menu without the deny, e.g. an
    // older channel) leaves the curator's mark as it stands.
    curate_with(&mut h, "t2", "talk", json!({}), "and tomorrow?");
    let call = brain_call(&h);
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([consult_call("c2", false)]),
    );
    assert_eq!(pending(&h), before, "the first mark stands: {:?}", h.stderr);
    assert_eq!(pending(&h)["consult_id"], json!(cid));
}

#[test]
fn a_folded_consult_family_is_taken_away_and_marks_too() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));
    let folded = json!({"tools_folded": "consult,web"});

    // The brain calls the folded family (`consult{op: "cogny"}`): a mark.
    curate_with(&mut h, "t1", "talk", folded.clone(), "plan my week");
    let call = brain_call(&h);
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([consult_call("c1", true)]),
    );
    assert_eq!(pending(&h)["consult_id"], json!("c1"), "{:?}", h.stderr);

    // With `consult` folded the menu entry is `consult`: it goes too
    // (review M1: a deny on `consult_cogny` alone would match nothing).
    curate_with(&mut h, "t2", "talk", folded, "and tomorrow?");
    let during = brain_call(&h);
    let deny = denied(&during);
    assert!(
        deny.contains(&"consult".to_string()) && deny.contains(&"consult_cogny".to_string()),
        "{deny:?}"
    );
}

#[test]
fn a_quick_call_loses_the_tool_only_when_its_allow_holds_it() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));
    curate_with(&mut h, "t1", "complex", json!({}), "plan my week");
    assert_eq!(h.routed("consult").len(), 1, "{:?}", h.out);

    // A simple turn whose quick menu holds no consult tool: its scope stays.
    curate_with(&mut h, "t2", "simple", json!({}), "what time is it?");
    let quick = h.routed("quick");
    assert!(!quick.is_empty(), "{:?} {:?}", h.out, h.stderr);
    for q in &quick {
        assert!(
            !denied(q).contains(&"consult_cogny".to_string()),
            "nothing to take away: {:?}",
            q.body.get("tool_scope")
        );
    }

    // A simple turn stamped with the consult family: its allow holds the
    // tool, the deny takes it away (review M2).
    curate_with(
        &mut h,
        "t3",
        "simple",
        json!({"families": "consult"}),
        "ask the backend",
    );
    let quick = h.routed("quick");
    let scoped: Vec<&Msg> = quick
        .iter()
        .filter(|q| {
            q.body
                .get("tool_scope")
                .and_then(|s| s.get("allow"))
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().any(|n| n == "consult"))
        })
        .collect();
    assert!(!scoped.is_empty(), "{:?} {:?}", h.out, h.stderr);
    for q in scoped {
        assert!(
            denied(q).contains(&"consult_cogny".to_string()),
            "{:?}",
            q.body.get("tool_scope")
        );
    }
}

#[test]
fn an_ask_back_of_the_backend_keeps_the_mark() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));
    curate_with(&mut h, "t1", "complex", json!({}), "plan my week");
    let errand = h.routed("consult");
    assert_eq!(errand.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let cid = errand[0].hop["consult_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // cogny asks back under the consult's id: the consult still runs, the
    // mark and the deny stay (review M3).
    curate_with(
        &mut h,
        "t2",
        "talk",
        json!({"consult_id": cid, "consult_class": "ask"}),
        "the core asks: which week?",
    );
    let ask = brain_call(&h);
    assert_eq!(
        pending(&h)["consult_id"],
        json!(cid),
        "the ask keeps the mark"
    );
    assert!(
        denied(&ask).contains(&"consult_cogny".to_string()),
        "{:?}",
        ask.body.get("tool_scope")
    );

    // The advice itself ends it.
    curate_with(
        &mut h,
        "t3",
        "talk",
        json!({"consult_id": cid}),
        "the advice",
    );
    brain_call(&h);
    assert_eq!(pending(&h), Value::Null);
}

#[test]
fn a_follow_up_consult_is_marked_under_the_id_it_continues() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));
    curate_with(&mut h, "t1", "talk", json!({}), "and the cost?");
    let call = brain_call(&h);
    // A follow-up passes the earlier consult's id; the dispatcher stamps it,
    // and cogny's advice comes back under it (GH #894, `async_keys`).
    let follow = json!({"origin": "assistant", "type": "tool_call", "id": "c2",
        "text": json!({"name": "consult_cogny",
                       "arguments": json!({"question": "cost?", "consult_id": "c1"}).to_string()})
            .to_string()});
    h.tap(&call, "tool_calls", json!({}), json!([follow]));
    assert_eq!(pending(&h)["consult_id"], json!("c1"), "{:?}", h.stderr);
}
