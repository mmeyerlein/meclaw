//! One client of the `voice` cell, whatever carries it (GH #643).
//!
//! **The transport is a parameter.** Everything below this line — the
//! recognition session, the framer, the retry, the displacement, the dispatch
//! queue — was written against a `WebSocket` and asks nothing of it that a pair
//! of channels cannot answer: text in, binary in, text out, binary out, and a
//! close with a code. So the socket became a parameter of the connection rather
//! than its foundation, and a second door — a `voice:<call>` topic on the socket
//! a display page already holds — reaches the same code with the same meanings.
//! What `hello`, `bad_audio_frame`, `4409` and `client_too_slow` mean does not
//! depend on which door a client came through.
//!
//! There is one asymmetry, and it belongs to the transports rather than to the
//! protocol: a WebSocket carries its own `Ping`/`Pong`, skipped here rather than
//! reported, and a channel link's capacity is the queue
//! [`meclaw_colony::LINK_QUEUE`] names instead of a TCP send buffer. Neither is
//! visible to the connection.
//!
//! # Why a link comes apart into two halves
//!
//! [`ClientLink::into_halves`] exists for the connection loop, and it is the
//! same move `WebSocket::split` was there for: the loop reads the client in one
//! `select!` arm and writes to it from four others, so one value borrowed
//! mutably by the read future cannot also be written to from a branch body. The
//! halves are what the loop holds; the link itself is what a door hands over.

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use meclaw_colony::LinkFrame;
use std::borrow::Cow;
use tokio::sync::mpsc;

/// Which door a client came through.
///
/// It is the link's own marker, set where the link is built and read back off
/// it: a WebSocket this cell was handed, or a topic on somebody else's socket.
/// No production caller reads it today; it stays as the marker of the link,
/// because a link that cannot say where it came from is one a later reader has
/// to guess about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    /// A WebSocket: a socket the colony's listener handed to this cell.
    Ws,
    /// A topic on somebody else's socket.
    Chan,
}

/// What arrives from the client, whatever carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    /// A text frame: one client frame as JSON.
    Text(String),
    /// A binary frame: audio in the format `hello` declared.
    Binary(Vec<u8>),
    /// The client is going away.
    Close,
}

/// What is sent to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    /// A server frame as JSON.
    Text(String),
    /// One audio frame.
    Binary(Vec<u8>),
}

/// The client this link named is not there any more.
///
/// A payload-free error, because there is exactly one cause: the other end went
/// away. What a caller does about it never depends on which transport noticed.
/// It is a named type rather than `()` because a public `Result<_, ()>` says
/// nothing at the call site — and this crate writes its errors by hand rather
/// than deriving them (`stdio_child::error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientGone;

impl std::fmt::Display for ClientGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the client is gone")
    }
}

impl std::error::Error for ClientGone {}

/// The writing half of a link.
pub enum LinkSink {
    /// A WebSocket's sending half.
    Ws(SplitSink<WebSocket, Message>),
    /// A forwarder's channel. `None` once the link was closed from here: the
    /// close is the last thing the other side hears.
    Chan(Option<mpsc::Sender<LinkFrame>>),
}

impl LinkSink {
    /// Send one frame; [`ClientGone`] when the client is not there any more.
    ///
    /// The channel arm awaits its `send`, and that await IS the backpressure: a
    /// page that stops reading fills the link, this connection stops writing,
    /// and the dispatch queue in front of it counts the client out — the same
    /// verdict a full TCP send buffer produces on the other transport.
    pub async fn send(&mut self, out: Outgoing) -> Result<(), ClientGone> {
        match self {
            Self::Ws(sink) => sink
                .send(match out {
                    Outgoing::Text(text) => Message::Text(text),
                    Outgoing::Binary(bytes) => Message::Binary(bytes),
                })
                .await
                .map_err(|_| ClientGone),
            Self::Chan(tx) => match tx.as_ref() {
                None => Err(ClientGone),
                Some(tx) => tx
                    .send(match out {
                        Outgoing::Text(text) => LinkFrame::Text(text),
                        Outgoing::Binary(bytes) => LinkFrame::Binary(bytes),
                    })
                    .await
                    .map_err(|_| ClientGone),
            },
        }
    }

    /// End the link with `code` and `reason`, as far as the transport allows.
    ///
    /// A failure is not reported: this is the last thing said on a connection
    /// that is over either way, and there is nobody left to tell.
    pub async fn close(&mut self, code: u16, reason: &str) {
        match self {
            Self::Ws(sink) => {
                let _ = sink
                    .send(Message::Close(Some(CloseFrame {
                        code,
                        reason: Cow::Owned(reason.to_string()),
                    })))
                    .await;
            }
            Self::Chan(tx) => {
                if let Some(sender) = tx.take() {
                    let _ = sender
                        .send(LinkFrame::Close {
                            code,
                            reason: reason.to_string(),
                        })
                        .await;
                }
            }
        }
    }
}

