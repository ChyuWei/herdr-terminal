//! The pure terminal model: core's snapshot and events, without IO.
//!
//! [`Model`] is fed the notifications of one `events.subscribe` stream. It
//! owns sequence ordering (`seq`), applies every documented event type to the
//! terminal records, and never draws: the screen asks it for the record it
//! shows. Keeping this pure makes lifecycle rules testable without a core, a
//! PTY or a terminal.

use crate::core::{EventKind, Notification, Snapshot, Terminal};
use serde_json::Value;
use std::collections::BTreeMap;

/// The terminals core last reported, and the sequence they are current at.
#[derive(Clone, Debug, Default)]
pub struct Model {
    seq: u64,
    terminals: BTreeMap<String, Terminal>,
    /// Document versions seen on the stream; M4 owns reading their values.
    docs: BTreeMap<String, u64>,
}

impl Model {
    /// Starts from an `events.subscribe` snapshot.
    pub fn from_snapshot(snapshot: &Snapshot) -> Model {
        let mut model = Model::default();
        model.reset(snapshot);
        model
    }

    /// Replaces the whole state with a fresh snapshot (`events.subscribe` or
    /// `missed`).
    pub fn reset(&mut self, snapshot: &Snapshot) {
        self.seq = snapshot.seq;
        self.terminals = snapshot
            .terminals
            .iter()
            .map(|terminal| (terminal.address.clone(), terminal.clone()))
            .collect();
    }

