#![cfg(unix)]
//! End-to-end M1 test: a real core in an isolated session, the real `ht
//! screen` under a PTY, and the rendered output read back through a local VT.
//! It skips when herdr-core is not installed.

mod common;

use common::pty::Pty;
use common::{Capture, HT, Probe, Session, secs, wait_until};
use herdr_terminal::vt::Vt;
use std::io::Write;
use std::process::Stdio;

/// Prints a marker and a wide/grapheme line, waits for a line of input,
/// echoes it, then reports the terminal size forever. Resizing must reach the
/// program.
const SHELL: &str = "printf 'READY\\n'; printf '你好 👋\\n'; read x; \
                     printf 'GOT:%s\\n' \"$x\"; while :; do stty size; sleep 0.1; done";

fn session() -> Option<Session> {
    let session = Session::new();
    if session.is_none() {
        if std::env::var_os("HERDR_TERMINAL_REQUIRE_CORE").is_some() {
            panic!("herdr-core (hc) not found, and HERDR_TERMINAL_REQUIRE_CORE is set");
        }
        eprintln!(
            "skipping: herdr-core (hc) not found (set HERDR_TERMINAL_REQUIRE_CORE=1 to fail)"
        );
    }
    session
}

/// Replays everything the outer terminal received and returns the visible
/// grid, so assertions are about what `ht screen` drew, not raw escape bytes.
fn rendered(output: &Capture, cols: u16, rows: u16) -> String {
    let mut vt = Vt::new(cols, rows).expect("local VT");
    vt.feed(&output.bytes());
    vt.snapshot().expect("snapshot").lines().join("\n")
}

