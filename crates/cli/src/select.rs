//! A minimal arrow-key menu.
//!
//! This is deliberately not a TUI framework. It needs one screen of options, so
//! it drives the terminal directly: raw mode to read keys unbuffered, and cursor
//! moves to repaint the list in place. Anything that cannot do that — Windows, a
//! pipe, a terminal that rejects raw mode — falls back to a numbered prompt so
//! the command still works everywhere.

use std::io::{self, IsTerminal, Write};
use std::time::Duration;

/// Ask the user to pick one of `options`, returning its index.
///
/// Returns `None` when the user cancels with Esc, `q`, or Ctrl-C.
pub fn select(title: &str, options: &[&str], hint: &str) -> io::Result<Option<usize>> {
    if options.is_empty() {
        return Ok(None);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Ok(Some(0));
    }
    match arrow_select(title, options, hint) {
        Ok(choice) => Ok(choice),
        // Raw mode is unavailable on this terminal; degrade rather than fail.
        Err(_) => numbered_select(title, options),
    }
}

fn numbered_select(title: &str, options: &[&str]) -> io::Result<Option<usize>> {
    println!("\n{title}");
    for (i, option) in options.iter().enumerate() {
        println!("  {}) {option}", i + 1);
    }
    print!("\nChoose [1]: ");
    io::stdout().flush()?;

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let answer = answer.trim();
    if answer.is_empty() {
        return Ok(Some(0));
    }
    if matches!(answer, "q" | "Q") {
        return Ok(None);
    }
    Ok(answer
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=options.len()).contains(n))
        .map(|n| n - 1)
        .or(Some(0)))
}

#[cfg(not(unix))]
fn arrow_select(_: &str, _: &[&str], _: &str) -> io::Result<Option<usize>> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "no raw mode"))
}

#[cfg(unix)]
fn arrow_select(title: &str, options: &[&str], hint: &str) -> io::Result<Option<usize>> {
    let _raw = RawMode::enable()?;
    let mut cursor = 0usize;
    let mut out = io::stdout();

    writeln!(out, "\r\n{title}\r")?;
    if !hint.is_empty() {
        writeln!(out, "{}\r", dim(hint))?;
    }
    draw(&mut out, options, cursor, false)?;

    let mut buf = [0u8; 3];
    loop {
        let read = read_key(&mut buf)?;
        let key = &buf[..read];
        let action = match key {
            [0x1b, b'[', b'A'] | [b'k'] => Action::Up,
            [0x1b, b'[', b'B'] | [b'j'] => Action::Down,
            [b'\r'] | [b'\n'] => Action::Accept,
            // A bare Esc arrives alone; Esc-[ is the start of an arrow sequence.
            [0x1b] | [b'q'] | [3] => Action::Cancel,
            [b'1'..=b'9'] => {
                let n = (key[0] - b'1') as usize;
                if n < options.len() {
                    cursor = n;
                    Action::Accept
                } else {
                    Action::None
                }
            }
            _ => Action::None,
        };

        match action {
            Action::Up => cursor = (cursor + options.len() - 1) % options.len(),
            Action::Down => cursor = (cursor + 1) % options.len(),
            Action::Accept => {
                redraw(&mut out, options, cursor, true)?;
                return Ok(Some(cursor));
            }
            Action::Cancel => {
                redraw(&mut out, options, cursor, true)?;
                return Ok(None);
            }
            Action::None => continue,
        }
        redraw(&mut out, options, cursor, false)?;
    }
}

enum Action {
    Up,
    Down,
    Accept,
    Cancel,
    None,
}

fn draw(out: &mut impl Write, options: &[&str], cursor: usize, done: bool) -> io::Result<()> {
    for (i, option) in options.iter().enumerate() {
        if i == cursor {
            writeln!(out, "  \x1b[1;32m>\x1b[0m \x1b[1m{option}\x1b[0m\r")?;
        } else {
            writeln!(out, "    {}\r", dim(option))?;
        }
    }
    if done {
        write!(out, "\x1b[?25h")?;
    } else {
        write!(out, "\x1b[?25l")?;
    }
    out.flush()
}

