//! GH #1411 — a cascade call speaks an answer while it is still being written
//! (`speak_partials: "1"`).
//!
//! The `llm` cell's `partial` lane carries the answer cumulated up to its last
//! sentence boundary, every partial a prefix of the next and of the finished
//! answer, all of them before the finished answer or the error. The handler is
//! driven directly, the I/O half is the fake synthesis: every `Speak` order on
//! the command seam is one call to the provider, in order, and that list is
//! what each test reads.

use meclaw_cells::voice::cell::{VoiceCell, VoiceEvent, VoiceReconfig};
use meclaw_cells::voice::contract::{ProviderTimeouts, SttEvent};
use meclaw_cells::voice::io::VoiceIo;
use meclaw_cells::voice::params::VoiceParams;
use meclaw_cells::voice::providers::build_stt;
use meclaw_cells::voice::wire::{Mode, SpeakEndReason};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OriginSink, OutputSink, Path, Uuid};
use std::time::Duration;
use tokio::sync::mpsc;

const CELL_PATH: &str = "/main/members/tester/channels/voice";
const CALL: &str = "call-1";

/// One order the fake synthesis received.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Order {
    Speak(String),
    Cancel,
}

struct Rig {
    cell: VoiceCell,
    db: DbConn,
    out: OutputSink,
    origin: OriginSink,
    seam: mpsc::Receiver<VoiceReconfig>,
    _rx: mpsc::Receiver<CellEmission>,
    /// The speak id of the synthesis running now, if any.
    running: Option<String>,
    /// Every order so far, in arrival order.
    orders: Vec<Order>,
}

fn db() -> DbConn {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS params (
             key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);",
    )
    .expect("the overlay table");
    DbConn::wrap(conn, None)
}

