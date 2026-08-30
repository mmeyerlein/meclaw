//! GH #517 — a round is provenance, and provenance is not derived from a path.
//!
//! The ingress edge of a grown channel declares `context.audience_set`: the
//! round every turn on that channel is spoken in. Until this repair the recipe
//! INVENTED it — it read the member's identity off the last segment of the
//! scope the wish had named:
//!
//! ```text
//! member = p["scope"].rstrip("/").rsplit("/", 1)[-1]   # the DIRECTORY name
//! ```
//!
//! which is the right answer only while the folder happens to be spelled like
//! the person. A member directory called `egon`, holding a person called
//! `marcus`, is the normal case rather than a pathology — and there the edge
//! declared `member:egon`, a participant that exists in no row of the store.
//!
//! What that costs is not a wrong string. `memory-hive/recall`'s gate admits a
//! row only when the declared round is a SUBSET of the row's own, so ONE wrong
//! name refuses EVERY row, in every leg, before the fusion. Measured on a live
//! colony (`mm-os-e14`, § N-3 of its rebuild receipt): 34 facts and 182
//! episodes, all carrying one correct round; with the round the recipe guessed,
//! keyword 0, semantic 0, graph 0, temporal 0, **zero** candidates, and a
//! bundle saying *"Nothing in this memory answers this question"* — word for
//! word what a genuinely empty store produces. The gate behaved exactly as
//! specified; the declaration handed to it was the lie. Nothing on the hop, in
//! the diagnostic or in the log told the two apart, because `leg_sizes_raw` is
//! post-gate by design (GH #297).
//!
//! So the person is a thing the WISH says, in the declaration's own `ctx` block,
//! and a wish that does not say it is **asked** — the same act `normalise`
//! already carries under `wish_incomplete` when a composer declines to invent a
//! model id. `templates/receptionist/greet` wrote the rule this recipe broke:
//! *"a channel identity is a room, not a participant, and turning one into the
//! other would invent a member nobody named."*
//!
//! Two smaller things on the same edge, and both are guards. `chat_id` and
//! `user_id` were promoted as bare reads. A modifier that fails to evaluate
//! skips the WHOLE edge, and the connector's own failure emissions carry
//! neither key — measured, every one of them logged
//! `cel eval set_context.chat_id: No such key: chat_id`.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, emit_one, shipped_script};
use std::path::PathBuf;

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const CLASSIFY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/classify/config.json"
);
const BRIEF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/brief/config.json"
);

/// The case the defect was found in: the member's directory is named after the
/// AGENT and the person it holds is somebody else. Both halves matter — a test
/// where the two coincide is the test that could not see this bug for four
/// releases, and `examples/organism` is exactly that shape (`alex` is the
/// folder and the person at once).
const MEMBER_DIR: &str = "/os/orgs/mm/members/egon";
const PERSON: &str = "marcus";
const AGENT: &str = "egon";

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(rel)
}

fn run_recipes(payload: Value) -> Value {
    emit_one(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": payload.to_string()}],
        }),
    )
}

fn run_classify(args: Value) -> Value {
    emit_one(
        &shipped_script(CLASSIFY),
        &json!({
            "target": "/os/builder/classify",
            "header": {"hop": {"route": "in_build"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_call", "id": "c1",
                          "text": args.to_string()}],
        }),
    )
}

/// The channel wish, with the person named — the form every caller has to use
/// from now on.
fn wish(person: Option<&str>) -> Value {
    let mut params = json!({
        "scope": MEMBER_DIR, "level": "channel", "name": "telegram",
        "template": "telegram-connector@2.0.1", "assistant": AGENT});
    if let Some(p) = person {
        params["ctx"] = json!({"member_person": p});
    }
    params
}

fn declaration(params: Value) -> Value {
    let out = run_recipes(json!({"recipe": "grow_level", "request": "…", "params": params}));
    out["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest: {out}"))[0]
        .clone()
}

