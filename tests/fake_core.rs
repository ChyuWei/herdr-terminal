//! Deterministic lifecycle regressions with a scripted stand-in for `hc`.
//!
//! This file is both the test and the fake core: cargo runs it with no
//! arguments (the tests below), while `ht` spawns the same executable as
//! `--hc PATH`, which answers `--version` and `connect`. The scripted core
//! forces the lifecycle events a real core only shows intermittently:
//!
//! * `terminal.attach` reports `finished` because the program exited first;
//! * a local stdout write fails after attach while the program keeps running;
//! * DATA and CLOSE arrive in one burst;
//! * a kept terminal is lost and restarted, the subscriber falls behind, a
//!   terminal is removed, and the connection drops;
//! * Ctrl-] detaches without killing or removing the program.
//!
//! The fake core logs every call, frame and scripted reply to `FAKE_LOG`, so
//! the tests can see what the client sent: whether it detached, removed the
//! record, killed the program, or started a duplicate terminal.

use serde_json::json;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod common;

use common::{Capture, HT, Probe, secs};

const DATA: u8 = 0;
const CLOSE: u8 = 2;
const EVENTS_CHANNEL: u32 = 3;
const ATTACH_CHANNEL: u32 = 5;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => println!("hc 9.9.9-fake"),
        Some("connect") => {
            if let Err(error) = fake_connect() {
                eprintln!("fake core: {error}");
                std::process::exit(1);
            }
        }
        _ => run_tests(),
    }
}

fn run_tests() {
    a_program_that_exits_before_attach_is_still_shown_and_removed();
    #[cfg(unix)]
    a_local_write_error_stops_and_removes_the_started_terminal();
    #[cfg(unix)]
    screen_draws_the_final_frame_when_data_and_close_arrive_together();
    #[cfg(unix)]
    screen_detaches_without_killing_the_program();
    #[cfg(unix)]
    screen_waits_for_a_restarted_terminal();
    #[cfg(unix)]
    screen_resnapshots_after_missed_events();
    #[cfg(unix)]
    screen_repaints_without_stale_cells();
    #[cfg(unix)]
    screen_exits_when_the_terminal_is_removed();
    #[cfg(unix)]
    screen_reconnects_to_the_same_terminal();
    println!("fake-core tests passed");
}

// --- scripted screen tests -------------------------------------------------

/// One `ht screen` under a PTY, against the scripted core.
#[cfg(unix)]
struct FakeScreen {
    pty: common::pty::Pty,
    probe: Probe,
    output: Capture,
    stderr: Capture,
    input: std::fs::File,
}

#[cfg(unix)]
impl FakeScreen {
    fn spawn(scratch: &Scratch, scenario: &str, args: &[&str]) -> FakeScreen {
        let exe = std::env::current_exe().expect("test executable");
        let pty = common::pty::Pty::open(80, 24);
        let (stdin, stdout) = pty.stdio();
        let mut child = Command::new(HT)
            .arg("screen")
            .arg("--hc")
            .arg(&exe)
            .args(["--cols", "80", "--rows", "24"])
            .args(args)
            .env("FAKE_SCENARIO", scenario)
            .env("FAKE_LOG", scratch.log())
            .env("HOME", &scratch.dir)
            .env("XDG_STATE_HOME", scratch.state())
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn ht screen");
        let output = Capture::spawn(pty.reader());
        let stderr = Capture::spawn(child.stderr.take().expect("stderr"));
        let input = pty.writer();
        let probe = Probe::new(child);
        FakeScreen {
            pty,
            probe,
            output,
            stderr,
            input,
        }
    }

    fn wait_text(&self, needle: &str, timeout: std::time::Duration) -> bool {
        common::wait_until(timeout, || self.rendered().contains(needle))
    }

    fn rendered(&self) -> String {
        let mut vt = herdr_terminal::vt::Vt::new(80, 24).expect("local VT");
        vt.feed(&self.output.bytes());
        vt.snapshot().expect("snapshot").lines().join("\n")
    }

