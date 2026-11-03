#![cfg(unix)]
//! End-to-end observer attach against a real, isolated core.
//!
//! A normal viewer owns the size; an observer asks for resize notifications.
//! The observer's stream must carry RESIZE before each paint: the first grid
//! before the first paint, and every later change before its replacement
//! paint. This is the ordering the peek path will rely on.
//!
//! The test skips when herdr-core is not installed (unless
//! `HERDR_TERMINAL_REQUIRE_CORE` is set) and when the installed core does not
//! advertise the optional feature.

use herdr_terminal::core::{CoreClient, RunOptions, StreamItem};
use herdr_terminal::vt::Vt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);

/// An isolated core: its own session, HOME and XDG roots under /var/tmp, and
/// a wrapper standing in for `hc` so every child inherits that environment.
struct Isolated {
    root: PathBuf,
    wrapper: PathBuf,
    started: bool,
}

impl Isolated {
    fn new(hc: &Path) -> Isolated {
        let root = PathBuf::from(format!("/var/tmp/ht-observer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for part in ["home", "run", "state", "config"] {
            std::fs::create_dir_all(root.join(part)).expect("scratch dir");
        }
        let wrapper = root.join("hc");
        let script = format!(
            "#!/bin/sh\n\
             HOME='{root}/home'\n\
             HC_SESSION='ht-observer-{pid}'\n\
             HC_SETTLE_MS=0\n\
             XDG_RUNTIME_DIR='{root}/run'\n\
             XDG_STATE_HOME='{root}/state'\n\
             XDG_CONFIG_HOME='{root}/config'\n\
             export HOME HC_SESSION HC_SETTLE_MS XDG_RUNTIME_DIR XDG_STATE_HOME XDG_CONFIG_HOME\n\
             exec '{}' \"$@\"\n",
            hc.display(),
            root = root.display(),
            pid = std::process::id(),
        );
        std::fs::write(&wrapper, script).expect("write hc wrapper");
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))
            .expect("make hc wrapper executable");
        Isolated {
            root,
            wrapper,
            started: false,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.wrapper);
        command.stdin(Stdio::null()).stderr(Stdio::piped());
        command
    }

    fn start(&mut self) {
        let out = self.command().args(["server", "start"]).output();
        let out = out.expect("run hc server start");
        assert!(
            out.status.success(),
            "hc server start failed: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        self.started = true;
    }

    fn stop(&mut self) {
        if self.started {
            let _ = self
                .command()
                .args(["server", "stop"])
                .stdout(Stdio::null())
                .output();
            self.started = false;
        }
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn find_core() -> Option<PathBuf> {
    let core = herdr_terminal::core::discovery::find(None);
    if core.is_none() {
        if std::env::var_os("HERDR_TERMINAL_REQUIRE_CORE").is_some() {
            panic!("herdr-core (hc) not found, and HERDR_TERMINAL_REQUIRE_CORE is set");
        }
        eprintln!(
            "skipping: herdr-core (hc) not found (set HERDR_TERMINAL_REQUIRE_CORE=1 to fail)"
        );
    }
    core
}

fn next_item(observer: &herdr_terminal::core::Attached) -> StreamItem {
    observer
        .stream
        .recv_timeout(TIMEOUT)
        .expect("an observer stream item")
}

/// Removes the test terminal even if an assertion fails first.
struct Terminal<'a> {
    client: &'a CoreClient,
    address: String,
}

impl Drop for Terminal<'_> {
    fn drop(&mut self) {
        let _ = self.client.kill(&self.address);
        let _ = self.client.rm(&self.address);
    }
}

#[test]
fn an_observer_receives_the_grid_before_each_paint() {
    let Some(core) = find_core() else {
        return;
    };
    let mut isolated = Isolated::new(&core);
    isolated.start();
    let client = CoreClient::connect(Some(&isolated.wrapper)).expect("connect to the test core");
    if !client.hello().supports("resize_notifications") {
        eprintln!(
            "skipping: {} does not advertise resize_notifications",
            client.hello().version
        );
        return;
    }

    let record = client
        .run(&RunOptions {
            argv: vec![
                "sh".into(),
                "-c".into(),
                "printf 'OBSERVE-HELLO\\n'; sleep 30".into(),
            ],
            rm: true,
            ..RunOptions::default()
        })
        .expect("start a terminal");
    let address = record.address;
    let _terminal = Terminal {
        client: &client,
        address: address.clone(),
    };

    // The normal viewer owns the grid; the observer only watches it.
    let normal = client.attach(&address, 80, 20, 0).expect("normal attach");
    assert!(!normal.resize_notifications);

    let observer = client
        .attach_observer(&address, 80, 20, 0)
        .expect("observer attach");
    assert!(
        observer.resize_notifications,
        "the installed core confirms resize notifications"
    );

    let mut vt = Vt::new(80, 20).expect("local VT");

    // The initial grid precedes the first paint.
    let StreamItem::Resize { cols, rows } = next_item(&observer) else {
        panic!("the first observer item is the terminal grid");
    };
    assert_eq!((cols, rows), (80, 20), "the initial grid is the viewer's");
    vt.resize(cols, rows).expect("resize the local VT");
    let StreamItem::Data(paint) = next_item(&observer) else {
        panic!("the first paint follows the initial grid");
    };
    vt.feed(&paint);
    client
        .credit(observer.channel, paint.len())
        .expect("credit the first paint");
    // The program may have printed before or after the paint was taken; live
    // output is DATA either way.
    let deadline = Instant::now() + TIMEOUT;
    while !vt
        .snapshot()
        .expect("snapshot")
        .lines()
        .join("\n")
        .contains("OBSERVE-HELLO")
    {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "the program's first output arrived");
        match observer.stream.recv_timeout(remaining) {
            Ok(StreamItem::Data(bytes)) => {
                vt.feed(&bytes);
                client
                    .credit(observer.channel, bytes.len())
                    .expect("credit live output");
            }
            Ok(other) => panic!("unexpected item before the first output: {other:?}"),
            Err(error) => panic!("no output from the program: {error}"),
        }
    }

    // The normal viewer changes size. The observer sees the new grid before
    // the replacement paint that uses it.
    client
        .resize(normal.channel, 40, 10)
        .expect("the normal viewer resizes");
    loop {
        match next_item(&observer) {
            StreamItem::Resize { cols, rows } => {
                vt.resize(cols, rows).expect("resize the local VT");
                if (cols, rows) != (40, 10) {
                    continue;
                }
                let StreamItem::Data(paint) = next_item(&observer) else {
                    panic!("the replacement paint follows its grid");
                };
                vt.feed(&paint);
                client
                    .credit(observer.channel, paint.len())
                    .expect("credit the replacement paint");
                break;
            }
            StreamItem::Data(paint) => {
                // Output produced before the resize is still worth crediting.
                vt.feed(&paint);
                client
                    .credit(observer.channel, paint.len())
                    .expect("credit live output");
            }
            StreamItem::Closed(reason) => panic!("the terminal ended: {reason:?}"),
            StreamItem::Lost => panic!("the observer stream was lost"),
        }
    }
    let screen = vt.snapshot().expect("snapshot after the resize");
    assert_eq!(
        (screen.cols, screen.rows),
        (40, 10),
        "the observer applied the new grid before its paint"
    );
    assert!(
        screen.lines().join("\n").contains("OBSERVE-HELLO"),
        "the replacement paint was laid out for the new grid"
    );

    // This test never calls an input method on the observer. The fake-core
    // tests inspect the wire and assert that an observer sends only CREDIT.
    drop(normal);
    drop(observer);
}
