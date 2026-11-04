//! Finding herdr-core (`hc`) on this machine.
//!
//! Where we look, in order: the configured path (`HERDR_TERMINAL_HC` or
//! `--hc`), this process's `PATH`, the person's login-shell `PATH` (a launcher
//! may not get it), then the usual install places. A candidate counts only if
//! `--version` prints `hc …`: some systems ship an unrelated `hc`.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// A full path to use instead of searching.
pub const CONFIGURED_ENV: &str = "HERDR_TERMINAL_HC";
/// Test hook: replaces every place we would look with these directories.
pub const DIRS_ENV: &str = "HERDR_CORE_DIRS";
/// How long a candidate's `--version` may take to exit and close stdout.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(2);
/// The most `--version` output accepted; a wedged or chatty binary is refused.
pub const VERSION_LIMIT: usize = 64 * 1024;

/// The first herdr-core found, or `None`.
pub fn find(configured: Option<&Path>) -> Option<PathBuf> {
    let home = home_dir();
    let configured = configured
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os(CONFIGURED_ENV).map(PathBuf::from))
        .map(|path| expand(&path, home.as_deref()));
    if let Some(found) = configured.and_then(|path| search([path])) {
        return Some(found);
    }

    if let Some(dirs) = std::env::var_os(DIRS_ENV) {
        return search(dirs_on(&dirs).flat_map(|dir| names(&dir)));
    }

    // The order is the brief's: this PATH, then the login shell's PATH, then
    // the usual install directories (a no-op when the login shell already
    // includes them).
    let path = std::env::var_os("PATH").unwrap_or_default();
    if let Some(found) = search(dirs_on(&path).flat_map(|dir| names(&dir))) {
        return Some(found);
    }
    if let Some(found) =
        login_shell_path().and_then(|path| search(dirs_on(&path).flat_map(|dir| names(&dir))))
    {
        return Some(found);
    }
    search(
        usual_dirs(home.as_deref())
            .into_iter()
            .flat_map(|dir| names(&dir)),
    )
}

/// The first candidate that is a file whose `--version` says `hc …`.
pub fn search(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .find(|bin| bin.is_file() && version(bin).is_some())
}

/// `hc X.Y.Z` when this binary is herdr-core and runs. The probe gives a
/// candidate [`VERSION_TIMEOUT`] to exit and close stdout: a wedged binary
/// must not stall discovery.
pub fn version(bin: &Path) -> Option<String> {
    version_within(bin, VERSION_TIMEOUT)
}

/// A few short retries cover a busy executable: another thread's fork can
/// briefly inherit a write handle to a script this process just wrote, and
/// the kernel reports `ETXTBSY` until that child execs. A binary being
/// replaced by an installer can be busy for the same reason.
const SPAWN_ATTEMPTS: u32 = 8;
const SPAWN_RETRY: Duration = Duration::from_millis(20);

fn version_within(bin: &Path, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    let mut child = spawn_probe(bin, deadline)?;
    let (status, stdout) = collect_version(&mut child, deadline)?;
    let text = String::from_utf8_lossy(&stdout).trim().to_owned();
    (status.success() && text.starts_with("hc ")).then_some(text)
}

/// Starts the version probe, retrying a busy executable while the probe's
/// own deadline allows it.
fn spawn_probe(bin: &Path, deadline: Instant) -> Option<Child> {
    let mut attempts = 1;
    loop {
        let mut command = Command::new(bin);
        command
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        match command.spawn() {
            Ok(child) => return Some(child),
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && attempts < SPAWN_ATTEMPTS
                    && Instant::now() + SPAWN_RETRY < deadline =>
            {
                attempts += 1;
                std::thread::sleep(SPAWN_RETRY);
            }
            Err(_) => return None,
        }
    }
}

