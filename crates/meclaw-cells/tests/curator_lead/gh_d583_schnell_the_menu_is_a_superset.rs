//! Wave d58-3, strand `schnell` (GH #1529, contract point 2) -- the tool menu
//! of a session is a superset; the turn's choice is a hint and a refusal at
//! the call, never a new menu.
//! OR-583.stapel.8, GH #1333: the element on top stands directly before the
//! person's words, which stay the last message before the answer.
//!
//! WHY this file exists: the provider sends the tool menu before the system
//! part, so a menu that changes without a rebuild breaks the cached prefix
//! like a system part that changes (GH #1497, the watch counts it). The quick
//! way's scope (`fast_scope`) builds the menu anew for every turn out of the
//! stamp's families: two turns of one session that need other families sent
//! two menus, and every such turn was a prefix break -- a cache write of the
//! whole window. The owner's line (11.11.): tools that come in while the cache
//! is warm go on top, the system part is sorted only at a cache miss. Now the
//! menu of a session is the union of every family the target was allowed in
//! it; it grows and shrinks only at a rebuild. The curator still chooses per
//! turn, but as a hint in the element on top and as a refusal at the call
//! (`hop.tools_refused`, the road the consult's deny takes since GH #1497).
//! A tool the superset does not hold yet forces the break -- the provider
//! knows no other way to add one -- and the curator says why: reason
//! `menu_grow`; the watch counts it as a break as before.
//!
//! The pin `gh_d58_prefix_a_menu_that_changes_without_a_rebuild_counts_as_a_break`
//! stays green: the watch keeps counting real breaks.
//!
//! Runs the shipped curator hive with talky's profiles
//! (`support/curator_hive.rs`); no model. The families are ones outside the
//! fast target's class (`allow` of its profile), so the stamp alone decides
//! whether they are on the menu.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;

/// The element on top of `call` (OR-583.stapel.8, GH #1333): the message
/// directly before the person's words of the running turn (`words`), which
/// stay the last message before the answer.
fn on_top_of(call: &Msg, words: &str) -> Value {
    let m = call.messages();
    let at = m
        .iter()
        .rposition(|x| sj::to_string(x).unwrap_or_default().contains(words))
        .unwrap_or_else(|| panic!("check of the setup: the words {words:?} in {m:?}"));
    assert_eq!(
        at + 1,
        m.len(),
        "the person's words are the last message before the answer (GH #1333): {m:?}"
    );
    m[at.saturating_sub(1)].clone()
}
use meclaw_core::serde_json::{self as sj, Value, json};

/// The fast target's calls of a turn, side calls left out.
fn fast_call(h: &Hive) -> Msg {
    let calls: Vec<Msg> = h
        .routed("quick")
        .into_iter()
        .filter(|m| m.hop.get("call_kind").is_none())
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "check of the setup: one call of the fast target: {:?} {:?}",
        h.out,
        h.stderr
    );
    calls[0].clone()
}

/// One `simple` turn of session `s1` whose stamp names `families` and no
/// need; the call of the fast target before its answer and the watch's
/// `prefix_break` on it (read before the tap takes the row down).
fn turn(h: &mut Hive, turn: &str, words: &str, families: &str) -> (Msg, Value) {
    turn_on(h, turn, words, families, "simple")
}

/// [`turn`] stamped `path`.
fn turn_on(h: &mut Hive, turn: &str, words: &str, families: &str, path: &str) -> (Msg, Value) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": "", "families": families});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(words)], "system": mode("Be brief.")}),
    );
    let call = fast_call(h);
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let est: Value = sj::from_str(&h.state(&format!("window_est:{id}"))).unwrap_or(Value::Null);
    let brk = est["prefix_break"].clone();
    h.tap(&call, "stop", warm_usage(), json!([said("Done.")]));
    (call, brk)
}

/// The tool menu the model holds for `call` (its snapshot's `tools` entry).
fn menu(h: &Hive, call: &Msg) -> Value {
    let sent = sent_of(h, call)
        .unwrap_or_else(|| panic!("check of the setup: a snapshot of {:?}", call.hop));
    sent_window(h, &sent).tools.unwrap_or(Value::Null)
}

/// The fast target's hive with the snapshot of the window on.
fn hive() -> Hive {
    clear_of_midnight();
    Hive::with(&[("policy", "snapshot_window", json!("1"))]).reporting_composite("talky")
}

/// The names a hop key lists (a comma string).
fn listed(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_str)
        .unwrap_or("")
        .split(',')
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect()
}

