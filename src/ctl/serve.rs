//! Calls over `linearctl serve --stdio`: a few long-lived linearctl
//! processes that keep their connection to Linear and their key, instead of
//! a process per call (linear-cli measured ~80 ms saved a call).
//!
//! A request is `{"id", "argv"}` on a line; the answer is `{"id", "exit",
//! "result", "stdout", "stderr"}`. Each process answers one request at a
//! time, in order. A process that dies is started again once; if serve
//! cannot run at all, calls go to a process each, as before.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::thread;

use serde_json::{Value, json};

use super::{Exec, Failure, Runner, outcome};

struct Proc {
    child: Child,
    stdin: ChildStdin,
    answers: Receiver<String>,
    next_id: u64,
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A small pool of `linearctl serve --stdio` processes.
pub struct Serve {
    runner: Runner,
    slots: Vec<Mutex<Option<Proc>>>,
    next: AtomicUsize,
}

impl Serve {
    /// Up to `size` processes, started on first use, with `runner`'s program,
    /// environment and deadline; `runner` also takes the calls serve cannot.
    pub fn new(runner: Runner, size: usize) -> Self {
        Self {
            runner,
            slots: (0..size.max(1)).map(|_| Mutex::new(None)).collect(),
            next: AtomicUsize::new(0),
        }
    }

    fn start(&self) -> Result<Proc, Failure> {
        let mut child = Command::new(&self.runner.program)
            .args(["serve", "--stdio"])
            .envs(self.runner.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Failure::Spawn(format!("{}: {e}", self.runner.program.display())))?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, answers) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Ok(Proc { child, stdin, answers, next_id: 0 })
    }

    /// One request on the process in `slot`, starting it if needed.
    fn call(&self, slot: &mut Option<Proc>, argv: &[String]) -> Result<Value, Failure> {
        if slot.is_none() {
            *slot = Some(self.start()?);
        }
        let p = slot.as_mut().expect("just started");
        p.next_id += 1;
        let id = p.next_id;
        let line = json!({"id": id, "argv": argv}).to_string();
        if writeln!(p.stdin, "{line}").and_then(|()| p.stdin.flush()).is_err() {
            *slot = None;
            return Err(Failure::Crashed { code: None, stderr: "linearctl serve stopped".into() });
        }
        let deadline = std::time::Instant::now() + self.runner.timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match p.answers.recv_timeout(left) {
                Ok(answer) => {
                    let Ok(a) = serde_json::from_str::<Value>(&answer) else {
                        continue;
                    };
                    if a["id"] != id {
                        continue; // an answer to a request given up on
                    }
                    return answer_outcome(&a);
                }
                Err(RecvTimeoutError::Timeout) => {
                    *slot = None; // killed on drop
                    return Err(Failure::Timeout(self.runner.timeout));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    *slot = None;
                    return Err(Failure::Crashed { code: None, stderr: "linearctl serve stopped".into() });
                }
            }
        }
    }

    /// A call, retried once on a fresh process if serve died, and by a
    /// process of its own if serve cannot run.
    fn call_with_retry(&self, slot: &mut Option<Proc>, argv: &[String]) -> Result<Value, Failure> {
        match self.call(slot, argv) {
            Err(Failure::Crashed { code: None, .. }) => match self.call(slot, argv) {
                Err(Failure::Crashed { code: None, .. } | Failure::Spawn(_)) => self.runner.run(argv),
                other => other,
            },
            Err(Failure::Spawn(_)) => self.runner.run(argv),
            other => other,
        }
    }
}

/// A serve answer as a call's outcome.
fn answer_outcome(a: &Value) -> Result<Value, Failure> {
    let code = a["exit"].as_i64().and_then(|c| i32::try_from(c).ok());
    let stderr = a["stderr"].as_str().unwrap_or_default();
    let body = match &a["result"] {
        Value::Null => a["stdout"].as_str().unwrap_or_default().as_bytes().to_vec(),
        v => serde_json::to_vec(v).expect("a JSON value serializes"),
    };
    outcome(code, &body, stderr)
}

impl Exec for Serve {
    fn exec(&self, argv: &[String]) -> Result<Value, Failure> {
        // A free process if there is one, else wait for one in turn.
        for slot in &self.slots {
            if let Ok(mut s) = slot.try_lock() {
                return self.call_with_retry(&mut s, argv);
            }
        }
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.slots.len();
        let mut s = self.slots[i].lock().unwrap_or_else(PoisonError::into_inner);
        self.call_with_retry(&mut s, argv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_become_outcomes() {
        let ok = json!({"id": 1, "exit": 0, "result": {"a": "x\u{1b}y"}, "stdout": null, "stderr": ""});
        assert_eq!(answer_outcome(&ok).unwrap(), json!({"a": "xy"}), "sanitized like any result");
        let failed = json!({"id": 1, "exit": 1, "result": {"error": "not found: issue X", "usage": false}, "stdout": null, "stderr": ""});
        assert_eq!(answer_outcome(&failed), Err(Failure::Failed("not found: issue X".into())));
        let text = json!({"id": 1, "exit": 0, "result": null, "stdout": "linearctl 0.4.0\n", "stderr": ""});
        assert!(matches!(answer_outcome(&text), Err(Failure::BadOutput(_))));
        let odd = json!({"id": 1, "exit": "x"});
        assert!(matches!(answer_outcome(&odd), Err(Failure::Crashed { code: None, .. })));
    }
}