/// Ends the probe: kill, then reap so no zombie is left.
fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// The child's exit status and stdout, both bounded by `deadline` and
/// [`VERSION_LIMIT`]. The single deadline covers exit and stdout collection.
#[cfg(unix)]
fn collect_version(child: &mut Child, deadline: Instant) -> Option<(ExitStatus, Vec<u8>)> {
    use std::os::fd::AsRawFd;

    let Some(mut stdout) = child.stdout.take() else {
        // No pipe to drain: the probe cannot succeed, but the child may still
        // be running, so end it rather than leave an unreaped process.
        stop(child);
        return None;
    };
    if set_nonblocking(stdout.as_raw_fd()).is_err() {
        stop(child);
        return None;
    }
    let mut out = Vec::new();
    let mut status = None;
    let mut eof = false;
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(done) => status = done,
                Err(_) => {
                    stop(child);
                    return None;
                }
            }
        }
        if !eof {
            let mut chunk = [0u8; 8192];
            match stdout.read(&mut chunk) {
                Ok(0) => eof = true,
                Ok(read) => {
                    out.extend_from_slice(&chunk[..read]);
                    if out.len() > VERSION_LIMIT {
                        stop(child);
                        return None;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => {
                    stop(child);
                    return None;
                }
            }
        }
        if status.is_some() && eof {
            return Some((status?, out));
        }
        if Instant::now() >= deadline {
            stop(child);
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn set_nonblocking(fd: std::os::fd::RawFd) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// The child's exit status and stdout, both bounded by `deadline` and
/// [`VERSION_LIMIT`]. A reader thread collects stdout so the wait cannot block
/// on a pipe. The thread is never joined: after a successful exit a descendant
/// can still hold stdout open, and joining would wait past the deadline. The
/// probe returns by the deadline anyway; the reader is left to finish or die
/// with the pipe (a bounded, best-effort cleanup). As soon as the received
/// buffer exceeds the cap the caller kills and returns, without waiting for a
/// chatty child to exit.
#[cfg(not(unix))]
fn collect_version(child: &mut Child, deadline: Instant) -> Option<(ExitStatus, Vec<u8>)> {
    use std::sync::mpsc;

    let Some(mut stdout) = child.stdout.take() else {
        stop(child);
        return None;
    };
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    out.extend_from_slice(&chunk[..read]);
                    if out.len() > VERSION_LIMIT {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = sender.send(out);
    });

    let mut status = None;
    let mut bytes = None;
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(done) => status = done,
                Err(_) => {
                    stop(child);
                    return None;
                }
            }
        }
        if bytes.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(left.min(Duration::from_millis(10))) {
                Ok(out) => bytes = Some(out),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => bytes = Some(Vec::new()),
            }
        }
        // The cap ends the probe even while the child is still running.
        if bytes.as_ref().is_some_and(|out| out.len() > VERSION_LIMIT) {
            stop(child);
            return None;
        }
        if status.is_some() && bytes.is_some() {
            return Some((status?, bytes?));
        }
        if Instant::now() >= deadline {
            stop(child);
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn expand(path: &Path, home: Option<&Path>) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    let Some(home) = home else {
        return path.to_path_buf();
    };
    if text == "~" {
        return home.to_path_buf();
    }
    match text.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => path.to_path_buf(),
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty())
}

fn dirs_on(path: &OsStr) -> impl Iterator<Item = PathBuf> + '_ {
    std::env::split_paths(path)
}

fn names(dir: &Path) -> [PathBuf; 2] {
    let exe = std::env::consts::EXE_SUFFIX;
    [
        dir.join(format!("herdr-core{exe}")),
        dir.join(format!("hc{exe}")),
    ]
}

/// Where herdr-core gets installed: its own installer and other common homes.
fn usual_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    if cfg!(windows) {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let mut dirs = Vec::new();
        if let Some(local) = local {
            dirs.push(local.join("Programs").join("herdr-core"));
        }
        if let Some(home) = home {
            dirs.push(home.join(".cargo").join("bin"));
        }
        return dirs;
    }
    let mut dirs = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/run/current-system/sw/bin"),
    ];
    if let Some(home) = home {
        dirs.insert(0, home.join(".local").join("bin"));
        dirs.push(home.join(".local").join("share").join("mise").join("shims"));
        dirs.push(home.join(".nix-profile").join("bin"));
        dirs.push(home.join(".cargo").join("bin"));
    }
    dirs
}