    /// The highest sequence applied, or the snapshot's sequence.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn terminals(&self) -> impl Iterator<Item = &Terminal> {
        self.terminals.values()
    }

    pub fn terminal(&self, address: &str) -> Option<&Terminal> {
        self.terminals.get(address)
    }

    /// Inserts or replaces one record, used for records fetched directly
    /// (`terminal.get`) that the snapshot did not carry. Later events still
    /// win; this only fills a gap.
    pub fn upsert(&mut self, terminal: Terminal) {
        if !terminal.address.is_empty() {
            self.terminals.insert(terminal.address.clone(), terminal);
        }
    }

    /// Replaces one terminal's labels for an optimistic shared-label update.
    /// The next `labels` event from core is authoritative.
    pub fn set_labels(&mut self, address: &str, labels: BTreeMap<String, String>) {
        if let Some(record) = self.terminals.get_mut(address) {
            record.labels = labels;
        }
    }

    /// Document versions seen so far, keyed by `name` or `machine/name`.
    pub fn docs(&self) -> &BTreeMap<String, u64> {
        &self.docs
    }

    /// Applies one notification. Returns whether a terminal record changed
    /// (the UI needs to redraw); heartbeats and doc events do not.
    ///
    /// An event at or below the sequence already applied is stale and is
    /// dropped. Unknown event types still advance `seq` so the stream does not
    /// appear to move backwards, but change no terminal.
    pub fn apply(&mut self, notification: &Notification) -> bool {
        match notification {
            Notification::Heartbeat { seq } => {
                self.advance(*seq);
                false
            }
            Notification::Missed { seq, terminals } => {
                self.seq = *seq;
                self.terminals = keyed(terminals);
                true
            }
            Notification::Event {
                seq,
                kind,
                terminal,
                machine,
                data,
            } => {
                if *seq <= self.seq {
                    return false;
                }
                self.seq = *seq;
                self.apply_event(kind, terminal.as_deref(), machine.as_deref(), data)
            }
            // A method this client does not know (a future notification).
            Notification::Other { .. } => false,
        }
    }

    fn advance(&mut self, seq: u64) {
        if seq > self.seq {
            self.seq = seq;
        }
    }

    fn apply_event(
        &mut self,
        kind: &EventKind,
        address: Option<&str>,
        machine: Option<&str>,
        data: &Value,
    ) -> bool {
        // A doc event has no terminal; everything else needs one.
        let Some(address) = address else {
            if let EventKind::Doc = kind {
                self.apply_doc(machine, data);
            }
            return false;
        };
        match kind {
            EventKind::Created | EventKind::Restarted => {
                let Some(record) = Self::record_from(data, Some(address)) else {
                    return false;
                };
                self.terminals.insert(record.address.clone(), record);
                true
            }
            EventKind::Lost => {
                let Some(record) = self.terminals.get_mut(address) else {
                    return false;
                };
                // A kept terminal stays live without a program until
                // `restarted` gives it a fresh shell.
                record.reason = string(data, "reason");
                record.exit_code = integer(data, "exit_code");
                record.signal = integer(data, "signal");
                record.finished_ms = data.get("finished_ms").and_then(Value::as_u64);
                record.pid = None;
                record.foreground = None;
                true
            }
            EventKind::Keep => {
                let Some(record) = self.terminals.get_mut(address) else {
                    return false;
                };
                record.keep = data.get("keep").and_then(Value::as_bool).unwrap_or(false);
                true
            }
            EventKind::Finished => {
                let Some(record) = self.terminals.get_mut(address) else {
                    return false;
                };
                record.state = "finished".into();
                record.reason = string(data, "reason");
                record.exit_code = integer(data, "exit_code");
                record.signal = integer(data, "signal");
                record.finished_ms = data.get("finished_ms").and_then(Value::as_u64);
                record.foreground = None;
                true
            }
            EventKind::Removed => self.terminals.remove(address).is_some(),
            EventKind::Status => {
                let Some(record) = self.terminals.get_mut(address) else {
                    return false;
                };
                record.status = data
                    .get("status")
                    .filter(|status| !status.is_null())
                    .and_then(|status| serde_json::from_value(status.clone()).ok());
                record.records = data
                    .get("records")
                    .and_then(|records| serde_json::from_value(records.clone()).ok())
                    .unwrap_or_default();
                record.reported = data
                    .get("reported")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                true
            }
            EventKind::Turn => {
                let Some(record) = self.terminals.get_mut(address) else {
                    return false;
                };
                record.turn = data
                    .get("turn")
                    .and_then(Value::as_u64)
                    .unwrap_or(record.turn);
                true
            }
            EventKind::Title => {
                let Some(record) = self.terminals.get_mut(address) else {
                    return false;
                };
                record.title = data
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                true
            }
            EventKind::Labels => {
                let mut changed = false;
                if let Some(record) = self.terminals.get_mut(address) {
                    record.labels = labels(data);
                    changed = true;
                }
                // A terminal that newly matches the subscription arrives with
                // its whole record.
                if let Some(record) = data
                    .get("record")
                    .filter(|record| !record.is_null())
                    .and_then(|record| Self::record_from(record, Some(address)))
                {
                    self.terminals.insert(record.address.clone(), record);
                    changed = true;
                }
                changed
            }
            EventKind::Size => {
                let Some(record) = self.terminals.get_mut(address) else {
                    return false;
                };
                record.cols = data
                    .get("cols")
                    .and_then(Value::as_u64)
                    .map(|cols| cols as u16)
                    .unwrap_or(record.cols);
                record.rows = data
                    .get("rows")
                    .and_then(Value::as_u64)
                    .map(|rows| rows as u16)
                    .unwrap_or(record.rows);
                true
            }
            EventKind::Foreground => {
                let Some(record) = self.terminals.get_mut(address) else {
                    return false;
                };
                record.foreground = data
                    .get("foreground")
                    .filter(|foreground| !foreground.is_null())
                    .and_then(|foreground| serde_json::from_value(foreground.clone()).ok())
                    .or_else(|| {
                        // Core documents the data itself as the foreground
                        // (or null), not wrapped in an object.
                        if data.is_null() {
                            None
                        } else {
                            serde_json::from_value(data.clone()).ok()
                        }
                    });
                true
            }
            EventKind::Doc => false,
            EventKind::Other(_) => false,
        }
    }

    fn apply_doc(&mut self, machine: Option<&str>, data: &Value) {
        let Some(name) = data.get("name").and_then(Value::as_str) else {
            return;
        };
        let key = match machine {
            Some(machine) => format!("{machine}/{name}"),
            None => name.to_owned(),
        };
        if let Some(version) = data.get("version").and_then(Value::as_u64) {
            self.docs.insert(key, version);
        }
    }

    /// The whole terminal record carried by an event, keyed by its address.
    fn record_from(data: &Value, address: Option<&str>) -> Option<Terminal> {
        let mut record: Terminal = serde_json::from_value(data.clone()).ok()?;
        if record.address.is_empty() {
            record.address = address?.to_owned();
        }
        if record.address.is_empty() {
            return None;
        }
        Some(record)
    }
}

fn keyed(terminals: &[Terminal]) -> BTreeMap<String, Terminal> {
    terminals
        .iter()
        .map(|terminal| (terminal.address.clone(), terminal.clone()))
        .collect()
}

