#![cfg(unix)]
//! End-to-end M3 tests: a real core in an isolated session, the real `ht`
//! workspace under a PTY, and the rendered screen read back through a local
//! VT. `hc` inspects the terminal records, labels and sizes. Tests skip when
//! herdr-core is not installed.
//!
//! Input routing is proven through core's terminal titles: a command typed
//! into the focused pane sets its OSC 2 title, and `hc get` says which
//! terminal received it. That avoids guessing from interleaved pane output.

mod common;

use common::pty::Pty;
use common::{Capture, HT, Probe, Session, secs, wait_until};
use herdr_terminal::vt::Vt;
use serde_json::Value;
use std::io::Write;
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

const PREFIX: u8 = 0x02;
const DETACH: u8 = 0x1d;

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

/// One `ht` workspace under a PTY, with both output streams captured.
struct Run {
    pty: Pty,
    probe: Probe,
    output: Capture,
    errors: Capture,
    input: std::fs::File,
    cols: u16,
    rows: u16,
}

impl Run {
    fn spawn(session: &Session, cols: u16, rows: u16) -> Run {
        Run::spawn_with(session, cols, rows, &[])
    }

    /// Starts the workspace with the first pane running `program` instead of
    /// core's shell, so tests can cover panes with different programs.
    fn spawn_with(session: &Session, cols: u16, rows: u16, program: &[&str]) -> Run {
        let pty = Pty::open(cols, rows);
        let (stdin, stdout) = pty.stdio();
        let cols_arg = cols.to_string();
        let rows_arg = rows.to_string();
        let mut child = session
            .command(HT)
            .args(["--cols", &cols_arg, "--rows", &rows_arg])
            .args(program)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn ht");
        let output = Capture::spawn(pty.reader());
        let errors = Capture::spawn(child.stderr.take().expect("stderr"));
        let input = pty.writer();
        let probe = Probe::new(child);
        Run {
            pty,
            probe,
            output,
            errors,
            input,
            cols,
            rows,
        }
    }

    fn rendered(&self) -> String {
        let mut vt = Vt::new(self.cols, self.rows).expect("local VT");
        vt.feed(&self.output.bytes());
        vt.snapshot().expect("snapshot").lines().join("\n")
    }

    fn wait_screen(&self, timeout: Duration, mut check: impl FnMut(&str) -> bool) -> bool {
        wait_until(timeout, || check(&self.rendered()))
    }

    fn wait_text(&self, needle: &str, timeout: Duration) -> bool {
        self.wait_screen(timeout, |screen| screen.contains(needle))
    }

    fn send(&mut self, bytes: &[u8]) {
        self.input.write_all(bytes).expect("write to ht");
        self.input.flush().expect("flush ht");
    }

    /// Prefix Ctrl-b followed by one command byte.
    fn command(&mut self, key: u8) {
        self.send(&[PREFIX, key]);
    }

    /// Types a shell command into the focused pane and runs it. The escape
    /// stays literal until the shell's printf turns it into an OSC title.
    fn title(&mut self, title: &str) {
        self.send(format!("printf '\\033]2;{title}\\007'\n").as_bytes());
    }

    fn wait_exit(&mut self, timeout: Duration) -> Option<ExitStatus> {
        self.probe.wait_timeout(timeout)
    }

    fn errors(&self) -> String {
        self.errors.text()
    }
}

/// Every live terminal in this isolated session, in creation order.
fn addresses(session: &Session) -> Vec<String> {
    session
        .terminals()
        .iter()
        .filter_map(|terminal| terminal["address"].as_str())
        .map(str::to_owned)
        .collect()
}

