//! Attach-byte draining, event application and lifecycle outcomes: finished,
//! lost, restarted, removed and `missed` all resolve to a pane state here.

use super::{ATTACH_RETRY, DRAIN_BYTES, DRAIN_TIME, EVENT_BATCH, Screen};
use crate::core::{EventKind, Notification, Result, StreamItem};
use crate::screen::lifecycle::{self, Disposition};
use crate::session;
use crate::tui::vt_error;
use crate::vt::Vt;
use crate::workspace::model::{DOC_NAME, PANE_LABEL, PaneId};
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
                    Ok(StreamItem::Resize { cols, rows }) => {
                        if cols > 0 && rows > 0 {
                            // A confirmed observer's grid arrives before the
                            // paint that uses it. Record it as the pane's
                            // size so sizing never reverts or re-sends it;
                            // RESIZE is not output, so it pays no credit.
                            link.vt.resize(cols, rows).map_err(vt_error)?;
                            link.vt_size = (cols, rows);
                            // Only an observer receives RESIZE; it follows
                            // core's grid and never resizes the program.
                            link.observed = true;
                            link.needs_facts = true;
                            self.dirty = true;
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
        // A stream-less `removed` may be half of core's machine-rename pair,
        // which ends with a `created` for the new address in the same batch.
        // Its removal is decided after this batch, one loop step later.
        let deferred = std::mem::take(&mut self.deferred_removals);
        let mut disconnected = false;
        for _ in 0..EVENT_BATCH {
            let notification = match self.events.try_recv() {
                Ok(notification) => notification,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            };
            if self.model.apply(&notification) {
                self.dirty = true;
            }
            match &notification {
                Notification::Missed { .. } => {
                    // A document change may have been missed too; read and
                    // merge it before reconciling, as the reconnect path does.
                    // The stored document may still name a machine's old
                    // addresses, while the snapshot already has the new ones.
                    self.doc.stale = true;
                    self.agents.reset();
                    self.fetch_document()?;
                    if self.reconn.is_some() {
                        // The reconnect path refetches and reconciles.
                        return Ok(());
                    }
                    self.reconcile_snapshot();
                    self.check_preview();
                    self.refresh_machines_soon();
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
                        if kind == EventKind::Created {
                            self.readdress_created(&address);
                        }
                        self.apply_event(kind, &address);
                    }
                }
                Notification::Heartbeat { .. } | Notification::Other { .. } => {}
            }
            self.apply_display();
        }
        if disconnected {
            self.begin_reconnect();
            return Ok(());
        }
        for (pane, address) in deferred {
            self.process_deferred_removal(pane, &address);
        }
        Ok(())
    }

    /// Moves a pane to the address of the terminal whose `herdr.terminal.pane`
    /// label names it, when core has renamed that terminal's machine. Core
    /// emits `removed` for the old qualified addresses and `created` for the
    /// new ones; the pane, its stream and its final screen belong to the same
    /// terminal. Never matches another screen's pane (its label carries that
    /// screen's id) and never duplicates an address another pane holds.
    fn readdress_pane_by_label(&mut self, pane: PaneId) -> bool {
        let Some(current) = self.workspace.pane(pane).map(|pane| pane.address.clone()) else {
            return false;
        };
        let candidates: Vec<String> = self
            .model
            .terminals()
            .filter(|terminal| terminal.address != current)
            .filter(|terminal| {
                terminal
                    .labels
                    .get(PANE_LABEL)
                    .and_then(|label| label.parse::<u64>().ok())
                    == Some(pane.0)
            })
            .map(|terminal| terminal.address.clone())
            .collect();
        // A terminal another pane already shows is a collision, not a move.
        let Some(address) = candidates.into_iter().find(|address| {
            !self
                .workspace
                .panes()
                .any(|other| other.id != pane && other.address == *address)
        }) else {
            return false;
        };
        if !self.workspace.rename_pane_address(pane, address.clone()) {
            return false;
        }
        if let Some(link) = self.links.get_mut(&pane) {
            link.session.set_address(address.clone());
            link.removed = false;
            // A pane that was waiting to attach (or had lost its stream)
            // must attach under the new address now; a finished pane keeps
            // its final screen. A live stream follows the same terminal.
            link.retry_at = (!link.finalized && link.stream.is_none()).then(Instant::now);
        }
        self.ui.message = Some(format!(
            "machine renamed: terminal {current} is now {address}"
        ));
        self.dirty = true;
        true
    }

    /// A stream-less pane's `removed` is confirmed one batch later, after a
    /// following `created` has had its chance to move the pane. It leaves only
    /// when no new address names it and core no longer has the old one.
    fn process_deferred_removal(&mut self, pane: PaneId, address: &str) {
        let Some(link) = self.links.get(&pane) else {
            return;
        };
        if !link.removed || link.session.address() != address {
            return;
        }
        if self.workspace.pane(pane).is_none() {
            return;
        }
        if self.readdress_pane_by_label(pane) {
            return;
        }
        if self.model.terminal(address).is_some() {
            return;
        }
        self.remove_gone(pane, address);
    }

    /// A `created` record whose pane label names a pane this screen already
    /// shows under an address core no longer has is the same terminal after a
    /// machine rename. Give the pane and its attach stream the new qualified
    /// address instead of dropping them.
    fn readdress_created(&mut self, address: &str) {
        let Some(record) = self.model.terminal(address) else {
            return;
        };
        let Some(label) = record.labels.get(PANE_LABEL) else {
            return;
        };
        let Ok(id) = label.parse::<u64>() else {
            return;
        };
        let pane = PaneId(id);
        let Some(existing) = self.workspace.pane(pane) else {
            return;
        };
        if existing.address == address || self.model.terminal(&existing.address).is_some() {
            return;
        }
        self.readdress_pane_by_label(pane);
    }

    /// Applies a core event to the pane that owns the terminal. The pure model
    /// has already applied it; this reacts to it (attach, wait, remove).
    pub(super) fn apply_event(&mut self, kind: EventKind, address: &str) {
        if matches!(&kind, EventKind::Finished | EventKind::Removed) {
            // A preview has no pane and no lifecycle of its own; an end or
            // removal closes its observer stream and returns to the manager.
            self.preview_gone(address);
        }
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
                    // Core sends `removed` for a machine's old addresses
                    // before `created` for the new ones. Wait one batch so a
                    // rename can move this pane instead of deleting it.
                    self.deferred_removals.push((pane, address.to_owned()));
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
                    // A rename in the `missed` window leaves the snapshot at
                    // the new address while the workspace still shows the old
                    // one; follow the pane's own label before deciding it is
                    // gone.
                    if self.readdress_pane_by_label(pane) {
                        continue;
                    }
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