/// Pin: the first turn allows `orga` and `story`; the second needs `orga`
/// only, the third `story` only. Both later calls send the menu of the first
/// -- the superset holds both families -- and the watch counts no break.
#[test]
fn gh_d583_schnell_turns_with_other_families_keep_the_menu() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let (c1, _) = turn(
        &mut h,
        "t1",
        "Put the doctor on my list and read me a story.",
        "orga,story",
    );
    let first = menu(&h, &c1);
    assert!(
        sj::to_string(&first).unwrap_or_default().contains("orga")
            && sj::to_string(&first).unwrap_or_default().contains("story"),
        "check of the setup: the first menu holds both families: {first}"
    );
    let (c2, b2) = turn(&mut h, "t2", "What is on my list?", "orga");
    let (c3, b3) = turn(&mut h, "t3", "Another story, please.", "story");
    assert_eq!(
        cut_epoch(&h, &c1),
        cut_epoch(&h, &c3),
        "check of the setup: no rebuild lay between the calls"
    );
    assert_eq!(menu(&h, &c2), first, "the second turn keeps the menu");
    assert_eq!(menu(&h, &c3), first, "the third turn keeps the menu");
    assert_eq!(b2, json!(0), "no prefix break on the second call");
    assert_eq!(b3, json!(0), "no prefix break on the third call");
}

/// Pin: a family on the menu that the turn's stamp does not name is refused
/// at the call (`hop.tools_refused`), the turn's own family is not; the
/// element at the end of the window names the turn's choice as a hint.
#[test]
fn gh_d583_schnell_a_family_outside_the_turn_is_refused_at_the_call() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    turn(
        &mut h,
        "t1",
        "Put the doctor on my list and read me a story.",
        "orga,story",
    );
    let (c2, _) = turn(&mut h, "t2", "What is on my list?", "orga");
    let refused = listed(c2.hop.get("tools_refused"));
    assert!(
        refused
            .iter()
            .any(|n| n == "story" || n.starts_with("story_")),
        "the family the turn does not name is refused at the call: {:?}",
        c2.hop
    );
    assert!(
        !refused
            .iter()
            .any(|n| n == "orga" || n.starts_with("orga_")),
        "the turn's own family is not refused: {refused:?}"
    );
    let last = on_top_of(&c2, "What is on my list?");
    let text = sj::to_string(&last).unwrap_or_default();
    assert!(
        text.contains("orga") && !text.contains("What is on my list?"),
        "the element on top names the turn's choice as a hint, right before the person's words: {last}"
    );
}

/// Pin: a family the superset does not hold yet forces the break -- the menu
/// grows, the watch counts the break, and the curator says why: `menu_grow`.
#[test]
fn gh_d583_schnell_a_tool_outside_the_superset_breaks_as_menu_grow() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let (c1, _) = turn(&mut h, "t1", "Put the doctor on my list.", "orga");
    assert!(
        !sj::to_string(&menu(&h, &c1))
            .unwrap_or_default()
            .contains("story"),
        "check of the setup: the first menu does not hold the new family"
    );
    let (c2, b2) = turn(&mut h, "t2", "Read me a story.", "story");
    assert!(
        sj::to_string(&menu(&h, &c2))
            .unwrap_or_default()
            .contains("story"),
        "the menu grows by the family the turn needs: {}",
        menu(&h, &c2)
    );
    assert_eq!(b2, json!(1), "the watch counts the break as before");
    let grows: Vec<Vec<Value>> = h
        .rows("SELECT kind, value FROM marks WHERE kind = 'menu_grow' OR value LIKE '%menu_grow%'");
    assert_eq!(
        grows.len(),
        1,
        "one mark of the curator says why: menu_grow -- {grows:?} {:?}",
        h.stderr
    );
}

/// A class whose entry takes no tool (`tools` "none", GH #1478), routed to
/// the fast target like `simple`.
const NO_TOOL_CLASS: &str = "vague";

/// [`hive`] with the class [`NO_TOOL_CLASS`]: its entry says `tools`
/// "none", and the lead routes it to the fast target.
fn hive_with_a_class_without_tools() -> Hive {
    clear_of_midnight();
    Hive::with(&[
        ("policy", "snapshot_window", json!("1")),
        (
            "policy",
            "class_notes",
            json!({NO_TOOL_CLASS: {"text": "Ask which one is meant.", "tools": "none"}}),
        ),
        (
            "lead",
            "path_routes",
            json!(format!(
                "talk:brain,simple:quick,{NO_TOOL_CLASS}:quick,complex:consult,research:research"
            )),
        ),
    ])
    .reporting_composite("talky")
}

/// The marks that name `menu_grow`.
fn menu_grows(h: &Hive) -> Vec<Vec<Value>> {
    h.rows("SELECT kind, value FROM marks WHERE kind = 'menu_grow' OR value LIKE '%menu_grow%'")
}

