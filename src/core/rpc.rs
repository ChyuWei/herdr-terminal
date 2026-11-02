//! The JSON-RPC messages and core records that this client exchanges.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

/// The protocol generation this client speaks.
pub const GENERATION: u32 = 1;

/// Methods this client needs; a core without one cannot serve it.
pub const REQUIRED_METHODS: &[&str] = &["events.subscribe", "terminal.run", "terminal.attach"];
/// Features this client needs, as `hello` reports them.
pub const REQUIRED_FEATURES: &[&str] = &["framed", "attach", "events"];

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CoreInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// The `hello` reply: what this core is and what it can do.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Hello {
    #[serde(default)]
    pub generation: u32,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub core: CoreInfo,
    #[serde(default)]
    pub framed: bool,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
}

impl Hello {
    /// Whether this core can serve this client. Feature-gate on the reply;
    /// protocol generation 1 only grows.
    pub fn check(&self) -> Result<(), Mismatch> {
        if !self.framed {
            return Err(Mismatch::NotFramed);
        }
        if self.generation != GENERATION {
            return Err(Mismatch::Generation {
                have: self.generation,
                want: GENERATION,
            });
        }
        let missing: Vec<String> = REQUIRED_METHODS
            .iter()
            .filter(|method| !self.methods.iter().any(|have| have == *method))
            .map(|method| (*method).to_owned())
            .collect();
        if !missing.is_empty() {
            return Err(Mismatch::MissingMethods(missing));
        }
        let missing: Vec<String> = REQUIRED_FEATURES
            .iter()
            .filter(|feature| !self.features.iter().any(|have| have == *feature))
            .map(|feature| (*feature).to_owned())
            .collect();
        if !missing.is_empty() {
            return Err(Mismatch::MissingFeatures(missing));
        }
        Ok(())
    }
}

/// Why a running core cannot serve this client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mismatch {
    NotFramed,
    Generation { have: u32, want: u32 },
    MissingMethods(Vec<String>),
    MissingFeatures(Vec<String>),
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mismatch::NotFramed => write!(f, "core did not switch the connection to framed mode"),
            Mismatch::Generation { have, want } => {
                write!(
                    f,
                    "core speaks protocol generation {have}; this client speaks generation {want}"
                )
            }
            Mismatch::MissingMethods(methods) => {
                write!(f, "core lacks required methods: {}", methods.join(", "))
            }
            Mismatch::MissingFeatures(features) => {
                write!(f, "core lacks required features: {}", features.join(", "))
            }
        }
    }
}

impl std::error::Error for Mismatch {}

/// A JSON-RPC error, including herdr-core's stable `data.kind` and hint.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RpcError {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub data: Value,
}

impl RpcError {
    pub fn from_json(error: &Value) -> RpcError {
        serde_json::from_value(error.clone()).unwrap_or_else(|_| RpcError {
            code: 0,
            message: error.to_string(),
            data: Value::Null,
        })
    }

    pub fn lost() -> RpcError {
        RpcError {
            code: 0,
            message: "lost the connection to core".into(),
            data: serde_json::json!({"kind": "lost"}),
        }
    }

    /// herdr-core's stable error kind (`not_found`, `needs_framed`, ...).
    pub fn kind(&self) -> Option<&str> {
        self.data.get("kind").and_then(Value::as_str)
    }

    pub fn hint(&self) -> Option<&str> {
        self.data.get("hint").and_then(Value::as_str)
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(hint) = self.hint().filter(|hint| !hint.is_empty()) {
            write!(f, " ({hint})")?;
        }
        Ok(())
    }
}

impl std::error::Error for RpcError {}

/// A terminal record. Only the fields this milestone uses are typed; core may
/// add more and they are ignored. Missing fields default so a partial record
/// never discards a whole event.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Terminal {
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: String,
    /// Replace a lost program with a fresh shell in place.
    #[serde(default)]
    pub keep: bool,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub rows: u16,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub signal: Option<i64>,
    #[serde(default)]
    pub finished_ms: Option<u64>,
    /// The program's own OSC 7501 word, stored as reported.
    #[serde(default)]
    pub status: Option<Status>,
    #[serde(default)]
    pub status_ms: Option<u64>,
    #[serde(default)]
    pub records: BTreeMap<String, Status>,
    #[serde(default)]
    pub turn: u64,
    #[serde(default)]
    pub reported: bool,
    /// What runs in it now; null once it has finished.
    #[serde(default)]
    pub foreground: Option<Foreground>,
}

impl Terminal {
    pub fn is_live(&self) -> bool {
        self.state == "live"
    }
}

/// The root or a named OSC 7501 record, as core stores it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Status {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub progress: Option<u8>,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub msg: String,
}

/// The foreground process group's leader and its folder.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Foreground {
    #[serde(default)]
    pub pid: i32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: String,
}

/// `events.subscribe`'s reply: the snapshot and, when framed, its channel.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub terminals: Vec<Terminal>,
    #[serde(default)]
    pub channel: Option<u32>,
}

/// `terminal.attach`'s reply: the stream channel and the terminal record.
#[derive(Clone, Debug, Deserialize)]
pub struct AttachResult {
    pub channel: u32,
    #[serde(default)]
    pub terminal: Terminal,
}

/// `terminal.read`'s reply: the screen text (or history above it).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ReadResult {
    #[serde(default)]
    pub text: String,
    /// False when part of what was asked for may be missing.
    #[serde(default)]
    pub complete: bool,
    /// `screen`, `history`, or `stitched`.
    #[serde(default)]
    pub how: String,
    /// Why only the screen, when it is.
    #[serde(default)]
    pub why: Option<String>,
}

/// What a viewer's own terminal looks like, for core to answer display
/// queries. Only sent when it changes (FACTS).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Display {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell: Option<[u32; 2]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(generation: u32, framed: bool, methods: &[&str], features: &[&str]) -> Hello {
        Hello {
            generation,
            version: "0.1.16".into(),
            framed,
            methods: methods.iter().map(|m| (*m).to_owned()).collect(),
            features: features.iter().map(|f| (*f).to_owned()).collect(),
            ..Hello::default()
        }
    }

    fn full() -> Hello {
        hello(GENERATION, true, REQUIRED_METHODS, REQUIRED_FEATURES)
    }

    #[test]
    fn a_core_that_cannot_serve_the_client_is_named_precisely() {
        assert_eq!(full().check(), Ok(()));

        assert_eq!(
            hello(1, false, REQUIRED_METHODS, REQUIRED_FEATURES).check(),
            Err(Mismatch::NotFramed)
        );
        assert_eq!(
            hello(2, true, REQUIRED_METHODS, REQUIRED_FEATURES).check(),
            Err(Mismatch::Generation { have: 2, want: 1 })
        );

        let missing = hello(
            GENERATION,
            true,
            &["events.subscribe", "terminal.run"],
            REQUIRED_FEATURES,
        );
        let Err(Mismatch::MissingMethods(methods)) = missing.check() else {
            panic!("missing method not detected")
        };
        assert_eq!(methods, ["terminal.attach"]);
        assert!(
            missing
                .check()
                .unwrap_err()
                .to_string()
                .contains("terminal.attach")
        );

        let missing = hello(GENERATION, true, REQUIRED_METHODS, &["framed", "attach"]);
        let Err(Mismatch::MissingFeatures(features)) = missing.check() else {
            panic!("missing feature not detected")
        };
        assert_eq!(features, ["events"]);
    }
}
