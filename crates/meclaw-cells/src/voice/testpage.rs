//! The built-in browser test page the `voice` cell serves at `GET /` (R-V9).
//!
//! One static string, no seed row, no template file: the page is part of the
//! cell the way the wire protocol is. A voice cell that boots on a machine
//! with nothing else installed can be pointed at with a browser and answers
//! with something that speaks its own protocol — press a key, watch partials
//! arrive, hear the synthesis come back. That is the calibration tool the
//! echo provider is for, made visible.
//!
//! Self-contained on purpose: no stylesheet link, no CDN script, no build
//! step. Everything the page needs — including the `AudioWorklet` that turns
//! microphone floats into PCM16 — lives in this string and is installed from
//! a blob URL at runtime. The two unit tests below hold that line.
//!
//! The page is a diagnostic, not a product: no design ambition, no automatic
//! reconnect, no state it keeps across a reload.

/// The built-in browser test page, as one self-contained HTML document.
///
/// Served verbatim as `text/html` from `GET /` on the cell's own listener.
pub fn html() -> &'static str {
    PAGE
}

const PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>meclaw voice test page</title>
<style>
:root { color-scheme: light dark; }
body { font: 14px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; margin: 1.5rem; max-width: 60rem; }
h1 { font-size: 1.2rem; margin: 0 0 .25rem; }
p.hint { margin: 0 0 1rem; padding: .5rem .75rem; border-left: 3px solid #c80; opacity: .85; }
fieldset { border: 1px solid #8888; margin: 0 0 .75rem; padding: .5rem .75rem; }
legend { padding: 0 .35rem; opacity: .7; }
button { font: inherit; padding: .3rem .7rem; margin-right: .35rem; }
button#talk { padding: .8rem 1.6rem; }
button#talk.live { outline: 3px solid #2a2; }
dl#facts { display: grid; grid-template-columns: max-content 1fr; gap: .1rem .75rem; margin: 0; }
dl#facts dt { opacity: .6; }
dl#facts dd { margin: 0; }
pre#log { border: 1px solid #8888; padding: .5rem .75rem; height: 22rem; overflow: auto; white-space: pre-wrap; }
</style>
</head>
<body>

<h1>meclaw voice &mdash; built-in test page</h1>
<p class="hint">
  The microphone needs a secure context: localhost or a TLS proxy in front; a LAN IP will not work.
  Playback and the socket work anywhere; only capture is gated by the browser.
</p>

<fieldset>
  <legend>connection</legend>
  <label><input type="radio" name="mode" value="hold" checked> hold</label>
  <label><input type="radio" name="mode" value="auto"> auto</label>
  <label>session <input id="session" size="14" placeholder="(server picks one)"></label>
  <button id="connect">connect</button>
  <button id="disconnect" disabled>disconnect</button>
</fieldset>

<fieldset>
  <legend>hello</legend>
  <dl id="facts">
    <dt>protocol</dt><dd id="f-protocol">&ndash;</dd>
    <dt>session_id</dt><dd id="f-session">&ndash;</dd>
    <dt>mode</dt><dd id="f-mode">&ndash;</dd>
    <dt>audio_in</dt><dd id="f-in">&ndash;</dd>
    <dt>audio_out</dt><dd id="f-out">&ndash;</dd>
    <dt>stt</dt><dd id="f-stt">&ndash;</dd>
    <dt>tts</dt><dd id="f-tts">&ndash;</dd>
  </dl>
</fieldset>

<fieldset>
  <legend>talk</legend>
  <button id="talk" disabled>hold to talk (or hold the space bar)</button>
  <button id="cancel" disabled>cancel</button>
  <button id="clear">clear log</button>
</fieldset>

<pre id="log"></pre>

<script type="module">
const el = (id) => document.getElementById(id);
const logEl = el('log');

let ws = null;         // the open socket, or null
let hello = null;      // the hello frame of the current connection
let mode = 'hold';     // the mode the CELL confirmed; the radios only propose
let t0 = 0;            // performance.now() at connect, for relative stamps
let micNode = null;    // AudioWorkletNode, rebuilt for every connection
let capturePending = null; // in-flight capture build, so it happens once
let micStream = null;  // the MediaStream behind it, so it can be released
let micCtx = null;     // capture AudioContext (browser rate, never resampled here)
let outCtx = null;     // playback AudioContext at audio_out.sample_rate
let scheduled = [];    // sources already queued, so a cancel can drop them
let nextPlayAt = 0;    // scheduling cursor that keeps playback gapless
let wantSending = false; // what the button says right now
let sending = false;   // true while captured frames may go on the wire
let holding = false;   // true between a hold frame and its release

/* ---------------------------------------------------------------- log --- */

function line(text) {
  const secs = t0 ? (performance.now() - t0) / 1000 : 0;
  logEl.textContent += secs.toFixed(3).padStart(8) + '  ' + text + '\n';
  logEl.scrollTop = logEl.scrollHeight;
}

/* ------------------------------------------------------------- worklet --- */

// Installed from a blob URL: an AudioWorklet needs a module of its own, and
// this page refuses to fetch anything. It frames the capture into 20 ms PCM16
// LE buffers and, when the browser would not give us the declared rate,
// resamples linearly on the way. The cell never resamples; the client does.
const workletSource = `
class PcmFramer extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.targetRate = options.processorOptions.targetRate;
    this.step = sampleRate / this.targetRate;
    this.frame = Math.round(this.targetRate / 50);
    this.out = new Int16Array(this.frame);
    this.filled = 0;
    this.pos = 0;
    this.tail = new Float32Array(0);
  }
  process(inputs) {
    const chunk = inputs[0] && inputs[0][0];
    if (!chunk) return true;
    const buf = new Float32Array(this.tail.length + chunk.length);
    buf.set(this.tail, 0);
    buf.set(chunk, this.tail.length);
    let p = this.pos;
    while (p + 1 < buf.length) {
      const i = p | 0;
      const frac = p - i;
      let s = buf[i] * (1 - frac) + buf[i + 1] * frac;
      if (s > 1) s = 1; else if (s < -1) s = -1;
      this.out[this.filled++] = s < 0 ? s * 0x8000 : s * 0x7fff;
      if (this.filled === this.frame) {
        const copy = this.out.slice();
        this.port.postMessage(copy.buffer, [copy.buffer]);
        this.filled = 0;
      }
      p += this.step;
    }
    // p overshoots the block by up to one step. Clamping the carry to the
    // block length is what keeps the phase: without it every block restarts
    // near zero, the overhang is swallowed and the output drifts.
    const used = Math.min(p | 0, buf.length);
    this.tail = buf.slice(used);
    this.pos = p - used;
    return true;
  }
}
registerProcessor('pcm-framer', PcmFramer);
`;

// A provider streams faster than real time, so seconds of audio can already
// be scheduled when a cancel or a barge-in arrives. Dropping them is the
// whole point of cancel, so every started source stays reachable until it
// has ended.
function stopScheduled() {
  for (const src of scheduled) {
    try { src.stop(); } catch (e) { /* already finished */ }
  }
  scheduled = [];
  nextPlayAt = 0;
}

function teardownCapture() {
  if (micStream) { for (const track of micStream.getTracks()) track.stop(); micStream = null; }
  if (micCtx) { micCtx.close(); micCtx = null; }
  micNode = null;
  capturePending = null;
}

// Torn down on every socket close: the capture chain is built for the rate
// the cell declared in hello, and the next connection may declare another.
function teardownAudio() {
  teardownCapture();
  stopScheduled();
  if (outCtx) { outCtx.close(); outCtx = null; }
}

async function buildCapture() {
  const targetRate = hello.audio_in.sample_rate;
  let filtered;
  try {
    micStream = await navigator.mediaDevices.getUserMedia({
      audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
    });
    // Checked after every await: the socket can close while the permission
    // dialog is up, and the teardown that runs then cannot reach handles this
    // build has not assigned yet - it would leave the microphone open.
    if (!hello) { teardownCapture(); return; }
    // Ask the browser for the declared rate first: its own resampler filters,
    // the linear one in the worklet does not. Only when the browser refuses
    // the rate does the worklet have to resample at all.
    try {
      micCtx = new AudioContext({ sampleRate: targetRate });
    } catch (e) {
      micCtx = null;
    }
    if (!micCtx) micCtx = new AudioContext();
    filtered = micCtx.sampleRate === targetRate;
    const url = URL.createObjectURL(new Blob([workletSource], { type: 'text/javascript' }));
    try {
      await micCtx.audioWorklet.addModule(url);
    } finally {
      URL.revokeObjectURL(url);
    }
    if (!hello) { teardownCapture(); return; }
    micNode = new AudioWorkletNode(micCtx, 'pcm-framer', {
      processorOptions: { targetRate: targetRate },
    });
    micNode.port.onmessage = (ev) => {
      if (sending && ws && ws.readyState === WebSocket.OPEN) ws.send(ev.data);
    };
    // Pulled through a silent gain node: an unconnected worklet may never run.
    const silent = micCtx.createGain();
    silent.gain.value = 0;
    micCtx.createMediaStreamSource(micStream).connect(micNode).connect(silent).connect(micCtx.destination);
  } catch (e) {
    teardownCapture();   // a half-built chain would hold the microphone open
    throw e;
  }
  line(filtered
    ? 'microphone open at ' + micCtx.sampleRate + ' Hz - the browser delivers the declared rate, filtered'
    : 'microphone open at ' + micCtx.sampleRate + ' Hz - linear resample to ' + targetRate + ' Hz in the worklet');
}

// Single-flight. Press, permission dialog, release, press again: micNode is
// still null, so a second build would start while the first is still in the
// dialog - two microphones, two contexts, and the orphaned worklet keeps
// posting, which interleaves two captures on the same wire.
function startCapture() {
  if (micNode || !hello) return Promise.resolve();
  if (!capturePending) {
    capturePending = buildCapture().finally(() => { capturePending = null; });
  }
  return capturePending;
}

/* ------------------------------------------------------------ playback --- */

// `Int16Array` reads the buffer in the platform's byte order; every browser
// this page can run in is little endian, which is what the wire declares, so
// the match is an assumption stated rather than a conversion performed.
function playPcm(bytes) {
  if (!outCtx) return;
  if (bytes.byteLength % 2 !== 0) {
    line('dropped an audio frame of ' + bytes.byteLength + ' bytes: not whole PCM16 samples');
    return;
  }
  const pcm = new Int16Array(bytes);
  const floats = new Float32Array(pcm.length);
  for (let i = 0; i < pcm.length; i++) floats[i] = pcm[i] / 0x8000;
  const buffer = outCtx.createBuffer(1, floats.length, outCtx.sampleRate);
  buffer.copyToChannel(floats, 0);
  const src = outCtx.createBufferSource();
  src.buffer = buffer;
  src.connect(outCtx.destination);
  const now = outCtx.currentTime;
  if (nextPlayAt < now) nextPlayAt = now + 0.06;   // small cushion after a gap
  src.start(nextPlayAt);
  nextPlayAt += buffer.duration;
  scheduled.push(src);
  src.onended = () => { scheduled = scheduled.filter((s) => s !== src); };
}

/* --------------------------------------------------------------- wire --- */

function send(frame) {
  if (!ws || ws.readyState !== WebSocket.OPEN) { line('not connected'); return; }
  ws.send(JSON.stringify(frame));
}

// The radios are a request; `hello` and every `mode` frame are the answer,
// and only the answer decides what the page does with the button.
function selectedMode() {
  return document.querySelector('input[name=mode]:checked').value;
}

function setMode(confirmed) {
  mode = confirmed;
  for (const radio of document.querySelectorAll('input[name=mode]')) {
    radio.checked = radio.value === confirmed;
  }
  labelTalk();
}

function socketUrl() {
  const params = new URLSearchParams({ mode: selectedMode() });
  const session = el('session').value.trim();
  if (session) params.set('session', session);
  // Relative to the page, never rooted at the host: behind a reverse proxy this
  // page is served under a prefix, and an absolute '/ws' would leave it. The
  // cell serves the page at its own root, so the page's path is a directory --
  // said out loud here, because a proxy that maps '/voice' without the trailing
  // slash would otherwise resolve 'ws' against '/' and land outside the prefix.
  // The scheme is the one thing rewritten: a WebSocket URL has no relative form
  // for it.
  const base = new URL(location.href);
  if (!base.pathname.endsWith('/')) base.pathname += '/';
  const url = new URL('ws?' + params.toString(), base);
  url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return url.toString();
}

function showHello(frame) {
  const fmt = (f) => (f ? f.encoding + ' ' + f.sample_rate + ' Hz ' + f.channels + ' ch' : 'none');
  el('f-protocol').textContent = frame.protocol;
  el('f-session').textContent = frame.session_id;
  el('f-mode').textContent = frame.mode;
  el('f-in').textContent = fmt(frame.audio_in);
  el('f-out').textContent = fmt(frame.audio_out);
  el('f-stt').textContent = frame.stt;
  el('f-tts').textContent = frame.tts === null ? 'null' : frame.tts;
}

function onText(frame) {
  switch (frame.type) {
    case 'hello':
      hello = frame;
      showHello(frame);
      setMode(frame.mode);
      if (frame.audio_out) {
        outCtx = new AudioContext({ sampleRate: frame.audio_out.sample_rate });
        nextPlayAt = 0;
      }
      el('talk').disabled = false;
      el('cancel').disabled = false;
      line('hello: ' + frame.stt + ' -> ' + (frame.tts === null ? 'no tts' : frame.tts));
      if (frame.stt === 'echo') {
        line('echo provider: audio comes straight back, no partial or turn frames;');
        line('  hold, release and cancel answer with error wrong_mode - use auto mode');
      }
      break;
    case 'partial':
      line((frame.eager ? '[eager] ' : '        ') + 'partial: ' + frame.text);
      break;
    case 'turn':
      line('turn ' + frame.turn_id + ': ' + frame.text);
      break;
    case 'speak_start':
      line('speak_start ' + frame.speak_id);
      break;
    case 'speak_end':
      line('speak_end ' + frame.speak_id + ' ' + frame.reason + (frame.detail ? ' (' + frame.detail + ')' : ''));
      if (frame.reason === 'cancelled') stopScheduled();
      break;
    case 'mode':
      setMode(frame.mode);
      line('mode ' + frame.mode);
      break;
    case 'error':
      line('error ' + frame.code + ': ' + frame.detail);
      setMode(mode);   // a refused mode request must not leave the radios lying
      break;
    default:
      line('unknown frame type ' + frame.type);
  }
}

function connect() {
  if (ws) return;
  t0 = performance.now();
  logEl.textContent = '';
  ws = new WebSocket(socketUrl());
  ws.binaryType = 'arraybuffer';
  ws.onopen = () => {
    line('socket open');
    el('connect').disabled = true;
    el('disconnect').disabled = false;
  };
  ws.onmessage = (ev) => {
    if (typeof ev.data === 'string') {
      let frame;
      try { frame = JSON.parse(ev.data); } catch (e) { line('unparseable text frame'); return; }
      onText(frame);
    } else {
      playPcm(ev.data);
    }
  };
  ws.onerror = () => line('socket error');
  ws.onclose = (ev) => {
    line('socket closed ' + ev.code + (ev.reason ? ' ' + ev.reason : ''));
    ws = null;
    hello = null;
    teardownAudio();
    wantSending = false;
    sending = false;
    holding = false;
    el('talk').classList.remove('live');
    el('connect').disabled = false;
    el('disconnect').disabled = true;
    el('talk').disabled = true;
    el('cancel').disabled = true;
  };
}

/* ---------------------------------------------------------- push to talk --- */

// In hold mode the button is push-to-talk: the press opens the turn
// boundary, the release closes it and is what makes exactly one turn. In auto
// mode there is no boundary to draw, so the same button is a plain mic switch
// and the provider decides where turns end.
async function startSending() {
  if (wantSending || !ws) return;
  // The intent is recorded BEFORE the first await: the permission dialog can
  // outlive the press, and a release that arrives meanwhile must not be lost
  // - it would leave the turn boundary open with nothing to close it.
  wantSending = true;
  el('talk').classList.add('live');
  try {
    await startCapture();
    if (micCtx && micCtx.state === 'suspended') await micCtx.resume();
    if (outCtx && outCtx.state === 'suspended') await outCtx.resume();
  } catch (e) {
    line('microphone refused: ' + e.name + ' (' + e.message + ')');
    wantSending = false;
    el('talk').classList.remove('live');
    return;
  }
  if (!wantSending) {          // released while the dialog was up
    el('talk').classList.remove('live');
    return;
  }
  if (mode === 'hold' && !holding) {
    stopScheduled();           // barge-in by key: drop what is still queued
    send({ type: 'hold' });
    holding = true;
  }
  sending = true;
}

function stopSending() {
  wantSending = false;
  sending = false;
  el('talk').classList.remove('live');
  if (holding) {
    send({ type: 'release' });
    holding = false;
  }
}

function pressTalk() {
  if (mode === 'auto') {
    if (wantSending) stopSending(); else startSending();
  } else {
    startSending();
  }
}

function releaseTalk() {
  if (mode === 'hold') stopSending();
}

function labelTalk() {
  el('talk').textContent = mode === 'hold'
    ? 'hold to talk (or hold the space bar)'
    : 'microphone on / off';
}

el('connect').addEventListener('click', connect);
el('disconnect').addEventListener('click', () => { if (ws) ws.close(1000, 'bye'); });
el('cancel').addEventListener('click', () => send({ type: 'cancel' }));
el('clear').addEventListener('click', () => { logEl.textContent = ''; });

const talk = el('talk');
talk.addEventListener('pointerdown', (ev) => { ev.preventDefault(); pressTalk(); });
talk.addEventListener('pointerup', releaseTalk);
talk.addEventListener('pointerleave', releaseTalk);
talk.addEventListener('pointercancel', releaseTalk);

addEventListener('keydown', (ev) => {
  if (ev.code === 'Space' && !ev.repeat && ev.target.tagName !== 'INPUT') { ev.preventDefault(); pressTalk(); }
});
addEventListener('keyup', (ev) => {
  if (ev.code === 'Space' && ev.target.tagName !== 'INPUT') { ev.preventDefault(); releaseTalk(); }
});

for (const radio of document.querySelectorAll('input[name=mode]')) {
  radio.addEventListener('change', () => {
    stopSending();
    if (ws && ws.readyState === WebSocket.OPEN) {
      send({ type: 'mode', mode: radio.value });   // the cell answers, then setMode
    } else {
      setMode(radio.value);                        // before connect the radio is all there is
    }
  });
}

setMode(selectedMode());
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::html;

    /// Every frame type of the wire protocol has to be reachable from the
    /// page, otherwise it is not a test page for *this* protocol.
    #[test]
    fn page_mentions_every_frame_type() {
        let page = html();
        for needle in [
            "hello",
            "partial",
            "turn",
            "speak_start",
            "speak_end",
            "hold",
            "release",
            "cancel",
            "AudioWorklet",
            "'ws?'",
        ] {
            assert!(page.contains(needle), "test page never mentions {needle}");
        }
    }

    /// The socket address is built relative to the page, so the cell survives a
    /// reverse proxy that serves it under a path prefix (`/voice/` and
    /// `/voice/ws`, not `/ws`). Only the scheme is rewritten, because a
    /// WebSocket URL has no relative form for it.
    #[test]
    fn page_builds_a_relative_socket_url() {
        let page = html();
        assert!(
            page.contains("new URL('ws?' + params.toString(), base)"),
            "the socket url must be resolved against the page, not against the host"
        );
        assert!(
            page.contains("if (!base.pathname.endsWith('/')) base.pathname += '/'"),
            "the page's own path is the directory the socket sits in, even when \
             a proxy mounted it without the trailing slash"
        );
        assert!(
            !page.contains("location.host"),
            "an absolute '/ws' leaves the prefix a proxy mounted this page under"
        );
        assert!(
            page.contains("url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'"),
            "the scheme still follows the page's, so a TLS proxy gets wss"
        );
    }

    /// The page must work on a machine with no internet: no foreign script,
    /// no external stylesheet, nothing to fetch but the socket itself.
    #[test]
    fn page_is_self_contained() {
        let page = html();
        assert!(
            !page.contains("http://"),
            "test page loads something over http"
        );
        assert!(
            !page.contains("https://"),
            "test page loads something over https"
        );
        assert!(
            !page.contains("<link"),
            "test page pulls an external stylesheet"
        );
    }
}