/// Pin (review I-1 of the strand, OR-583.schnell.5): a turn whose class
/// takes no tool is a choice of the turn, not a new menu -- the call sends
/// the session's menu, refuses every tool on it at the call and says in the
/// element at the end that this turn takes none; the turn after it sends the
/// same menu again. No break, no `menu_grow`.
#[test]
fn gh_d583_schnell_a_turn_without_tools_keeps_the_menu_and_refuses_it() {
    if !shipped() {
        return;
    }
    let mut h = hive_with_a_class_without_tools();
    let (c1, _) = turn(
        &mut h,
        "t1",
        "Put the doctor on my list and read me a story.",
        "orga,story",
    );
    let first = menu(&h, &c1);
    let (c2, b2) = turn_on(&mut h, "t2", "Which one?", "", NO_TOOL_CLASS);
    let (c3, b3) = turn(&mut h, "t3", "What is on my list?", "orga");
    assert_eq!(
        cut_epoch(&h, &c1),
        cut_epoch(&h, &c3),
        "check of the setup: no rebuild lay between the calls"
    );
    assert_eq!(
        menu(&h, &c2),
        first,
        "the turn without tools keeps the menu"
    );
    assert_eq!(menu(&h, &c3), first, "the turn after it keeps the menu");
    assert_eq!(b2, json!(0), "no prefix break on the turn without tools");
    assert_eq!(b3, json!(0), "no prefix break on the turn after it");
    let allowed: Vec<String> = c2
        .body
        .get("tool_scope")
        .and_then(|s| s.get("allow"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|n| n.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !allowed.is_empty(),
        "check of the setup: the menu holds tools: {:?}",
        c2.body.get("tool_scope")
    );
    let refused = listed(c2.hop.get("tools_refused"));
    for n in &allowed {
        assert!(
            refused.contains(n),
            "every tool on the menu is refused at the call: {n} not in {refused:?}"
        );
    }
    let last = on_top_of(&c2, "Which one?");
    let text = sj::to_string(&last).unwrap_or_default();
    assert!(
        text.contains("takes no tool") && !text.contains("Which one?"),
        "the element on top says the turn takes no tool, right before the person's words: {last}"
    );
    assert!(
        menu_grows(&h).is_empty(),
        "no menu grew: {:?}",
        menu_grows(&h)
    );
}

/// The block and the slot row of a slot the curator writes itself (the form
/// `./push` books its own slot in) -- a change of it orders the change of
/// plan (GH #1517 (d)).
fn the_curator_writes_a_slot(h: &Hive, path: &str, text: &str) {
    let body = canonical(&json!({"path": path, "text": text}));
    let hash = sha256_hex(&body);
    h.db.execute(
        "INSERT INTO blocks (hash, kind, chars, body, first_seen) VALUES (?1, 'system', ?2, ?3, 'x')",
        rusqlite::params![hash, text.len() as i64, body],
    )
    .expect("the slot's block");
    h.db.execute(
        "INSERT INTO slots (path, hash, owner, at) VALUES (?1, ?2, 'curator', 'x')",
        [path, hash.as_str()],
    )
    .expect("the slot's row");
}

/// Pin (review M-1 of the strand, contract point 2): the menu shrinks with
/// the next state of the plan -- after the rebuild a turn sends its own
/// choice, not the families the session needed before it.
#[test]
fn gh_d583_schnell_the_next_state_of_the_plan_shrinks_the_menu() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let (c1, _) = turn(
        &mut h,
        "t1",
        "Put the doctor on my list and read me a story.",
        "orga,story",
    );
    the_curator_writes_a_slot(&h, "facts.today", "The person's daughter visits on Sunday.");
    let (c2, _) = turn(&mut h, "t2", "What is on my list?", "orga");
    assert!(
        sj::to_string(&menu(&h, &c2))
            .unwrap_or_default()
            .contains("story"),
        "check of the setup: on the first state of the plan the menu holds both families"
    );
    let order = h
        .clock_order_for(ROUND_E)
        .unwrap_or_else(|| panic!("check of the setup: an order at the clock: {:?}", h.clock));
    h.fire(&order);
    let (c3, _) = turn(&mut h, "t3", "And what else?", "orga");
    assert_ne!(
        cut_epoch(&h, &c3),
        cut_epoch(&h, &c1),
        "check of the setup: the third turn is cut on the next state of the plan"
    );
    let third = sj::to_string(&menu(&h, &c3)).unwrap_or_default();
    assert!(
        third.contains("orga") && !third.contains("story"),
        "the next state of the plan holds the turn's own choice only: {third}"
    );
}

// ---------------------------------------------------------------------------
// Fix round 2 of the strand (re-check of fc3169233e).

/// The flat tools two folded families hold, two ops each, as the tools hive
/// declares them (provider-neutral; the collector wraps them).
const FLAT_TOOLS: [(&str, &str); 4] = [
    ("orga_add", "Add an item to the list."),
    ("orga_list", "List the items."),
    ("story_read", "Read a story aloud."),
    ("story_tell", "Tell a story of its own."),
];

/// The shipped collector's pure half under `params`, then the fold of
/// `menu` (`fold_menu`) as the `system.tools` subtree it hands on
/// (`tools_slot`) and the names of the families it folded.
const COLLECTOR_FOLD: &str = r#"
import ast, io, json, sys
inp = json.load(sys.stdin)
sys.stdin = io.StringIO("")
params = inp["params"]
scope = {"P": params, "doc": {"params": params, "body": {}, "envelope": {}}}
for n in ast.parse(inp["src"]).body:
    if isinstance(n, (ast.Import, ast.ImportFrom, ast.FunctionDef)) or (
            isinstance(n, ast.Assign)
            and all(isinstance(t, ast.Name) and t.id.isupper() for t in n.targets)):
        try:
            exec(compile(ast.Module(body=[n], type_ignores=[]), "collector", "exec"), scope)
        except Exception:
            if not isinstance(n, ast.Assign):
                raise
