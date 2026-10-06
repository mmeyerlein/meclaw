//! The server side of every WebSocket route on the colony's listener, with
//! permessage-deflate (RFC 7692; GH #1024).
//!
//! Two routes take a socket: the `web` cell's `/<mount>/live/websocket` (the
//! display, the village, every LiveView page) and the voice cell's `/ws` (the
//! phone line). Both used `axum::extract::ws`, which sits on tungstenite — and
//! no tungstenite release implements permessage-deflate, while every browser
//! offers it. The join tree of a large page went over the wire as plain JSON:
//! ~755 KB for the lab page of the web-cell hardening wave, 17.8 s on Slow 3G.
//!
//! This module keeps the shape those two routes were written against — an
//! extractor with `on_upgrade`, a socket that is a `Stream` and a `Sink` of
//! [`Message`] — and answers an offer with the extension. What goes over the
//! wire compressed is decided per message: a frame of at least
//! [`DEFLATE_MIN_BYTES`] is deflated, anything smaller goes as it is, because a
//! heartbeat, a click's diff or 20 ms of audio gains nothing and would pay the
//! compressor's time on every frame. A client without the offer gets exactly
//! the frames it got before.

mod codec;
mod negotiate;

use axum::extract::{FromRequestParts, OptionalFromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use flate2::Compression;
use futures_util::{Sink, Stream};
use hyper_util::rt::TokioIo;
use std::borrow::Cow;
use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

pub use codec::MAX_INFLATED;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::Framed;

/// Messages of at least this many bytes are compressed when the client agreed
/// to permessage-deflate (4 KiB).
///
/// Below it the frame goes as it is: heartbeats, replies, the diff of one
/// click and 20 ms of audio are 40-500 bytes and gain nothing worth a
/// compressor's time. The plan's default was 16 KiB; measured on the village
/// (45 frames from page load to connected + 1.5 s, 738 KB), the join arrives
/// as 7 chunks of 65-98 KB AND 15 tranches of 5.9-10 KB. At 16 KiB the
/// tranches went plain and the join took 32.7 % of its size on the wire; at
/// 4 KiB 15.1 % (level 6). No frame of that load lies between 0.5 and 5.9 KB,
/// so 4 KiB and 1 KiB select the same frames (receipt of the WS compression
/// wave, OR-WS-1).
pub const DEFLATE_MIN_BYTES: usize = 4 * 1024;

/// The deflate level: zlib's default (6).
///
/// Level 1 left the village join at 20.4 %, level 6 at 15.1 % — the
/// difference between missing and meeting the first-picture budget — for
/// 0.75 vs 1.6 ms per 98 KB chunk, which the socket spends in 16 KiB steps
/// ([`codec::STEP`]) so no step holds a worker thread for a millisecond.
const DEFLATE_LEVEL: u32 = 6;

/// One WebSocket message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close(Option<CloseFrame>),
}

/// The code and reason of a close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseFrame {
    pub code: u16,
    pub reason: Cow<'static, str>,
}

/// The I/O under a socket the listener handed over.
pub type Upgraded = TokioIo<hyper::upgrade::Upgraded>;

/// A server-side WebSocket: a [`Stream`] of what the client sends and a
/// [`Sink`] for what it is sent.
///
/// Pings are answered on their own and a client's close is echoed, as
/// `axum::extract::ws::WebSocket` did; both are still yielded, so a reader that
/// wants to see them can.
pub struct WebSocket<T = Upgraded> {
    inner: Framed<T, codec::Codec>,
    /// A pong or a close (echo or failure), waiting for room on the write side.
    owed: Option<Message>,
    read_closed: bool,
    /// A close went out or is owed: never a second one, and no data after it
    /// (RFC 6455 § 5.5.1; review M-2).
    close_sent: bool,
    /// The compressor, once permessage-deflate was agreed.
    deflate: Option<codec::Deflate>,
    /// The large message being compressed, step by step.
    job: Option<codec::Job>,
    /// The compressed client message being inflated, step by step.
    inflating: Option<codec::Inflate>,
}

