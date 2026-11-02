//! The screen's live state machine: one terminal, one connection at a time.
//!
//! [`Screen`] owns the VT, the attach stream, the event subscription and the
//! pure [`Model`]. It moves between three states:
//!
//! * **attached**: core's bytes are fed to the VT, events update the model,
//!   input goes back as DATA, and frames are drawn;
//! * **waiting**: a kept terminal lost its program and waits for core to give
//!   it a fresh shell (`restarted`);
//! * **reconnecting**: the connection died. The programs keep running in core;
//!   the screen retries with bounded backoff, resubscribes and reattaches the
//!   same terminal instead of starting a duplicate.
//!
//! Ctrl-] is handled in every state: it closes the attach stream, leaves the
//! program running, and leaves the pointer for the next client.

use super::lifecycle::{Disposition, backoff, disposition, reconnect_message, retryable};
use super::pointer;
use super::{DRAIN_BYTES, DRAIN_TIME, FRAME, ScreenGuard, ScreenTerminal, TICK};
use crate::core::{
    Attached, CoreClient, Display, Error, EventKind, EventStream, Notification, Result, RunOptions,
    StreamItem, Terminal,
};
use crate::model::Model;
use crate::session::{self, Session};
use crate::vt::{self, Screen as VtScreen, Vt};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{self, Event};
use crossterm::execute;
use serde_json::Value;
use std::io::stdout;
use std::sync::Arc;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

/// How long a parked terminal waits before another attach attempt.
const ATTACH_RETRY: Duration = Duration::from_millis(500);
/// The most event notifications one loop iteration applies before drawing.
const EVENT_BATCH: usize = 256;

/// How the screen first enters its loop.
pub(super) enum First {
    /// The terminal is attached and its stream is ready.
    Attached(Box<Attached>),
    /// A kept terminal is parked and waits for `restarted`.
    Waiting,
}

/// Why the screen loop ended.
pub(super) enum Outcome {
    /// Core closed the stream: the program ended. `reason` is the stream's
    /// close reason; `record` is the model's record when it has one.
    Program {
        reason: Option<Value>,
        record: Option<Box<Terminal>>,
    },
    /// The person pressed Ctrl-].
    Quit,
    /// The terminal disappeared while the screen was showing it.
    Removed,
}

enum Flow {
    Next(State),
    Done(Outcome),
}

enum State {
    Attached(Box<Attached>),
    Waiting,
    Reconnecting { attempt: u32, next: Instant },
}

/// What one attach attempt produced.
enum Attempt {
    Attached(Attached),
    /// The kept terminal is waiting for its new shell.
    Unavailable,
    /// The terminal ended; its record carries the status.
    Finished(Option<Terminal>),
    /// The terminal was removed.
    Removed,
    /// The connection died during the attempt.
    Lost,
}

pub(super) struct Screen<'a> {
    cols: u16,
    rows: u16,
    hc: Option<std::path::PathBuf>,
    argv: Vec<String>,
    verbose: bool,
    /// The core's stable id, used to scope the local pointer.
    core_id: String,
    address: String,
    session: Session,
    client: Arc<CoreClient>,
    events: EventStream,
    model: Model,
    terminal: ScreenTerminal,
    vt: Vt,
    guard: &'a mut ScreenGuard,
    last_facts: Option<Display>,
    last_cursor_style: Option<SetCursorStyle>,
    mouse_on: bool,
    dirty: bool,
    last_draw: Instant,
    drew_first_frame: bool,
    message: Option<String>,
    /// A `finished` event arrived; the stream's CLOSE is still coming.
    finished: Option<Terminal>,
    /// A `removed` event (or a `not_found` attach) named this terminal.
    removed: bool,
}

