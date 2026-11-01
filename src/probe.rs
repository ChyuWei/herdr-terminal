//! `ht probe`: the M0 way to prove the client without a TUI.
//!
//! It subscribes to events, runs a shell (or a supplied argv), attaches, and
//! copies raw program bytes to stdout while forwarding stdin as DATA. Ctrl-C
//! quits the probe (it is not forwarded), detaches and ends its terminal.
//!
//! A program can exit before the attach lands. The run therefore keeps its
//! record (`rm: false`); when attach reports `finished`, the probe reads the
//! final screen and exit status with `terminal.get`/`terminal.read`, prints
//! them, and removes the record. Every other exit after `terminal.run` goes
//! through [`Started`], which cleans up in one place; a lost connection is the
//! exception and leaves the program running.

use crate::core::{
    Attached, CoreClient, Error, EventStream, Notification, Result, RpcError, RunOptions,
    StreamItem, Terminal,
};
use crate::term::{self, RawMode};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Default attach size when there is no terminal and no explicit size.
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

/// How long the main loop waits for the next output chunk or quit.
const TICK: Duration = Duration::from_millis(100);
/// How often the loop checks whether its terminal was resized.
const RESIZE_POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub hc: Option<PathBuf>,
    /// Program and arguments; empty starts core's own shell.
    pub argv: Vec<String>,
    pub verbose: bool,
}

/// Runs the probe. Returns the exit code to report: the program's own when it
/// exits (including an exit before attach landed), or 0 when the viewer quits.
pub fn run(options: Options) -> Result<i32> {
    let terminal_size = term::window_size();
    let fixed_size = options.cols.is_some() || options.rows.is_some();
    let cols = options
        .cols
        .or(terminal_size.map(|(cols, _)| cols))
        .unwrap_or(DEFAULT_COLS);
    let rows = options
        .rows
        .or(terminal_size.map(|(_, rows)| rows))
        .unwrap_or(DEFAULT_ROWS);
    let watch_resize = !fixed_size && terminal_size.is_some();

    // Raw mode captures Ctrl-C as a byte instead of a signal, and is restored
    // when this function returns.
    let _raw = RawMode::enter()?;

    let client = Arc::new(CoreClient::connect(options.hc.as_deref())?);
    if options.verbose {
        let hello = client.hello();
        eprintln!(
            "core {} {}: generation {}, features {:?}",
            hello.core.name, hello.version, hello.generation, hello.features
        );
    }
    let mut events = client.subscribe()?;

    let run = RunOptions {
        argv: options.argv.clone(),
        // Keep the finished record until this probe has read it: attach can
        // lose the race with a program that exits at once.
        rm: false,
        ..RunOptions::default()
    };
    let address = client.run(&run)?.address;
    // From here on this probe owns a terminal, and every path cleans it up.
    let mut started = Started::new(client, address);
    let mut stdout = std::io::stdout();

    let attached = match started.attach(cols, rows) {
        Ok(attached) => attached,
        Err(Error::Rpc { error, .. }) if error.kind() == Some("finished") => {
            return recover_finished(&mut started, &mut stdout, &error);
        }
        Err(error) => return Err(error),
    };
    let channel = attached.channel;
    if options.verbose {
        eprintln!(
            "attached to {} on channel {channel} at {cols}x{rows}",
            started.address
        );
    }

    let (quit_tx, quit_rx) = mpsc::channel();
    spawn_input(Arc::downgrade(&started.client), channel, quit_tx)?;

    let result = play(
        &mut started,
        attached.stream,
        &mut events,
        &mut stdout,
        quit_rx,
        Watch {
            size: (cols, rows),
            resize: watch_resize,
            verbose: options.verbose,
        },
    );
    match result {
        Ok(Exit::Program(reason)) => {
            let code = exit_code_from(reason.as_ref());
            // The program already ended; only its record is left to forget.
            if let Err(error) = started.remove_finished() {
                eprintln!(
                    "ht: warning: could not remove terminal {}: {error}",
                    started.address
                );
            }
            Ok(code)
        }
        Ok(Exit::Quit) => {
            started.end()?;
            Ok(0)
        }
        Err(error) => {
            if is_lost(&error) {
                // Core is gone, not the program: leave it running.
                started.lost();
            }
            Err(error)
        }
    }
}