/// An echo cascade cell with one live call. `speak_partials` absent when
/// `None`, so the default is the cell's own.
async fn rig(speak_partials: Option<&str>) -> Rig {
    let path = Path::new(CELL_PATH);
    let mut raw = json!({"mount": "voice", "stt": {"provider": "echo"}});
    if let Some(v) = speak_partials {
        raw["speak_partials"] = json!(v);
    }
    let params = VoiceParams::parse(&raw).expect("the config parses");
    let stt = build_stt(&params.stt, ProviderTimeouts::default()).expect("the echo provider");
    let (events_tx, _events_rx) = mpsc::channel(8);
    let io = VoiceIo::new(
        params.mount.clone(),
        stt,
        None,
        Mode::Auto,
        Duration::from_millis(params.external_timeout_ms),
        Duration::from_millis(params.provider_idle_timeout_ms),
        events_tx,
    );
    let mut cell = VoiceCell::new(path.clone(), io, &params, &raw);
    let mut io = cell.split_io().expect("the i/o half");
    let seam = io.from_handler.take().expect("the command seam");
    let (tx, rx) = mpsc::channel::<CellEmission>(64);
    let out = OutputSink::new(
        tx.clone(),
        path.clone(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );
    let origin = OriginSink::new(tx, path.clone(), 64);
    let mut rig = Rig {
        cell,
        db: db(),
        out,
        origin,
        seam,
        _rx: rx,
        running: None,
        orders: Vec::new(),
    };
    rig.event(VoiceEvent::Connected {
        session_id: CALL.to_string(),
        mode: Mode::Auto,
        ack: None,
    })
    .await;
    rig
}

impl Rig {
    /// Read what the handler ordered so far (every send is awaited inside the
    /// handler, so it is on the seam when the call returns).
    fn drain(&mut self) {
        while let Ok(cmd) = self.seam.try_recv() {
            match cmd {
                VoiceReconfig::Speak { speak_id, text, .. } => {
                    self.running = Some(speak_id);
                    self.orders.push(Order::Speak(text));
                }
                VoiceReconfig::CancelSpeak { .. } => self.orders.push(Order::Cancel),
                _ => {}
            }
        }
    }

    async fn event(&mut self, event: VoiceEvent) {
        self.cell
            .handle_event(event, &self.origin, &mut self.db)
            .await;
        self.drain();
    }

    /// An answer message on `route` (`None` = no route, the finished answer).
    async fn say(&mut self, route: Option<&str>, text: &str, extra_hop: Value) {
        let mut ctx = Map::new();
        ctx.insert("call_id".to_string(), json!(CALL));
        let mut hop = Map::new();
        if let Some(r) = route {
            hop.insert("route".to_string(), json!(r));
        }
        if let Some(extra) = extra_hop.as_object() {
            hop.extend(extra.clone());
        }
        let msg = MessageBuilder::new(Path::new(CELL_PATH))
            .context(ctx)
            .hop(hop)
            .body(Body::Inline(json!({
                "messages": [{"origin": "assistant", "type": "text", "text": text}]
            })))
            .build();
        self.send(msg).await;
    }

    /// An error answer of the kind the `llm` cell sends (lane contract item 6),
    /// with an assistant text when a fallback wrote one.
    async fn fail(&mut self, text: Option<&str>) {
        let mut ctx = Map::new();
        ctx.insert("call_id".to_string(), json!(CALL));
        let mut hop = Map::new();
        hop.insert("finish_reason".to_string(), json!("error"));
        hop.insert("error_code".to_string(), json!("upstream_error"));
        let msg = MessageBuilder::new(Path::new(CELL_PATH))
            .context(ctx)
            .hop(hop)
            .body(Body::Inline(match text {
                Some(t) => {
                    json!({"messages": [{"origin": "assistant", "type": "text", "text": t}]})
                }
                None => json!({"messages": []}),
            }))
            .build();
        self.send(msg).await;
    }

    async fn send(&mut self, msg: meclaw_core::Message) {
        let (reconfig_tx, _r) = mpsc::channel(8);
        self.cell
            .handle(msg, &self.out, &mut self.db, &reconfig_tx)
            .await;
        self.drain();
    }

    /// The fake synthesis finishes the running order.
    async fn finish(&mut self) {
        let speak_id = self.running.take().expect("something is being spoken");
        self.event(VoiceEvent::SpeakEnded {
            session_id: CALL.to_string(),
            speak_id,
            reason: SpeakEndReason::Done,
            detail: None,
        })
        .await;
    }

    fn spoken(&self) -> Vec<String> {
        self.orders
            .iter()
            .filter_map(|o| match o {
                Order::Speak(t) => Some(t.clone()),
                Order::Cancel => None,
            })
            .collect()
    }
}

/// Pin 4: sentences go to the synthesis as they arrive, in order; the finished
/// answer says only the rest; an interim sentence already said by its partial
/// is not said again; a different answer is said whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sentences_are_spoken_as_they_arrive_without_repeat() {
    let mut r = rig(Some("1")).await;

    // The quick cell's interim sentence, first as a partial, then as the
    // interim answer itself.
    r.say(
        Some("partial"),
        "Hold on, I will check.",
        json!({"seq": 1, "final": false}),
    )
    .await;
    assert_eq!(
        r.spoken(),
        vec!["Hold on, I will check."],
        "the partial leaves at once"
    );
    r.say(None, "Hold on, I will check.", json!({"interim": "1"}))
        .await;
    assert_eq!(
        r.spoken().len(),
        1,
        "the interim already said is not said again"
    );
    r.finish().await;

    // The brain's answer, sentence by sentence.
    r.say(
        Some("partial"),
        "Tomorrow will be sunny.",
        json!({"seq": 1, "final": false}),
    )
    .await;
    r.say(
        Some("partial"),
        "Tomorrow will be sunny. Up to twenty degrees in the afternoon.",
        json!({"seq": 2, "final": false}),
    )
    .await;
    // The second sentence waits in the queue behind the first.
    assert_eq!(
        r.spoken().len(),
        2,
        "one synthesis at a time: {:?}",
        r.orders
    );
    r.finish().await;
    assert_eq!(
        r.spoken().last().map(String::as_str),
        Some("Up to twenty degrees in the afternoon."),
        "the queue hands over the next sentence in order"
    );
    r.say(
        None,
        "Tomorrow will be sunny. Up to twenty degrees in the afternoon. Shall I remind you?",
        json!({}),
    )
    .await;
    // The rest waited behind the second sentence; it runs now.
    r.finish().await;
    r.finish().await;
    r.say(None, "Something else entirely.", json!({})).await;

    assert_eq!(
        r.spoken(),
        vec![
            "Hold on, I will check.",
            "Tomorrow will be sunny.",
            "Up to twenty degrees in the afternoon.",
            "Shall I remind you?",
            "Something else entirely.",
        ],
        "order kept, the rest after the final, no sentence twice"
    );
}

