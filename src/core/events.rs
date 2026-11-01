//! Event notifications and the snapshot they apply to.
//!
//! In framed mode `events.subscribe` returns a channel carrying JSON lines
//! (`event`, `heartbeat`, `missed`). A DATA frame may split a line and a frame
//! may hold several, so [`LineBuffer`] reassembles them before parsing.
//! M0 keeps the snapshot and parses notifications; applying every terminal
//! change and reconnecting are M2's work.

use super::rpc::{Snapshot, Terminal};
use serde_json::Value;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;

/// The event types core documents, plus one for a future type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    Created,
    Lost,
    Restarted,
    Keep,
    Finished,
    Removed,
    Status,
    Turn,
    Title,
    Labels,
    Size,
    Foreground,
    Doc,
    Other(String),
}

impl EventKind {
    pub fn from_name(name: &str) -> EventKind {
        match name {
            "created" => EventKind::Created,
            "lost" => EventKind::Lost,
            "restarted" => EventKind::Restarted,
            "keep" => EventKind::Keep,
            "finished" => EventKind::Finished,
            "removed" => EventKind::Removed,
            "status" => EventKind::Status,
            "turn" => EventKind::Turn,
            "title" => EventKind::Title,
            "labels" => EventKind::Labels,
            "size" => EventKind::Size,
            "foreground" => EventKind::Foreground,
            "doc" => EventKind::Doc,
            other => EventKind::Other(other.to_owned()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            EventKind::Created => "created",
            EventKind::Lost => "lost",
            EventKind::Restarted => "restarted",
            EventKind::Keep => "keep",
            EventKind::Finished => "finished",
            EventKind::Removed => "removed",
            EventKind::Status => "status",
            EventKind::Turn => "turn",
            EventKind::Title => "title",
            EventKind::Labels => "labels",
            EventKind::Size => "size",
            EventKind::Foreground => "foreground",
            EventKind::Doc => "doc",
            EventKind::Other(name) => name,
        }
    }
}

/// One notification from the event channel.
#[derive(Clone, Debug)]
pub enum Notification {
    Event {
        seq: u64,
        kind: EventKind,
        terminal: Option<String>,
        machine: Option<String>,
        data: Value,
    },
    Heartbeat {
        seq: u64,
    },
    /// The subscriber fell behind: this is a fresh snapshot.
    Missed {
        seq: u64,
        terminals: Vec<Terminal>,
    },
    /// A method this client does not know yet (forward compatibility).
    Other {
        method: String,
        params: Value,
    },
}

/// Reassembles JSON lines from DATA-frame bytes.
#[derive(Debug, Default)]
pub struct LineBuffer {
    pending: Vec<u8>,
}

impl LineBuffer {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Value> {
        self.pending.extend_from_slice(bytes);
        let mut lines = Vec::new();
        let mut start = 0;
        for (at, byte) in self.pending.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            if let Ok(value) = serde_json::from_slice(&self.pending[start..at]) {
                lines.push(value);
            }
            start = at + 1;
        }
        if start > 0 {
            self.pending.drain(..start);
        }
        lines
    }

    /// Bytes of an incomplete line waiting for the rest.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

/// Parses one notification message; `None` when it has no method.
pub fn parse_notification(message: Value) -> Option<Notification> {
    let method = message.get("method")?.as_str()?.to_owned();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match method.as_str() {
        "event" => Some(Notification::Event {
            seq: params.get("seq").and_then(Value::as_u64).unwrap_or(0),
            kind: EventKind::from_name(params.get("type").and_then(Value::as_str).unwrap_or("")),
            terminal: params
                .get("terminal")
                .and_then(Value::as_str)
                .map(str::to_owned),
            machine: params
                .get("machine")
                .and_then(Value::as_str)
                .map(str::to_owned),
            data: params.get("data").cloned().unwrap_or(Value::Null),
        }),
        "heartbeat" => Some(Notification::Heartbeat {
            seq: params.get("seq").and_then(Value::as_u64).unwrap_or(0),
        }),
        "missed" => Some(Notification::Missed {
            seq: params.get("seq").and_then(Value::as_u64).unwrap_or(0),
            terminals: params
                .get("terminals")
                .cloned()
                .and_then(|terminals| serde_json::from_value(terminals).ok())
                .unwrap_or_default(),
        }),
        _ => Some(Notification::Other { method, params }),
    }
}

/// The event subscription: its snapshot plus the notifications as they come.
pub struct EventStream {
    snapshot: Snapshot,
    rx: Receiver<Notification>,
}

impl EventStream {
    pub fn new(snapshot: Snapshot, rx: Receiver<Notification>) -> EventStream {
        EventStream { snapshot, rx }
    }

    /// The channel notifications arrive on.
    pub fn channel(&self) -> Option<u32> {
        self.snapshot.channel
    }

    /// The latest snapshot: `events.subscribe`'s reply, replaced on `missed`.
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn terminals(&self) -> &[Terminal] {
        &self.snapshot.terminals
    }

    /// The highest sequence applied (or in the snapshot).
    pub fn seq(&self) -> u64 {
        self.snapshot.seq
    }