impl<T: AsyncRead + AsyncWrite + Unpin> WebSocket<T> {
    /// Wrap an upgraded connection; `deflate` says whether the handshake agreed
    /// on permessage-deflate.
    pub fn from_upgraded(io: T, deflate: bool) -> Self {
        Self::with_max_inflated(io, deflate, MAX_INFLATED)
    }

    /// [`Self::from_upgraded`], with the most a compressed client message may
    /// inflate to; past it the socket closes with 1009.
    pub fn with_max_inflated(io: T, deflate: bool, max_inflated: usize) -> Self {
        Self {
            inner: Framed::new(io, codec::Codec::new(deflate.then_some(max_inflated))),
            owed: None,
            read_closed: false,
            close_sent: false,
            deflate: deflate
                .then(|| codec::Deflate::new(DEFLATE_MIN_BYTES, Compression::new(DEFLATE_LEVEL))),
            job: None,
            inflating: None,
        }
    }

    /// Owe the client a close, unless one went out already.
    fn owe_close(&mut self, frame: Option<CloseFrame>) {
        if !self.close_sent {
            self.close_sent = true;
            self.owed = Some(Message::Close(frame));
        }
    }

    /// The read side failed: close with the code the error carries (1002,
    /// 1007, 1009) so the browser sees why, not a bare 1006 (review M-1).
    fn fail(&mut self, e: io::Error, cx: &mut Context<'_>) -> Poll<Option<io::Result<Message>>> {
        self.read_closed = true;
        self.inflating = None;
        if let Some(code) = codec::close_code_of(&e) {
            self.owe_close(Some(CloseFrame {
                code,
                reason: Cow::Borrowed(""),
            }));
            let _ = self.pay(cx);
        }
        Poll::Ready(Some(Err(e)))
    }

