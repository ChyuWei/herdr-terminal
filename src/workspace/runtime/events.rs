//! Attach-byte draining, event application and lifecycle outcomes: finished,
//! lost, restarted, removed and `missed` all resolve to a pane state here.

use super::{ATTACH_RETRY, DRAIN_BYTES, DRAIN_TIME, EVENT_BATCH, Screen};
use crate::core::{EventKind, Notification, Result, StreamItem};
use crate::screen::lifecycle::{self, Disposition};
use crate::session;
use crate::tui::vt_error;
use crate::vt::Vt;
use crate::workspace::model::{DOC_NAME, PaneId};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

/// A pane's display data, read from the event model only.
struct Displayed {
    pane: PaneId,
    title: String,
    status: Option<String>,
    labels: BTreeMap<String, String>,
}

impl Screen<'_> {
    /// Drains each pane's stream a bounded amount, feeding its VT and paying
    /// credit for every consumed byte.
    pub(super) fn drain_panes(&mut self) -> Result<bool> {
        let panes: Vec<PaneId> = self.links.keys().copied().collect();
        let client = self.client.clone();
        let mut had_data = false;
        let mut closed: Vec<(PaneId, Option<Value>)> = Vec::new();
        let mut lost = false;
        for pane in panes {
            let Some(link) = self.links.get_mut(&pane) else {
                continue;
            };
            let Some(stream) = link.stream.as_ref() else {
                continue;
            };
            let start = Instant::now();
            let mut drained = 0usize;
            loop {
                match stream.try_recv() {
                    Ok(StreamItem::Data(bytes)) => {
                        had_data = true;
                        self.dirty = true;
                        drained += bytes.len();
                        link.vt.feed(&bytes);
                        link.needs_facts = true;
                        if let Some(channel) = link.session.channel()
                            && let Err(error) = client.credit(channel, bytes.len())
                        {
                            if session::is_lost(&error) {
                                lost = true;
                                break;
                            }
                            return Err(error);
                        }
                        if drained >= DRAIN_BYTES || start.elapsed() >= DRAIN_TIME {
                            break;
                        }
                    }
                    Ok(StreamItem::Closed(reason)) => {
                        closed.push((pane, reason));
                        break;
                    }
                    Ok(StreamItem::Lost) | Err(TryRecvError::Disconnected) => {
                        lost = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
            if lost {
                break;
            }
        }
        if lost {
            self.begin_reconnect();
            return Ok(had_data);
        }
        for (pane, reason) in closed {
            self.close_stream(pane, reason);
        }
        Ok(had_data)
    }

    pub(super) fn apply_events(&mut self) -> Result<()> {
        for _ in 0..EVENT_BATCH {
            let notification = match self.events.try_recv() {
                Ok(notification) => notification,
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    self.begin_reconnect();
                    return Ok(());
                }
            };
            if self.model.apply(&notification) {
                self.dirty = true;
            }
            match &notification {
                Notification::Missed { .. } => {
                    // A document change may have been missed too; refetch it
                    // before trusting the local layout.
                    self.doc.stale = true;
                    self.agents.reset();
                    self.reconcile_snapshot();
                }
                Notification::Event {
                    kind: EventKind::Doc,
                    machine,
                    data,
                    ..
                } => {
                    if machine.is_none()
                        && data.get("name").and_then(Value::as_str) == Some(DOC_NAME)
                    {
                        if let Some(version) = data.get("version").and_then(Value::as_u64) {
                            self.doc.note_event(version);
                        } else {
                            self.doc.stale = true;
                        }
                    }
                }
                Notification::Event { kind, terminal, .. } => {
                    if let Some(address) = terminal {
                        let kind = kind.clone();
                        let address = address.clone();
                        if kind == EventKind::Labels {
                            self.note_labels(&address);
                        }
                        self.apply_event(kind, &address);
                    }
                }
                Notification::Heartbeat { .. } | Notification::Other { .. } => {}
            }
            self.apply_display();
        }
        Ok(())
    }

    /// Applies a core event to the pane that owns the terminal. The pure model
    /// has already applied it; this reacts to it (attach, wait, remove).
    pub(super) fn apply_event(&mut self, kind: EventKind, address: &str) {
        let Some(pane) = self.workspace.pane_id_by_address(address) else {
            return;
        };
        match kind {
            EventKind::Restarted => {
                if let Some(link) = self.links.get_mut(&pane) {
                    let _ = link.session.detach_current();
                    link.session.waiting();
                    link.stream = None;
                    link.retry_at = Some(Instant::now());
                    link.finished = false;
                    link.finalized = false;
                    link.removed = false;
                    link.focus_sent = None;
                }
                if self.workspace.mark_live(pane) {
                    self.dirty = true;
                }
                self.ui.message = Some(format!("terminal {address} restarted; reattaching"));
                self.dirty = true;
            }
            EventKind::Lost => {
                if let Some(link) = self.links.get_mut(&pane) {
                    let _ = link.session.detach_current();
                    link.session.waiting();
                    link.stream = None;
                    link.retry_at = Some(Instant::now() + ATTACH_RETRY);
                    link.finished = false;
                    link.finalized = false;
                    link.removed = false;
                    link.focus_sent = None;
                }
                if self.workspace.mark_live(pane) {
                    self.dirty = true;
                }
                self.ui.message = Some(format!(
                    "terminal {address} lost its program; waiting for a new shell"
                ));
                self.dirty = true;
            }
            EventKind::Finished => {
                let (has_stream, finalized) = self
                    .links
                    .get(&pane)
                    .map(|link| (link.stream.is_some(), link.finalized))
                    .unwrap_or((false, false));
                if finalized {
                    // The final screen is already on screen; a later record
                    // only refines the status.
                    let code = self
                        .model
                        .terminal(address)
                        .map(session::record_exit_code)
                        .unwrap_or(0);
                    self.workspace.mark_finished(pane, code);
                    self.ui.message = Some(format!(
                        "terminal {address} exited ({code}) · final screen kept; prefix x closes it"
                    ));
                    self.dirty = true;
                } else if has_stream {
                    // The last bytes and the CLOSE reason are still coming.
                    if let Some(link) = self.links.get_mut(&pane) {
                        link.finished = true;
                        link.retry_at = None;
                    }
                } else {
                    let code = self.model.terminal(address).map(session::record_exit_code);
                    self.recover_finished_pane(pane, address, code);
                }
            }
            EventKind::Removed => {
                if let Some(link) = self.links.get_mut(&pane) {
                    link.removed = true;
                    link.retry_at = None;
                }
                if self
                    .links
                    .get(&pane)
                    .is_some_and(|link| link.stream.is_none())
                {
                    self.remove_gone(pane, address);
                }
            }
            _ => {}
        }
    }

    /// Applies a fresh snapshot after `missed` or a reconnect: live terminals
    /// are reattached in place, finished ones are removed, missing ones leave
    /// the workspace. No terminal is started.
    pub(super) fn reconcile_snapshot(&mut self) {
        let panes: Vec<(PaneId, String)> = self
            .workspace
            .panes()
            .map(|pane| (pane.id, pane.address.clone()))
            .collect();
        for (pane, address) in panes {
            let record = self.model.terminal(&address).cloned();
            match record {
                Some(record) if record.is_live() => {
                    let parked = record.pid.is_none();
                    if let Some(link) = self.links.get_mut(&pane) {
                        let _ = link.session.detach_current();
                        link.session.waiting();
                        link.stream = None;
                        link.retry_at = Some(
                            Instant::now() + if parked { ATTACH_RETRY } else { Duration::ZERO },
                        );
                        link.finished = false;
                        link.finalized = false;
                        link.removed = false;
                        link.focus_sent = None;
                    }
                    if self.workspace.mark_live(pane) {
                        self.dirty = true;
                    }
                    self.ui.message = Some(format!("reattaching terminal {address}…"));
                    self.dirty = true;
                }
                Some(record) => {
                    // A finished record still has its final screen in core;
                    // recover and show it rather than removing the pane.
                    let code = session::record_exit_code(&record);
                    let finalized = self.links.get(&pane).is_some_and(|link| link.finalized);
                    if finalized {
                        self.workspace.mark_finished(pane, code);
                        self.ui.message = Some(format!(
                            "terminal {address} exited ({code}) · final screen kept; prefix x closes it"
                        ));
                        self.dirty = true;
                    } else {
                        self.recover_finished_pane(pane, &address, Some(code));
                        if self.reconn.is_some() {
                            return;
                        }
                    }
                }
                None => {
                    // A finished pane's screen is local now; a core restart
                    // that forgot the record must not silently discard it.
                    // An explicit `removed` event still does (above).
                    if self.links.get(&pane).is_some_and(|link| link.finalized) {
                        if let Some(link) = self.links.get_mut(&pane) {
                            link.session.lost();
                            link.retry_at = None;
                        }
                        self.ui.message = Some(format!(
                            "terminal {address} is no longer in core; final screen kept; \
                             prefix x closes it"
                        ));
                        self.dirty = true;
                    } else {
                        self.remove_gone(pane, &address);
                    }
                }
            }
        }
    }

    pub(super) fn apply_display(&mut self) {
        let updates: Vec<Displayed> = self
            .workspace
            .panes()
            .map(|pane| {
                let record = self.model.terminal(&pane.address);
                Displayed {
                    pane: pane.id,
                    title: record
                        .map(|record| record.title.clone())
                        .unwrap_or_default(),
                    status: record
                        .and_then(|record| record.status.as_ref())
                        .map(|status| status.state.clone()),
                    labels: record
                        .map(|record| record.labels.clone())
                        .unwrap_or_else(|| pane.labels.clone()),
                }
            })
            .collect();
        let mut changed = false;
        for update in updates {
            changed |= self.workspace.set_title(update.pane, update.title);
            changed |= self.workspace.set_status(update.pane, update.status);
            changed |= self
                .workspace
                .set_attention_labels(update.pane, &update.labels);
        }
        if changed {
            self.dirty = true;
        }
    }

    // --- lifecycle outcomes ------------------------------------------------

    pub(super) fn close_stream(&mut self, pane: PaneId, reason: Option<Value>) {
        let Some(link) = self.links.get(&pane) else {
            return;
        };
        let address = link.session.address().to_owned();
        let finished = link.finished;
        let removed = link.removed;
        let disposition =
            lifecycle::disposition(self.model.terminal(&address), finished, removed, &reason);
        match disposition {
            Disposition::Finished => self.finish_attached(pane, &address, reason),
            Disposition::Removed => self.remove_gone(pane, &address),
            Disposition::Lost => {
                if let Some(link) = self.links.get_mut(&pane) {
                    let _ = link.session.detach_current();
                    link.session.waiting();
                    link.stream = None;
                    link.retry_at = Some(Instant::now() + ATTACH_RETRY);
                    link.finished = false;
                    link.removed = false;
                    link.focus_sent = None;
                    link.needs_facts = false;
                    link.last_facts = None;
                }
                self.ui.message = Some(format!(
                    "terminal {address} lost its program; waiting for a new shell"
                ));
                self.dirty = true;
            }
        }
    }

    /// Finalizes a pane whose program ended while attached. Its VT already
    /// holds the last bytes, so the final frame is preserved and the pane
    /// stays visible until the person closes it.
    pub(super) fn finish_attached(&mut self, pane: PaneId, address: &str, reason: Option<Value>) {
        let code = session::finished_exit_code(self.model.terminal(address), reason.as_ref());
        self.finalize_pane(pane, address, code);
    }

    /// Recovers a terminal that finished without this pane's stream (it exited
    /// before the first attach, while the connection was down, or before a
    /// parked terminal could be reattached): read its record and final screen
    /// from core and show them. The record is not removed here; `x` on the
    /// finished pane removes it after it has been shown.
    pub(super) fn recover_finished_pane(
        &mut self,
        pane: PaneId,
        address: &str,
        fallback: Option<i32>,
    ) {
        let record = match self.client.get(address) {
            Ok(record) => Some(record),
            Err(error) if session::is_lost(&error) => {
                self.begin_reconnect();
                return;
            }
            Err(error) => {
                let code = fallback
                    .or_else(|| self.model.terminal(address).map(session::record_exit_code))
                    .unwrap_or(1);
                self.finalize_pane(pane, address, code);
                self.ui.message = Some(format!(
                    "terminal {address} exited ({code}); its record could not be read: {error}"
                ));
                return;
            }
        };
        let code = record
            .as_ref()
            .map(session::record_exit_code)
            .or(fallback)
            .or_else(|| self.model.terminal(address).map(session::record_exit_code))
            .unwrap_or(1);
        let read = match self.client.read(address, 0) {
            Ok(read) => read,
            Err(error) if session::is_lost(&error) => {
                self.begin_reconnect();
                return;
            }
            Err(error) => {
                self.finalize_pane(pane, address, code);
                self.ui.message = Some(format!(
                    "terminal {address} exited ({code}); its final screen could not be read: \
                     {error} (hc read {address})"
                ));
                return;
            }
        };
        let shown = self.show_final_text(pane, &read.text);
        self.finalize_pane(pane, address, code);
        if let Err(error) = shown {
            self.ui.message = Some(format!(
                "terminal {address} exited ({code}); its final screen could not be drawn: {error}"
            ));
        }
    }

    /// Replaces a pane's VT with the text core kept for a finished terminal.
    /// `terminal.read` separates lines with `\n`; the VT needs CRLF so every
    /// line starts at the first column.
    fn show_final_text(&mut self, pane: PaneId, text: &str) -> Result<()> {
        let (cols, rows) = self.pane_attach_size(pane);
        let mut vt = Vt::new(cols, rows).map_err(vt_error)?;
        let mut bytes = Vec::with_capacity(text.len() + 16);
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                bytes.extend_from_slice(b"\r\n");
            }
            bytes.extend_from_slice(line.trim_end_matches('\r').as_bytes());
        }
        vt.feed(&bytes);
        if let Some(link) = self.links.get_mut(&pane) {
            link.vt = vt;
            link.vt_size = (cols, rows);
            link.needs_facts = false;
            link.last_facts = None;
        }
        Ok(())
    }

    /// Keeps a pane as a finished view: its final VT stays, its status is
    /// shown, and its session stops tracking a stream. `terminal.rm` waits for
    /// the explicit close of the pane.
    fn finalize_pane(&mut self, pane: PaneId, address: &str, code: i32) {
        let already = self.links.get(&pane).is_some_and(|link| link.finalized);
        if !already && let Some(link) = self.links.get_mut(&pane) {
            link.session.finished();
            link.stream = None;
            link.retry_at = None;
            link.finished = true;
            link.finalized = true;
            link.removed = false;
            link.needs_facts = false;
            link.last_facts = None;
            link.focus_sent = None;
        }
        self.workspace.mark_finished(pane, code);
        self.ui.message = Some(format!(
            "terminal {address} exited ({code}) · final screen kept; prefix x closes it"
        ));
        self.dirty = true;
    }

    pub(super) fn remove_gone(&mut self, pane: PaneId, address: &str) {
        if let Some(mut link) = self.links.remove(&pane) {
            link.session.lost();
        }
        self.workspace.remove_pane(pane);
        self.ui.message = Some(format!("terminal {address} was removed"));
        self.dirty = true;
    }

    /// Drops a pane whose attach failed for a reason that is neither
    /// "finished" nor "not found". Cleanup ends a terminal this client
    /// started, so no program is left untracked without a word.
    pub(super) fn forget_pane(&mut self, pane: PaneId, address: &str, why: &str) {
        if let Some(mut link) = self.links.remove(&pane)
            && let Err(error) = link.session.cleanup()
        {
            eprintln!("ht: warning: could not clean up terminal {address}: {error}");
        }
        self.workspace.remove_pane(pane);
        self.ui.message = Some(format!("terminal {address} {why}"));
        self.dirty = true;
    }
}
