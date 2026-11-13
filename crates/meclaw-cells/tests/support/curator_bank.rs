//! The bank of constructed windows (GH #1465): walls with counted contents,
//! the rebuild each of them is run through, and the statements that hold
//! after it.
//!
//! WHY this file exists: what the garbage collection of a rebuild is worth
//! was measured on live runs only, where nobody knows what a wall held. A
//! wall of the bank is built by hand, so every row of it is counted before
//! the rebuild: this is garbage of that class, these are the person's words,
//! this is an answer, this is a tool that failed. After the rebuild the
//! statements below read the window the next call is sent and the marks the
//! rebuild wrote, and compare them with the count.
//!
//! The form, for whoever adds a wall:
//!
//! * A wall is a file `support/curator_bank/<wall>.rs` with one function
//!   `register(&mut Bank)`. The nine seats are declared at the end of this
//!   file; a seat whose file registers nothing is named by `Bank::pending`.
//!   A wall is added in its own file, never here.
//! * `WallBuilder` sows the rows. Every row gets a keyword of its own (the
//!   placeholder `{k}` in its text), so a statement can say which row it
//!   missed, and a piece of garbage is known to be gone when its keyword is.
//! * A wall names its runs (`Run`): the example value set its model runs
//!   with is a parameter of the run, and so is every knob a run sets beside
//!   it. The curator keeps no roles (D58 target, R-58-15): the hive's one
//!   target `brain` reports the set as its profile (`window_profile` on
//!   `in_catalog`, `support/curator_profiles.rs`), holding its tree, and
//!   every curate names that target (`hop.target`). No statement branches
//!   on a set; what a run may expect of a class is read off the knobs the
//!   policy itself reports for that profile (`Knobs`, `Bank::rules`).
//! * A row is written into the ledger at its instant, or -- a tool round
//!   whose way into the ledger is what a wall is about, like a tool that
//!   failed with a code in its header -- it travels the lanes of the hive
//!   as a live round does (`WallBuilder::lane_round`).
//! * `rebuild` runs a wall through the shipped hive: the rows are sown, one
//!   turn finds the window at its target, the clock order of that turn is
//!   fired, the summarizer is answered by a stub, and one more turn shows
//!   the window the rebuild left.
//! * The six statements are `garbage_is_out`, `talk_is_whole`,
//!   `errors_stand`, `lands_at_aim`, `marks_count` and `snapshot_shows`;
//!   `open_errands_stand` is the half of the first that names what must not
//!   be collected, and `outcomes_stand` the statement of an errand that
//!   ended without an answer (GH #1469).
//!
//! Time: the shipped programs read the wall clock for the instant a rebuild
//! plans at (expiry, the deadline of an errand, the age of a day). The bank
//! runs them behind `CLOCK`, the clock of the policy corpus (GH #1446), set
//! to the wall's `now`.
//! Nothing here waits and nothing is compared with the clock of the machine.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`).
#![allow(dead_code)]

use crate::curator_hive::*;
use crate::curator_profiles::{self, VALUE_SETS};
use chrono::{DateTime, Duration, TimeZone, Utc};
use meclaw_core::serde_json::{self as sj, Value, json};

/// The clock in front of every shipped program, as the policy corpus runs
/// them (`gh_d38_c0_policy_corpus_is_byte_identical`, the same text): it
/// replaces `datetime.datetime.now`/`utcnow`, `time.time`/`time_ns` and
/// `uuid.uuid4` by readings of the instant the harness writes as the first
/// key of the stdin document (`Hive::set_clock`).
pub const CLOCK: &str = r#"import datetime as _c0_dt, time as _c0_time, uuid as _c0_uuid, sys as _c0_sys, io as _c0_io, re as _c0_re
if not hasattr(_c0_dt, "_c0_state"):
    _c0_dt._c0_state = {"us": 0, "k": 0, "u": 0}
    _c0_real = _c0_dt.datetime
    _c0_epoch = _c0_real(1970, 1, 1, tzinfo=_c0_dt.timezone.utc)

    def _c0_tick():
        st = _c0_dt._c0_state
        st["k"] += 1
        return st["us"] + st["k"]

    class _C0DateTime(_c0_real):
        @classmethod
        def now(cls, tz=None):
            t = _c0_epoch + _c0_dt.timedelta(microseconds=_c0_tick())
            t = cls.combine(t.date(), t.timetz())
            return t.astimezone(tz) if tz is not None else t.replace(tzinfo=None)

        @classmethod
        def utcnow(cls):
            return cls.now(None)

    _c0_dt.datetime = _C0DateTime
    _c0_time.time = lambda: _c0_tick() / 1e6
    _c0_time.time_ns = lambda: _c0_tick() * 1000
    def _c0_uuid4():
        st = _c0_dt._c0_state
        st["u"] += 1
        return _c0_uuid.UUID(int=st["us"] << 64 | st["u"] << 8 | 0xc0, version=4)

    _c0_uuid.uuid4 = _c0_uuid4
_c0_text = _c0_sys.stdin.read()
_c0_sys.stdin = _c0_io.StringIO(_c0_text)
_c0_m = _c0_re.match(r'\{"c0_clock_us":(\d+),', _c0_text)
if _c0_m:
    _c0_dt._c0_state["us"] = int(_c0_m.group(1))
    _c0_dt._c0_state["k"] = 0
    _c0_dt._c0_state["u"] = 0
del _c0_text, _c0_m"#;

/// The day every wall of the bank ends on: 2026-03-02, a wall names its
/// instants as offsets of its midnight (`day`).
pub const DAY_S: i64 = 1_772_409_600;

/// `days` days before or after the bank's day, at `h`:`m`:00 UTC.
pub fn day(days: i64, h: i64, m: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(DAY_S + days * 86_400 + h * 3600 + m * 60, 0)
        .single()
        .expect("a stamp")
}