menu, fams, _ = scope["fold_menu"](inp["menu"])
print(json.dumps({"tools": scope["tools_slot"](menu), "folded": ",".join(fams)}))
"#;

/// The menu of [`FLAT_TOOLS`] as the SHIPPED talky collector folds it: its
/// `override_params` over the collector template's params, run through the
/// template's own `fold_menu` and `tools_slot`. Returns the `system.tools`
/// subtree and the seam's `tools_folded`.
fn talky_folded_menu() -> (Value, String) {
    let over =
        read_json(&repo("templates/talky/collector/config.json"))["override_params"]["assemble"]
            .clone();
    assert_eq!(
        over["fold_tools"],
        json!("1"),
        "check of the setup: the shipped talky folds its menu"
    );
    let cfg = read_json(&repo("templates/collector/assemble/config.json"));
    let src = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let mut params = cfg["params"].as_object().cloned().unwrap_or_default();
    params.remove("script_inline");
    for (k, v) in over.as_object().cloned().unwrap_or_default() {
        params.insert(k, v);
    }
    let menu: Vec<Value> = FLAT_TOOLS
        .iter()
        .map(|(name, what)| {
            json!({"name": name, "description": what,
                   "parameters": {"type": "object", "properties": {}}})
        })
        .collect();
    let doc = json!({"src": src, "params": params, "menu": menu});
    let out = run_python(COLLECTOR_FOLD, &doc.to_string());
    assert!(
        out.status.success(),
        "the collector's pure half does not load: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got: Value = sj::from_slice(&out.stdout).expect("the fold as JSON");
    let folded = got["folded"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        folded, "orga,story",
        "check of the setup: the collector folds both families: {got}"
    );
    (got["tools"].clone(), folded)
}

/// One curate of `session`'s turn on `path`, its stamp naming `families`,
/// the menu `tools` (a `system.tools` subtree, or null) with the seam's
/// `tools_folded`; the model call that left on `route`, side calls left out.
#[allow(clippy::too_many_arguments)]
fn call_on(
    h: &mut Hive,
    route: &str,
    session: &str,
    turn: &str,
    iter: u32,
    path: &str,
    families: &str,
    messages: Value,
    tools: &Value,
    folded: &str,
) -> Msg {
    h.out.clear();
    let mut hop = json!({"session_id": session, "turn_id": turn, "iter": iter.to_string(),
                         "phase": "", "path": path, "needs": "", "families": families});
    if !folded.is_empty() {
        hop["tools_folded"] = json!(folded);
    }
    let ctx = json!({"session_id": session, "turn_id": turn, "iter": iter.to_string(),
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    let mut system = mode("Be brief.");
    if tools.is_object() {
        system["tools"] = tools.clone();
    }
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": messages, "system": system}),
    );
    the_call(h, route)
}

/// The one model call on `route` among what left, side calls left out.
fn the_call(h: &Hive, route: &str) -> Msg {
    let calls: Vec<Msg> = h
        .routed(route)
        .into_iter()
        .filter(|m| m.hop.get("call_kind").is_none())
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "check of the setup: one call on {route}: {:?} {:?}",
        h.out,
        h.stderr
    );
    calls[0].clone()
}

/// The shipped dispatcher over a model answer of `calls` (`(id, name,
/// args)` in the provider's form) with the context the talky lifts onto the
/// brain's answer: `tools_folded` and the call's `tools_refused`. Returns
/// what it emits.
fn dispatch(folded: &str, refused: &str, calls: &[(&str, &str, Value)]) -> Vec<Value> {
    let cfg = read_json(&repo("templates/dispatcher/config.json"));
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let turns: Vec<Value> = calls
        .iter()
        .map(|(id, name, args)| {
            json!({"origin": "assistant", "type": "tool_call", "id": id,
                   "text": json!({"name": name, "arguments": args.to_string()}).to_string()})
        })
        .collect();
    let doc = json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t9", "iter": "0",
                               "tools_folded": folded, "tools_refused": refused},
                   "hop": {"finish_reason": "tool_calls"}},
        "messages": turns,
        "params": {}
    });
    let out = run_python(&script, &meclaw_testing::code_stdin(&doc).to_string());
    assert!(
        out.status.success(),
        "the dispatcher exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    sj::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "the dispatcher's output is no message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The ids the dispatcher answered `tool_refused`, and the tool names it let go.
fn refused_and_sent(out: &[Value]) -> (Vec<String>, Vec<String>) {
    let s = |m: &Value, k: &str| m["header"][k].as_str().unwrap_or_default().to_string();
    let refused = out
        .iter()
        .filter(|m| s(m, "route") == "result" && s(m, "error_code") == "tool_refused")
        .map(|m| s(m, "tool_call_id"))
        .collect();
    let sent = out
        .iter()
        .filter(|m| s(m, "route") == "tool")
        .map(|m| s(m, "tool_name"))
        .collect();
    (refused, sent)
}

/// Pin (re-check C-1 of fix round 1, OR-583.schnell.8): with the shipped
/// talky's fold on, the menu carries ONE leaf per family (`tools.story`,
/// an `op` enum), and the dispatcher unfolds `story{op}` to `story_<op>`
/// before it matches the refusal exactly. A folded family the turn does not
/// choose is refused in every unfolded form its leaf's `op` enum names --
/// the call `story{op: "read"}` comes back `tool_refused`, the turn's own
/// `orga{op: "add"}` leaves as `orga_add`.
#[test]
fn gh_d583_schnell_a_folded_family_outside_the_turn_is_refused_in_every_op() {
    if !shipped() {
        return;
    }
    let (tools, folded) = talky_folded_menu();
    let mut h = hive();
    let c1 = call_on(
        &mut h,
        "quick",
        "s1",
        "t1",
        0,
        "simple",
        "orga,story",
        json!([user("Put the doctor on my list and read me a story.")]),
        &tools,
        &folded,
    );
    h.tap(&c1, "stop", warm_usage(), json!([said("Done.")]));
    let c2 = call_on(
        &mut h,
        "quick",
        "s1",
        "t2",
        0,
        "simple",
        "orga",
        json!([user("What is on my list?")]),
        &tools,
        &folded,
    );
    let refused = listed(c2.hop.get("tools_refused"));
    for name in ["story", "story_read", "story_tell", "story_help"] {
        assert!(
            refused.iter().any(|n| n == name),
            "{name} is refused at the call: {refused:?}"
        );
    }
    assert!(
        !refused
            .iter()
            .any(|n| n == "orga" || n.starts_with("orga_")),
        "the turn's own family is not refused: {refused:?}"
    );
    let out = dispatch(
        &folded,
        &refused.join(","),
        &[
            ("c1", "story", json!({"op": "read", "args": {}})),
            (
                "c2",
                "orga",
                json!({"op": "add", "args": {"item": "doctor"}}),
            ),
        ],
    );
    let (no, sent) = refused_and_sent(&out);
    assert_eq!(
        no,
        vec!["c1".to_string()],
        "the folded call outside the turn is refused: {out:?}"
    );
    assert_eq!(
        sent,
        vec!["orga_add".to_string()],
        "the turn's own call leaves: {out:?}"
    );
}

/// Pin (re-check C-1, OR-583.schnell.8 with OR-.5): a turn whose class takes
/// no tool keeps the session's menu on a scoped call and refuses all of it
/// -- with the fold on, every unfolded op of every family on it.
#[test]
fn gh_d583_schnell_a_turn_without_tools_refuses_every_op_of_a_folded_family() {
    if !shipped() {
        return;
    }
    let (tools, folded) = talky_folded_menu();
    let mut h = hive_with_a_class_without_tools();
    let c1 = call_on(
        &mut h,
        "quick",
        "s1",
        "t1",
        0,
        "simple",
        "orga,story",
        json!([user("Put the doctor on my list and read me a story.")]),
        &tools,
        &folded,
    );
    h.tap(&c1, "stop", warm_usage(), json!([said("Done.")]));
    let c2 = call_on(
        &mut h,
        "quick",
        "s1",
        "t2",
        0,
        NO_TOOL_CLASS,
        "",
        json!([user("Which one?")]),
        &tools,
        &folded,
    );
    let refused = listed(c2.hop.get("tools_refused"));
    for name in [
        "orga_add",
        "orga_list",
        "orga_help",
        "story_read",
        "story_tell",
    ] {
        assert!(
            refused.iter().any(|n| n == name),
            "{name} is refused at the call: {refused:?}"
        );
    }
    let out = dispatch(
        &folded,
        &refused.join(","),
        &[
            (
                "c1",
                "orga",
                json!({"op": "add", "args": {"item": "doctor"}}),
            ),
            ("c2", "story", json!({"op": "tell"})),
        ],
    );
    let (no, sent) = refused_and_sent(&out);
    assert_eq!(
        no,
        vec!["c1".to_string(), "c2".to_string()],
        "no folded call of the menu leaves: {out:?}"
    );
    assert!(sent.is_empty(), "nothing is sent: {out:?}");
}

/// Pin (OR-583.schnell.8, the fallback of OR-.7): a refused folded family
/// whose leaf names no readable `op` enum cannot be refused op by op -- the
/// call sends the empty menu instead, so no tool of it slips through.
#[test]
fn gh_d583_schnell_a_folded_family_without_readable_ops_gets_the_empty_menu() {
    if !shipped() {
        return;
    }
    let (mut tools, folded) = talky_folded_menu();
    tools["story"] = json!({"text": json!({"type": "function", "function": {
        "name": "story", "description": "Stories.",
        "parameters": {"type": "object", "properties": {}}}}).to_string()});
    let mut h = hive();
    let c1 = call_on(
        &mut h,
        "quick",
        "s1",
        "t1",
        0,
        "simple",
        "orga,story",
        json!([user("Put the doctor on my list and read me a story.")]),
        &tools,
        &folded,
    );
    h.tap(&c1, "stop", warm_usage(), json!([said("Done.")]));
    let c2 = call_on(
        &mut h,
        "quick",
        "s1",
        "t2",
        0,
        "simple",
        "orga",
        json!([user("What is on my list?")]),
        &tools,
        &folded,
    );
    assert_eq!(
        c2.body.get("tool_scope").and_then(|s| s.get("allow")),
        Some(&json!([])),
        "the call sends the empty menu: {:?} {:?}",
        c2.body.get("tool_scope"),
        c2.hop.get("tools_refused")
    );
}

/// Pin (wave d58-3, stapel minor m-C of the re-check of fix round 2): when
/// the menu falls back to empty (OR-.8, an `op` enum it cannot read), the
/// turn's choice is no tool -- the element on top says so (`This turn takes
/// no tool.`) and never names a tool the empty menu does not hold. Before,
/// it said "Tools for this turn: orga" over a call that offered none.
#[test]
fn gh_d583_schnell_the_empty_menu_of_unreadable_ops_names_no_tool_on_top() {
    if !shipped() {
        return;
    }
    let (mut tools, folded) = talky_folded_menu();
    tools["story"] = json!({"text": json!({"type": "function", "function": {
        "name": "story", "description": "Stories.",
        "parameters": {"type": "object", "properties": {}}}}).to_string()});
    let mut h = hive();
    let c1 = call_on(
        &mut h,
        "quick",
        "s1",
        "t1",
        0,
        "simple",
        "orga,story",
        json!([user("Put the doctor on my list and read me a story.")]),
        &tools,
        &folded,
    );
    h.tap(&c1, "stop", warm_usage(), json!([said("Done.")]));
    let c2 = call_on(
        &mut h,
        "quick",
        "s1",
        "t2",
        0,
        "simple",
        "orga",
        json!([user("What is on my list?")]),
        &tools,
        &folded,
    );
    assert_eq!(
        c2.body.get("tool_scope").and_then(|s| s.get("allow")),
        Some(&json!([])),
        "check of the setup: the call sends the empty menu: {:?}",
        c2.body.get("tool_scope")
    );
    let text = sj::to_string(&c2.messages()).unwrap_or_default();
    assert!(
        !text.contains("Tools for this turn"),
        "the element names no tool over an empty menu: {text}"
    );
    assert!(
        text.contains("This turn takes no tool."),
        "the element says the turn takes no tool: {text}"
    );
}

/// Pin (wave d58-3, stapel minor m-B): the range read of a session's records
/// (`w-top`) keeps only the session's own keys, and a read that came back
/// full of them says so on stderr like the read of the targets does -- a
/// record past it would be read as none. The store's `where` takes one
/// operator per column (docs/cell-types.md § store), so the read cannot stop
/// at the end of the prefix; what lies past it is dropped here.
#[test]
fn gh_d583_schnell_the_read_of_a_sessions_records_says_when_it_came_back_full() {
    if !shipped() {
        return;
    }
    let probe = "[sorted(top_reads({'w-top': {'payload': [{'key': top_key(s, 't%02d' % i), \
                 'value': '{}'} for s, n in ARGS for i in range(n)]}}, 'w-top', 's1')), TOP_READ]";
    let (got, err) = policy_scope(json!({}), probe, json!([["s1", 3], ["s2", 29]]));
    assert_eq!(
        got[0],
        json!([
            "window_top:s1:t00",
            "window_top:s1:t01",
            "window_top:s1:t02"
        ]),
        "only the session's own records: {got}"
    );
    assert!(
        !err.contains("came back full"),
        "a read that ran past the session is not full: {err}"
    );
    let top_read = got[1].as_u64().expect("TOP_READ");
    let (got, err) = policy_scope(json!({}), probe, json!([["s1", top_read]]));
    assert_eq!(
        got[0].as_array().map(Vec::len),
        Some(top_read as usize),
        "{got}"
    );
    assert!(
        err.contains("came back full"),
        "a read full of the session's records says so: {err}"
    );
}

/// Pin (OR-583.schnell.10): a refused folded family that has no leaf
/// `tools.<F>` on this call's menu is no tool the call could name -- the
/// dispatcher unfolds only what the model was offered. It adds no op and
/// does not empty the menu; the fallback to the empty menu (OR-.8) holds
/// only for a family on the menu whose `op` enum cannot be read.
#[test]
fn gh_d583_schnell_a_folded_family_without_a_leaf_keeps_the_menu() {
    if !shipped() {
        return;
    }
    let (mut tools, folded) = talky_folded_menu();
    if let Some(t) = tools.as_object_mut() {
        t.remove("story");
    }
    let mut h = hive();
    let c1 = call_on(
        &mut h,
        "quick",
        "s1",
        "t1",
        0,
        "simple",
        "orga,story",
        json!([user("Put the doctor on my list and read me a story.")]),
        &tools,
        &folded,
    );
    h.tap(&c1, "stop", warm_usage(), json!([said("Done.")]));
    let c2 = call_on(
        &mut h,
        "quick",
        "s1",
        "t2",
        0,
        "simple",
        "orga",
        json!([user("What is on my list?")]),
        &tools,
        &folded,
    );
    let refused = listed(c2.hop.get("tools_refused"));
    assert!(
        refused.iter().any(|n| n == "story"),
        "check of the setup: the folded family outside the turn is refused: {refused:?}"
    );
    assert!(
        !refused.iter().any(|n| n.starts_with("story_")),
        "a family without a leaf brings no op: {refused:?}"
    );
    let allow = c2
        .body
        .get("tool_scope")
        .and_then(|s| s.get("allow"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        allow.iter().any(|n| n == "orga"),
        "the menu is not emptied, the turn's own family stays on it: {:?} {refused:?}",
        c2.body.get("tool_scope")
    );
}

/// The fact the curator writes while the cache is warm, for the pins below.
const FACT_TEXT: &str = "The person's daughter visits on Sunday.";

/// Pin (re-check I-A, OR-583.schnell.9): two targets of one session whose
/// calls run at once each keep what they wrote -- the brain the element on
/// top of its turn (its tool round shows it again), the fast target the menu
/// it grew (its next turn grows nothing).
#[test]
fn gh_d583_schnell_two_targets_of_a_session_keep_their_element_and_menu() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let b1 = call_on(
        &mut h,
        "brain",
        "s1",
        "t1",
        0,
        "talk",
        "",
        json!([user("Good morning.")]),
        &Value::Null,
        "",
    );
    h.tap(
        &b1,
        "stop",
        warm_usage(),
        json!([said("Good morning to you.")]),
    );
    let q1 = call_on(
        &mut h,
        "quick",
        "s1",
        "t2",
        0,
        "simple",
        "orga,story",
        json!([user("Put the doctor on my list and read me a story.")]),
        &Value::Null,
        "",
    );
    h.tap(&q1, "stop", warm_usage(), json!([said("Done.")]));
    the_curator_writes_a_slot(&h, "facts.today", FACT_TEXT);
    let lane = |turn: &str, path: &str, families: &str, words: &str| {
        (
            "in_curate",
            json!({"session_id": "s1", "turn_id": turn, "iter": "0", "channel": "test",
                   "audience_set": ROUND_E, "path": path}),
            json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                   "path": path, "needs": "", "families": families}),
            json!({"messages": [user(words)], "system": mode("Be brief.")}),
        )
    };
    h.out.clear();
    h.lanes_together(vec![
        lane("t3", "talk", "", "What is on this week?"),
        lane("t4", "simple", "orga,news", "Any news on my list?"),
    ]);
    let b3 = the_call(&h, "brain");
    let q4 = the_call(&h, "quick");
    for (c, words) in [
        (&b3, "What is on this week?"),
        (&q4, "Any news on my list?"),
    ] {
        assert!(
            sj::to_string(&on_top_of(c, words))
                .unwrap_or_default()
                .contains(FACT_TEXT),
            "check of the setup: the fact stands on top of both calls: {:?}",
            c.messages()
        );
    }
    assert_eq!(
        menu_grows(&h).len(),
        1,
        "check of the setup: the fast target's menu grew by `news`: {:?}",
        menu_grows(&h)
    );
    let id = "call_read_1";
    h.tap(
        &b3,
        "tool_calls",
        warm_usage(),
        json!([tool_call(id, "read_file")]),
    );
    h.tap(&q4, "stop", warm_usage(), json!([said("Nothing new.")]));
    let b3r = call_on(
        &mut h,
        "brain",
        "s1",
        "t3",
        1,
        "talk",
        "",
        json!([tool_call(id, "read_file"), tool_result(id, "file 1: abc")]),
        &Value::Null,
        "",
    );
    let (m1, m2) = (b3.messages(), b3r.messages());
    assert!(
        m2.len() > m1.len() && m2[..m1.len()] == m1[..],
        "the brain's tool round begins byte for byte with its turn's first call: {m1:?} / {m2:?}"
    );
    h.tap(&b3r, "stop", warm_usage(), json!([said("All read.")]));
    let q5 = call_on(
        &mut h,
        "quick",
        "s1",
        "t5",
        0,
        "simple",
        "news",
        json!([user("And the news?")]),
        &Value::Null,
        "",
    );
    assert_eq!(
        menu_grows(&h).len(),
        1,
        "the fast target kept the menu it grew -- nothing grows again: {:?}",
        menu_grows(&h)
    );
    assert_eq!(
        menu(&h, &q5),
        menu(&h, &q4),
        "the fast target's next turn sends the menu it grew"
    );
}

