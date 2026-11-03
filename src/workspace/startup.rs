//! Opening a workspace: subscribe, read or claim the shared document, restore
//! and adopt terminals, recover labeled orphans, apply the local view, and run
//! an explicit PROGRAM. All core IO for startup lives here; the decisions are
//! in `model::doc` and `model::view`.
//!
//! The subscription snapshot is a starting point, not the truth: an address
//! named by the document but absent from the snapshot is checked with
//! `terminal.get` before it may be pruned, so a pane another screen created
//! just after the snapshot is restored rather than lost. A document deleted at
//! a retained version is rebuilt from live labeled terminals, never replaced by
//! a fresh shell.

use super::Options;
use super::local::{self, ViewStore};
use super::model::{
    DOC_NAME, IdAllocator, PaneId, SharedDoc, StartKind, TerminalInfo, Workspace, pane_addresses,
    recover_orphans, restore,
};
use super::runtime::Boot;
use super::sync::{DOC_TIMEOUT, DocState, is_conflict};
use crate::core::{CoreClient, Error, Result, RunOptions, Terminal};
use crate::model::Model;
use crate::session::{self, Session};
use ratatui::layout::Rect;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

/// A workspace document this screen created empty. It is removed again if the
/// screen cannot start, so a failed start never leaves a half-claimed document.
pub(super) struct DocClaim {
    client: Arc<CoreClient>,
    version: u64,
}

impl DocClaim {
    pub(super) fn rollback(self) -> Option<String> {
        match self
            .client
            .doc_set(DOC_NAME, &Value::Null, self.version, DOC_TIMEOUT)
        {
            Ok(_) => None,
            Err(error) => Some(format!(
                "could not remove the empty workspace document: {error}"
            )),
        }
    }
}

pub(super) struct Opened {
    pub boot: Boot,
    pub claim: Option<DocClaim>,
}

/// The workspace resolved from the shared document or from core's records.
struct Resolved {
    workspace: Workspace,
    sessions: Vec<(PaneId, Session)>,
    doc: DocState,
    seen: BTreeSet<PaneId>,
}

/// The result of trying to create a document that did not exist.
enum Claimed {
    /// This screen wrote the empty document and started its initial shell.
    Won {
        workspace: Workspace,
        sessions: Vec<(PaneId, Session)>,
        doc: DocState,
        seen: BTreeSet<PaneId>,
        claim: DocClaim,
    },
    /// Another screen won the CAS; restore the document it wrote.
    Lost { value: Value, version: u64 },
}

pub(super) fn open(options: &Options, viewport: Rect) -> Result<Opened> {
    let screen = local::screen_id()?;
    let mut view = ViewStore::load(&screen);
    let mut notes: Vec<String> = view.take_warning().into_iter().collect();

    // Subscribe before reading the document: documents are not in snapshots
    // and disconnected changes are not replayed.
    let client = Arc::new(CoreClient::connect(options.hc.as_deref())?);
    let events = client.subscribe()?;
    let mut model = Model::from_snapshot(events.snapshot());

    let first = client.doc_get(DOC_NAME, DOC_TIMEOUT).map_err(read_error)?;
    let mut value = first.value;
    let mut version = first.version;
    let mut claim: Option<DocClaim> = None;
    let mut started_initial = false;

    // Resolve the document, re-reading whenever a claim race is lost. The
    // bound keeps a permanently contended document from spinning startup.
    let mut resolved: Option<Resolved> = None;
    for _ in 0..3 {
        if !value.is_null() {
            let shared = parse_doc(&value)?;
            // The snapshot may predate a terminal the document already names.
            let infos = startup_infos(&client, &mut model, &[&shared])?;
            resolved = Some(restore_existing(
                &client, &screen, &view, shared, version, &infos, &mut notes,
            ));
            break;
        }
        if version > 0 {
            // Deleted but retained version: rebuild from the live labeled
            // terminals before deciding anything. Never replace them with a
            // fresh shell.
            let infos: Vec<TerminalInfo> = model.terminals().map(info).collect();
            let mut recovered = recover_labels(&client, &screen, &infos);
            if recovered.workspace.pane_count() > 0 {
                view.apply(&mut recovered.workspace);
                let captured = recovered.workspace.capture_doc();
                match client.doc_set(DOC_NAME, &captured.to_value(), version, DOC_TIMEOUT) {
                    Ok(set) => {
                        notes.push(recovered_message(recovered.sessions.len()));
                        recovered.doc = DocState::new(captured, set.version);
                        resolved = Some(recovered);
                        break;
                    }
                    Err(error) if is_conflict(&error) => {
                        let next = client.doc_get(DOC_NAME, DOC_TIMEOUT).map_err(read_error)?;
                        value = next.value;
                        version = next.version;
                        continue;
                    }
                    Err(error) => {
                        return Err(Error::Protocol(format!(
                            "could not recreate the workspace document {DOC_NAME:?}: {error}"
                        )));
                    }
                }
            }
            // No labeled terminal survived: this is a new document at a
            // retained version. Only the claim winner starts a shell.
            match claim_empty(&client, &screen, version, &options.argv, viewport)? {
                Claimed::Won {
                    workspace,
                    sessions,
                    doc,
                    seen,
                    claim: created,
                } => {
                    claim = Some(created);
                    started_initial = true;
                    resolved = Some(Resolved {
                        workspace,
                        sessions,
                        doc,
                        seen,
                    });
                    break;
                }
                Claimed::Lost {
                    value: next_value,
                    version: next_version,
                } => {
                    value = next_value;
                    version = next_version;
                    continue;
                }
            }
        }
        // Never written: this screen claims the brand-new document.
        match claim_empty(&client, &screen, 0, &options.argv, viewport)? {
            Claimed::Won {
                workspace,
                sessions,
                doc,
                seen,
                claim: created,
            } => {
                claim = Some(created);
                started_initial = true;
                resolved = Some(Resolved {
                    workspace,
                    sessions,
                    doc,
                    seen,
                });
                break;
            }
            Claimed::Lost {
                value: next_value,
                version: next_version,
            } => {
                value = next_value;
                version = next_version;
            }
        }
    }
    let Some(mut resolved) = resolved else {
        return Err(Error::Protocol(format!(
            "could not resolve the workspace document {DOC_NAME:?}: it kept changing"
        )));
    };

    if !started_initial && !options.argv.is_empty() {
        let (pane, session) =
            start_program(&client, &mut resolved.workspace, &options.argv, viewport)?;
        resolved.seen.insert(pane);
        resolved.sessions.push((pane, session));
    }

    let Resolved {
        workspace,
        sessions,
        doc,
        seen,
    } = resolved;
    Ok(Opened {
        boot: Boot {
            client,
            events,
            model,
            workspace,
            sessions,
            doc,
            view,
            seen,
            notice: join(notes),
        },
        claim,
    })
}

