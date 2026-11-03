//! The JSON-RPC messages and core records that this client exchanges.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

/// The protocol generation this client speaks.
pub const GENERATION: u32 = 1;

/// Methods this client needs; a core without one cannot serve it.
pub const REQUIRED_METHODS: &[&str] = &[
    "events.subscribe",
    "terminal.run",
    "terminal.attach",
    "terminal.label",
    "doc.get",
    "doc.set",
];
/// The feature that lets an observer receive RESIZE before paints. Observer
/// attach has no fallback, so this feature is required.
pub const RESIZE_NOTIFICATIONS: &str = "resize_notifications";
/// Features this client needs, as `hello` reports them. Observer attach is the
/// only path and has no fallback, so `resize_notifications` is required.
pub const REQUIRED_FEATURES: &[&str] = &["framed", "attach", "events", RESIZE_NOTIFICATIONS];

/// The oldest Core this client works with.
pub const MINIMUM_CORE: CoreVersion = CoreVersion {
    major: 0,
    minor: 1,
    patch: 17,
};

/// A core version's numeric `major.minor.patch`. A trailing `-suffix` or
/// `+build` is ignored; anything else that is not exactly three numbers is
/// unreadable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoreVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl CoreVersion {
    pub fn parse(text: &str) -> Option<CoreVersion> {
        let head = text.trim().split(['-', '+']).next()?;
        let mut parts = head.split('.');
        let version = CoreVersion {
            major: number(parts.next()?)?,
            minor: number(parts.next()?)?,
            patch: number(parts.next()?)?,
        };
        parts.next().is_none().then_some(version)
    }
}

fn number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl fmt::Display for CoreVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CoreInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// A machine this core added, with the state of its line when the view has
/// one. `machine.add` and `machine.rename` return the profile alone (empty
/// `state`); `machine.list` and `machine.connect` return the view. Unknown
/// fields are ignored and every field defaults, so a partial record never
/// discards the whole answer.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Machine {
    /// What this core calls it: `rohan` in `rohan/t7`.
    #[serde(default)]
    pub name: String,
    /// How it is reached: an ssh target, or (`via`) a command.
    #[serde(default)]
    pub target: String,
    /// `target` is a command that runs a shell line there, not an ssh target.
    #[serde(default)]
    pub via: bool,
    /// Short-lived, for a task; clients may show it apart.
    #[serde(default)]
    pub temporary: bool,
    /// The hc session there; `None` is its default.
    #[serde(default)]
    pub session: Option<String>,
    /// The other core's permanent id.
    #[serde(default)]
    pub id: String,
    /// What that core calls itself (its hostname).
    #[serde(default)]
    pub host: String,
    /// Its hc version when last seen.
    #[serde(default)]
    pub version: String,
    /// `connecting`, `connected`, `needs_login`, `needs_update` or
    /// `unreachable`; empty on a profile reply.
    #[serde(default)]
    pub state: String,
    /// Since when the line is not connected, in milliseconds since the epoch.
    #[serde(default)]
    pub since_ms: Option<u64>,
    /// Why the line is not connected, when core knows.
    #[serde(default)]
    pub why: Option<String>,
}

/// `machine.list`'s reply.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct MachineListResult {
    #[serde(default)]
    pub machines: Vec<Machine>,
}

/// `machine.add` params. `target` is passed through unchanged (an ssh target
/// such as `user@host:port`); `via` is a command instead. The client omits
/// `install`: copying an hc is an interactive CLI concern, and the API reports
/// `hc_missing`/`needs_update` instead.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct MachineAdd {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub temporary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

impl MachineAdd {
    /// Add `target` unchanged; core derives the name and uses the default
    /// session.
    pub fn target(target: impl Into<String>) -> MachineAdd {
        MachineAdd {
            target: Some(target.into()),
            ..MachineAdd::default()
        }
    }
}

/// The `machine.connect`/`machine.rm` params.
pub fn machine_params(machine: &str) -> Value {
    serde_json::json!({"machine": machine})
}