/// The one edge that raises a turn: `./telegram -> .` on `!has(hop.error_code)`.
fn ingress(decl: &Value) -> Value {
    decl["diff"]["add_edges"]
        .as_array()
        .expect("add_edges")
        .iter()
        .find(|e| e["condition"] == json!("!has(hop.error_code)"))
        .expect("the ingress edge")
        .clone()
}

fn payload(out: &Value) -> Value {
    meclaw_core::serde_json::from_str(out["messages"][0]["text"].as_str().expect("a payload"))
        .expect("json payload")
}

// ══════════════════════════════════════════════ the round comes from the wish

/// The whole of the defect, in one assertion: the person the wish names is the
/// person in the round, and the directory the member stands in is nowhere.
#[test]
fn the_round_is_the_person_the_wish_named_and_never_the_directory() {
    let edge = ingress(&declaration(wish(Some(PERSON))));
    let round = edge["modifier"]["set_context"]["audience_set"]
        .as_str()
        .expect("the ingress edge declares a round");
    assert_eq!(
        round,
        format!("'[\"agent:{AGENT}\",\"member:{PERSON}\"]'"),
        "the round is the one the wish named"
    );
    assert!(
        !round.contains("member:egon"),
        "the round still carries the member's DIRECTORY name ({round}) — that \
         is the guess this issue is about: `member:egon` is a participant no \
         row of the store carries, and the audience gate refuses every row \
         against it, in every leg, silently"
    );
}

/// The claim is VISIBLE. A derivation nobody can see in the manifest is a
/// derivation nobody reviews — issue #517's own option 2. It rides in the
/// declaration's `ctx`, which the mutation door ignores (it checks the keys a
/// template REQUIRES and no others), so the cost of showing it is zero.
#[test]
fn the_person_stands_in_the_rendered_manifest_where_a_reviewer_sees_it() {
    let decl = declaration(wish(Some(PERSON)));
    assert_eq!(
        decl["ctx"]["member_person"],
        json!(PERSON),
        "the person the round was built from is not in the declaration: a \
         reviewer reading this manifest cannot tell a named person from a \
         guessed one, which is what made the live defect take a trace to find"
    );
}

// ══════════════════════════════════════════════════ a wish that did not say

/// Refused by NAME, with the question in the words a human has to answer. Not
/// `recipe_params_incomplete`: "you left an argument out of your tool call" and
/// "the wish never said who is in the room" call for opposite repairs.
#[test]
fn a_wish_that_names_no_person_is_asked_rather_than_guessed_at() {
    let out = run_recipes(json!({"recipe": "grow_level", "request": "…",
                                 "params": wish(None)}));
    assert_eq!(
        out["header"]["error_code"],
        json!("wish_incomplete"),
        "the renderer invented a round instead of asking for one"
    );
    assert!(
        out["manifest"].is_null(),
        "no manifest slot on a refusal — an empty manifest is a failure wearing \
         the face of an honest answer (GH #308)"
    );
    let said = payload(&out);
    assert_eq!(said["missing"], json!(["ctx.member_person"]));
    let question = said["reason"].as_str().expect("the question, verbatim");
    assert!(
        question.contains("PERSON") && question.contains("member_person"),
        "the refusal does not name what it needs — a refusal a human cannot \
         read is one they cannot answer: {question:?}"
    );

    // And the switch refuses it one cell earlier, before an inference is
    // bought. The table is held in both places for exactly this reason.
    let early = run_classify(json!({"request": "…", "recipe": "grow_level",
                                    "params": wish(None)}));
    assert_eq!(early["header"]["error_code"], json!("wish_incomplete"));
    assert_eq!(early["header"]["route"], json!("error"));
    assert_eq!(payload(&early)["missing"], json!(["ctx.member_person"]));
}