    fn send(&mut self, bytes: &[u8]) {
        self.input.write_all(bytes).expect("write to the screen");
        self.input.flush().expect("flush the screen");
    }

    fn wait_exit(&mut self, timeout: std::time::Duration) -> Option<std::process::ExitStatus> {
        self.probe.wait_timeout(timeout)
    }

    fn tail(&self) -> String {
        let raw = self.output.bytes();
        String::from_utf8_lossy(&raw[raw.len().saturating_sub(400)..]).into_owned()
    }
}

/// DATA and CLOSE in one attach burst: the screen must draw the final frame
/// (past its frame throttle) before it leaves the alternate screen.
#[cfg(unix)]
fn screen_draws_the_final_frame_when_data_and_close_arrive_together() {
    let scratch = Scratch::new("screen-final");
    let mut screen = FakeScreen::spawn(&scratch, "screen-final", &["--", "sh", "-c", "sleep 30"]);

    let status = screen
        .wait_exit(secs(10))
        .expect("ht screen exits after the scripted CLOSE");
    assert_eq!(
        status.code(),
        Some(0),
        "ht screen exit {status}; stderr={:?}",
        screen.stderr.text()
    );
    let raw = screen.output.bytes();
    assert!(
        screen.output.contains(b"READY"),
        "the final frame is missing READY; raw={:?}",
        String::from_utf8_lossy(&raw)
    );
    assert!(
        screen.output.contains(b"END"),
        "the final frame is missing END; raw={:?}",
        String::from_utf8_lossy(&raw)
    );
    let end = rfind(&raw, b"END").expect("END was drawn");
    let leave = rfind(&raw, b"\x1b[?1049l").expect("the alternate screen was left");
    assert!(
        end < leave,
        "the final frame must be drawn before the screen is restored; raw={:?}",
        String::from_utf8_lossy(&raw)
    );
    assert!(
        screen.pty.is_cooked(),
        "ht screen did not restore the terminal after the program ended"
    );
    let log = scratch.entries();
    assert!(
        log.iter().any(|entry| entry["call"] == "terminal.rm"),
        "the finished record must be removed; log={log:?}"
    );
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.kill"),
        "a program that ended by itself must not be killed; log={log:?}"
    );
}

/// Ctrl-] detaches: the attach stream is closed, the program keeps running,
/// and the record is neither killed nor removed.
#[cfg(unix)]
fn screen_detaches_without_killing_the_program() {
    let scratch = Scratch::new("screen-detach");
    let mut screen = FakeScreen::spawn(&scratch, "screen-detach", &["--", "sh", "-c", "sleep 30"]);
    assert!(
        screen.wait_text("READY", secs(10)),
        "the first frame was not drawn; raw={:?} stderr={:?}",
        screen.output.text(),
        screen.stderr.text()
    );

    screen.send(&[0x1d]);
    let status = screen.wait_exit(secs(10)).expect("ht exits after Ctrl-]");
    assert_eq!(
        status.code(),
        Some(0),
        "a detach is not an error; stderr={:?}",
        screen.stderr.text()
    );
    assert!(
        screen.pty.is_cooked(),
        "the terminal was not restored after a detach"
    );

    let log = scratch.entries();
    // The client may exit before the fake reads the CLOSE frame it wrote: the
    // connection closing is what core sees either way. What must not happen is
    // a kill or a removal.
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.kill"),
        "a detach must leave the program running; log={log:?}"
    );
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.rm"),
        "a detach must leave the record in place; log={log:?}"
    );
    let run = log
        .iter()
        .find(|entry| entry["call"] == "terminal.run")
        .expect("the screen started a terminal");
    assert_eq!(
        run["params"]["keep"],
        json!(true),
        "the screen's terminal must be kept"
    );
    assert_eq!(run["params"]["rm"], json!(false));
    assert_eq!(
        run["params"]["labels"]["herdr.client"],
        json!("herdr-terminal"),
        "the terminal must carry the client label"
    );
}

