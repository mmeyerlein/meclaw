//! A WebSocket client for the `voice` cell, for tests and for measuring.
//!
//! Wave voice-cell (2026-09-05), strand t5. The cell speaks `meclaw-voice/1`:
//! text frames are JSON objects tagged by `type`, binary frames are raw
//! PCM16-LE mono audio in the rate the `hello` frame declares. This helper is
//! the other end of that: connect, read `hello`, stream a WAV in real time,
//! collect what comes back with the instant it arrived.
//!
//! # Why the frames are `serde_json::Value` and not the cell's own types
//!
//! `meclaw-testing` is a dependency *of* `meclaw-cells`, never the other way
//! round, so the wire types of the cell are not nameable here. That is a
//! feature rather than a workaround: a helper that parses the JSON generically
//! fails a test when the cell stops sending a field, instead of failing to
//! compile alongside it — and the same helper drives the manual smoke script's
//! Rust twin without dragging the cell crate into it.
//!
//! # Why the handshake is `client_async` over a `TcpStream`
//!
//! `connect_async` lives behind tokio-tungstenite's `connect` feature, which
//! this crate does not enable (it would pull a TLS stack into a test crate that
//! only ever talks to `127.0.0.1`). `client_async` is part of `handshake`,
//! which is already on, and it takes a stream we opened ourselves — which is
//! also where `TCP_NODELAY` gets set, without which every measured round trip
//! carries Nagle's delay instead of the cell's.

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use meclaw_core::serde_json::{Value as JsonValue, json};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// How long `connect` waits for the `hello` frame before giving up.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// One frame as it arrived from the cell.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// A text frame, parsed as JSON. A text frame that is not valid JSON
    /// arrives as `JsonValue::String` of the raw text, so a malformed-output
    /// test can assert on it instead of losing it.
    Text(JsonValue),
    /// A binary frame: raw audio in the format `hello.audio_out` declared.
    Audio(Vec<u8>),
    /// The peer closed. Carries the close code, `1005` when none was sent.
    Close(u16),
}

impl Frame {
    /// The `type` field of a text frame, if this is one.
    pub fn frame_type(&self) -> Option<&str> {
        match self {
            Frame::Text(v) => v.get("type").and_then(JsonValue::as_str),
            _ => None,
        }
    }

    /// True when this is a text frame with exactly this `type`.
    pub fn is_type(&self, ty: &str) -> bool {
        self.frame_type() == Some(ty)
    }

    /// The JSON of a text frame, if this is one.
    pub fn as_text(&self) -> Option<&JsonValue> {
        match self {
            Frame::Text(v) => Some(v),
            _ => None,
        }
    }

    /// The bytes of a binary frame, if this is one.
    pub fn as_audio(&self) -> Option<&[u8]> {
        match self {
            Frame::Audio(b) => Some(b),
            _ => None,
        }
    }

    /// The close code, if this is a close.
    pub fn as_close(&self) -> Option<u16> {
        match self {
            Frame::Close(c) => Some(*c),
            _ => None,
        }
    }
}

/// Raw PCM read from a WAV file, with the format it was stored in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavPcm {
    /// The sample data, exactly as it sits in the `data` chunk.
    pub pcm: Vec<u8>,
    /// Samples per second.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
}

impl WavPcm {
    /// How long this audio plays.
    pub fn duration(&self) -> Duration {
        let bytes_per_second = self.sample_rate as u64 * 2 * self.channels as u64;
        if bytes_per_second == 0 {
            return Duration::ZERO;
        }
        Duration::from_secs_f64(self.pcm.len() as f64 / bytes_per_second as f64)
    }
}

/// Read a 16-bit PCM WAV file.
///
/// Deliberately a hand-rolled RIFF walk rather than a crate: the allow-list is
/// closed, and the only files this ever sees are the 16 kHz mono fixtures the
/// stack has used since July.
///
/// # Errors
///
/// When the file is unreadable, is not RIFF/WAVE, has no `fmt `/`data` chunk,
/// or is not 16-bit PCM.
pub fn load_wav_pcm16(path: &Path) -> Result<WavPcm, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format!("{}: not a RIFF/WAVE file", path.display()));
    }
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32)> = None;
    let mut data: Option<Vec<u8>> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(size).min(bytes.len());
        if id == b"fmt " && body_end - body_start >= 16 {
            let b = &bytes[body_start..body_end];
            let format_tag = u16::from_le_bytes([b[0], b[1]]);
            let channels = u16::from_le_bytes([b[2], b[3]]);
            let rate = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
            let bits = u16::from_le_bytes([b[14], b[15]]);
            if format_tag != 1 || bits != 16 {
                return Err(format!(
                    "{}: need 16-bit PCM, got format {format_tag} / {bits} bit",
                    path.display()
                ));
            }
            fmt = Some((channels, bits, rate));
        } else if id == b"data" {
            data = Some(bytes[body_start..body_end].to_vec());
        }
        // RIFF chunks are word-aligned: an odd size is followed by a pad byte.
        pos = body_start + size + (size % 2);
    }
    let (channels, _bits, sample_rate) =
        fmt.ok_or_else(|| format!("{}: no fmt chunk", path.display()))?;
    let pcm = data.ok_or_else(|| format!("{}: no data chunk", path.display()))?;
    Ok(WavPcm {
        pcm,
        sample_rate,
        channels,
    })
}