/// The other half of the switch's rule, unchanged: a SENTENCE that names no
/// person is not an error, because nobody named a recipe. It falls through to
/// the design lane, where the composer asks the same question out of the brief.
#[test]
fn a_grow_sentence_without_a_person_falls_to_the_design_lane_not_to_an_error() {
    let sentence =
        format!("grow a channel named telegram from telegram-connector@2.0.1 under {MEMBER_DIR}");
    let out = run_classify(json!({"request": sentence, "assistant": AGENT}));
    assert_eq!(
        out["header"]["route"],
        json!("design"),
        "a half-read wish belongs in the design lane, not in a recipe that \
         guesses the missing half"
    );

    // Told who the person is, the same sentence takes the fast lane again.
    let told = run_classify(json!({"request": sentence, "assistant": AGENT,
                                   "ctx": {"member_person": PERSON}}));
    assert_eq!(told["header"]["route"], json!("recipe"));
    assert_eq!(told["header"]["recipe"], json!("grow_level"));
}

/// A code that travels must be declared. Both cells emit it, and the hive's
/// error edge carries it on `hop.operation` plus `hop.error_code` — no new edge
/// was needed, which is the reason a NAMED code is affordable at all here.
#[test]
fn both_cells_publish_the_code_they_now_emit() {
    for path in [RECIPES, CLASSIFY] {
        let raw = std::fs::read_to_string(path).expect("the shipped config");
        let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
        let values = cfg["contract"]["emits"]["hop"]["error_code"]["values"]
            .as_array()
            .expect("the error_code enum");
        assert!(
            values.contains(&json!("wish_incomplete")),
            "{path} emits `wish_incomplete` and does not declare it — the \
             documented error_code strings are public contract (README § \
             Stability)"
        );
    }
}

// ═══════════════════════════════════════════════ nothing is read unguarded

/// Every promotion on the ingress edge is `has(...) ? ... : ''`, which is the
/// rule `templates/member/README.md` publishes one storey up and the reason it
/// gives: a modifier that fails to evaluate SKIPS THE WHOLE EDGE, so a turn
/// that vanishes on an edge is invisible, while a turn arriving with an empty
/// key is refused by the holder on a lane that is drained, with a reason.
///
/// The connector emits on this wire twice: an inbound message, and its own
/// failure. The failure carries no chat and no user, and `chat_id: hop.chat_id`
/// logged `No such key: chat_id` on every one of them.
#[test]
fn no_promotion_on_the_ingress_edge_reads_a_hop_key_unguarded() {
    let edge = ingress(&declaration(wish(Some(PERSON))));
    let set = edge["modifier"]["set_context"]
        .as_object()
        .expect("set_context");
    for (key, expr) in set {
        let expr = expr.as_str().expect("a CEL expression");
        if !expr.contains("hop.") {
            continue; // a literal reads nothing and cannot fail
        }
        assert!(
            expr.contains("has(hop."),
            "`{key}` reads a hop key without `has(...)`: {expr:?} — the \
             connector's own failure emissions carry none of them, and an \
             unguarded read makes the modifier fail, which skips the whole edge"
        );
    }
    assert_eq!(
        set["chat_id"],
        json!("has(hop.chat_id) ? hop.chat_id : ''"),
        "the reply has no chat to go to without it, and it must not be the \
         thing that swallows an error emission"
    );
    assert_eq!(set["user_id"], json!("has(hop.user_id) ? hop.user_id : ''"));
}