/// Pin (re-check I-B, M-4 of the strand): two sessions of one target each
/// keep their own menu -- a menu the second session grows is not the first
/// one's, and the first one's next turn sends its own menu unchanged.
#[test]
fn gh_d583_schnell_two_sessions_of_a_target_keep_their_own_menu() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let turn_in = |h: &mut Hive, session: &str, turn: &str, families: &str| {
        let c = call_on(
            h,
            "quick",
            session,
            turn,
            0,
            "simple",
            families,
            json!([user("Please help.")]),
            &Value::Null,
            "",
        );
        h.tap(&c, "stop", warm_usage(), json!([said("Done.")]));
        c
    };
    let a1 = turn_in(&mut h, "s1", "t1", "orga,story");
    let b1 = turn_in(&mut h, "s2", "u1", "news");
    let a2 = turn_in(&mut h, "s1", "t2", "orga");
    let second = sj::to_string(&menu(&h, &b1)).unwrap_or_default();
    assert!(
        second.contains("news") && !second.contains("orga") && !second.contains("story"),
        "the second session's menu is its own: {second}"
    );
    assert_eq!(
        menu(&h, &a2),
        menu(&h, &a1),
        "the first session's next turn sends its own menu"
    );
}

/// Pin (re-check I-B, the I-1 fix of the strand): a menu the session held
/// empty -- its first turn chose no tool -- grows like any other: the next
/// turn that needs a family breaks the prefix and says why (`menu_grow`).
#[test]
fn gh_d583_schnell_a_menu_held_empty_grows_as_menu_grow() {
    if !shipped() {
        return;
    }
    let mut h = hive_with_a_class_without_tools();
    let (c1, _) = turn_on(&mut h, "t1", "Which one?", "", NO_TOOL_CLASS);
    assert_eq!(
        c1.body.get("tool_scope").and_then(|s| s.get("allow")),
        Some(&json!([])),
        "check of the setup: the first turn chose no tool: {:?}",
        c1.body.get("tool_scope")
    );
    let (c2, _) = turn(&mut h, "t2", "Put the doctor on my list.", "orga");
    assert!(
        sj::to_string(&menu(&h, &c2))
            .unwrap_or_default()
            .contains("orga"),
        "check of the setup: the menu grows by the family: {}",
        menu(&h, &c2)
    );
    assert_eq!(
        menu_grows(&h).len(),
        1,
        "the curator says why the menu held empty grew: {:?} {:?}",
        menu_grows(&h),
        h.stderr
    );
}