/// A kept terminal is lost, stays unattachable until core restarts it, and
/// the client reattaches to the same address instead of starting a duplicate.
#[cfg(unix)]
fn screen_waits_for_a_restarted_terminal() {
    let scratch = Scratch::new("screen-lost-restarted");
    let mut screen = FakeScreen::spawn(&scratch, "screen-lost-restarted", &[]);
    assert!(
        screen.wait_text("BEFORE", secs(10)),
        "the adopted terminal was not drawn; raw={:?} stderr={:?}",
        screen.output.text(),
        screen.stderr.text()
    );

    // A key on the stream triggers the scripted loss: core reports `lost`,
    // closes the stream, and only restarts the terminal after an attach
    // finds it parked.
    screen.send(b"g");
    assert!(
        screen.wait_text("RESTARTED", secs(10)),
        "the client did not reattach after `restarted`; raw tail={:?} stderr={:?}",
        screen.tail(),
        screen.stderr.text()
    );

    let log = scratch.entries();
    let attached: Vec<&serde_json::Value> = log
        .iter()
        .filter(|entry| entry.get("attached").is_some())
        .collect();
    assert_eq!(
        attached.len(),
        2,
        "one attach before the loss and one after the restart; log={log:?}"
    );
    assert_eq!(attached[0]["attached"], json!(5));
    assert_eq!(attached[1]["attached"], json!(6));
    for entry in &attached {
        assert_eq!(
            entry["terminal"],
            json!("t1"),
            "the client must reattach the same address; log={log:?}"
        );
    }
    assert!(
        log.iter()
            .any(|entry| entry.get("attach_unavailable").is_some()),
        "a parked terminal must not be attachable before `restarted`; log={log:?}"
    );
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.run"),
        "an adopted terminal must never be started again; log={log:?}"
    );

    screen.send(&[0x1d]);
    let status = screen.wait_exit(secs(10)).expect("ht exits after Ctrl-]");
    assert_eq!(status.code(), Some(0), "stderr={:?}", screen.stderr.text());
    assert!(screen.pty.is_cooked());
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.kill"));
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.rm"));
}

/// `missed` carries a fresh snapshot: the client replaces its state and
/// reattaches the live terminal rather than trusting the old stream.
#[cfg(unix)]
fn screen_resnapshots_after_missed_events() {
    let scratch = Scratch::new("screen-missed");
    let mut screen = FakeScreen::spawn(&scratch, "screen-missed", &[]);
    assert!(
        screen.wait_text("BEFORE", secs(10)),
        "the adopted terminal was not drawn; raw={:?} stderr={:?}",
        screen.output.text(),
        screen.stderr.text()
    );

    screen.send(b"g");
    assert!(
        screen.wait_text("RESYNCED", secs(10)),
        "the client did not reattach after `missed`; raw tail={:?} stderr={:?}",
        screen.tail(),
        screen.stderr.text()
    );

    let log = scratch.entries();
    let attached: Vec<&serde_json::Value> = log
        .iter()
        .filter(|entry| entry.get("attached").is_some())
        .collect();
    assert_eq!(
        attached.len(),
        2,
        "the missed snapshot must lead to a fresh attach; log={log:?}"
    );
    assert_eq!(attached[1]["attached"], json!(6));
    assert_eq!(attached[1]["terminal"], json!("t1"));
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.run"),
        "a resnapshot must not start a duplicate terminal; log={log:?}"
    );
    assert!(
        log.iter()
            .any(|entry| { entry["frame"]["channel"] == 5 && entry["frame"]["kind"] == CLOSE }),
        "the replaced stream must be closed before the fresh attach; log={log:?}"
    );

    screen.send(&[0x1d]);
    let status = screen.wait_exit(secs(10)).expect("ht exits after Ctrl-]");
    assert_eq!(status.code(), Some(0), "stderr={:?}", screen.stderr.text());
    assert!(screen.pty.is_cooked());
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.kill"));
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.rm"));
}

