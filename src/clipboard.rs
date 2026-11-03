//! Clipboard output without a new dependency.
//!
//! A copy can go to the outer terminal with OSC 52, to a native helper
//! (`pbcopy` on macOS, `wl-copy`/`xclip` on Linux), or both. The OSC 52 bytes
//! are built locally with a small base64 encoder; the native helper is a
//! short-lived process, so a copy never blocks longer than the helper takes.

/// The mode's OSC 52 sequence: `ESC ] 52 ; c ; <base64> BEL`.
///
/// The outer terminal owns the clipboard; the program reading our output is
/// the outer terminal itself, not a pane's program.
pub fn osc52(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 16);
    out.extend_from_slice(b"\x1b]52;c;");
    out.extend_from_slice(base64(text.as_bytes()).as_bytes());
    out.push(0x07);
    out
}

/// Standard base64 with padding, encoded locally.
pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let value = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(TABLE[(value >> 18) as usize & 0x3f] as char);
        out.push(TABLE[(value >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(value >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[value as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

/// The native helpers a copy tries, in order. Empty when the platform has no
/// practical helper; the caller then falls back to OSC 52.
pub fn native_candidates() -> &'static [(&'static str, &'static [&'static str])] {
    if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(target_os = "linux") {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])]
    } else {
        &[]
    }
}

/// Copies through the first available native helper. The error names what was
/// tried so the caller can report the fallback honestly.
pub fn native_copy(text: &str) -> Result<(), String> {
    let candidates = native_candidates();
    if candidates.is_empty() {
        return Err("no native clipboard helper on this platform".into());
    }
    let mut attempts = Vec::new();
    for (program, args) in candidates {
        match try_native(program, args, text) {
            Ok(()) => return Ok(()),
            Err(error) => attempts.push(format!("{program}: {error}")),
        }
    }
    Err(attempts.join("; "))
}

#[cfg(unix)]
fn try_native(program: &str, args: &[&str], text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let write = child
        .stdin
        .take()
        .ok_or_else(|| "no stdin".to_owned())
        .and_then(|mut stdin| stdin.write_all(text.as_bytes()).map_err(|e| e.to_string()));
    if let Err(error) = write {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    match child.wait() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("exited with {status}")),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(not(unix))]
fn try_native(_program: &str, _args: &[&str], _text: &str) -> Result<(), String> {
    Err("native clipboard helpers are not available on this platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(b"\xff\x00"), "/wA=");
    }

    #[test]
    fn osc52_wraps_the_text_in_a_clipboard_sequence() {
        assert_eq!(osc52("hi"), b"\x1b]52;c;aGk=\x07");
        assert_eq!(osc52(""), b"\x1b]52;c;\x07");
        let unicode = osc52("héllo");
        assert!(unicode.starts_with(b"\x1b]52;c;"));
        assert!(unicode.ends_with(b"\x07"));
        assert_eq!(
            &unicode[7..unicode.len() - 1],
            base64("héllo".as_bytes()).as_bytes()
        );
    }

    #[test]
    fn native_candidates_fit_the_platform() {
        if cfg!(target_os = "macos") {
            assert_eq!(native_candidates(), &[("pbcopy", &[][..])]);
        } else if cfg!(target_os = "linux") {
            assert_eq!(native_candidates()[0].0, "wl-copy");
            assert!(native_candidates().iter().any(|(name, _)| *name == "xclip"));
        } else {
            assert!(native_candidates().is_empty());
        }
    }
}
