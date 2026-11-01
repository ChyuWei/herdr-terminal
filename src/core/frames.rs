//! The framed wire format: `[channel u32 LE][kind u8][len u32 LE][payload]`.
//!
//! Channel 0 carries JSON-RPC messages; every other channel is a stream
//! (terminal bytes, or JSON lines for events). Every field is little-endian,
//! matching herdr-core.

use std::fmt;

pub const HEADER: usize = 9;
/// The largest payload a frame may carry; a longer length is malformed.
pub const MAX_PAYLOAD: usize = 16 << 20;

/// Payload bytes: JSON-RPC on channel 0, terminal bytes on streams.
pub const DATA: u8 = 0;
/// u32 LE: the receiver consumed this many bytes; the sender may send more.
pub const CREDIT: u8 = 1;
/// The channel is over (a JSON reason from core, empty from a viewer).
pub const CLOSE: u8 = 2;
/// Viewer → core: u16 LE cols, u16 LE rows.
pub const RESIZE: u8 = 3;
/// Viewer → core: 1 = gained focus, 0 = lost it.
pub const FOCUS: u8 = 4;
/// Viewer → core: JSON display facts (colors, cell size).
pub const FACTS: u8 = 5;

/// A frame kind. Unknown kinds are kept as their raw byte so a newer core can
/// add one without breaking this client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Data = DATA,
    Credit = CREDIT,
    Close = CLOSE,
    Resize = RESIZE,
    Focus = FOCUS,
    Facts = FACTS,
}

impl Kind {
    pub fn from_u8(kind: u8) -> Option<Kind> {
        match kind {
            DATA => Some(Kind::Data),
            CREDIT => Some(Kind::Credit),
            CLOSE => Some(Kind::Close),
            RESIZE => Some(Kind::Resize),
            FOCUS => Some(Kind::Focus),
            FACTS => Some(Kind::Facts),
            _ => None,
        }
    }
}

/// One decoded frame. `kind` stays a raw `u8` for forward compatibility.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub channel: u32,
    pub kind: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn data(channel: u32, payload: impl Into<Vec<u8>>) -> Frame {
        Frame {
            channel,
            kind: DATA,
            payload: payload.into(),
        }
    }

    /// Credit for `bytes` consumed; the wire field is a u32.
    pub fn credit(channel: u32, bytes: usize) -> Frame {
        let bytes = u32::try_from(bytes).unwrap_or(u32::MAX);
        Frame {
            channel,
            kind: CREDIT,
            payload: bytes.to_le_bytes().to_vec(),
        }
    }

    pub fn close(channel: u32, reason: Option<&serde_json::Value>) -> Frame {
        Frame {
            channel,
            kind: CLOSE,
            payload: reason
                .map(|r| r.to_string().into_bytes())
                .unwrap_or_default(),
        }
    }

    pub fn resize(channel: u32, cols: u16, rows: u16) -> Frame {
        let mut payload = Vec::with_capacity(4);
        payload.extend_from_slice(&cols.to_le_bytes());
        payload.extend_from_slice(&rows.to_le_bytes());
        Frame {
            channel,
            kind: RESIZE,
            payload,
        }
    }

    pub fn focus(channel: u32, focused: bool) -> Frame {
        Frame {
            channel,
            kind: FOCUS,
            payload: vec![u8::from(focused)],
        }
    }

    pub fn facts(channel: u32, facts: &serde_json::Value) -> Frame {
        Frame {
            channel,
            kind: FACTS,
            payload: facts.to_string().into_bytes(),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        encode(self.channel, self.kind, &self.payload)
    }
}

/// Encodes one frame. A payload above [`MAX_PAYLOAD`] is refused in every
/// build, not merely asserted: core would read it as a malformed header.
pub fn encode(channel: u32, kind: u8, payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(FrameError::Oversized { len: payload.len() });
    }
    let len = payload.len() as u32;
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(&channel.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Reads a frame header. An oversized length is refused before any payload
/// is allocated.
pub fn decode_header(header: &[u8; HEADER]) -> Result<(u32, u8, usize), FrameError> {
    let channel = u32::from_le_bytes(header[0..4].try_into().expect("four bytes"));
    let kind = header[4];
    let len = u32::from_le_bytes(header[5..9].try_into().expect("four bytes")) as usize;
    if len > MAX_PAYLOAD {
        return Err(FrameError::Oversized { len });
    }
    Ok((channel, kind, len))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    Oversized { len: usize },
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Oversized { len } => {
                write!(
                    f,
                    "frame of {len} bytes exceeds the {MAX_PAYLOAD}-byte limit"
                )
            }
        }
    }
}

impl std::error::Error for FrameError {}

/// Incremental decoder: reads can deliver part of a header, several frames
/// at once, or a payload in pieces.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    start: usize,
}