/// A client of one `voice` connection.
///
/// Reading runs in its own task from the moment `connect` returns, so the
/// instant attached to a frame is when it *arrived*, not when the test got
/// round to asking — which is the whole point when the next assertion is about
/// a round trip. Sends go straight to the socket.
pub struct VoiceClient {
    write: SplitSink<WebSocketStream<TcpStream>, WsMessage>,
    frames: mpsc::UnboundedReceiver<(Instant, Frame)>,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for VoiceClient {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl VoiceClient {
    /// Connect to `url` (`ws://host:port/path?query`) and read the `hello`
    /// frame the cell always sends first.
    ///
    /// # Errors
    ///
    /// When the URL is not a plain `ws://` URL, the TCP connect or the
    /// WebSocket handshake fails, or the first frame is not a `hello` within
    /// ten seconds.
    pub async fn connect(url: &str) -> Result<(Self, JsonValue), String> {
        let authority = ws_authority(url)?;
        let tcp = TcpStream::connect(&authority)
            .await
            .map_err(|e| format!("connect {authority}: {e}"))?;
        tcp.set_nodelay(true)
            .map_err(|e| format!("set_nodelay: {e}"))?;
        let (ws, _resp) = tokio_tungstenite::client_async(url, tcp)
            .await
            .map_err(|e| format!("websocket handshake {url}: {e}"))?;
        let (write, mut read) = ws.split();
        let (tx, frames) = mpsc::unbounded_channel();
        let reader = tokio::spawn(async move {
            while let Some(item) = read.next().await {
                let frame = match item {
                    Ok(WsMessage::Text(t)) => Frame::Text(
                        meclaw_core::serde_json::from_str(&t)
                            .unwrap_or_else(|_| JsonValue::String(t.to_string())),
                    ),
                    Ok(WsMessage::Binary(b)) => Frame::Audio(b.to_vec()),
                    Ok(WsMessage::Close(c)) => {
                        let code = c.map(|f| u16::from(f.code)).unwrap_or(1005);
                        let _ = tx.send((Instant::now(), Frame::Close(code)));
                        break;
                    }
                    Ok(_) => continue,
                    Err(_) => break,
                };
                if tx.send((Instant::now(), frame)).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            write,
            frames,
            reader,
        };
        let (_, first) = client.next_frame_at(HELLO_TIMEOUT).await?;
        match first {
            Frame::Text(v) if v.get("type").and_then(JsonValue::as_str) == Some("hello") => {
                Ok((client, v))
            }
            other => Err(format!("first frame was not a hello: {other:?}")),
        }
    }

    /// Send one binary frame of raw audio.
    ///
    /// # Errors
    ///
    /// When the socket refuses the write.
    pub async fn send_audio(&mut self, pcm: &[u8]) -> Result<(), String> {
        self.write
            .send(WsMessage::Binary(pcm.to_vec().into()))
            .await
            .map_err(|e| format!("send audio: {e}"))
    }

    /// Send one text frame. The value is a client frame of `meclaw-voice/1`,
    /// e.g. `json!({"type": "hold"})`.
    ///
    /// # Errors
    ///
    /// When the socket refuses the write.
    pub async fn send(&mut self, frame: JsonValue) -> Result<(), String> {
        self.write
            .send(WsMessage::Text(frame.to_string().into()))
            .await
            .map_err(|e| format!("send frame: {e}"))
    }

    /// `{"type":"hold"}`.
    ///
    /// # Errors
    ///
    /// When the socket refuses the write.
    pub async fn hold(&mut self) -> Result<(), String> {
        self.send(json!({"type": "hold"})).await
    }

    /// `{"type":"release"}`.
    ///
    /// # Errors
    ///
    /// When the socket refuses the write.
    pub async fn release(&mut self) -> Result<(), String> {
        self.send(json!({"type": "release"})).await
    }

    /// `{"type":"cancel"}`.
    ///
    /// # Errors
    ///
    /// When the socket refuses the write.
    pub async fn cancel(&mut self) -> Result<(), String> {
        self.send(json!({"type": "cancel"})).await
    }

    /// `{"type":"mode","mode":…}`.
    ///
    /// # Errors
    ///
    /// When the socket refuses the write.
    pub async fn set_mode(&mut self, mode: &str) -> Result<(), String> {
        self.send(json!({"type": "mode", "mode": mode})).await
    }

    /// Stream a 16-bit PCM WAV in real time, `chunk_ms` of audio per binary
    /// frame, sleeping between chunks so the timings mean something (the
    /// measuring rule the July stack settled on: a file pushed as fast as the
    /// socket takes it measures the socket, not the provider).
    ///
    /// Returns the instant the last chunk was handed to the socket — the
    /// "last sent byte" every end-of-turn latency is counted from.
    ///
    /// The WAV's own rate is returned in [`WavPcm`] by [`load_wav_pcm16`]; this
    /// helper never resamples (R-V2), it only refuses to guess: pass a file
    /// whose rate matches `hello.audio_in`.
    ///
    /// # Errors
    ///
    /// When the file cannot be read as 16-bit PCM, is not mono, or a send
    /// fails.
    pub async fn stream_wav_realtime(
        &mut self,
        path: &Path,
        chunk_ms: u64,
    ) -> Result<Instant, String> {
        let wav = load_wav_pcm16(path)?;
        if wav.channels != 1 {
            return Err(format!(
                "{}: need mono, got {} channels",
                path.display(),
                wav.channels
            ));
        }
        self.stream_pcm_realtime(&wav.pcm, wav.sample_rate, chunk_ms)
            .await
    }

    /// The same as [`Self::stream_wav_realtime`] for PCM already in memory.
    ///
    /// # Errors
    ///
    /// When `sample_rate` is zero or a send fails.
    pub async fn stream_pcm_realtime(
        &mut self,
        pcm: &[u8],
        sample_rate: u32,
        chunk_ms: u64,
    ) -> Result<Instant, String> {
        if sample_rate == 0 {
            return Err("sample_rate must not be zero".to_string());
        }
        let chunk_ms = chunk_ms.max(1);
        let bytes_per_chunk = (sample_rate as usize * 2 * chunk_ms as usize / 1000).max(2);
        let started = Instant::now();
        let mut last = started;
        for (i, chunk) in pcm.chunks(bytes_per_chunk).enumerate() {
            // Pace against the stream's own start, not against the previous
            // sleep: per-chunk sleeps accumulate their own overshoot, and over
            // a ten-second file that drift is larger than what is measured.
            let due = started + Duration::from_millis(chunk_ms * i as u64);
            let now = Instant::now();
            if due > now {
                tokio::time::sleep(due - now).await;
            }
            self.send_audio(chunk).await?;
            last = Instant::now();
        }
        Ok(last)
    }

    /// The next frame, or an error when nothing arrives within `timeout`.
    ///
    /// # Errors
    ///
    /// On timeout, or when the reader task is gone (socket closed and drained).
    pub async fn next_frame(&mut self, timeout: Duration) -> Result<Frame, String> {
        self.next_frame_at(timeout).await.map(|(_, f)| f)
    }

    /// The next frame together with the instant it arrived.
    ///
    /// # Errors
    ///
    /// On timeout, or when the reader task is gone.
    pub async fn next_frame_at(&mut self, timeout: Duration) -> Result<(Instant, Frame), String> {
        match tokio::time::timeout(timeout, self.frames.recv()).await {
            Ok(Some(f)) => Ok(f),
            Ok(None) => Err("connection closed with no further frames".to_string()),
            Err(_) => Err(format!("no frame within {timeout:?}")),
        }
    }

    /// The next text frame with this `type`, skipping everything else.
    ///
    /// # Errors
    ///
    /// When `timeout` passes before such a frame arrives.
    pub async fn next_frame_of_type(
        &mut self,
        ty: &str,
        timeout: Duration,
    ) -> Result<JsonValue, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("no {ty} frame within {timeout:?}"));
            }
            let frame = self.next_frame(left).await?;
            if let Frame::Text(v) = &frame
                && v.get("type").and_then(JsonValue::as_str) == Some(ty)
            {
                return Ok(v.clone());
            }
        }
    }