/// An error after partials drops the rest: nothing is appended, and the next
/// answer starts clean.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_error_after_partials_drops_the_rest() {
    let mut r = rig(Some("1")).await;
    r.say(Some("partial"), "I will work it out.", json!({"seq": 1}))
        .await;
    r.fail(Some("Sorry, something went wrong.")).await;
    let emitted_before = r.orders.len();
    r.finish().await;
    assert_eq!(
        r.orders.len(),
        emitted_before,
        "nothing appended after the error"
    );
    // The caller's next turn starts clean: the same text as a new answer is
    // said whole, not read as a continuation.
    r.caller_turn("Und?").await;
    r.say(None, "I will work it out. Result: four.", json!({}))
        .await;
    assert_eq!(
        r.spoken(),
        vec!["I will work it out.", "I will work it out. Result: four."],
    );
}

/// A partial is not spoken while the caller speaks: once the caller started
/// talking over the answer (the existing barge-in), its later partials and its
/// finished form are dropped like a cancelled answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_partial_is_not_spoken_while_the_caller_speaks() {
    let mut r = rig(Some("1")).await;
    r.say(Some("partial"), "That is a long story.", json!({"seq": 1}))
        .await;
    r.event(VoiceEvent::Stt {
        session_id: CALL.to_string(),
        event: SttEvent::SpeechStarted,
    })
    .await;
    assert_eq!(
        r.orders.last(),
        Some(&Order::Cancel),
        "the barge-in cancels"
    );
    let speak_id = r.running.take().expect("running");
    r.event(VoiceEvent::SpeakEnded {
        session_id: CALL.to_string(),
        speak_id,
        reason: SpeakEndReason::Cancelled,
        detail: None,
    })
    .await;
    r.say(
        Some("partial"),
        "That is a long story. It begins in 1990.",
        json!({"seq": 2}),
    )
    .await;
    r.say(
        None,
        "That is a long story. It begins in 1990. And ends today.",
        json!({}),
    )
    .await;
    assert_eq!(
        r.spoken(),
        vec!["That is a long story."],
        "nothing of the interrupted answer is said after the barge-in"
    );
    // The next answer is a new one and is spoken.
    r.say(None, "Ja, bitte?", json!({})).await;
    assert_eq!(r.spoken().last().map(String::as_str), Some("Ja, bitte?"));
}

/// Pin 5 (voice part): off — absent or `"0"` — every answer is spoken whole,
/// exactly as before: one synthesis per message with the message's own text.
/// A partial of the model's answer (route `partial` on the voice input) is
/// dropped silently: the member wires the `partial` edge whatever the channel
/// says, and speaking it here would say every sentence twice (o41).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn off_leaves_every_channel_byte_identical() {
    for setting in [None, Some("0")] {
        let mut r = rig(setting).await;
        r.say(Some("partial"), "Erster Satz.", json!({"seq": 1}))
            .await;
        assert!(
            r.orders.is_empty(),
            "speak_partials {setting:?}: a partial says nothing"
        );
        r.say(None, "Erster Satz. Zweiter Satz.", json!({})).await;
        r.finish().await;
        r.say(None, "Erster Satz.", json!({"interim": "1"})).await;
        r.finish().await;
        r.say(Some("partial"), "After the error.", json!({"seq": 1}))
            .await;
        r.fail(None).await;
        assert_eq!(
            r.orders,
            vec![
                Order::Speak("Erster Satz. Zweiter Satz.".into()),
                Order::Speak("Erster Satz.".into()),
            ],
            "speak_partials {setting:?}: every answer whole, no partial, the error says nothing"
        );
    }
}

/// A value other than `"0"` | `"1"` is refused by name.
#[test]
fn a_foreign_value_is_refused() {
    let raw = json!({"mount": "voice", "stt": {"provider": "echo"}, "speak_partials": "yes"});
    let err = VoiceParams::parse(&raw).expect_err("refused");
    assert!(err.contains("speak_partials"), "{err}");
    let on = json!({"mount": "voice", "stt": {"provider": "echo"}, "speak_partials": "1"});
    assert!(VoiceParams::parse(&on).expect("parses").speak_partials);
}

impl Rig {
    async fn stt(&mut self, event: SttEvent) {
        self.event(VoiceEvent::Stt {
            session_id: CALL.to_string(),
            event,
        })
        .await;
    }

