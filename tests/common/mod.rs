#![allow(dead_code)]
//! Shared harness for the M0 integration tests: an isolated core session, a
//! captured child, and (unix) a real PTY for resize.

use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The `ht` built for this test run.
pub const HT: &str = env!("CARGO_BIN_EXE_ht");

pub fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// An isolated core: its own HC_SESSION and scratch XDG dirs under /var/tmp.
pub struct Session {
    root: PathBuf,
    hc: PathBuf,
    started: bool,
}

impl Session {
    /// `None` when herdr-core is not installed; the caller skips the test.
    pub fn new() -> Option<Session> {
        let hc = herdr_terminal::core::discovery::find(None)?;
        static N: AtomicU32 = AtomicU32::new(0);
        let root = PathBuf::from(format!(
            "/var/tmp/ht-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["run", "state", "config"] {
            std::fs::create_dir_all(root.join(dir)).expect("scratch dir");
        }
        Some(Session {
            root,
            hc,
            started: false,
        })
    }

    fn envs(&self) -> Vec<(String, String)> {
        let root = self.root.display().to_string();
        vec![
            ("PATH".into(), std::env::var("PATH").unwrap_or_default()),
            ("HOME".into(), root.clone()),
            ("HC_SESSION".into(), "test".into()),
            ("HC_SETTLE_MS".into(), "0".into()),
            ("XDG_RUNTIME_DIR".into(), format!("{root}/run")),
            ("XDG_STATE_HOME".into(), format!("{root}/state")),
            ("XDG_CONFIG_HOME".into(), format!("{root}/config")),
            ("HERDR_TERMINAL_HC".into(), self.hc.display().to_string()),
        ]
    }

    /// A command in this session's environment, with nothing from the real one.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        command.env_clear().envs(self.envs());
        command
    }

    pub fn start(&mut self) {
        let out = self
            .command(&self.hc)
            .args(["server", "start"])
            .stdin(Stdio::null())
            .output()
            .expect("run hc server start");
        assert!(
            out.status.success(),
            "hc server start failed: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        self.started = true;
    }

    pub fn hc_json(&self, args: &[&str]) -> serde_json::Value {
        let out = self
            .command(&self.hc)
            .arg("--json")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run hc");
        assert!(
            out.status.success(),
            "hc {args:?} failed: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("json from hc")
    }

    pub fn terminals(&self) -> Vec<serde_json::Value> {
        self.hc_json(&["ls"])["terminals"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    pub fn stop(&mut self) {
        if self.started {
            let _ = self
                .command(&self.hc)
                .args(["server", "stop"])
                .stdin(Stdio::null())
                .output();
            self.started = false;
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Reads a child stream into a shared buffer, on its own thread.
pub struct Capture {
    buf: Arc<Mutex<Vec<u8>>>,
    done: Arc<AtomicBool>,
}

impl Capture {
    pub fn spawn(mut reader: impl Read + Send + 'static) -> Capture {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        let thread_buf = buf.clone();
        let thread_done = done.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => lock(&thread_buf).extend_from_slice(&chunk[..n]),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            thread_done.store(true, Ordering::Relaxed);
        });
        Capture { buf, done }
    }

    pub fn bytes(&self) -> Vec<u8> {
        lock(&self.buf).clone()
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes()).into_owned()
    }

    pub fn contains(&self, needle: &[u8]) -> bool {
        contains(&lock(&self.buf), needle)
    }

    pub fn count(&self, byte: u8) -> usize {
        lock(&self.buf).iter().filter(|b| **b == byte).count()
    }

    /// Waits for `needle`; returns whether it arrived within `timeout`.
    pub fn wait_for(&self, needle: &[u8], timeout: Duration) -> bool {
        wait_until(timeout, || self.contains(needle))
    }

    /// Waits for at least `minimum` copies of `byte` in the output.
    pub fn wait_count(&self, byte: u8, minimum: usize, timeout: Duration) -> bool {
        wait_until(timeout, || self.count(byte) >= minimum)
    }

    pub fn finished(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty()
        || haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

pub fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if done() {
            return true;
        }
        if start.elapsed() >= timeout {
            return done();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A child that is killed if the test panics before reaping it.
pub struct Probe {
    child: Child,
    exited: bool,
}

impl Probe {
    pub fn new(child: Child) -> Probe {
        Probe {
            child,
            exited: false,
        }
    }

    pub fn stdin(&mut self) -> &mut ChildStdin {
        self.child.stdin.as_mut().expect("piped stdin")
    }

    pub fn write(&mut self, bytes: &[u8]) {
        self.stdin().write_all(bytes).expect("write to probe");
        self.stdin().flush().expect("flush to probe");
    }

    pub fn wait_timeout(&mut self, timeout: Duration) -> Option<ExitStatus> {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("wait for probe") {
                self.exited = true;
                return Some(status);
            }
            if start.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        if !self.exited {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(unix)]
pub mod pty {
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::process::Stdio;

    /// A pseudo-terminal pair: the test writes and reads the master; the
    /// probe sees the slave as its stdin/stdout.
    pub struct Pty {
        master: File,
        slave: File,
    }

    impl Pty {
        pub fn open(cols: u16, rows: u16) -> Pty {
            let mut size = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            let mut master = -1;
            let mut slave = -1;
            // SAFETY: openpty fills both descriptors on success; the size is
            // plain data and the optional name/termios pointers are null.
            let rc = unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut size,
                )
            };
            assert_eq!(rc, 0, "openpty: {}", io::Error::last_os_error());
            // SAFETY: openpty returned two fresh, owned descriptors.
            unsafe {
                Pty {
                    master: File::from_raw_fd(master),
                    slave: File::from_raw_fd(slave),
                }
            }
        }

        /// stdin and stdout for the child, as duplicates of the slave.
        pub fn stdio(&self) -> (Stdio, Stdio) {
            (
                Stdio::from(self.slave.try_clone().expect("dup slave")),
                Stdio::from(self.slave.try_clone().expect("dup slave")),
            )
        }

        pub fn reader(&self) -> File {
            self.master.try_clone().expect("dup master")
        }

        pub fn writer(&self) -> File {
            self.master.try_clone().expect("dup master")
        }

        pub fn resize(&self, cols: u16, rows: u16) {
            let size = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            // SAFETY: the master is a valid pty fd and size is a winsize.
            let rc = unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) };
            assert_eq!(rc, 0, "TIOCSWINSZ: {}", io::Error::last_os_error());
        }

        /// Whether the slave still has the normal line discipline (canonical
        /// input and signals). Raw mode clears both; a child that restores
        /// the terminal puts them back.
        pub fn is_cooked(&self) -> bool {
            let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
            // SAFETY: the slave is a valid fd; tcgetattr fills termios.
            if unsafe { libc::tcgetattr(self.slave.as_raw_fd(), termios.as_mut_ptr()) } != 0 {
                return false;
            }
            // SAFETY: tcgetattr succeeded, so termios is initialized.
            let termios = unsafe { termios.assume_init() };
            termios.c_lflag & (libc::ICANON | libc::ISIG) == (libc::ICANON | libc::ISIG)
        }
    }
}