/// An instant as the wall and the tools write it (RFC 3339, seconds, `Z`).
pub fn rfc(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// What the head of a one-line form shows of a block: its first line, cut
/// at this many characters (`LINE_CHARS` of the policy). A keyword behind it
/// is gone when the block is shown in its form.
pub const FORM_HEAD: usize = 120;

/// The measured prompt the turn in front of the rebuild reports: far above
/// every wall, under the soft line of the package, so the window of the
/// turn is the policy's own projection of the call (`window_of`).
const MEASURED: i64 = 240_000;

/// The measured prompt the turn in front of a rebuild for an expired cache
/// reports (`Trigger::Cache`): under every target, so the turn crosses no
/// line and its order is the one of the cache.
const MEASURED_WARM: i64 = 1;

/// How long the cache of that turn stays warm: the moment its order names.
const CACHE_TTL_S: i64 = 300;

/// The package the turns of a run report on the tap.
const INPUT_SOFT: i64 = 250_000;
const INPUT_HARD: i64 = 1_000_000;

/// What a row of a wall is counted as.
#[derive(Clone, Debug, PartialEq)]
pub enum What {
    /// The person's words.
    Words,
    /// The companion's answer -- a bridge or a sentence said ahead among them.
    Answer,
    /// A tool result that failed.
    Error,
    /// Garbage of the class named: a key of the `classes` of the `gc` mark.
    Junk(&'static str),
    /// What the class named would collect, were its errand over: it is not,
    /// and the row stays.
    Open(&'static str),
    /// Anything else a wall is made of (a tool call, a result still fresh):
    /// no statement of its own.
    Other,
}

/// One row of a wall, counted.
#[derive(Clone, Debug)]
pub struct Piece {
    pub what: What,
    /// The keyword only this row carries.
    pub key: String,
    pub at: DateTime<Utc>,
    pub session: String,
    pub turn: String,
    /// The ledger kind of the row, as `./intake` names it. The kind of a
    /// row of a lane round is `./intake`'s own to give (it files the pair of
    /// a recall tool as `recall`); the field holds the kind of a plain
    /// round there, and nothing sows by it.
    pub kind: String,
    pub el: Value,
    pub final_: i64,
    /// Whether the row reaches the ledger through the lanes of the hive
    /// (`WallBuilder::lane_round`) instead of being written into it.
    pub lane: bool,
    /// The `error_code` in the header a tool result of a lane round came
    /// back with ("" for none). It is no part of the element: the collector
    /// names it per call id in the `tool_errors` slot of the curate.
    pub code: String,
    /// How the errand this row hands on ends, for the `(delegation)` row of
    /// an errand round (`WallBuilder::errand_round`); `None` for every other
    /// row.
    pub errand: Option<Ending>,
}

/// How an errand of a wall ends without an answer (GH #1469).
#[derive(Clone, Debug, PartialEq)]
pub enum Ending {
    /// The call of its round comes back with `finish_reason` "error" and
    /// this `error_code` in its header.
    Failed(&'static str),
    /// Nobody answers before the deadline.
    Silent,
}

impl Ending {
    /// The outcome as the row of the curator says it, and the code beside
    /// it ("" for none).
    pub fn said(&self) -> (&'static str, &'static str) {
        match self {
            Ending::Failed(code) => ("failed", *code),
            Ending::Silent => ("no answer", ""),
        }
    }
}

impl Piece {
    pub fn text(&self) -> &str {
        self.el["text"].as_str().unwrap_or("")
    }

    /// The short id the window names the block by.
    pub fn id(&self) -> String {
        short_id(&self.el)
    }
}

/// What orders the rebuild of a run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trigger {
    /// The turn finds the window at its target (`curate_mark` "target"),
    /// the rebuild aims at the aim of the target.
    Target,
    /// The turn crosses no line; its cache runs out (`curate_mark` "none",
    /// the order of the cache, GH #1464). The aim is the one the order
    /// inherited; with `gc_on_cold` "1" the rebuild under it only collects.
    Cache,
}

/// One run of a wall: the example value set of its target, the knobs set
/// beside it, and what orders its rebuild.
#[derive(Clone, Debug)]
pub struct Run {
    /// The name of the example value set (`curator_profiles::VALUE_SETS`)
    /// the target `brain` reports as its profile.
    pub role: &'static str,
    /// `(param of ./policy, value)`.
    pub knobs: Vec<(&'static str, Value)>,
    pub trigger: Trigger,
}

impl Run {
    pub fn of(role: &'static str) -> Self {
        Self {
            role,
            knobs: Vec::new(),
            trigger: Trigger::Target,
        }
    }

    pub fn with(mut self, knob: &'static str, value: Value) -> Self {
        self.knobs.push((knob, value));
        self
    }

    /// The same run as a rebuild for an expired cache with the cold
    /// collection on (GH #1464): trigger `cache`, `gc` "1" and `gc_on_cold`
    /// "1" over whatever the run set -- a role whose preset is `gc` "0"
    /// collects too, its preset stays.
    pub fn cold(run: &Run) -> Run {
        let mut run = run
            .clone()
            .with("gc", json!("1"))
            .with("gc_on_cold", json!("1"));
        run.trigger = Trigger::Cache;
        run
    }

    /// The run in words, for the message of a statement.
    pub fn label(&self) -> String {
        let knobs: Vec<String> = self.knobs.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let way = match self.trigger {
            Trigger::Target => "",
            Trigger::Cache => " through the cache",
        };
        if knobs.is_empty() {
            format!("{}{way}", self.role)
        } else {
            format!("{} with {}{way}", self.role, knobs.join(", "))
        }
    }
}

/// A wall of the bank: its counted rows, the instant its rebuild plans at,
/// and the runs it is made for.
#[derive(Clone)]
pub struct Wall {
    pub name: &'static str,
    /// The instant of the turn that orders the rebuild.
    pub now: DateTime<Utc>,
    pub pieces: Vec<Piece>,
    pub runs: Vec<Run>,
    /// `(cell, param, value)` every run of the wall sets -- the target the
    /// wall is built to stand over among them.
    pub knobs: Vec<(&'static str, &'static str, Value)>,
    /// The session the two turns of the run are said in.
    pub session: String,
    /// What the stub answers every request of the summarizer with.
    pub summary: String,
    /// What a wall sows beside its rows (marks, notes, pins), run after the
    /// rows and before the first turn.
    pub prepare: fn(&mut Hive),
    /// What `prepare` sows that the window shows and that is no row of the
    /// wall (a note): counted like a row, known by its keyword alone.
    /// `What::Junk` is gone from the window after the rebuild, `What::Open`
    /// stands in it.
    pub beside: Vec<(What, String)>,
    /// Why every run of the wall has the same role ("" for a wall that runs
    /// as more than one): the bank runs every wall as each role it fits, and
    /// a wall that fits one alone says so here (`Wall::one_role`).
    pub one_role: &'static str,
}

fn nothing(_: &mut Hive) {}

impl Wall {
    /// The pieces counted as `what`, with their place in the wall.
    pub fn counted(&self, pick: impl Fn(&What) -> bool) -> Vec<usize> {
        (0..self.pieces.len())
            .filter(|i| pick(&self.pieces[*i].what))
            .collect()
    }

    /// The classes the wall holds garbage of, each once, in wall order.
    pub fn classes(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        for p in &self.pieces {
            if let What::Junk(c) = p.what
                && !out.contains(&c)
            {
                out.push(c);
            }
        }
        out
    }
}

/// Sows a wall row by row. `at` sets the instant of the next row, every row
/// moves it on by a second; `{k}` in a text is the keyword of the row.
pub struct WallBuilder {
    name: &'static str,
    word: &'static str,
    cursor: DateTime<Utc>,
    pieces: Vec<Piece>,
    tools: u64,
}

impl WallBuilder {
    /// A wall `name` whose keywords are `<word>-<nnn>`: an invented word no
    /// other wall uses and no text holds by chance.
    pub fn new(name: &'static str, word: &'static str) -> Self {
        Self {
            name,
            word,
            cursor: day(0, 0, 0),
            pieces: Vec::new(),
            tools: 0,
        }
    }

    /// The next row is written at `at`. The first row of a wall may lie on
    /// a day before the bank's day (a wall over several days).
    pub fn at(&mut self, at: DateTime<Utc>) -> &mut Self {
        assert!(
            self.pieces.is_empty() || at >= self.cursor,
            "{}: a wall is sown oldest first ({at} after {})",
            self.name,
            self.cursor
        );
        self.cursor = at;
        self
    }

    /// The instant the next row is written at.
    pub fn cursor(&self) -> DateTime<Utc> {
        self.cursor
    }

    /// One row: the element is made from the row's keyword. Returns the
    /// keyword.
    pub fn push(
        &mut self,
        what: What,
        session: &str,
        turn: &str,
        kind: &str,
        final_: i64,
        el: impl FnOnce(&str) -> Value,
    ) -> String {
        let key = format!("{}-{:03}", self.word, self.pieces.len() + 1);
        let el = el(&key);
        let text = el["text"].as_str().unwrap_or("");
        // A failed result stands as its tool wrote it, to the byte: nothing
        // is sewn into it, its call carries the keyword of the round.
        assert!(
            what == What::Error || text.contains(&key),
            "{}: the row {key} does not carry its keyword",
            self.name
        );
        if matches!(what, What::Junk(_)) {
            let head: String = text
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("")
                .chars()
                .take(FORM_HEAD)
                .collect();
            assert!(
                !head.contains(&key),
                "{}: the keyword of the garbage {key} stands in the head a form keeps",
                self.name
            );
        }
        self.pieces.push(Piece {
            what,
            key: key.clone(),
            at: self.cursor,
            session: session.to_string(),
            turn: turn.to_string(),
            kind: kind.to_string(),
            el,
            final_,
            lane: false,
            code: String::new(),
            errand: None,
        });
        self.cursor += Duration::seconds(1);
        key
    }

    /// One tool round that reaches the ledger the way a live one does:
    /// through the lanes of the hive, at the instant of its first row. The
    /// person's words arrive on `in_curate`, the call on the tap, the result
    /// on the `in_curate` of the next iteration -- with the `error_code` of
    /// its header, when `code` names one, in the slot `tool_errors` the
    /// collector fills (GH #1279) --, the answer on the tap again. Nothing
    /// of it is written into the ledger by the bank, so whatever `./intake`
    /// makes of the round is what the rebuild finds. The result is counted
    /// as `what`; returns its keyword.
    #[allow(clippy::too_many_arguments)]
    pub fn lane_round(
        &mut self,
        what: What,
        session: &str,
        turn: &str,
        words: &str,
        name: &str,
        arguments: Value,
        code: &str,
        result: impl FnOnce(&str) -> String,
        answer: &str,
    ) -> String {
        let from = self.pieces.len();
        self.words(session, turn, words);
        let key = self.tool(what, session, turn, name, arguments, None, result);
        self.pieces[from + 2].code = code.to_string();
        self.answer(session, turn, answer);
        for p in &mut self.pieces[from..] {
            p.lane = true;
        }
        key
    }

    /// One errand that ends without an answer, the way a live one reaches
    /// the hive (GH #1469): its `(delegation)` row arrives on `in_curate`
    /// under the turn `inner` (`<turn>~<deadline_ms>~<hex8>`) with a
    /// `delegation_id` beside it, at the instant of the row. The curator
    /// parks the errand itself; then the call of the round comes back as an
    /// error (`Ending::Failed`), or nothing comes back (`Ending::Silent`).
    /// The row is counted as scaffold: its errand is over when the rebuild
    /// plans. Returns its keyword.
    pub fn errand_round(
        &mut self,
        session: &str,
        inner: &str,
        ending: Ending,
        el: impl FnOnce(&str) -> Value,
    ) -> String {
        let key = self.push(What::Junk("scaffold"), session, inner, "delegation", 0, el);
        let p = self.pieces.last_mut().expect("the row just pushed");
        p.lane = true;
        p.errand = Some(ending);
        key
    }

    /// The person's words; `{k}` is the keyword.
    pub fn words(&mut self, session: &str, turn: &str, text: &str) -> String {
        self.push(What::Words, session, turn, "user", 0, |k| {
            user(&keyed(text, k))
        })
    }

    /// The answer that ends a round.
    pub fn answer(&mut self, session: &str, turn: &str, text: &str) -> String {
        self.push(What::Answer, session, turn, "assistant", 1, |k| {
            said(&keyed(text, k))
        })
    }

    /// A sentence the companion says before the answer of the round: a
    /// bridge over an errand, a sentence said ahead.
    pub fn interim(&mut self, session: &str, turn: &str, text: &str) -> String {
        self.push(What::Answer, session, turn, "assistant", 0, |k| {
            said(&keyed(text, k))
        })
    }

    /// A tool round the way `./intake` writes it: the call (`What::Other`)
    /// and its result, counted as `what`. `result` makes the result's text
    /// from the keyword; `valid_until` stamps it transient. Returns the
    /// keyword of the result.
    #[allow(clippy::too_many_arguments)]
    pub fn tool(
        &mut self,
        what: What,
        session: &str,
        turn: &str,
        name: &str,
        arguments: Value,
        valid_until: Option<DateTime<Utc>>,
        result: impl FnOnce(&str) -> String,
    ) -> String {
        self.tools += 1;
        let id = format!("call-{}-{:04}", self.word, self.tools);
        self.pair(
            what,
            session,
            turn,
            "tool_call",
            "tool_result",
            &id,
            name,
            arguments,
            valid_until,
            result,
        )
    }

    /// The recall the curator pushes in front of a turn: an ambient pair
    /// (`call_recall_<16 hex>`, ledger kind `recall`) whose result is the
    /// memory's own rendering. Returns the keyword of the result.
    pub fn recall(
        &mut self,
        what: What,
        session: &str,
        turn: &str,
        query: &str,
        result: impl FnOnce(&str) -> String,
    ) -> String {
        self.tools += 1;
        let id = format!("call_recall_{:016x}", self.tools);
        self.pair(
            what,
            session,
            turn,
            "recall",
            "recall",
            &id,
            "memory_recall",
            json!({"query": query}),
            None,
            result,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn pair(
        &mut self,
        what: What,
        session: &str,
        turn: &str,
        call_kind: &str,
        result_kind: &str,
        id: &str,
        name: &str,
        arguments: Value,
        valid_until: Option<DateTime<Utc>>,
        result: impl FnOnce(&str) -> String,
    ) -> String {
        self.push(What::Other, session, turn, call_kind, 0, |k| {
            // The keyword of a call rides in its arguments: the text of a
            // call is the function call a provider parses.
            let mut args = arguments.clone();
            args["ref"] = json!(k);
            json!({"origin": "assistant", "type": "tool_call", "id": id,
                   "text": json!({"name": name, "arguments": args.to_string()}).to_string()})
        });
        self.push(what, session, turn, result_kind, 0, |k| {
            let mut el = tool_result(id, &result(k));
            if let Some(until) = valid_until {
                el["valid_until"] = json!(rfc(until));
            }
            el
        })
    }

    /// The wall, to be run at `now` with the two turns of a run said in
    /// `session`.
    pub fn build(self, now: DateTime<Utc>, session: &str) -> Wall {
        assert!(
            self.cursor <= now,
            "{}: the wall ends after its own rebuild",
            self.name
        );
        Wall {
            name: self.name,
            now,
            pieces: self.pieces,
            runs: Vec::new(),
            knobs: Vec::new(),
            session: session.to_string(),
            summary: format!(
                "What came before, condensed by the stub of the bank ({}).",
                self.name
            ),
            prepare: nothing,
            beside: Vec::new(),
            one_role: "",
        }
    }
}

impl Wall {
    /// One more run of the wall.
    pub fn run(mut self, run: Run) -> Self {
        self.runs.push(run);
        self
    }

    /// A knob every run of the wall sets.
    pub fn knob(mut self, cell: &'static str, param: &'static str, value: Value) -> Self {
        self.knobs.push((cell, param, value));
        self
    }

    /// The target the wall is built to stand over, in tokens
    /// (`window_target` of `./policy`): every run rebuilds at it.
    pub fn target(self, tokens: i64) -> Self {
        self.knob("policy", "window_target", json!(tokens))
    }

    /// What the wall sows beside its rows.
    pub fn prepare(mut self, sow: fn(&mut Hive)) -> Self {
        self.prepare = sow;
        self
    }

    /// One counted piece `prepare` sows beside the rows, known by `key`.
    pub fn beside(mut self, what: What, key: &str) -> Self {
        self.beside.push((what, key.to_string()));
        self
    }

    /// The wall is made for one role alone, and `why`: every one of its runs
    /// has that role, its other runs differ by a knob.
    pub fn one_role(mut self, why: &'static str) -> Self {
        assert!(
            !why.trim().is_empty(),
            "{}: a wall of one role says why",
            self.name
        );
        self.one_role = why;
        self
    }

    /// The classes the wall holds garbage of beside its rows, each once.
    pub fn beside_classes(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        for (what, _) in &self.beside {
            if let What::Junk(c) = what
                && !out.contains(c)
            {
                out.push(*c);
            }
        }
        out
    }
}

fn keyed(text: &str, key: &str) -> String {
    assert!(text.contains("{k}"), "a text names its keyword: {text}");
    text.replace("{k}", key)
}

/// English filler of `sentences` sentences, the same for the same `seed`:
/// the bulk of a result nobody reads twice.
pub fn filler(seed: u64, sentences: usize) -> String {
    const SUBJECTS: [&str; 8] = [
        "The ferry",
        "A night bus",
        "The harbour office",
        "The upper market",
        "A late train",
        "The tide table",
        "The lighthouse keeper",
        "The town archive",
    ];
    const VERBS: [&str; 8] = [
        "lists",
        "moves",
        "closes",
        "repeats",
        "delays",
        "confirms",
        "posts",
        "withdraws",
    ];
    const OBJECTS: [&str; 8] = [
        "the winter timetable",
        "a notice for the outer quay",
        "its opening hours",
        "the fares of last season",
        "two crossings before noon",
        "the route over the old bridge",
        "a correction of the earlier figure",
        "the list of closed piers",
    ];
    let mut x = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    let mut out = Vec::new();
    for _ in 0..sentences {
        x = x
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let (a, b, c) = (
            (x >> 33) as usize % 8,
            (x >> 41) as usize % 8,
            (x >> 49) as usize % 8,
        );
        out.push(format!("{} {} {}.", SUBJECTS[a], VERBS[b], OBJECTS[c]));
    }
    out.join(" ")
}

/// The one target of the bank's hive: the model cell every curate names
/// (`hop.target`) and the one that reports the run's profile.
pub const TARGET: &str = "brain";

/// The knobs of a run as the policy itself reports them: the profile of the
/// target under the knobs of the instance (`profile_params`).
#[derive(Clone, Debug)]
pub struct Knobs {
    /// The name of the run's example value set.
    pub role: String,
    pub gc: bool,
    pub gc_scaffold: bool,
    pub keep_rounds: i64,
    pub window_target: i64,
    /// Every knob of the shipped set (`SHIPPED`), as it applies to the run.
    pub all: Value,
    /// The example value sets (`curator_profiles::VALUE_SETS`).
    pub roles: Vec<String>,
}

/// The knobs of `./policy` the wall and the run set on the instance.
fn instance_knobs(wall: &Wall, run: &Run) -> Value {
    let mut over = json!({});
    for (cell, k, v) in &wall.knobs {
        if *cell == "policy" {
            over[*k] = v.clone();
        }
    }
    for (k, v) in &run.knobs {
        over[*k] = v.clone();
    }
    over
}

/// The profile the target reports in a run: the run's example value set
/// (`curator_profiles::profile`, holding its tree), and over it every
/// window value the wall or the run sets. A value of the profile wins over
/// the instance's knob (contract point 3), so a window value a run sets
/// beside its set moves into the profile, as a composite moves the window
/// values it set on the curator into the profile of its main model; the
/// window stays the one the run cut while the set was a role.
pub fn profile_of(wall: &Wall, run: &Run) -> Value {
    let mut profile = curator_profiles::profile(run.role);
    let set = curator_profiles::value_set(run.role);
    for (k, v) in instance_knobs(wall, run)
        .as_object()
        .cloned()
        .unwrap_or_default()
    {
        if set.get(&k).is_some() {
            profile[k] = v;
        }
    }
    profile
}

/// The knobs of `run` on `wall`, read off the pure half of the policy: the
/// profile the target reports, under the knobs of the instance.
pub fn knobs_of(wall: &Wall, run: &Run) -> Knobs {
    let (got, err) = policy_scope(
        instance_knobs(wall, run),
        "(lambda p: [switch_on(knob_of(p, 'gc')), switch_on(knob_of(p, 'gc_scaffold')), \
         int(number_of(p, 'keep_rounds')), max(0, int(number_of(p, 'window_target'))), \
         dict((k, knob_of(p, k)) for k in SHIPPED)])(profile_params(ARGS, True))",
        profile_of(wall, run),
    );
    assert!(!err.contains("Traceback"), "{}: {err}", wall.name);
    Knobs {
        role: run.role.to_string(),
        gc: got[0].as_bool().expect("gc"),
        gc_scaffold: got[1].as_bool().expect("gc_scaffold"),
        keep_rounds: got[2].as_i64().expect("keep_rounds"),
        window_target: got[3].as_i64().expect("window_target"),
        all: got[4].clone(),
        roles: VALUE_SETS.iter().map(|r| r.to_string()).collect(),
    }
}

/// Whether a class collects under the knobs of a run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Collects {
    /// At every rebuild: each counted piece is out, the mark names the count.
    Always,
    /// Not under these knobs: the mark names none.
    Never,
    /// Only when the plan is over its aim: the mark names what the window
    /// shows shortened, the count of the wall is no promise.
    OverAim,
}

/// The rule of a class: what the knobs of a run make of it.
pub type Rule = fn(&Knobs) -> Collects;

fn always(_: &Knobs) -> Collects {
    Collects::Always
}

fn with_gc(k: &Knobs) -> Collects {
    if k.gc {
        Collects::Always
    } else {
        Collects::Never
    }
}

fn scaffold_rule(k: &Knobs) -> Collects {
    if k.gc && k.gc_scaffold {
        Collects::Always
    } else {
        Collects::Never
    }
}

fn tools_rule(k: &Knobs) -> Collects {
    if k.keep_rounds <= 0 {
        Collects::Never
    } else if k.gc {
        Collects::Always
    } else {
        Collects::OverAim
    }
}

/// The one-line form a window shows a block in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Form {
    Shortened,
    Expired,
    Released,
}

/// The classes that leave a block in each form: the marks of the classes of
/// a form add up to the blocks the window shows in it.
pub const FORM_CLASSES: [(Form, &[&str]); 3] = [
    (Form::Shortened, &["shrunk_tools", "dup_recall", "scaffold"]),
    (Form::Expired, &["transient"]),
    (Form::Released, &["released"]),
];

/// How the window of the next call shows a piece.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    /// Its text, whole (behind its id where the window names ids, and behind
    /// the frame of an ambient pair).
    Whole,
    /// One of the one-line forms.
    Form(Form),
    /// Under its id, but neither whole nor a form the bank knows.
    Other(String),
    /// Not in the window.
    Absent,
}

/// The registry of the bank: the walls, the rule of every class, and the
/// seats that registered nothing.
#[derive(Default)]
pub struct Bank {
    pub walls: Vec<Wall>,
    pub rules: Vec<(&'static str, Rule)>,
    pub pending: Vec<&'static str>,
    /// The classes no rebuild collects: the call that shows their block
    /// clears them, every time (`Bank::class_of_the_call`).
    pub of_the_call: Vec<&'static str>,
}

impl Bank {
    /// A wall takes its place. Every piece of garbage it counts needs the
    /// rule of its class (`Bank::class`) by the time a statement reads it.
    pub fn wall(&mut self, wall: Wall) {
        assert!(
            !wall.runs.is_empty(),
            "{}: a wall names the runs it is made for",
            wall.name
        );
        assert!(
            self.walls.iter().all(|w| w.name != wall.name),
            "{}: registered twice",
            wall.name
        );
        self.walls.push(wall);
    }

    /// The rule of a class the policy collects that the bank does not know
    /// yet. The five classes of the `gc` mark are known.
    pub fn class(&mut self, class: &'static str, rule: Rule) {
        if self.rules.iter().all(|(c, _)| *c != class) {
            self.rules.push((class, rule));
        }
    }

    /// The rule of a class the rebuild does not collect: the call clears it
    /// itself, in the block it builds at the end of the window, at every
    /// call. Statement 1 holds for it as for any class (the piece is gone);
    /// statement 5 does not: the `gc` mark is the count of what the rebuild
    /// collected, and a mark per call would be noise.
    pub fn class_of_the_call(&mut self, class: &'static str, rule: Rule) {
        self.class(class, rule);
        if !self.of_the_call.contains(&class) {
            self.of_the_call.push(class);
        }
    }

    /// Whether `class` is one the rebuild collects (statement 5 reads its
    /// mark) -- or one every call clears by itself.
    pub fn rebuild_collects(&self, class: &str) -> bool {
        !self.of_the_call.contains(&class)
    }

    /// What the knobs of a run make of `class`.
    pub fn collects(&self, class: &str, knobs: &Knobs) -> Collects {
        let rule = self
            .rules
            .iter()
            .find(|(c, _)| *c == class)
            .unwrap_or_else(|| panic!("no rule for the class {class}: Bank::class names it"));
        (rule.1)(knobs)
    }

    /// The wall `name`.
    pub fn named(&self, name: &str) -> &Wall {
        self.walls
            .iter()
            .find(|w| w.name == name)
            .unwrap_or_else(|| panic!("no wall {name} in the bank"))
    }
}

/// What a run left behind.
#[derive(Clone)]
pub struct Outcome {
    pub wall: &'static str,
    pub run: String,
    /// What ordered the rebuild of the run.
    pub trigger: Trigger,
    /// The short ids the model pinned (`pin` marks, the `#` left off): a
    /// pin keeps a result whole whatever collects it.
    pub pinned: Vec<String>,
    pub knobs: Knobs,
    /// The aim the hive named for the rebuild, in tokens.
    pub aim: i64,
    /// The plan the rebuild wrote for the round.
    pub plan: Value,
    /// The `seq` of every piece, in wall order.
    pub seqs: Vec<i64>,
    /// The values of the `gc` marks the run wrote, oldest first.
    pub gc: Vec<Value>,
    /// The values of the `over_aim` marks the run wrote.
    pub over_aim: Vec<Value>,
    /// The requests the summarizer was sent, in order.
    pub requests: Vec<Msg>,
    /// The call of the turn after the rebuild: the window it left.
    pub next: Msg,
    /// The plan snapshot the ledger keeps for that call, when it keeps one.
    pub snapshot: Option<Vec<Value>>,
    /// The stamp of the turn that ordered the rebuild, and of the one after.
    pub before: Value,
    pub after: Value,
    /// The words of the two turns of the run: person, companion, person.
    pub live: [String; 3],
    pub stderr: Vec<String>,
}

fn stamp(h: &Hive) -> Value {
    sj::from_str(&h.state_in("curate", ROUND_E)).unwrap_or(Value::Null)
}

fn marks(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("null")).unwrap_or(Value::Null))
    .collect()
}

/// One wall through one rebuild with the trigger `target`: the rows sown,
/// a turn that finds the window at its target, the order of that turn
/// fired, every request of the summarizer answered by the stub of the wall,
/// and one more turn, whose call is the window the rebuild left.
pub fn rebuild(wall: &Wall, run: &Run) -> Outcome {
    run_wall(wall, run).1.unwrap_or_else(|why| panic!("{why}"))
}

/// The figures the watch reads, counted over one run of a wall: the wall is
/// run as `run` (`rebuild`, the same two turns), and the hive's own `stats`
/// door is asked for `window`, `rebuild` and `gc` over the day up to the
/// run. Returns the `counts` of that answer. A run whose turn orders no
/// rebuild is counted too: its two turns stand in the ledger all the same.
pub fn figures(wall: &Wall, run: &Run) -> Value {
    let (mut h, _) = run_wall(wall, run);
    h.out.clear();
    h.lane(
        "in_stats",
        json!({}),
        json!({"stats_tag": "wait:bank",
               "stats_from": rfc(wall.now - Duration::days(1)),
               "stats_to": rfc(wall.now + Duration::minutes(1)),
               "stats_kinds": json!(["window", "rebuild", "gc"]).to_string()}),
        json!({"messages": []}),
    );
    let who = format!("{} as {}", wall.name, run.label());
    assert_eq!(h.out.len(), 1, "{who}: one answer of stats: {:?}", h.out);
    let m = &h.out[0];
    assert_eq!(
        m.hop["error_code"], "",
        "{who}: {:?} {:?}",
        m.body, h.stderr
    );
    m.body["stats"]["counts"].clone()
}

/// The figures of `figures`, counted over a run of the wall whose rebuild the
/// day boundary orders instead of the target: the rows sown, one turn at the
/// instant of the wall, the test clock moved on by a day, one more turn --
/// it finds itself on another day than the round's last and orders the
/// rebuild --, the order fired and every request of the summarizer answered.
/// The run sets its own `window_target` far above the wall, so the plan of
/// the rebuild is not over its aim. Returns None when the second turn orders
/// no rebuild of the day (the role of the run has no day horizon); the role
/// is the run's own. Beside the figures stand the `gc` mark of the rebuild
/// (`gc_mark`) and how many pieces the wall counts as garbage lie in the raw
/// part of its plan (`raw_garbage`).
pub fn figures_at_day(wall: &Wall, run: &Run) -> Option<Value> {
    let who = format!("{} as {} at the day boundary", wall.name, run.label());
    let (mut h, seqs) = sow(wall, run);
    let package = json!({"input_soft": INPUT_SOFT, "input_hard": INPUT_HARD});
    let first = curate_target(
        &mut h,
        &wall.session,
        "bank-day-1",
        0,
        json!([user(&format!("Good night for today, {}-day-1.", wall.name))]),
        mode("Be brief."),
    );
    h.tap(
        &first,
        "stop",
        package.clone(),
        json!([said(&format!("Good night, {}-day-2.", wall.name))]),
    );
    h.set_clock((wall.now + Duration::days(1)).timestamp_micros());
    let second = curate_target(
        &mut h,
        &wall.session,
        "bank-day-2",
        0,
        json!([user(&format!("Good morning, {}-day-3.", wall.name))]),
        mode("Be brief."),
    );
    h.tap(
        &second,
        "stop",
        package,
        json!([said(&format!("Good morning, {}-day-4.", wall.name))]),
    );
    let at = stamp(&h);
    if at["curate_mark"] != "day" {
        return None;
    }
    let order = h
        .clock_order_for(ROUND_E)
        .unwrap_or_else(|| panic!("{who}: the day orders a rebuild: {:?}", h.stderr));
    assert_eq!(order.body["emit_body"]["reason"], "day", "{who}");
    h.fire(&order);
    condense(&mut h, wall);
    let over = marks(&h, "over_aim");
    assert!(
        over.is_empty(),
        "{who}: the rebuild of the day is not over its aim: {over:?}"
    );
    let gc = marks(&h, "gc");
    assert_eq!(gc.len(), 1, "{who}: one rebuild, one gc mark: {gc:?}");
    h.out.clear();
    h.lane(
        "in_stats",
        json!({}),
        json!({"stats_tag": "wait:bank",
               "stats_from": rfc(wall.now - Duration::days(1)),
               "stats_to": rfc(wall.now + Duration::days(1) + Duration::minutes(1)),
               "stats_kinds": json!(["window", "rebuild", "gc"]).to_string()}),
        json!({"messages": []}),
    );
    assert_eq!(h.out.len(), 1, "{who}: one answer of stats: {:?}", h.out);
    let m = &h.out[0];
    assert_eq!(
        m.hop["error_code"], "",
        "{who}: {:?} {:?}",
        m.body, h.stderr
    );
    let mut counts = m.body["stats"]["counts"].clone();
    counts["gc_mark"] = gc[0].clone();
    let plan = h.plan_in(ROUND_E);
    let cover = plan["cover"].as_i64().unwrap_or(0);
    let keep = plan["keep"].as_array().cloned().unwrap_or_default();
    let raw = wall
        .counted(|w| matches!(w, What::Junk(_)))
        .into_iter()
        .filter(|i| seqs[*i] > cover || keep.contains(&json!(seqs[*i])))
        .count();
    counts["raw_garbage"] = json!(raw);
    Some(counts)
}

/// The run behind `rebuild` and `figures`: the hive it ran in, and the
/// outcome of the rebuild -- or, when the turn ordered none, why not.
fn run_wall(wall: &Wall, run: &Run) -> (Hive, Result<Outcome, String>) {
    let knobs = knobs_of(wall, run);
    let who = format!("{} as {}", wall.name, run.label());
    let (h, seqs) = sow(wall, run);
    run_sown(wall, run, knobs, who, h, seqs)
}

/// The hive of a run with the rows of the wall in its ledger, and the `seq`
/// of every piece in wall order: the knobs of the wall and of the run set,
/// the target's package and profile reported (`profile_of`), the rows
/// written oldest first, what the wall sows beside them sown, the clock at
/// the instant of the wall.
fn sow(wall: &Wall, run: &Run) -> (Hive, Vec<i64>) {
    let who = format!("{} as {}", wall.name, run.label());
    // The policy's `now_utc` went with the wellbeing line it dated (GH #1479).
    let mut over: Vec<(&str, &str, Value)> = Vec::new();
    for (cell, k, v) in &wall.knobs {
        over.push((*cell, *k, v.clone()));
    }
    for (k, v) in &run.knobs {
        over.push(("policy", *k, v.clone()));
    }
    let mut h = Hive::with_prelude(&over, CLOCK);
    h.set_clock(wall.now.timestamp_micros());
    // The target reports its package and its profile, as a model cell with
    // `package_line` does after its birth.
    let (hop, body) = curator_profiles::package_with(TARGET, profile_of(wall, run));
    h.lane("in_catalog", json!({}), hop, body);
    h.out.clear();
    // The rows oldest first: written into the ledger, or -- the rows of a
    // lane round, one turn at a time -- through the lanes at their instant.
    let mut seqs = Vec::new();
    let mut i = 0;
    while i < wall.pieces.len() {
        let p = &wall.pieces[i];
        if !p.lane {
            let (seq, _) = h.row(p.at, &p.session, &p.turn, &p.kind, &p.el, p.final_);
            seqs.push(seq);
            i += 1;
            continue;
        }
        let mut j = i;
        while j < wall.pieces.len()
            && wall.pieces[j].lane
            && wall.pieces[j].session == p.session
            && wall.pieces[j].turn == p.turn
        {
            j += 1;
        }
        seqs.extend(through_lanes(&mut h, &who, &wall.pieces[i..j]));
        i = j;
    }
    h.set_clock(wall.now.timestamp_micros());
    (wall.prepare)(&mut h);
    (h, seqs)
}

/// The day the oldest pending summarizer request shows, None for a
/// transcript without a dated row line (the prose form).
fn summary_day(h: &Hive) -> Option<String> {
    h.summ
        .front()
        .and_then(|r| r.messages().last().cloned())
        .and_then(|m| m["text"].as_str().and_then(transcript_day_of))
}

/// Every request of the summarizer a rebuild left open, answered by the stub
/// of the wall: on the way of the summarizer, and -- a run that has its own
/// model condense (`summarizer_self`) asks as a model call instead (D58
/// target: route `model_call`, `hop.target` the target holding the tree) -- home
/// on `in_summary_self`. Returns the requests, in order.
fn condense(h: &mut Hive, wall: &Wall) -> Vec<Msg> {
    let mut requests = Vec::new();
    // GH #1496: a stub brief names its day as `{day}` and is dated with a day
    // its transcript shows -- the policy drops a line dated with another.
    let dated = |summary: &str, transcript: Option<String>| match transcript {
        Some(day) if summary.contains("{day}") => summary.replace("{day}", &day),
        _ => summary.to_string(),
    };
    while !h.summ.is_empty() {
        let day = summary_day(h);
        requests.push(h.answer(&dated(&wall.summary, day), "stop"));
    }
    let own: Vec<Msg> = h
        .routed("model_call")
        .into_iter()
        .filter(|m| m.hop.get("summarize") == Some(&json!("1")))
        .collect();
    for call in own {
        h.lane(
            "in_summary_self",
            json!({}),
            json!({"finish_reason": "stop", "model": "role-model", "summarize": "1",
                   "summary_call": call.hop.get("summary_call").cloned().unwrap_or_default(),
                   "summary_token": call.hop.get("summary_token").cloned().unwrap_or_default()}),
            json!({"messages": [said(&dated(&wall.summary, call
                .messages()
                .last()
                .and_then(|m| m["text"].as_str())
                .and_then(transcript_day_of)))]}),
        );
        requests.push(call);
    }
    requests
}

/// The two turns of `run_wall` on a sown hive.
fn run_sown(
    wall: &Wall,
    run: &Run,
    knobs: Knobs,
    who: String,
    mut h: Hive,
    seqs: Vec<i64>,
) -> (Hive, Result<Outcome, String>) {
    let live = [
        format!("And what is still open for today, {}-live-1?", wall.name),
        format!("Nothing that cannot wait, {}-live-2.", wall.name),
        format!(
            "Good. Then tell me again where we stand, {}-live-3.",
            wall.name
        ),
    ];
    let package = json!({"input_soft": INPUT_SOFT, "input_hard": INPUT_HARD});
    let call = curate_target(
        &mut h,
        &wall.session,
        "bank-live-1",
        0,
        json!([user(&live[0])]),
        mode("Be brief."),
    );
    let mut measured = package.clone();
    let (mark, reason) = match run.trigger {
        Trigger::Target => {
            measured["tokens_prompt"] = json!(MEASURED);
            ("target", "target")
        }
        Trigger::Cache => {
            measured["tokens_prompt"] = json!(MEASURED_WARM);
            measured["cache_expires_at"] = json!(rfc(wall.now + Duration::seconds(CACHE_TTL_S)));
            ("none", "cache")
        }
    };
    h.tap(&call, "stop", measured, json!([said(&live[1])]));
    let before = stamp(&h);
    if before["curate_mark"] != mark {
        let why = format!(
            "{who}: the wall stands over its target ({}): {before} {:?}",
            knobs.window_target, h.stderr
        );
        let next = curate_target(
            &mut h,
            &wall.session,
            "bank-live-2",
            0,
            json!([user(&live[2])]),
            mode("Be brief."),
        );
        h.tap(
            &next,
            "stop",
            package,
            json!([said("Here is where we stand.")]),
        );
        return (h, Err(why));
    }
    let order = h
        .clock_order_for(ROUND_E)
        .unwrap_or_else(|| panic!("{who}: the {reason} orders a rebuild: {:?}", h.stderr));
    assert_eq!(order.body["emit_body"]["reason"], reason, "{who}");
    let aim: i64 = h
        .state_in("aim", ROUND_E)
        .parse()
        .unwrap_or_else(|_| panic!("{who}: the hive names the aim of the rebuild"));
    h.fire(&order);
    let requests = condense(&mut h, wall);
    // The way through the cache is the collection, not a rebuild with the
    // summarizer (review coldgc M4): a wall that took the other branch would
    // have its statements `through the cache` read a summary instead.
    assert!(
        run.trigger != Trigger::Cache || requests.is_empty(),
        "{who}: the strike of the cache collects and asks the summarizer nothing: {} requests",
        requests.len()
    );
    let plan = h.plan_in(ROUND_E);
    // A cold collection that finds nothing writes no plan (GH #1464): the
    // window stays the one of before, and the statements read it so.
    assert!(
        plan["cover"].as_i64().is_some() || run.trigger == Trigger::Cache,
        "{who}: the rebuild wrote a plan: {plan} {:?}",
        h.stderr
    );

    let next = curate_target(
        &mut h,
        &wall.session,
        "bank-live-2",
        0,
        json!([user(&live[2])]),
        mode("Be brief."),
    );
    let snapshot = snapshot_of(&h, &next);
    h.tap(
        &next,
        "stop",
        package,
        json!([said("Here is where we stand.")]),
    );
    let after = stamp(&h);
    let outcome = Outcome {
        wall: wall.name,
        run: run.label(),
        trigger: run.trigger,
        pinned: h
            .rows("SELECT value FROM marks WHERE kind = 'pin' ORDER BY seq")
            .into_iter()
            .map(|r| {
                r[0].as_str()
                    .unwrap_or("")
                    .trim_matches('"')
                    .trim_start_matches('#')
                    .to_string()
            })
            .collect(),
        knobs,
        aim,
        plan,
        seqs,
        gc: marks(&h, "gc"),
        over_aim: marks(&h, "over_aim"),
        requests,
        next,
        snapshot,
        before,
        after,
        live,
        stderr: h.stderr.clone(),
    };
    (h, Ok(outcome))
}

/// `Hive::curate` for the bank's target: one round on `in_curate` that
/// names `TARGET` in `hop.target`; returns the call that left for it.
fn curate_target(
    h: &mut Hive,
    session: &str,
    turn: &str,
    iter: u32,
    round: Value,
    system: Value,
) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": session, "turn_id": turn, "iter": iter.to_string(),
                     "phase": "", "target": TARGET});
    let ctx = json!({"session_id": session, "turn_id": turn, "iter": iter.to_string(),
                     "channel": "test", "audience_set": ROUND_E});
    let mut body = json!({"messages": round});
    if system.is_object() {
        body["system"] = system;
    }
    h.lane("in_curate", ctx, hop, body);
    let calls = h.routed(TARGET);
    assert_eq!(
        calls.len(),
        1,
        "one round, one call: {:?} {:?}",
        h.out,
        h.stderr
    );
    calls[0].clone()
}

