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
//! through [`Session`], which cleans up in one place; a lost connection is the
//! exception and leaves the program running.

use crate::core::{CoreClient, Error, EventStream, Notification, Result, RunOptions, StreamItem};
use crate::session::{self, Session};
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
    // From here on this probe owns a terminal, and every path cleans it up.
    let mut started = Session::start(client, &run)?;
    let mut stdout = std::io::stdout();

    let attached = match started.attach(cols, rows) {
        Ok(attached) => attached,
        Err(Error::Rpc { error, .. }) if error.kind() == Some("finished") => {
            return session::recover_finished(&mut started, &mut stdout, &error);
        }
        Err(error) => return Err(error),
    };
    let channel = attached.channel;
    if options.verbose {
        eprintln!(
            "attached to {} on channel {channel} at {cols}x{rows}",
            started.address()
        );
    }

    let (quit_tx, quit_rx) = mpsc::channel();
    spawn_input(Arc::downgrade(started.client()), channel, quit_tx)?;

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
            let code = session::exit_code_from(reason.as_ref());
            // The program already ended; only its record is left to forget.
            if let Err(error) = started.remove_finished() {
                eprintln!(
                    "ht: warning: could not remove terminal {}: {error}",
                    started.address()
                );
            }
            Ok(code)
        }
        Ok(Exit::Quit) => {
            started.end()?;
            Ok(0)
        }
        Err(error) => {
            if session::is_lost(&error) {
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
    started: &mut Session,
    stream: Receiver<StreamItem>,
    events: &mut EventStream,
    stdout: &mut impl Write,
    quit: Receiver<()>,
    watch: Watch,
) -> Result<Exit> {
    let channel = started.channel().expect("the probe attached before play");
    let mut last_size = watch.size;
    let mut next_resize = Instant::now() + RESIZE_POLL;
    loop {
        match stream.recv_timeout(TICK) {
            Ok(StreamItem::Data(bytes)) => {
                write_output(stdout, &bytes)?;
                started.client().credit(channel, bytes.len())?;
            }
            Ok(StreamItem::Closed(reason)) => return Ok(Exit::Program(reason)),
            Ok(StreamItem::Lost) | Err(RecvTimeoutError::Disconnected) => {
                started.lost();
                return Err(session::lost_error());
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
                started.client().resize(channel, size.0, size.1)?;
                if watch.verbose {
                    eprintln!("resize {}x{}", size.0, size.1);
                }
                last_size = size;
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
