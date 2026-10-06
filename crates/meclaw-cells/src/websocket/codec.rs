//! The server side of RFC 6455 framing, with permessage-deflate (RFC 7692).
//!
//! Why a codec of our own: no tungstenite release (through 0.30) implements
//! RFC 7692, and it rejects every frame with RSV1 set — so a browser that
//! negotiated the extension could not even send its first click. The framing a
//! server needs is small and fully specified, and it lets the one decision this
//! wave is about sit where it belongs: per message, compressed or not
//! ([`super::DEFLATE_MIN_BYTES`]).
//!
//! The codec frames and reads. Both directions of deflate run in the socket,
//! in steps of [`STEP`] with a yield between them (AGENTS.md rule 13): a
//! compressed client message comes out of the codec still compressed
//! ([`In::Deflated`]) and is inflated there.

use super::{CloseFrame, Message};
use bytes::{Buf, BufMut, BytesMut};
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use std::borrow::Cow;
use std::fmt;
use std::io;
use tokio_util::codec::{Decoder, Encoder};

/// The largest single frame a client may send (axum 0.7's default, kept).
pub const MAX_FRAME: usize = 16 << 20;
/// The largest plain message a client may send (axum 0.7's default, kept).
pub const MAX_MESSAGE: usize = 64 << 20;
/// The largest COMPRESSED client message, once inflated (1 MiB).
///
/// A client sends clicks, events and 20 ms of audio — tens to hundreds of
/// bytes. Before permessage-deflate a client had to put 64 MiB on the wire to
/// make the server hold 64 MiB; with it, ~64 KB of deflated zeros were enough
/// (review I-1 of GH #1024: ~1000:1, up to ~128 MiB of buffer per connection
/// and tens of ms of inflate on one worker). 1 MiB is 2000 times the largest
/// event and keeps the whole inflate of a message under a millisecond.
/// [`super::WebSocketUpgrade::max_inflated`] sets another value per route.
pub const MAX_INFLATED: usize = 1 << 20;
/// A compressed message may grow at most this many times its wire size ...
const INFLATE_RATIO: usize = 64;
/// ... but always to this much, so a short, very regular message still reads.
const INFLATE_FLOOR: usize = 64 * 1024;
/// How much buffer one frame header reserves ahead of its payload (review
/// M-4: a 16 MiB header reserved 16 MiB before a single payload byte came).
const RESERVE_AHEAD: usize = 64 * 1024;

const OP_CONT: u8 = 0x0;
const OP_TEXT: u8 = 0x1;
const OP_BINARY: u8 = 0x2;
const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;

/// Close codes the server sends when it fails a connection (RFC 6455 § 7.4.1).
pub(crate) const CLOSE_PROTOCOL: u16 = 1002;
pub(crate) const CLOSE_INVALID_DATA: u16 = 1007;
pub(crate) const CLOSE_TOO_BIG: u16 = 1009;

/// The tail a sync flush ends with; stripped on the wire (RFC 7692 § 7.2.1).
const TAIL: [u8; 4] = [0x00, 0x00, 0xff, 0xff];

/// Why the client's input failed the connection, with the close code that
/// says so on the wire (review M-1: before, the browser only saw 1006).
#[derive(Debug)]
pub(crate) struct Violation {
    pub(crate) code: u16,
    what: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "websocket: {} (close {})", self.what, self.code)
    }
}

impl std::error::Error for Violation {}

fn violation(code: u16, what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        Violation {
            code,
            what: what.to_string(),
        },
    )
}

fn protocol(what: &str) -> io::Error {
    violation(CLOSE_PROTOCOL, what)
}

fn too_big(what: &str) -> io::Error {
    violation(CLOSE_TOO_BIG, what)
}

fn not_utf8(what: &str) -> io::Error {
    violation(CLOSE_INVALID_DATA, what)
}

/// The close code an error of this codec owes the client, if it is one.
pub(crate) fn close_code_of(e: &io::Error) -> Option<u16> {
    e.get_ref()?.downcast_ref::<Violation>().map(|v| v.code)
}