/// One round on `in_curate` as the collector sends it: `round` and, when a
/// tool result of it came back with a header code, the slot `tool_errors`
/// (`{tool_call_id: error_code}`). Returns the call that left for the model.
fn curate_round(
    h: &mut Hive,
    who: &str,
    p: &Piece,
    iter: u32,
    round: Value,
    errors: Option<Value>,
) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": p.session, "turn_id": p.turn, "iter": iter.to_string(),
                     "phase": "", "target": TARGET});
    let ctx = json!({"session_id": p.session, "turn_id": p.turn, "iter": iter.to_string(),
                     "channel": "test", "audience_set": ROUND_E});
    let mut body = json!({"messages": round, "system": mode("Be brief.")});
    if let Some(errors) = errors {
        body["tool_errors"] = errors;
    }
    h.lane("in_curate", ctx, hop, body);
    let calls = h.routed(TARGET);
    assert_eq!(
        calls.len(),
        1,
        "{who}: the round of {} leaves as one call: {:?} {:?}",
        p.key,
        h.out,
        h.stderr
    );
    calls[0].clone()
}

/// The rows of one lane round (`WallBuilder::lane_round`) through the
/// lanes of the hive, at the instant of the first of them. Returns the
/// `seq` `./intake` gave each row; a row the ledger does not hold exactly
/// once, with the element the wall counts, stops the run.
fn through_lanes(h: &mut Hive, who: &str, pieces: &[Piece]) -> Vec<i64> {
    h.set_clock(pieces[0].at.timestamp_micros());
    let package = json!({"input_soft": INPUT_SOFT, "input_hard": INPUT_HARD});
    let mut iter = 0;
    let mut call: Option<Msg> = None;
    let mut asked: Option<&Piece> = None;
    for p in pieces {
        let open = |call: &Option<Msg>| -> Msg {
            call.clone()
                .unwrap_or_else(|| panic!("{who}: the row {} answers no call of its round", p.key))
        };
        if let Some(ending) = &p.errand {
            errand_through_lanes(h, who, p, ending);
            continue;
        }
        match (
            p.el["origin"].as_str().unwrap_or(""),
            p.el["type"].as_str().unwrap_or(""),
        ) {
            ("user", "text") => {
                call = Some(curate_round(h, who, p, iter, json!([p.el]), None));
            }
            ("assistant", "tool_call") => {
                h.tap(&open(&call), "tool_calls", package.clone(), json!([p.el]));
                asked = Some(p);
            }
            ("tool", "tool_result") => {
                let a = asked
                    .take()
                    .unwrap_or_else(|| panic!("{who}: the result {} has no call", p.key));
                iter += 1;
                let errors = (!p.code.is_empty()).then(|| {
                    let mut slot = sj::Map::new();
                    slot.insert(p.el["id"].as_str().unwrap_or("").to_string(), json!(p.code));
                    Value::Object(slot)
                });
                call = Some(curate_round(h, who, p, iter, json!([a.el, p.el]), errors));
            }
            ("assistant", "text") => {
                h.tap(&open(&call), "stop", package.clone(), json!([p.el]));
            }
            other => panic!(
                "{who}: the row {} is no row of a lane round: {other:?}",
                p.key
            ),
        }
    }
    pieces
        .iter()
        .map(|p| {
            let hash = sha256_hex(&canonical(&p.el));
            let got = h.rows(&format!(
                "SELECT seq FROM wall WHERE hash = '{hash}' AND turn_id = '{}' ORDER BY seq",
                p.turn
            ));
            assert!(
                got.len() == 1,
                "{who}: the ledger holds the row {} of the lane round once: {got:?}; the rows \
                 of the turn: {:?} {:?}",
                p.key,
                h.rows(&format!(
                    "SELECT w.seq, w.kind, w.iter, b.body FROM wall w JOIN blocks b \
                     ON b.hash = w.hash WHERE w.turn_id = '{}' ORDER BY w.seq",
                    p.turn
                )),
                h.stderr
            );
            got[0][0].as_i64().expect("a seq")
        })
        .collect()
}

