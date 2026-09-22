//! Welle Live, L2c — an `in_advise` carries its words in `payload`, and that is
//! where the cell reads them (GH #797).
//!
//! Measured against the twin on 2026-09-21: thirteen delegations answered, zero
//! paraphrases spoken. The producer — `templates/talky/splitter/config.json` —
//! cuts the `sidecar` block by top-level key and sends one message per section:
//!
//! ```python
//! for key, payload in sections.items():
//!     if not isinstance(payload, dict): dropped.append(key); continue
//!     out.append({"header": {"route": "sidecar", "section": key},
//!                 "messages": [], "section": key, "payload": payload})
//! ```
//!
//! So `payload` is the section's VALUE — `sections[key]`, always an object,
//! never `{section: value}` — and `messages[]` is empty. The measured body
//! `{"payload": {"fact": "…"}, "section": "fact"}` is a model that nested
//! twice, not the shape of the lane. These locks hold the sentences that come
//! out of that:
//!
//! * `payload` IS the section, and it is read before the older slots;
//! * the string in it is found by NAME — the section's own key, then `text`,
//!   then, since GH #799, `payload` (the wrapper the splitter writes around a
//!   bare string), and only then the first string in sorted key order — so no
//!   shape the block contract invites costs the caller the answer, and no rule
//!   depends on the order a map happens to iterate in;
//! * the older paths stay, and a section that really has no words is an
//!   `error bad_section` on the lane instead of a line nobody sees.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{Live, advise_msg_with_body, boot_fake, duplex_params, header, message_text};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl};
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::voice_client::VoiceClient;
use std::time::Duration;

/// Open a duplex cell with one connection on it.
async fn live_with_a_caller(session: &str) -> (Live, VoiceClient) {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (client, hello) = live.connect(&format!("session={session}&mode=auto")).await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    let _session = live.session().await;
    (live, client)
}

/// The body the splitter actually writes for one section: `payload` is the
/// section's value, with no key of the section's name around it.
fn splitter_body(section: &str, payload: Value) -> Value {
    json!({"messages": [], "payload": payload, "section": section})
}

/// The content of the one append the model was handed.
async fn one_append(live: &mut Live) -> (AppendKind, String) {
    match live.control().await {
        DuplexControl::Append { kind, content, .. } => (kind, content),
        other => panic!("expected one append, got {other:?}"),
    }
}

/// A `fact` in the producer's own shape reaches the model, word for word. This
/// is the shape of a model that followed the block preamble (`{"fact": {...}}`)
/// and wrote its sentence under `text`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fact_in_the_payload_reaches_the_model() {
    let (mut live, _client) = live_with_a_caller("payload-1").await;

    live.send(advise_msg_with_body(
        "payload-1",
        "fact",
        splitter_body("fact", json!({"text": "The appointment is on Thursday."})),
        Some("del-7"),
    ))
    .await;

    let (kind, content) = one_append(&mut live).await;
    assert_eq!(kind, AppendKind::Commentary, "a fact is what is said next");
    assert_eq!(
        content, "The appointment is on Thursday.",
        "the words of the section are the words of the append"
    );
}

/// The body GH #797 measured: a model that nested TWICE, so the section's own
/// name stands inside its own value. It was the one shape the twin produced and
/// it stays green.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_measured_double_nest_still_arrives() {
    let (mut live, _client) = live_with_a_caller("payload-2").await;

    live.send(advise_msg_with_body(
        "payload-2",
        "fact",
        splitter_body("fact", json!({"fact": "The practice is open until 6 pm."})),
        None,
    ))
    .await;

    assert_eq!(
        one_append(&mut live).await.1,
        "The practice is open until 6 pm."
    );
}

/// The two silent sections travel the same way — the lane is one lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn context_and_correction_read_the_payload_too() {
    let (mut live, _client) = live_with_a_caller("payload-3").await;

    live.send(advise_msg_with_body(
        "payload-3",
        "context",
        splitter_body("context", json!({"text": "The caller is a regular."})),
        None,
    ))
    .await;
    let (kind, content) = one_append(&mut live).await;
    assert_eq!(kind, AppendKind::Thinking);
    assert_eq!(content, "The caller is a regular.");

    live.send(advise_msg_with_body(
        "payload-3",
        "correction",
        splitter_body("correction", json!({"text": "Do not quote any prices."})),
        None,
    ))
    .await;
    let (kind, content) = one_append(&mut live).await;
    assert_eq!(kind, AppendKind::Instructions);
    assert_eq!(content, "Do not quote any prices.");
}

/// A bare string as the section value. The splitter dropped this one until
/// GH #799 and wraps it as `{"payload": "<string>"}` since -- this lock is
/// about the CELL either way: a payload that IS the sentence is the sentence,
/// whichever producer put it there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_string_payload_is_the_advice() {
    let (mut live, _client) = live_with_a_caller("payload-4").await;

    live.send(advise_msg_with_body(
        "payload-4",
        "fact",
        splitter_body("fact", json!("The bus leaves at ten past.")),
        None,
    ))
    .await;

    assert_eq!(one_append(&mut live).await.1, "The bus leaves at ten past.");
}