    /// Finish compressing the message in hand and give it to the codec. Each
    /// poll does one step and yields (wakes itself) until the message is done.
    fn drive(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let Some(job) = self.job.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        let deflate = self
            .deflate
            .as_mut()
            .ok_or_else(|| io::Error::other("websocket: a job without a compressor"))?;
        if !deflate.step(job)? {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        ready!(Pin::new(&mut self.inner).poll_ready(cx))?;
        let job = self.job.take().expect("checked above");
        Pin::new(&mut self.inner).start_send(job.into_out())?;
        Poll::Ready(Ok(()))
    }

    /// Write what is owed to the client (a pong, a close echo) if there is
    /// room. Never waits: the next poll of either half tries again.
    fn pay(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.owed.is_some() {
            ready!(Pin::new(&mut self.inner).poll_ready(cx))?;
            let msg = self.owed.take().expect("checked above");
            Pin::new(&mut self.inner).start_send(codec::Out::Msg(msg))?;
            // Start writing it; a pending flush finishes on the next poll.
            let _ = Pin::new(&mut self.inner).poll_flush(cx)?;
        }
        Poll::Ready(Ok(()))
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> Stream for WebSocket<T> {
    type Item = io::Result<Message>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.read_closed {
            return Poll::Ready(None);
        }
        // A write error surfaces on the write half; reading goes on.
        let _ = this.pay(cx);
        loop {
            // A compressed message in hand: one inflate step per poll, then
            // yield (AGENTS.md rule 13), as the write side does with deflate.
            if let Some(job) = this.inflating.as_mut() {
                let step = match this.deflate.as_mut() {
                    Some(d) => d.inflate_step(job),
                    None => Err(io::Error::other("websocket: inflate without a compressor")),
                };
                return match step {
                    Ok(false) => {
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                    Ok(true) => match this.inflating.take().map(codec::Inflate::into_message) {
                        Some(Ok(m)) => Poll::Ready(Some(Ok(m))),
                        Some(Err(e)) => this.fail(e, cx),
                        None => this.fail(io::Error::other("websocket: inflate job lost"), cx),
                    },
                    Err(e) => this.fail(e, cx),
                };
            }
            return match ready!(Pin::new(&mut this.inner).poll_next(cx)) {
                Some(Ok(codec::In::Deflated(job))) => {
                    this.inflating = Some(job);
                    continue;
                }
                Some(Ok(codec::In::Msg(Message::Ping(p)))) => {
                    if !this.close_sent {
                        this.owed = Some(Message::Pong(p.clone()));
                        let _ = this.pay(cx);
                    }
                    Poll::Ready(Some(Ok(Message::Ping(p))))
                }
                Some(Ok(codec::In::Msg(Message::Close(frame)))) => {
                    this.read_closed = true;
                    // The echo carries the code alone (RFC 6455 § 5.5.1),
                    // and only when this side has not closed first.
                    this.owe_close(frame.as_ref().map(|f| CloseFrame {
                        code: f.code,
                        reason: Cow::Borrowed(""),
                    }));
                    let _ = this.pay(cx);
                    Poll::Ready(Some(Ok(Message::Close(frame))))
                }
                Some(Ok(codec::In::Msg(m))) => Poll::Ready(Some(Ok(m))),
                Some(Err(e)) => this.fail(e, cx),
                None => {
                    this.read_closed = true;
                    Poll::Ready(None)
                }
            };
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> Sink<Message> for WebSocket<T> {
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.pay(cx))?;
        ready!(this.drive(cx))?;
        Pin::new(&mut this.inner).poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Message) -> io::Result<()> {
        let this = self.get_mut();
        if this.close_sent {
            return Err(io::Error::other("websocket: send after close"));
        }
        let len = match &item {
            Message::Text(s) => s.len(),
            Message::Binary(b) => b.len(),
            Message::Close(_) => {
                this.close_sent = true;
                0
            }
            Message::Ping(_) | Message::Pong(_) => 0,
        };
        // Only a large data message becomes a job; everything else goes as
        // it is, without a round trip through bytes (review M-6).
        if this.deflate.as_ref().is_some_and(|d| len >= d.min_bytes) {
            match codec::Job::for_message(item) {
                Ok(job) => {
                    this.job = Some(job);
                    return Ok(());
                }
                Err(other) => return Pin::new(&mut this.inner).start_send(codec::Out::Msg(other)),
            }
        }
        Pin::new(&mut this.inner).start_send(codec::Out::Msg(item))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.pay(cx))?;
        ready!(this.drive(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.pay(cx))?;
        ready!(this.drive(cx))?;
        Pin::new(&mut this.inner).poll_close(cx)
    }
}

/// The upgrade request of a WebSocket route, as an extractor.
///
/// `Option<WebSocketUpgrade>` is `None` for a request that is not an upgrade,
/// which is how both routes answer a plain `GET` with a `400`.
pub struct WebSocketUpgrade {
    on_upgrade: hyper::upgrade::OnUpgrade,
    accept: HeaderValue,
    /// The `Sec-WebSocket-Extensions` answer, once an offer was accepted.
    answer: Option<&'static str>,
    max_inflated: usize,
}

fn header_has_token(parts: &Parts, name: header::HeaderName, token: &str) -> bool {
    parts.headers.get_all(name).iter().any(|v| {
        v.to_str()
            .is_ok_and(|s| s.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
    })
}

impl WebSocketUpgrade {
    fn from_parts(parts: &mut Parts) -> Option<Self> {
        if parts.method != Method::GET
            || !header_has_token(parts, header::CONNECTION, "upgrade")
            || !header_has_token(parts, header::UPGRADE, "websocket")
            || parts
                .headers
                .get(header::SEC_WEBSOCKET_VERSION)
                .is_none_or(|v| v.as_bytes() != b"13")
        {
            return None;
        }
        let key = parts.headers.get(header::SEC_WEBSOCKET_KEY)?;
        let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
        let accept = HeaderValue::from_str(&accept).ok()?;
        let answer = negotiate::answer(
            parts
                .headers
                .get_all(header::SEC_WEBSOCKET_EXTENSIONS)
                .iter()
                .filter_map(|v| v.to_str().ok()),
        );
        let on_upgrade = parts.extensions.remove::<hyper::upgrade::OnUpgrade>()?;
        Some(Self {
            on_upgrade,
            accept,
            answer,
            max_inflated: MAX_INFLATED,
        })
    }

    /// Whether the handshake agreed on permessage-deflate.
    pub fn deflate(&self) -> bool {
        self.answer.is_some()
    }

    /// The most a compressed client message may inflate to (default
    /// [`MAX_INFLATED`]); past it the socket closes with 1009.
    pub fn max_inflated(mut self, bytes: usize) -> Self {
        self.max_inflated = bytes;
        self
    }

    /// Answer the upgrade and run `callback` on the socket once the connection
    /// is handed over — on a task of its own, as axum's `on_upgrade` did.
    pub fn on_upgrade<C, Fut>(self, callback: C) -> Response
    where
        C: FnOnce(WebSocket) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let Self {
            on_upgrade,
            accept,
            answer,
            max_inflated,
        } = self;
        let deflate = answer.is_some();
        tokio::spawn(async move {
            let Ok(upgraded) = on_upgrade.await else {
                tracing::debug!("websocket: the connection was not handed over");
                return;
            };
            let io = TokioIo::new(upgraded);
            callback(WebSocket::with_max_inflated(io, deflate, max_inflated)).await;
        });
        let mut response = StatusCode::SWITCHING_PROTOCOLS.into_response();
        let h = response.headers_mut();
        h.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        h.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        h.insert(header::SEC_WEBSOCKET_ACCEPT, accept);
        if let Some(answer) = answer {
            h.insert(
                header::SEC_WEBSOCKET_EXTENSIONS,
                HeaderValue::from_static(answer),
            );
        }
        response
    }
}

impl<S: Send + Sync> FromRequestParts<S> for WebSocketUpgrade {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Response> {
        Self::from_parts(parts).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "this path is a websocket endpoint\n",
            )
                .into_response()
        })
    }
}

impl<S: Send + Sync> OptionalFromRequestParts<S> for WebSocketUpgrade {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Option<Self>, Infallible> {
        Ok(Self::from_parts(parts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn masked(b0: u8, payload: &[u8]) -> Vec<u8> {
        let mut f = vec![b0];
        match payload.len() {
            n if n < 126 => f.push(0x80 | n as u8),
            n if n <= u16::MAX as usize => {
                f.push(0x80 | 126);
                f.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                f.push(0x80 | 127);
                f.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        let mask = [9u8, 8, 7, 6];
        f.extend_from_slice(&mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        f
    }

    /// What a browser sends for `data`: one deflate stream, sync-flushed, the
    /// tail stripped (RFC 7692 § 7.2.1).
    fn deflated(data: &[u8]) -> Vec<u8> {
        let mut c = flate2::Compress::new(Compression::best(), false);
        sync_deflate(&mut c, data)
    }

    /// The next message on a compressor that keeps its context.
    fn sync_deflate(c: &mut flate2::Compress, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + 1024);
        let base = c.total_in() as usize;
        loop {
            let at = c.total_in() as usize - base;
            c.compress_vec(&data[at..], &mut out, flate2::FlushCompress::Sync)
                .unwrap();
            if c.total_in() as usize - base == data.len() && out.len() < out.capacity() {
                break;
            }
            out.reserve(out.capacity());
        }
        assert!(out.ends_with(&[0, 0, 0xff, 0xff]));
        out.truncate(out.len() - 4);
        out
    }

    /// The close the server wrote, as its code; `None` when nothing came.
    async fn close_code(client: &mut tokio::io::DuplexStream) -> Option<u16> {
        let read = async {
            let mut head = [0u8; 2];
            client.read_exact(&mut head).await.ok()?;
            assert_eq!(head[0], 0x88, "a close frame");
            let mut body = vec![0u8; head[1] as usize];
            client.read_exact(&mut body).await.ok()?;
            (body.len() >= 2).then(|| u16::from_be_bytes([body[0], body[1]]))
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), read)
            .await
            .ok()
            .flatten()
    }

    /// Send `frames`, read until the socket fails, return the close code.
    async fn refused_with(deflate: bool, frames: &[Vec<u8>]) -> Option<u16> {
        let (server_io, mut client) = tokio::io::duplex(1 << 20);
        let mut ws = WebSocket::from_upgraded(server_io, deflate);
        for f in frames {
            client.write_all(f).await.unwrap();
        }
        loop {
            match ws.next().await {
                Some(Ok(_)) => continue,
                Some(Err(_)) => break,
                None => panic!("the stream ended without an error"),
            }
        }
        close_code(&mut client).await
    }

    #[tokio::test]
    async fn an_inflate_bomb_closes_with_1009() {
        // 2 MiB of zeros deflate to ~2 KB: past the 1 MiB client limit.
        let bomb = deflated(&vec![0u8; 2 << 20]);
        assert!(bomb.len() < 8 * 1024);
        assert_eq!(refused_with(true, &[masked(0xC2, &bomb)]).await, Some(1009));
    }

    #[tokio::test]
    async fn a_bomb_over_fragments_closes_with_1009() {
        let bomb = deflated(&vec![0u8; 4 << 20]);
        let third = bomb.len() / 3;
        let frames = [
            masked(0x42, &bomb[..third]),
            masked(0x00, &bomb[third..2 * third]),
            masked(0x80, &bomb[2 * third..]),
        ];
        assert_eq!(refused_with(true, &frames).await, Some(1009));
    }

    #[tokio::test]
    async fn a_compressed_message_over_fragments_joins() {
        let text = "{\"event\":\"click\",\"id\":\"fig-1\"}".repeat(300);
        let wire = deflated(text.as_bytes());
        let half = wire.len() / 2;
        let (server_io, mut client) = tokio::io::duplex(1 << 20);
        let mut ws = WebSocket::from_upgraded(server_io, true);
        client
            .write_all(&masked(0x41, &wire[..half]))
            .await
            .unwrap();
        client.write_all(&masked(0x89, b"p")).await.unwrap();
        client
            .write_all(&masked(0x80, &wire[half..]))
            .await
            .unwrap();
        assert_eq!(
            ws.next().await.unwrap().unwrap(),
            Message::Ping(b"p".to_vec())
        );
        assert_eq!(ws.next().await.unwrap().unwrap(), Message::Text(text));
    }

    #[tokio::test]
    async fn rsv1_on_a_continuation_closes_with_1002() {
        let wire = deflated(b"hello hello hello");
        let frames = [masked(0x41, &wire[..2]), masked(0xC0, &wire[2..])];
        assert_eq!(refused_with(true, &frames).await, Some(1002));
    }

    #[tokio::test]
    async fn two_compressed_messages_read_on_one_socket() {
        let (server_io, mut client) = tokio::io::duplex(1 << 20);
        let mut ws = WebSocket::from_upgraded(server_io, true);
        let a = "a".repeat(10_000);
        let b = "b".repeat(20_000);
        client
            .write_all(&masked(0xC1, &deflated(a.as_bytes())))
            .await
            .unwrap();
        client
            .write_all(&masked(0xC1, &deflated(b.as_bytes())))
            .await
            .unwrap();
        assert_eq!(ws.next().await.unwrap().unwrap(), Message::Text(a));
        assert_eq!(ws.next().await.unwrap().unwrap(), Message::Text(b));
    }

    #[tokio::test]
    async fn a_back_reference_into_the_last_message_does_not_break_the_socket() {
        // A client that ignores `client_no_context_takeover`: the second
        // message points into the first one. The inflater (miniz, wrapping
        // window) cannot tell such a distance from a legal one, so this side
        // cannot refuse it with certainty (OR-WS-4). What it must do: no
        // panic, and either the message or a close with a code — never a
        // stuck or silently dead socket.
        let text = "the same sentence, again and again. ".repeat(100);
        let mut c = flate2::Compress::new(Compression::best(), false);
        let first = sync_deflate(&mut c, text.as_bytes());
        let second = sync_deflate(&mut c, text.as_bytes());
        assert!(
            second.len() < first.len() / 2,
            "the second leans on the first"
        );
        let (server_io, mut client) = tokio::io::duplex(1 << 20);
        let mut ws = WebSocket::from_upgraded(server_io, true);
        client.write_all(&masked(0xC1, &first)).await.unwrap();
        client.write_all(&masked(0xC1, &second)).await.unwrap();
        client.write_all(&masked(0x89, b"after")).await.unwrap();
        assert_eq!(ws.next().await.unwrap().unwrap(), Message::Text(text));
        match ws.next().await.unwrap() {
            Ok(Message::Text(_)) => {
                assert_eq!(
                    ws.next().await.unwrap().unwrap(),
                    Message::Ping(b"after".to_vec()),
                    "the socket reads on"
                );
            }
            Ok(other) => panic!("unexpected {other:?}"),
            Err(e) => {
                let code = codec::close_code_of(&e);
                assert!(matches!(code, Some(1002 | 1007)), "{e}");
                assert_eq!(close_code(&mut client).await, code);
            }
        }
    }

    #[tokio::test]
    async fn text_that_is_not_utf8_closes_with_1007() {
        let bad = [b'o', b'k', 0xff, 0xfe];
        assert_eq!(refused_with(false, &[masked(0x81, &bad)]).await, Some(1007));
        let wire = deflated(&bad.repeat(2000));
        assert_eq!(refused_with(true, &[masked(0xC1, &wire)]).await, Some(1007));
    }

    #[tokio::test]
    async fn an_oversized_or_fragmented_control_frame_closes_with_1002() {
        assert_eq!(
            refused_with(false, &[masked(0x89, &[0; 126])]).await,
            Some(1002)
        );
        assert_eq!(refused_with(false, &[masked(0x09, b"p")]).await, Some(1002));
    }

    #[tokio::test]
    async fn a_broken_close_frame_closes_with_1002() {
        for payload in [
            vec![0x03],                     // one byte
            1005u16.to_be_bytes().to_vec(), // reserved, never on the wire
            1006u16.to_be_bytes().to_vec(),
            1015u16.to_be_bytes().to_vec(),
            999u16.to_be_bytes().to_vec(),  // below the range
            2000u16.to_be_bytes().to_vec(), // 1016-2999: unassigned
        ] {
            assert_eq!(
                refused_with(false, &[masked(0x88, &payload)]).await,
                Some(1002),
                "{payload:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_close_reason_that_is_not_utf8_closes_with_1007() {
        let frame = masked(0x88, &[0x03, 0xe8, 0xff]);
        assert_eq!(refused_with(false, &[frame]).await, Some(1007));
    }

    #[tokio::test]
    async fn valid_close_codes_are_echoed() {
        for code in [1000u16, 1001, 1003, 1007, 1011, 1014, 3000, 4409, 4999] {
            let (server_io, mut client) = tokio::io::duplex(4096);
            let mut ws = WebSocket::from_upgraded(server_io, false);
            client
                .write_all(&masked(0x88, &code.to_be_bytes()))
                .await
                .unwrap();
            let Some(Ok(Message::Close(Some(cf)))) = ws.next().await else {
                panic!("close {code}")
            };
            assert_eq!(cf.code, code);
            ws.flush().await.unwrap();
            assert_eq!(close_code(&mut client).await, Some(code));
        }
    }

    #[tokio::test]
    async fn a_server_close_is_not_closed_twice() {
        let (server_io, mut client) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_upgraded(server_io, false);
        ws.send(Message::Close(Some(CloseFrame {
            code: 4409,
            reason: Cow::Borrowed("replaced"),
        })))
        .await
        .unwrap();
        // The client's echo.
        client
            .write_all(&masked(0x88, &4409u16.to_be_bytes()))
            .await
            .unwrap();
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
        assert!(
            ws.send(Message::Text("late".into())).await.is_err(),
            "no data after a close"
        );
        ws.flush().await.unwrap();
        drop(ws);
        let mut got = Vec::new();
        client.read_to_end(&mut got).await.unwrap();
        let mut once = vec![0x88, 10];
        once.extend_from_slice(&4409u16.to_be_bytes());
        once.extend_from_slice(b"replaced");
        assert_eq!(got, once, "one close, no echo of the echo");
    }

    #[tokio::test]
    async fn a_ping_during_a_compression_job_is_answered_first() {
        let (server_io, mut client) = tokio::io::duplex(4 << 20);
        let mut ws = WebSocket::from_upgraded(server_io, true);
        let large = (0..200_000u32)
            .map(|i| char::from(b'a' + (i * 7 % 26) as u8))
            .collect::<String>();
        // Queued, not yet compressed: the job runs on the next flush.
        ws.feed(Message::Text(large.clone())).await.unwrap();
        client.write_all(&masked(0x89, b"hb")).await.unwrap();
        assert_eq!(
            ws.next().await.unwrap().unwrap(),
            Message::Ping(b"hb".to_vec())
        );
        ws.flush().await.unwrap();
        let mut pong = [0u8; 4];
        client.read_exact(&mut pong).await.unwrap();
        assert_eq!(pong, [0x8A, 2, b'h', b'b'], "the pong goes first");
        let mut head = [0u8; 2];
        client.read_exact(&mut head).await.unwrap();
        assert_eq!(head[0], 0xC1, "then the message, whole and compressed");
        let len = match head[1] {
            126 => {
                let mut l = [0u8; 2];
                client.read_exact(&mut l).await.unwrap();
                u16::from_be_bytes(l) as usize
            }
            127 => {
                let mut l = [0u8; 8];
                client.read_exact(&mut l).await.unwrap();
                u64::from_be_bytes(l) as usize
            }
            n => n as usize,
        };
        let mut body = vec![0u8; len];
        client.read_exact(&mut body).await.unwrap();
        body.extend_from_slice(&[0, 0, 0xff, 0xff]);
        let mut d = flate2::Decompress::new(false);
        let mut out = Vec::with_capacity(large.len() + 16);
        d.decompress_vec(&body, &mut out, flate2::FlushDecompress::Sync)
            .unwrap();
        assert_eq!(out, large.as_bytes());
    }

    #[tokio::test]
    async fn a_ping_is_answered_and_a_close_echoed() {
        let (server_io, mut client) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_upgraded(server_io, true);
        client.write_all(&masked(0x89, b"hi")).await.unwrap();
        client.write_all(&masked(0x81, b"text")).await.unwrap();
        client
            .write_all(&masked(0x88, &[0x03, 0xe8, b'o', b'k']))
            .await
            .unwrap();
        assert_eq!(
            ws.next().await.unwrap().unwrap(),
            Message::Ping(b"hi".to_vec())
        );
        assert_eq!(
            ws.next().await.unwrap().unwrap(),
            Message::Text("text".into())
        );
        let Message::Close(Some(cf)) = ws.next().await.unwrap().unwrap() else {
            panic!("close")
        };
        assert_eq!(cf.code, 1000);
        assert!(ws.next().await.is_none(), "nothing after a close");
        ws.flush().await.unwrap();
        let mut got = [0u8; 8];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got[..4], &[0x8A, 2, b'h', b'i'], "the pong");
        assert_eq!(&got[4..], &[0x88, 2, 0x03, 0xe8][..], "the close echo");
    }

    #[tokio::test]
    async fn the_threshold_decides_per_message() {
        let (server_io, mut client) = tokio::io::duplex(1 << 20);
        let mut ws = WebSocket::from_upgraded(server_io, true);
        let small = "x".repeat(DEFLATE_MIN_BYTES - 1);
        let large = "y".repeat(DEFLATE_MIN_BYTES * 8);
        ws.send(Message::Text(small.clone())).await.unwrap();
        ws.send(Message::Text(large)).await.unwrap();
        let mut head = [0u8; 4];
        client.read_exact(&mut head).await.unwrap();
        assert_eq!(head[0], 0x81, "below the threshold: no RSV1");
        assert_eq!(u16::from_be_bytes([head[2], head[3]]) as usize, small.len());
        let mut body = vec![0u8; small.len()];
        client.read_exact(&mut body).await.unwrap();
        let mut head = [0u8; 2];
        client.read_exact(&mut head).await.unwrap();
        assert_eq!(head[0], 0xC1, "at or above the threshold: compressed");
        assert!(
            (head[1] as usize) < 126,
            "32 KiB of one byte deflates to a few bytes"
        );
    }

    #[tokio::test]
    async fn a_sent_message_is_one_frame() {
        let (server_io, mut client) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_upgraded(server_io, false);
        ws.send(Message::Text("x".into())).await.unwrap();
        let mut got = [0u8; 3];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(got, [0x81, 1, b'x']);
    }
}