/// A fresh repaint after the viewer fell behind starts with a soft reset and
/// a clear; the old screen must be gone, not merely overdrawn.
#[cfg(unix)]
fn screen_repaints_without_stale_cells() {
    let scratch = Scratch::new("screen-repaint");
    let mut screen = FakeScreen::spawn(&scratch, "screen-repaint", &[]);
    assert!(
        screen.wait_text("BEFORE", secs(10)),
        "the adopted terminal was not drawn; raw={:?} stderr={:?}",
        screen.output.text(),
        screen.stderr.text()
    );

    screen.send(b"g");
    assert!(
        screen.wait_text("REPAINTED", secs(10)),
        "the repaint was not drawn; raw tail={:?} stderr={:?}",
        screen.tail(),
        screen.stderr.text()
    );
    assert!(
        !screen.rendered().contains("BEFORE"),
        "a fresh repaint must clear the stale screen; grid={:?}",
        screen.rendered()
    );

    screen.send(&[0x1d]);
    let status = screen.wait_exit(secs(10)).expect("ht exits after Ctrl-]");
    assert_eq!(status.code(), Some(0), "stderr={:?}", screen.stderr.text());
    assert!(screen.pty.is_cooked());
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.kill"));
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.rm"));
}

/// A `removed` event ends the screen with a clear failure; there is nothing
/// left to kill or remove.
#[cfg(unix)]
fn screen_exits_when_the_terminal_is_removed() {
    let scratch = Scratch::new("screen-removed");
    let mut screen = FakeScreen::spawn(&scratch, "screen-removed", &[]);
    assert!(
        screen.wait_text("BEFORE", secs(10)),
        "the adopted terminal was not drawn; raw={:?} stderr={:?}",
        screen.output.text(),
        screen.stderr.text()
    );

    screen.send(b"g");
    let status = screen
        .wait_exit(secs(10))
        .expect("ht exits when its terminal is removed");
    assert_eq!(
        status.code(),
        Some(1),
        "a removed terminal is a failure; stderr={:?}",
        screen.stderr.text()
    );
    assert!(
        screen.stderr.wait_for(b"was removed", secs(2)),
        "the failure must name what happened; stderr={:?}",
        screen.stderr.text()
    );
    assert!(
        screen.pty.is_cooked(),
        "the terminal was not restored after removal"
    );
    let log = scratch.entries();
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.kill"),
        "a removed terminal must not be killed; log={log:?}"
    );
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.rm"),
        "a removed terminal must not be removed again; log={log:?}"
    );
}

/// When the connection dies, a new `hc connect` is made, a new snapshot is
/// taken, and the same terminal is reattached: the program never restarts.
#[cfg(unix)]
fn screen_reconnects_to_the_same_terminal() {
    let scratch = Scratch::new("screen-reconnect");
    let mut screen = FakeScreen::spawn(&scratch, "screen-reconnect", &[]);
    assert!(
        screen.wait_text("BEFORE", secs(10)),
        "the adopted terminal was not drawn; raw={:?} stderr={:?}",
        screen.output.text(),
        screen.stderr.text()
    );

    // The key makes generation 1 drop the connection; generation 2 answers
    // with a snapshot that still has t1 live.
    screen.send(b"g");
    assert!(
        screen.wait_text("RECONNECTED", secs(15)),
        "the client did not reconnect; raw tail={:?} stderr={:?}",
        screen.tail(),
        screen.stderr.text()
    );

    let log = scratch.entries();
    let generations: Vec<u64> = log
        .iter()
        .filter_map(|entry| entry["generation"].as_u64())
        .collect();
    assert!(
        generations.contains(&1) && generations.contains(&2),
        "both connections must have run; log={log:?}"
    );
    let attached: Vec<&serde_json::Value> = log
        .iter()
        .filter(|entry| entry.get("attached").is_some())
        .collect();
    assert_eq!(attached.len(), 2, "one attach per connection; log={log:?}");
    for entry in &attached {
        assert_eq!(
            entry["terminal"],
            json!("t1"),
            "the reconnect must adopt the same address; log={log:?}"
        );
    }
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.run"),
        "a reconnect must never start a duplicate terminal; log={log:?}"
    );

    screen.send(&[0x1d]);
    let status = screen.wait_exit(secs(10)).expect("ht exits after Ctrl-]");
    assert_eq!(status.code(), Some(0), "stderr={:?}", screen.stderr.text());
    assert!(screen.pty.is_cooked());
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.kill"));
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.rm"));
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

