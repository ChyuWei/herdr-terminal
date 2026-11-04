//! Per-pane sizing, attach streams and per-pane protocol (RESIZE, FOCUS, FACTS
//! are sent from the loop and the frame, so this module owns the stream side).

use super::{ATTACH_RETRY, Link, Screen};
use crate::core::{Attached, Error, Result};
use crate::session::{self, Session};
use crate::tui::vt_error;
use crate::vt::Vt;
use crate::workspace::geometry;
use crate::workspace::model::PaneId;
use std::time::Instant;

impl Screen<'_> {
    /// Recomputes visible pane sizes, resizes their VTs and sends RESIZE only
    /// for the panes whose grid actually changed.
    pub(super) fn sync_sizes(&mut self) -> Result<()> {
        let rects = self.rects();
        let client = self.client.clone();
        let mut changed = Vec::new();
        for (pane, rect) in &rects.panes {
            let size = geometry::attach_size(*rect);
            let Some(link) = self.links.get_mut(pane) else {
                continue;
            };
            if link.observed {
                // An observer follows the grid core sends and never sizes the
                // program; its recorded vt_size is that grid already.
                continue;
            }
            if link.vt_size == size {
                continue;
            }
            link.vt.resize(size.0, size.1).map_err(vt_error)?;
            link.vt_size = size;
            link.needs_facts = true;
            changed.push(*pane);
            self.dirty = true;
        }
        let mut lost = false;
        for pane in changed {
            let Some(link) = self.links.get(&pane) else {
                continue;
            };
            let Some(channel) = link.session.channel() else {
                continue;
            };
            if let Err(error) = client.resize(channel, link.vt_size.0, link.vt_size.1) {
                if session::is_lost(&error) {
                    lost = true;
                } else {
                    return Err(error);
                }
            }
        }
        if lost {
            self.begin_reconnect();
        }
        Ok(())
    }

    pub(super) fn attach_due(&mut self) -> Result<()> {
        let now = Instant::now();
        let due: Vec<PaneId> = self
            .links
            .iter()
            .filter(|(_, link)| {
                !link.finalized
                    && link.stream.is_none()
                    && link.retry_at.is_some_and(|at| at <= now)
            })
            .map(|(pane, _)| *pane)
            .collect();
        for pane in due {
            self.attach_pane(pane)?;
            if self.reconn.is_some() {
                break;
            }
        }
        Ok(())
    }

    /// Opens one attach stream, replacing the pane's VT with a fresh one.
    /// Failures are per pane: a finished or removed terminal leaves the
    /// workspace, a parked one keeps retrying, and an unexpected refusal is
    /// reported instead of silently dropping the program.
    pub(super) fn attach_pane(&mut self, pane: PaneId) -> Result<()> {
        let (cols, rows) = self.pane_attach_size(pane);
        let attempt = {
            let Some(link) = self.links.get_mut(&pane) else {
                return Ok(());
            };
            link.session
                .attach(cols, rows, self.config.scrollback)
                .map_err(|error| (error, link.session.address().to_owned()))
        };
        let attached: Attached = match attempt {
            Ok(attached) => attached,
            Err((error, address)) => {
                if session::is_lost(&error) {
                    self.begin_reconnect();
                    return Ok(());
                }
                match &error {
                    Error::Rpc { error: rpc, .. } => match rpc.kind() {
                        Some("unavailable") => {
                            if let Some(link) = self.links.get_mut(&pane) {
                                link.session.waiting();
                                link.retry_at = Some(Instant::now() + ATTACH_RETRY);
                            }
                            self.ui.message = Some(format!(
                                "terminal {address} has no program; waiting for a new shell"
                            ));
                            self.dirty = true;
                        }
                        Some("finished") => {
                            // A terminal this screen started is a local exit:
                            // close its pane. An adopted or restored record
                            // that was already finished keeps its final view.
                            let owned = self
                                .links
                                .get(&pane)
                                .is_some_and(|link| link.session.owned());
                            if owned {
                                // The exact status and cwd come from the
                                // record; the final screen is not read. A
                                // record fetched here also reaches the
                                // replacement, which starts in its cwd.
                                let code = match self
                                    .model
                                    .terminal(&address)
                                    .map(session::record_exit_code)
                                {
                                    Some(code) => Some(code),
                                    None => match self.client.get(&address) {
                                        Ok(record) => {
                                            let code = session::record_exit_code(&record);
                                            self.model.upsert(record);
                                            Some(code)
                                        }
                                        Err(error) if session::is_lost(&error) => {
                                            self.begin_reconnect();
                                            return Ok(());
                                        }
                                        Err(_) => None,
                                    },
                                };
                                self.close_finished_pane(pane, &address, code.unwrap_or(1));
                            } else {
                                self.recover_finished_pane(pane, &address, None);
                            }
                        }
                        Some("not_found") => self.remove_gone(pane, &address),
                        _ => self.forget_pane(
                            pane,
                            &address,
                            &format!("could not be attached: {error}"),
                        ),
                    },
                    _ => {
                        self.forget_pane(pane, &address, &format!("could not be attached: {error}"))
                    }
                }
                return Ok(());
            }
        };

        let vt = Vt::new_with_scrollback(cols, rows, self.config.scrollback).map_err(vt_error)?;
        let observed = attached.resize_notifications;
        if let Some(link) = self.links.get_mut(&pane) {
            link.vt = vt;
            link.vt_size = (cols, rows);
            link.observed = observed;
            link.stream = Some(attached.stream);
            link.retry_at = None;
            link.finished = false;
            link.finalized = false;
            link.removed = false;
            link.needs_facts = true;
            link.last_facts = None;
            link.focus_sent = None;
            self.dirty = true;
        }
        Ok(())
    }

    pub(super) fn add_link(&mut self, pane: PaneId, session: Session) -> Result<()> {
        let (cols, rows) = self.pane_attach_size(pane);
        let vt = Vt::new_with_scrollback(cols, rows, self.config.scrollback).map_err(vt_error)?;
        self.links.insert(
            pane,
            Link {
                session,
                stream: None,
                vt,
                vt_size: (cols, rows),
                observed: false,
                retry_at: Some(Instant::now()),
                finished: false,
                finalized: false,
                removed: false,
                needs_facts: true,
                last_facts: None,
                focus_sent: None,
            },
        );
        self.dirty = true;
        Ok(())
    }

    /// Tells every attached pane whether it is the focused one and the outer
    /// window has focus. Only transitions are sent.
    pub(super) fn sync_focus(&mut self) -> Result<()> {
        let focused = self.workspace.focused_pane();
        let outer = self.outer_focused;
        let client = self.client.clone();
        let mut updates = Vec::new();
        for (pane, link) in &mut self.links {
            if link.observed {
                link.focus_sent = None;
                continue;
            }
            if link.stream.is_none() {
                link.focus_sent = None;
                continue;
            }
            let want = Some(*pane) == focused && outer;
            if link.focus_sent == Some(want) {
                continue;
            }
            if let Some(channel) = link.session.channel() {
                link.focus_sent = Some(want);
                updates.push((channel, want));
            }
        }
        for (channel, want) in updates {
            if let Err(error) = client.focus(channel, want) {
                if session::is_lost(&error) {
                    self.begin_reconnect();
                    return Ok(());
                }
                return Err(error);
            }
        }
        Ok(())
    }

    /// Enables outer mouse reporting whenever local mouse handling is on.
    /// Clicks, wheel and drags are then interpreted locally first: chrome
    /// clicks focus panes or tabs, non-tracking programs scroll their local
    /// history, and only a program that tracks the mouse receives its events.
    pub(super) fn sync_mouse(&mut self) -> Result<()> {
        let on = self.config.mouse;
        if on != self.mouse_on {
            self.guard.set_mouse(on)?;
            self.mouse_on = on;
        }
        Ok(())
    }
}