/// The round of one errand (`WallBuilder::errand_round`) through the lanes:
/// the `(delegation)` row on `in_curate` with the `delegation_id` in hop and
/// context (`dg-<hex8>` of its turn), and -- an errand that fails -- the
/// call of the round back on the tap as an error with its code.
fn errand_through_lanes(h: &mut Hive, who: &str, p: &Piece, ending: &Ending) {
    let id = format!("dg-{}", p.turn.rsplit('~').next().unwrap_or(""));
    h.out.clear();
    let hop = json!({"session_id": p.session, "turn_id": p.turn, "iter": "0", "phase": "",
                     "delegation_id": id, "target": TARGET});
    let ctx = json!({"session_id": p.session, "turn_id": p.turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "delegation_id": id});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [p.el], "system": mode("Be brief.")}),
    );
    let calls = h.routed(TARGET);
    assert_eq!(
        calls.len(),
        1,
        "{who}: the round of the errand {} leaves as one call: {:?} {:?}",
        p.key,
        h.out,
        h.stderr
    );
    if let Ending::Failed(code) = ending {
        h.tap(
            &calls[0],
            "error",
            json!({"input_soft": INPUT_SOFT, "input_hard": INPUT_HARD, "error_code": code}),
            json!([]),
        );
    }
}