fn wait_text(
    output: &Capture,
    cols: u16,
    rows: u16,
    needle: &str,
    timeout: std::time::Duration,
) -> bool {
    wait_until(timeout, || rendered(output, cols, rows).contains(needle))
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

/// The last cursor-shape command (DECSCUSR) in the outer bytes, if any.
fn last_shape(raw: &[u8]) -> Option<usize> {
    (1..=6)
        .filter_map(|shape| rfind(raw, format!("\x1b[{shape} q").as_bytes()))
        .max()
}

#[test]
fn screen_renders_input_resize_and_detaches_cleanly() {
    let Some(mut session) = session() else { return };
    session.start();

    let pty = Pty::open(40, 10);
    let (stdin, stdout) = pty.stdio();
    let mut command = session.command(HT);
    command
        .args(["screen", "--cols", "40", "--rows", "10", "--"])
        .args(["sh", "-c", SHELL])
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn ht screen");
    let output = Capture::spawn(pty.reader());
    let errors = Capture::spawn(child.stderr.take().expect("stderr"));
    let mut input = pty.writer();
    let mut screen = Probe::new(child);

    assert!(
        wait_text(&output, 40, 10, "READY", secs(15)),
        "no READY was drawn; raw={:?} stderr={:?}",
        output.text(),
        errors.text()
    );
    assert!(
        output.contains(b"READY"),
        "READY is missing from the outer bytes; raw={:?}",
        output.text()
    );
    assert!(
        !pty.is_cooked(),
        "ht screen did not put its terminal into raw mode"
    );
    assert!(
        output.contains(b"\x1b[?1049h"),
        "ht screen did not enter the alternate screen"
    );
    assert!(
        wait_text(&output, 40, 10, "你好 👋", secs(5)),
        "wide characters and graphemes were not drawn; raw={:?} stderr={:?}",
        output.text(),
        errors.text()
    );
    // Independent of the implementation's own Vt: the bytes for each glyph
    // must have been written to the outer terminal.
    for glyph in ["你", "好", "👋"] {
        assert!(
            output.contains(glyph.as_bytes()),
            "{glyph} is missing from the outer bytes; raw={:?}",
            output.text()
        );
    }

    let labelled = |session: &Session| -> bool {
        let addresses: Vec<String> = session
            .terminals()
            .iter()
            .filter_map(|terminal| terminal["address"].as_str().map(str::to_owned))
            .collect();
        addresses.iter().any(|address| {
            let record = session.hc_json(&["get", address.as_str()]);
            record["labels"]["herdr.client"] == "herdr-terminal"
        })
    };
    assert!(
        wait_until(secs(2), || labelled(&session)),
        "ht screen's terminal is not labelled: {:?}",
        session.terminals()
    );

    // Keys go to the program; the program's echo is drawn as cells.
    input.write_all(b"hi\n").expect("send input");
    input.flush().expect("flush input");
    assert!(
        wait_text(&output, 40, 10, "GOT:hi", secs(5)),
        "input was not drawn; raw={:?} stderr={:?}",
        output.text(),
        errors.text()
    );
    assert!(
        output.contains(b"GOT:hi"),
        "the echoed input is missing from the outer bytes; raw={:?}",
        output.text()
    );

    // A terminal resize reaches the program through RESIZE and the redraw
    // uses the new size. The test's pty is not a controlling terminal, so
    // the SIGWINCH a real terminal would deliver is sent explicitly.
    pty.resize(50, 12);
    screen.signal(libc::SIGWINCH);
    assert!(
        wait_text(&output, 50, 12, "12 50", secs(10)),
        "resize was not forwarded; raw={:?} stderr={:?}",
        output.text(),
        errors.text()
    );

    // Ctrl-] detaches: the program is ended, the record removed, and the
    // outer terminal restored.
    input.write_all(&[0x1d]).expect("send Ctrl-]");
    input.flush().expect("flush Ctrl-]");
    let status = screen
        .wait_timeout(secs(5))
        .expect("ht screen exits after Ctrl-]");
    assert!(
        status.success(),
        "ht screen exit {status}; stderr={:?}",
        errors.text()
    );
    assert!(
        pty.is_cooked(),
        "ht screen did not restore the terminal after raw mode"
    );
    let raw = output.bytes();
    let shape = last_shape(&raw).expect("the screen set a cursor shape");
    let reset = rfind(&raw, b"\x1b[0 q").expect("the cursor shape was not reset");
    assert!(
        reset > shape,
        "a stale cursor shape was left behind; raw tail={:?}",
        String::from_utf8_lossy(&raw[raw.len().saturating_sub(200)..])
    );
    let gone = wait_until(secs(5), || session.terminals().is_empty());
    assert!(
        gone,
        "ht screen left its terminal behind: {:?}",
        session.terminals()
    );
}

#[test]
fn screen_returns_the_programs_exit_status_and_draws_its_last_output() {
    let Some(mut session) = session() else { return };
    session.start();

    // The program can exit before attach (the recovered path) or after it
    // (the stream path); both must report 3 and show the last line. Repeating
    // exercises the DATA+CLOSE race that used to drop it.
    for attempt in 0..5 {
        let pty = Pty::open(40, 10);
        let (stdin, stdout) = pty.stdio();
        let mut command = session.command(HT);
        command
            .args(["screen", "--cols", "40", "--rows", "10", "--"])
            .args(["sh", "-c", "printf 'BYE\\n'; exit 3"])
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("spawn ht screen");
        let output = Capture::spawn(pty.reader());
        let errors = Capture::spawn(child.stderr.take().expect("stderr"));
        let mut screen = Probe::new(child);

        let status = screen
            .wait_timeout(secs(10))
            .expect("ht screen exits when the program does");
        assert_eq!(
            status.code(),
            Some(3),
            "attempt {attempt}: ht screen exit {status}; stderr={:?}",
            errors.text()
        );
        assert!(
            pty.is_cooked(),
            "attempt {attempt}: ht screen did not restore the terminal"
        );
        assert!(
            output.contains(b"BYE"),
            "attempt {attempt}: the program's last output was lost; raw={:?}",
            output.text()
        );
        let gone = wait_until(secs(5), || session.terminals().is_empty());
        assert!(
            gone,
            "attempt {attempt}: ht screen left a finished terminal behind: {:?}",
            session.terminals()
        );
    }
}

#[test]
fn screen_restores_the_cursor_shape_it_changed() {
    let Some(mut session) = session() else { return };
    session.start();

    let pty = Pty::open(40, 10);
    let (stdin, stdout) = pty.stdio();
    let mut command = session.command(HT);
    command
        .args(["screen", "--cols", "40", "--rows", "10", "--"])
        // The shape is emitted only after attach has landed (triggered by a
        // key), so it reaches the viewer as live output.
        .args([
            "sh",
            "-c",
            "printf 'READY\\n'; read x; printf '\\033[5 q'; sleep 30",
        ])
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn ht screen");
    let output = Capture::spawn(pty.reader());
    let errors = Capture::spawn(child.stderr.take().expect("stderr"));
    let mut input = pty.writer();
    let mut screen = Probe::new(child);

    assert!(
        output.wait_for(b"READY", secs(10)),
        "no READY was drawn; raw={:?} stderr={:?}",
        output.text(),
        errors.text()
    );
    input
        .write_all(b"go\n")
        .expect("let the program set its shape");
    input.flush().expect("flush input");
    assert!(
        output.wait_for(b"\x1b[5 q", secs(5)),
        "the program's bar cursor did not reach the outer terminal; raw={:?}",
        output.text()
    );

    input.write_all(&[0x1d]).expect("send Ctrl-]");
    input.flush().expect("flush Ctrl-]");
    let status = screen
        .wait_timeout(secs(5))
        .expect("ht screen exits after Ctrl-]");
    assert!(
        status.success(),
        "ht screen exit {status}; stderr={:?}",
        errors.text()
    );
    let raw = output.bytes();
    let bar = rfind(&raw, b"\x1b[5 q").expect("the bar shape was set");
    let reset = rfind(&raw, b"\x1b[0 q").expect("the cursor shape was not reset");
    assert!(
        reset > bar,
        "the bar cursor was left behind; raw tail={:?}",
        String::from_utf8_lossy(&raw[raw.len().saturating_sub(200)..])
    );
    assert!(pty.is_cooked(), "ht screen did not restore the terminal");
    let gone = wait_until(secs(5), || session.terminals().is_empty());
    assert!(
        gone,
        "ht screen left its terminal behind: {:?}",
        session.terminals()
    );
}

#[test]
fn screen_stays_responsive_while_the_program_floods_output() {
    let Some(mut session) = session() else { return };
    session.start();

    let pty = Pty::open(80, 24);
    let (stdin, stdout) = pty.stdio();
    let mut command = session.command(HT);
    command
        .args(["screen", "--cols", "80", "--rows", "24", "--"])
        // The flood starts only after a key, so attach reliably sees READY
        // and the test knows when output is actually flooding.
        .args(["sh", "-c", "printf 'READY\\n'; read x; yes X"])
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn ht screen");
    let output = Capture::spawn(pty.reader());
    let errors = Capture::spawn(child.stderr.take().expect("stderr"));
    let mut input = pty.writer();
    let mut screen = Probe::new(child);

    assert!(
        output.wait_for(b"READY", secs(10)),
        "the first frame was not drawn; raw_len={} stderr={:?}",
        output.bytes().len(),
        errors.text()
    );

    // Start the flood and wait until it is visibly reaching the outer
    // terminal, so the detach below happens under load.
    input.write_all(b"go\n").expect("start the flood");
    input.flush().expect("flush input");
    // `yes X` fills one X per row and the grid then repeats as it scrolls,
    // so a couple of dozen X's prove the flood is being drawn.
    assert!(
        output.wait_count(b'X', 20, secs(10)),
        "the flood never reached the outer terminal; raw_len={} stderr={:?}",
        output.bytes().len(),
        errors.text()
    );

    input.write_all(&[0x1d]).expect("send Ctrl-]");
    input.flush().expect("flush Ctrl-]");
    let status = screen
        .wait_timeout(secs(5))
        .expect("Ctrl-] must be read during a flood");
    assert!(
        status.success(),
        "ht screen exit {status} during a flood; stderr={:?}",
        errors.text()
    );
    assert!(
        pty.is_cooked(),
        "ht screen did not restore the terminal after a flood"
    );
    let gone = wait_until(secs(5), || session.terminals().is_empty());
    assert!(
        gone,
        "ht screen left its terminal behind after a flood: {:?}",
        session.terminals()
    );
}