/// **The wrapper the splitter writes for a bare string** (GH #799), which is
/// the form 17 of 22 advise sections arrived in when the twin was measured on
/// 2026-09-21. The producer sends `{"payload": {"payload": "<sentence>"}}` --
/// the outer key is the body slot the lane declares, the inner one the wrapper
/// that lets a string ride an object slot -- so the consumer has to know the
/// name. It did not: the sentence came out of the sorted-key GUESS at the end
/// of `section_text`, which is the fallback for a shape nobody planned, and a
/// lane that carries three quarters of its traffic on a guess has no lock on
/// the shape at all. The second case is the one the guess gets wrong.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_splitters_own_wrapper_is_read_by_name() {
    let (mut live, _client) = live_with_a_caller("payload-9").await;

    live.send(advise_msg_with_body(
        "payload-9",
        "fact",
        splitter_body("fact", json!({"payload": "The bus leaves at ten past."})),
        None,
    ))
    .await;
    assert_eq!(
        one_append(&mut live).await.1,
        "The bus leaves at ten past.",
        "the wrapper the splitter itself writes carries the sentence"
    );

    live.send(advise_msg_with_body(
        "payload-9",
        "fact",
        splitter_body(
            "fact",
            json!({"annotation": "Just a note.", "payload": "The sentence."}),
        ),
        None,
    ))
    .await;
    assert_eq!(
        one_append(&mut live).await.1,
        "The sentence.",
        "`payload` is named before the guess: sorted key order would answer \
         `annotation` here, and the wrapper is not a shape to be guessed at -- \
         this cell's own producer writes it"
    );
}

/// Several keys in one section object, and the rule is a NAME rather than a
/// position: the section's own key first, then `text`. Both cases here would
/// pick the other string if the rule were "the first one in sorted key order"
/// (`context` < `fact`, `note` < `text`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn several_keys_are_read_by_name_and_not_by_position() {
    let (mut live, _client) = live_with_a_caller("payload-5").await;

    live.send(advise_msg_with_body(
        "payload-5",
        "fact",
        splitter_body(
            "fact",
            json!({"context": "Background, not the sentence.", "fact": "The sentence."}),
        ),
        None,
    ))
    .await;
    assert_eq!(
        one_append(&mut live).await.1,
        "The sentence.",
        "the section's own key wins over an earlier one"
    );

    live.send(advise_msg_with_body(
        "payload-5",
        "fact",
        splitter_body(
            "fact",
            json!({"note": "Just a note.", "text": "The second sentence."}),
        ),
        None,
    ))
    .await;
    assert_eq!(
        one_append(&mut live).await.1,
        "The second sentence.",
        "`text` wins where the section's own key is absent"
    );
}

/// Neither name is there: then the first string in SORTED key order is taken,
/// and sorted is not "as written". This object is written `second` before
/// `first`, so a rule that read the map in its own order would answer
/// differently the day `serde_json` runs with `preserve_order` — which is a
/// feature some other dependency can switch on without touching this cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unnamed_object_is_read_in_sorted_key_order() {
    let (mut live, _client) = live_with_a_caller("payload-6").await;

    live.send(advise_msg_with_body(
        "payload-6",
        "fact",
        splitter_body(
            "fact",
            json!({"second": "The second one.", "first": "The first one."}),
        ),
        None,
    ))
    .await;

    assert_eq!(one_append(&mut live).await.1, "The first one.");
}

/// The older paths keep working: an assistant turn and a bare `body.text` are
/// both still an advise, because a sender that uses them is not broken. They
/// are read AFTER the payload, and the last case here says so: where both
/// stand, the section wins, because `hop.section` named it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_older_paths_still_carry_a_section() {
    let (mut live, _client) = live_with_a_caller("payload-7").await;

    live.send(advise_msg_with_body(
        "payload-7",
        "fact",
        json!({"messages": [{"origin": "assistant", "type": "text", "text": "From the turn."}]}),
        None,
    ))
    .await;
    assert_eq!(one_append(&mut live).await.1, "From the turn.");

    live.send(advise_msg_with_body(
        "payload-7",
        "fact",
        json!({"messages": [], "text": "From the body."}),
        None,
    ))
    .await;
    assert_eq!(one_append(&mut live).await.1, "From the body.");

    live.send(advise_msg_with_body(
        "payload-7",
        "fact",
        json!({"messages": [{"origin": "assistant", "type": "text", "text": "From the turn."}],
               "payload": {"text": "From the section."},
               "section": "fact",
               "text": "From the body."}),
        None,
    ))
    .await;
    assert_eq!(
        one_append(&mut live).await.1,
        "From the section.",
        "the named slot wins over the two generic ones"
    );
}

/// A section that arrived with no words in it is said out loud on the error
/// lane. The `debug!` line it used to be is the reason this took a proof strand
/// to find: an advise that vanishes looks, from outside, like a model ignoring
/// an append.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_section_without_words_is_named_on_the_lane() {
    let (mut live, _client) = live_with_a_caller("payload-8").await;

    live.send(advise_msg_with_body(
        "payload-8",
        "fact",
        splitter_body("fact", json!({"fact": "   "})),
        None,
    ))
    .await;

    let refusal = live.emission("error").await;
    assert_eq!(
        header(&refusal, "error_code"),
        Some(&json!("bad_section")),
        "got {refusal}"
    );
    assert_eq!(header(&refusal, "call_id"), Some(&json!("payload-8")));
    assert_eq!(
        refusal["messages"],
        json!([]),
        "a refusal carries no words: {refusal}"
    );
    assert_eq!(message_text(&refusal, 0), "");
    let detail = refusal["meta"]["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("fact"),
        "the refusal names the section that was empty: {refusal}"
    );
    assert!(
        live.controls_for(Duration::from_millis(200))
            .await
            .is_empty(),
        "nothing reached the model"
    );
}