/// Every terminal, including finished records kept for a pane's final screen.
fn all_addresses(session: &Session) -> Vec<String> {
    session.hc_json(&["ls", "--all"])["terminals"]
        .as_array()
        .map(|terminals| {
            terminals
                .iter()
                .filter_map(|terminal| terminal["address"].as_str())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn wait_addresses(session: &Session, expected: usize, timeout: Duration) -> Vec<String> {
    let mut last = Vec::new();
    assert!(
        wait_until(timeout, || {
            last = addresses(session);
            last.len() == expected
        }),
        "expected {expected} terminals, found {last:?}"
    );
    last
}

fn record(session: &Session, address: &str) -> Value {
    session.hc_json(&["get", address])
}

fn title_of(session: &Session, address: &str) -> String {
    record(session, address)["title"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn wait_title(session: &Session, address: &str, title: &str, timeout: Duration) -> bool {
    wait_until(timeout, || title_of(session, address) == title)
}

fn foreground_of(session: &Session, address: &str) -> String {
    record(session, address)["foreground"]["name"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn wait_foreground(session: &Session, address: &str, name: &str, timeout: Duration) -> bool {
    wait_until(timeout, || foreground_of(session, address) == name)
}

fn label(session: &Session, address: &str, key: &str) -> String {
    record(session, address)["labels"][key]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn sizes(session: &Session, known: &[String]) -> Vec<(u64, u64)> {
    known
        .iter()
        .map(|address| {
            let record = record(session, address);
            (
                record["cols"].as_u64().unwrap_or(0),
                record["rows"].as_u64().unwrap_or(0),
            )
        })
        .collect()
}

fn wait_sizes_changed(
    session: &Session,
    known: &[String],
    before: &[(u64, u64)],
    timeout: Duration,
) -> bool {
    wait_until(timeout, || {
        sizes(session, known)
            .iter()
            .zip(before)
            .all(|(now, old)| now != old)
    })
}

fn cleanup_all(session: &Session) {
    for address in addresses(session) {
        let _ = session.hc_try(&["kill", &address]);
        let _ = session.hc_try(&["rm", &address]);
    }
    assert!(
        wait_until(secs(5), || addresses(session).is_empty()),
        "cleanup left terminals behind: {:?}",
        addresses(session)
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
fn four_panes_split_route_input_and_follow_the_window_size() {
    let Some(mut session) = session() else { return };
    session.start();
    // The first pane runs a program of its own; the three panes split from it
    // run core's shell, so the workspace is managing different programs.
    let mut run = Run::spawn_with(
        &session,
        80,
        24,
        &[
            "--",
            "sh",
            "-c",
            "printf 'PANE-INIT-%s\\n' OK; while :; do sleep 1; done",
        ],
    );
    wait_addresses(&session, 1, secs(10));
    assert!(
        run.wait_text("PANE-INIT-OK", secs(10)),
        "the first pane's program did not run; screen={:?} stderr={:?}",
        run.rendered(),
        run.errors()
    );

    // v (columns) twice, then s (rows) on the newest pane.
    run.command(b'v');
    wait_addresses(&session, 2, secs(10));
    run.command(b'v');
    wait_addresses(&session, 3, secs(10));
    run.command(b's');
    let known = wait_addresses(&session, 4, secs(10));

    // One long-lived connection, four attach streams: each terminal carries
    // the client label and the same space/tab ids.
    let space = label(&session, &known[0], "herdr.terminal.space");
    let tab = label(&session, &known[0], "herdr.terminal.tab");
    assert!(!space.is_empty() && !tab.is_empty());
    for address in &known {
        assert_eq!(
            label(&session, address, "herdr.client"),
            "herdr-terminal",
            "{address}"
        );
        assert_eq!(label(&session, address, "herdr.terminal.space"), space);
        assert_eq!(label(&session, address, "herdr.terminal.tab"), tab);
    }

    // The newest split (t4) is focused and below t3 in the third column.
    // Focus up and type: the title lands on t3, not on t4 or the others.
    run.command(b'k');
    run.title("ROUTE-UP");
    assert!(
        wait_title(&session, &known[2], "ROUTE-UP", secs(10)),
        "input did not reach the pane focused up: {:?}",
        known
            .iter()
            .map(|address| (address, title_of(&session, address)))
            .collect::<Vec<_>>()
    );
    assert_ne!(title_of(&session, &known[3]), "ROUTE-UP");

    // Ctrl-C is program input: interrupt a long foreground program in the
    // focused pane, and the shell runs what follows the semicolon.
    run.send(b"sleep 30; printf 'INT-%s\\n' DONE\n");
    assert!(
        wait_foreground(&session, &known[2], "sleep", secs(10)),
        "the focused pane never ran sleep; foreground={:?}",
        foreground_of(&session, &known[2])
    );
    run.send(&[0x03]);
    assert!(
        run.wait_text("INT-DONE", secs(10)),
        "Ctrl-C did not reach the focused program; screen={:?}",
        run.rendered()
    );

    // A window resize reaches every visible program, through each pane's
    // own RESIZE on its own attach stream.
    let before = sizes(&session, &known);
    run.pty.resize(100, 30);
    run.cols = 100;
    run.rows = 30;
    run.probe.signal(libc::SIGWINCH);
    assert!(
        wait_sizes_changed(&session, &known, &before, secs(10)),
        "resize did not reach every pane: before={before:?} after={:?}",
        sizes(&session, &known)
    );

    // x closes the focused owned pane (t3) and ends it.
    run.command(b'x');
    assert!(
        wait_until(secs(10), || addresses(&session).len() == 3),
        "close left {:?}",
        addresses(&session)
    );
    assert!(
        !addresses(&session).contains(&known[2]),
        "closing t3 must end it"
    );

    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("the workspace detaches");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    assert!(run.pty.is_cooked(), "the terminal was not restored");
    assert_eq!(
        addresses(&session).len(),
        3,
        "detach leaves the surviving programs running"
    );
    cleanup_all(&session);
}

#[test]
fn keep_close_leaves_the_program_and_kill_ends_it() {
    let Some(mut session) = session() else { return };
    session.start();
    let mut run = Run::spawn(&session, 80, 24);
    wait_addresses(&session, 1, secs(10));
    run.command(b'v');
    let known = wait_addresses(&session, 2, secs(10));
    let second = &known[1];

    // e: keep the focused pane's program and close the pane. The record
    // stays live and labelled, so it is tracked, not silently orphaned.
    run.command(b'e');
    assert!(
        live(&session, second),
        "keep must leave the program running"
    );
    // The status bar names the address that still runs, so the user can find
    // the program again after the pane is gone.
    assert!(
        run.wait_text(&format!("hc attach {second}"), secs(5)),
        "keep did not show the kept address; screen={:?}",
        run.rendered()
    );
    // X on the now-focused first pane kills and removes it; the kept second
    // terminal must be untouched.
    run.command(b'X');
    assert!(
        wait_until(secs(10), || addresses(&session) == vec![second.clone()]),
        "kill left {:?}, expected only {second}",
        addresses(&session)
    );
    assert!(live(&session, second), "the kept program must keep running");
    assert!(
        run.wait_text("no terminals in this tab", secs(10)),
        "closing the last pane must show the empty state; screen={:?}",
        run.rendered()
    );

    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach from the empty tab");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    assert_eq!(
        label(&session, second, "herdr.client"),
        "herdr-terminal",
        "the kept terminal keeps its workspace labels"
    );
    assert!(live(&session, second));
    cleanup_all(&session);
}

#[test]
fn an_exiting_program_keeps_its_final_screen_and_status() {
    let Some(mut session) = session() else { return };
    session.start();
    let mut run = Run::spawn_with(
        &session,
        80,
        24,
        &["--", "sh", "-c", "printf 'FINAL-%s\\n' OK; exit 7"],
    );

    // Whether the exit beat the first attach or not, the pane must show the
    // program's last output and the record's exact status.
    assert!(
        run.wait_text("FINAL-OK", secs(10)),
        "the final screen was lost; screen={:?} stderr={:?}",
        run.rendered(),
        run.errors()
    );
    assert!(
        run.wait_text("exited (7)", secs(5)),
        "the exit status was not shown; screen={:?}",
        run.rendered()
    );
    let known = all_addresses(&session);
    assert_eq!(known.len(), 1, "the finished record must stay for the pane");
    let record = record(&session, &known[0]);
    assert_eq!(record["state"].as_str(), Some("finished"));
    assert_eq!(record["exit_code"].as_i64(), Some(7));
    assert!(
        !run.rendered().contains("no terminals in this tab"),
        "the finished pane must stay visible; screen={:?}",
        run.rendered()
    );

    // x closes the finished pane and only then removes its record.
    run.command(b'x');
    assert!(
        wait_until(secs(10), || all_addresses(&session).is_empty()),
        "closing the finished pane did not remove its record: {:?}",
        all_addresses(&session)
    );
    assert!(
        run.wait_text("no terminals in this tab", secs(5)),
        "the empty state did not follow the close; screen={:?}",
        run.rendered()
    );

    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    assert!(run.pty.is_cooked());
}

fn live(session: &Session, address: &str) -> bool {
    record(session, address)["state"].as_str() == Some("live")
}

fn tab_bar(run: &Run) -> String {
    run.rendered().lines().next().unwrap_or_default().to_owned()
}

#[test]
fn move_zoom_and_rename_change_the_screen() {
    let Some(mut session) = session() else { return };
    session.start();
    let mut run = Run::spawn(&session, 80, 24);
    wait_addresses(&session, 1, secs(10));
    run.command(b'v');
    let known = wait_addresses(&session, 2, secs(10));
    let (left, right) = (&known[0], &known[1]);

    session.hc_json(&["send", left, "printf '\\033]2;LEFT-ONE\\007'"]);
    session.hc_json(&["send", right, "printf '\\033]2;RIGHT-TWO\\007'"]);
    assert!(
        run.wait_screen(secs(10), |screen| {
            screen.contains("LEFT-ONE") && screen.contains("RIGHT-TWO")
        }),
        "pane titles did not reach the borders; screen={:?}",
        run.rendered()
    );
    let before_left = run.rendered().find("LEFT-ONE").expect("left title");
    let before_right = run.rendered().find("RIGHT-TWO").expect("right title");
    assert!(before_left < before_right, "t1 starts on the left");

    // H moves the focused (right) pane left, swapping the two panes.
    run.command(b'H');
    assert!(
        run.wait_screen(secs(10), |screen| {
            let a = screen.find("LEFT-ONE");
            let b = screen.find("RIGHT-TWO");
            a.zip(b).is_some_and(|(a, b)| b < a)
        }),
        "move did not swap the panes; screen={:?}",
        run.rendered()
    );

    // z shows only the focused pane; the bars stay.
    run.command(b'z');
    assert!(
        run.wait_screen(secs(10), |screen| {
            screen.contains("RIGHT-TWO") && !screen.contains("LEFT-ONE")
        }),
        "zoom did not hide the other pane; screen={:?}",
        run.rendered()
    );
    run.command(b'z');
    assert!(
        run.wait_text("LEFT-ONE", secs(10)),
        "unzoom did not bring the other pane back; screen={:?}",
        run.rendered()
    );

    // ? opens help; any key closes it without reaching the program or
    // closing a pane. `x` would close a pane if it were a command.
    run.command(b'?');
    assert!(
        run.wait_text("any key closes help", secs(5)),
        "help did not open; screen={:?}",
        run.rendered()
    );
    run.send(b"x");
    assert!(
        run.wait_screen(secs(5), |screen| !screen.contains("any key closes help")),
        "help did not close; screen={:?}",
        run.rendered()
    );
    assert_eq!(
        addresses(&session).len(),
        2,
        "closing help must not close a pane"
    );

    // , renames the tab through the one-line prompt; Enter commits.
    run.command(b',');
    run.send(b"work\r");
    assert!(
        run.wait_screen(secs(10), |_| tab_bar(&run).contains("work")),
        "the tab rename did not reach the tab bar; screen={:?}",
        run.rendered()
    );
    // Escape cancels, and the keys typed while the prompt was open never
    // reach the program: "nope" disappears with the prompt.
    run.command(b',');
    assert!(run.wait_text("rename tab:", secs(5)));
    run.send(b"nope");
    run.send(&[0x1b]);
    assert!(
        run.wait_screen(secs(5), |screen| {
            !screen.contains("rename tab:") && !screen.contains("nope")
        }),
        "escape did not cancel the rename prompt; screen={:?}",
        run.rendered()
    );
    // . renames the space.
    run.command(b'.');
    run.send(b"main\r");
    assert!(
        run.wait_screen(secs(10), |_| tab_bar(&run).contains("main")),
        "the space rename did not reach the tab bar; screen={:?}",
        run.rendered()
    );

    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    cleanup_all(&session);
}

#[test]
fn a_new_tab_and_a_new_space_get_their_own_labels() {
    let Some(mut session) = session() else { return };
    session.start();
    let mut run = Run::spawn(&session, 80, 24);
    wait_addresses(&session, 1, secs(10));
    run.command(b'c');
    wait_addresses(&session, 2, secs(10));
    run.command(b'C');
    let known = wait_addresses(&session, 3, secs(10));

    let space0 = label(&session, &known[0], "herdr.terminal.space");
    let space1 = label(&session, &known[1], "herdr.terminal.space");
    let space2 = label(&session, &known[2], "herdr.terminal.space");
    let tab0 = label(&session, &known[0], "herdr.terminal.tab");
    let tab1 = label(&session, &known[1], "herdr.terminal.tab");
    let tab2 = label(&session, &known[2], "herdr.terminal.tab");
    assert_eq!(space0, space1, "a new tab stays in the same space");
    assert_ne!(tab0, tab1, "a new tab has a new tab id");
    assert_ne!(space2, space0, "a new space has a new space id");
    assert_ne!(tab2, tab0, "a new space's tab has a new id");
    assert_ne!(tab2, tab1, "a new space's tab has a new id");

    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    cleanup_all(&session);
}

#[test]
fn reconnecting_reattaches_every_pane_without_duplicates() {
    let Some(mut session) = session() else { return };
    session.start();
    let mut run = Run::spawn(&session, 80, 24);
    wait_addresses(&session, 1, secs(10));
    run.command(b'v');
    let known = wait_addresses(&session, 2, secs(10));
    let focused = &known[1];

    // Core lists the second terminal before the workspace necessarily has
    // inserted it. Prove the workspace actually owns the pane and has its
    // attach stream: type an OSC title until the second terminal's record
    // changes. Only then is the connection dropped, so this test exercises
    // reconnect rather than racing terminal.run.
    let deadline = Instant::now() + secs(10);
    let mut ready = false;
    while Instant::now() < deadline {
        run.title("BEFORE-RECONNECT");
        if wait_title(&session, focused, "BEFORE-RECONNECT", secs(1)) {
            ready = true;
            break;
        }
    }
    assert!(
        ready,
        "the workspace did not attach the second pane before the outage; screen={:?} stderr={:?}",
        run.rendered(),
        run.errors()
    );
    assert!(
        run.wait_text("BEFORE-RECONNECT", secs(5)),
        "the attached pane is not drawn; screen={:?}",
        run.rendered()
    );

    let child = connect_child(run.probe.id(), secs(5)).expect("ht has an hc connect child");
    // Only the client's connection dies; the programs stay in core.
    // SAFETY: the pid is this test's child process of its own ht.
    assert_eq!(unsafe { libc::kill(child as i32, libc::SIGKILL) }, 0);

    // Do not inject a command while the connection is down: part of it could
    // be delivered and the rest dropped, leaving the shell in a partial line.
    // Wait for the reconnect to land, then retry complete commands until the
    // focused pane is attached and runs one.
    assert!(
        run.wait_text("reconnected to core", secs(20)),
        "the workspace did not reconnect; screen={:?} stderr={:?}",
        run.rendered(),
        run.errors()
    );
    let deadline = Instant::now() + secs(10);
    let mut attached = false;
    while Instant::now() < deadline {
        run.title("AFTER-RECONNECT");
        if wait_title(&session, focused, "AFTER-RECONNECT", secs(1)) {
            attached = true;
            break;
        }
    }
    assert!(
        attached,
        "the workspace did not reattach the focused pane; screen={:?} stderr={:?}",
        run.rendered(),
        run.errors()
    );
    assert_eq!(
        addresses(&session),
        known,
        "reconnecting must not start or lose terminals"
    );
    assert!(
        run.wait_text("AFTER-RECONNECT", secs(5)),
        "the reattached pane is not drawn; screen={:?}",
        run.rendered()
    );

    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach after reconnect");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    cleanup_all(&session);
}

#[test]
fn pressing_the_prefix_twice_sends_the_literal_prefix_byte() {
    let Some(mut session) = session() else { return };
    session.start();
    // Read exactly one byte from the pane's program and print it as hex.
    let mut run = Run::spawn_with(
        &session,
        80,
        24,
        &[
            "--",
            "sh",
            "-c",
            "b=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\n'); \
             printf 'BYTE-%s\\n' \"$b\"; sleep 30",
        ],
    );
    wait_addresses(&session, 1, secs(10));
    run.send(&[PREFIX, PREFIX]);
    // The first pane's program has canonical stdin: a lone byte is not
    // readable until a line ends. The newline is not part of what it prints.
    run.send(b"\n");
    assert!(
        run.wait_text("BYTE-02", secs(10)),
        "prefix twice did not send a literal Ctrl-b; screen={:?} stderr={:?}",
        run.rendered(),
        run.errors()
    );
    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    cleanup_all(&session);
}