/// A program that exited before attach: the probe must read the final screen,
/// report the exit status, and remove the record it kept with `rm: false`.
fn a_program_that_exits_before_attach_is_still_shown_and_removed() {
    let scratch = Scratch::new("finished");
    let exe = std::env::current_exe().expect("test executable");
    let mut child = Command::new(HT)
        .arg("probe")
        .arg("--hc")
        .arg(&exe)
        .arg("--")
        .env("FAKE_SCENARIO", "finished")
        .env("FAKE_LOG", scratch.log())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ht probe");
    let stdout = Capture::spawn(child.stdout.take().expect("stdout"));
    let stderr = Capture::spawn(child.stderr.take().expect("stderr"));
    let mut probe = Probe::new(child);

    let status = probe
        .wait_timeout(secs(10))
        .expect("the probe exits after recovering the program");
    assert_eq!(
        status.code(),
        Some(7),
        "the recovered program's exit status must be reported; stderr={:?}",
        stderr.text()
    );
    assert!(
        stdout.wait_for(b"FAST", secs(2)),
        "the recovered screen must reach stdout; output={:?}",
        stdout.text()
    );

    let log = scratch.entries();
    assert!(
        log.iter().any(|entry| entry["call"] == "terminal.rm"),
        "the finished record must be removed; log={log:?}"
    );
    let run = log
        .iter()
        .find(|entry| entry["call"] == "terminal.run")
        .expect("the probe ran a terminal");
    assert_eq!(
        run["params"]["rm"],
        json!(false),
        "the run must keep the record until the probe reads it"
    );
}

/// A local stdout failure while the program is alive: the probe must detach,
/// end the program it started, and remove the record.
#[cfg(unix)]
fn a_local_write_error_stops_and_removes_the_started_terminal() {
    use std::fs::File;
    use std::os::fd::FromRawFd;

    let scratch = Scratch::new("write-error");
    let exe = std::env::current_exe().expect("test executable");
    // A stdout pipe with no reader: every write fails with EPIPE.
    let mut fds = [0i32; 2];
    // SAFETY: fds is a two-element array that pipe fills on success.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
    // SAFETY: both descriptors came from the pipe above.
    unsafe { libc::close(fds[0]) };
    // SAFETY: fds[1] is the pipe's write end and is owned from here on.
    let stdout = unsafe { Stdio::from(File::from_raw_fd(fds[1])) };

    let mut child = Command::new(HT)
        .arg("probe")
        .arg("--hc")
        .arg(&exe)
        .arg("--")
        .env("FAKE_SCENARIO", "live")
        .env("FAKE_LOG", scratch.log())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ht probe");
    let stderr = Capture::spawn(child.stderr.take().expect("stderr"));
    let mut probe = Probe::new(child);

    let status = probe.wait_timeout(secs(10)).expect("the probe exits");
    assert_eq!(
        status.code(),
        Some(1),
        "a broken stdout must fail the probe; stderr={:?}",
        stderr.text()
    );
    assert!(
        stderr.wait_for(b"cannot write terminal output", secs(2)),
        "stderr={:?}",
        stderr.text()
    );

    let log = scratch.entries();
    assert!(
        log.iter().any(|entry| entry["call"] == "terminal.kill"),
        "the started program must be ended; log={log:?}"
    );
    assert!(
        log.iter().any(|entry| entry["call"] == "terminal.rm"),
        "the finished record must be removed; log={log:?}"
    );
    assert!(
        log.iter().any(|entry| {
            entry["frame"]["channel"] == ATTACH_CHANNEL && entry["frame"]["kind"] == CLOSE
        }),
        "the probe must detach from its stream; log={log:?}"
    );
}

