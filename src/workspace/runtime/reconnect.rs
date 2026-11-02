//! One dropped connection: pause, retry with bounded backoff, resubscribe,
//! point every session at the fresh client and reattach in place.

use super::{Reconnect, Screen, TICK};
use crate::core::{CoreClient, Error, Result};
use crate::model::Model;
use crate::screen::lifecycle;
use crate::vt;
use crate::workspace::keys;
use crate::workspace::state::Connection;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::sync::Arc;
use std::time::Instant;

impl Screen<'_> {
    pub(super) fn begin_reconnect(&mut self) {
        if self.reconn.is_some() {
            return;
        }
        for link in self.links.values_mut() {
            link.stream = None;
            link.session.lost();
            link.retry_at = None;
            link.finished = false;
            link.removed = false;
            link.focus_sent = None;
            link.needs_facts = false;
            link.last_facts = None;
        }
        self.reconn = Some(Reconnect {
            attempt: 0,
            next: Instant::now(),
        });
        self.ui.connection = Connection::Reconnecting { attempt: 0 };
        self.ui.prefix = false;
        self.ui.help = false;
        self.ui.prompt = None;
        self.ui.message =
            Some("lost the connection to core; the programs may still be running".into());
        self.dirty = true;
        if self.mouse_on {
            let _ = self.guard.set_mouse(false);
            self.mouse_on = false;
        }
    }

    pub(super) fn reconnect_step(&mut self) -> Result<()> {
        let Some((attempt, next)) = self
            .reconn
            .as_ref()
            .map(|state| (state.attempt, state.next))
        else {
            return Ok(());
        };
        if Instant::now() < next {
            if event::poll(TICK).map_err(Error::Io)? {
                match event::read().map_err(Error::Io)? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        if vt::is_detach(&key) || (self.ui.prefix && key.code == KeyCode::Char('q'))
                        {
                            self.detach = true;
                        } else if key.modifiers.contains(KeyModifiers::CONTROL)
                            && key.code == KeyCode::Char(keys::PREFIX)
                        {
                            self.ui.prefix = true;
                            self.dirty = true;
                        } else {
                            self.ui.prefix = false;
                            self.dirty = true;
                        }
                    }
                    Event::Resize(cols, rows) if cols > 0 && rows > 0 => {
                        self.size = (cols, rows);
                        self.dirty = true;
                    }
                    _ => {}
                }
            }
            return Ok(());
        }
        if self.try_reconnect()? {
            return Ok(());
        }
        let attempt = attempt.saturating_add(1);
        if let Some(state) = self.reconn.as_mut() {
            state.attempt = attempt;
            state.next = Instant::now() + lifecycle::backoff(attempt);
        }
        self.ui.connection = Connection::Reconnecting { attempt };
        self.ui.message = Some(lifecycle::reconnect_message(attempt));
        self.dirty = true;
        Ok(())
    }

    pub(super) fn try_reconnect(&mut self) -> Result<bool> {
        let client = match CoreClient::connect(self.hc.as_deref()) {
            Ok(client) => Arc::new(client),
            Err(error) if lifecycle::retryable(&error) => return Ok(false),
            Err(error) => return Err(error),
        };
        let events = match client.subscribe() {
            Ok(events) => events,
            Err(error) if lifecycle::retryable(&error) => return Ok(false),
            Err(error) => return Err(error),
        };
        let model = Model::from_snapshot(events.snapshot());
        if self.verbose {
            eprintln!("ht: reconnected to core");
        }
        self.client = client.clone();
        self.events = events;
        self.model = model;
        self.reconn = None;
        self.ui.connection = Connection::Live;
        for link in self.links.values_mut() {
            link.session.set_client(client.clone());
            link.stream = None;
            link.retry_at = None;
            link.finished = false;
            link.removed = false;
            link.focus_sent = None;
            link.last_facts = None;
            link.needs_facts = false;
        }
        self.reconcile_snapshot();
        if self.workspace.pane_count() > 0 {
            self.ui.message = Some("reconnected to core".into());
        }
        self.dirty = true;
        Ok(true)
    }
}
