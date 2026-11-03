//! The lifecycle of a terminal this client manages.
//!
//! One [`Session`] owns the terminal address from `terminal.run` (or adopts an
//! address found again after a client restart) until the program ends or the
//! viewer leaves. Its normal paths are explicit: [`Session::remove_finished`]
//! after the program exited by itself, [`Session::detach`] when the viewer
//! leaves (the program keeps running), [`Session::waiting`] when a kept
//! terminal lost its program, and [`Session::lost`] when core is unreachable.
//! Any other early return runs `Drop`, which cleans up an owned terminal and
//! only detaches an adopted one.
//!
//! [`recover_finished`] handles the race where a program exits before
//! `terminal.attach` lands: the run keeps its record (`rm: false`), so the
//! final screen and exit status can still be read and then removed.

use crate::core::{Attached, CoreClient, Error, Result, RpcError, Terminal};
use serde_json::Value;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A terminal managed by this client, with cleanup on every path.
///
/// A session is either **owned** (this process ran `terminal.run`) or adopted
/// (this process reattached to a terminal a previous client left running). A
/// local error ends an owned terminal; an adopted one is only detached, so a
/// client restart can never kill the program it came back for.
pub struct Session {
    client: Arc<CoreClient>,
    address: String,
    channel: Option<u32>,
    done: bool,
    owned: bool,
}

impl Session {
    /// Runs a program (or core's shell) and takes ownership of its terminal.
    pub fn start(client: Arc<CoreClient>, options: &crate::core::RunOptions) -> Result<Session> {
        let address = client.run(options)?.address;
        Ok(Session {
            client,
            address,
            channel: None,
            done: false,
            owned: true,
        })
    }

    /// Reattaches to a terminal this process did not start (discovered from
    /// the pointer or its labels). Its program is not ours to kill on a local
    /// error.
    pub fn adopt(client: Arc<CoreClient>, address: String) -> Session {
        Session {
            client,
            address,
            channel: None,
            done: false,
            owned: false,
        }
    }

    /// Reclaims a terminal this process started whose `terminal.run` reply was
    /// lost with the connection. Core created it; it is still ours to clean up.
    pub fn reclaim(client: Arc<CoreClient>, address: String) -> Session {
        Session {
            client,
            address,
            channel: None,
            done: false,
            owned: true,
        }
    }

