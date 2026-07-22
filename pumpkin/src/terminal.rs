//! Terminal (TTY) state restoration.
//!
//! `rustyline` switches the terminal into raw mode (echo off, non-canonical)
//! while it reads a line and restores the previous state through an RAII guard
//! once `readline` returns. When the process exits while the console reader
//! thread is still blocked inside `readline` — for example from the panic hook
//! after a crash — that guard never runs, so the terminal is left in raw mode:
//! typed characters stop being echoed and stay that way across restarts until
//! the terminal session ends (see issue #2441).
//!
//! To avoid this we snapshot the terminal attributes before the console reader
//! ever enters raw mode and restore them explicitly on every exit path.

#[cfg(unix)]
mod imp {
    use std::mem::MaybeUninit;
    use std::os::fd::AsRawFd;
    use std::sync::OnceLock;

    static ORIGINAL_TERMIOS: OnceLock<libc::termios> = OnceLock::new();

    /// Snapshot the current terminal attributes of standard input so they can
    /// later be restored. Does nothing when stdin is not a TTY or when a
    /// snapshot has already been taken.
    pub fn save() {
        let fd = std::io::stdin().as_raw_fd();
        // SAFETY: `fd` is a valid file descriptor for the duration of the call.
        if unsafe { libc::isatty(fd) } != 1 {
            return;
        }

        let mut termios = MaybeUninit::<libc::termios>::uninit();
        // SAFETY: `termios` points to writable memory of the correct size.
        if unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) } == 0 {
            // SAFETY: `tcgetattr` succeeded, so `termios` is now initialized.
            let _ = ORIGINAL_TERMIOS.set(unsafe { termios.assume_init() });
        }
    }

    /// Restore the terminal attributes captured by [`save`]. Safe to call
    /// multiple times, including from a panic hook. Does nothing when no
    /// snapshot was taken.
    pub fn restore() {
        if let Some(termios) = ORIGINAL_TERMIOS.get() {
            let fd = std::io::stdin().as_raw_fd();
            // SAFETY: `termios` was produced by a successful `tcgetattr` and
            // `fd` is a valid file descriptor.
            unsafe {
                libc::tcsetattr(fd, libc::TCSANOW, std::ptr::from_ref(termios));
            }
        }
    }
}

#[cfg(not(unix))]
mod imp {
    /// No-op on non-Unix platforms; the Windows console driver does not leave
    /// stdin in a broken state when the process exits mid-read.
    pub const fn save() {}

    /// No-op on non-Unix platforms.
    pub const fn restore() {}
}

/// Snapshot the terminal state before the interactive console enters raw mode.
// `imp::save` is a `const` no-op on non-unix but performs FFI on unix, so this
// wrapper cannot be `const` on every target.
#[allow(clippy::missing_const_for_fn)]
pub fn save_terminal_state() {
    imp::save();
}

/// Restore the terminal to the state captured by [`save_terminal_state`].
///
/// This must be called on every path that terminates the process while the
/// console reader thread may be holding the terminal in raw mode.
// `imp::restore` is a `const` no-op on non-unix but performs FFI on unix, so
// this wrapper cannot be `const` on every target.
#[allow(clippy::missing_const_for_fn)]
pub fn restore_terminal() {
    imp::restore();
}

#[cfg(test)]
mod tests {
    use super::{restore_terminal, save_terminal_state};

    #[test]
    fn save_and_restore_are_safe_without_a_tty() {
        // Under a test harness stdin is not a TTY, so `save` records nothing and
        // `restore` becomes a no-op. Both must be safe to call (including
        // multiple times) without panicking.
        save_terminal_state();
        restore_terminal();
        restore_terminal();
    }
}