fn string(data: &Value, key: &str) -> Option<String> {
    data.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn integer(data: &Value, key: &str) -> Option<i64> {
    data.get(key).and_then(Value::as_i64)
}

fn labels(data: &Value) -> BTreeMap<String, String> {
    data.get("labels")
        .and_then(|labels| serde_json::from_value(labels.clone()).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::EventKind;
    use serde_json::json;

    fn terminal(address: &str) -> Terminal {
        Terminal {
            address: address.into(),
            state: "live".into(),
            pid: Some(42),
            keep: true,
            ..Terminal::default()
        }
    }

    fn snapshot(seq: u64, terminals: Vec<Terminal>) -> Snapshot {
        Snapshot {
            seq,
            terminals,
            channel: Some(3),
        }
    }

    fn event(seq: u64, kind: EventKind, terminal: &str, data: Value) -> Notification {
        Notification::Event {
            seq,
            kind,
            terminal: Some(terminal.into()),
            machine: None,
            data,
        }
    }

    fn model() -> Model {
        Model::from_snapshot(&snapshot(1, vec![terminal("t1")]))
    }

    #[test]
    fn a_stale_event_is_dropped_and_a_heartbeat_only_moves_the_sequence() {
        let mut model = model();
        assert!(!model.apply(&event(1, EventKind::Title, "t1", json!({"title": "stale"}))));
        assert_eq!(model.terminal("t1").unwrap().title, "");

        assert!(!model.apply(&Notification::Heartbeat { seq: 9 }));
        assert_eq!(model.seq(), 9);
        assert_eq!(model.terminal("t1").unwrap().title, "");

        assert!(model.apply(&event(10, EventKind::Title, "t1", json!({"title": "new"}))));
        assert_eq!(model.terminal("t1").unwrap().title, "new");
        assert_eq!(model.seq(), 10);
    }

    #[test]
    fn created_and_restarted_replace_the_whole_record() {
        let mut model = model();
        assert!(model.apply(&event(
            2,
            EventKind::Created,
            "t2",
            json!({"address": "t2", "state": "live", "pid": 7, "keep": true})
        )));
        assert_eq!(model.terminal("t2").unwrap().pid, Some(7));

        assert!(model.apply(&event(
            3,
            EventKind::Restarted,
            "t1",
            json!({"address": "t1", "state": "live", "pid": 99, "keep": true, "title": "fresh"})
        )));
        let record = model.terminal("t1").unwrap();
        assert_eq!((record.pid, record.title.as_str()), (Some(99), "fresh"));
    }

    #[test]
    fn lost_keeps_a_kept_terminal_live_without_a_program() {
        let mut model = model();
        model.terminal_mut_for_test("t1").foreground = Some(crate::core::Foreground {
            pid: 42,
            name: "sh".into(),
            ..crate::core::Foreground::default()
        });
        assert!(model.apply(&event(
            2,
            EventKind::Lost,
            "t1",
            json!({"reason": "signaled", "exit_code": null, "signal": 9, "finished_ms": null})
        )));
        let record = model.terminal("t1").unwrap();
        assert_eq!(record.state, "live");
        assert_eq!((record.pid, record.signal), (None, Some(9)));
        assert_eq!(record.reason.as_deref(), Some("signaled"));
        assert!(record.foreground.is_none());
    }

    #[test]
    fn finished_sets_the_state_and_drops_the_foreground() {
        let mut model = model();
        model.terminal_mut_for_test("t1").foreground = Some(crate::core::Foreground::default());
        assert!(model.apply(&event(
            2,
            EventKind::Finished,
            "t1",
            json!({"reason": "exited", "exit_code": 7, "signal": null, "finished_ms": 1234})
        )));
        let record = model.terminal("t1").unwrap();
        assert_eq!(record.state, "finished");
        assert_eq!(
            (record.exit_code, record.finished_ms),
            (Some(7), Some(1234))
        );
        assert!(record.foreground.is_none());
    }

    #[test]
    fn keep_status_turn_title_labels_size_and_foreground_are_applied() {
        let mut model = model();
        assert!(model.apply(&event(2, EventKind::Keep, "t1", json!({"keep": false}))));
        assert!(!model.terminal("t1").unwrap().keep);

        let status = json!({
            "status": {"state": "working", "app": "claude", "progress": 30},
            "records": {"build": {"state": "done"}},
            "reported": true
        });
        assert!(model.apply(&event(3, EventKind::Status, "t1", status)));
        let record = model.terminal("t1").unwrap();
        assert_eq!(record.status.as_ref().unwrap().state, "working");
        assert_eq!(record.records["build"].state, "done");
        assert!(record.reported);
        // `status: null` clears it.
        assert!(model.apply(&event(
            4,
            EventKind::Status,
            "t1",
            json!({"status": null, "records": {}, "reported": true})
        )));
        assert!(model.terminal("t1").unwrap().status.is_none());

        assert!(model.apply(&event(
            5,
            EventKind::Turn,
            "t1",
            json!({"turn": 4, "state": "done"})
        )));
        assert_eq!(model.terminal("t1").unwrap().turn, 4);

        assert!(model.apply(&event(6, EventKind::Title, "t1", json!({"title": "htop"}))));
        assert_eq!(model.terminal("t1").unwrap().title, "htop");

        assert!(model.apply(&event(
            7,
            EventKind::Labels,
            "t1",
            json!({"labels": {"herdr.client": "herdr-terminal", "x": "y"}})
        )));
        assert_eq!(model.terminal("t1").unwrap().labels["x"], "y");

        assert!(model.apply(&event(
            8,
            EventKind::Size,
            "t1",
            json!({"cols": 100, "rows": 40})
        )));
        assert_eq!(
            (
                model.terminal("t1").unwrap().cols,
                model.terminal("t1").unwrap().rows
            ),
            (100, 40)
        );

        assert!(model.apply(&event(
            9,
            EventKind::Foreground,
            "t1",
            json!({"pid": 7, "name": "vim", "argv": ["vim", "x"], "cwd": "/tmp"})
        )));
        let foreground = model.terminal("t1").unwrap().foreground.as_ref().unwrap();
        assert_eq!((foreground.pid, foreground.name.as_str()), (7, "vim"));
        assert!(model.apply(&event(10, EventKind::Foreground, "t1", Value::Null)));
        assert!(model.terminal("t1").unwrap().foreground.is_none());
    }

    #[test]
    fn removed_drops_the_record_and_labels_can_add_a_new_one() {
        let mut model = model();
        assert!(model.apply(&event(
            2,
            EventKind::Labels,
            "t9",
            json!({
                "labels": {"herdr.client": "herdr-terminal"},
                "record": {"address": "t9", "state": "live", "keep": true}
            })
        )));
        assert!(model.terminal("t9").is_some());

        assert!(model.apply(&event(3, EventKind::Removed, "t1", json!({}))));
        assert!(model.terminal("t1").is_none());
        assert!(!model.apply(&event(4, EventKind::Removed, "t1", json!({}))));
    }

    #[test]
    fn unknown_types_advance_the_sequence_without_changing_a_terminal() {
        let mut model = model();
        assert!(!model.apply(&event(
            5,
            EventKind::Other("sparkle".into()),
            "t1",
            json!({})
        )));
        assert_eq!(model.seq(), 5);
        assert_eq!(model.terminals().count(), 1);
        assert!(!model.apply(&Notification::Other {
            method: "notice".into(),
            params: Value::Null,
        }));
    }

    #[test]
    fn doc_events_only_record_the_version() {
        let mut model = model();
        let mut notification = event(
            2,
            EventKind::Doc,
            "",
            json!({"name": "herdr-terminal", "version": 3}),
        );
        let Notification::Event { terminal, .. } = &mut notification else {
            unreachable!()
        };
        *terminal = None;
        assert!(!model.apply(&notification));
        assert_eq!(model.docs()["herdr-terminal"], 3);
        assert_eq!(model.terminals().count(), 1);
    }

    #[test]
    fn missed_replaces_everything_and_later_events_still_apply() {
        let mut model = model();
        assert!(model.apply(&Notification::Missed {
            seq: 20,
            terminals: vec![terminal("t9")],
        }));
        assert_eq!(model.seq(), 20);
        assert!(model.terminal("t1").is_none());
        assert_eq!(model.terminal("t9").unwrap().pid, Some(42));

        assert!(!model.apply(&event(20, EventKind::Title, "t9", json!({"title": "old"}))));
        assert!(model.apply(&event(21, EventKind::Title, "t9", json!({"title": "new"}))));
        assert_eq!(model.terminal("t9").unwrap().title, "new");
    }

    #[test]
    fn an_update_for_an_unknown_terminal_is_ignored() {
        let mut model = model();
        assert!(!model.apply(&event(
            2,
            EventKind::Size,
            "t404",
            json!({"cols": 1, "rows": 1})
        )));
        assert!(!model.apply(&event(3, EventKind::Lost, "t404", json!({}))));
        assert_eq!(model.terminals().count(), 1);
    }

    /// Test-only access for setting fields the event stream normally writes.
    impl Model {
        fn terminal_mut_for_test(&mut self, address: &str) -> &mut Terminal {
            self.terminals.get_mut(address).expect("terminal")
        }
    }
}
