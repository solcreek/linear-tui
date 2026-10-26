//! `linearctl watch`: a long-running process whose lines are events.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use super::{Failure, Runner, outcome};
use crate::model::WatchEvent;

/// What a watcher reports.
#[derive(Debug, Clone, PartialEq)]
pub enum WatchLine {
    Event(WatchEvent),
    /// It ended: why, if it failed (not when stopped).
    End(Option<Failure>),
}

/// A running `watch`; dropping it stops it.
pub struct Watcher {
    child: Arc<Mutex<Child>>,
    stopped: Arc<Mutex<bool>>,
}

impl Watcher {
    /// Start `watch` (argv after the program) with `runner`'s program and
    /// environment; `report` gets each line on a thread of its own.
    pub fn start(
        runner: &Runner,
        argv: &[String],
        report: impl Fn(WatchLine) + Send + 'static,
    ) -> Result<Self, Failure> {
        let mut child = Command::new(&runner.program)
            .args(argv)
            .envs(runner.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Failure::Spawn(format!("{}: {e}", runner.program.display())))?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let child = Arc::new(Mutex::new(child));
        let stopped = Arc::new(Mutex::new(false));
        let (c, s) = (child.clone(), stopped.clone());
        thread::spawn(move || {
            // The last line that is not an event: a failure's {error}.
            let mut last = String::new();
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<WatchEvent>(&line) {
                    Ok(ev) => report(WatchLine::Event(ev)),
                    Err(_) => last = line,
                }
            }
            let mut err = String::new();
            let _ = stderr.read_to_string(&mut err);
            // Poll rather than wait, so Drop can take the lock to kill it.
            let code = loop {
                match c.lock().unwrap_or_else(PoisonError::into_inner).try_wait() {
                    Ok(Some(status)) => break status.code(),
                    Ok(None) => {}
                    Err(_) => break None,
                }
                thread::sleep(std::time::Duration::from_millis(10));
            };
            if *s.lock().unwrap_or_else(PoisonError::into_inner) {
                return; // stopped on purpose
            }
            let failure = match code {
                Some(0) => None,
                _ => outcome(code, last.as_bytes(), &err).err(),
            };
            report(WatchLine::End(failure));
        });
        Ok(Self { child, stopped })
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        *self.stopped.lock().unwrap_or_else(PoisonError::into_inner) = true;
        let _ = self.child.lock().unwrap_or_else(PoisonError::into_inner).kill();
    }
}
