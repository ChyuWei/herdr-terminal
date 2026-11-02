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

/// A program that reports its own state through OSC 7501 and reads commands
/// from its terminal: the way a real agent talks to core.
const AGENT: &str = r#"printf 'AGENT-READY\n'
printf '\033]2;Agent Build\007'
printf '\033]7501;state=idle:app=demo:title=QnVpbGQ=\033\\'
while IFS= read -r line; do
  case "$line" in
    working) printf '\033]7501;state=working:app=demo:title=QnVpbGQ=:progress=10\033\\' ;;
    done) printf '\033]7501;state=done:app=demo:title=QnVpbGQ=\033\\' ;;
    blocked) printf '\033]7501;state=blocked:kind=permission:app=demo:title=QnVpbGQ=\033\\' ;;
    clear) printf '\033]7501;state=clear\033\\' ;;
  esac
done"#;

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
        Run::spawn_env(session, cols, rows, program, None)
    }

    /// A named screen: its own local view on the shared document.
    fn spawn_as(session: &Session, cols: u16, rows: u16, screen: &str) -> Run {
        Run::spawn_env(session, cols, rows, &[], Some(screen))
    }

    fn spawn_env(
        session: &Session,
        cols: u16,
        rows: u16,
        program: &[&str],
        screen: Option<&str>,
    ) -> Run {
        let pty = Pty::open(cols, rows);
        let (stdin, stdout) = pty.stdio();
        let cols_arg = cols.to_string();
        let rows_arg = rows.to_string();
        let mut command = session.command(HT);
        command
            .args(["--cols", &cols_arg, "--rows", &rows_arg])
            .args(program)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::piped());
        if let Some(screen) = screen {
            command.env("HERDR_TERMINAL_SCREEN", screen);
        }
        let mut child = command.spawn().expect("spawn ht");
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

fn status_state(session: &Session, address: &str) -> Option<String> {
    record(session, address)["status"]["state"]
        .as_str()
        .map(str::to_owned)
}

fn wait_status(session: &Session, address: &str, state: &str, timeout: Duration) -> bool {
    wait_until(timeout, || {
        status_state(session, address).as_deref() == Some(state)
    })
}

fn turn_of(session: &Session, address: &str) -> u64 {
    record(session, address)["turn"].as_u64().unwrap_or(0)
}

fn wait_turn(session: &Session, address: &str, turn: u64, timeout: Duration) -> bool {
    wait_until(timeout, || turn_of(session, address) == turn)
}

