//! Runs a command on a PTY whose window size carries pixel dimensions,
//! records everything it writes, and types into it from a steps file. For
//! scripts/herdr-screenshots.sh: herdr cuts kitty images into one strip per
//! row of its cell height, and under tmux it cannot learn termshot's, so the
//! strips are rescaled and their seams show. Here it learns it from the PTY.
//!
//!     cargo run --example pty-record -- LOG COLSxROWS CELL_WxCELL_H STEPS -- CMD...
//!
//! STEPS has one step a line: `sleep SECONDS`, `keys TEXT` (with `\r`, `\n`,
//! `\t`, `\\` and `\xHH` escapes), or `run SHELL-COMMAND` (run beside it, e.g.
//! a herdr CLI call).

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pty-record: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: pty-record LOG COLSxROWS CELL_WxCELL_H STEPS -- CMD...";
    let split = args.iter().position(|a| a == "--").ok_or(usage)?;
    let ([log, size, cell, steps], cmd) = (&args[..split], &args[split + 1..]) else {
        return Err(usage.into());
    };
    let (cols, rows) = pair(size)?;
    let (cw, ch) = pair(cell)?;
    if cmd.is_empty() {
        return Err(usage.into());
    }

    let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: cols * cw, ws_ypixel: rows * ch };
    let (mut master, slave) = openpty(&ws)?;
    let mut child = unsafe {
        Command::new(&cmd[0])
            .args(&cmd[1..])
            .stdin(Stdio::from(slave.try_clone().map_err(|e| e.to_string())?))
            .stdout(Stdio::from(slave.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(slave))
            // The PTY becomes the child's controlling terminal.
            .pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            })
            .spawn()
            .map_err(|e| format!("{}: {e}", cmd[0]))?
    };

    let mut out = File::create(log).map_err(|e| format!("{log}: {e}"))?;
    // What a terminal answers to the pixel-size queries.
    let answers = [
        (&b"\x1b[14t"[..], format!("\x1b[4;{};{}t", rows * ch, cols * cw)),
        (&b"\x1b[16t"[..], format!("\x1b[6;{ch};{cw}t")),
    ];
    let mut pump = |master: &mut File, seconds: f64| -> Result<(), String> {
        let end = Instant::now() + Duration::from_secs_f64(seconds);
        let mut buf = [0u8; 65536];
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            if !readable(master, left) {
                continue;
            }
            let n = match master.read(&mut buf) {
                Ok(0) | Err(_) => return Ok(()), // the child is gone
                Ok(n) => n,
            };
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            for (query, answer) in &answers {
                if buf[..n].windows(query.len()).any(|w| w == *query) {
                    master.write_all(answer.as_bytes()).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    };

    let steps = std::fs::read_to_string(steps).map_err(|e| format!("{steps}: {e}"))?;
    for line in steps.lines() {
        let (verb, arg) = line.split_once(' ').unwrap_or((line, ""));
        match verb {
            "sleep" => pump(&mut master, arg.parse().map_err(|_| format!("bad step: {line}"))?)?,
            "keys" => {
                master.write_all(&unescape(arg)?).map_err(|e| e.to_string())?;
                pump(&mut master, 0.3)?;
            }
            "run" => {
                let status = Command::new("sh").args(["-c", arg]).status().map_err(|e| e.to_string())?;
                if !status.success() {
                    return Err(format!("{arg}: {status}"));
                }
                pump(&mut master, 0.2)?;
            }
            "" => {}
            _ => return Err(format!("bad step: {line}")),
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// `AxB` as two numbers.
fn pair(s: &str) -> Result<(u16, u16), String> {
    let bad = || format!("not WxH: {s}");
    let (a, b) = s.split_once('x').ok_or_else(bad)?;
    Ok((a.parse().map_err(|_| bad())?, b.parse().map_err(|_| bad())?))
}

/// A PTY pair with this window size: the master as a file, the slave end.
fn openpty(ws: &libc::winsize) -> Result<(File, OwnedFd), String> {
    let (mut m, mut s): (RawFd, RawFd) = (-1, -1);
    let ws = ws as *const libc::winsize as *mut libc::winsize;
    if unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), ws) } < 0 {
        return Err(format!("openpty: {}", std::io::Error::last_os_error()));
    }
    Ok(unsafe { (File::from_raw_fd(m), OwnedFd::from_raw_fd(s)) })
}

/// Whether the master has output within `wait`.
fn readable(master: &File, wait: Duration) -> bool {
    use std::os::fd::AsRawFd;
    let mut fds = libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let ms = wait.as_millis().min(i32::MAX as u128) as i32;
    unsafe { libc::poll(&mut fds, 1, ms) > 0 }
}

/// `keys` text with its escapes turned into bytes.
fn unescape(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut it = s.bytes();
    while let Some(b) = it.next() {
        if b != b'\\' {
            out.push(b);
            continue;
        }
        match it.next() {
            Some(b'r') => out.push(b'\r'),
            Some(b'n') => out.push(b'\n'),
            Some(b't') => out.push(b'\t'),
            Some(b'\\') => out.push(b'\\'),
            Some(b'x') => {
                let hex = [it.next(), it.next()];
                let hex: Vec<u8> = hex.into_iter().flatten().collect();
                let v = std::str::from_utf8(&hex).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                out.push(v.ok_or_else(|| format!("bad \\x escape in: {s}"))?);
            }
            _ => return Err(format!("bad escape in: {s}")),
        }
    }
    Ok(out)
}