impl Outcome {
    /// `<wall> as <run>`, the head of every message.
    pub fn who(&self) -> String {
        format!("{} as {}", self.wall, self.run)
    }

    /// The cover of the plan: every row at or under it left the raw part.
    pub fn cover(&self) -> i64 {
        self.plan["cover"].as_i64().unwrap_or(0)
    }

    /// Whether the piece `i` lies in the raw part of the plan: above the
    /// cover, or kept raw under it.
    pub fn raw(&self, i: usize) -> bool {
        let keep = self.plan["keep"].as_array().cloned().unwrap_or_default();
        self.seqs[i] > self.cover() || keep.contains(&json!(self.seqs[i]))
    }

    /// The texts of the window the rebuild left.
    pub fn window(&self) -> Vec<String> {
        texts(&self.next)
    }

    /// The run at a glance, for the message of a failed statement: the two
    /// stamps, the plan without its blocks, the marks, and the window with
    /// the head of every message.
    pub fn view(&self) -> String {
        let mut plan = self.plan.clone();
        if let Some(p) = plan.as_object_mut() {
            p.remove("blocks");
            p.remove("marks_on");
        }
        let mut out = vec![
            format!("before: {}", self.before),
            format!("after: {}", self.after),
            format!("aim: {} plan: {plan}", self.aim),
            format!("gc: {:?} over_aim: {:?}", self.gc, self.over_aim),
            format!("summarizer requests: {}", self.requests.len()),
            format!("stderr: {:?}", self.stderr),
        ];
        for m in self.next.messages() {
            let t = m["text"].as_str().unwrap_or("");
            let head: String = t.chars().take(90).collect();
            out.push(format!(
                "  {}/{} {} chars: {head:?}",
                m["origin"].as_str().unwrap_or(""),
                m["type"].as_str().unwrap_or(""),
                t.chars().count()
            ));
        }
        out.join("\n")
    }

