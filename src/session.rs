//! The lifecycle of a terminal this client started.
//!
//! One [`Session`] owns the terminal address from `terminal.run` until the
//! program ends or the viewer leaves. Its normal paths are explicit:
//! [`Session::remove_finished`] after the program exited by itself,
//! [`Session::end`] when the viewer quits (detach, kill, remove), and
//! [`Session::lost`] when core is unreachable and the program must be left
//! alone. Any other early return runs `Drop`, which ends and removes the
//! terminal so a local error cannot leave a program behind.
//!
//! [`recover_finished`] handles the race where a program exits before
//! `terminal.attach` lands: the run keeps its record (`rm: false`), so the
//! final screen and exit status can still be read and then removed.

use crate::core::{Attached, CoreClient, Error, Result, RpcError, Terminal};
use serde_json::Value;
use std::io::Write;
use std::sync::Arc;

/// A terminal started by this client, with cleanup on every path.
pub struct Session {
    client: Arc<CoreClient>,
    address: String,
    channel: Option<u32>,
    done: bool,
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
        })
    }

    pub fn client(&self) -> &Arc<CoreClient> {
        &self.client
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    /// The attach channel, once attached.
    pub fn channel(&self) -> Option<u32> {
        self.channel
    }

    /// Attaches at the given size and remembers the channel for cleanup.
    pub fn attach(&mut self, cols: u16, rows: u16) -> Result<Attached> {
        let attached = self.client.attach(&self.address, cols, rows)?;
        self.channel = Some(attached.channel);
        Ok(attached)
    }

    /// The program ended by itself: forget its finished record.
    pub fn remove_finished(&mut self) -> Result<()> {
        self.done = true;
        self.channel = None;
        missing_ok(self.client.rm(&self.address))
    }

    /// The viewer quit: detach, end the program, and remove the record. Each
    /// step is tried even if an earlier one fails, so a refused detach cannot
    /// leave the program running.
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
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.done {
            // A local error after `terminal.run` (a write, a thread spawn, an
            // attach failure): end the program this client started.
            let _ = self.end();
        }
    }
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
}
