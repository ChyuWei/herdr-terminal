//! The reusable Herdr Core client: discovery, one framed connection, calls,
//! events and attach streams. Nothing here draws; `ht probe` is one consumer.

pub mod connection;
pub mod discovery;
pub mod events;
pub mod frames;
pub mod rpc;

pub use connection::{Attached, CoreClient, RunOptions, StreamItem};
pub use events::{EventKind, EventStream, LineBuffer, Notification, parse_notification};
pub use frames::{Frame, FrameDecoder, FrameError, Kind};
pub use rpc::{
    AttachResult, CoreInfo, Display, Hello, Mismatch, ReadResult, RpcError, Snapshot, Terminal,
};

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong while talking to core.
#[derive(Debug)]
pub enum Error {
    /// No herdr-core binary was found.
    NotFound,
    /// The configured binary exists but could not be run.
    Spawn { path: PathBuf, source: io::Error },
    /// The connection itself failed.
    Io(io::Error),
    /// A message from core was not the JSON we expected.
    Json(serde_json::Error),
    /// A frame was malformed (for example, past the 16 MiB cap).
    Frame(FrameError),
    /// Core refused a call.
    Rpc { method: String, error: RpcError },
    /// Core answered, but not in a shape this client can use.
    Protocol(String),
    /// Core cannot serve this client (generation, methods, features).
    Mismatch(Mismatch),
    /// The connection to core ended. The programs may still be running.
    Closed,
    /// Core did not answer a call in time.
    Timeout { method: String, timeout: Duration },
    /// The command line was wrong.
    Usage(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound => write!(
                f,
                "no herdr-core (hc) found; set HERDR_TERMINAL_HC, put hc on PATH, or pass --hc PATH"
            ),
            Error::Spawn { path, source } => {
                write!(f, "cannot run {} connect: {source}", path.display())
            }
            Error::Io(source) => write!(f, "core connection I/O failed: {source}"),
            Error::Json(source) => write!(f, "invalid JSON from core: {source}"),
            Error::Frame(source) => write!(f, "invalid frame from core: {source}"),
            Error::Rpc { method, error } => write!(f, "{method}: {error}"),
            Error::Protocol(message) => write!(f, "{message}"),
            Error::Mismatch(mismatch) => write!(f, "{mismatch}"),
            Error::Closed => write!(f, "lost the connection to core"),
            Error::Timeout { method, timeout } => {
                write!(
                    f,
                    "{method}: no reply from core after {}s",
                    timeout.as_secs()
                )
            }
            Error::Usage(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Spawn { source, .. } => Some(source),
            Error::Io(source) => Some(source),
            Error::Json(source) => Some(source),
            Error::Frame(source) => Some(source),
            Error::Rpc { error, .. } => Some(error),
            Error::Mismatch(mismatch) => Some(mismatch),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(source: io::Error) -> Error {
        Error::Io(source)
    }
}

impl From<serde_json::Error> for Error {
    fn from(source: serde_json::Error) -> Error {
        Error::Json(source)
    }
}

impl From<FrameError> for Error {
    fn from(source: FrameError) -> Error {
        Error::Frame(source)
    }
}

impl From<Mismatch> for Error {
    fn from(mismatch: Mismatch) -> Error {
        Error::Mismatch(mismatch)
    }
}