// --- fake core -------------------------------------------------------------

fn fake_connect() -> io::Result<()> {
    let scenario = std::env::var("FAKE_SCENARIO").unwrap_or_else(|_| "finished".into());
    let log_path = std::env::var_os("FAKE_LOG").map(PathBuf::from);
    let log_path = log_path.as_deref();
    let generation = bump_generation(log_path);
    log(log_path, json!({"generation": generation}));

    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();

    // Plain line mode: one hello, one line back, then framed for good.
    let mut line = String::new();
    input.read_line(&mut line)?;
    let hello: serde_json::Value =
        serde_json::from_str(line.trim()).map_err(|error| invalid(error.to_string()))?;
    assert_eq!(hello["method"], "hello");
    let result = json!({
        "generation": 1,
        "version": "9.9.9-fake",
        "core": {"id": "fake", "name": "fake"},
        "framed": true,
        "methods": [
            "events.subscribe", "terminal.run", "terminal.attach",
            "terminal.get", "terminal.read", "terminal.rm", "terminal.kill"
        ],
        "features": ["framed", "attach", "events"],
    });
    writeln!(
        output,
        "{}",
        json!({"jsonrpc": "2.0", "id": hello["id"], "result": result})
    )?;
    output.flush()?;

    let mut next_channel = ATTACH_CHANNEL;
    let mut attached_channel: Option<u32> = None;
    // A kept terminal whose program was lost and not yet replaced.
    let mut parked = false;
    let mut triggered = false;

    while let Some((channel, kind, payload)) = read_frame(&mut input)? {
        log(
            log_path,
            json!({"frame": {"channel": channel, "kind": kind, "len": payload.len()}}),
        );

        // A key on the attach stream drives the scripted lifecycle. Ctrl-]
        // never arrives here: the client handles it locally.
        if kind == DATA && attached_channel == Some(channel) {
            if !triggered {
                triggered = true;
                match scenario.as_str() {
                    "screen-lost-restarted" => {
                        parked = true;
                        notify(&mut output, lost_event())?;
                        write_frame(
                            &mut output,
                            channel,
                            CLOSE,
                            br#"{"reason":"signaled","signal":9}"#,
                        )?;
                    }
                    "screen-missed" => notify(&mut output, missed_event())?,
                    "screen-removed" => notify(&mut output, removed_event())?,
                    // Core's fresh paint after a viewer fell behind: a soft
                    // reset, mode resets and a clear before the screen.
                    "screen-repaint" => write_frame(
                        &mut output,
                        channel,
                        DATA,
                        b"\x1b[!p\x1b[?1004l\x1b[H\x1b[2JREPAINTED\n",
                    )?,
                    // Drop the connection: the client must make a new one.
                    "screen-reconnect" if generation == 1 => return Ok(()),
                    _ => {}
                }
            }
            continue;
        }
        if channel != 0 || kind != DATA {
            continue;
        }
        let Ok(message) = serde_json::from_slice::<serde_json::Value>(&payload) else {
            continue;
        };
        let id = message["id"].clone();
        let method = message["method"].as_str().unwrap_or_default();
        log(
            log_path,
            json!({"call": method, "params": message.get("params")}),
        );
        match method {
            "events.subscribe" => {
                let snapshot = if scripted_terminal(&scenario) {
                    json!({
                        "seq": 1,
                        "terminals": [live_record(4242)],
                        "channel": EVENTS_CHANNEL,
                    })
                } else {
                    json!({"seq": 1, "terminals": [], "channel": EVENTS_CHANNEL})
                };
                reply(&mut output, &id, snapshot)?;
            }
            "terminal.run" => {
                let record = if scenario == "finished" {
                    record("finished")
                } else {
                    live_record(4242)
                };
                reply(&mut output, &id, record)?;
            }
            "terminal.attach" => {
                if scenario == "finished" {
                    reply_error(
                        &mut output,
                        &id,
                        "finished",
                        "t1 has finished",
                        "read what it left: hc read t1",
                    )?;
                } else if parked {
                    // The kept terminal has no program until core restarts
                    // it; the `restarted` event follows immediately.
                    parked = false;
                    log(log_path, json!({"attach_unavailable": "t1"}));
                    reply_error(
                        &mut output,
                        &id,
                        "unavailable",
                        "t1 is waiting for a new shell",
                        "it will restart soon",
                    )?;
                    notify(
                        &mut output,
                        event_message(3, "restarted", live_record(4343)),
                    )?;
                } else {
                    let first = attached_channel.is_none();
                    let channel = next_channel;
                    next_channel += 1;
                    attached_channel = Some(channel);
                    log(log_path, json!({"attached": channel, "terminal": "t1"}));
                    reply(
                        &mut output,
                        &id,
                        json!({"channel": channel, "terminal": live_record(4242)}),
                    )?;
                    match scenario.as_str() {
                        // The whole ending arrives as one burst: two DATA
                        // frames and CLOSE, so the last text is only in the
                        // VT when the close is seen.
                        "screen-final" => {
                            write_frame(&mut output, channel, DATA, b"\x1b[38;2;1;2;3mREADY\n")?;
                            write_frame(&mut output, channel, DATA, b"END\n")?;
                            write_frame(
                                &mut output,
                                channel,
                                CLOSE,
                                br#"{"reason":"exited","exit_code":0}"#,
                            )?;
                        }
                        "screen-detach" => {
                            write_frame(&mut output, channel, DATA, b"READY\n")?;
                        }
                        "screen-lost-restarted" if first => {
                            write_frame(&mut output, channel, DATA, b"BEFORE\n")?;
                        }
                        "screen-lost-restarted" => {
                            write_frame(&mut output, channel, DATA, b"RESTARTED\n")?;
                        }
                        "screen-missed" if first => {
                            write_frame(&mut output, channel, DATA, b"BEFORE\n")?;
                        }
                        "screen-missed" => {
                            write_frame(&mut output, channel, DATA, b"RESYNCED\n")?;
                        }
                        "screen-removed" => {
                            write_frame(&mut output, channel, DATA, b"BEFORE\n")?;
                        }
                        "screen-repaint" => {
                            write_frame(&mut output, channel, DATA, b"BEFORE\n")?;
                        }
                        "screen-reconnect" if generation == 1 => {
                            write_frame(&mut output, channel, DATA, b"BEFORE\n")?;
                        }
                        "screen-reconnect" => {
                            write_frame(&mut output, channel, DATA, b"RECONNECTED\n")?;
                        }
                        _ => {
                            // The probe cannot write this anywhere: its stdout
                            // is broken.
                            write_frame(&mut output, channel, DATA, b"PAINT\n")?;
                        }
                    }
                }
            }
            "terminal.get" => reply(&mut output, &id, record("finished"))?,
            "terminal.read" => reply(
                &mut output,
                &id,
                json!({"text": "FAST", "complete": true, "how": "screen"}),
            )?,
            "terminal.rm" => reply(&mut output, &id, json!({"removed": "t1"}))?,
            "terminal.kill" => reply(&mut output, &id, record("finished"))?,
            _ => reply(&mut output, &id, json!({}))?,
        }
    }
    Ok(())
}