impl FrameDecoder {
    pub fn new() -> FrameDecoder {
        FrameDecoder::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame, or `None` while more bytes are needed.
    ///
    /// An error means the stream can no longer be framed: the buffer is
    /// cleared and the caller should drop the connection.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        let avail = self.buf.len() - self.start;
        if avail < HEADER {
            return Ok(None);
        }
        let header: [u8; HEADER] = self.buf[self.start..self.start + HEADER]
            .try_into()
            .expect("header length");
        let (channel, kind, len) = match decode_header(&header) {
            Ok(parts) => parts,
            Err(e) => {
                self.buf.clear();
                self.start = 0;
                return Err(e);
            }
        };
        if avail < HEADER + len {
            return Ok(None);
        }
        let payload = self.buf[self.start + HEADER..self.start + HEADER + len].to_vec();
        self.start += HEADER + len;
        self.compact();
        Ok(Some(Frame {
            channel,
            kind,
            payload,
        }))
    }

    /// Bytes received but not framed yet (tests and diagnostics).
    pub fn pending(&self) -> usize {
        self.buf.len() - self.start
    }

    fn compact(&mut self) {
        if self.start > 64 * 1024 || self.start * 2 > self.buf.len() {
            self.buf.drain(..self.start);
            self.start = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_kind_round_trips_on_its_channel() {
        let frames = [
            Frame::data(0, b"{\"id\":1}".to_vec()),
            Frame::credit(7, 1234),
            Frame::close(7, Some(&json!({"reason": "exited", "exit_code": 0}))),
            Frame::close(8, None),
            Frame::resize(9, 120, 40),
            Frame::focus(9, true),
            Frame::focus(9, false),
            Frame::facts(9, &json!({"fg": "#ffffff", "cell": [8, 16]})),
        ];
        let mut decoder = FrameDecoder::new();
        for frame in &frames {
            decoder.push(&frame.encode().expect("small enough"));
        }
        for want in frames {
            let got = decoder.next_frame().expect("valid frame").expect("a frame");
            assert_eq!(got, want);
        }
        assert_eq!(decoder.next_frame().expect("valid frame"), None);
        assert_eq!(decoder.pending(), 0);

        assert_eq!(Kind::from_u8(CREDIT), Some(Kind::Credit));
        assert_eq!(
            Kind::from_u8(42),
            None,
            "unknown kinds stay readable as raw bytes"
        );
    }

    #[test]
    fn partial_reads_batches_and_empty_payloads_are_framed_in_order() {
        let frames = [
            Frame::data(0, b"one".to_vec()),
            Frame::data(0, Vec::new()),
            Frame::data(7, vec![b'x'; 70_000]),
            Frame::close(7, None),
        ];
        let mut wire = Vec::new();
        for frame in &frames {
            wire.extend(frame.encode().expect("small enough"));
        }

        // One byte at a time: no frame appears before it is complete.
        let mut decoder = FrameDecoder::new();
        let mut got = Vec::new();
        for byte in &wire {
            decoder.push(std::slice::from_ref(byte));
            if let Some(frame) = decoder.next_frame().expect("valid frame") {
                got.push(frame);
            }
        }
        assert_eq!(got, frames.to_vec());

        // Everything at once.
        let mut decoder = FrameDecoder::new();
        decoder.push(&wire);
        let batch: Vec<Frame> =
            std::iter::from_fn(|| decoder.next_frame().expect("valid frame")).collect();
        assert_eq!(batch, frames.to_vec());
    }

    #[test]
    fn an_oversized_length_is_refused_before_its_payload_arrives() {
        let mut header = [0u8; HEADER];
        header[5..9].copy_from_slice(&(u32::MAX).to_le_bytes());
        assert_eq!(
            decode_header(&header),
            Err(FrameError::Oversized {
                len: u32::MAX as usize
            })
        );

        let mut decoder = FrameDecoder::new();
        decoder.push(&header);
        assert!(decoder.next_frame().is_err());
        assert_eq!(decoder.pending(), 0, "a broken stream is not kept");

        // The cap itself is a valid length.
        let mut at_cap = [0u8; HEADER];
        at_cap[5..9].copy_from_slice(&(MAX_PAYLOAD as u32).to_le_bytes());
        assert_eq!(decode_header(&at_cap), Ok((0, DATA, MAX_PAYLOAD)));
    }

    #[test]
    fn a_payload_past_the_cap_is_refused_when_encoding_in_every_build() {
        assert_eq!(
            encode(0, DATA, &vec![0u8; MAX_PAYLOAD + 1]),
            Err(FrameError::Oversized {
                len: MAX_PAYLOAD + 1
            })
        );
        // The cap itself still encodes; its header says exactly MAX_PAYLOAD.
        let wire = encode(0, DATA, &vec![0u8; MAX_PAYLOAD]).expect("the cap is allowed");
        assert_eq!(wire.len(), HEADER + MAX_PAYLOAD);
        assert_eq!(
            decode_header(wire[..HEADER].try_into().expect("header")),
            Ok((0, DATA, MAX_PAYLOAD))
        );
    }
}
