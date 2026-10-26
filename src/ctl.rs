//! Running linearctl: the argv for each request, and one call from spawn to
//! parsed, sanitized JSON with linearctl's exit codes mapped to [`Failure`].

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::model::ErrorDoc;
use crate::sanitize::clean_json;

/// The `issue list` fields the list screen shows.
pub const ROW_FIELDS: &str = "id,identifier,title,priority,priorityLabel,url,createdAt,updatedAt,state,assignee,team,project,cycle,labels";

/// Comments loaded with an issue; more come from `comment list`.
pub const DETAIL_COMMENTS: usize = 50;

/// Seconds a signed file URL stays valid.
pub const SIGNED_URL_SECONDS: u32 = 300;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_string()).collect()
}

/// One page of a view: its filter flags, then paging and fields.
pub fn list_argv(view: &[String], limit: usize, cursor: Option<&str>) -> Vec<String> {
    let mut a = argv(&["issue", "list"]);
    a.extend(view.iter().cloned());
    a.extend(argv(&["--limit", &limit.to_string(), "--fields", ROW_FIELDS]));
    if let Some(c) = cursor {
        a.extend(argv(&["--cursor", c]));
    }
    a.push("--json".into());
    a
}

/// One issue with its comments, and image URLs signed for the viewer.
pub fn show_argv(issue: &str) -> Vec<String> {
    argv(&[
        "issue",
        "show",
        issue,
        &format!("--comments={DETAIL_COMMENTS}"),
        "--signed-urls",
        &SIGNED_URL_SECONDS.to_string(),
        "--json",
    ])
}

pub fn auth_status_argv() -> Vec<String> {
    argv(&["auth", "status", "--json"])
}

/// Why a call produced no result.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// Exit 1: Linear or the network said no; the message names what.
    Failed(String),
    /// Exit 2: the TUI built a bad command line (a bug here).
    Usage(String),
    /// Exit 3: partial data; not expected from the commands the TUI runs.
    Partial(String),
    /// Exit 4: linearctl's lock was held, after every retry.
    Busy(String),
    /// Exit 5: no API key, or Linear rejected it.
    Auth(String),
    /// Exit 6: the budget is spent until `retry_at` (RFC 3339).
    RateLimited {
        message: String,
        retry_at: Option<String>,
    },
    /// Any other exit, e.g. a crash.
    Crashed { code: Option<i32>, stderr: String },
    /// linearctl could not be started.
    Spawn(String),
    /// No answer within the deadline; the process was killed.
    Timeout(Duration),
    /// Exit 0 with stdout that is not the JSON expected.
    BadOutput(String),
}

impl Failure {
    /// One line for the status bar.
    pub fn message(&self) -> String {
        match self {
            Self::Failed(m) | Self::Partial(m) | Self::Busy(m) | Self::Auth(m) => m.clone(),
            Self::Usage(m) => format!("internal error (bad linearctl invocation): {m}"),
            Self::RateLimited {
                message,
                retry_at: Some(t),
            } => format!("{message}; retry after {t}"),
            Self::RateLimited {
                message,
                retry_at: None,
            } => message.clone(),
            Self::Crashed { code, stderr } => {
                let code = code.map_or("a signal".to_string(), |c| format!("exit {c}"));
                format!(
                    "linearctl failed ({code}): {}",
                    stderr.lines().last().unwrap_or("").trim()
                )
            }
            Self::Spawn(m) => format!("cannot run linearctl: {m}"),
            Self::Timeout(d) => format!("linearctl gave no answer in {} s", d.as_secs()),
            Self::BadOutput(m) => format!("unexpected output from linearctl: {m}"),
        }
    }
}

/// Spawns linearctl. Cheap to clone; each call is its own process.
#[derive(Debug, Clone)]
pub struct Runner {
    pub program: PathBuf,
    pub env: Vec<(String, String)>,
    /// The deadline for one process, kill included.
    pub timeout: Duration,
    /// Retries after exit 4 (busy), waiting `busy_backoff`, then twice that...
    pub busy_retries: u32,
    pub busy_backoff: Duration,
    /// The working directory; `file download` writes only inside it.
    pub cwd: Option<PathBuf>,
}