/// Scenarios whose snapshot already contains the terminal to adopt.
fn scripted_terminal(scenario: &str) -> bool {
    matches!(
        scenario,
        "screen-lost-restarted"
            | "screen-missed"
            | "screen-removed"
            | "screen-reconnect"
            | "screen-repaint"
    )
}

/// A live record, or a finished one with exit status 7.
fn record(state: &str) -> serde_json::Value {
    let finished = state == "finished";
    json!({
        "address": "t1",
        "name": null,
        "labels": {"herdr.client": "herdr-terminal"},
        "argv": [],
        "cwd": "/",
        "keep": true,
        "state": state,
        "cols": 80,
        "rows": 24,
        "pid": if finished { serde_json::Value::Null } else { json!(4242) },
        "title": "",
        "parent": null,
        "reason": if finished { Some("exited") } else { None },
        "exit_code": if finished { Some(7) } else { None },
        "signal": null,
    })
}

fn live_record(pid: u32) -> serde_json::Value {
    let mut record = record("live");
    record["pid"] = json!(pid);
    record
}

/// One `event` notification, on the events stream.
fn event_message(seq: u64, kind: &str, data: serde_json::Value) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "method": "event",
        "params": {"seq": seq, "type": kind, "terminal": "t1", "data": data},
    })
}