/// Whether `code` may stand in a close frame (RFC 6455 § 7.4.1/7.4.2 and the
/// IANA registry): 1004-1006 and 1015 are reserved, 1016-2999 unassigned.
pub(crate) fn valid_close_code(code: u16) -> bool {
    matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999)
}

/// How much input one compression step takes before it yields (16 KiB); the
/// same bound holds for the output of one inflate step.
///
/// AGENTS.md rule 13: a CPU burst over 1 ms does not sit on a worker thread.
/// Deflating a 98 KB join chunk at level 6 measured 1.6 ms (zlib, mm-os01
/// under load, receipt of the WS compression wave), so the socket compresses a
/// large message in steps and yields between them; one step is ~0.25 ms.
pub(crate) const STEP: usize = 16 * 1024;

/// The compression state of one connection, present once negotiated.
pub(crate) struct Deflate {
    /// Messages at least this long are compressed.
    pub(crate) min_bytes: usize,
    level: Compression,
    /// Created on the first large message and reset per message
    /// (`server_no_context_takeover`) — a socket that never sends a large
    /// frame never pays for a compressor.
    compress: Option<Compress>,
    /// The same for the client direction (`client_no_context_takeover`).
    decompress: Option<Decompress>,
}

/// One message being compressed, step by step.
pub(crate) struct Job {
    opcode: u8,
    input: Vec<u8>,
    pos: usize,
    out: Vec<u8>,
    started: bool,
    done: bool,
}

impl Job {
    /// A job for `msg`; the message back when it is not data.
    pub(crate) fn for_message(msg: Message) -> Result<Self, Message> {
        let (opcode, input) = match msg {
            Message::Text(s) => (OP_TEXT, s.into_bytes()),
            Message::Binary(b) => (OP_BINARY, b),
            other => return Err(other),
        };
        Ok(Self {
            opcode,
            input,
            pos: 0,
            out: Vec::new(),
            started: false,
            done: false,
        })
    }

    /// What goes to the codec once the job is done: the deflated payload, or
    /// the message as it was when deflate would not make it smaller.
    pub(crate) fn into_out(self) -> Out {
        if self.out.len() < self.input.len() {
            Out::Deflated {
                opcode: self.opcode,
                payload: self.out,
            }
        } else {
            Out::Plain {
                opcode: self.opcode,
                payload: self.input,
            }
        }
    }
}

/// One compressed client message being inflated, step by step.
pub(crate) struct Inflate {
    opcode: u8,
    /// The payload with the sync-flush tail put back.
    input: Vec<u8>,
    out: Vec<u8>,
    /// The most this message may inflate to.
    cap: usize,
    started: bool,
}

impl Inflate {
    fn new(opcode: u8, mut input: Vec<u8>, max_inflated: usize) -> Self {
        let cap = max_inflated.min(INFLATE_FLOOR.max(input.len().saturating_mul(INFLATE_RATIO)));
        input.extend_from_slice(&TAIL);
        Self {
            opcode,
            input,
            out: Vec::new(),
            cap,
            started: false,
        }
    }

    /// The message, once [`Deflate::inflate_step`] said it is done.
    pub(crate) fn into_message(self) -> io::Result<Message> {
        data_message(self.opcode, self.out)
    }
}

/// A complete data message; text is checked for UTF-8 here, once.
fn data_message(opcode: u8, data: Vec<u8>) -> io::Result<Message> {
    match opcode {
        OP_TEXT => String::from_utf8(data)
            .map(Message::Text)
            .map_err(|_| not_utf8("text message is not UTF-8")),
        _ => Ok(Message::Binary(data)),
    }
}

impl Deflate {
    pub(crate) fn new(min_bytes: usize, level: Compression) -> Self {
        Self {
            min_bytes,
            level,
            compress: None,
            decompress: None,
        }
    }