impl Runner {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            env: Vec::new(),
            timeout: Duration::from_secs(20),
            busy_retries: 3,
            busy_backoff: Duration::from_millis(200),
            cwd: None,
        }
    }

    /// Run and parse into `T`.
    pub fn run_as<T: DeserializeOwned>(&self, argv: &[String]) -> Result<T, Failure> {
        let v = self.run(argv)?;
        serde_json::from_value(v).map_err(|e| Failure::BadOutput(e.to_string()))
    }

    /// Run, retrying while linearctl is busy; the JSON comes back sanitized.
    pub fn run(&self, argv: &[String]) -> Result<Value, Failure> {
        let mut wait = self.busy_backoff;
        let mut tries = 0;
        loop {
            match self.run_once(argv) {
                Err(Failure::Busy(_)) if tries < self.busy_retries => {
                    tries += 1;
                    thread::sleep(wait);
                    wait *= 2;
                }
                other => return other,
            }
        }
    }

    fn run_once(&self, argv: &[String]) -> Result<Value, Failure> {
        let mut cmd = Command::new(&self.program);
        if let Some(dir) = &self.cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd
            .args(argv)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Failure::Spawn(format!("{}: {e}", self.program.display())))?;
        // Drain both pipes on threads, so a large answer cannot block the child.
        let drain = |mut r: Box<dyn Read + Send>| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = r.read_to_end(&mut buf);
                buf
            })
        };
        let out = drain(Box::new(child.stdout.take().expect("stdout is piped")));
        let err = drain(Box::new(child.stderr.take().expect("stderr is piped")));
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            if let Some(s) = child.try_wait().map_err(|e| Failure::Spawn(e.to_string()))? {
                break s;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::Timeout(self.timeout));
            }
            thread::sleep(Duration::from_millis(5));
        };
        let stdout = out.join().unwrap_or_default();
        let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
        let code = status.code();
        if code == Some(0) {
            let mut v: Value =
                serde_json::from_slice(&stdout).map_err(|e| Failure::BadOutput(e.to_string()))?;
            clean_json(&mut v);
            return Ok(v);
        }
        let doc: Option<ErrorDoc> = serde_json::from_slice::<Value>(&stdout).ok().and_then(|mut v| {
            clean_json(&mut v);
            serde_json::from_value(v).ok()
        });
        let Some(doc) = doc else {
            return Err(Failure::Crashed {
                code,
                stderr: crate::sanitize::clean(&stderr),
            });
        };
        Err(match code {
            Some(1) => Failure::Failed(doc.error),
            Some(2) => Failure::Usage(doc.error),
            Some(3) => Failure::Partial(doc.error),
            Some(4) => Failure::Busy(doc.error),
            Some(5) => Failure::Auth(doc.error),
            Some(6) => Failure::RateLimited {
                message: doc.error,
                retry_at: doc.retry_at,
            },
            _ => Failure::Crashed {
                code,
                stderr: doc.error,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::Describe;

    fn describe() -> Describe {
        Describe::from_json(include_str!("../tests/fixtures/linearctl-describe-0.1.0.json")).unwrap()
    }

    fn assert_valid(argv: &[String]) {
        assert_eq!(argv.last().map(String::as_str), Some("--json"));
        let problems = describe().check_argv(argv);
        assert!(problems.is_empty(), "{argv:?}: {problems:?}");
    }

    #[test]
    fn list_argv_is_valid_linearctl() {
        let view: Vec<String> = [
            "--assignee",
            "me",
            "--state-type",
            "started",
            "--sort",
            "priority",
        ]
        .map(String::from)
        .to_vec();
        let a = list_argv(&view, 50, None);
        assert_valid(&a);
        assert_eq!(a[..4], ["issue", "list", "--assignee", "me"]);
        assert!(a.windows(2).any(|w| w == ["--limit", "50"]));
        assert!(!a.contains(&"--cursor".to_string()));
        let next = list_argv(&view, 50, Some("abc"));
        assert_valid(&next);
        assert!(next.windows(2).any(|w| w == ["--cursor", "abc"]));
    }

    #[test]
    fn show_and_auth_argv_are_valid_linearctl() {
        let s = show_argv("ENG-142");
        assert_valid(&s);
        assert!(s.contains(&"--comments=50".to_string()));
        assert!(s.windows(2).any(|w| w == ["--signed-urls", "300"]));
        assert_valid(&auth_status_argv());
    }

    #[test]
    fn row_fields_are_issue_list_fields() {
        // linearctl 0.1.0's documented issue list fields.
        let known = "id identifier title priority priorityLabel url createdAt updatedAt state assignee team project cycle labels";
        for f in ROW_FIELDS.split(',') {
            assert!(known.split(' ').any(|k| k == f), "{f}");
        }
    }

    #[test]
    fn messages() {
        let m = |f: Failure| f.message();
        assert_eq!(
            m(Failure::Failed("not found: issue X".into())),
            "not found: issue X"
        );
        assert!(m(Failure::Usage("unknown flag".into())).starts_with("internal error"));
        assert_eq!(
            m(Failure::RateLimited {
                message: "slow down".into(),
                retry_at: Some("T".into())
            }),
            "slow down; retry after T"
        );
        assert_eq!(
            m(Failure::RateLimited {
                message: "slow down".into(),
                retry_at: None
            }),
            "slow down"
        );
        assert_eq!(
            m(Failure::Crashed {
                code: Some(101),
                stderr: "a\nboom \n".into()
            }),
            "linearctl failed (exit 101): boom"
        );
        assert_eq!(
            m(Failure::Crashed {
                code: None,
                stderr: String::new()
            }),
            "linearctl failed (a signal): "
        );
        assert!(m(Failure::Spawn("x".into())).starts_with("cannot run linearctl"));
        assert_eq!(
            m(Failure::Timeout(Duration::from_secs(20))),
            "linearctl gave no answer in 20 s"
        );
        assert!(m(Failure::BadOutput("eof".into())).contains("eof"));
        for f in [
            Failure::Partial("p".into()),
            Failure::Busy("b".into()),
            Failure::Auth("a".into()),
        ] {
            assert_eq!(f.message().len(), 1);
        }
    }
}