/// The reading half of a link.
pub enum LinkStream {
    /// A WebSocket's receiving half.
    Ws(SplitStream<WebSocket>),
    /// A forwarder's channel.
    Chan(mpsc::Receiver<LinkFrame>),
}

impl LinkStream {
    /// The next thing the client sent, or `None` when it is gone.
    ///
    /// `Ping`/`Pong` are the socket's own business and are skipped rather than
    /// reported; a `Close` on either transport is [`Incoming::Close`], and a
    /// channel whose sender is gone is the same fact as a socket that ended.
    pub async fn next(&mut self) -> Option<Incoming> {
        match self {
            Self::Ws(stream) => loop {
                match stream.next().await {
                    None | Some(Err(_)) => return None,
                    Some(Ok(Message::Text(text))) => return Some(Incoming::Text(text)),
                    Some(Ok(Message::Binary(bytes))) => {
                        return Some(Incoming::Binary(bytes));
                    }
                    Some(Ok(Message::Close(_))) => return Some(Incoming::Close),
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                }
            },
            Self::Chan(rx) => match rx.recv().await {
                None => None,
                Some(LinkFrame::Text(text)) => Some(Incoming::Text(text)),
                Some(LinkFrame::Binary(bytes)) => Some(Incoming::Binary(bytes)),
                Some(LinkFrame::Close { .. }) => Some(Incoming::Close),
            },
        }
    }
}

/// One client, over a WebSocket or over an in-process link.
pub struct ClientLink {
    sink: LinkSink,
    stream: LinkStream,
}

impl ClientLink {
    /// A client on a WebSocket, handed to this cell by the colony's one listener.
    pub fn ws(ws: WebSocket) -> Self {
        let (sink, stream) = ws.split();
        Self {
            sink: LinkSink::Ws(sink),
            stream: LinkStream::Ws(stream),
        }
    }

    /// A client on a topic, forwarded as frames by whoever holds its socket.
    pub fn chan(rx: mpsc::Receiver<LinkFrame>, tx: mpsc::Sender<LinkFrame>) -> Self {
        Self {
            sink: LinkSink::Chan(Some(tx)),
            stream: LinkStream::Chan(rx),
        }
    }

    /// The next thing the client sent, or `None` when it is gone.
    pub async fn next(&mut self) -> Option<Incoming> {
        self.stream.next().await
    }

    /// Send one frame; [`ClientGone`] when the client is not there any more.
    pub async fn send(&mut self, out: Outgoing) -> Result<(), ClientGone> {
        self.sink.send(out).await
    }

    /// End the link with `code` and `reason`.
    pub async fn close(&mut self, code: u16, reason: &str) {
        self.sink.close(code, reason).await
    }

    /// Which door this client came through.
    pub fn door(&self) -> Door {
        match self.stream {
            LinkStream::Ws(_) => Door::Ws,
            LinkStream::Chan(_) => Door::Chan,
        }
    }

    /// The reading and the writing half, for a loop that needs both at once.
    pub fn into_halves(self) -> (LinkSink, LinkStream) {
        (self.sink, self.stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_channel_link_carries_text_binary_and_close_both_ways() {
        let (to_cell_tx, to_cell_rx) = mpsc::channel::<LinkFrame>(4);
        let (from_cell_tx, mut from_cell_rx) = mpsc::channel::<LinkFrame>(4);
        let mut link = ClientLink::chan(to_cell_rx, from_cell_tx);
        to_cell_tx
            .send(LinkFrame::Text("{\"type\":\"hold\"}".into()))
            .await
            .expect("send");
        to_cell_tx
            .send(LinkFrame::Binary(vec![0, 1]))
            .await
            .expect("send");
        assert!(matches!(link.next().await, Some(Incoming::Text(t)) if t.contains("hold")));
        assert!(matches!(link.next().await, Some(Incoming::Binary(b)) if b == vec![0, 1]));
        link.send(Outgoing::Binary(vec![7])).await.expect("open");
        link.close(4409, "replaced").await;
        assert_eq!(from_cell_rx.recv().await, Some(LinkFrame::Binary(vec![7])));
        assert_eq!(
            from_cell_rx.recv().await,
            Some(LinkFrame::Close {
                code: 4409,
                reason: "replaced".into()
            })
        );
        assert_eq!(
            from_cell_rx.recv().await,
            None,
            "the sender is gone after a close"
        );
        assert_eq!(
            link.door(),
            Door::Chan,
            "a topic is not a socket of our own"
        );
        drop(to_cell_tx);
        assert!(link.next().await.is_none());
    }
}