    /// Compress up to [`STEP`] more bytes of `job`; `true` once it is done.
    pub(crate) fn step(&mut self, job: &mut Job) -> io::Result<bool> {
        if job.done {
            return Ok(true);
        }
        let level = self.level;
        let c = self
            .compress
            .get_or_insert_with(|| Compress::new(level, false));
        if !job.started {
            c.reset();
            job.out.reserve(job.input.len() / 4 + 64);
            job.started = true;
        }
        let end = (job.pos + STEP).min(job.input.len());
        let last = end == job.input.len();
        let flush = if last {
            FlushCompress::Sync
        } else {
            FlushCompress::None
        };
        let base = c.total_in() as usize - job.pos;
        loop {
            let at = c.total_in() as usize - base;
            c.compress_vec(&job.input[at..end], &mut job.out, flush)
                .map_err(|e| io::Error::other(format!("deflate: {e}")))?;
            let consumed = c.total_in() as usize - base == end;
            if consumed && job.out.len() < job.out.capacity() {
                break;
            }
            let more = job.out.capacity().max(1024);
            job.out.reserve(more);
        }
        job.pos = end;
        if !last {
            return Ok(false);
        }
        if !job.out.ends_with(&TAIL) {
            return Err(io::Error::other("deflate: sync flush without its tail"));
        }
        job.out.truncate(job.out.len() - TAIL.len());
        job.done = true;
        Ok(true)
    }

    /// Deflate one message in one go (tests and measurements); `None` when
    /// it would not get smaller.
    #[cfg(test)]
    fn deflate(&mut self, input: &[u8]) -> io::Result<Option<Vec<u8>>> {
        let mut job = Job::for_message(Message::Binary(input.to_vec())).expect("data");
        while !self.step(&mut job)? {}
        Ok(match job.into_out() {
            Out::Deflated { payload, .. } => Some(payload),
            Out::Plain { .. } | Out::Msg(_) => None,
        })
    }

    /// Inflate up to [`STEP`] more bytes of `job`; `true` once it is done.
    /// Past the job's cap it fails with close code 1009.
    pub(crate) fn inflate_step(&mut self, job: &mut Inflate) -> io::Result<bool> {
        let d = self
            .decompress
            .get_or_insert_with(|| Decompress::new(false));
        if !job.started {
            // client_no_context_takeover: every message on its own.
            d.reset(false);
            job.started = true;
        }
        let start = job.out.len();
        // One byte past the cap is room enough to tell "too large".
        let room = (job.cap + 1 - start).min(STEP);
        job.out.resize(start + room, 0);
        let at = d.total_in() as usize;
        let out0 = d.total_out();
        let status = d.decompress(
            &job.input[at..],
            &mut job.out[start..],
            FlushDecompress::Sync,
        );
        let produced = (d.total_out() - out0) as usize;
        job.out.truncate(start + produced);
        let status = status.map_err(|e| protocol(&format!("inflate: {e}")))?;
        if job.out.len() > job.cap {
            return Err(too_big("inflated message too large"));
        }
        if status == Status::StreamEnd {
            return Ok(true);
        }
        let all_in = d.total_in() as usize == job.input.len();
        // Everything read and room left over: the flush is complete.
        if all_in && produced < room {
            return Ok(true);
        }
        if produced == 0 && d.total_in() as usize == at {
            return Err(protocol("inflate made no progress"));
        }
        Ok(false)
    }
}

/// A data message whose frames are still arriving.
struct Partial {
    opcode: u8,
    compressed: bool,
    data: Vec<u8>,
}

/// What the codec reads.
pub(crate) enum In {
    /// A message, complete.
    Msg(Message),
    /// A compressed data message, for the socket to inflate in steps.
    Deflated(Inflate),
}

/// What the socket hands the codec to write.
pub(crate) enum Out {
    /// A message, written as it is.
    Msg(Message),
    /// A data message already compressed by [`Deflate::step`] (RSV1 set).
    Deflated { opcode: u8, payload: Vec<u8> },
    /// A data message that was meant for compression and would not shrink.
    Plain { opcode: u8, payload: Vec<u8> },
}

/// The codec of one server-side connection. It frames and reads; compressing
/// and inflating are the socket's job, in steps.
pub(crate) struct Codec {
    /// The inflate limit, present once permessage-deflate was agreed.
    max_inflated: Option<usize>,
    partial: Option<Partial>,
}

/// XOR the client's mask over `payload`.
fn unmask(payload: &mut [u8], mask: [u8; 4]) {
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= mask[i & 3];
    }
}