/// The `PATH` the person's login shell sets up. Waiting for the shell to exit
/// is bounded to 3 s, but collecting its stdout after that exit can still wait
/// for EOF when a descendant holds the pipe: discovery as a whole is not
/// deadline-bounded, only the `--version` probe is.
fn login_shell_path() -> Option<std::ffi::OsString> {
    if cfg!(windows) {
        return None;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let mut child = Command::new(shell)
        .args(["-lc", "printf %s \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().ok()?.is_some() {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            // Reap the killed child so discovery leaves no zombie behind.
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // The exit above does not close stdout by itself: a descendant the shell
    // started can keep it open, and this read can wait for EOF. Discovery as a
    // whole is therefore not deadline-bounded; only the `--version` probe is.
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    Some(out.into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = PathBuf::from(format!(
                "/var/tmp/ht-discovery-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Scratch(dir)
        }

        fn script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("script");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_a_binary_whose_version_says_hc_counts() {
        let scratch = Scratch::new("identity");
        let good = scratch.script("good", "echo 'hc 9.9.9'");
        let other = scratch.script("other", "echo 'hc-tool 1.0'");
        let broken = scratch.script("broken", "echo 'hc 9.9.9'; exit 1");
        let empty = scratch.script("empty", "exit 0");

        assert_eq!(search([good.clone()]), Some(good));
        assert_eq!(search([other]), None, "an unrelated hc is not the core");
        assert_eq!(
            search([broken]),
            None,
            "a failing --version is not the core"
        );
        assert_eq!(search([empty]), None);
        assert_eq!(search([scratch.0.join("missing")]), None);
    }

    #[test]
    fn both_install_names_are_tried_in_order() {
        let scratch = Scratch::new("names");
        let herdr_core = scratch.script("herdr-core", "echo 'hc 9.9.9'");
        assert_eq!(search(names(&scratch.0)), Some(herdr_core));

        let other = Scratch::new("hc-name");
        let hc = other.script("hc", "echo 'hc 9.9.9'");
        assert_eq!(search(names(&other.0)), Some(hc));
    }

    #[test]
    fn a_busy_candidate_is_retried() {
        let scratch = Scratch::new("busy");
        let script = scratch.script("busy", "echo 'hc 9.9.9'");
        // Linux refuses to exec a file held open for writing (ETXTBSY) until
        // the handle is dropped, like a forked child that briefly inherited
        // another thread's handle; macOS permits it, so there the retry path
        // is covered by inspection rather than by this test.
        let handle = std::fs::OpenOptions::new()
            .write(true)
            .open(&script)
            .expect("hold open");
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(25));
            drop(handle);
        });
        assert_eq!(version(&script), Some("hc 9.9.9".to_owned()));
        release.join().expect("release thread");
    }

    #[test]
    fn a_wedged_version_probe_returns_by_its_deadline() {
        let scratch = Scratch::new("version-slow");
        // `exec` makes the sleep the probe's direct child, so kill+wait reaps
        // it instead of leaving a forked `sleep 5` behind.
        let slow = scratch.script("slow", "exec sleep 5");
        let started = Instant::now();
        assert_eq!(version_within(&slow, Duration::from_millis(150)), None);
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
    }

    #[test]
    fn a_version_probe_refuses_output_over_the_limit() {
        let scratch = Scratch::new("version-big");
        let big = scratch.script(
            "big",
            "printf 'hc '; head -c 131072 /dev/zero | tr '\\0' 'x'; echo",
        );
        let started = Instant::now();
        // The cap must end the probe well before this generous deadline: a
        // deadline path (about 3 s) would fail the bound below.
        assert_eq!(version_within(&big, Duration::from_secs(3)), None);
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
    }

    /// The pid a descendant recorded, waiting up to 2 s for the shell to write
    /// it. No pid is not a failure: the shell may have been killed first.
    fn recorded_pid(path: &Path) -> Option<i32> {
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until {
            if let Ok(text) = std::fs::read_to_string(path)
                && let Ok(pid) = text.trim().parse::<i32>()
            {
                return Some(pid);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn a_version_probe_does_not_wait_for_a_descendant_holding_stdout() {
        let scratch = Scratch::new("version-descendant");
        let pidfile = scratch.0.join("descendant.pid");
        let script = scratch.script(
            "parent",
            &format!(
                // The descendant inherits the version pipe and keeps it open
                // after the shell exits. The pid is recorded when the shell
                // gets that far; the probe may kill it first, so the test
                // never depends on the file (the 5 s sleep bounds any
                // residue).
                "sleep 5 &\necho $! > '{}'\necho 'hc 9.9.9'",
                pidfile.display()
            ),
        );
        let started = Instant::now();
        let result = version_within(&script, Duration::from_millis(500));
        let elapsed = started.elapsed();
        // Clean up before asserting so no failure can leak the descendant.
        // Best effort only: a missing pid means the shell was killed before
        // it recorded the pid, and the descendant ends by itself within 5 s.
        if let Some(pid) = recorded_pid(&pidfile) {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        assert_eq!(result, None, "the probe waited for the descendant");
        // Waiting for EOF would take about 5 s; the 500 ms deadline is what
        // makes the probe return early.
        assert!(elapsed < Duration::from_secs(3), "took {elapsed:?}");
    }
}
