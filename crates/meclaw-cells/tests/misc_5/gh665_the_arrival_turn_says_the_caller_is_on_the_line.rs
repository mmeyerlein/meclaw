//! GH #665 — the arrival turn says what is true, and names nobody.
//!
//! The dialplan answers an inbound leg at once and starts its audio fork. The
//! README has said so since GH #614 and makes `call_incoming` the ONE turn of an
//! inbound call — *the moment a person is on the line*, whose answer is the
//! greeting. The turn the signal half first raised read `Incoming call from
//! +…` with `call_state: incoming`, and named only the number.
//!
//! Measured on a real call at 0.35.0: the assistant read that as a notice to the
//! owner, answered *"Incoming call from …. Shall I pick up?"* — and the answer
//! went into the line, to a caller who was already connected. The caller said
//! *"you are already on"*, the assistant said *"I take the call"* and *"right,
//! you are already through"*. Three turns lost before the conversation began.
//!
//! The repair — one sentence at both arrival places — first carried the number
//! and the member id in its own text, and a loopback showed what that costs: the
//! assistant read the id back to the caller, digit by digit, as a greeting.
//! A turn is answered, so everything in it can be spoken.
//!
//! The sentence now has three properties, each with its reason:
//!
//! - **No question.** A model answers what it is handed (ADR-0025, and the very
//!   argument with which GH #614 struck the second turn). A sentence ending in a
//!   question mark gets answered, and the answer goes into the line.
//! - **Nobody is named in it.** The number and the member id travel in the hop
//!   (`hop.number`, `hop.user_id`) where the member's firewall reads them; the
//!   text carries the situation and nothing that can be read out loud.
//! - **`call_state: live`.** The row moves to `live` in the same second anyway
//!   (`book("live", …)` right above), and `answered` is spoken for — it belongs
//!   to the turn of a call this channel PLACED.
//!
//! A drift lock in the sense of the development rules § 2d: the sentence is
//! greped out of the README's own `hop.route` table and asserted against the
//! shipped script, so prose and mechanism cannot part company.

const SIGNAL: &str = "templates/freeswitch/signal/config.json";
const README: &str = "templates/freeswitch/README.md";

/// What a caller and a member look like on the wire. Synthetic on purpose: the
/// point of the measurement is that NEITHER shape can reach the text.
const A_NUMBER: &str = "+15550100042";
const A_USER_ID: &str = "members/synthetic-member";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn script() -> String {
    let cfg: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_str(&read(SIGNAL)).expect("the signal half parses");
    cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// The sentence the README quotes for `call_incoming`, without its surrounding
/// prose — everything between the typographic quotes of that row.
fn quoted_in_the_readme() -> String {
    let table_row = read(README)
        .lines()
        .find(|l| l.starts_with("| `call_incoming` |"))
        .expect("the hop.route table names call_incoming")
        .to_string();
    let start = table_row.find('\u{201c}').expect("an opening quote");
    let rest = &table_row[start + '\u{201c}'.len_utf8()..];
    let end = rest.find('\u{201d}').expect("a closing quote");
    rest[..end].to_string()
}

#[test]
fn the_readme_and_the_script_speak_one_sentence() {
    let quoted = quoted_in_the_readme();
    assert_eq!(
        quoted, "The caller is on the line. Greet them.",
        "the README's own quote is what an inbound call is announced with, and \
         it has to be the repaired sentence"
    );
    assert!(
        !quoted.contains('?'),
        "a turn that ends in a question gets answered, and the answer goes into \
         the line: {quoted:?}"
    );

    // The mechanism: BOTH arrival places render it, with `call_state="live"`.
    let script = script();
    let rendered = "turn(\"The caller is on the line. Greet them.\",";
    let hits = script.matches(rendered).count();
    assert_eq!(
        hits, 2,
        "both arrival places speak the sentence — the free line (or the policy \
         taking the call) and the caller who was put through out of the queue. \
         For the model the situation is identical: greet them. What separates \
         the two is already where it belongs, in the receipt (`call_accepted` \
         with \"dequeued\"). Found {hits} rendering(s)."
    );
    assert_eq!(
        script.matches("call_state=\"live\"").count(),
        2,
        "and both carry the state the booking beside them is writing in the same \
         second — two names for one second would be prose that outlives the \
         mechanism"
    );
    assert!(
        !script.contains("Incoming call from"),
        "the old sentence describes a ringing telephone, and the situation is a \
         running conversation"
    );
    assert!(
        !script.contains("call_state=\"incoming\""),
        "and the state that went with it is gone too"
    );

    // Untouched on purpose (GH #614): the turn of a call this channel PLACED
    // keeps `answered`, which is why `live` and not `answered` is the new state.
    assert!(
        script.contains("call_state=\"answered\""),
        "an outbound call this channel placed still announces itself as answered"
    );
}

/// The measurement that follows the loopback: the text names nobody, and both
/// facts are still on the hop, where a firewall reads them and a model does not.
#[test]
fn the_arrival_turn_names_nobody_in_its_text() {
    let sentence = quoted_in_the_readme();

    // Nothing that can be read out loud. A number and a member id are digits and
    // a path; a sentence carrying neither cannot be greeted with either.
    assert!(
        !sentence.contains(A_NUMBER) && !sentence.contains(A_USER_ID),
        "no caller and no member reaches the text: {sentence:?}"
    );
    assert!(
        !sentence.chars().any(|c| c.is_ascii_digit()),
        "a digit in the arrival turn is a digit the assistant reads back to the \
         caller — the loopback of GH #665 heard exactly that: {sentence:?}"
    );
    assert!(
        !sentence.contains('%') && !sentence.contains('{'),
        "and the sentence is a literal, so nothing can be interpolated into it \
         later: {sentence:?}"
    );

    let script = script();

    // Neither arrival place formats anything into the text...
    assert_eq!(
        script.matches("% (num, uid)").count(),
        0,
        "the arrival sentence is rendered from no values at all"
    );

    // ...while both still hand `num` and `uid` to `turn()`, positionally.
    for (place, call) in [
        (
            "the free line",
            "turn(\"The caller is on the line. Greet them.\",\n                     arriving, num, uid, call_state=\"live\")",
        ),
        (
            "the caller put through from the queue",
            "turn(\"The caller is on the line. Greet them.\",\n                 waited, num, uid, call_state=\"live\")",
        ),
    ] {
        assert!(
            script.contains(call),
            "{place} still hands the caller and the member to `turn()`, which is \
             where they belong: on the hop, not in the text"
        );
    }

    // And `turn()` puts exactly those two on the hop and only `text` in the
    // message — the reason the text may stay anonymous without losing a fact.
    assert!(
        script.contains("\"number\": number, \"user_id\": user_id"),
        "`turn()` carries the caller and the member on the header, which the \
         ingress edge promotes into context and the member's firewall reads"
    );
    assert!(
        script
            .contains("\"messages\": [{\"origin\": \"user\", \"type\": \"text\", \"text\": text}]"),
        "and the message body carries the text and nothing else"
    );
}

#[test]
fn the_contract_says_what_live_means() {
    let cfg: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_str(&read(SIGNAL)).expect("the signal half parses");
    let described = cfg["contract"]["emits"]["hop"]["call_state"]["description"]
        .as_str()
        .expect("call_state carries a description rather than a values enum");
    assert!(
        described.contains("live"),
        "`call_state` is a free string, so the contract's own description is \
         where a new value is declared: {described}"
    );
}