impl Codec {
    pub(crate) fn new(max_inflated: Option<usize>) -> Self {
        Self {
            max_inflated,
            partial: None,
        }
    }

    /// The most a compressed message may take on the wire: deflate never
    /// grows its input by more than a few bytes per 64 KiB block.
    fn compressed_cap(max_inflated: usize) -> usize {
        max_inflated + max_inflated / 64 + 1024
    }

    /// Turn a complete data message into what the socket reads.
    fn finish(&mut self, opcode: u8, compressed: bool, data: Vec<u8>) -> io::Result<In> {
        if !compressed {
            return data_message(opcode, data).map(In::Msg);
        }
        match self.max_inflated {
            Some(max) => Ok(In::Deflated(Inflate::new(opcode, data, max))),
            None => Err(protocol("RSV1 without permessage-deflate")),
        }
    }
}

impl Decoder for Codec {
    type Item = In;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> io::Result<Option<In>> {
        loop {
            if src.len() < 2 {
                return Ok(None);
            }
            let b0 = src[0];
            let b1 = src[1];
            let fin = b0 & 0x80 != 0;
            let rsv1 = b0 & 0x40 != 0;
            if b0 & 0x30 != 0 {
                return Err(protocol("RSV2/RSV3 set"));
            }
            let opcode = b0 & 0x0f;
            if b1 & 0x80 == 0 {
                return Err(protocol("client frame not masked"));
            }
            let (len, mut at) = match b1 & 0x7f {
                126 => {
                    if src.len() < 4 {
                        return Ok(None);
                    }
                    (u16::from_be_bytes([src[2], src[3]]) as u64, 4)
                }
                127 => {
                    if src.len() < 10 {
                        return Ok(None);
                    }
                    let mut b = [0u8; 8];
                    b.copy_from_slice(&src[2..10]);
                    (u64::from_be_bytes(b), 10)
                }
                n => (n as u64, 2),
            };
            if len > MAX_FRAME as u64 {
                return Err(too_big("frame too large"));
            }
            let len = len as usize;
            let control = opcode & 0x8 != 0;
            if control && (!fin || len > 125) {
                return Err(protocol("fragmented or oversized control frame"));
            }
            if rsv1 && (control || opcode == OP_CONT) {
                return Err(protocol("RSV1 on a control or continuation frame"));
            }
            if rsv1 && self.max_inflated.is_none() {
                return Err(protocol("RSV1 without permessage-deflate"));
            }
            if rsv1
                && self
                    .max_inflated
                    .is_some_and(|max| len > Self::compressed_cap(max))
            {
                return Err(too_big("compressed frame too large"));
            }
            if src.len() < at + 4 + len {
                src.reserve((at + 4 + len - src.len()).min(RESERVE_AHEAD));
                return Ok(None);
            }
            let mask = [src[at], src[at + 1], src[at + 2], src[at + 3]];
            at += 4;
            src.advance(at);
            let mut payload = src.split_to(len).to_vec();
            unmask(&mut payload, mask);
            match opcode {
                OP_PING => return Ok(Some(In::Msg(Message::Ping(payload)))),
                OP_PONG => return Ok(Some(In::Msg(Message::Pong(payload)))),
                OP_CLOSE => {
                    let frame = match payload.len() {
                        0 => None,
                        1 => return Err(protocol("close payload of one byte")),
                        _ => {
                            let code = u16::from_be_bytes([payload[0], payload[1]]);
                            if !valid_close_code(code) {
                                return Err(protocol("invalid close code"));
                            }
                            let reason = String::from_utf8(payload[2..].to_vec())
                                .map_err(|_| not_utf8("close reason is not UTF-8"))?;
                            Some(CloseFrame {
                                code,
                                reason: Cow::Owned(reason),
                            })
                        }
                    };
                    return Ok(Some(In::Msg(Message::Close(frame))));
                }
                OP_TEXT | OP_BINARY => {
                    if self.partial.is_some() {
                        return Err(protocol("new message inside a fragmented one"));
                    }
                    if fin {
                        return self.finish(opcode, rsv1, payload).map(Some);
                    }
                    self.partial = Some(Partial {
                        opcode,
                        compressed: rsv1,
                        data: payload,
                    });
                }
                OP_CONT => {
                    let Some(p) = self.partial.as_mut() else {
                        return Err(protocol("continuation without a message"));
                    };
                    let cap = match self.max_inflated {
                        Some(max) if p.compressed => Self::compressed_cap(max),
                        _ => MAX_MESSAGE,
                    };
                    if p.data.len() + payload.len() > cap {
                        return Err(too_big("message too large"));
                    }
                    p.data.extend_from_slice(&payload);
                    if fin {
                        let p = self.partial.take().expect("checked above");
                        return self.finish(p.opcode, p.compressed, p.data).map(Some);
                    }
                }
                _ => return Err(protocol("reserved opcode")),
            }
        }
    }
}