/// How the viewer's loop ended.
enum Exit {
    /// Core closed the stream; the JSON reason carries the program's status.
    Program(Option<Value>),
    /// The person pressed Ctrl-C.
    Quit,
}

/// What the output loop watches besides the stream itself.
struct Watch {
    size: (u16, u16),
    resize: bool,
    verbose: bool,
}

/// Copies program output and watches for a quit, a resize, or the end.
fn play(
    started: &mut Started,
    stream: Receiver<StreamItem>,
    events: &mut EventStream,
    stdout: &mut impl Write,
    quit: Receiver<()>,
    watch: Watch,
) -> Result<Exit> {
    let channel = started.channel.expect("the probe attached before play");
    let mut last_size = watch.size;
    let mut next_resize = Instant::now() + RESIZE_POLL;
    loop {
        match stream.recv_timeout(TICK) {
            Ok(StreamItem::Data(bytes)) => {
                write_output(stdout, &bytes)?;
                started.client.credit(channel, bytes.len())?;
            }
            Ok(StreamItem::Closed(reason)) => return Ok(Exit::Program(reason)),
            Ok(StreamItem::Lost) | Err(RecvTimeoutError::Disconnected) => {
                started.lost();
                return Err(lost_error());
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        drain_events(events, watch.verbose);
        if quit.try_recv().is_ok() {
            return Ok(Exit::Quit);
        }
        // Poll the local size at a modest rate even when output floods in.
        if watch.resize && Instant::now() >= next_resize {
            next_resize = Instant::now() + RESIZE_POLL;
            if let Some(size) = term::window_size()
                && size != last_size
            {
                started.client.resize(channel, size.0, size.1)?;
                if watch.verbose {
                    eprintln!("resize {}x{}", size.0, size.1);
                }
                last_size = size;
            }
        }
    }
}

/// The terminal this probe started. The normal paths call [`Started::end`],
/// [`Started::remove_finished`], or [`Started::lost`]; if a local error returns
/// early, `Drop` ends and removes the terminal so nothing is left running.
struct Started {
    client: Arc<CoreClient>,
    address: String,
    channel: Option<u32>,
    done: bool,
}

impl Started {
    fn new(client: Arc<CoreClient>, address: String) -> Started {
        Started {
            client,
            address,
            channel: None,
            done: false,
        }
    }

    fn attach(&mut self, cols: u16, rows: u16) -> Result<Attached> {
        let attached = self.client.attach(&self.address, cols, rows)?;
        self.channel = Some(attached.channel);
        Ok(attached)
    }

    /// The program ended by itself: forget its finished record.
    fn remove_finished(&mut self) -> Result<()> {
        self.done = true;
        self.channel = None;
        missing_ok(self.client.rm(&self.address))
    }

    /// The viewer quit: detach, end the program, and remove the record. Each
    /// step is tried even if an earlier one fails, so a refused detach cannot
    /// leave the program running.
    fn end(&mut self) -> Result<()> {
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
    fn keep(&mut self) {
        self.done = true;
    }

    /// Core is unreachable. The program may still be running.
    fn lost(&mut self) {
        self.done = true;
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        if !self.done {
            // A local error after `terminal.run` (stdout write, credit, a
            // thread spawn): end the program this probe started.
            let _ = self.end();
        }
    }
}

/// A program that exits before `terminal.attach` lands has no stream, but core
/// keeps its final screen (`rm: false`). Show that screen and its status, then
/// remove the finished record. If the screen cannot be read, keep the record
/// and fail clearly rather than silently losing the output.
fn recover_finished(
    started: &mut Started,
    stdout: &mut impl Write,
    attach_error: &RpcError,
) -> Result<i32> {
    let record = match started.client.get(&started.address) {
        Ok(record) => record,
        Err(error) if is_lost(&error) => {
            started.lost();
            return Err(error);
        }
        Err(error) => {
            started.keep();
            return Err(Error::Protocol(format!(
                "the program exited before attach and its terminal could not be read \
                 (attach: {attach_error}; terminal.get: {error}); exit status unknown, \
                 output not captured"
            )));
        }
    };
    let code = record_exit_code(&record);
    let read = match started.client.read(&started.address, 0) {
        Ok(read) => read,
        Err(error) if is_lost(&error) => {
            started.lost();
            return Err(error);
        }
        Err(error) => {
            started.keep();
            return Err(Error::Protocol(format!(
                "the program exited before attach (exit status {code}); its output could not \
                 be recovered (attach: {attach_error}; terminal.read: {error}); terminal {} was \
                 kept: hc read {}",
                started.address, started.address
            )));
        }
    };
    if !read.text.is_empty() {
        let written =
            write_output(stdout, read.text.as_bytes()).and_then(|()| write_output(stdout, b"\n"));
        if let Err(error) = written {
            // The screen is the only copy left; keep it for `hc read`.
            started.keep();
            return Err(error);
        }
    }
    if let Err(error) = started.remove_finished() {
        eprintln!(
            "ht: warning: could not remove finished terminal {}: {error}",
            started.address
        );
    }
    Ok(code)
}

fn lost_error() -> Error {
    Error::Protocol("lost the connection to core; the program may still be running".into())
}

/// Whether an error means the connection is gone (rather than core refusing).
fn is_lost(error: &Error) -> bool {
    match error {
        Error::Closed => true,
        Error::Rpc { error, .. } => error.kind() == Some("lost"),
        _ => false,
    }
}

/// `terminal.rm` on a record that is already gone is the outcome we wanted.
fn missing_ok(result: Result<()>) -> Result<()> {
    match result {
        Err(Error::Rpc { error, .. }) if error.kind() == Some("not_found") => Ok(()),
        other => other,
    }
}

fn write_output(stdout: &mut impl Write, bytes: &[u8]) -> Result<()> {
    stdout
        .write_all(bytes)
        .and_then(|()| stdout.flush())
        .map_err(|error| Error::Protocol(format!("cannot write terminal output: {error}")))
}

/// Forwards stdin chunks as DATA. Ctrl-C (0x03) quits instead of being sent;
/// bytes before it in the same read are still forwarded.
fn spawn_input(client: std::sync::Weak<CoreClient>, channel: u32, quit: Sender<()>) -> Result<()> {
    std::thread::Builder::new()
        .name("ht-probe-input".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut stdin = stdin.lock();
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        // A weak handle: the input thread must not keep the
                        // connection alive after the probe returns.
                        let Some(client) = client.upgrade() else {
                            break;
                        };
                        let (forward, stop) = split_quit(&buf[..n]);
                        if !forward.is_empty() && client.keys(channel, &forward).is_err() {
                            break;
                        }
                        if stop {
                            let _ = quit.send(());
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })
        .map_err(|error| Error::Protocol(format!("cannot start the input thread: {error}")))?;
    Ok(())
}

fn drain_events(events: &mut EventStream, verbose: bool) {
    while let Ok(notification) = events.try_recv() {
        if verbose {
            log_notification(&notification);
        }
    }
}

fn log_notification(notification: &Notification) {
    match notification {
        Notification::Event {
            seq,
            kind,
            terminal,
            machine,
            ..
        } => {
            let at = terminal.as_deref().or(machine.as_deref()).unwrap_or("-");
            eprintln!("event seq={seq} type={} terminal={at}", kind.name());
        }
        Notification::Heartbeat { seq } => eprintln!("heartbeat seq={seq}"),
        Notification::Missed { seq, terminals } => {
            eprintln!(
                "missed: resynced at seq={seq} with {} terminals",
                terminals.len()
            );
        }
        Notification::Other { method, .. } => eprintln!("notice {method}"),
    }
}

/// The bytes to forward and whether Ctrl-C appeared in this chunk.
fn split_quit(bytes: &[u8]) -> (Vec<u8>, bool) {
    match bytes.iter().position(|byte| *byte == 0x03) {
        Some(at) => (bytes[..at].to_vec(), true),
        None => (bytes.to_vec(), false),
    }
}

/// The exit status to report for a core CLOSE reason.
fn exit_code_from(reason: Option<&Value>) -> i32 {
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
fn record_exit_code(record: &Terminal) -> i32 {
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
    fn ctrl_c_quits_and_bytes_before_it_still_go() {
        assert_eq!(split_quit(b"hello"), (b"hello".to_vec(), false));
        assert_eq!(
            split_quit(b"hi\x03rest"),
            (b"hi".to_vec(), true),
            "the quit byte and after are dropped"
        );
        assert_eq!(split_quit(b"\x03"), (Vec::new(), true));
        assert_eq!(split_quit(b""), (Vec::new(), false));
    }

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