fn redraw(out: &mut impl Write, options: &[&str], cursor: usize, done: bool) -> io::Result<()> {
    write!(out, "\x1b[{}A", options.len())?;
    draw(out, options, cursor, done)
}

fn dim(text: &str) -> String {
    format!("\x1b[2m{text}\x1b[0m")
}

#[cfg(unix)]
fn read_key(buf: &mut [u8; 3]) -> io::Result<usize> {
    use std::io::Read;
    let mut first = [0u8; 1];
    io::stdin().read_exact(&mut first)?;
    buf[0] = first[0];
    if first[0] != 0x1b {
        return Ok(1);
    }
    // Escape sequence: grab the two bytes that follow, if they are there.
    let mut rest = [0u8; 2];
    match io::stdin().read(&mut rest)? {
        2 => {
            buf[1] = rest[0];
            buf[2] = rest[1];
            Ok(3)
        }
        _ => Ok(1),
    }
}

#[cfg(unix)]
struct RawMode(libc::termios);

#[cfg(unix)]
impl RawMode {
    fn enable() -> io::Result<Self> {
        // SAFETY: tcgetattr/tcsetattr on a valid fd with a zeroed termios we own.
        unsafe {
            let mut original: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut original) != 0 {
                return Err(io::Error::last_os_error());
            }
            let mut raw = original;
            raw.c_lflag &= !(libc::ICANON | libc::ECHO);
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(RawMode(original))
        }
    }
}

#[cfg(unix)]
impl Drop for RawMode {
    fn drop(&mut self) {
        // Always restore the terminal, including on panic; a shell left in raw
        // mode with the cursor hidden is unusable.
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0);
        }
        let _ = write!(io::stdout(), "\x1b[?25h");
        let _ = io::stdout().flush();
    }
}

/// Ask a yes/no question, but never hold up an install for an answer.
///
/// Setup is meant to be one command that finishes. If nobody is watching, or
/// the user simply does not care, the default applies after `timeout` and the
/// install continues.
pub fn confirm_with_timeout(question: &str, default_yes: bool, timeout: Duration) -> bool {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return default_yes;
    }
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{question} {hint} ");
    io::stdout().flush().ok();

    let answered = read_line_with_timeout(timeout);
    match answered.as_deref().map(str::trim) {
        Some("y") | Some("Y") | Some("yes") => {
            println!();
            true
        }
        Some("n") | Some("N") | Some("no") => {
            println!();
            false
        }
        Some("") => {
            println!();
            default_yes
        }
        _ => {
            // Timed out. Say so, otherwise the default looks like a silent decision.
            println!("\n  no answer, keeping the default");
            default_yes
        }
    }
}

#[cfg(unix)]
fn read_line_with_timeout(timeout: Duration) -> Option<String> {
    use std::os::unix::io::AsRawFd;
    let fd = io::stdin().as_raw_fd();
    // SAFETY: a zeroed fd_set and a select() on a valid fd we own.
    let ready = unsafe {
        let mut set: libc::fd_set = std::mem::zeroed();
        libc::FD_ZERO(&mut set);
        libc::FD_SET(fd, &mut set);
        let mut tv = libc::timeval {
            tv_sec: timeout.as_secs() as libc::time_t,
            tv_usec: timeout.subsec_micros() as libc::suseconds_t,
        };
        libc::select(
            fd + 1,
            &mut set,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut tv,
        )
    };
    if ready <= 0 {
        return None;
    }
    let mut line = String::new();
    io::stdin().read_line(&mut line).ok().map(|_| line)
}

#[cfg(not(unix))]
fn read_line_with_timeout(_timeout: Duration) -> Option<String> {
    // No select() on stdin here; block rather than skip the question.
    let mut line = String::new();
    io::stdin().read_line(&mut line).ok().map(|_| line)
}