/// The channel key and the answer's way back are ONE decision, and GH #522
/// took it. #517 left the key alone deliberately: `context.channel` was the
/// node name, although `templates/session-keeper/README.md` describes it as the
/// chat identity and e9's hand-drawn edge promoted `hop.chat_id` into it. It
/// had to be, because the third rendered edge is the answer's way back —
/// `. -> ./telegram`, and `Edge.to` is a static path in this substrate, so it
/// has to say WHICH child of the container it is for (the address rule, GH
/// #454). A `channel` carrying a chat id routed no answer anywhere.
///
/// So the address moved to a key of its own. This test stays here, on the #517
/// file, because the two halves are still one decision: whoever changes the
/// promotion has to move the guard in the same breath, and the assertion below
/// says so in both directions.
#[test]
fn the_channel_key_and_the_answers_way_back_are_one_decision() {
    let decl = declaration(wish(Some(PERSON)));
    let edges = decl["diff"]["add_edges"].as_array().expect("add_edges");
    let ingress = ingress(&decl);
    let set = &ingress["modifier"]["set_context"];
    let back = edges
        .iter()
        .find(|e| e["from"] == json!(".") && e["to"] == json!("./telegram"))
        .expect("the answer's way back");
    let guard = back["condition"].as_str().expect("a condition");
    assert_eq!(
        set["channel_node"],
        json!("'telegram'"),
        "the ingress edge stopped promoting the node name — the answer edge \
         below is guarded on it, and without it every answer of this channel \
         is dropped at the container"
    );
    assert_eq!(
        set["channel"],
        json!("has(hop.chat_id) ? hop.chat_id : ''"),
        "`context.channel` is the CHAT (GH #522). Put the node name back here \
         and every chat of this connector shares one session generation, one \
         rate bucket and one memory room"
    );
    assert!(
        guard.contains("context.channel_node == 'telegram'"),
        "the answer edge stopped routing on `context.channel_node` \
         ({guard:?}) — the two are one decision, and this is where they are \
         taken together"
    );
    assert!(
        !guard.contains("context.channel =="),
        "the answer edge routes on the CHAT again ({guard:?}) — a container \
         may hold several channels and a chat id names none of them"
    );
}

// ════════════════════════════════════════════════ the example and the brief

/// The byte pin, from the other side. `gh466_grow_level_renders_the_level.rs`
/// compares the whole edge set; this one asserts that the SHIPPED example is
/// the repaired form — an example carrying the old literal is the template the
/// design lane copies from, and the corpus row is generated out of it.
#[test]
fn the_shipped_example_carries_the_repaired_form() {
    let Ok(raw) = std::fs::read_to_string(repo("examples/organism/grow-channel.json")) else {
        return; // a tree without the examples cannot make this assertion
    };
    let decl: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    assert_eq!(
        decl["ctx"]["member_person"],
        json!("alex"),
        "the example does not name the person its round is built from"
    );
    let edge = ingress(&decl);
    let set = &edge["modifier"]["set_context"];
    assert_eq!(
        set["audience_set"],
        json!("'[\"agent:scribe\",\"member:alex\"]'")
    );
    assert_eq!(set["chat_id"], json!("has(hop.chat_id) ? hop.chat_id : ''"));
    assert_eq!(set["user_id"], json!("has(hop.user_id) ? hop.user_id : ''"));
}

/// The design lane is the other half, and it needs the same rule in prose — a
/// composer that copies the corpus row and substitutes a name would otherwise
/// substitute the folder's. The brief has to say three things: that the round
/// is not derivable, where it goes, and that the answer to a wish that omits it
/// is the QUESTION form rather than a plausible literal.
#[test]
fn the_briefing_makes_the_round_a_question_rather_than_a_guess() {
    let out = emit_all(
        &shipped_script(BRIEF),
        &json!({
            "target": "/os/builder/brief",
            "header": {"hop": {"route": "brief"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "user", "type": "text", "id": "",
                          "text": "grow a channel"}],
        }),
    );
    let brief = out
        .into_iter()
        .find(|m| m["header"]["route"] == "compose")
        .expect("the brief's leg to the composer");
    let text = brief["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions");
    for phrase in [
        "THE ROUND OF A CHANNEL IS NOT YOURS TO INVENT",
        "member_person",
        "{\"question\"",
    ] {
        assert!(
            text.contains(phrase),
            "the briefing does not carry {phrase:?} — a model told to copy a \
             level's edge set out of the corpus and substitute the child's name \
             will substitute the folder's name for the person's, which is the \
             defect one lane over"
        );
    }
}
