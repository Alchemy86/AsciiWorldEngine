//! Raw mode, the alternate screen, and putting both back however we exit.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static RESTORED: AtomicBool = AtomicBool::new(false);
static mut SAVED: Option<libc::termios> = None;

/// Owns the terminal for the lifetime of the run. Restores on drop, on panic,
/// and on SIGINT/SIGTERM — a game that leaves your shell in raw mode with the
/// cursor hidden is a bug, not a quirk.
pub struct RawTerm {
    pub kitty: bool,
}

impl RawTerm {
    pub fn enter() -> std::io::Result<RawTerm> {
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut t) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            SAVED = Some(t);
            let mut raw = t;
            libc::cfmakeraw(&mut raw);
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
                libc::signal(sig, on_signal as *const () as libc::sighandler_t);
            }
        }
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            prev(info);
        }));

        let mut out = std::io::stdout();
        // Alternate screen, cursor off, clear.
        let _ = out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[2J");
        // Ask for the kitty keyboard protocol: disambiguate (1) + report event
        // types (2) + report all keys as escape codes (8). Then query what we
        // actually got, because most terminals answer nothing at all.
        let _ = out.write_all(b"\x1b[>11u\x1b[?u");
        let _ = out.flush();
        let kitty = probe_kitty();
        Ok(RawTerm { kitty })
    }
}

impl Drop for RawTerm {
    fn drop(&mut self) {
        restore();
    }
}

extern "C" fn on_signal(_sig: libc::c_int) {
    restore();
    std::process::exit(130);
}

pub fn restore() {
    if RESTORED.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut out = std::io::stdout();
    // Pop the keyboard flags before leaving, or the next program in this
    // terminal inherits them and its arrow keys stop working.
    let _ = out.write_all(b"\x1b[<u\x1b[?25h\x1b[?1049l\x1b[0m");
    let _ = out.flush();
    unsafe {
        if let Some(t) = SAVED {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t);
        }
    }
}

/// Read the reply to `CSI ? u`, if one comes within a short window.
fn probe_kitty() -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(140);
    let mut buf = [0u8; 64];
    let mut seen = Vec::new();
    while std::time::Instant::now() < deadline {
        let n = unsafe {
            let mut p = libc::pollfd { fd: libc::STDIN_FILENO, events: libc::POLLIN, revents: 0 };
            if libc::poll(&mut p, 1, 20) <= 0 {
                continue;
            }
            libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
        };
        if n <= 0 {
            continue;
        }
        seen.extend_from_slice(&buf[..n as usize]);
        // CSI ? <flags> u
        if let Some(i) = find(&seen, b"\x1b[?") {
            if let Some(j) = seen[i..].iter().position(|&c| c == b'u') {
                let digits: String = seen[i + 3..i + j].iter().map(|&c| c as char).collect();
                return digits.trim().parse::<u32>().map(|f| f & 2 != 0).unwrap_or(false);
            }
        }
    }
    false
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Terminal size in character cells.
pub fn terminal_size() -> (usize, usize) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
            && ws.ws_col > 0
            && ws.ws_row > 0
        {
            return (ws.ws_col as usize, ws.ws_row as usize);
        }
    }
    (120, 40)
}