    pub fn client(&self) -> &Arc<CoreClient> {
        &self.client
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    /// Whether this process ran the terminal (as opposed to adopting it).
    pub fn owned(&self) -> bool {
        self.owned
    }

    /// The attach channel, once attached.
    pub fn channel(&self) -> Option<u32> {
        self.channel
    }

    /// Attaches at the given size, asking for `history` lines above the
    /// screen, and remembers the channel for cleanup. A successful attach
    /// clears the "leave it alone" state a reconnect or a loss set.
    pub fn attach(&mut self, cols: u16, rows: u16, history: usize) -> Result<Attached> {
        let attached = self.client.attach(&self.address, cols, rows, history)?;
        self.channel = Some(attached.channel);
        self.done = false;
        Ok(attached)
    }

    /// Points the session at a fresh connection after a reconnect. The old
    /// channel belonged to the dead connection and is not reusable.
    pub fn set_client(&mut self, client: Arc<CoreClient>) {
        self.client = client;
        self.channel = None;
        self.done = false;
    }

    /// Closes the current attach stream, if any, without ending the program.
    pub fn detach_current(&mut self) -> Result<()> {
        match self.channel.take() {
            Some(channel) => self.client.detach(channel),
            None => Ok(()),
        }
    }

    /// The viewer left the screen: detach and leave the program running. No
    /// `terminal.kill` or `terminal.rm` is sent, so a later `ht` reattaches.
    pub fn detach(&mut self) -> Result<()> {
        self.done = true;
        self.detach_current()
    }

    /// The program ended by itself: forget its finished record.
    pub fn remove_finished(&mut self) -> Result<()> {
        self.done = true;
        self.channel = None;
        missing_ok(self.client.rm(&self.address))
    }

    /// The program ended and its attach stream closed. The record and final
    /// screen stay until the pane is closed; no `terminal.rm` is sent yet.
    pub fn finished(&mut self) {
        self.done = true;
        self.channel = None;
    }

    /// A local error after `terminal.run`: end the terminal this process
    /// started; an adopted terminal is only detached, never killed.
    pub fn cleanup(&mut self) -> Result<()> {
        if self.owned {
            self.end()
        } else {
            self.detach()
        }
    }

    /// The program ended by itself: detach, end the program, and remove the
    /// record. Each step is tried even if an earlier one fails, so a refused
    /// detach cannot leave the program running.
    pub fn end(&mut self) -> Result<()> {
        self.done = true;
        let mut first = None;
        if let Some(channel) = self.channel.take()
            && let Err(error) = self.client.detach(channel)
        {
            first.get_or_insert(error);
        }
        if let Err(error) = self.client.kill(&self.address) {
            first.get_or_insert(error);
        }
        if let Err(error) = missing_ok(self.client.rm(&self.address)) {
            first.get_or_insert(error);
        }
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// The program finished before attach and its screen could not be read;
    /// keep the record so `hc read` can still show it.
    pub fn keep(&mut self) {
        self.done = true;
    }

    /// Core is unreachable. The program may still be running.
    pub fn lost(&mut self) {
        self.done = true;
        self.channel = None;
    }

    /// A kept terminal lost its program and waits for a fresh shell. Nothing
    /// is detached or killed; a later `restarted` reattaches.
    pub fn waiting(&mut self) {
        self.done = true;
        self.channel = None;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.done {
            // A local error after the terminal appeared (a write, a thread
            // spawn, an attach failure): clean up what this process owns.
            let _ = self.cleanup();
        }
    }
}

/// A name unique among live terminals, used only so a workspace can reclaim a
/// terminal whose `terminal.run` reply was lost. Core accepts 1-32 characters
/// from `a-z0-9-_.`; the process id and a counter keep it unique in practice.
pub fn start_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let seq = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("ht-{:x}-{:x}", std::process::id(), seq)
}

/// A program that exits before `terminal.attach` lands has no stream, but core
/// keeps its final screen (`rm: false`). Show that screen and its status, then
/// remove the finished record. If the screen cannot be read, keep the record
/// and fail clearly rather than silently losing the output.
pub fn recover_finished(
    session: &mut Session,
    out: &mut impl Write,
    attach_error: &RpcError,
) -> Result<i32> {
    let record = match session.client.get(&session.address) {
        Ok(record) => record,
        Err(error) if is_lost(&error) => {
            session.lost();
            return Err(error);
        }
        Err(error) => {
            session.keep();
            return Err(Error::Protocol(format!(
                "the program exited before attach and its terminal could not be read \
                 (attach: {attach_error}; terminal.get: {error}); exit status unknown, \
                 output not captured"
            )));
        }
    };
    let code = record_exit_code(&record);
    let read = match session.client.read(&session.address, 0) {
        Ok(read) => read,
        Err(error) if is_lost(&error) => {
            session.lost();
            return Err(error);
        }
        Err(error) => {
            session.keep();
            return Err(Error::Protocol(format!(
                "the program exited before attach (exit status {code}); its output could not \
                 be recovered (attach: {attach_error}; terminal.read: {error}); terminal {} was \
                 kept: hc read {}",
                session.address, session.address
            )));
        }
    };
    if !read.text.is_empty() {
        let written = out
            .write_all(read.text.as_bytes())
            .and_then(|()| out.write_all(b"\n"))
            .and_then(|()| out.flush());
        if let Err(error) = written {
            // The screen is the only copy left; keep it for `hc read`.
            session.keep();
            return Err(Error::Protocol(format!(
                "cannot write terminal output: {error}"
            )));
        }
    }
    if let Err(error) = session.remove_finished() {
        eprintln!(
            "ht: warning: could not remove finished terminal {}: {error}",
            session.address
        );
    }
    Ok(code)
}

pub fn lost_error() -> Error {
    Error::Protocol("lost the connection to core; the program may still be running".into())
}

/// Whether an error means the connection is gone (rather than core refusing).
pub fn is_lost(error: &Error) -> bool {
    match error {
        Error::Closed => true,
        Error::Rpc { error, .. } => error.kind() == Some("lost"),
        _ => false,
    }
}

/// `terminal.rm` on a record that is already gone is the outcome we wanted.
pub fn missing_ok(result: Result<()>) -> Result<()> {
    match result {
        Err(Error::Rpc { error, .. }) if error.kind() == Some("not_found") => Ok(()),
        other => other,
    }
}

/// The exit status to report for a core CLOSE reason.
pub fn exit_code_from(reason: Option<&Value>) -> i32 {
    let Some(reason) = reason else { return 0 };
    if let Some(code) = reason.get("exit_code").and_then(Value::as_i64) {
        return code as i32;
    }
    if let Some(signal) = reason.get("signal").and_then(Value::as_i64) {
        return 128 + signal as i32;
    }
    if reason.get("reason").and_then(Value::as_str) == Some("exited") {
        0
    } else {
        1
    }
}

/// The exit status to show when a program ended: a finished record is
/// authoritative; a record that still looks live must not override the CLOSE
/// reason core just sent (a live record has no status of its own).
pub fn finished_exit_code(record: Option<&Terminal>, reason: Option<&Value>) -> i32 {
    match record {
        Some(record) if record.state == "finished" => record_exit_code(record),
        _ if reason.is_some() => exit_code_from(reason),
        Some(record) => record_exit_code(record),
        None => 0,
    }
}

/// The exit status to report for a finished terminal record.
pub fn record_exit_code(record: &Terminal) -> i32 {
    if let Some(code) = record.exit_code {
        return code as i32;
    }
    if let Some(signal) = record.signal {
        return 128 + signal as i32;
    }
    if record.reason.as_deref() == Some("exited") {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_programs_exit_status_is_reported() {
        assert_eq!(exit_code_from(None), 0, "a viewer detach has no reason");
        assert_eq!(
            exit_code_from(Some(&json!({"reason": "exited", "exit_code": 0}))),
            0
        );
        assert_eq!(
            exit_code_from(Some(&json!({"reason": "exited", "exit_code": 7}))),
            7
        );
        assert_eq!(
            exit_code_from(Some(&json!({"reason": "killed", "signal": 9}))),
            137
        );
        assert_eq!(exit_code_from(Some(&json!({"reason": "killed"}))), 1);
    }

    #[test]
    fn a_finished_records_exit_status_is_reported() {
        let mut record = Terminal {
            reason: Some("exited".into()),
            exit_code: Some(7),
            ..Terminal::default()
        };
        assert_eq!(record_exit_code(&record), 7);
        record.exit_code = None;
        record.signal = Some(9);
        assert_eq!(record_exit_code(&record), 137);
        record.signal = None;
        record.reason = Some("killed".into());
        assert_eq!(record_exit_code(&record), 1);
        record.reason = Some("exited".into());
        assert_eq!(record_exit_code(&record), 0);
    }

    #[test]
    fn a_live_record_does_not_hide_the_close_reason() {
        let live = Terminal {
            state: "live".into(),
            keep: true,
            pid: Some(1),
            ..Terminal::default()
        };
        let finished = Terminal {
            state: "finished".into(),
            exit_code: Some(7),
            ..live.clone()
        };

        // The model still has the live record when the CLOSE arrives: its lack
        // of a status must not turn exit 7 into "exited (1)".
        let reason = json!({"reason": "exited", "exit_code": 7});
        assert_eq!(finished_exit_code(Some(&live), Some(&reason)), 7);
        // A finished record wins over a stale reason.
        let stale = json!({"reason": "exited", "exit_code": 0});
        assert_eq!(finished_exit_code(Some(&finished), Some(&stale)), 7);
        // Without either, the old record is all there is.
        assert_eq!(finished_exit_code(Some(&live), None), 1);
        assert_eq!(finished_exit_code(None, Some(&reason)), 7);
        assert_eq!(finished_exit_code(None, None), 0);
    }
}