    /// The caller finishes a turn (auto mode, the provider's end of turn).
    async fn caller_turn(&mut self, text: &str) {
        self.stt(SttEvent::SpeechStarted).await;
        self.stt(SttEvent::EndOfTurn {
            text: text.to_string(),
        })
        .await;
    }

    /// Whether the handler emitted anything (an answer, a refusal) since the
    /// last call.
    fn emitted(&mut self) -> usize {
        let mut n = 0;
        while self._rx.try_recv().is_ok() {
            n += 1;
        }
        n
    }
}

/// I2: an answer left open — interrupted in the gap between two sentences,
/// or one whose finished form never came — ends with the caller's next turn.
/// It never swallows a later bridge sentence with the same words, nor a later
/// answer that starts with them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_cut_answer_never_swallows_a_later_one() {
    let mut r = rig(Some("1")).await;
    // Turn 1: the bridge sentence, then the caller cuts in between sentences.
    r.say(
        Some("partial"),
        "Hold on, I will check.",
        json!({"seq": 1, "partial_of": "q-1"}),
    )
    .await;
    r.finish().await;
    r.caller_turn("And tomorrow?").await;
    // Turn 2: the same bridge sentence again, then an answer starting with it.
    r.say(
        Some("partial"),
        "Hold on, I will check.",
        json!({"seq": 1, "partial_of": "q-2"}),
    )
    .await;
    r.finish().await;
    r.say(None, "Hold on, I will check.", json!({"interim": "1"}))
        .await;
    r.say(
        None,
        "Hold on, I will check. Tomorrow will be sunny.",
        json!({}),
    )
    .await;
    r.finish().await;
    // Turn 3: an open answer nobody cut (its finished form never came) ends
    // with the next caller turn as well.
    r.say(Some("partial"), "Just a moment.", json!({"seq": 1}))
        .await;
    r.finish().await;
    r.stt(SttEvent::EndOfTurn {
        text: "Hello?".to_string(),
    })
    .await;
    r.say(Some("partial"), "Just a moment.", json!({"seq": 1}))
        .await;
    assert_eq!(
        r.spoken(),
        vec![
            "Hold on, I will check.",
            "Hold on, I will check.",
            "Hold on, I will check. Tomorrow will be sunny.",
            "Just a moment.",
            "Just a moment.",
        ],
        "every turn says its own answer"
    );
}

/// I3: leading or trailing whitespace on either side never makes a sentence
/// be said twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn whitespace_around_the_text_never_says_a_sentence_twice() {
    let mut r = rig(Some("1")).await;
    r.say(
        Some("partial"),
        "\nHold on, I will check. ",
        json!({"seq": 1}),
    )
    .await;
    r.say(None, "Hold on, I will check. It is sunny.", json!({}))
        .await;
    r.finish().await;
    r.finish().await;
    r.say(Some("partial"), "Hello.", json!({"seq": 1})).await;
    r.say(None, "\n Hello. See you soon.\n", json!({})).await;
    r.finish().await;
    assert_eq!(
        r.spoken(),
        vec![
            "Hold on, I will check.",
            "It is sunny.",
            "Hello.",
            "See you soon.",
        ],
    );
}

/// I4: the caller cutting in while nothing is being synthesised — in the gap
/// between two sentences — ends the answer: its next sentences and its
/// finished form are not said.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_caller_cutting_in_between_two_sentences_ends_the_answer() {
    let mut r = rig(Some("1")).await;
    r.say(Some("partial"), "That is a long story.", json!({"seq": 1}))
        .await;
    r.finish().await;
    r.stt(SttEvent::SpeechStarted).await;
    assert!(
        !r.orders.contains(&Order::Cancel),
        "nothing was running, nothing to cancel"
    );
    r.say(
        Some("partial"),
        "That is a long story. It begins in 1990.",
        json!({"seq": 2}),
    )
    .await;
    r.say(
        None,
        "That is a long story. It begins in 1990. And ends today.",
        json!({}),
    )
    .await;
    assert_eq!(r.spoken(), vec!["That is a long story."]);
}

