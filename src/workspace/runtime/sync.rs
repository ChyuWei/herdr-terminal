//! Shared-document synchronization and applying a document to the live
//! workspace. The pure decisions live in `model::doc`; this file performs the
//! bounded `doc.get`/`doc.set` calls, CAS merge and pane reconciliation.

use super::Screen;
use crate::core::{Result, Terminal};
use crate::session::{self, Session};
use crate::workspace::model::{
    DOC_NAME, LocalView, PaneId, SharedDoc, TerminalInfo, merge, pane_addresses, restore,
};
use crate::workspace::sync::{DOC_TIMEOUT, MAX_WRITE_ROUNDS, is_conflict};
use std::collections::BTreeMap;
use std::time::Instant;

impl Screen<'_> {
    /// Runs once per live loop: notice local shared changes, refetch a stale
    /// document, write dirty changes, and save the local view.
    pub(super) fn sync_workspace(&mut self) -> Result<()> {
        if self.reconn.is_some() {
            return Ok(());
        }
        let local = self.workspace.capture_doc();
        if local != self.doc.base {
            self.doc.dirty = true;
        }
        let now = Instant::now();
        if self.doc.stale {
            self.fetch_document()?;
        } else if self.doc.dirty && self.doc.due(now) {
            self.write_document(MAX_WRITE_ROUNDS)?;
        }
        self.save_view();
        Ok(())
    }

    pub(super) fn save_view(&mut self) {
        if let Some(error) = self
            .view
            .save_if_changed(&self.workspace, self.ui.sidebar.open)
            && self.ui.message.as_deref() != Some(error.as_str())
        {
            self.ui.message = Some(error);
            self.dirty = true;
        }
    }

    /// Refetches the document after a doc event, `missed` or a reconnect:
    /// merge unsaved local work, apply remote changes, recover the other
    /// screen's terminals, and write anything that remains.
    pub(super) fn fetch_document(&mut self) -> Result<()> {
        let get = match self.client.doc_get(DOC_NAME, DOC_TIMEOUT) {
            Ok(get) => get,
            Err(error) if session::is_lost(&error) => {
                self.begin_reconnect();
                return Ok(());
            }
            Err(error) => {
                self.doc.retry_later(Instant::now());
                self.ui.message = Some(format!(
                    "could not refresh the workspace: {error}; retrying"
                ));
                self.dirty = true;
                return Ok(());
            }
        };
        if get.value.is_null() {
            // The document was deleted while this screen was away. Core keeps
            // the version of a deleted document, so recreate it at that
            // version; a conflict re-reads the winner.
            self.doc.base = SharedDoc::empty();
            self.doc.version = get.version;
            self.doc.stale = false;
            self.doc.dirty = true;
            self.doc.force = true;
            self.ui.message = Some("the workspace document was removed; recreating it".into());
            self.dirty = true;
            return self.write_document(MAX_WRITE_ROUNDS);
        }
        let remote = match SharedDoc::parse(&get.value) {
            Ok(doc) => doc,
            Err(error) => {
                self.doc.stale = false;
                self.ui.message = Some(format!(
                    "the workspace document cannot be used ({error}); it was not changed"
                ));
                self.dirty = true;
                return Ok(());
            }
        };
        let Some(infos) = self.terminal_infos(&[&remote])? else {
            return Ok(());
        };
        let local = self.workspace.capture_doc();
        let merged = merge(&self.doc.base, &local, &remote);
        self.apply_document(&merged.doc, &infos)?;
        if self.reconn.is_some() {
            return Ok(());
        }
        self.recover_orphans(&infos)?;
        // The base is what core holds, not the merge: unsaved local work must
        // still be written, and the next merge must see the true remote side.
        self.doc.base = remote.clone();
        self.doc.version = get.version;
        self.doc.stale = false;
        self.doc.force = false;
        let after = self.workspace.capture_doc();
        self.doc.dirty = after != remote;
        if !merged.conflicts.is_empty() {
            self.report_conflict();
        }
        if self.doc.dirty {
            self.write_document(MAX_WRITE_ROUNDS)?;
        }
        self.dirty = true;
        Ok(())
    }

    /// After a reconnect: read the document again before any reattach, so a
    /// change made while this screen was disconnected is not lost.
    pub(super) fn refresh_after_reconnect(&mut self) -> Result<()> {
        self.doc.stale = true;
        self.doc.retry_at = None;
        self.fetch_document()
    }

    /// Saves a dirty document, merging on a CAS conflict and retrying with the
    /// version just read. Bounded; a remaining failure leaves it dirty.
    pub(super) fn write_document(&mut self, rounds: u32) -> Result<()> {
        for _ in 0..rounds {
            let local = self.workspace.capture_doc();
            if !self.doc.force && local == self.doc.base {
                self.doc.dirty = false;
                self.doc.retry_at = None;
                return Ok(());
            }
            match self
                .client
                .doc_set(DOC_NAME, &local.to_value(), self.doc.version, DOC_TIMEOUT)
            {
                Ok(set) => {
                    self.doc.written(local, set.version);
                    self.dirty = true;
                    return Ok(());
                }
                Err(error) if session::is_lost(&error) => {
                    self.begin_reconnect();
                    return Ok(());
                }
                Err(error) if is_conflict(&error) => {
                    let get = match self.client.doc_get(DOC_NAME, DOC_TIMEOUT) {
                        Ok(get) => get,
                        Err(error) if session::is_lost(&error) => {
                            self.begin_reconnect();
                            return Ok(());
                        }
                        Err(error) => {
                            self.save_failed(format!(
                                "could not read the changed workspace document: {error}"
                            ));
                            return Ok(());
                        }
                    };
                    if get.value.is_null() {
                        self.doc.base = SharedDoc::empty();
                        self.doc.version = get.version;
                        self.doc.force = true;
                        continue;
                    }
                    let remote = match SharedDoc::parse(&get.value) {
                        Ok(doc) => doc,
                        Err(error) => {
                            self.save_failed(format!(
                                "the changed workspace document cannot be used ({error})"
                            ));
                            return Ok(());
                        }
                    };
                    let Some(infos) = self.terminal_infos(&[&remote])? else {
                        return Ok(());
                    };
                    let local = self.workspace.capture_doc();
                    let merged = merge(&self.doc.base, &local, &remote);
                    self.apply_document(&merged.doc, &infos)?;
                    if self.reconn.is_some() {
                        return Ok(());
                    }
                    self.recover_orphans(&infos)?;
                    // Keep the true remote base so the next iteration writes
                    // the local side of the merge.
                    self.doc.base = remote;
                    self.doc.version = get.version;
                    self.doc.force = false;
                    if !merged.conflicts.is_empty() {
                        self.report_conflict();
                    }
                    continue;
                }
                Err(error) => {
                    self.save_failed(format!("could not save the workspace: {error}"));
                    return Ok(());
                }
            }
        }
        self.doc.dirty = true;
        self.doc.retry_later(Instant::now());
        self.ui.message = Some("workspace changes are not saved yet; retrying".into());
        self.dirty = true;
        Ok(())
    }

    /// Best-effort flush on detach, with the same bounded conflict/merge
    /// budget as a normal write. Reports rather than stops the detach.
    pub(super) fn flush_document(&mut self) -> Option<String> {
        let _ = self.write_document(MAX_WRITE_ROUNDS);
        if self.reconn.is_some() {
            Some("lost the connection before the workspace document was saved".into())
        } else if self.doc.dirty {
            Some("the workspace document could not be flushed before detaching".into())
        } else {
            None
        }
    }

    fn save_failed(&mut self, message: String) {
        self.doc.dirty = true;
        self.doc.retry_later(Instant::now());
        self.ui.message = Some(format!("{message}; retrying"));
        self.dirty = true;
    }

    fn report_conflict(&mut self) {
        self.ui.message = Some("layout changed on another screen".into());
        self.dirty = true;
    }

    /// Rebuilds the workspace from `doc`, preserving the local view where ids
    /// still exist and keeping links for unchanged id+address pairs. New panes
    /// adopt existing terminals; removed panes only detach.
    fn apply_document(&mut self, doc: &SharedDoc, infos: &[TerminalInfo]) -> Result<()> {
        let view = LocalView::capture(&self.workspace);
        let restored = restore(
            doc,
            self.workspace.allocator().clone(),
            self.workspace.name_counters(),
            infos,
        );
        let mut workspace = restored.workspace;
        let ids: Vec<PaneId> = workspace.panes().map(|pane| pane.id).collect();
        for pane in ids {
            let Some(old) = self.workspace.pane(pane) else {
                continue;
            };
            let same_address = workspace
                .pane(pane)
                .is_some_and(|new| new.address == old.address);
            if same_address && let Some(new) = workspace.pane_mut(pane) {
                new.title = old.title.clone();
                new.status = old.status.clone();
                new.exit = old.exit;
                // Ownership and the keep flag follow the pane this screen
                // already knows; applying a document never changes them.
                new.owned = old.owned;
                new.keep = old.keep;
            }
        }
        view.apply(&mut workspace);

        let mut kept = BTreeMap::new();
        let mut lost = false;
        for (pane, mut link) in std::mem::take(&mut self.links) {
            let keep = workspace
                .pane(pane)
                .is_some_and(|record| record.address == link.session.address());
            if keep {
                kept.insert(pane, link);
            } else if let Err(error) = link.session.detach()
                && session::is_lost(&error)
            {
                lost = true;
            }
        }
        self.links = kept;
        self.workspace = workspace;
        self.seen.extend(restored.seen);
        let pane_ids: Vec<PaneId> = self.workspace.panes().map(|pane| pane.id).collect();
        self.seen.extend(pane_ids);
        if lost {
            self.begin_reconnect();
            return Ok(());
        }
        let new_panes: Vec<(PaneId, String)> = self
            .workspace
            .panes()
            .filter(|pane| !self.links.contains_key(&pane.id))
            .map(|pane| (pane.id, pane.address.clone()))
            .collect();
        for (pane, address) in new_panes {
            let session = Session::adopt(self.client.clone(), address);
            self.add_link(pane, session)?;
        }
        self.dirty = true;
        Ok(())
    }

    /// Recovers labeled terminals that are not in the workspace. `infos` is
    /// the event model plus records fetched for the document being applied.
    fn recover_orphans(&mut self, infos: &[TerminalInfo]) -> Result<usize> {
        let recovered =
            crate::workspace::model::recover_orphans(&mut self.workspace, infos, &self.seen);
        for (pane, address) in &recovered {
            self.seen.insert(*pane);
            let session = Session::adopt(self.client.clone(), address.clone());
            self.add_link(*pane, session)?;
        }
        if !recovered.is_empty() {
            self.dirty = true;
        }
        Ok(recovered.len())
    }

    /// Terminal records for the event model plus any address mentioned by
    /// `docs` that the model has not seen yet. A `not_found` record is left
    /// out; the restore prunes it.
    fn terminal_infos(&mut self, docs: &[&SharedDoc]) -> Result<Option<Vec<TerminalInfo>>> {
        // Local panes first: a terminal this screen just started may not have
        // reached the event model yet, and a document apply must not prune it.
        let mut infos: Vec<TerminalInfo> = self
            .workspace
            .panes()
            .map(|pane| TerminalInfo::new(pane.address.clone(), pane.labels.clone()))
            .collect();
        let mut known: std::collections::BTreeSet<String> =
            infos.iter().map(|info| info.address.clone()).collect();
        for terminal in self.model.terminals() {
            if known.insert(terminal.address.clone()) {
                infos.push(info(terminal));
            }
        }
        for doc in docs {
            for address in pane_addresses(doc) {
                if !known.insert(address.clone()) {
                    continue;
                }
                match self.client.get(&address) {
                    Ok(record) => {
                        // The snapshot may predate a terminal the document
                        // names; keep its facts for the agent rows and for
                        // later events.
                        self.model.upsert(record.clone());
                        infos.push(info(&record));
                    }
                    Err(error) if session::is_lost(&error) => {
                        self.begin_reconnect();
                        return Ok(None);
                    }
                    Err(_) => {}
                }
            }
        }
        Ok(Some(infos))
    }
}

fn info(terminal: &Terminal) -> TerminalInfo {
    TerminalInfo::new(terminal.address.clone(), terminal.labels.clone())
}
