//! Finding herdr-core (`hc`) on this machine.
//!
//! Where we look, in order: the configured path (`HERDR_TERMINAL_HC` or
//! `--hc`), this process's `PATH`, the person's login-shell `PATH` (a launcher
//! may not get it), then the usual install places. A candidate counts only if
//! `--version` prints `hc …`: some systems ship an unrelated `hc`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A full path to use instead of searching.
pub const CONFIGURED_ENV: &str = "HERDR_TERMINAL_HC";
/// Test hook: replaces every place we would look with these directories.
pub const DIRS_ENV: &str = "HERDR_CORE_DIRS";

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

/// `hc X.Y.Z` when this binary is herdr-core and runs. The probe has no
/// deadline yet (an M2 gap): a wedged binary can stall discovery.
pub fn version(bin: &Path) -> Option<String> {
    let out = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && text.starts_with("hc ")).then_some(text)
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

/// The `PATH` the person's login shell sets up, or `None` if it doesn't
/// answer quickly. Slow profiles must not hold up the client.
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
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
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
}
