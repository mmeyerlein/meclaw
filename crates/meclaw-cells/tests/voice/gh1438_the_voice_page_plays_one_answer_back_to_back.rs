//! GH #1438 -- the display's voice page plays the syntheses of one answer back to
//! back, never over each other.
//!
//! The OS mark's hook schedules what the `voice` cell speaks on a running play
//! clock (`playAt`): a chunk starts at `max(now + lead, playAt)` and moves the clock
//! to its own end. Every `speak_start` used to set that clock back to 0, so a new
//! synthesis started at once even while the previous one was still playing. With
//! `channels/voice.speak_partials "1"` one answer of two sentences is two syntheses
//! shortly after each other -- the provider streams faster than real time -- and the
//! second sentence played over the rest of the first: two voices at once (found on
//! the colony in the d53 hand-over; the colony side was correct, one answer and one
//! final). `max(...)` already discards a stale clock after a pause, so the reset was
//! never needed for that.
//!
//! What this runs is the hook's own bytes: `compose.py` is asked for
//! `OS_CLIENT_JS`, the slice from the hook's playback state up to `onClose` is cut
//! out of it and executed under `node` against an `AudioContext` whose clock the
//! test moves by hand. So the locks below measure WHEN a chunk starts, not which
//! words the source contains. The rest of the hook needs a DOM and a socket and is
//! not part of the question.
//!
//! Skips when the templates do not ship, or `python3` or `node` is missing (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// The harness: the slice of the hook, a fake `AudioContext` with a hand-moved
/// clock, and a script of frames and audio. It prints one JSON object.
const HARNESS: &str = r#"
const src = require("fs").readFileSync(process.argv[1], "utf8");
const head = "var ctx = null, mctx = null";
const end = "function onClose(c) {";
const a = src.indexOf(head), b = src.indexOf(end);
if (a < 0 || b < a) { console.log(JSON.stringify({ error: "the hook has no playback slice" })); process.exit(0); }
const body = src.slice(a, b);
const sources = [];
let clock = 0;
function Ctx(opts) { this.sampleRate = (opts && opts.sampleRate) || 24000; this.destination = {}; this.state = "running"; }
Object.defineProperty(Ctx.prototype, "currentTime", { get: function () { return clock; } });
Ctx.prototype.close = function () {};
Ctx.prototype.createBuffer = function (ch, len, rate) { return { duration: len / rate, getChannelData: function () { return { set: function () {} }; } }; };
Ctx.prototype.createBufferSource = function () {
  const s = { id: 0, at: null, stopped: false, connect: function () {}, start: function (t) { s.at = t; }, stop: function () { s.stopped = true; } };
  sources.push(s); return s;
};
const root = { AudioContext: Ctx };
const st = { played: 0, turns: 0, speakEnd: 0, hello: null };
const hook = new Function("root", "st", "phase", "say", "idleText", "WORDS",
  body + "\nreturn { onFrame: onFrame, onAudio: onAudio, bargeIn: typeof bargeIn === \"function\" ? bargeIn : function () {} };")(
  root, st, function () {}, function () {}, function () { return ""; }, {});
// One second of 24 kHz PCM16 is 48 000 bytes.
function second() { return new ArrayBuffer(48000); }
function chunks(n) { const from = sources.length; for (let i = 0; i < n; i++) hook.onAudio(second()); return sources.slice(from); }
// The clock moves by hand; a source whose second has run out ends, as in a browser.
function tick(t) {
  clock = t;
  sources.forEach(function (s) { if (!s.ended && s.at !== null && s.at + 1 <= t) { s.ended = true; if (s.onended) s.onended(); } });
}
const out = {};
hook.onFrame({ type: "hello", stt: "s", tts: "t", audio_out: { sample_rate: 24000 } });
// (1) One answer, two sentences: the first synthesis streams 2 s of audio at t=0,
// the second starts 0.3 s later while the first is still playing.
hook.onFrame({ type: "speak_start" });
const first = chunks(2);
hook.onFrame({ type: "speak_end", reason: "done" });
tick(0.3);
hook.onFrame({ type: "speak_start" });
const second2 = chunks(1);
out.first_end = first[1].at + 1;
out.second_start = second2[0].at;
// (2) A pause longer than everything queued: 10 s later a new synthesis.
hook.onFrame({ type: "speak_end", reason: "done" });
tick(10);
hook.onFrame({ type: "speak_start" });
out.after_pause = chunks(1)[0].at;
out.after_pause_now = clock;
// (3) A barge-in: 3 s queued at t=20, cancelled at t=20.5, then a new synthesis.
hook.onFrame({ type: "speak_end", reason: "done" });
tick(20);
hook.onFrame({ type: "speak_start" });
const cut = chunks(3);
tick(20.5);
hook.onFrame({ type: "speak_end", reason: "cancelled" });
hook.onFrame({ type: "speak_start" });
out.after_cancel = chunks(1)[0].at;
out.after_cancel_now = clock;
out.cancelled_stopped = cut.filter(function (s) { return s.stopped; }).length;
out.done_stopped = first.concat(second2).filter(function (s) { return s.stopped; }).length;
// (4) A rejoin says hello again and builds a new context whose clock starts at 0;
// a clock left over from the old context would push the next chunk minutes out.
tick(300);
hook.onFrame({ type: "speak_start" });
chunks(60);
tick(0);
hook.onFrame({ type: "hello", stt: "s", tts: "t", audio_out: { sample_rate: 24000 } });
hook.onFrame({ type: "speak_start" });
out.after_rejoin = chunks(1)[0].at;
// (5) A barge-in by key after `speak_end done`: the cell has sent the last chunk, so
// it has nothing left to cancel and says no `speak_end cancelled`, but 3 s are still
// queued here. The press itself drops them, as the voice test page does.
tick(400);
hook.onFrame({ type: "speak_start" });
const keyed = chunks(3);
hook.onFrame({ type: "speak_end", reason: "done" });
tick(400.5);
hook.bargeIn();
hook.onFrame({ type: "speak_start" });
out.after_key = chunks(1)[0].at;
out.after_key_now = clock;
out.key_stopped = keyed.filter(function (s) { return s.stopped; }).length;
// (6) Behind a duplex provider a hold opens the model's ear and cuts no turn, so the
// press leaves the answer playing.
tick(0);
hook.onFrame({ type: "hello", stt: "d", tts: "d", duplex: true, audio_out: { sample_rate: 24000 } });
hook.onFrame({ type: "speak_start" });
const duplex = chunks(3);
tick(0.5);
hook.bargeIn();
out.duplex_stopped = duplex.filter(function (s) { return s.stopped; }).length;
console.log(JSON.stringify(out));
"#;