/// Restores an existing document. Every address it names is checked against
/// core when the snapshot lacks it, so a pane created after the snapshot is
/// adopted and never pruned.
fn restore_existing(
    client: &Arc<CoreClient>,
    screen: &str,
    view: &ViewStore,
    shared: SharedDoc,
    version: u64,
    infos: &[TerminalInfo],
    notes: &mut Vec<String>,
) -> Resolved {
    let restored = restore(&shared, IdAllocator::new(screen), (1, 1), infos);
    let mut workspace = restored.workspace;
    let mut seen = restored.seen;
    let mut sessions: Vec<(PaneId, Session)> = restored
        .matched
        .iter()
        .map(|(pane, address)| (*pane, Session::adopt(client.clone(), address.clone())))
        .collect();
    let recovered = recover_orphans(&mut workspace, infos, &seen);
    let recovered_count = recovered.len();
    for (pane, address) in recovered {
        seen.insert(pane);
        sessions.push((pane, Session::adopt(client.clone(), address)));
    }
    seen.extend(workspace.panes().map(|pane| pane.id));
    // The view selects existing ids only; it is applied after the document.
    view.apply(&mut workspace);
    if restored.missing > 0 {
        notes.push(missing_message(restored.missing));
    }
    if recovered_count > 0 {
        notes.push(recovered_message(recovered_count));
    }
    Resolved {
        workspace,
        sessions,
        doc: DocState::new(shared, version),
        seen,
    }
}

/// Builds a workspace from the labels of live terminals in the snapshot.
fn recover_labels(client: &Arc<CoreClient>, screen: &str, infos: &[TerminalInfo]) -> Resolved {
    let mut workspace = Workspace::for_screen(screen);
    let recovered = recover_orphans(&mut workspace, infos, &BTreeSet::new());
    let mut seen = BTreeSet::new();
    let mut sessions = Vec::new();
    for (pane, address) in recovered {
        seen.insert(pane);
        sessions.push((pane, Session::adopt(client.clone(), address)));
    }
    Resolved {
        workspace,
        sessions,
        doc: DocState::new(SharedDoc::empty(), 0),
        seen,
    }
}

