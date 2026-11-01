//! Deterministic probe regressions with a scripted stand-in for `hc`.
//!
//! This file is both the test and the fake core: cargo runs it with no
//! arguments (the tests below), while the probe spawns the same executable as
//! `--hc PATH`, which answers `--version` and `connect`. That lets the tests
//! force the two races a real core only shows intermittently:
//!
//! * `terminal.attach` reports `finished` because the program exited first;
//! * a local stdout write fails after attach while the program keeps running.
//!
//! The fake core logs every call and frame to `FAKE_LOG`, so the tests can see
//! whether the probe read the screen, removed the record, or killed the program.

use serde_json::json;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod common;

use common::{Capture, HT, Probe, secs};

const DATA: u8 = 0;
const CLOSE: u8 = 2;
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
    println!("fake-core probe tests passed");
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

    while let Some((channel, kind, payload)) = read_frame(&mut input)? {
        log(
            log_path,
            json!({"frame": {"channel": channel, "kind": kind, "len": payload.len()}}),
        );
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
                reply(
                    &mut output,
                    &id,
                    json!({"seq": 1, "terminals": [], "channel": 3}),
                )?;
            }
            "terminal.run" => {
                reply(
                    &mut output,
                    &id,
                    record(if scenario == "finished" {
                        "finished"
                    } else {
                        "live"
                    }),
                )?;
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
                } else {
                    reply(
                        &mut output,
                        &id,
                        json!({
                            "channel": ATTACH_CHANNEL,
                            "terminal": record("live"),
                        }),
                    )?;
                    // The probe cannot write this anywhere: its stdout is broken.
                    write_frame(&mut output, ATTACH_CHANNEL, DATA, b"PAINT\n")?;
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

/// A finished record with exit status 7, or a live one.
fn record(state: &str) -> serde_json::Value {
    let finished = state == "finished";
    json!({
        "address": "t1",
        "name": null,
        "labels": {"herdr.client": "herdr-terminal"},
        "argv": [],
        "cwd": "/",
        "state": state,
        "cols": 80,
        "rows": 24,
        "pid": 4242,
        "title": "",
        "parent": null,
        "reason": if finished { Some("exited") } else { None },
        "exit_code": if finished { Some(7) } else { None },
        "signal": null,
    })
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
