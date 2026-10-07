//! Protobuf definitions and helpers for the gRPC-web protocol.
//!
//! The definitions are derived from the JS `grpc-web` source of the
//! Flightradar24 web client; see `proto/fr24/proto/README.md` upstream.
//!
//! Since the gRPC protocol is remarkably simple, we construct and parse
//! [length-prefixed messages](https://github.com/grpc/grpc/blob/master/doc/PROTOCOL-HTTP2.md)
//! manually.

use bytes::{Buf, Bytes, BytesMut};
use prost::Message;

use crate::error::{GrpcError, GrpcErrorKind};

/// Messages in the `fr24.feed.api.v1` namespace, plus `common` (`_common`).
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/_includes.rs"));
}

pub use v1::common;

/// Flag for an uncompressed data frame.
const FLAG_DATA: u8 = 0x00;
/// Flag for a compressed data frame.
const FLAG_COMPRESSED: u8 = 0x01;
/// Flag for a trailers frame (gRPC-web specific).
const FLAG_TRAILERS: u8 = 0x80;
/// 1 byte flag + 4 bytes big endian length.
const HEADER_LEN: usize = 5;

/// Encode to a length-prefixed message.
pub fn encode_message(msg: &impl Message) -> Vec<u8> {
    let len = msg.encoded_len();
    let mut out = Vec::with_capacity(HEADER_LEN + len);
    out.push(FLAG_DATA); // u8, no compression
    out.extend_from_slice(&(len as u32).to_be_bytes()); // u32, big endian
    msg.encode_raw(&mut out); // binary octet
    out
}

/// A single gRPC-web frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// An uncompressed protobuf message.
    Data(Bytes),
    /// The trailers frame. `Ok` if `grpc-status` was `0` (or missing).
    Trailers(Result<(), GrpcError>),
}

/// Incrementally splits a gRPC-web byte stream into frames.
///
/// Server-streaming RPCs like `FollowFlight` deliver multiple `DATA` frames
/// in a single HTTP response body, and chunk boundaries do not necessarily
/// align with frame boundaries.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: BytesMut,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// Returns the next complete frame, or `None` if more bytes are needed.
    pub fn next_frame(&mut self) -> Option<Result<Frame, GrpcError>> {
        if self.buf.len() < HEADER_LEN {
            return None;
        }
        let flag = self.buf[0];
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if self.buf.len() < HEADER_LEN + len {
            return None;
        }
        self.buf.advance(HEADER_LEN);
        let payload = self.buf.split_to(len).freeze();
        Some(match flag {
            FLAG_DATA if payload.is_empty() => Err(GrpcError::new(GrpcErrorKind::EmptyPayload)),
            FLAG_DATA => Ok(Frame::Data(payload)),
            FLAG_COMPRESSED => Err(GrpcError::new(GrpcErrorKind::Compressed)),
            f if f & FLAG_TRAILERS != 0 => Ok(Frame::Trailers(parse_trailers(&payload))),
            _ => Err(GrpcError::new(GrpcErrorKind::Malformed)),
        })
    }

    /// Bytes that have been pushed but not yet consumed as a frame.
    pub fn remaining(&self) -> usize {
        self.buf.len()
    }
}

/// Decode a unary response body (one `DATA` frame, optionally followed by
/// trailers) into a protobuf message.
pub fn parse_data<M: Message + Default>(data: &[u8]) -> crate::Result<M> {
    let mut decoder = FrameDecoder::new();
    decoder.push(data);
    match decoder.next_frame() {
        Some(Ok(Frame::Data(payload))) => Ok(M::decode(payload)?),
        Some(Ok(Frame::Trailers(Err(e)))) | Some(Err(e)) => Err(e.into()),
        Some(Ok(Frame::Trailers(Ok(())))) => {
            Err(GrpcError::new(GrpcErrorKind::EmptyDataFrame).into())
        }
        None if data.is_empty() => Err(GrpcError::new(GrpcErrorKind::EmptyDataFrame).into()),
        None => Err(GrpcError::new(GrpcErrorKind::Malformed).into()),
    }
}

/// Parse a trailers block, e.g. `grpc-status:5\r\ngrpc-message:not%20found\r\n`.
pub fn parse_trailers(block: &[u8]) -> Result<(), GrpcError> {
    let text = String::from_utf8_lossy(block);
    let pairs = text
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()));
    status_from_pairs(pairs)
}

/// Build a status from `grpc-*` header pairs. Used for both trailers frames
/// and "trailers-only" responses, where the status is sent in the HTTP headers.
pub(crate) fn status_from_pairs(
    pairs: impl IntoIterator<Item = (String, String)>,
) -> Result<(), GrpcError> {
    let mut err = GrpcError::new(GrpcErrorKind::Status);
    for (key, value) in pairs {
        match key.as_str() {
            "grpc-status" => err.status = value.parse().ok(),
            "grpc-message" => err.message = Some(percent_decode(&value)),
            "grpc-status-details-bin" => err.details = Some(value.into_bytes()),
            _ => {}
        }
    }
    match err.status {
        None | Some(0) => Ok(()),
        Some(_) => Err(err),
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2]))
        {
            out.push(hi << 4 | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use v1::TopFlightsRequest;

    #[test]
    fn roundtrip() {
        let msg = TopFlightsRequest { limit: 10 };
        let encoded = encode_message(&msg);
        assert_eq!(encoded, [0x00, 0, 0, 0, 2, 0x08, 10]);
        let decoded: TopFlightsRequest = parse_data(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn trailers_error() {
        let mut body = vec![0x80];
        let trailers = b"grpc-status:5\r\ngrpc-message:flight%20not%20found\r\n";
        body.extend_from_slice(&(trailers.len() as u32).to_be_bytes());
        body.extend_from_slice(trailers);
        let err = match parse_data::<TopFlightsRequest>(&body) {
            Err(crate::Error::Grpc(e)) => e,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(err.status, Some(5));
        assert_eq!(err.message.as_deref(), Some("flight not found"));
    }

    #[test]
    fn empty_body() {
        let err = parse_data::<TopFlightsRequest>(&[]).unwrap_err();
        assert!(matches!(
            err,
            crate::Error::Grpc(GrpcError {
                kind: GrpcErrorKind::EmptyDataFrame,
                ..
            })
        ));
    }

    #[test]
    fn split_stream() {
        let a = encode_message(&TopFlightsRequest { limit: 1 });
        let b = encode_message(&TopFlightsRequest { limit: 2 });
        let stream: Vec<u8> = [a, b].concat();
        let mut decoder = FrameDecoder::new();
        let mut frames = vec![];
        for chunk in stream.chunks(3) {
            decoder.push(chunk);
            while let Some(frame) = decoder.next_frame() {
                frames.push(frame.unwrap());
            }
        }
        assert_eq!(frames.len(), 2);
        assert_eq!(decoder.remaining(), 0);
    }
}