fn put_frame(dst: &mut BytesMut, opcode: u8, rsv1: bool, payload: &[u8]) {
    dst.reserve(payload.len() + 10);
    dst.put_u8(0x80 | if rsv1 { 0x40 } else { 0 } | opcode);
    match payload.len() {
        n if n < 126 => dst.put_u8(n as u8),
        n if n <= u16::MAX as usize => {
            dst.put_u8(126);
            dst.put_u16(n as u16);
        }
        n => {
            dst.put_u8(127);
            dst.put_u64(n as u64);
        }
    }
    dst.extend_from_slice(payload);
}

impl Encoder<Out> for Codec {
    type Error = io::Error;

    fn encode(&mut self, item: Out, dst: &mut BytesMut) -> io::Result<()> {
        let item = match item {
            Out::Deflated { opcode, payload } => {
                put_frame(dst, opcode, true, &payload);
                return Ok(());
            }
            Out::Plain { opcode, payload } => {
                put_frame(dst, opcode, false, &payload);
                return Ok(());
            }
            Out::Msg(m) => m,
        };
        let (opcode, data): (u8, Vec<u8>) = match item {
            Message::Text(s) => (OP_TEXT, s.into_bytes()),
            Message::Binary(b) => (OP_BINARY, b),
            Message::Ping(p) | Message::Pong(p) if p.len() > 125 => {
                return Err(io::Error::other(
                    "websocket: control payload over 125 bytes",
                ));
            }
            Message::Ping(p) => (OP_PING, p),
            Message::Pong(p) => (OP_PONG, p),
            Message::Close(None) => (OP_CLOSE, Vec::new()),
            Message::Close(Some(CloseFrame { code, reason })) => {
                let mut p = code.to_be_bytes().to_vec();
                // 125 bytes of payload, two of them the code.
                let mut end = reason.len().min(123);
                while !reason.is_char_boundary(end) {
                    end -= 1;
                }
                p.extend_from_slice(&reason.as_bytes()[..end]);
                (OP_CLOSE, p)
            }
        };
        put_frame(dst, opcode, false, &data);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked(b0: u8, payload: &[u8]) -> BytesMut {
        let mut f = BytesMut::new();
        f.put_u8(b0);
        match payload.len() {
            n if n < 126 => f.put_u8(0x80 | n as u8),
            n if n <= u16::MAX as usize => {
                f.put_u8(0x80 | 126);
                f.put_u16(n as u16);
            }
            n => {
                f.put_u8(0x80 | 127);
                f.put_u64(n as u64);
            }
        }
        let mask = [1u8, 2, 3, 4];
        f.extend_from_slice(&mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        f
    }

    fn deflating() -> Codec {
        Codec::new(Some(MAX_INFLATED))
    }

    /// Decode one message the way the socket does, inflating in steps.
    fn read(c: &mut Codec, src: &mut BytesMut) -> io::Result<Option<Message>> {
        let mut d = Deflate::new(STEP, Compression::fast());
        match c.decode(src)? {
            None => Ok(None),
            Some(In::Msg(m)) => Ok(Some(m)),
            Some(In::Deflated(mut job)) => {
                while !d.inflate_step(&mut job)? {}
                job.into_message().map(Some)
            }
        }
    }

    /// A packed-tree-like JSON message of about `n` bytes.
    fn tree(n: usize) -> String {
        let mut s = String::from("[\"1\",\"1\",\"lv:x\",\"phx_reply\",{\"rendered\":{");
        let mut i = 0;
        while s.len() < n {
            s.push_str(&format!(
                "\"{i}\":{{\"s\":1,\"0\":\"fig-{i}\",\"1\":\"left:{}px;top:{}px\",\"r\":1}},",
                i * 7 % 1000,
                i * 13 % 800
            ));
            i += 1;
        }
        s.push_str("\"x\":0}}]");
        s
    }

    fn decode_server(frame: &mut BytesMut) -> (bool, u8, Vec<u8>) {
        let b0 = frame[0];
        let (len, at) = match frame[1] & 0x7f {
            126 => (u16::from_be_bytes([frame[2], frame[3]]) as usize, 4),
            127 => {
                let mut b = [0u8; 8];
                b.copy_from_slice(&frame[2..10]);
                (u64::from_be_bytes(b) as usize, 10)
            }
            n => (n as usize, 2),
        };
        assert_eq!(frame[1] & 0x80, 0, "server frames are unmasked");
        let payload = frame[at..at + len].to_vec();
        frame.advance(at + len);
        (b0 & 0x40 != 0, b0 & 0x0f, payload)
    }

    #[test]
    fn a_large_message_is_compressed_and_round_trips() {
        let text = tree(100 * 1024);
        let mut d = Deflate::new(4 * 1024, Compression::default());
        let mut job = Job::for_message(Message::Text(text.clone())).expect("data");
        let mut steps = 1;
        while !d.step(&mut job).unwrap() {
            steps += 1;
        }
        assert_eq!(
            steps,
            text.len().div_ceil(STEP),
            "one step per 16 KiB of input"
        );
        let mut c = Codec::new(None);
        let mut out = BytesMut::new();
        c.encode(job.into_out(), &mut out).unwrap();
        let (rsv1, op, wire) = decode_server(&mut out);
        assert!(rsv1);
        assert_eq!(op, OP_TEXT);
        assert!(
            wire.len() * 3 <= text.len(),
            "{} of {}",
            wire.len(),
            text.len()
        );
        // The same bytes read back through the client direction.
        let mut back = deflating();
        let mut f = masked(0xC1, &wire);
        let Some(Message::Text(t)) = read(&mut back, &mut f).unwrap() else {
            panic!("text")
        };
        assert_eq!(t, text);
        // And a second message on the same compressor: reset, no context.
        let again = d.deflate(text.as_bytes()).unwrap().expect("shrinks");
        assert_eq!(
            again, wire,
            "server_no_context_takeover: same input, same bytes"
        );
    }

    #[test]
    fn a_message_deflate_cannot_shrink_goes_plain() {
        // xorshift64: bytes deflate cannot shrink.
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let noise: Vec<u8> = (0..8192)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        let mut d = Deflate::new(1, Compression::default());
        let mut job = Job::for_message(Message::Binary(noise.clone())).expect("data");
        while !d.step(&mut job).unwrap() {}
        let mut c = Codec::new(None);
        let mut out = BytesMut::new();
        c.encode(job.into_out(), &mut out).unwrap();
        let (rsv1, _, wire) = decode_server(&mut out);
        assert!(!rsv1);
        assert_eq!(wire, noise);
    }

    #[test]
    fn without_the_extension_nothing_is_compressed() {
        let mut c = Codec::new(None);
        let text = tree(100 * 1024);
        let mut out = BytesMut::new();
        c.encode(Out::Msg(Message::Text(text.clone())), &mut out)
            .unwrap();
        let (rsv1, _, wire) = decode_server(&mut out);
        assert!(!rsv1);
        assert_eq!(wire, text.as_bytes());
        // And a compressed client frame is a protocol error.
        let mut f = masked(0xC1, b"abc");
        assert!(c.decode(&mut f).is_err());
    }

    #[test]
    fn frames_arrive_in_pieces_and_fragments_join() {
        let mut c = Codec::new(None);
        let mut all = masked(0x01, b"hel");
        all.extend_from_slice(&masked(0x89, b"p")); // a ping between fragments
        all.extend_from_slice(&masked(0x80, b"lo"));
        let mut src = BytesMut::new();
        let mut got = Vec::new();
        for b in all.iter() {
            src.put_u8(*b);
            while let Some(m) = read(&mut c, &mut src).unwrap() {
                got.push(m);
            }
        }
        assert!(matches!(&got[0], Message::Ping(p) if p == b"p"));
        assert!(matches!(&got[1], Message::Text(t) if t == "hello"));
    }

    #[test]
    fn unmasked_and_reserved_bits_are_refused() {
        let mut c = deflating();
        let mut f = BytesMut::from(&[0x81u8, 0x01, b'a'][..]);
        assert!(c.decode(&mut f).is_err(), "unmasked");
        let mut c = deflating();
        let mut f = masked(0xA1, b"a");
        assert!(c.decode(&mut f).is_err(), "RSV2");
        let mut c = deflating();
        let mut f = masked(0xC9, b"");
        assert!(c.decode(&mut f).is_err(), "RSV1 on a ping");
    }

    #[test]
    fn a_close_keeps_its_code_and_reason() {
        let mut c = Codec::new(None);
        let mut out = BytesMut::new();
        c.encode(
            Out::Msg(Message::Close(Some(CloseFrame {
                code: 4001,
                reason: Cow::Borrowed("gone"),
            }))),
            &mut out,
        )
        .unwrap();
        let (_, op, wire) = decode_server(&mut out);
        assert_eq!(op, OP_CLOSE);
        assert_eq!(&wire[..2], &4001u16.to_be_bytes());
        assert_eq!(&wire[2..], b"gone");
        let mut f = masked(0x88, &wire);
        let Some(Message::Close(Some(cf))) = read(&mut c, &mut f).unwrap() else {
            panic!("close")
        };
        assert_eq!((cf.code, cf.reason.as_ref()), (4001, "gone"));
    }

    #[test]
    fn an_inflate_bomb_is_refused() {
        // 2 MiB of zeros deflate to ~2 KB; the reader stops at MAX_INFLATED.
        let zeros = vec![0u8; 2 * MAX_INFLATED];
        let mut d = Deflate::new(1, Compression::best());
        let small = d.deflate(&zeros).unwrap().expect("zeros compress");
        let mut c = deflating();
        let mut f = masked(0xC2, &small);
        let e = read(&mut c, &mut f).unwrap_err();
        assert_eq!(close_code_of(&e), Some(CLOSE_TOO_BIG), "{e}");
        // Under the size limit, the ratio still holds: 512 KiB of zeros are
        // ~0.5 KB on the wire, 1000:1, far past 64:1 and the 64 KiB floor.
        let half = d.deflate(&zeros[..MAX_INFLATED / 2]).unwrap().unwrap();
        let mut f = masked(0xC2, &half);
        let e = read(&mut deflating(), &mut f).unwrap_err();
        assert_eq!(close_code_of(&e), Some(CLOSE_TOO_BIG), "{e}");
    }

    #[test]
    fn inflate_goes_in_steps_and_respects_the_ratio() {
        // xorshift text: compresses ~2:1, well inside the ratio.
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let text: Vec<u8> = (0..MAX_INFLATED)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                b'a' + (x % 16) as u8
            })
            .collect();
        let mut d = Deflate::new(1, Compression::default());
        let wire = d.deflate(&text).unwrap().unwrap();
        let mut c = deflating();
        let Some(In::Deflated(mut job)) = c.decode(&mut masked(0xC2, &wire)).unwrap() else {
            panic!("compressed")
        };
        let mut steps = 1;
        while !d.inflate_step(&mut job).unwrap() {
            assert!(job.out.len() <= steps * STEP, "one step, at most STEP out");
            steps += 1;
        }
        assert!(steps >= MAX_INFLATED / STEP, "{steps} steps");
        assert_eq!(job.into_message().unwrap(), Message::Binary(text));
    }

