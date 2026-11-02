#![cfg(unix)]
//! End-to-end M2 tests: a real core in an isolated session, the real `ht`
//! under a PTY, and the rendered output read back through a local VT.
//! Lifecycle tests also use `hc` to inspect the terminal records and to stop
//! or restart the core. Tests skip when herdr-core is not installed.

mod common;

use common::pty::Pty;
use common::{Capture, HT, Probe, Session, secs, wait_until};
use herdr_terminal::vt::Vt;
use std::io::Write;
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

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
/// grid, so assertions are about what `ht` drew, not raw escape bytes.
fn render(output: &Capture, cols: u16, rows: u16) -> String {
    let mut vt = Vt::new(cols, rows).expect("local VT");
    vt.feed(&output.bytes());
    vt.snapshot().expect("snapshot").lines().join("\n")
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

/// One `ht` under a PTY, with its output captured.
struct ScreenRun {
    pty: Pty,
    probe: Probe,
    output: Capture,
    errors: Capture,
    input: std::fs::File,
    cols: u16,
    rows: u16,
}

impl ScreenRun {
    /// Runs `ht` with `args` (after the binary) at the given PTY size. The
    /// size is also passed with `--cols/--rows` by the callers that need it;
    /// the PTY size covers the default path.
    fn spawn(session: &Session, args: &[&str], cols: u16, rows: u16) -> ScreenRun {
        let pty = Pty::open(cols, rows);
        let (stdin, stdout) = pty.stdio();
        let mut command = session.command(HT);
        command
            .args(args)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("spawn ht");
        let output = Capture::spawn(pty.reader());
        let errors = Capture::spawn(child.stderr.take().expect("stderr"));
        let input = pty.writer();
        let probe = Probe::new(child);
        ScreenRun {
            pty,
            probe,
            output,
            errors,
            input,
            cols,
            rows,
        }
    }

    fn raw(&self) -> Vec<u8> {
        self.output.bytes()
    }

    fn rendered(&self) -> String {
        render(&self.output, self.cols, self.rows)
    }

    fn wait_text(&self, needle: &str, timeout: Duration) -> bool {
        wait_until(timeout, || self.rendered().contains(needle))
    }

    fn wait_raw(&self, needle: &[u8], timeout: Duration) -> bool {
        self.output.wait_for(needle, timeout)
    }

    fn send(&mut self, bytes: &[u8]) {
        self.input.write_all(bytes).expect("write to the screen");
        self.input.flush().expect("flush the screen");
    }

    fn wait_exit(&mut self, timeout: Duration) -> Option<ExitStatus> {
        self.probe.wait_timeout(timeout)
    }

    fn errors(&self) -> String {
        self.errors.text()
    }
}

/// The live terminals carrying this client's label, in address order.
fn labelled_live(session: &Session) -> Vec<String> {
    session
        .terminals()
        .iter()
        .filter_map(|terminal| terminal["address"].as_str())
        .filter(|address| {
            session.hc_json(&["get", address])["labels"]["herdr.client"] == "herdr-terminal"
        })
        .map(str::to_owned)
        .collect()
}

/// The one terminal this single-pane test expects, and no duplicate.
fn only_labelled(session: &Session) -> String {
    let terminals = labelled_live(session);
    assert_eq!(
        terminals.len(),
        1,
        "expected exactly one labelled terminal, found {terminals:?}"
    );
    terminals[0].clone()
}

/// Waits until the screen has started its terminal, then returns its address.
/// Input written before that only reaches the outer pty's echo, not the
/// program.
fn wait_only_labelled(session: &Session, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        let terminals = labelled_live(session);
        if !terminals.is_empty() {
            assert_eq!(
                terminals.len(),
                1,
                "expected exactly one labelled terminal, found {terminals:?}"
            );
            return terminals[0].clone();
        }
        assert!(
            Instant::now() < deadline,
            "the screen never started a labelled terminal"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Ends and removes a terminal a test detached from, so the isolated core is
/// empty when the session is dropped.
fn cleanup(session: &Session, address: &str) {
    let _ = session.hc_try(&["kill", address]);
    let _ = session.hc_try(&["rm", address]);
    assert!(
        wait_until(secs(5), || session.terminals().is_empty()),
        "cleanup left a terminal behind: {:?}",
        session.terminals()
    );
}

/// The `hc connect` child of `parent`, once it exists.
fn connect_child(parent: u32, timeout: Duration) -> Option<u32> {
    let deadline = Instant::now() + timeout;
    loop {
        let output = std::process::Command::new("pgrep")
            .args(["-P", &parent.to_string()])
            .output()
            .ok()?;
        if let Some(pid) = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .find_map(|pid| pid.parse::<u32>().ok())
        {
            return Some(pid);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn screen_renders_input_resize_and_detaches_cleanly() {
    let Some(mut session) = session() else { return };
    session.start();

    let mut run = ScreenRun::spawn(
        &session,
        &[
            "screen", "--cols", "40", "--rows", "10", "--", "sh", "-c", SHELL,
        ],
        40,
        10,
    );
    assert!(
        run.wait_text("READY", secs(15)),
        "no READY was drawn; raw={:?} stderr={:?}",
        run.output.text(),
        run.errors()
    );
    assert!(
        run.output.contains(b"READY"),
        "READY is missing from the outer bytes; raw={:?}",
        run.output.text()
    );
    assert!(
        !run.pty.is_cooked(),
        "ht screen did not put its terminal into raw mode"
    );
    assert!(
        run.output.contains(b"\x1b[?1049h"),
        "ht screen did not enter the alternate screen"
    );
    assert!(
        run.wait_text("你好 👋", secs(5)),
        "wide characters and graphemes were not drawn; raw={:?} stderr={:?}",
        run.output.text(),
        run.errors()
    );
    // Independent of the implementation's own Vt: the bytes for each glyph
    // must have been written to the outer terminal.
    for glyph in ["你", "好", "👋"] {
        assert!(
            run.output.contains(glyph.as_bytes()),
            "{glyph} is missing from the outer bytes; raw={:?}",
            run.output.text()
        );
    }

    let address = only_labelled(&session);
    let record = session.hc_json(&["get", &address]);
    assert_eq!(record["labels"]["herdr.client"], "herdr-terminal");

    // Keys go to the program; the program's echo is drawn as cells.
    run.send(b"hi\n");
    assert!(
        run.wait_text("GOT:hi", secs(5)),
        "input was not drawn; raw={:?} stderr={:?}",
        run.output.text(),
        run.errors()
    );
    assert!(
        run.output.contains(b"GOT:hi"),
        "the echoed input is missing from the outer bytes; raw={:?}",
        run.output.text()
    );

    // A terminal resize reaches the program through RESIZE and the redraw
    // uses the new size. The test's pty is not a controlling terminal, so
    // the SIGWINCH a real terminal would deliver is sent explicitly.
    run.pty.resize(50, 12);
    run.probe.signal(libc::SIGWINCH);
    run.cols = 50;
    run.rows = 12;
    assert!(
        run.wait_text("12 50", secs(10)),
        "resize was not forwarded; raw={:?} stderr={:?}",
        run.output.text(),
        run.errors()
    );

    // Ctrl-] detaches: the program keeps running, the record stays, and the
    // outer terminal is restored. A later ht adopts the same terminal.
    run.send(&[0x1d]);
    let status = run.wait_exit(secs(5)).expect("ht exits after Ctrl-]");
    assert!(
        status.success(),
        "ht exit {status}; stderr={:?}",
        run.errors()
    );
    assert!(
        run.pty.is_cooked(),
        "ht screen did not restore the terminal after raw mode"
    );
    let raw = run.raw();
    let shape = last_shape(&raw).expect("the screen set a cursor shape");
    let reset = rfind(&raw, b"\x1b[0 q").expect("the cursor shape was not reset");
    assert!(
        reset > shape,
        "a stale cursor shape was left behind; raw tail={:?}",
        String::from_utf8_lossy(&raw[raw.len().saturating_sub(200)..])
    );
    let record = session.hc_json(&["get", &address]);
    assert_eq!(
        record["state"], "live",
        "Ctrl-] must leave the program running"
    );
    assert_eq!(record["keep"], true, "the screen's terminal must be kept");
    let pointer = session.state_home().join("herdr-terminal/screen.json");
    let saved = std::fs::read_to_string(&pointer).expect("the pointer was written");
    assert!(
        saved.contains(&address),
        "the pointer does not name {address}: {saved}"
    );
    cleanup(&session, &address);
}

#[test]
fn screen_returns_the_programs_exit_status_and_draws_its_last_output() {
    let Some(mut session) = session() else { return };
    session.start();

    // The program can exit before attach (the recovered path) or after it
    // (the stream path); both must report 3 and show the last line. Repeating
    // exercises the DATA+CLOSE race that used to drop it.
    for attempt in 0..5 {
        let mut run = ScreenRun::spawn(
            &session,
            &[
                "screen",
                "--cols",
                "40",
                "--rows",
                "10",
                "--",
                "sh",
                "-c",
                "printf 'BYE\\n'; exit 3",
            ],
            40,
            10,
        );

        let status = run
            .wait_exit(secs(10))
            .expect("ht exits when the program does");
        assert_eq!(
            status.code(),
            Some(3),
            "attempt {attempt}: ht exit {status}; stderr={:?}",
            run.errors()
        );
        assert!(
            run.pty.is_cooked(),
            "attempt {attempt}: ht screen did not restore the terminal"
        );
        assert!(
            run.output.contains(b"BYE"),
            "attempt {attempt}: the program's last output was lost; raw={:?}",
            run.output.text()
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

    let mut run = ScreenRun::spawn(
        &session,
        &[
            "screen",
            "--cols",
            "40",
            "--rows",
            "10",
            "--",
            "sh",
            "-c",
            "printf 'READY\\n'; read x; printf '\\033[5 q'; sleep 30",
        ],
        40,
        10,
    );
    assert!(
        run.wait_raw(b"READY", secs(10)),
        "no READY was drawn; raw={:?} stderr={:?}",
        run.output.text(),
        run.errors()
    );
    run.send(b"go\n");
    assert!(
        run.wait_raw(b"\x1b[5 q", secs(5)),
        "the program's bar cursor did not reach the outer terminal; raw={:?}",
        run.output.text()
    );

    run.send(&[0x1d]);
    let status = run.wait_exit(secs(5)).expect("ht exits after Ctrl-]");
    assert!(
        status.success(),
        "ht exit {status}; stderr={:?}",
        run.errors()
    );
    let raw = run.raw();
    let bar = rfind(&raw, b"\x1b[5 q").expect("the bar shape was set");
    let reset = rfind(&raw, b"\x1b[0 q").expect("the cursor shape was not reset");
    assert!(
        reset > bar,
        "the bar cursor was left behind; raw tail={:?}",
        String::from_utf8_lossy(&raw[raw.len().saturating_sub(200)..])
    );
    assert!(
        run.pty.is_cooked(),
        "ht screen did not restore the terminal"
    );
    let address = only_labelled(&session);
    cleanup(&session, &address);
}

#[test]
fn screen_stays_responsive_while_the_program_floods_output() {
    let Some(mut session) = session() else { return };
    session.start();

    let mut run = ScreenRun::spawn(
        &session,
        &[
            "screen",
            "--cols",
            "80",
            "--rows",
            "24",
            "--",
            "sh",
            "-c",
            "printf 'READY\\n'; read x; yes X",
        ],
        80,
        24,
    );
    assert!(
        run.wait_raw(b"READY", secs(10)),
        "the first frame was not drawn; raw_len={} stderr={:?}",
        run.raw().len(),
        run.errors()
    );

    run.send(b"go\n");
    assert!(
        run.output.wait_count(b'X', 20, secs(10)),
        "the flood never reached the outer terminal; raw_len={} stderr={:?}",
        run.raw().len(),
        run.errors()
    );

    run.send(&[0x1d]);
    let status = run
        .wait_exit(secs(5))
        .expect("Ctrl-] must be read during a flood");
    assert!(
        status.success(),
        "ht exit {status} during a flood; stderr={:?}",
        run.errors()
    );
    assert!(
        run.pty.is_cooked(),
        "ht did not restore the terminal after a flood"
    );
    let address = only_labelled(&session);
    cleanup(&session, &address);
}

#[test]
fn ctrl_c_is_input_to_the_program_not_a_quit() {
    let Some(mut session) = session() else { return };
    session.start();

    // The trap proves the byte reached the program. A kept terminal whose
    // program is signaled is restarted rather than reported as an exit, so
    // the client must stay up instead of quitting.
    let mut run = ScreenRun::spawn(
        &session,
        &[
            "screen",
            "--cols",
            "40",
            "--rows",
            "10",
            "--",
            "sh",
            "-c",
            "trap 'printf \"GOT-INT\\n\"' INT; printf 'READY-FOR-INT\\n'; \
             while :; do sleep 0.1; done",
        ],
        40,
        10,
    );
    assert!(
        run.wait_text("READY-FOR-INT", secs(10)),
        "the program did not start; raw={:?} stderr={:?}",
        run.output.text(),
        run.errors()
    );

    // Ctrl-C belongs to the program: the outer terminal has no signals in raw
    // mode, so ht must forward 0x03.
    run.send(&[0x03]);
    assert!(
        run.wait_text("GOT-INT", secs(10)),
        "Ctrl-C did not reach the program; raw={:?} stderr={:?}",
        run.output.text(),
        run.errors()
    );
    assert!(
        run.probe.wait_timeout(Duration::ZERO).is_none(),
        "ht must not quit on Ctrl-C; stderr={:?}",
        run.errors()
    );

    run.send(&[0x1d]);
    let status = run.wait_exit(secs(5)).expect("the client detaches");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    assert!(run.pty.is_cooked(), "the terminal was not restored");
    let address = only_labelled(&session);
    cleanup(&session, &address);
}

#[test]
fn a_program_that_ends_while_detached_is_recovered_by_the_next_ht() {
    let Some(mut session) = session() else { return };
    session.start();

    let mut first = ScreenRun::spawn(&session, &["--cols", "40", "--rows", "10"], 40, 10);
    let address = wait_only_labelled(&session, secs(10));
    first.send(&[0x1d]);
    first.wait_exit(secs(5)).expect("the first client detaches");

    // The program ends with no viewer attached; core keeps its record and
    // final screen because the screen started it with keep: true, rm: false.
    session.hc_json(&["send", &address, "printf 'FINAL-%s\\n' BYE; exit 7"]);
    assert!(
        wait_until(secs(10), || {
            session.hc_json(&["get", &address])["state"] == "finished"
        }),
        "the program did not finish: {:?}",
        session.hc_json(&["get", &address])
    );

    // The next client finds the finished terminal through its pointer, reads
    // its final screen, reports its status, and removes the record.
    let mut second = ScreenRun::spawn(&session, &["--cols", "40", "--rows", "10"], 40, 10);
    let status = second
        .wait_exit(secs(10))
        .expect("the next ht recovers the finished terminal");
    assert_eq!(
        status.code(),
        Some(7),
        "the program's exit status must be reported; raw={:?} stderr={:?}",
        second.output.text(),
        second.errors()
    );
    assert!(
        second.output.contains(b"FINAL-BYE"),
        "the final screen was not recovered; raw={:?} stderr={:?}",
        second.output.text(),
        second.errors()
    );
    assert!(
        wait_until(secs(5), || session.terminals().is_empty()),
        "the recovered record was not removed: {:?}",
        session.terminals()
    );
}

#[test]
fn two_live_labelled_terminals_are_a_clear_failure_not_a_duplicate() {
    let Some(mut session) = session() else { return };
    session.start();

    // Two terminals an earlier client could have left behind.
    for _ in 0..2 {
        session.hc_json(&[
            "run",
            "--json",
            "--keep",
            "--bg",
            "--label",
            "herdr.client=herdr-terminal",
            "--",
            "sh",
            "-c",
            "sleep 30",
        ]);
    }
    let addresses = labelled_live(&session);
    assert_eq!(addresses.len(), 2, "setup: {addresses:?}");

    let mut run = ScreenRun::spawn(&session, &["--cols", "40", "--rows", "10"], 40, 10);
    let status = run
        .wait_exit(secs(10))
        .expect("ht fails instead of guessing");
    assert_eq!(
        status.code(),
        Some(1),
        "ambiguity must be a clear failure; stderr={:?}",
        run.errors()
    );
    assert!(
        run.errors().contains("several live terminals"),
        "the failure must name the ambiguity; stderr={:?}",
        run.errors()
    );
    assert_eq!(
        labelled_live(&session),
        addresses,
        "no duplicate terminal may be started"
    );

    for address in &addresses {
        let _ = session.hc_try(&["kill", address]);
        let _ = session.hc_try(&["rm", address]);
    }
    assert!(
        wait_until(secs(5), || session.terminals().is_empty()),
        "cleanup left terminals behind: {:?}",
        session.terminals()
    );
}

#[test]
fn ht_reattaches_to_the_terminal_a_previous_client_left() {
    let Some(mut session) = session() else { return };
    session.start();

    let mut first = ScreenRun::spawn(&session, &["--cols", "40", "--rows", "10"], 40, 10);
    let address = wait_only_labelled(&session, secs(10));
    // The marker cannot appear in the typed line's echo: only a running shell
    // turns `%s` into the word.
    first.send(b"printf 'FIRST-%s\\n' DONE\n");
    assert!(
        first.wait_text("FIRST-DONE", secs(10)),
        "the first client did not run the shell; raw={:?} stderr={:?}",
        first.output.text(),
        first.errors()
    );

    first.send(&[0x1d]);
    let status = first.wait_exit(secs(5)).expect("the first client detaches");
    assert!(status.success(), "detach failed: {:?}", first.errors());
    assert_eq!(
        session.hc_json(&["get", &address])["state"],
        "live",
        "detach must not end the program"
    );

    // The second client must adopt the same terminal, not start a duplicate.
    let mut second = ScreenRun::spawn(&session, &["--cols", "40", "--rows", "10"], 40, 10);
    assert!(
        second.wait_text("FIRST-DONE", secs(10)),
        "the second client did not reattach to the running shell; raw={:?} stderr={:?}",
        second.output.text(),
        second.errors()
    );
    assert_eq!(
        only_labelled(&session),
        address,
        "a reattach must not spawn a duplicate terminal"
    );
    second.send(b"printf 'SECOND-%s\\n' DONE\n");
    assert!(
        second.wait_text("SECOND-DONE", secs(10)),
        "input did not reach the reattached shell; raw={:?} stderr={:?}",
        second.output.text(),
        second.errors()
    );

    second.send(&[0x1d]);
    let status = second
        .wait_exit(secs(5))
        .expect("the second client detaches");
    assert!(status.success(), "detach failed: {:?}", second.errors());
    cleanup(&session, &address);
}

#[test]
fn ht_reconnects_when_its_connection_child_dies() {
    let Some(mut session) = session() else { return };
    session.start();

    let mut run = ScreenRun::spawn(&session, &["--cols", "40", "--rows", "10"], 40, 10);
    let address = wait_only_labelled(&session, secs(10));
    run.send(b"printf 'BEFORE-RECONNECT-%s\\n' DONE\n");
    assert!(
        run.wait_text("BEFORE-RECONNECT-DONE", secs(10)),
        "the shell did not start; raw={:?} stderr={:?}",
        run.output.text(),
        run.errors()
    );

    let child = connect_child(run.probe.id(), secs(5)).expect("ht has an hc connect child");
    // The program is core's; only the client's connection dies here.
    // SAFETY: the pid is this test's child process of its own ht.
    assert_eq!(unsafe { libc::kill(child as i32, libc::SIGKILL) }, 0);

    // Input sent while reconnecting is dropped; keep sending until the new
    // connection carries one. That also proves input still works afterwards.
    // The marker cannot come from the typed line's echo.
    let deadline = Instant::now() + secs(20);
    let mut seen = false;
    while Instant::now() < deadline {
        run.send(b"printf 'AFTER-RECONNECT-%s\\n' DONE\n");
        if run.wait_text("AFTER-RECONNECT-DONE", secs(1)) {
            seen = true;
            break;
        }
    }
    assert!(
        seen,
        "ht did not reconnect and reattach; raw tail={:?} stderr={:?}",
        String::from_utf8_lossy(&run.raw()[run.raw().len().saturating_sub(400)..]),
        run.errors()
    );
    assert_eq!(
        only_labelled(&session),
        address,
        "a reconnect must not spawn a duplicate terminal"
    );
    assert_eq!(
        session.hc_json(&["get", &address])["state"],
        "live",
        "the program must keep running across a client reconnect"
    );

    run.send(&[0x1d]);
    let status = run.wait_exit(secs(5)).expect("the client detaches");
    assert!(status.success(), "detach failed: {:?}", run.errors());
    cleanup(&session, &address);
}

#[test]
fn ht_reattaches_after_a_core_restart() {
    let Some(mut session) = session() else { return };
    session.start();

    let mut run = ScreenRun::spawn(&session, &["--cols", "40", "--rows", "10"], 40, 10);
    let address = wait_only_labelled(&session, secs(10));
    run.send(b"printf 'BEFORE-RESTART-%s\\n' DONE\n");
    assert!(
        run.wait_text("BEFORE-RESTART-DONE", secs(10)),
        "the shell did not start; raw={:?} stderr={:?}",
        run.output.text(),
        run.errors()
    );

    // A kept terminal survives the core process: the next core gives it a
    // fresh shell. The client must resubscribe and reattach to it.
    session.stop();
    session.start();

    let deadline = Instant::now() + secs(20);
    let mut seen = false;
    while Instant::now() < deadline {
        run.send(b"printf 'AFTER-RESTART-%s\\n' DONE\n");
        if run.wait_text("AFTER-RESTART-DONE", secs(1)) {
            seen = true;
            break;
        }
    }
    assert!(
        seen,
        "ht did not reattach after the core restart; raw tail={:?} stderr={:?}",
        String::from_utf8_lossy(&run.raw()[run.raw().len().saturating_sub(400)..]),
        run.errors()
    );
    assert_eq!(
        only_labelled(&session),
        address,
        "a core restart must not spawn a duplicate terminal"
    );

    run.send(&[0x1d]);
    let status = run.wait_exit(secs(5)).expect("the client detaches");
    assert!(status.success(), "detach failed: {:?}", run.errors());
    cleanup(&session, &address);
}

#[test]
fn a_large_burst_reaches_the_screen_and_input_still_works() {
    let Some(mut session) = session() else { return };
    session.start();

    let mut run = ScreenRun::spawn(
        &session,
        &[
            "screen",
            "--cols",
            "80",
            "--rows",
            "24",
            "--",
            "sh",
            "-c",
            "yes A | head -c 5000000; printf '\\nBURST-DONE\\n'; read x; \
             printf 'GOT:%s\\n' \"$x\"",
        ],
        80,
        24,
    );
    assert!(
        run.wait_raw(b"BURST-DONE", secs(20)),
        "the burst's last line never reached the screen; raw_len={} stderr={:?}",
        run.raw().len(),
        run.errors()
    );
    run.send(b"hi\n");
    assert!(
        run.wait_raw(b"GOT:hi", secs(10)),
        "input stalled after the burst; raw_len={} stderr={:?}",
        run.raw().len(),
        run.errors()
    );
    let status = run
        .wait_exit(secs(10))
        .expect("the program ends after read");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    assert!(run.pty.is_cooked(), "the terminal was not restored");
    assert!(
        wait_until(secs(5), || session.terminals().is_empty()),
        "the finished terminal was not removed: {:?}",
        session.terminals()
    );
}
