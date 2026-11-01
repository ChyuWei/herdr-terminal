#![cfg(unix)]
//! End-to-end M0 tests: a real core in an isolated session, the real `ht
//! probe`, real PTYs on the program side, and (first test) a real local PTY
//! for resize. They skip when herdr-core is not installed.

mod common;

use common::pty::Pty;
use common::{Capture, HT, Probe, Session, secs, wait_until};
use std::io::Write;
use std::process::Stdio;
use std::time::Instant;

/// Prints a marker, waits for input, then reports the terminal size forever.
const SHELL: &str = "printf 'READY\\n'; read x; printf 'GOT:%s\\n' \"$x\"; \
                     while :; do stty size; sleep 0.1; done";

/// Prints a marker, waits for input, then more output than the 256 KiB
/// starting credit, and exits. Without CREDIT frames this stalls.
const BURST: &str = r#"printf 'READY\n'; read x; stty size; awk 'BEGIN { for (i = 0; i < 300000; i++) printf "a" }'; printf '\nDONE\n'; exit 0"#;

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

#[test]
fn probe_carries_input_resize_and_a_clean_detach() {
    let Some(mut session) = session() else { return };
    session.start();

    let pty = Pty::open(40, 10);
    let (stdin, stdout) = pty.stdio();
    let mut command = session.command(HT);
    command
        .args(["probe", "--"])
        .args(["sh", "-c", SHELL])
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn ht probe");
    let output = Capture::spawn(pty.reader());
    let errors = Capture::spawn(child.stderr.take().expect("stderr"));
    let mut input = pty.writer();
    let mut probe = Probe::new(child);

    assert!(
        output.wait_for(b"READY", secs(10)),
        "no READY from the shell; output={:?} stderr={:?}",
        output.text(),
        errors.text()
    );
    assert!(
        !pty.is_cooked(),
        "the probe did not put its terminal into raw mode"
    );

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
        "the probe's terminal is not labelled: {:?}",
        session.terminals()
    );

    input.write_all(b"hi\n").expect("send input");
    input.flush().expect("flush input");
    assert!(
        output.wait_for(b"GOT:hi", secs(5)),
        "input did not reach the program; output={:?}",
        output.text()
    );
    assert!(
        output.wait_for(b"10 40", secs(5)),
        "the attach size was not 40x10; output={:?}",
        output.text()
    );

    pty.resize(50, 12);
    assert!(
        output.wait_for(b"12 50", secs(5)),
        "resize was not forwarded; output={:?}",
        output.text()
    );

    // Ctrl-C quits the probe: it detaches and ends the shell it started.
    input.write_all(&[0x03]).expect("send Ctrl-C");
    input.flush().expect("flush Ctrl-C");
    let status = probe
        .wait_timeout(secs(5))
        .expect("probe exits after Ctrl-C");
    assert!(
        status.success(),
        "probe exit {status}; stderr={:?}",
        errors.text()
    );
    assert!(
        pty.is_cooked(),
        "the probe did not restore the terminal after raw mode"
    );

    let gone = wait_until(secs(5), || session.terminals().is_empty());
    assert!(
        gone,
        "the probe left its terminal behind: {:?}",
        session.terminals()
    );
}

#[test]
fn probe_sends_credit_and_exits_with_the_program() {
    let Some(mut session) = session() else { return };
    session.start();

    let mut command = session.command(HT);
    command
        .args(["probe", "--cols", "100", "--rows", "30", "--"])
        .args(["sh", "-c", BURST])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn ht probe");
    let output = Capture::spawn(child.stdout.take().expect("stdout"));
    let errors = Capture::spawn(child.stderr.take().expect("stderr"));
    let mut probe = Probe::new(child);

    assert!(
        output.wait_for(b"READY", secs(10)),
        "no READY from the program; output={:?} stderr={:?}",
        output.text(),
        errors.text()
    );

    probe.write(b"go\n");
    let started = Instant::now();
    let status = probe
        .wait_timeout(secs(20))
        .expect("probe exits when the program does");
    assert!(
        status.success(),
        "probe exit {status}; stderr={:?}",
        errors.text()
    );
    assert!(
        output.wait_for(b"DONE", secs(5)),
        "the program's last output was lost; output len={}",
        output.bytes().len()
    );

    let bytes = output.bytes().len();
    assert!(
        output.wait_count(b'a', 300_000, secs(5)) && output.count(b'a') >= 300_000,
        "only {} of 300000 burst bytes arrived after {} ms; credits stalled (total {} bytes)",
        output.count(b'a'),
        started.elapsed().as_millis(),
        bytes
    );
    assert!(
        output.contains(b"30 100"),
        "program saw the wrong size; output={:?}",
        output.text()
    );

    let gone = wait_until(secs(5), || session.terminals().is_empty());
    assert!(
        gone,
        "the probe left its terminal behind: {:?}",
        session.terminals()
    );
}

/// A program that exits at once races `terminal.attach`. Repeated runs exercise
/// both sides: attach in time (live stream) and attach after the exit (recover
/// the screen and status with `terminal.get`/`terminal.read`).
#[test]
fn probe_recovers_a_program_that_exits_at_once() {
    let Some(mut session) = session() else { return };
    session.start();

    for attempt in 0..5 {
        let mut command = session.command(HT);
        command
            .args(["probe", "--", "sh", "-c", "printf 'FAST\\n'"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("spawn ht probe");
        let output = Capture::spawn(child.stdout.take().expect("stdout"));
        let errors = Capture::spawn(child.stderr.take().expect("stderr"));
        let mut probe = Probe::new(child);

        let status = probe.wait_timeout(secs(10)).expect("probe exits");
        assert_eq!(
            status.code(),
            Some(0),
            "attempt {attempt}: probe exit {status}; stderr={:?}",
            errors.text()
        );
        assert!(
            output.wait_for(b"FAST", secs(2)),
            "attempt {attempt}: the program's output was lost; output={:?}",
            output.text()
        );
    }

    let gone = wait_until(secs(5), || session.terminals().is_empty());
    assert!(
        gone,
        "the probe left finished terminals behind: {:?}",
        session.terminals()
    );
}