fn wait_label(session: &Session, address: &str, key: &str, value: &str, timeout: Duration) -> bool {
    wait_until(timeout, || label(session, address, key) == value)
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

/// Every pane address in a workspace document, in document order.
fn doc_addresses(doc: &Value) -> Vec<String> {
    fn leaves(layout: &Value, out: &mut Vec<String>) {
        if let Some(leaf) = layout.get("leaf") {
            if let Some(address) = leaf.get("address").and_then(Value::as_str) {
                out.push(address.to_owned());
            }
            return;
        }
        if let Some(children) = layout.get("children").and_then(Value::as_array) {
            for child in children {
                if let Some(layout) = child.get("layout") {
                    leaves(layout, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    for space in doc["spaces"].as_array().into_iter().flatten() {
        for tab in space["tabs"].as_array().into_iter().flatten() {
            leaves(&tab["layout"], &mut out);
        }
    }
    out
}

/// A restart restores the document's layout and titles, adopts the same
/// terminals without starting new ones, and applies the local focus and zoom.
#[test]
fn a_restart_restores_layout_titles_zoom_and_focus() {
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
        "titles were not drawn; screen={:?}",
        run.rendered()
    );

    // Focus and zoom the left pane: both are local view, saved on detach.
    run.command(b'h');
    run.command(b'z');
    assert!(
        run.wait_screen(secs(5), |screen| {
            screen.contains("LEFT-ONE") && !screen.contains("RIGHT-TWO")
        }),
        "zoom did not hide the right pane; screen={:?}",
        run.rendered()
    );
    let before = session.doc("herdr-terminal");
    assert_eq!(
        doc_addresses(&before),
        known,
        "the document names both panes; doc={before}"
    );

    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    assert_eq!(
        addresses(&session),
        known,
        "detaching must leave both programs running"
    );
    assert!(
        session.view_file("default").is_file(),
        "the local view was not saved"
    );

    // A fresh screen restores the document and its own view: same terminals,
    // no new ones, the left pane zoomed and focused.
    let mut run = Run::spawn(&session, 80, 24);
    assert!(
        run.wait_screen(secs(10), |screen| {
            screen.contains("LEFT-ONE") && !screen.contains("RIGHT-TWO")
        }),
        "the restored view did not keep the zoom; screen={:?} stderr={:?}",
        run.rendered(),
        run.errors()
    );
    assert_eq!(
        addresses(&session),
        known,
        "restoring must not start terminals"
    );
    assert_eq!(
        doc_addresses(&session.doc("herdr-terminal")),
        known,
        "a clean restore must not change the document"
    );

    run.command(b'z');
    assert!(
        run.wait_text("RIGHT-TWO", secs(5)),
        "unzooming must show the restored right pane; screen={:?}",
        run.rendered()
    );
    run.title("RESTORED-FOCUS");
    assert!(
        wait_title(&session, left, "RESTORED-FOCUS", secs(10)),
        "the restored focus must reach the left pane; titles={:?}",
        known
            .iter()
            .map(|address| (address, title_of(&session, address)))
            .collect::<Vec<_>>()
    );
    assert_ne!(title_of(&session, right), "RESTORED-FOCUS");

    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach after restore");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    assert_eq!(addresses(&session), known);
    assert_eq!(
        doc_addresses(&session.doc("herdr-terminal")),
        known,
        "detaching must leave the document intact"
    );
    cleanup_all(&session);
}

/// Two screens share one document: a change on one is applied on the other,
/// while focus/zoom/active tab stay per screen and nothing is duplicated.
#[test]
fn two_screens_share_the_layout_and_keep_their_own_view() {
    let Some(mut session) = session() else { return };
    session.start();
    let mut first = Run::spawn_as(&session, 80, 24, "one");
    wait_addresses(&session, 1, secs(10));
    first.command(b'v');
    let known = wait_addresses(&session, 2, secs(10));
    let (left, right) = (&known[0], &known[1]);
    session.hc_json(&["send", left, "printf '\\033]2;SCREEN-ONE-A\\007'"]);
    session.hc_json(&["send", right, "printf '\\033]2;SCREEN-ONE-B\\007'"]);
    assert!(
        first.wait_screen(secs(10), |screen| {
            screen.contains("SCREEN-ONE-A") && screen.contains("SCREEN-ONE-B")
        }),
        "the first screen's panes were not drawn; screen={:?}",
        first.rendered()
    );

    // The second screen adopts the same two terminals.
    let mut second = Run::spawn_as(&session, 80, 24, "two");
    assert!(
        second.wait_screen(secs(10), |screen| {
            screen.contains("SCREEN-ONE-A") && screen.contains("SCREEN-ONE-B")
        }),
        "the second screen did not restore the shared layout; screen={:?} stderr={:?}",
        second.rendered(),
        second.errors()
    );
    assert_eq!(
        addresses(&session),
        known,
        "opening a second screen must not start terminals"
    );

    // A new tab on the second screen reaches the first through the document.
    second.command(b'c');
    let three = wait_addresses(&session, 3, secs(10));
    let added = three
        .iter()
        .find(|address| !known.contains(address))
        .expect("the new tab's terminal")
        .clone();
    assert!(
        first.wait_screen(secs(10), |screen| {
            screen
                .lines()
                .next()
                .is_some_and(|bar| bar.contains("tab 2"))
        }),
        "the doc change did not reach the first screen; screen={:?}",
        first.rendered()
    );
    assert_eq!(
        addresses(&session),
        three,
        "the first screen must not start a duplicate"
    );

    // The first screen can visit the shared tab and type into its terminal.
    first.command(b'2');
    assert!(
        first.wait_screen(secs(10), |screen| screen
            .lines()
            .next()
            .is_some_and(|bar| { bar.contains("tab 2") })),
        "the first screen did not switch to the shared tab; screen={:?}",
        first.rendered()
    );
    first.title("FROM-SCREEN-ONE");
    assert!(
        wait_title(&session, &added, "FROM-SCREEN-ONE", secs(10)),
        "the first screen did not attach the tab added by the other; title={:?}",
        title_of(&session, &added)
    );

    // Each screen writes its own local view; the shared document has all
    // three panes and no view state.
    second.send(&[DETACH]);
    let status = second.wait_exit(secs(5)).expect("detach second screen");
    assert_eq!(status.code(), Some(0), "stderr={:?}", second.errors());
    first.send(&[DETACH]);
    let status = first.wait_exit(secs(5)).expect("detach first screen");
    assert_eq!(status.code(), Some(0), "stderr={:?}", first.errors());
    assert!(
        session.view_file("one").is_file() && session.view_file("two").is_file(),
        "each screen keeps its own local view"
    );
    let doc = session.doc("herdr-terminal");
    assert_eq!(
        doc_addresses(&doc),
        three,
        "the document keeps every pane; doc={doc}"
    );
    assert!(
        !doc.to_string().contains("active_space"),
        "the shared document must not hold the local view; doc={doc}"
    );
    assert_eq!(
        addresses(&session),
        three,
        "detaching both screens leaves every program running"
    );
    cleanup_all(&session);
}

/// An explicit PROGRAM on a restored workspace runs in a new tab; the
/// documented panes are adopted, never duplicated.
#[test]
fn an_explicit_program_starts_a_new_tab_after_restore() {
    let Some(mut session) = session() else { return };
    session.start();
    let mut run = Run::spawn(&session, 80, 24);
    wait_addresses(&session, 1, secs(10));
    run.command(b'v');
    let known = wait_addresses(&session, 2, secs(10));
    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    assert_eq!(addresses(&session), known);

    let mut run = Run::spawn_with(
        &session,
        80,
        24,
        &["--", "sh", "-c", "printf 'PROGRAM-%s\\n' OK; sleep 30"],
    );
    assert!(
        run.wait_text("PROGRAM-OK", secs(10)),
        "the explicit program did not run; screen={:?} stderr={:?}",
        run.rendered(),
        run.errors()
    );
    let three = wait_addresses(&session, 3, secs(10));
    assert!(
        known.iter().all(|address| three.contains(address)),
        "the restored terminals must survive; before={known:?} after={three:?}"
    );
    let added: Vec<&String> = three
        .iter()
        .filter(|address| !known.contains(address))
        .collect();
    assert_eq!(added.len(), 1, "exactly one new terminal: {three:?}");
    let added = added[0];
    assert_ne!(
        label(&session, added, "herdr.terminal.tab"),
        label(&session, &known[0], "herdr.terminal.tab"),
        "the program must run in a tab of its own"
    );
    let doc = session.doc("herdr-terminal");
    assert_eq!(doc_addresses(&doc), three, "doc={doc}");
    assert_eq!(
        doc["spaces"][0]["tabs"].as_array().map(Vec::len),
        Some(2),
        "the document must have the restored tab and the program's tab; doc={doc}"
    );

    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    assert_eq!(addresses(&session), three);
    assert_eq!(doc_addresses(&session.doc("herdr-terminal")), three);
    cleanup_all(&session);
}

/// A document deleted by another client while the programs run is rebuilt
/// from the terminals' labels on the next start: same terminals, no new
/// shell, and one recreated document at the retained version.
#[test]
fn a_deleted_document_is_rebuilt_from_live_labeled_terminals() {
    let Some(mut session) = session() else { return };
    session.start();
    let mut run = Run::spawn(&session, 80, 24);
    wait_addresses(&session, 1, secs(10));
    run.command(b'v');
    let known = wait_addresses(&session, 2, secs(10));
    assert!(
        wait_until(secs(5), || doc_addresses(&session.doc("herdr-terminal"))
            == known),
        "the document must name both panes; doc={:?}",
        session.doc("herdr-terminal")
    );
    let (_, version) = session.doc_with_version("herdr-terminal");

    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    assert_eq!(addresses(&session), known);

    // Another client deletes the document; core keeps its version.
    let deleted = session.set_doc("herdr-terminal", &Value::Null, version);
    assert_eq!(deleted, version + 1, "the delete bumps the version");
    let (value, retained) = session.doc_with_version("herdr-terminal");
    assert!(value.is_null(), "the document is gone; value={value}");
    assert_eq!(retained, deleted);

    let mut run = Run::spawn(&session, 80, 24);
    assert!(
        wait_until(secs(10), || doc_addresses(&session.doc("herdr-terminal"))
            == known),
        "the deleted document was not rebuilt from labels; doc={:?}",
        session.doc("herdr-terminal")
    );
    assert_eq!(
        addresses(&session),
        known,
        "recovery must not start a replacement shell"
    );
    let (doc, recreated) = session.doc_with_version("herdr-terminal");
    assert_eq!(
        recreated,
        retained + 1,
        "the document is recreated once at the retained version"
    );
    assert_eq!(doc_addresses(&doc), known);
    assert_eq!(
        doc["spaces"][0]["tabs"][0]["layout"]["children"][0]["layout"]["leaf"]["labels"]["herdr.client"],
        "herdr-terminal",
        "the recreated panes keep their labels; doc={doc}"
    );

    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach after recovery");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    assert_eq!(addresses(&session), known);
    assert_eq!(
        doc_addresses(&session.doc("herdr-terminal")),
        known,
        "detaching leaves the recreated document intact"
    );
    cleanup_all(&session);
}

/// A real program's OSC 7501 reports drive the agent row: status, turn, app,
/// progress, title, foreground, seen/unread and pin through `terminal.label`.
#[test]
fn agents_report_status_turns_seen_and_pin_end_to_end() {
    let Some(mut session) = session() else { return };
    session.start();
    let mut run = Run::spawn_with(&session, 100, 30, &["sh", "-c", AGENT]);
    let address = wait_addresses(&session, 1, secs(10))
        .into_iter()
        .next()
        .expect("agent terminal");
    assert!(
        wait_until(secs(10), || {
            let record = record(&session, &address);
            record["reported"].as_bool() == Some(true)
                && record["status"]["state"] == "idle"
                && record["turn"].as_u64() == Some(0)
        }),
        "the agent did not report idle: {}",
        record(&session, &address)
    );
    assert!(
        wait_title(&session, &address, "Agent Build", secs(5)),
        "core did not keep the program's title"
    );
    assert_eq!(turn_of(&session, &address), 0);
    assert!(
        wait_until(secs(5), || !foreground_of(&session, &address).is_empty()),
        "the foreground process was not reported: {}",
        record(&session, &address)
    );
    assert!(run.wait_text("[idle]", secs(5)), "{}", run.rendered());

    session.hc_json(&["send", &address, "working"]);
    assert!(wait_status(&session, &address, "working", secs(10)));
    assert!(run.wait_text("[working]", secs(5)), "{}", run.rendered());
    run.command(b'a');
    assert!(run.wait_text("agents", secs(5)), "{}", run.rendered());
    let sidebar = run.rendered();
    assert!(sidebar.contains("Build"), "{sidebar}");
    assert!(sidebar.contains("demo"), "{sidebar}");
    assert!(sidebar.contains("10%"), "{sidebar}");
    assert!(sidebar.contains("turn 0"), "{sidebar}");
    run.send(b"\x1b");
    assert!(
        run.wait_screen(secs(5), |screen| !screen.contains("agents")),
        "{}",
        run.rendered()
    );

    session.hc_json(&["send", &address, "done"]);
    assert!(wait_status(&session, &address, "done", secs(10)));
    assert!(wait_turn(&session, &address, 1, secs(10)));
    assert!(
        wait_label(&session, &address, "herdr.seen", "1", secs(10)),
        "a visible completed turn was not marked seen: {}",
        record(&session, &address)
    );
    assert!(run.wait_text("[idle]", secs(5)), "{}", run.rendered());

    // Unread is the one lower write, and the row shows done/unseen until the
    // pane is looked at again.
    run.command(b'a');
    assert!(run.wait_text("agents", secs(5)));
    run.send(b"u");
    assert!(
        wait_label(&session, &address, "herdr.seen", "0", secs(10)),
        "unread did not write turn-1: {}",
        record(&session, &address)
    );
    assert!(
        run.wait_screen(secs(5), |screen| {
            screen.contains("done") && screen.contains("unseen")
        }),
        "the unread row was not shown: {}",
        run.rendered()
    );

    // Pin and unpin write the two exact shared values.
    run.send(b"p");
    assert!(
        wait_label(&session, &address, "herdr.pinned", "1", secs(10)),
        "pin did not reach core: {}",
        record(&session, &address)
    );
    assert!(
        run.wait_screen(secs(5), |screen| screen.contains("* done")),
        "the pin marker is missing: {}",
        run.rendered()
    );

    // A second screen sees the same shared labels: pinned and still unseen.
    assert!(
        wait_until(secs(10), || {
            !doc_addresses(&session.doc("herdr-terminal")).is_empty()
        }),
        "the layout document was not written: {}",
        session.doc("herdr-terminal")
    );
    let mut observer = Run::spawn_as(&session, 100, 30, "observer");
    assert!(
        observer.wait_text("Agent Build", secs(10)),
        "the second screen did not restore the agent: {}",
        observer.rendered()
    );
    // The second screen is looking at the agent, so its own seen write raises
    // the shared label; the first screen's open sidebar applies that labels
    // event and stops showing the turn unseen.
    assert!(
        wait_label(&session, &address, "herdr.seen", "1", secs(10)),
        "the second screen did not mark its visible turn seen: {}",
        record(&session, &address)
    );
    assert!(
        run.wait_screen(secs(5), |screen| {
            screen.contains("idle") && !screen.contains("unseen")
        }),
        "another client's seen write did not reach the first screen: {}",
        run.rendered()
    );

    observer.command(b'a');
    assert!(
        observer.wait_screen(secs(5), |screen| {
            screen.contains("* idle") && screen.contains("pinned")
        }),
        "the second screen did not show the shared pin state: {}",
        observer.rendered()
    );

    run.send(b"p");
    assert!(
        wait_label(&session, &address, "herdr.pinned", "", secs(10)),
        "unpin did not remove the label: {}",
        record(&session, &address)
    );
    assert!(
        observer.wait_screen(secs(5), |screen| {
            screen.contains("idle") && !screen.contains("pinned")
        }),
        "the second screen did not apply another client's unpin: {}",
        observer.rendered()
    );
    observer.send(&[DETACH]);
    let status = observer.wait_exit(secs(5)).expect("observer detach");
    assert_eq!(status.code(), Some(0), "stderr={:?}", observer.errors());

    // Clearing the status leaves the agent reported and falls back to core's
    // terminal title; the unseen cleared state reads as done.
    session.hc_json(&["send", &address, "clear"]);
    assert!(
        wait_until(secs(10), || status_state(&session, &address).is_none()),
        "the status was not cleared: {}",
        record(&session, &address)
    );
    assert!(
        run.wait_text("Agent Build", secs(5)),
        "the row did not fall back to the terminal title: {}",
        run.rendered()
    );

    run.send(b"\x1b");
    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    cleanup_all(&session);
}

/// Across tabs, a hidden agent finishing is one notice and a BEL; jumping
/// focuses it, marks the turn seen, and peek draws its local VT plus facts.
#[test]
fn agents_jump_and_peek_across_tabs_end_to_end() {
    let Some(mut session) = session() else { return };
    session.start();
    let mut run = Run::spawn_with(&session, 100, 30, &["sh", "-c", AGENT]);
    let address = wait_addresses(&session, 1, secs(10))
        .into_iter()
        .next()
        .expect("agent terminal");
    assert!(wait_status(&session, &address, "idle", secs(10)));
    assert!(run.wait_text("AGENT-READY", secs(5)), "{}", run.rendered());

    // A new tab holds a plain shell; the agent is now off-screen.
    run.command(b'c');
    let all = wait_addresses(&session, 2, secs(10));
    assert!(all.contains(&address));
    assert!(
        run.wait_screen(secs(5), |screen| !screen.contains("AGENT-READY")),
        "the agent's tab stayed visible: {}",
        run.rendered()
    );

    session.hc_json(&["send", &address, "working"]);
    assert!(wait_status(&session, &address, "working", secs(10)));
    session.hc_json(&["send", &address, "done"]);
    assert!(wait_status(&session, &address, "done", secs(10)));
    assert!(wait_turn(&session, &address, 1, secs(10)));
    assert!(
        run.output.wait_count(7, 1, secs(10)),
        "the hidden finish did not beep: {}",
        run.rendered()
    );
    assert!(
        run.wait_text("finished", secs(5)),
        "the hidden finish had no notice: {}",
        run.rendered()
    );
    assert_eq!(
        label(&session, &address, "herdr.seen"),
        "0",
        "a hidden turn must stay unseen"
    );

    // `g` cycles to the only agent (from a non-agent) and marks it seen.
    run.command(b'g');
    assert!(
        run.wait_text("AGENT-READY", secs(5)),
        "jump did not focus the agent: {}",
        run.rendered()
    );
    assert!(
        wait_label(&session, &address, "herdr.seen", "1", secs(10)),
        "the jumped-to turn was not marked seen: {}",
        record(&session, &address)
    );

    // Peek shows the same local VT and core facts, and any key closes it.
    run.command(b'i');
    assert!(
        run.wait_text("peek · any key closes", secs(5)),
        "peek did not open: {}",
        run.rendered()
    );
    let peek = run.rendered();
    assert!(peek.contains("demo") && peek.contains("Build"), "{peek}");
    assert!(peek.contains("AGENT-READY"), "{peek}");
    run.send(b"x");
    assert!(
        run.wait_screen(secs(5), |screen| {
            !screen.contains("peek · any key closes")
        }),
        "peek did not close: {}",
        run.rendered()
    );

    run.send(&[DETACH]);
    let status = run.wait_exit(secs(5)).expect("detach");
    assert_eq!(status.code(), Some(0), "stderr={:?}", run.errors());
    cleanup_all(&session);
}