    #[test]
    fn a_header_past_max_frame_is_refused_before_any_allocation() {
        let mut c = Codec::new(None);
        let mut f = BytesMut::from(&[0x82u8, 0xFF, 0x80, 0, 0, 0, 0, 0, 0, 0][..]);
        assert!(c.decode(&mut f).is_err(), "64-bit length with the MSB set");
        let mut c = Codec::new(None);
        let mut f = BytesMut::from(&[0x82u8, 0xFF][..]);
        f.put_u64(MAX_FRAME as u64 + 1);
        assert!(c.decode(&mut f).is_err(), "one byte past MAX_FRAME");
        // A legal 16 MiB header reserves little before the payload arrives.
        let mut c = Codec::new(None);
        let mut f = BytesMut::from(&[0x82u8, 0xFF][..]);
        f.put_u64(MAX_FRAME as u64);
        f.extend_from_slice(&[1, 2, 3, 4]);
        assert!(matches!(c.decode(&mut f), Ok(None)));
        assert!(f.capacity() < 1 << 20, "reserved {}", f.capacity());
    }

    #[test]
    fn random_bytes_never_panic() {
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for run in 0..5000 {
            let len = (next() % 300) as usize;
            let mut bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            // Mostly masked and with a sane opcode, so the fuzz gets past
            // the first checks into lengths, fragments and inflate.
            if bytes.len() > 2 && run % 4 != 0 {
                bytes[0] &= 0xC3;
                bytes[1] |= 0x80;
                if run % 3 == 0 {
                    bytes[1] = 0x80 | (bytes[1] & 0x3f);
                }
            }
            let mut c = if run % 2 == 0 {
                deflating()
            } else {
                Codec::new(None)
            };
            let mut src = BytesMut::from(&bytes[..]);
            while let Ok(Some(_)) = read(&mut c, &mut src) {}
        }
    }