fn lost_event() -> serde_json::Value {
    event_message(2, "lost", json!({"reason": "signaled", "signal": 9}))
}

fn removed_event() -> serde_json::Value {
    event_message(2, "removed", json!({}))
}

/// A fresh snapshot, as core sends when the subscriber fell behind.
fn missed_event() -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "method": "missed",
        "params": {"seq": 9, "terminals": [live_record(5555)]},
    })
}

/// A notification from core: a newline-terminated JSON line on the events
/// channel, as core sends it.
fn notify(out: &mut impl Write, message: serde_json::Value) -> io::Result<()> {
    let mut line = message.to_string();
    line.push('\n');
    write_frame(out, EVENTS_CHANNEL, DATA, line.as_bytes())
}

fn bump_generation(log_path: Option<&Path>) -> u32 {
    let Some(log_path) = log_path else { return 1 };
    let path = log_path.with_extension("generation");
    let current: u32 = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0);
    let next = current + 1;
    let _ = std::fs::write(&path, next.to_string());
    next
}

fn reply(
    out: &mut impl Write,
    id: &serde_json::Value,
    result: serde_json::Value,
) -> io::Result<()> {
    write_json(out, json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn reply_error(
    out: &mut impl Write,
    id: &serde_json::Value,
    kind: &str,
    message: &str,
    hint: &str,
) -> io::Result<()> {
    write_json(
        out,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32000,
                "message": message,
                "data": {"kind": kind, "hint": hint},
            },
        }),
    )
}

fn write_json(out: &mut impl Write, value: serde_json::Value) -> io::Result<()> {
    write_frame(out, 0, DATA, value.to_string().as_bytes())
}

fn write_frame(out: &mut impl Write, channel: u32, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut wire = Vec::with_capacity(9 + payload.len());
    wire.extend_from_slice(&channel.to_le_bytes());
    wire.push(kind);
    wire.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    wire.extend_from_slice(payload);
    out.write_all(&wire)?;
    out.flush()
}

fn read_frame(input: &mut impl Read) -> io::Result<Option<(u32, u8, Vec<u8>)>> {
    let mut header = [0u8; 9];
    match input.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let channel = u32::from_le_bytes(header[0..4].try_into().expect("four bytes"));
    let kind = header[4];
    let len = u32::from_le_bytes(header[5..9].try_into().expect("four bytes")) as usize;
    let mut payload = vec![0u8; len];
    input.read_exact(&mut payload)?;
    Ok(Some((channel, kind, payload)))
}

fn log(path: Option<&Path>, record: serde_json::Value) {
    let Some(path) = path else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{record}");
    }
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

// --- scratch ---------------------------------------------------------------

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = PathBuf::from(format!("/var/tmp/ht-fake-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch { dir }
    }

    fn log(&self) -> PathBuf {
        self.dir.join("log")
    }

    fn state(&self) -> PathBuf {
        self.dir.join("state")
    }

    fn entries(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