    /// How the window shows the piece `i` of `wall`.
    pub fn shown(&self, wall: &Wall, i: usize) -> Shown {
        let p = &wall.pieces[i];
        let (id, text) = (p.id(), p.text());
        let mut found = Shown::Absent;
        for m in self.next.messages() {
            let t = m["text"].as_str().unwrap_or("");
            let named = t.starts_with(&format!("[#{id}] ")) || t.starts_with(&format!("[#{id} "));
            // Whole: the text as it was written, or behind its id -- and in
            // front of the id whatever frame the window sets there (the
            // line that names the session of the person's words, the frame
            // of an ambient pair behind it).
            let behind_id = t
                .find(&format!("[#{id}] "))
                .is_some_and(|at| t[at..].ends_with(text));
            if m["type"] == p.el["type"]
                && m["origin"] == p.el["origin"]
                && (t == text || behind_id)
            {
                return Shown::Whole;
            }
            if !named {
                continue;
            }
            found = if t == format!("[#{id} expired]") {
                Shown::Form(Form::Expired)
            } else if t.starts_with(&format!("[#{id} released ")) {
                Shown::Form(Form::Released)
            } else if t.contains(" [shortened, ")
                && t.ends_with(&format!("history_read(\"#{id}\")]"))
            {
                Shown::Form(Form::Shortened)
            } else {
                Shown::Other(t.to_string())
            };
        }
        found
    }

    /// The pieces of `wall` the window shows in `form`.
    pub fn in_form(&self, wall: &Wall, form: Form) -> Vec<usize> {
        (0..wall.pieces.len())
            .filter(|i| self.shown(wall, *i) == Shown::Form(form))
            .collect()
    }

    /// The `gc` mark of the rebuild: the one mark the run wrote.
    pub fn gc_mark(&self) -> &Value {
        assert_eq!(
            self.gc.len(),
            1,
            "{}: one rebuild, one gc mark: {:?}",
            self.who(),
            self.gc
        );
        &self.gc[0]
    }
}

/// Statement 1: every piece counted as garbage of a class that collects
/// under the knobs of the run is gone from the window or stands there in a
/// one-line form, and its keyword with it. Returns how many pieces it
/// judged.
pub fn garbage_is_out(bank: &Bank, wall: &Wall, o: &Outcome) -> usize {
    let window = o.window();
    let mut judged = 0;
    for i in wall.counted(|w| matches!(w, What::Junk(_))) {
        let p = &wall.pieces[i];
        let What::Junk(class) = p.what else { continue };
        if bank.collects(class, &o.knobs) != Collects::Always {
            continue;
        }
        let shown = o.shown(wall, i);
        assert!(
            matches!(shown, Shown::Form(_) | Shown::Absent),
            "{}: the garbage {} of class {class} is still in the window: {shown:?}",
            o.who(),
            p.key
        );
        assert!(
            window.iter().all(|t| !t.contains(&p.key)),
            "{}: the keyword of the garbage {} of class {class} is still in the window",
            o.who(),
            p.key
        );
        judged += 1;
    }
    for (what, key) in &wall.beside {
        let What::Junk(class) = what else { continue };
        if bank.collects(class, &o.knobs) != Collects::Always {
            continue;
        }
        assert!(
            window.iter().all(|t| !t.contains(key.as_str())),
            "{}: the garbage {key} of class {class}, sown beside the rows, is still in the window",
            o.who()
        );
        judged += 1;
    }
    judged
}

/// Statement 1 for what a wall sows beside its rows and counts as open: its
/// keyword stands in the window of the next call. Returns how many it
/// judged.
pub fn beside_stands(wall: &Wall, o: &Outcome) -> usize {
    let window = o.window();
    let mut judged = 0;
    for (what, key) in &wall.beside {
        if !matches!(what, What::Open(_)) {
            continue;
        }
        assert!(
            window.iter().any(|t| t.contains(key.as_str())),
            "{}: the open piece {key}, sown beside the rows, is not in the window",
            o.who()
        );
        judged += 1;
    }
    judged
}

/// The other half of statement 1: what a class would collect once its
/// errand is over stays whole while the errand is open. Returns how many
/// pieces it judged.
pub fn open_errands_stand(wall: &Wall, o: &Outcome) -> usize {
    let open = wall.counted(|w| matches!(w, What::Open(_)));
    for i in &open {
        let shown = o.shown(wall, *i);
        assert_eq!(
            shown,
            Shown::Whole,
            "{}: the row {} of an open errand does not stand whole",
            o.who(),
            wall.pieces[*i].key
        );
    }
    open.len()
}

/// Statement 2: every word of the person and every answer of the companion
/// in the raw part of the plan stands in the window byte for byte, and so
/// do the words of the turn that ordered the rebuild. Returns how many
/// pieces of the wall it judged.
pub fn talk_is_whole(wall: &Wall, o: &Outcome) -> usize {
    let mut judged = 0;
    for i in wall.counted(|w| matches!(w, What::Words | What::Answer)) {
        if !o.raw(i) {
            continue;
        }
        let shown = o.shown(wall, i);
        assert_eq!(
            shown,
            Shown::Whole,
            "{}: {:?} {} of the raw part does not stand byte for byte\n{}",
            o.who(),
            wall.pieces[i].what,
            wall.pieces[i].key,
            o.view()
        );
        judged += 1;
    }
    let window = o.window();
    for said in &o.live {
        assert!(
            window.iter().any(|t| t.ends_with(said.as_str())),
            "{}: the words of the running turns stand whole: {said}",
            o.who()
        );
    }
    judged
}

/// Statement 3: every failed tool result in the raw part of the plan stands
/// in the window byte for byte. Returns how many it judged.
pub fn errors_stand(wall: &Wall, o: &Outcome) -> usize {
    let mut judged = 0;
    for i in wall.counted(|w| matches!(w, What::Error)) {
        if !o.raw(i) {
            continue;
        }
        let shown = o.shown(wall, i);
        assert_eq!(
            shown,
            Shown::Whole,
            "{}: the failed result {} does not stand byte for byte",
            o.who(),
            wall.pieces[i].key
        );
        judged += 1;
    }
    judged
}

/// The statement of an errand that ended without an answer (GH #1469): the
/// window after the rebuild holds one message that names the errand by the
/// head of its `(delegation)` row (its first line, cut where a one-line form
/// cuts it) and says its outcome -- `failed`, with the code the call came
/// back with, or `no answer` --, and that message is in no one-line form.
/// Returns how many errands it judged.
pub fn outcomes_stand(wall: &Wall, o: &Outcome) -> usize {
    let window = o.window();
    let mut judged = 0;
    for p in &wall.pieces {
        let Some(ending) = &p.errand else { continue };
        let head: String = p.text().chars().take(FORM_HEAD).collect();
        let head = head.trim_end();
        let (word, code) = ending.said();
        let said: Vec<&String> = window
            .iter()
            .filter(|t| {
                t.contains(head)
                    && t.contains(word)
                    && t.contains(code)
                    && !t.contains(" [shortened, ")
            })
            .collect();
        assert_eq!(
            said.len(),
            1,
            "{}: the outcome of the errand {} ({word}) stands in the window once, whole: {said:?}",
            o.who(),
            p.key
        );
        judged += 1;
    }
    judged
}