/// M1: the error of one branch (here the brain's) never makes the other
/// branch's sentence be said twice: the quick cell's interim, already said by
/// its partial, arrives after the error and is not said again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_error_of_one_branch_never_says_the_other_twice() {
    let mut r = rig(Some("1")).await;
    r.say(Some("partial"), "Hold on, I will check.", json!({"seq": 1}))
        .await;
    r.say(
        Some("partial"),
        "Tomorrow will be sunny.",
        json!({"seq": 1}),
    )
    .await;
    r.fail(None).await;
    r.say(None, "Hold on, I will check.", json!({"interim": "1"}))
        .await;
    r.finish().await;
    r.finish().await;
    assert_eq!(
        r.spoken(),
        vec!["Hold on, I will check.", "Tomorrow will be sunny."],
    );
}

/// Fix-Runde 2, Mangel 1: an answer the caller talked over stays silent when
/// its stream runs on past the end of the caller's turn — its next partial
/// (the same `partial_of`) is not read as a new answer — while the answer to
/// the new turn (another `partial_of`) with the same first words is said.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cut_answer_stays_silent_after_the_caller_turn_ends() {
    let mut r = rig(Some("1")).await;
    r.say(
        Some("partial"),
        "That is a long story.",
        json!({"seq": 1, "partial_of": "llm-1"}),
    )
    .await;
    r.finish().await;
    r.caller_turn("Briefly, please.").await;
    r.say(
        Some("partial"),
        "That is a long story. It begins in 1990.",
        json!({"seq": 2, "partial_of": "llm-1"}),
    )
    .await;
    r.say(
        Some("partial"),
        "That is a long story.",
        json!({"seq": 1, "partial_of": "llm-2"}),
    )
    .await;
    r.finish().await;
    r.say(
        Some("partial"),
        "That is a long story. In short: it went well.",
        json!({"seq": 2, "partial_of": "llm-2"}),
    )
    .await;
    assert_eq!(
        r.spoken(),
        vec![
            "That is a long story.",
            "That is a long story.",
            "In short: it went well.",
        ],
        "the cut answer says nothing more, the new one is said whole"
    );
}

/// Fix-Runde 2, Mangel 2: the error of one branch (here the quick cell's)
/// leaves the other branch's answer whole: the brain's later sentences and
/// the rest of its finished form are still said.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_error_of_one_branch_leaves_the_other_answer_whole() {
    let mut r = rig(Some("1")).await;
    r.say(
        Some("partial"),
        "Hold on, I will check.",
        json!({"seq": 1, "partial_of": "quick-1"}),
    )
    .await;
    r.say(
        Some("partial"),
        "Tomorrow will be sunny.",
        json!({"seq": 1, "partial_of": "brain-1"}),
    )
    .await;
    r.fail(None).await;
    r.say(
        Some("partial"),
        "Tomorrow will be sunny. At twenty degrees.",
        json!({"seq": 2, "partial_of": "brain-1"}),
    )
    .await;
    r.say(
        None,
        "Tomorrow will be sunny. At twenty degrees. Rain in the evening.",
        json!({}),
    )
    .await;
    for _ in 0..4 {
        r.finish().await;
    }
    assert_eq!(
        r.spoken(),
        vec![
            "Hold on, I will check.",
            "Tomorrow will be sunny.",
            "At twenty degrees.",
            "Rain in the evening.",
        ],
    );
}

/// M2: a partial the cell will not speak — off, or with no call to speak it
/// to — is dropped before any check that answers it with a refusal: one
/// refusal per sentence would be noise, the finished answer still meets them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_partial_never_draws_a_refusal() {
    let mut off = rig(Some("0")).await;
    off.emitted();
    let blob = MessageBuilder::new(Path::new(CELL_PATH))
        .hop({
            let mut hop = Map::new();
            hop.insert("route".to_string(), json!("partial"));
            hop
        })
        .body(Body::Blob(Uuid::now_v7()))
        .build();
    off.send(blob).await;
    assert_eq!(
        off.emitted(),
        0,
        "\"0\": a partial is dropped before the body is read"
    );

    let mut on = rig(Some("1")).await;
    on.emitted();
    let mut ctx = Map::new();
    ctx.insert("call_id".to_string(), json!("call-nobody"));
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("partial"));
    let stray = MessageBuilder::new(Path::new(CELL_PATH))
        .context(ctx)
        .hop(hop)
        .body(Body::Inline(json!({
            "messages": [{"origin": "assistant", "type": "text", "text": "Hello."}]
        })))
        .build();
    on.send(stray).await;
    assert_eq!(
        on.emitted(),
        0,
        "\"1\": a partial for no live call draws no refusal"
    );
}