/// The `machine.rename` params. Only this core's name changes; addresses with
/// the old prefix stop working.
pub fn machine_rename_params(machine: &str, new_name: &str) -> Value {
    serde_json::json!({"machine": machine, "new_name": new_name})
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
    /// Whether this core advertises a method. Optional API such as the
    /// machine manager is gated on the reply, never on a version number.
    pub fn has_method(&self, method: &str) -> bool {
        self.methods.iter().any(|have| have == method)
    }

    /// Whether this core advertises a feature. Optional capabilities are
    /// gated on the reply, never on a version number.
    pub fn supports(&self, feature: &str) -> bool {
        self.features.iter().any(|have| have == feature)
    }

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
        match CoreVersion::parse(&self.version) {
            Some(have) if have >= MINIMUM_CORE => {}
            Some(have) => {
                return Err(Mismatch::VersionTooOld {
                    have,
                    want: MINIMUM_CORE,
                });
            }
            None => {
                return Err(Mismatch::VersionUnreadable {
                    have: self.version.clone(),
                    want: MINIMUM_CORE,
                });
            }
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
            .filter(|feature| !self.supports(feature))
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
    Generation {
        have: u32,
        want: u32,
    },
    VersionTooOld {
        have: CoreVersion,
        want: CoreVersion,
    },
    VersionUnreadable {
        have: String,
        want: CoreVersion,
    },
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
            Mismatch::VersionTooOld { have, want } => write!(
                f,
                "core {have} is older than the minimum supported {want}; update herdr-core \
                 (for example with `hc update`)"
            ),
            Mismatch::VersionUnreadable { have, want } => write!(
                f,
                "core reported an unreadable version \"{have}\"; this client needs herdr-core \
                 {want} or newer (update with `hc update`)"
            ),
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
    /// When core created the terminal, in milliseconds since the epoch. A
    /// `terminal.run` reply may omit it; a snapshot record carries it.
    #[serde(default)]
    pub created_ms: Option<u64>,
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

/// `doc.get`'s reply: the opaque value and the version it belongs to. An
/// absent document is `value: null, version: 0`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct DocGetResult {
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub version: u64,
}

/// `doc.set`'s reply: the version the write created.
#[derive(Clone, Debug, Deserialize)]
pub struct DocSetResult {
    pub version: u64,
}

/// `terminal.attach`'s reply: the stream channel and the terminal record.
#[derive(Clone, Debug, Deserialize)]
pub struct AttachResult {
    pub channel: u32,
    #[serde(default)]
    pub terminal: Terminal,
    /// True only when an observer asked for resize notifications and the
    /// whole path confirmed them. An observer whose reply is absent or false
    /// is an error; there is no normal-attach fallback.
    #[serde(default)]
    pub resize_notifications: bool,
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
            version: "0.1.17".into(),
            framed,
            methods: methods.iter().map(|m| (*m).to_owned()).collect(),
            features: features.iter().map(|f| (*f).to_owned()).collect(),
            ..Hello::default()
        }
    }

    fn full() -> Hello {
        hello(GENERATION, true, REQUIRED_METHODS, REQUIRED_FEATURES)
    }

    fn at_version(version: &str) -> Hello {
        Hello {
            version: version.into(),
            ..full()
        }
    }

    #[test]
    fn optional_features_are_read_from_hello_not_from_a_version() {
        let bare = full();
        assert!(bare.supports("framed"));
        assert!(bare.supports(RESIZE_NOTIFICATIONS));
        assert!(!bare.supports("automatic_input"));

        let supporting = hello(
            GENERATION,
            true,
            REQUIRED_METHODS,
            &[
                "framed",
                "attach",
                "events",
                RESIZE_NOTIFICATIONS,
                "automatic_input",
            ],
        );
        assert!(supporting.supports("automatic_input"));
        assert_eq!(supporting.check(), Ok(()));
    }

    #[test]
    fn the_minimum_core_version_is_enforced() {
        for version in [
            "0.1.17",
            "0.1.17-9",
            "0.1.17+build.2",
            "0.1.17-rc.1+any",
            " 0.1.17 ",
            "0.1.18",
            "0.2.0",
            "1.0.0",
        ] {
            assert_eq!(at_version(version).check(), Ok(()), "{version:?}");
        }

        assert_eq!(
            at_version("0.1.16").check(),
            Err(Mismatch::VersionTooOld {
                have: CoreVersion {
                    major: 0,
                    minor: 1,
                    patch: 16
                },
                want: MINIMUM_CORE,
            })
        );
        assert_eq!(
            at_version("0.0.99").check(),
            Err(Mismatch::VersionTooOld {
                have: CoreVersion {
                    major: 0,
                    minor: 0,
                    patch: 99
                },
                want: MINIMUM_CORE,
            })
        );
        for version in ["", "dev", "0.1", "0.1.x", "0.1.17.1", "v0.1.17", "0.1.17x"] {
            assert!(
                matches!(
                    at_version(version).check(),
                    Err(Mismatch::VersionUnreadable { .. })
                ),
                "{version:?} must be unreadable"
            );
        }

        let message = at_version("0.1.16").check().unwrap_err().to_string();
        assert!(message.contains("0.1.16"), "{message}");
        assert!(message.contains("0.1.17"), "{message}");
        assert!(message.contains("hc update"), "{message}");

        let message = at_version("").check().unwrap_err().to_string();
        assert!(message.contains("unreadable"), "{message}");
        assert!(message.contains("0.1.17"), "{message}");
    }

    #[test]
    fn a_version_is_three_numeric_parts_with_an_ignored_suffix() {
        let version = |major, minor, patch| CoreVersion {
            major,
            minor,
            patch,
        };
        assert_eq!(CoreVersion::parse("0.1.17"), Some(version(0, 1, 17)));
        assert_eq!(
            CoreVersion::parse("12.34.56-rc.1"),
            Some(version(12, 34, 56))
        );
        assert_eq!(
            CoreVersion::parse("12.34.56+deadbeef"),
            Some(version(12, 34, 56))
        );
        assert_eq!(CoreVersion::parse(" 12.34.56 "), Some(version(12, 34, 56)));
        for bad in [
            "", " ", "dev", "0", "0.1", "0.1.x", "0.1.17.1", "-1.2.3", "1.2.3. ", "0.1.17 x",
        ] {
            assert_eq!(CoreVersion::parse(bad), None, "{bad:?}");
        }
        assert!(version(0, 1, 17) < version(0, 1, 18));
        assert!(version(0, 2, 0) > version(0, 1, 99));
    }

    #[test]
    fn an_attach_reply_confirms_resize_notifications_only_when_it_says_so() {
        let bare: AttachResult =
            serde_json::from_value(serde_json::json!({"channel": 7})).expect("reply");
        assert!(!bare.resize_notifications);
        let confirmed: AttachResult = serde_json::from_value(serde_json::json!({
            "channel": 7,
            "resize_notifications": true,
        }))
        .expect("reply");
        assert!(confirmed.resize_notifications);
    }

    #[test]
    fn a_machine_view_keeps_every_documented_fact_and_ignores_new_ones() {
        let machine: Machine = serde_json::from_value(serde_json::json!({
            "name": "rohan",
            "target": "can@10.0.0.7:2222",
            "via": false,
            "temporary": true,
            "session": "work",
            "id": "core-id",
            "host": "rohan-host",
            "version": "0.1.18",
            "state": "needs_login",
            "since_ms": 1234,
            "why": "login required",
            "future_field": {"anything": true},
        }))
        .expect("machine view");
        assert_eq!(machine.name, "rohan");
        assert_eq!(machine.target, "can@10.0.0.7:2222");
        assert!(!machine.via);
        assert!(machine.temporary);
        assert_eq!(machine.session.as_deref(), Some("work"));
        assert_eq!(machine.id, "core-id");
        assert_eq!(machine.host, "rohan-host");
        assert_eq!(machine.version, "0.1.18");
        assert_eq!(machine.state, "needs_login");
        assert_eq!(machine.since_ms, Some(1234));
        assert_eq!(machine.why.as_deref(), Some("login required"));

        let empty: Machine = serde_json::from_value(serde_json::json!({})).expect("partial");
        assert_eq!(empty, Machine::default());

        let list: MachineListResult = serde_json::from_value(serde_json::json!({
            "machines": [{"name": "one"}, {"name": "two", "state": "connected"}],
            "unknown": 1,
        }))
        .expect("list");
        assert_eq!(list.machines.len(), 2);
        assert_eq!(list.machines[1].state, "connected");
    }

    #[test]
    fn machine_add_passes_the_target_through_unchanged() {
        let target = "can@10.0.0.7:2222";
        let params = serde_json::to_value(MachineAdd::target(target)).expect("params");
        assert_eq!(
            params,
            serde_json::json!({"target": target}),
            "the target is not parsed or rewritten and unset options are absent"
        );
        assert_eq!(params["target"], serde_json::json!(target));
        assert!(params.get("via").is_none());
        assert!(params.get("temporary").is_none());
        assert!(params.get("name").is_none());
        assert!(params.get("session").is_none());

        let full = MachineAdd {
            target: Some(target.into()),
            via: None,
            temporary: true,
            name: Some("rohan".into()),
            session: Some("work".into()),
        };
        assert_eq!(
            serde_json::to_value(full).expect("params"),
            serde_json::json!({
                "target": target,
                "temporary": true,
                "name": "rohan",
                "session": "work",
            })
        );

        let via = MachineAdd {
            target: None,
            via: Some("fly ssh console -a app --pty=false -C".into()),
            ..MachineAdd::default()
        };
        assert_eq!(
            serde_json::to_value(via).expect("params"),
            serde_json::json!({"via": "fly ssh console -a app --pty=false -C"})
        );
    }

    #[test]
    fn machine_requests_name_the_machine_exactly() {
        assert_eq!(
            machine_params("rohan"),
            serde_json::json!({"machine": "rohan"})
        );
        assert_eq!(
            machine_rename_params("rohan", "vps"),
            serde_json::json!({"machine": "rohan", "new_name": "vps"})
        );
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
        assert_eq!(
            methods,
            ["terminal.attach", "terminal.label", "doc.get", "doc.set"]
        );
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
        assert_eq!(features, ["events", RESIZE_NOTIFICATIONS]);
    }
}