/// The `spec_ahead` hive of the shipped talky: the spec call of a
/// turn goes to the fast target itself before the stamp, outside the menu.
fn spec_hive() -> Hive {
    clear_of_midnight();
    Hive::with(&[
        ("policy", "snapshot_window", json!("1")),
        ("lead", "spec_ahead", json!("1")),
        ("lead", "spec_tools", json!("web,weather")),
        // The spec call goes to the target of the normal call (`spec_target`
        // may name it, 10404 of the policy): the one case a text it kept
        // would bind.
        ("lead", "spec_target", json!("quick")),
    ])
    .reporting_composite("talky")
}

/// Pin (re-check I-B, the correction of fix round 1b): a spec call does not
/// fix the element on top of its turn. It leaves before the stamp and
/// without the turn's menu (no tool hint), so a text it kept would bind the
/// normal call of the same target and turn; the normal call shows its own
/// element -- the fact and the turn's choice of tools.
#[test]
fn gh_d583_schnell_a_spec_call_does_not_fix_the_element_of_its_turn() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive();
    turn(
        &mut h,
        "t1",
        "Put the doctor on my list and read me a story.",
        "orga,story",
    );
    the_curator_writes_a_slot(&h, "facts.today", FACT_TEXT);
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": "t2", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("What is on my list?")]}),
    );
    assert!(
        !h.routed("quick").is_empty(),
        "check of the setup: the spec call left: {:?} {:?}",
        h.stderr,
        h.lead_rows("SELECT key, value FROM state")
    );
    let spec = the_call(&h, "quick");
    assert_eq!(
        spec.hop.get("spec"),
        Some(&json!("1")),
        "check of the setup: the spec call"
    );
    let spec_top = sj::to_string(&on_top_of(&spec, "What is on my list?")).unwrap_or_default();
    assert!(
        spec_top.contains(FACT_TEXT) && !spec_top.contains("Tools for this turn"),
        "check of the setup: the spec call shows the fact without a tool hint: {spec_top}"
    );
    // The normal curate of the turn. While its spec call is out, the lead
    // waits for the spec answer -- unless the stamp dooms it (GH #1398): the
    // turn needs a tool of a family outside `spec_tools`, so the spec answer
    // is dropped whatever it says and the normal call leaves at once.
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "phase": "",
                     "path": "simple", "needs": "tool", "families": "orga"});
    let ctx = json!({"session_id": "s1", "turn_id": "t2", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("What is on my list?")], "system": mode("Be brief.")}),
    );
    let c2 = the_call(&h, "quick");
    assert!(
        c2.hop.get("spec").is_none(),
        "check of the setup: the normal call of the turn: {:?}",
        c2.hop
    );
    // The turn's choice is the class's own families and the stamp's
    // (`web, weather, orga` in the shipped talky); the spec call's element
    // has no such line.
    let top = sj::to_string(&on_top_of(&c2, "What is on my list?")).unwrap_or_default();
    let choice = top
        .split("Tools for this turn: ")
        .nth(1)
        .and_then(|r| r.split('.').next())
        .unwrap_or_default();
    assert!(
        top.contains(FACT_TEXT) && choice.split(", ").any(|f| f == "orga"),
        "the normal call shows its own element, the fact and the turn's choice: {top}"
    );
}