impl<'a> Screen<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        cols: u16,
        rows: u16,
        options: &super::Options,
        guard: &'a mut ScreenGuard,
        client: Arc<CoreClient>,
        events: EventStream,
        model: Model,
        session: Session,
    ) -> Result<Screen<'a>> {
        let vt = Vt::new(cols, rows).map_err(super::vt_error)?;
        let terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout()))
            .map_err(Error::Io)?;
        Ok(Screen {
            cols,
            rows,
            hc: options.hc.clone(),
            argv: options.argv.clone(),
            verbose: options.verbose,
            core_id: client.hello().core.id.clone(),
            address: session.address().to_owned(),
            session,
            client,
            events,
            model,
            terminal,
            vt,
            guard,
            last_facts: None,
            last_cursor_style: None,
            mouse_on: false,
            dirty: true,
            last_draw: Instant::now() - FRAME,
            drew_first_frame: false,
            message: None,
            finished: None,
            removed: false,
        })
    }

    /// Runs until the program ends, the viewer detaches, or a fatal local
    /// error. Returns the session and the last core id (a reconnect may have
    /// moved to a different core), so the caller can finish the lifecycle.
    pub(super) fn run(mut self, first: First) -> (Result<Outcome>, Session, String) {
        let result = self.play(first);
        (result, self.session, self.core_id)
    }

    fn play(&mut self, first: First) -> Result<Outcome> {
        let mut state = match first {
            First::Attached(attached) => State::Attached(attached),
            First::Waiting => State::Waiting,
        };
        loop {
            let flow = match state {
                State::Attached(attached) => self.attached(*attached)?,
                State::Waiting => self.waiting()?,
                State::Reconnecting { attempt, next } => self.reconnecting(attempt, next)?,
            };
            match flow {
                Flow::Done(outcome) => return Ok(outcome),
                Flow::Next(next) => state = next,
            }
        }
    }

    // --- attached ---------------------------------------------------------

    fn attached(&mut self, attached: Attached) -> Result<Flow> {
        let channel = attached.channel;
        // A new attach gets a fresh VT: core's first paint is the whole truth
        // for the new stream, and stale cells from the old one must not leak.
        self.vt = Vt::new(self.cols, self.rows).map_err(super::vt_error)?;
        self.drew_first_frame = false;
        self.finished = None;
        self.removed = false;
        self.message = None;
        self.dirty = true;
        self.last_facts = None;
        self.last_draw = Instant::now() - FRAME;
        pointer::pointer(&self.core_id, &self.address).save();
        if self.verbose {
            eprintln!(
                "attached to {} on channel {channel} at {}x{}",
                self.address, self.cols, self.rows
            );
        }
        let stream = attached.stream;
        loop {
            // Take what core has sent, but only up to a budget: a flood must
            // not keep input, events and drawing from running.
            let mut had_data = false;
            let mut closing = None;
            let mut drained = 0usize;
            let drain_start = Instant::now();
            loop {
                match stream.try_recv() {
                    Ok(StreamItem::Data(bytes)) => {
                        had_data = true;
                        drained = drained.saturating_add(bytes.len());
                        self.vt.feed(&bytes);
                        self.client.credit(channel, bytes.len())?;
                        self.dirty = true;
                        if !self.drew_first_frame
                            || drained >= DRAIN_BYTES
                            || drain_start.elapsed() >= DRAIN_TIME
                        {
                            break;
                        }
                    }
                    Ok(StreamItem::Closed(reason)) => {
                        closing = Some(reason);
                        break;
                    }
                    Ok(StreamItem::Lost) | Err(TryRecvError::Disconnected) => {
                        self.lost_connection();
                        return Ok(Flow::Next(State::Reconnecting {
                            attempt: 0,
                            next: Instant::now(),
                        }));
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }

            if let Some(reason) = closing {
                match self.disposition(&reason) {
                    Disposition::Finished => {
                        // Bytes fed in this drain are still the program's last
                        // output; draw them before the terminal is restored.
                        self.draw_frame(false)?;
                        let record = self.model.terminal(&self.address).cloned();
                        return Ok(Flow::Done(Outcome::Program {
                            reason,
                            record: record.map(Box::new),
                        }));
                    }
                    Disposition::Lost => {
                        self.session.waiting();
                        self.message = Some("terminal lost; waiting for a new shell".into());
                        self.dirty = true;
                        self.draw_frame(false)?;
                        return Ok(Flow::Next(State::Waiting));
                    }
                    Disposition::Removed => {
                        self.removed = true;
                        return Ok(Flow::Done(Outcome::Removed));
                    }
                }
            }

            // Facts from the event stream. Bounded so a burst of events cannot
            // starve input; every event is small.
            for _ in 0..EVENT_BATCH {
                match self.events.try_recv() {
                    Ok(notification) => {
                        if self.model.apply(&notification) {
                            self.dirty = true;
                        }
                        if let Some(flow) = self.event_flow(&notification)? {
                            return Ok(flow);
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.lost_connection();
                        return Ok(Flow::Next(State::Reconnecting {
                            attempt: 0,
                            next: Instant::now(),
                        }));
                    }
                }
            }

            // The program's mouse mode decides whether the outer terminal
            // should report the mouse at all; the encoder filters the rest.
            let tracking = self.vt.mouse_tracking();
            if tracking != self.mouse_on {
                self.guard.set_mouse(tracking)?;
                self.mouse_on = tracking;
            }

            let timeout = if had_data { Duration::ZERO } else { TICK };
            if event::poll(timeout).map_err(Error::Io)? {
                match event::read().map_err(Error::Io)? {
                    Event::Key(key) => {
                        if vt::is_detach(&key) {
                            return Ok(Flow::Done(Outcome::Quit));
                        }
                        let bytes = self.vt.encode_key(&key);
                        if !bytes.is_empty()
                            && let Err(error) = self.client.keys(channel, &bytes)
                        {
                            if session::is_lost(&error) {
                                self.lost_connection();
                                return Ok(Flow::Next(State::Reconnecting {
                                    attempt: 0,
                                    next: Instant::now(),
                                }));
                            }
                            return Err(error);
                        }
                    }
                    Event::Mouse(mouse) => {
                        let bytes = self.vt.encode_mouse(&mouse);
                        if !bytes.is_empty()
                            && let Err(error) = self.client.keys(channel, &bytes)
                        {
                            if session::is_lost(&error) {
                                self.lost_connection();
                                return Ok(Flow::Next(State::Reconnecting {
                                    attempt: 0,
                                    next: Instant::now(),
                                }));
                            }
                            return Err(error);
                        }
                    }
                    Event::Resize(cols, rows) if cols > 0 && rows > 0 => {
                        self.vt.resize(cols, rows).map_err(super::vt_error)?;
                        self.cols = cols;
                        self.rows = rows;
                        self.client.resize(channel, cols, rows)?;
                        self.dirty = true;
                    }
                    Event::FocusGained => self.client.focus(channel, true)?,
                    Event::FocusLost => self.client.focus(channel, false)?,
                    _ => {}
                }
            }

            // The first frame that follows attach is drawn at once, before a
            // flood can scroll it away; later frames keep the throttle.
            let first_frame = !self.drew_first_frame && had_data;
            if self.dirty && (first_frame || self.last_draw.elapsed() >= FRAME) {
                self.draw_frame(true)?;
                self.last_draw = Instant::now();
                self.dirty = false;
                if first_frame {
                    self.drew_first_frame = true;
                }
            }
        }
    }

    /// Turns one event into a lifecycle change, if it concerns this terminal.
    fn event_flow(&mut self, notification: &Notification) -> Result<Option<Flow>> {
        match notification {
            Notification::Missed { .. } => {
                // The model already replaced the snapshot. The old attach
                // stream may be from a replaced terminal; reattach cleanly.
                self.finished = None;
                self.removed = false;
                match self.model.terminal(&self.address) {
                    Some(record) if record.is_live() => {
                        let _ = self.session.detach_current();
                        Ok(Some(Flow::Next(State::Waiting)))
                    }
                    Some(_) => {
                        self.draw_frame(false)?;
                        let record = self.model.terminal(&self.address).cloned();
                        Ok(Some(Flow::Done(Outcome::Program {
                            reason: None,
                            record: record.map(Box::new),
                        })))
                    }
                    None => {
                        self.removed = true;
                        Ok(Some(Flow::Done(Outcome::Removed)))
                    }
                }
            }
            Notification::Event {
                kind,
                terminal: Some(address),
                ..
            } if *address == self.address => match kind {
                EventKind::Restarted => {
                    let _ = self.session.detach_current();
                    self.finished = None;
                    self.removed = false;
                    self.message = Some("reattaching to the new shell…".into());
                    self.dirty = true;
                    Ok(Some(Flow::Next(State::Waiting)))
                }
                EventKind::Lost => {
                    let _ = self.session.detach_current();
                    self.finished = None;
                    self.session.waiting();
                    self.message = Some("terminal lost; waiting for a new shell".into());
                    self.dirty = true;
                    Ok(Some(Flow::Next(State::Waiting)))
                }
                EventKind::Finished => {
                    self.finished = self.model.terminal(&self.address).cloned();
                    Ok(None)
                }
                EventKind::Removed => {
                    self.removed = true;
                    self.session.lost();
                    Ok(Some(Flow::Done(Outcome::Removed)))
                }
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }

    // --- waiting for a new shell -----------------------------------------

    fn waiting(&mut self) -> Result<Flow> {
        self.message = Some("terminal lost; waiting for a new shell".into());
        self.dirty = true;
        let mut next_attach = Instant::now();
        let mut last_draw = Instant::now() - FRAME;
        loop {
            if Instant::now() >= next_attach {
                match self.try_attach()? {
                    Attempt::Attached(attached) => {
                        return Ok(Flow::Next(State::Attached(Box::new(attached))));
                    }
                    Attempt::Unavailable => {
                        next_attach = Instant::now() + ATTACH_RETRY;
                    }
                    Attempt::Finished(record) => {
                        self.draw_frame(false)?;
                        return Ok(Flow::Done(Outcome::Program {
                            reason: None,
                            record: record.map(Box::new),
                        }));
                    }
                    Attempt::Removed => {
                        self.removed = true;
                        return Ok(Flow::Done(Outcome::Removed));
                    }
                    Attempt::Lost => {
                        self.lost_connection();
                        return Ok(Flow::Next(State::Reconnecting {
                            attempt: 0,
                            next: Instant::now(),
                        }));
                    }
                }
            }

            for _ in 0..EVENT_BATCH {
                match self.events.try_recv() {
                    Ok(notification) => {
                        if self.model.apply(&notification) {
                            self.dirty = true;
                        }
                        match &notification {
                            Notification::Event {
                                kind,
                                terminal: Some(address),
                                ..
                            } if *address == self.address => match kind {
                                EventKind::Restarted => next_attach = Instant::now(),
                                EventKind::Finished => {
                                    let record = self.model.terminal(&self.address).cloned();
                                    self.draw_frame(false)?;
                                    return Ok(Flow::Done(Outcome::Program {
                                        reason: None,
                                        record: record.map(Box::new),
                                    }));
                                }
                                EventKind::Removed => {
                                    self.removed = true;
                                    return Ok(Flow::Done(Outcome::Removed));
                                }
                                _ => {}
                            },
                            Notification::Missed { .. } => {
                                match self.model.terminal(&self.address) {
                                    Some(record) if record.is_live() => {
                                        if record.pid.is_some() {
                                            next_attach = Instant::now();
                                        }
                                    }
                                    Some(_) => {
                                        let record = self.model.terminal(&self.address).cloned();
                                        self.draw_frame(false)?;
                                        return Ok(Flow::Done(Outcome::Program {
                                            reason: None,
                                            record: record.map(Box::new),
                                        }));
                                    }
                                    None => {
                                        self.removed = true;
                                        return Ok(Flow::Done(Outcome::Removed));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.lost_connection();
                        return Ok(Flow::Next(State::Reconnecting {
                            attempt: 0,
                            next: Instant::now(),
                        }));
                    }
                }
            }

            if self.input_without_stream()? {
                return Ok(Flow::Done(Outcome::Quit));
            }
            if self.dirty && last_draw.elapsed() >= FRAME {
                self.draw_frame(false)?;
                last_draw = Instant::now();
                self.dirty = false;
            }
        }
    }

    // --- reconnecting -----------------------------------------------------

    fn reconnecting(&mut self, mut attempt: u32, mut next: Instant) -> Result<Flow> {
        self.message = Some(reconnect_message(attempt));
        self.dirty = true;
        let mut last_draw = Instant::now() - FRAME;
        loop {
            if Instant::now() >= next {
                match self.try_reconnect()? {
                    Some(flow) => return Ok(flow),
                    None => {
                        attempt = attempt.saturating_add(1);
                        next = Instant::now() + backoff(attempt);
                        self.message = Some(reconnect_message(attempt));
                        self.dirty = true;
                    }
                }
            }
            if self.input_without_stream()? {
                return Ok(Flow::Done(Outcome::Quit));
            }
            if self.dirty && last_draw.elapsed() >= FRAME {
                self.draw_frame(false)?;
                last_draw = Instant::now();
                self.dirty = false;
            }
        }
    }

    /// One reconnect attempt. `None` means "try again later"; a `Flow` means
    /// the screen has a connection (or the terminal ended).
    fn try_reconnect(&mut self) -> Result<Option<Flow>> {
        let client = match CoreClient::connect(self.hc.as_deref()) {
            Ok(client) => Arc::new(client),
            Err(error) if retryable(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        let events = match client.subscribe() {
            Ok(events) => events,
            Err(error) if retryable(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        self.core_id = client.hello().core.id.clone();
        self.model = Model::from_snapshot(events.snapshot());
        self.events = events;
        self.client = client.clone();
        self.session.set_client(client.clone());
        if self.verbose {
            eprintln!(
                "reconnected to core {} at seq {}",
                self.core_id,
                self.model.seq()
            );
        }

        let record = self.model.terminal(&self.address).cloned();
        match record {
            Some(record) if record.is_live() && record.pid.is_some() => match self.try_attach()? {
                Attempt::Attached(attached) => {
                    Ok(Some(Flow::Next(State::Attached(Box::new(attached)))))
                }
                Attempt::Unavailable => Ok(Some(Flow::Next(State::Waiting))),
                Attempt::Finished(record) => Ok(Some(Flow::Done(Outcome::Program {
                    reason: None,
                    record: record.map(Box::new),
                }))),
                Attempt::Removed => Ok(Some(Flow::Done(Outcome::Removed))),
                Attempt::Lost => Ok(None),
            },
            Some(record) if record.is_live() => {
                // Kept but parked: wait for the `restarted` event.
                Ok(Some(Flow::Next(State::Waiting)))
            }
            Some(record) => {
                // The program ended while the connection was down.
                self.draw_frame(false)?;
                Ok(Some(Flow::Done(Outcome::Program {
                    reason: None,
                    record: Some(Box::new(record)),
                })))
            }
            None => {
                // The terminal is gone (a fresh core state, or it was
                // removed). There is nothing to duplicate, so start one.
                self.session.lost();
                let run = RunOptions {
                    argv: self.argv.clone(),
                    keep: true,
                    rm: false,
                    ..RunOptions::default()
                };
                match Session::start(client, &run) {
                    Ok(session) => {
                        self.address = session.address().to_owned();
                        self.session = session;
                        pointer::pointer(&self.core_id, &self.address).save();
                        match self.try_attach()? {
                            Attempt::Attached(attached) => {
                                Ok(Some(Flow::Next(State::Attached(Box::new(attached)))))
                            }
                            Attempt::Unavailable => Ok(Some(Flow::Next(State::Waiting))),
                            Attempt::Finished(record) => Ok(Some(Flow::Done(Outcome::Program {
                                reason: None,
                                record: record.map(Box::new),
                            }))),
                            Attempt::Removed => Ok(Some(Flow::Done(Outcome::Removed))),
                            Attempt::Lost => Ok(None),
                        }
                    }
                    Err(error) if retryable(&error) => Ok(None),
                    Err(error) => Err(error),
                }
            }
        }
    }

    // --- shared helpers ---------------------------------------------------

    fn try_attach(&mut self) -> Result<Attempt> {
        match self.session.attach(self.cols, self.rows) {
            Ok(attached) => Ok(Attempt::Attached(attached)),
            // Checked before the RPC kinds: a call interrupted by a dead
            // connection is also an `Error::Rpc`, with kind `lost`.
            Err(error) if session::is_lost(&error) => Ok(Attempt::Lost),
            Err(Error::Rpc { error, .. }) => match error.kind() {
                Some("finished") => Ok(Attempt::Finished(
                    self.model.terminal(&self.address).cloned(),
                )),
                Some("unavailable") => Ok(Attempt::Unavailable),
                Some("not_found") => Ok(Attempt::Removed),
                _ => Err(Error::Rpc {
                    method: "terminal.attach".into(),
                    error,
                }),
            },
            Err(error) => Err(error),
        }
    }

    /// What a closed attach stream means for this terminal.
    fn disposition(&self, reason: &Option<Value>) -> Disposition {
        disposition(
            self.model.terminal(&self.address),
            self.finished.is_some(),
            self.removed,
            reason,
        )
    }

    /// Reads local input when there is no attach stream. Only Ctrl-] matters;
    /// a resize is kept so the next attach uses the current size.
    fn input_without_stream(&mut self) -> Result<bool> {
        if !event::poll(TICK).map_err(Error::Io)? {
            return Ok(false);
        }
        match event::read().map_err(Error::Io)? {
            Event::Key(key) if vt::is_detach(&key) => Ok(true),
            Event::Resize(cols, rows) if cols > 0 && rows > 0 => {
                self.vt.resize(cols, rows).map_err(super::vt_error)?;
                self.cols = cols;
                self.rows = rows;
                self.dirty = true;
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    fn lost_connection(&mut self) {
        // The connection is gone, not the program: leave it running. A later
        // reconnect adopts the same address.
        self.session.lost();
        self.message = Some("lost the connection to core; the program may still be running".into());
        self.dirty = true;
    }

    fn draw_frame(&mut self, send_facts: bool) -> Result<()> {
        let screen: VtScreen = self.vt.snapshot().map_err(super::vt_error)?;
        let message = self.message.clone();
        self.terminal
            .draw(|frame| super::view::draw(frame, &screen, message.as_deref()))
            .map_err(Error::Io)?;
        let shape = super::view::cursor_shape(&screen.cursor);
        if self.last_cursor_style != Some(shape) {
            execute!(stdout(), shape).map_err(Error::Io)?;
            self.last_cursor_style = Some(shape);
        }
        if send_facts && let Some(channel) = self.session.channel() {
            let facts = super::display(&screen);
            if self.last_facts.as_ref() != Some(&facts) {
                self.client.facts(channel, &facts)?;
                self.last_facts = Some(facts);
            }
        }
        Ok(())
    }
}