    /// The CPU a large frame costs, printed for the receipt (not a timing lock:
    /// lanes differ, the number goes into the wave's measurement table).
    #[test]
    fn deflate_cost_per_large_frame() {
        let text = tree(98 * 1024);
        for (name, level) in [
            ("fast", Compression::fast()),
            ("default", Compression::default()),
        ] {
            let mut d = Deflate::new(4 * 1024, level);
            let start = std::time::Instant::now();
            let mut wire = 0;
            for _ in 0..50 {
                wire = d.deflate(text.as_bytes()).unwrap().unwrap().len();
            }
            let per = start.elapsed() / 50;
            eprintln!(
                "gh1024 deflate {name}: {} B -> {wire} B ({:.1} %), {per:?} per frame",
                text.len(),
                100.0 * wire as f64 / text.len() as f64
            );
        }
        // Rule 13: what ONE step holds a worker, deflate at level 6 and
        // inflate, over 50 frames (max and mean).
        let mut d = Deflate::new(4 * 1024, Compression::new(6));
        let (mut max, mut sum, mut n) =
            (std::time::Duration::ZERO, std::time::Duration::ZERO, 0u32);
        let mut wire = Vec::new();
        for _ in 0..50 {
            let mut job = Job::for_message(Message::Text(text.clone())).expect("data");
            loop {
                let t = std::time::Instant::now();
                let done = d.step(&mut job).unwrap();
                let e = t.elapsed();
                (max, sum, n) = (max.max(e), sum + e, n + 1);
                if done {
                    break;
                }
            }
            if let Out::Deflated { payload, .. } = job.into_out() {
                wire = payload;
            }
        }
        eprintln!(
            "gh1024 deflate step level 6: max {max:?}, mean {:?}",
            sum / n
        );
        let (mut max, mut sum, mut n) =
            (std::time::Duration::ZERO, std::time::Duration::ZERO, 0u32);
        for _ in 0..50 {
            let Some(In::Deflated(mut job)) = deflating().decode(&mut masked(0xC1, &wire)).unwrap()
            else {
                panic!("compressed")
            };
            loop {
                let t = std::time::Instant::now();
                let done = d.inflate_step(&mut job).unwrap();
                let e = t.elapsed();
                (max, sum, n) = (max.max(e), sum + e, n + 1);
                if done {
                    break;
                }
            }
        }
        eprintln!("gh1024 inflate step: max {max:?}, mean {:?}", sum / n);
    }
}