/// What the harness measured, or `None` where the tree, `python3` or `node` is missing.
fn measured() -> Option<serde_json::Value> {
    if !library_ships() {
        return None;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let os = td.path().join("os.js");
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             open(sys.argv[2], 'w').write(m.OS_CLIENT_JS)",
        )
        .arg(repo(COMPOSE))
        .arg(&os)
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = std::process::Command::new("node")
        .arg("-e")
        .arg(HARNESS)
        .arg(&os)
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    assert!(
        run.status.success(),
        "the harness failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("the harness printed no JSON ({e}):\n{stdout}"));
    assert!(v.get("error").is_none(), "{v}");
    Some(v)
}

fn num(v: &serde_json::Value, key: &str) -> f64 {
    v[key]
        .as_f64()
        .unwrap_or_else(|| panic!("{key} is not a number in {v}"))
}

#[test]
fn two_consecutive_speak_starts_of_one_answer_never_overlap() {
    let Some(v) = measured() else { return };
    let (end, start) = (num(&v, "first_end"), num(&v, "second_start"));
    assert!(
        start >= end - 1e-9,
        "two consecutive speak_start of one answer never overlap: the second starts \
         at or after the scheduled end of the first (first ends {end}, second starts \
         {start}): {v}"
    );
}

#[test]
fn after_a_pause_longer_than_the_queued_audio_a_new_synthesis_starts_without_added_delay() {
    let Some(v) = measured() else { return };
    let (at, now) = (num(&v, "after_pause"), num(&v, "after_pause_now"));
    assert!(
        (at - (now + 0.02)).abs() < 1e-9,
        "after a pause longer than the queued audio a new synthesis starts without \
         added delay (now {now}, started {at}): {v}"
    );
    let rejoin = num(&v, "after_rejoin");
    assert!(
        (rejoin - 0.02).abs() < 1e-9,
        "a rejoin's new context starts its own play clock; a clock left from the old \
         context delays the next synthesis (started {rejoin}): {v}"
    );
}

#[test]
fn a_cancelled_synthesis_is_cut_and_the_next_one_starts_without_its_rest() {
    let Some(v) = measured() else { return };
    assert_eq!(
        v["cancelled_stopped"],
        serde_json::json!(3),
        "a barge-in or stop clears the queued audio of the cancelled synthesis: {v}"
    );
    assert_eq!(
        v["done_stopped"],
        serde_json::json!(0),
        "a synthesis that ended normally is never cut: {v}"
    );
    let (at, now) = (num(&v, "after_cancel"), num(&v, "after_cancel_now"));
    assert!(
        (at - (now + 0.02)).abs() < 1e-9,
        "after a cancel the next synthesis starts without the rest of the cancelled \
         one in front of it (now {now}, started {at}): {v}"
    );
}

#[test]
fn a_barge_in_by_key_after_the_last_chunk_drops_what_is_still_queued() {
    let Some(v) = measured() else { return };
    assert_eq!(
        v["key_stopped"],
        serde_json::json!(3),
        "a press after `speak_end done` stops the audio still queued on the page; the \
         cell has nothing left to cancel and sends no `cancelled`: {v}"
    );
    let (at, now) = (num(&v, "after_key"), num(&v, "after_key_now"));
    assert!(
        (at - (now + 0.02)).abs() < 1e-9,
        "the answer after a barge-in by key starts without the rest of the old one in \
         front of it (now {now}, started {at}): {v}"
    );
    assert_eq!(
        v["duplex_stopped"],
        serde_json::json!(0),
        "behind a duplex provider a hold only opens the ear and cuts nothing: {v}"
    );
}