/// Claims an absent or deleted document by writing `version`. The winner
/// starts the initial shell (the explicit PROGRAM, when one was given); a
/// loser starts nothing and restores the winner's document instead.
fn claim_empty(
    client: &Arc<CoreClient>,
    screen: &str,
    version: u64,
    argv: &[String],
    viewport: Rect,
) -> Result<Claimed> {
    let mut workspace = Workspace::for_screen(screen);
    let empty = workspace.capture_doc();
    match client.doc_set(DOC_NAME, &empty.to_value(), version, DOC_TIMEOUT) {
        Ok(set) => {
            let claim = DocClaim {
                client: client.clone(),
                version: set.version,
            };
            let request = workspace.start_request(StartKind::Initial);
            let size = workspace.pending_attach_size(&request, viewport);
            let run = run_options(argv, &request, size);
            let session = match Session::start(client.clone(), &run) {
                Ok(session) => session,
                Err(error) => {
                    let warning = claim.rollback();
                    let mut message = format!("could not start the first terminal: {error}");
                    if let Some(warning) = warning {
                        message.push_str(&format!(" ({warning})"));
                    }
                    return Err(Error::Protocol(message));
                }
            };
            let address = session.address().to_owned();
            let pane = workspace.insert_terminal(&request, address.clone(), address);
            let mut seen = BTreeSet::new();
            seen.insert(pane);
            Ok(Claimed::Won {
                workspace,
                sessions: vec![(pane, session)],
                doc: DocState::new(empty, set.version),
                seen,
                claim,
            })
        }
        Err(error) if is_conflict(&error) => {
            // Another screen won the claim: restore its document and do not
            // start an initial shell.
            let next = client.doc_get(DOC_NAME, DOC_TIMEOUT).map_err(read_error)?;
            Ok(Claimed::Lost {
                value: next.value,
                version: next.version,
            })
        }
        Err(error) => Err(Error::Protocol(format!(
            "could not create the workspace document {DOC_NAME:?}: {error}"
        ))),
    }
}

/// Starts an explicit PROGRAM in the workspace, as a new tab unless the
/// workspace is empty.
fn start_program(
    client: &Arc<CoreClient>,
    workspace: &mut Workspace,
    argv: &[String],
    viewport: Rect,
) -> Result<(PaneId, Session)> {
    let kind = if workspace.pane_count() == 0 {
        StartKind::Initial
    } else {
        StartKind::Tab
    };
    let request = workspace.start_request(kind);
    let size = workspace.pending_attach_size(&request, viewport);
    let run = run_options(argv, &request, size);
    let session = Session::start(client.clone(), &run).map_err(|error| {
        Error::Protocol(format!(
            "could not start {:?}: {error}",
            argv.first().map(String::as_str).unwrap_or("")
        ))
    })?;
    let address = session.address().to_owned();
    let pane = workspace.insert_terminal(&request, address.clone(), address);
    Ok((pane, session))
}

/// Terminal records from the snapshot plus a `terminal.get` for every address
/// the document names that the snapshot does not contain. A `not_found`
/// record stays out, so restore may prune it; an unreachable core fails
/// startup before any prune can be written.
fn startup_infos(
    client: &CoreClient,
    model: &mut Model,
    docs: &[&SharedDoc],
) -> Result<Vec<TerminalInfo>> {
    let mut infos: Vec<TerminalInfo> = model.terminals().map(info).collect();
    let mut known: BTreeSet<String> = infos.iter().map(|info| info.address.clone()).collect();
    for doc in docs {
        for address in pane_addresses(doc) {
            if !known.insert(address.clone()) {
                continue;
            }
            match client.get(&address) {
                Ok(record) => {
                    // Keep the fetched record in the model so agent rows have
                    // its status/turn/labels before its next event arrives.
                    model.upsert(record.clone());
                    infos.push(info(&record));
                }
                // Only a confirmed `not_found` may be pruned. Any other
                // refusal fails startup before a prune can be written, so an
                // unknown error never loses a live terminal.
                Err(Error::Rpc { error, .. }) if error.kind() == Some("not_found") => {}
                Err(error) => {
                    return Err(Error::Protocol(format!(
                        "could not check terminal {address} named by the workspace document: \
                         {error}"
                    )));
                }
            }
        }
    }
    Ok(infos)
}

fn run_options(
    argv: &[String],
    request: &super::model::StartRequest,
    size: (u16, u16),
) -> RunOptions {
    RunOptions {
        argv: argv.to_vec(),
        name: Some(session::start_name()),
        labels: request.labels(),
        keep: true,
        rm: false,
        size: Some(size),
        ..RunOptions::default()
    }
}

fn info(terminal: &Terminal) -> TerminalInfo {
    TerminalInfo::new(terminal.address.clone(), terminal.labels.clone())
}

fn parse_doc(value: &Value) -> Result<SharedDoc> {
    SharedDoc::parse(value).map_err(|error| {
        Error::Protocol(format!(
            "workspace document {DOC_NAME:?} {error}; it was not changed"
        ))
    })
}

fn read_error(error: Error) -> Error {
    Error::Protocol(format!(
        "could not read the workspace document {DOC_NAME:?}: {error}"
    ))
}

fn missing_message(count: usize) -> String {
    if count == 1 {
        "1 pane had no running terminal and was removed".into()
    } else {
        format!("{count} panes had no running terminal and were removed")
    }
}

fn recovered_message(count: usize) -> String {
    if count == 1 {
        "recovered 1 terminal that was missing from the layout".into()
    } else {
        format!("recovered {count} terminals that were missing from the layout")
    }
}

fn join(notes: Vec<String>) -> Option<String> {
    if notes.is_empty() {
        None
    } else {
        Some(notes.join(" · "))
    }
}