/// Whether the shipped cell `cell` declares the param `param`: a run sets a
/// knob only where the door would let a set turn it.
pub fn declared(cell: &str, param: &str) -> bool {
    cell_config(cell)["params"].get(param).is_some()
}

/// The tokens the policy counts a text of `chars` characters at when the
/// tap names no rate (`CHARS_PER_TOKEN`), rounded up.
fn tokens_of(chars: usize) -> i64 {
    chars.div_ceil(3) as i64
}

/// How far over its aim the window of the turn after a rebuild may lie, in
/// percent of the aim (OR-58-78): the envelope of the call. The rebuild
/// aims at the size of the blocks; the call lays keys, id prefixes and its
/// frame on top.
pub const ENVELOPE_PCT: i64 = 10;

/// The window a rebuild left, at the measure the watch reads and the
/// provider bills: `window_tokens` of the turn after the rebuild -- the
/// policy's own projection of the finished call --, less the words that
/// call added.
pub fn window_after(o: &Outcome) -> i64 {
    let added = tokens_of(o.live[2].chars().count());
    o.after["window_tokens"].as_i64().unwrap_or_else(|| {
        panic!(
            "{}: the turn after the rebuild names window_tokens: {}",
            o.who(),
            o.after
        )
    }) - added
}

/// Statement 4 (OR-58-78): after the rebuild `window_tokens` of the turn
/// after, less its new words, lies at most `ENVELOPE_PCT` percent over the
/// aim, or the rebuild wrote the mark `over_aim`. The statement catches a
/// rebuild that misses its aim by far (a class of the collection does not
/// take, the summary replaces nothing), not the envelope of the call.
pub fn lands_at_aim(o: &Outcome) {
    assert!(o.aim > 0, "{}: the rebuild has an aim", o.who());
    if let Some(mark) = o.over_aim.last() {
        // The rebuild said so itself; its mark names both sides.
        let (estimate, aim) = (mark["estimate"].as_i64(), mark["aim"].as_i64());
        assert!(
            estimate.is_some() && aim.is_some() && estimate > aim,
            "{}: the mark over_aim names an estimate over its aim: {mark}",
            o.who()
        );
        return;
    }
    let window = window_after(o);
    assert!(
        window * 100 <= o.aim * (100 + ENVELOPE_PCT),
        "{}: the window after the rebuild lies more than {ENVELOPE_PCT} percent over its aim and \
         the rebuild wrote no over_aim: {window} tokens (window_tokens of the turn after, less \
         its new words), aim {}\n{}",
        o.who(),
        o.aim,
        o.view()
    );
}

/// The window of a run beside its aim as one line `DISTANCE | wall | run |
/// aim | window | over_aim | percent`: `window_after`, the estimate of the
/// mark `over_aim` (`-` without one), and how far the window lies over the
/// aim in percent of the aim (negative: under it). A figure for the report.
pub fn distance_line(o: &Outcome) -> String {
    let window = window_after(o);
    format!(
        "DISTANCE | {} | {} | {} | {} | {} | {:+.1}",
        o.wall,
        o.run,
        o.aim,
        window,
        o.over_aim
            .last()
            .and_then(|m| m["estimate"].as_i64())
            .map_or("-".to_string(), |e| e.to_string()),
        (window - o.aim) as f64 * 100.0 / o.aim as f64
    )
}

/// Statement 5: for every class the rebuild collects, the one `gc` mark of
/// the rebuild names the counted amount. A class that collects at every
/// rebuild names the pieces of it the plan shows; a class that does not
/// collect under the knobs names none; and the classes of each one-line
/// form add up to the blocks the window shows in that form. A class the
/// rebuild does not collect (`Bank::class_of_the_call`: every call clears
/// it in the block it builds) has no figure in the mark and is not asked
/// for one -- statement 1 is the statement about it.
///
/// Through the cache (GH #1464, OR-d58-2.coldgc.12) two readings differ from
/// the rebuild of the target, which stays as it is. A collection that took
/// nothing writes no plan and no mark: then the run has no `gc` mark, and the
/// wall counts no raw garbage of a class that collects at every rebuild. And
/// a collection writes no summary: the results of closed parts the target
/// would hand to the summarizer stay raw, so with no summary in the plan the
/// class `shrunk_tools` counts every raw tool result outside `keep_rounds`
/// (`cold_tools`), not only the ones the wall counts as its garbage.
pub fn marks_count(bank: &Bank, wall: &Wall, o: &Outcome) {
    let cold = o.trigger == Trigger::Cache;
    if cold && o.plan["cover"].as_i64().is_none() {
        assert!(
            o.gc.is_empty(),
            "{}: a collection that wrote no plan wrote no gc mark: {:?}",
            o.who(),
            o.gc
        );
        // Nor did it pass over an old tool result it would have shrunk
        // (review coldgc M4): with nothing found, the wall holds none.
        let missed = cold_tools(wall, o);
        assert!(
            missed.is_empty(),
            "{}: the collection found nothing, yet the wall holds old tool results it shrinks: \
             {:?}",
            o.who(),
            missed
                .iter()
                .map(|i| wall.pieces[*i].id())
                .collect::<Vec<_>>()
        );
        for class in wall.classes() {
            if bank.collects(class, &o.knobs) != Collects::Always {
                continue;
            }
            let counted = wall
                .counted(|w| matches!(w, What::Junk(c) if *c == class))
                .len();
            assert_eq!(
                counted,
                0,
                "{}: the collection found nothing, yet the wall counts raw garbage of class \
                 {class}",
                o.who()
            );
        }
        return;
    }
    let unsummed = cold && o.plan["summary"].as_str().unwrap_or("").is_empty();
    let mark = o.gc_mark();
    let classes = mark["classes"]
        .as_object()
        .unwrap_or_else(|| panic!("{}: the gc mark names its classes: {mark}", o.who()));
    for (class, n) in classes {
        let n = n
            .as_i64()
            .unwrap_or_else(|| panic!("{}: class {class} is a whole number: {mark}", o.who()));
        let mut counted = wall
            .counted(|w| matches!(w, What::Junk(c) if *c == class.as_str()))
            .into_iter()
            .filter(|i| o.raw(*i))
            .count() as i64;
        if unsummed && class == "shrunk_tools" {
            counted += cold_tools(wall, o).len() as i64;
        }
        match bank.collects(class, &o.knobs) {
            Collects::Always => assert_eq!(
                n,
                counted,
                "{}: the mark of class {class} names the pieces the wall counts: {mark}",
                o.who()
            ),
            Collects::Never => assert_eq!(
                n,
                0,
                "{}: class {class} does not collect under these knobs: {mark}",
                o.who()
            ),
            Collects::OverAim => {}
        }
    }
    for class in wall.classes().into_iter().chain(wall.beside_classes()) {
        if !bank.rebuild_collects(class) {
            continue;
        }
        assert!(
            classes.contains_key(class),
            "{}: the gc mark knows the class {class} the wall counts: {mark}",
            o.who()
        );
    }
    for (form, of) in FORM_CLASSES {
        let marked: i64 = of
            .iter()
            .map(|c| classes.get(*c).and_then(Value::as_i64).unwrap_or(0))
            .sum();
        let shown = o.in_form(wall, form).len() as i64;
        assert_eq!(
            marked,
            shown,
            "{}: the classes {of:?} of the mark add up to the blocks the window shows as \
             {form:?}: {mark}",
            o.who()
        );
    }
}

/// The raw tool results of `wall` outside the last `keep_rounds` tool rounds
/// that the wall counts as nothing of their own (`What::Other`, no
/// `valid_until`) and the model did not pin: a collection through the cache
/// with no summary shrinks them with the class `shrunk_tools` (GH #1464).
/// A round is a turn of the
/// wall, the way every wall of the bank sows one tool round a turn.
pub fn cold_tools(wall: &Wall, o: &Outcome) -> Vec<usize> {
    let tools: Vec<usize> = (0..wall.pieces.len())
        .filter(|i| wall.pieces[*i].kind == "tool_result")
        .collect();
    let mut rounds: Vec<&str> = Vec::new();
    for i in &tools {
        let t = wall.pieces[*i].turn.as_str();
        if !rounds.contains(&t) {
            rounds.push(t);
        }
    }
    let keep = o.knobs.keep_rounds.max(0) as usize;
    let kept = &rounds[rounds.len().saturating_sub(keep)..];
    tools
        .into_iter()
        .filter(|i| {
            let p = &wall.pieces[*i];
            p.what == What::Other
                && p.el.get("valid_until").is_none_or(Value::is_null)
                && !kept.contains(&p.turn.as_str())
                && !o.pinned.contains(&p.id())
                && o.raw(*i)
        })
        .collect()
}

/// Statement 6: the plan snapshot of the call after the rebuild shows the
/// same window -- one block per message the call was sent, and every piece
/// of the wall the window shows named by its short id. Returns false when
/// the ledger keeps no snapshot of the call (nothing is judged then).
pub fn snapshot_shows(wall: &Wall, o: &Outcome) -> bool {
    let Some(blocks) = &o.snapshot else {
        return false;
    };
    let ids: Vec<String> = blocks
        .iter()
        .map(|b| b["id"].as_str().unwrap_or("").to_string())
        .collect();
    for i in 0..wall.pieces.len() {
        let id = wall.pieces[i].id();
        let listed = ids.iter().any(|b| b.ends_with(&format!(":{id}")));
        let shown = o.shown(wall, i) != Shown::Absent;
        assert_eq!(
            listed,
            shown,
            "{}: the snapshot and the window agree on the piece {} (#{id}): {ids:?}",
            o.who(),
            wall.pieces[i].key
        );
    }
    true
}

/// What statements 1 and 5 hold against `class` alone on a run, one line per
/// statement that fails. Statement 1 is read on the wall with the garbage of
/// every other class left out of the count, so the line is this class's and
/// no other's; statement 5 is read on the whole wall, and its line is kept
/// when it names the class.
pub fn against_class(bank: &Bank, wall: &Wall, o: &Outcome, class: &str) -> Vec<String> {
    let mut only = wall.clone();
    for p in &mut only.pieces {
        if matches!(p.what, What::Junk(c) if c != class) {
            p.what = What::Other;
        }
    }
    only.beside
        .retain(|(what, _)| !matches!(what, What::Junk(c) if *c != class));
    let mut found = Findings::default();
    found.judge("garbage", || garbage_is_out(bank, &only, o));
    found.judge("marks", || marks_count(bank, wall, o));
    let named = format!("class {class}");
    found
        .lines
        .into_iter()
        .filter(|l| l.contains(&named))
        .collect()
}