    pub fn recv_timeout(&mut self, timeout: Duration) -> Result<Notification, RecvTimeoutError> {
        let notification = self.rx.recv_timeout(timeout)?;
        self.apply(&notification);
        Ok(notification)
    }

    pub fn try_recv(&mut self) -> Result<Notification, TryRecvError> {
        let notification = self.rx.try_recv()?;
        self.apply(&notification);
        Ok(notification)
    }

    fn apply(&mut self, notification: &Notification) {
        match notification {
            // The core dropped the old sequence: this replaces it wholesale.
            Notification::Missed { seq, terminals } => {
                self.snapshot.seq = *seq;
                self.snapshot.terminals = terminals.clone();
            }
            // Never let a late event move the snapshot backwards.
            Notification::Event { seq, .. } if *seq > self.snapshot.seq => self.snapshot.seq = *seq,
            _ => {}
        }
    }
}

/// A channel pair for tests and for constructing a detached stream.
pub fn channel() -> (Sender<Notification>, Receiver<Notification>) {
    mpsc::channel()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn terminal(address: &str) -> Terminal {
        Terminal {
            address: address.into(),
            state: "live".into(),
            ..Terminal::default()
        }
    }

    #[test]
    fn event_heartbeat_and_missed_notifications_are_parsed() {
        let event = parse_notification(json!({
            "jsonrpc": "2.0",
            "method": "event",
            "params": {"seq": 42, "type": "size", "terminal": "t7", "data": {"cols": 80, "rows": 24}}
        }))
        .expect("event");
        let Notification::Event {
            seq,
            kind,
            terminal,
            machine,
            data,
        } = event
        else {
            panic!("not an event")
        };
        assert_eq!(
            (seq, kind, terminal, machine),
            (42, EventKind::Size, Some("t7".into()), None)
        );
        assert_eq!(data, json!({"cols": 80, "rows": 24}));

        let heartbeat = parse_notification(json!({"method": "heartbeat", "params": {"seq": 43}}));
        assert!(matches!(
            heartbeat,
            Some(Notification::Heartbeat { seq: 43 })
        ));

        let missed = parse_notification(json!({
            "method": "missed",
            "params": {"seq": 57, "terminals": [{"address": "t1", "state": "live"}]}
        }));
        let Some(Notification::Missed { seq, terminals }) = missed else {
            panic!("not a missed snapshot")
        };
        assert_eq!((seq, terminals.len()), (57, 1));

        let future = parse_notification(json!({"method": "event", "params": {"type": "sparkle"}}))
            .expect("event");
        assert!(
            matches!(future, Notification::Event { kind: EventKind::Other(name), .. } if name == "sparkle")
        );
        let unknown =
            parse_notification(json!({"method": "notice", "params": {}})).expect("notice");
        assert!(matches!(unknown, Notification::Other { method, .. } if method == "notice"));
    }

    #[test]
    fn event_lines_survive_arbitrary_frame_boundaries() {
        let first = json!({"method": "event", "params": {"seq": 1, "type": "title", "terminal": "t1", "data": {"title": "hi"}}});
        let second = json!({"method": "heartbeat", "params": {"seq": 2}});
        let wire = format!("{first}\n{second}\n");

        let wire = wire.as_bytes();
        for split in 0..=wire.len() {
            let mut lines = LineBuffer::default();
            let mut got = lines.push(&wire[..split]);
            got.extend(lines.push(&wire[split..]));
            assert_eq!(got, vec![first.clone(), second.clone()], "split at {split}");
            assert_eq!(lines.pending(), 0);
        }

        let mut lines = LineBuffer::default();
        let mut got = Vec::new();
        for chunk in wire.chunks(3) {
            got.extend(lines.push(chunk));
        }
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn a_missed_snapshot_replaces_the_terminals_and_stale_events_do_not_move_seq_back() {
        let (tx, rx) = channel();
        let snapshot = Snapshot {
            seq: 5,
            terminals: vec![terminal("t1")],
            channel: Some(3),
        };
        let mut stream = EventStream::new(snapshot, rx);
        assert_eq!(stream.channel(), Some(3));
        assert_eq!(stream.seq(), 5);

        tx.send(Notification::Missed {
            seq: 20,
            terminals: vec![terminal("t9")],
        })
        .unwrap();
        assert!(matches!(stream.try_recv(), Ok(Notification::Missed { .. })));
        assert_eq!(stream.seq(), 20);
        assert_eq!(
            stream
                .terminals()
                .iter()
                .map(|t| t.address.as_str())
                .collect::<Vec<_>>(),
            ["t9"]
        );

        tx.send(Notification::Event {
            seq: 10,
            kind: EventKind::Title,
            terminal: None,
            machine: None,
            data: Value::Null,
        })
        .unwrap();
        assert!(stream.try_recv().is_ok());
        assert_eq!(
            stream.seq(),
            20,
            "an event below the snapshot changes nothing"
        );

        tx.send(Notification::Heartbeat { seq: 21 }).unwrap();
        assert!(stream.try_recv().is_ok());
        assert_eq!(stream.seq(), 20);
        assert_eq!(stream.terminals().len(), 1);
    }
}