    /// Collect frames until `pred` accepts one (that frame is included) or
    /// `timeout` passes. Returns what arrived either way — a test asserts on
    /// the collection, so a timeout is data, not a panic.
    pub async fn collect_until(
        &mut self,
        pred: impl Fn(&Frame) -> bool,
        timeout: Duration,
    ) -> Vec<(Instant, Frame)> {
        let deadline = Instant::now() + timeout;
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return out;
            }
            match self.next_frame_at(left).await {
                Ok((at, frame)) => {
                    let done = pred(&frame);
                    out.push((at, frame));
                    if done {
                        return out;
                    }
                }
                Err(_) => return out,
            }
        }
    }

    /// Everything that arrives within `timeout`, with no stop condition.
    pub async fn drain_for(&mut self, timeout: Duration) -> Vec<(Instant, Frame)> {
        self.collect_until(|_| false, timeout).await
    }

    /// Close the connection from this side.
    ///
    /// # Errors
    ///
    /// When the close frame cannot be written.
    pub async fn close(mut self) -> Result<(), String> {
        self.write.close().await.map_err(|e| format!("close: {e}"))
    }
}

/// `ws://host:port/path` → `host:port`. Only plain `ws://` — this talks to a
/// loopback cell, and a TLS stack in a test crate would be a dependency nobody
/// asked for.
fn ws_authority(url: &str) -> Result<String, String> {
    let rest = url
        .strip_prefix("ws://")
        .ok_or_else(|| format!("{url}: only ws:// urls are supported"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err(format!("{url}: no host"));
    }
    if authority.contains(':') {
        Ok(authority.to_string())
    } else {
        Ok(format!("{authority}:80"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::free_port;
    use tokio::net::TcpListener;

    /// A one-connection WebSocket server that sends `hello`, then whatever the
    /// script says, and records what the client sent.
    async fn mini_ws_server(
        port: u16,
        hello: JsonValue,
        echo_binary: bool,
    ) -> tokio::sync::oneshot::Receiver<Vec<WsMessage>> {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("mini ws server binds");
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                return;
            };
            let (mut write, mut read) = ws.split();
            if write
                .send(WsMessage::Text(hello.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
            let mut seen = Vec::new();
            while let Some(Ok(msg)) = read.next().await {
                match &msg {
                    WsMessage::Binary(b) if echo_binary => {
                        if write.send(WsMessage::Binary(b.clone())).await.is_err() {
                            break;
                        }
                    }
                    WsMessage::Close(_) => {
                        seen.push(msg);
                        break;
                    }
                    _ => {}
                }
                if !matches!(msg, WsMessage::Close(_)) {
                    seen.push(msg);
                }
            }
            let _ = done_tx.send(seen);
        });
        done_rx
    }

    fn hello_json() -> JsonValue {
        json!({
            "type": "hello",
            "protocol": "meclaw-voice/1",
            "session_id": "s1",
            "mode": "auto",
            "audio_in": {"encoding": "pcm_s16le", "sample_rate": 16000, "channels": 1},
            "audio_out": null,
            "stt": "echo",
            "tts": null
        })
    }

    #[tokio::test]
    async fn voice_client_parses_hello() {
        let port = free_port();
        let _srv = mini_ws_server(port, hello_json(), false).await;
        let (client, hello) = VoiceClient::connect(&format!("ws://127.0.0.1:{port}/ws"))
            .await
            .expect("connect");
        assert_eq!(hello["protocol"], "meclaw-voice/1");
        assert_eq!(hello["session_id"], "s1");
        assert_eq!(hello["audio_in"]["sample_rate"], 16000);
        drop(client);
    }

    #[tokio::test]
    async fn voice_client_round_trips_binary_audio() {
        let port = free_port();
        let _srv = mini_ws_server(port, hello_json(), true).await;
        let (mut client, _) = VoiceClient::connect(&format!("ws://127.0.0.1:{port}/ws"))
            .await
            .expect("connect");
        let payload = vec![7u8; 640];
        client.send_audio(&payload).await.expect("send audio");
        let back = client
            .next_frame(Duration::from_secs(5))
            .await
            .expect("audio comes back");
        assert_eq!(back.as_audio(), Some(payload.as_slice()));
    }

    #[tokio::test]
    async fn voice_client_reports_control_frames_to_the_server() {
        let port = free_port();
        let seen = mini_ws_server(port, hello_json(), false).await;
        let (mut client, _) = VoiceClient::connect(&format!("ws://127.0.0.1:{port}/ws"))
            .await
            .expect("connect");
        client.hold().await.expect("hold");
        client.release().await.expect("release");
        client.cancel().await.expect("cancel");
        client.set_mode("hold").await.expect("mode");
        client.close().await.expect("close");
        let msgs = tokio::time::timeout(Duration::from_secs(5), seen)
            .await
            .expect("server finishes")
            .expect("server reports");
        // Compared as JSON, not as text: key order is serde_json's business,
        // and an assertion that depends on it breaks for the wrong reason.
        let texts: Vec<JsonValue> = msgs
            .iter()
            .filter_map(|m| match m {
                WsMessage::Text(t) => meclaw_core::serde_json::from_str(t).ok(),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                json!({"type": "hold"}),
                json!({"type": "release"}),
                json!({"type": "cancel"}),
                json!({"type": "mode", "mode": "hold"}),
            ]
        );
    }

    #[tokio::test]
    async fn next_frame_times_out_instead_of_hanging() {
        let port = free_port();
        let _srv = mini_ws_server(port, hello_json(), false).await;
        let (mut client, _) = VoiceClient::connect(&format!("ws://127.0.0.1:{port}/ws"))
            .await
            .expect("connect");
        let err = client
            .next_frame(Duration::from_millis(50))
            .await
            .expect_err("nothing is sent, so this must time out");
        assert!(err.contains("no frame within"), "{err}");
    }

    #[tokio::test]
    async fn collect_until_stops_on_the_predicate() {
        let port = free_port();
        let _srv = mini_ws_server(port, hello_json(), true).await;
        let (mut client, _) = VoiceClient::connect(&format!("ws://127.0.0.1:{port}/ws"))
            .await
            .expect("connect");
        for i in 0..4u8 {
            client.send_audio(&[i; 4]).await.expect("send");
        }
        let got = client
            .collect_until(
                |f| f.as_audio().map(|b| b[0] == 2).unwrap_or(false),
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(got.len(), 3, "stops on the third frame, inclusive");
        assert_eq!(got[2].1.as_audio(), Some([2u8; 4].as_slice()));
    }

    #[tokio::test]
    async fn streaming_pcm_takes_about_as_long_as_the_audio_lasts() {
        let port = free_port();
        let _srv = mini_ws_server(port, hello_json(), true).await;
        let (mut client, _) = VoiceClient::connect(&format!("ws://127.0.0.1:{port}/ws"))
            .await
            .expect("connect");
        // 300 ms of 16 kHz mono PCM16, sent in 20 ms chunks.
        let pcm = vec![0u8; 16_000 * 2 * 300 / 1000];
        let started = Instant::now();
        let last = client
            .stream_pcm_realtime(&pcm, 16_000, 20)
            .await
            .expect("stream");
        let elapsed = last.duration_since(started);
        assert!(
            elapsed >= Duration::from_millis(260),
            "real time means it does not race ahead: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_millis(900),
            "and it does not crawl: {elapsed:?}"
        );
        let frames = client.drain_for(Duration::from_millis(200)).await;
        assert_eq!(frames.len(), 15, "300 ms in 20 ms chunks is 15 frames");
    }

    #[test]
    fn load_wav_pcm16_reads_a_mono_16k_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tone.wav");
        let samples: Vec<u8> = (0..1600u16).flat_map(|i| i.to_le_bytes()).collect();
        std::fs::write(&path, wav_bytes(&samples, 16_000, 1)).expect("write wav");
        let wav = load_wav_pcm16(&path).expect("read wav");
        assert_eq!(wav.sample_rate, 16_000);
        assert_eq!(wav.channels, 1);
        assert_eq!(wav.pcm, samples);
        assert_eq!(wav.duration(), Duration::from_millis(100));
    }

    #[test]
    fn load_wav_pcm16_refuses_a_file_that_is_not_riff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nope.wav");
        std::fs::write(&path, b"not a wav at all").expect("write");
        let err = load_wav_pcm16(&path).expect_err("must refuse");
        assert!(err.contains("not a RIFF/WAVE file"), "{err}");
    }

    #[test]
    fn ws_authority_defaults_the_port_and_refuses_wss() {
        assert_eq!(
            ws_authority("ws://127.0.0.1:7900/ws?session=a").expect("parses"),
            "127.0.0.1:7900"
        );
        assert_eq!(
            ws_authority("ws://example/ws").expect("parses"),
            "example:80"
        );
        assert!(ws_authority("wss://example/ws").is_err());
    }

    /// A minimal canonical WAV header around `samples`.
    fn wav_bytes(samples: &[u8], rate: u32, channels: u16) -> Vec<u8> {
        let block_align = channels * 2;
        let byte_rate = rate * u32::from(block_align);
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((36 + samples.len()) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        out.extend_from_slice(samples);
        out
    }
}