/// How a mutant switches a class off.
pub enum Off {
    /// By a knob of the policy, set beside the knobs of the run: the shipped
    /// curator runs the wall again without the class.
    Knob(&'static str, Value),
    /// The class has no knob. The run is what a curator without the class
    /// would have left (`kept`): the sabotage is made in the outcome, as the
    /// mutants of the watch make theirs in the ledger.
    Kept,
}

/// The statement of the bank a mutant expects to turn red.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Red {
    /// Statement 1: the garbage is still in the window.
    Window,
    /// Statement 5: the `gc` mark does not name the counted amount.
    Mark,
}

/// The outcome of `o` had the curator not collected `class`: every piece of
/// the class the window shows in a one-line form stands whole behind its id
/// again, and the `gc` mark counts none of the class.
pub fn kept(wall: &Wall, o: &Outcome, class: &'static str) -> Outcome {
    let mut out = o.clone();
    let mut messages = out.next.messages();
    for i in wall.counted(|w| matches!(w, What::Junk(c) if *c == class)) {
        if !matches!(o.shown(wall, i), Shown::Form(_)) {
            continue;
        }
        let p = &wall.pieces[i];
        let id = p.id();
        for m in messages.iter_mut() {
            let t = m["text"].as_str().unwrap_or("");
            if t.starts_with(&format!("[#{id} ")) {
                m["text"] = json!(format!("[#{id}] {}", p.text()));
            }
        }
    }
    out.next
        .body
        .insert("messages".to_string(), json!(messages));
    for mark in out.gc.iter_mut() {
        if mark["classes"].get(class).is_some() {
            mark["classes"][class] = json!(0);
        }
    }
    out
}

/// A mutant of the bank: `class` is switched off on the wall `name`, and the
/// bank turns red for that class. Every run of the wall on which the class
/// collects and the wall holds a piece of it in the raw part is run twice:
/// as it is (the statements hold nothing against the class) and with the
/// class off, judged by the knobs of the run as it is (the statement `red`
/// names the class). The role is the run's own. Returns the runs judged.
pub fn class_off_turns_the_bank_red(name: &str, class: &'static str, off: Off, red: Red) -> usize {
    let bank = bank();
    let wall = bank.named(name);
    let beside = wall.beside_classes().contains(&class);
    let mut judged = 0;
    for run in &wall.runs {
        if bank.collects(class, &knobs_of(wall, run)) != Collects::Always {
            continue;
        }
        let sound = rebuild(wall, run);
        let raw = wall
            .counted(|w| matches!(w, What::Junk(c) if *c == class))
            .into_iter()
            .any(|i| sound.raw(i));
        if !raw && !beside {
            continue;
        }
        let lines = against_class(&bank, wall, &sound, class);
        assert!(
            lines.is_empty(),
            "{}: the bank holds nothing against the class {class} while it collects: {lines:?}",
            sound.who()
        );
        let broken = match &off {
            Off::Knob(knob, value) => {
                let mut o = rebuild(wall, &run.clone().with(knob, value.clone()));
                o.knobs = sound.knobs.clone();
                o
            }
            Off::Kept => kept(wall, &sound, class),
        };
        let lines = against_class(&bank, wall, &broken, class);
        let head = match red {
            Red::Window => "garbage:",
            Red::Mark => "marks:",
        };
        assert!(
            lines.iter().any(|l| l.starts_with(head)),
            "{} with the class {class} off: the bank turns red for the class at {red:?}: \
             {lines:?}\n{}",
            broken.who(),
            broken.view()
        );
        judged += 1;
    }
    assert!(
        judged > 0,
        "{name}: a run collects the class {class} on a piece of the raw part"
    );
    judged
}

// THE SEATS OF THE BANK. One file per wall under `curator_bank/`, each with
// `register(&mut Bank)`. A wall is built in its own file; this list stays.
#[path = "curator_bank/coding.rs"]
pub mod coding;
#[path = "curator_bank/delegations.rs"]
pub mod delegations;
#[path = "curator_bank/errors.rs"]
pub mod errors;
#[path = "curator_bank/long_chat.rs"]
pub mod long_chat;
#[path = "curator_bank/notes.rs"]
pub mod notes;
#[path = "curator_bank/over_aim.rs"]
pub mod over_aim;
#[path = "curator_bank/phone_day.rs"]
pub mod phone_day;
#[path = "curator_bank/released.rs"]
pub mod released;
#[path = "curator_bank/research.rs"]
pub mod research;

/// One seat: the name of a wall and the function that registers it.
type Seat = (&'static str, fn(&mut Bank));

/// The seats, in the order of the plan.
const SEATS: [Seat; 9] = [
    ("phone_day", phone_day::register),
    ("long_chat", long_chat::register),
    ("delegations", delegations::register),
    ("released", released::register),
    ("notes", notes::register),
    ("errors", errors::register),
    ("over_aim", over_aim::register),
    ("coding", coding::register),
    ("research", research::register),
];

/// The bank: every registered wall, the rules of the classes, and the seats
/// that registered nothing (`pending`).
pub fn bank() -> Bank {
    let mut bank = Bank::default();
    bank.class("shrunk_tools", tools_rule);
    bank.class("dup_recall", with_gc);
    bank.class("transient", always);
    bank.class("released", always);
    bank.class("scaffold", scaffold_rule);
    for (seat, register) in SEATS {
        let before = bank.walls.len();
        register(&mut bank);
        if bank.walls.len() == before {
            bank.pending.push(seat);
        }
    }
    bank
}

// THE WALLS BY NAME (OR-58-102). A statement that runs every wall through
// the curator is one test per wall: a test over all walls ran as long as
// all of their runs, and under the load of a gate that came near its
// timeout. The names below are the one list the tests per wall are made
// from (`per_wall!`); `gh_d58_bank_every_role_runs_its_walls::
// the_wall_list_is_the_bank` holds it to the walls `bank()` registers, so a
// wall added to the bank and missing here turns red, and so does a name
// here no seat registers.

/// Hands the names of the walls of the bank to the macro `$then` after the
/// tokens `$pre` and a `;`, in the order `bank()` registers them.
///
/// The macros of this file are no `#[macro_export]`: they are re-exported
/// from the module (`pub(crate) use`), so a test file calls them by the name
/// its `use curator_bank::*;` brings, without a `crate::` path --
/// `scripts/harness_fold.py` keeps a file with a `crate::` path out of its
/// harness.
macro_rules! bank_walls {
    ($then:ident; $($pre:tt)*) => {
        $crate::curator_bank::$then! {
            $($pre)*;
            phone_day long_chat delegations released notes errors over_aim coding research
        }
    };
}

/// The names of the walls as an array (`bank_walls!(wall_names;)`).
macro_rules! wall_names {
    (; $($wall:ident)+) => {
        [$(stringify!($wall)),+]
    };
}

/// One `#[test]` per wall, named after the wall, that calls `$judge(name)`:
/// `per_wall!(judge)` makes one for every wall of the bank,
/// `per_wall!(judge; long_chat coding)` one for each wall named. A second
/// way through the same walls (another trigger, other knobs) is a second
/// `per_wall!` inside a `mod` of its own, so the names do not clash.
#[allow(unused_macros)]
macro_rules! per_wall {
    ($judge:ident) => {
        $crate::curator_bank::bank_walls!(per_wall; $judge);
    };
    ($judge:ident; $($wall:ident)+) => {
        $(
            #[test]
            fn $wall() {
                $judge(stringify!($wall));
            }
        )+
    };
}

#[allow(unused_imports)]
pub(crate) use {bank_walls, per_wall, wall_names};

/// The walls of the bank by name, as `per_wall!` makes its tests.
pub const WALLS: &[&str] = &bank_walls!(wall_names;);

/// The violations of a test that judges many runs. A statement that fails
/// on one run must not hide the runs behind it: every statement is judged
/// on every run (`judge`), each violation is kept, and the test fails once,
/// at its end (`finish`), with all of them. Nothing is let through: one
/// line is a red test.
#[derive(Default)]
pub struct Findings {
    pub lines: Vec<String>,
}

impl Findings {
    /// Runs one statement. What it asserts and misses is kept as a line
    /// under `what` (the head of the message: wall, role, measured value and
    /// aim as the statement words them, and the two sides of an equality);
    /// the whole message stands above, where the statement printed it.
    pub fn judge<T>(&mut self, what: &str, statement: impl FnOnce() -> T) -> Option<T> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(statement)) {
            Ok(v) => Some(v),
            Err(e) => {
                let said = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "a panic without words".to_string());
                let mut head: Vec<String> = Vec::new();
                for (i, line) in said.lines().enumerate() {
                    let side = line.trim_start();
                    if i == 0 || side.starts_with("left:") || side.starts_with("right:") {
                        head.push(line.trim().chars().take(400).collect());
                    }
                }
                self.lines.push(format!("{what}: {}", head.join(" | ")));
                None
            }
        }
    }

    /// One violation a test found itself.
    pub fn note(&mut self, line: String) {
        self.lines.push(line);
    }

    /// The end of the test: red with every violation, one per line.
    pub fn finish(self) {
        assert!(
            self.lines.is_empty(),
            "{} violation(s) over all runs:\n{}",
            self.lines.len(),
            self.lines.join("\n")
        );
    }
}

/// Every run of every wall `pick` chooses, through its rebuild: the wall,
/// the run, what it left. A run whose rebuild does not come about is a
/// violation of its own in `found`, and the runs behind it are still run.
pub fn run_walls<'a>(
    bank: &'a Bank,
    pick: impl Fn(&Wall) -> bool,
    found: &mut Findings,
) -> Vec<(&'a Wall, Outcome)> {
    run_walls_as(bank, pick, found, Run::clone)
}

/// `run_walls` with every run of a wall made `as_run` of it first --
/// `Run::cold`: the same runs as rebuilds for an expired cache.
pub fn run_walls_as<'a>(
    bank: &'a Bank,
    pick: impl Fn(&Wall) -> bool,
    found: &mut Findings,
    as_run: fn(&Run) -> Run,
) -> Vec<(&'a Wall, Outcome)> {
    let mut out = Vec::new();
    for wall in bank.walls.iter().filter(|w| pick(w)) {
        for run in &wall.runs {
            let run = as_run(run);
            if let Some(o) = found.judge("rebuild", || rebuild(wall, &run)) {
                out.push((wall, o));
            }
        }
    }
    out
}
