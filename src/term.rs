//! The probe's local terminal: raw mode and its window size.
//!
//! M0 copies raw bytes; it does not encode keys. Raw mode makes Ctrl-C and
//! Ctrl-Z bytes for the probe to handle itself, and is restored on drop.
//! Platforms without the unix termios calls fall back to no raw mode and no
//! size detection; the probe then uses 80x24 unless told otherwise.

#[cfg(unix)]
mod platform {
    use std::io;
    use std::mem::MaybeUninit;

    pub struct RawMode {
        original: libc::termios,
    }

    impl RawMode {
        /// Puts stdin in raw mode when it is a terminal.
        pub fn enter() -> io::Result<Option<RawMode>> {
            use std::io::IsTerminal;
            if !io::stdin().is_terminal() {
                return Ok(None);
            }
            // SAFETY: termios is initialized by tcgetattr before we read it.
            let mut termios = MaybeUninit::<libc::termios>::uninit();
            // SAFETY: fd 0 is our stdin and the buffer is valid for one termios.
            if unsafe { libc::tcgetattr(libc::STDIN_FILENO, termios.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: tcgetattr succeeded, so the value is initialized.
            let original = unsafe { termios.assume_init() };
            let mut raw = original;
            // SAFETY: `raw` is an initialized termios for this terminal.
            unsafe { libc::cfmakeraw(&mut raw) };
            // SAFETY: same file descriptor and a valid termios.
            if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Some(RawMode { original }))
        }
    }

    impl Drop for RawMode {
        fn drop(&mut self) {
            // SAFETY: restoring the termios captured from the same descriptor.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original);
            }
        }
    }

    /// The terminal's size in cells, from stdin or stdout, if either is one.
    pub fn window_size() -> Option<(u16, u16)> {
        size_of(libc::STDIN_FILENO).or_else(|| size_of(libc::STDOUT_FILENO))
    }

    fn size_of(fd: libc::c_int) -> Option<(u16, u16)> {
        // SAFETY: a winsize is plain data; ioctl fills it on success.
        let mut size = MaybeUninit::<libc::winsize>::zeroed();
        // SAFETY: fd is a valid descriptor (or fails harmlessly) and the
        // pointer is valid for one winsize.
        if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ as _, size.as_mut_ptr()) } != 0 {
            return None;
        }
        // SAFETY: ioctl succeeded, so winsize was written.
        let size = unsafe { size.assume_init() };
        (size.ws_col > 0 && size.ws_row > 0).then_some((size.ws_col, size.ws_row))
    }
}

#[cfg(not(unix))]
mod platform {
    use std::io;

    pub struct RawMode;

    impl RawMode {
        pub fn enter() -> io::Result<Option<RawMode>> {
            Ok(None)
        }
    }

    pub fn window_size() -> Option<(u16, u16)> {
        None
    }
}

pub use platform::{RawMode, window_size};
