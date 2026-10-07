use std::fmt;

/// Errors produced by the fr24 client.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("http request failed with status {0}")]
    HttpStatus(reqwest::StatusCode),

    #[error(transparent)]
    Grpc(#[from] GrpcError),

    #[error("failed to decode protobuf message: {0}")]
    ProtoDecode(#[from] prost::DecodeError),

    #[error("failed to decode json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A gRPC error, either from the `grpc-status` trailers or from a malformed
/// length-prefixed message.
///
/// When an application or runtime error occurs during an RPC, a `Status` and
/// `Status-Message` are delivered in `Trailers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrpcError {
    pub kind: GrpcErrorKind,
    /// `grpc-status`, `0-16`
    pub status: Option<u32>,
    /// `grpc-message`, percent-encoded
    pub message: Option<String>,
    /// `grpc-status-details-bin`, a `google.rpc.Status` proto message
    pub details: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrpcErrorKind {
    /// The response body did not contain any `DATA` frame.
    ///
    /// FR24 returns this when e.g. requesting details for a flight that is
    /// no longer live.
    EmptyDataFrame,
    /// The frame had a zero-length payload.
    EmptyPayload,
    /// The compressed flag was set; compression is not implemented.
    Compressed,
    /// The server sent a trailers frame with a non-zero `grpc-status`.
    Status,
    /// The frame header was truncated or had an unknown flag.
    Malformed,
}

impl GrpcError {
    pub(crate) fn new(kind: GrpcErrorKind) -> Self {
        Self {
            kind,
            status: None,
            message: None,
            details: None,
        }
    }
}

impl fmt::Display for GrpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            GrpcErrorKind::EmptyDataFrame => write!(f, "grpc: empty DATA frame")?,
            GrpcErrorKind::EmptyPayload => write!(f, "grpc: empty message payload")?,
            GrpcErrorKind::Compressed => write!(f, "grpc: message is compressed, not implemented")?,
            GrpcErrorKind::Status => write!(f, "grpc: errored")?,
            GrpcErrorKind::Malformed => write!(
                f,
                "grpc: unknown message, possibly wrong encoding or non-grpc"
            )?,
        }
        if let Some(status) = self.status {
            write!(f, " (status {status})")?;
        }
        if let Some(message) = &self.message {
            write!(f, ": {message}")?;
        }
        Ok(())
    }
}

impl std::error::Error for GrpcError {}
