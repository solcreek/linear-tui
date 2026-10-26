//! A stand-in for `linearctl` that serves the demo workspace.
//!
//! It answers the read commands the TUI uses with the same JSON shapes as
//! linearctl, built from `demo/workspace.json`, and refuses every write. It
//! exists until linearctl has a demo mode of its own; after that it stays
//! as the test double for exit codes and odd answers.
//!
//! Times in the seed are shifted so its `anchor` is "now", so the demo
//! always looks recent and relative times are the same on every run.

use std::collections::HashMap;

use chrono::{DateTime, Duration, NaiveDate, SecondsFormat, Utc};
use serde_json::{Map, Value, json};

pub mod stream;
mod writes;

/// The demo workspace.
pub const SEED: &str = include_str!("../demo/workspace.json");
/// What `describe` prints: linearctl 0.3.0's command tree. The fake does the
/// commands linear-tui runs, `serve --stdio` and `watch` among them.
const DESCRIBE: &str = include_str!("../tests/fixtures/linearctl-describe-0.3.0.json");

/// One finished command: exit code, stdout, stderr.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    fn ok(v: Value) -> Self {
        Self { code: 0, stdout: format!("{v}\n"), stderr: String::new() }
    }

    fn page(items: Vec<Value>, limit: usize, cursor: Option<&str>) -> Self {
        let start = match cursor {
            Some(c) => match items.iter().position(|i| i["id"] == c) {
                Some(p) => p + 1,
                None => return Self::failed(&format!("unknown cursor {c}")),
            },
            None => 0,
        };
        let rest = &items[start.min(items.len())..];
        let n = if limit == 0 { rest.len() } else { limit.min(rest.len()) };
        let truncated = n < rest.len();
        let shown = rest[..n].to_vec();
        let next = if truncated { shown.last().map(|i| i["id"].clone()) } else { None };
        let mut out = Self::ok(json!({
            "items": shown, "total": null, "shown": n, "truncated": truncated,
            "nextCursor": next.clone().unwrap_or(Value::Null),
        }));
        if let Some(Value::String(c)) = next {
            out.stderr =
                format!("{n} shown, more remain; --cursor {c} for the next page, --limit 0 for all\n");
        }
        out
    }

    fn failed(msg: &str) -> Self {
        Self::error(1, msg, false)
    }

    fn usage(msg: &str) -> Self {
        Self::error(2, msg, true)
    }

    fn error(code: i32, msg: &str, usage: bool) -> Self {
        Self {
            code,
            stdout: format!("{}\n", json!({"error": msg, "usage": usage})),
            stderr: format!("linearctl: {msg}\n"),
        }
    }
}

/// Flags that take no value.
const BOOLEAN: &[&str] = &["json", "yes", "dry-run", "active", "help", "version"];

/// Parsed argv: positional words and flags (repeatable ones keep every value).
#[derive(Debug, Default)]
struct Args {
    words: Vec<String>,
    flags: HashMap<String, Vec<String>>,
}

impl Args {
    fn parse(argv: &[String]) -> Result<Self, String> {
        let mut a = Self::default();
        let mut it = argv.iter();
        while let Some(tok) = it.next() {
            let Some(flag) = tok.strip_prefix("--") else {
                a.words.push(tok.clone());
                continue;
            };
            let (name, value) = match flag.split_once('=') {
                Some((n, v)) => (n.to_string(), v.to_string()),
                None if BOOLEAN.contains(&flag) => (flag.to_string(), "true".into()),
                None => {
                    let v = it.next().ok_or_else(|| format!("flag needs an argument: --{flag}"))?;
                    (flag.to_string(), v.clone())
                }
            };
            a.flags.entry(name).or_default().push(value);
        }
        Ok(a)
    }

    fn one(&self, name: &str) -> Option<&str> {
        self.flags.get(name).and_then(|v| v.last()).map(String::as_str)
    }

    fn all(&self, name: &str) -> &[String] {
        self.flags.get(name).map(Vec::as_slice).unwrap_or_default()
    }

    fn has(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }

    /// Fail on any flag not in `known` (global flags are always allowed).
    fn only(&self, known: &[&str]) -> Result<(), String> {
        let global = ["json", "timeout"];
        match self.flags.keys().find(|k| !known.contains(&k.as_str()) && !global.contains(&k.as_str())) {
            Some(k) => Err(format!("unknown flag: --{k}")),
            None => Ok(()),
        }
    }

    fn limit(&self, default: usize) -> Result<usize, String> {
        self.one("limit")
            .map_or(Ok(default), |v| v.parse().map_err(|_| format!("--limit: not a number: {v}")))
    }
}

/// Run one linearctl command line (without the program name) at `now`.
pub fn run(argv: &[String], now: DateTime<Utc>) -> Output {
    run_in(argv, now, None)
}

/// The fake run in-process, as a [`crate::ctl::Exec`]: for tests that need
/// no binary. Times are relative to `now`; writes go to `state` if given.
pub struct InProcess {
    pub now: DateTime<Utc>,
    pub state: Option<std::path::PathBuf>,
}

impl crate::ctl::Exec for InProcess {
    fn exec(&self, argv: &[String]) -> Result<Value, crate::ctl::Failure> {
        let out = run_in(argv, self.now, self.state.as_deref());
        crate::ctl::outcome(Some(out.code), out.stdout.as_bytes(), &out.stderr)
    }
}

/// The environment variable naming a file that keeps the demo workspace's
/// writes; without it, writes are refused as in linearctl's demo mode.
pub const STATE_ENV: &str = "FAKE_LINEARCTL_STATE";

/// Run with the workspace kept in `state` (created on the first write).
pub fn run_in(argv: &[String], now: DateTime<Utc>, state: Option<&std::path::Path>) -> Output {
    let args = match Args::parse(argv) {
        Ok(a) => a,
        Err(e) => return Output::usage(&e),
    };
    let ws = Workspace::from_seed(writes::load_raw(state), now);
    let words: Vec<&str> = args.words.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        [] if args.has("version") => {
            return Output {
                code: 0,
                stdout: "linearctl 0.3.0 (linear-tui demo)\n".into(),
                stderr: String::new(),
            };
        }
        ["describe"] => {
            return Output { code: 0, stdout: DESCRIBE.into(), stderr: String::new() };
        }
        ["auth", "status"] => args.only(&[]).map(|()| Output::ok(ws.auth_status())),
        ["team", "list"] => list(&args, &[], || ws.teams()),
        ["user", "list"] => list(&args, &["active"], || ws.users()),
        ["state", "list"] => list(&args, &["team"], || ws.states()),
        ["label", "list"] => list(&args, &["team"], || ws.labels()),
        ["project", "list"] => list(&args, &["team", "status"], || ws.projects(args.all("status"))),
        ["cycle", "list"] => list(&args, &["team"], || ws.cycles()),
        ["issue", "list"] => ws.issue_list(&args),
        ["issue", "show" | "view", id] => ws.issue_show(id, &args),
        ["comment", "list", id] => ws.comment_list(id, &args),
        ["issue", "update" | "archive", _] | ["issue", "create"] | ["comment", "create", _] => {
            writes::write(&words, &args, now, state)
        }
        ["file", "download", ..] | ["config", "set", ..] | ["auth", "login" | "logout"] => {
            Ok(Output::failed("the demo workspace is read-only"))
        }
        _ => Err(format!("unknown command: {}", args.words.join(" "))),
    };
    result.unwrap_or_else(|e| Output::usage(&e))
}

/// The environment variable that makes fake-linearctl misbehave, for tests.
pub const FAULT_ENV: &str = "FAKE_LINEARCTL_FAULT";

/// Run with an injected fault (the value of [`FAULT_ENV`]), else normally:
///
/// - `exit:N`: fail with exit code N and linearctl's error document
///   (`retryAt` included for 6);
/// - `sleep:MS`: wait, then answer;
/// - `garbage`: exit 0 with stdout that is not JSON;
/// - `crash`: exit 101 with nothing on stdout;
/// - `busy-once:PATH`: exit 4 the first time (creating PATH), then answer.
pub fn run_with_fault(
    argv: &[String],
    now: DateTime<Utc>,
    fault: Option<&str>,
    state: Option<&std::path::Path>,
) -> Output {
    let Some(fault) = fault.filter(|f| !f.is_empty()) else {
        return run_in(argv, now, state);
    };
    let (kind, arg) = fault.split_once(':').unwrap_or((fault, ""));
    match kind {
        "exit" => match arg.parse::<i32>() {
            Ok(6) => Output {
                code: 6,
                stdout: format!(
                    "{}\n",
                    json!({"error": "rate limited by Linear", "usage": false, "retryAt": "2026-10-26T13:00:00Z"})
                ),
                stderr: "linearctl: rate limited by Linear\n".into(),
            },
            Ok(code) => Output::error(code, &format!("injected failure {code}"), code == 2),
            Err(_) => Output::usage(&format!("{FAULT_ENV}: bad exit code: {arg}")),
        },
        "sleep" => match arg.parse() {
            Ok(ms) => {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                run_in(argv, now, state)
            }
            Err(_) => Output::usage(&format!("{FAULT_ENV}: bad sleep: {arg}")),
        },
        "garbage" => Output { code: 0, stdout: "this is not json\n".into(), stderr: String::new() },
        "crash" => Output { code: 101, stdout: String::new(), stderr: "thread 'main' panicked\n".into() },
        "busy-once" => {
            let marker = std::path::Path::new(arg);
            if marker.exists() {
                run_in(argv, now, state)
            } else {
                let _ = std::fs::write(marker, "");
                Output::error(4, "the config lock is held; nothing changed", false)
            }
        }
        _ => Output::usage(&format!("{FAULT_ENV}: unknown fault: {fault}")),
    }
}

fn list(args: &Args, known: &[&str], items: impl FnOnce() -> Vec<Value>) -> Result<Output, String> {
    let mut all = vec!["limit", "cursor", "fields"];
    all.extend_from_slice(known);
    args.only(&all)?;
    let limit = args.limit(50)?;
    let items = pick_all(items(), args.one("fields"));
    Ok(Output::page(items, limit, args.one("cursor")))
}

fn pick_all(items: Vec<Value>, fields: Option<&str>) -> Vec<Value> {
    match fields {
        None => items,
        Some(f) => items.iter().map(|i| pick(i, f)).collect(),
    }
}

fn pick(item: &Value, fields: &str) -> Value {
    let keep: Vec<&str> = fields.split(',').map(str::trim).collect();
    // The id stays for paging, as linearctl's cursor is the last item's id.
    let m: Map<String, Value> = item
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| keep.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Value::Object(m)
}

/// A stable UUID-shaped id from a name, so ids are the same on every run.
fn uuid(name: &str) -> String {
    let h = |seed: u64| {
        name.bytes()
            .fold(0xcbf2_9ce4_8422_2325 ^ seed, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
    };
    let (a, b) = (h(1), h(2));
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xfff,
        b >> 52,
        b & 0xffff_ffff_ffff
    )
}

/// An issue's id: the one it was created with, else derived from its
/// identifier.
fn issue_uuid(i: &Value) -> String {
    i["uuid"].as_str().map_or_else(|| uuid(i["identifier"].as_str().unwrap_or_default()), str::to_string)
}

/// A comment's id, the same way.
fn comment_uuid(c: &Value) -> String {
    c["uuid"].as_str().map_or_else(|| uuid(c["id"].as_str().unwrap_or_default()), str::to_string)
}

fn priority_label(p: i64) -> &'static str {
    match p {
        1 => "Urgent",
        2 => "High",
        3 => "Medium",
        4 => "Low",
        _ => "No priority",
    }
}

fn slug(title: &str) -> String {
    let s: String =
        title.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    s.split('-').filter(|w| !w.is_empty()).collect::<Vec<_>>().join("-")
}

/// The seed, with times shifted to `now`.
struct Workspace {
    seed: Value,
    now: DateTime<Utc>,
}

impl Workspace {
    fn from_seed(mut seed: Value, now: DateTime<Utc>) -> Self {
        let anchor = DateTime::parse_from_rfc3339(seed["anchor"].as_str().expect("anchor"))
            .expect("anchor is RFC 3339")
            .with_timezone(&Utc);
        shift(&mut seed, now - anchor);
        Self { seed, now }
    }

    fn org_key(&self) -> &str {
        self.seed["organization"]["urlKey"].as_str().unwrap_or_default()
    }

    fn user(&self, email: &str) -> Option<&Value> {
        self.seed["users"].as_array()?.iter().find(|u| u["email"] == email)
    }

    fn user_ref(&self, email: &Value) -> Value {
        match email.as_str().and_then(|e| self.user(e)) {
            Some(u) => json!({"id": uuid(&u["email"].to_string()), "name": u["name"], "email": u["email"]}),
            None => Value::Null,
        }
    }

    fn named(&self, kind: &str, name: &str) -> Option<&Value> {
        self.seed[kind].as_array()?.iter().find(|x| x["name"] == name)
    }

    fn issues(&self) -> &[Value] {
        self.seed["issues"].as_array().map(Vec::as_slice).unwrap_or_default()
    }

    fn issue(&self, id: &str) -> Option<&Value> {
        self.issues().iter().find(|i| i["identifier"] == id || issue_uuid(i) == id)
    }

    fn team(&self) -> &Value {
        &self.seed["teams"][0]
    }

    fn auth_status(&self) -> Value {
        let viewer = self.user_ref(&self.seed["viewer"]);
        let reset = (self.now + Duration::hours(1)).to_rfc3339_opts(SecondsFormat::Secs, true);
        json!({
            "organization": self.seed["organization"],
            "rateLimit": {
                "complexity": {"limit": 3_000_000, "remaining": 3_000_000, "resetAt": reset},
                "requests": {"limit": 2500, "remaining": 2500, "resetAt": reset},
            },
            "source": "demo",
            "user": viewer,
        })
    }

    fn teams(&self) -> Vec<Value> {
        let t = self.team();
        vec![json!({
            "description": t["description"], "id": uuid(&format!("team {}", t["key"])),
            "key": t["key"], "name": t["name"], "private": t["private"],
        })]
    }

    fn users(&self) -> Vec<Value> {
        let users = self.seed["users"].as_array().into_iter().flatten();
        users
            .map(|u| {
                json!({
                    "active": true, "admin": u["admin"].as_bool().unwrap_or(false),
                    "displayName": u["displayName"], "email": u["email"], "guest": false,
                    "id": uuid(&u["email"].to_string()), "name": u["name"],
                })
            })
            .collect()
    }

    fn team_ref(&self) -> Value {
        json!({"key": self.team()["key"]})
    }

    fn states(&self) -> Vec<Value> {
        let states = self.seed["states"].as_array().into_iter().flatten();
        states
            .enumerate()
            .map(|(i, s)| {
                json!({
                    "color": s["color"], "id": uuid(&format!("state {}", s["name"])), "name": s["name"],
                    "position": i, "team": self.team_ref(), "type": s["type"],
                })
            })
            .collect()
    }

    fn labels(&self) -> Vec<Value> {
        let labels = self.seed["labels"].as_array().into_iter().flatten();
        labels
            .map(|l| {
                json!({
                    "color": l["color"], "id": uuid(&format!("label {}", l["name"])), "isGroup": false,
                    "name": l["name"], "parent": null, "team": self.team_ref(),
                })
            })
            .collect()
    }

    fn projects(&self, status: &[String]) -> Vec<Value> {
        let projects = self.seed["projects"].as_array().into_iter().flatten();
        projects
            .filter(|p| status.is_empty() || status.iter().any(|s| p["status"] == s.as_str()))
            .map(|p| {
                let name = p["name"].as_str().unwrap_or_default();
                let mut status = p["status"].as_str().unwrap_or_default().to_string();
                status[..1].make_ascii_uppercase();
                json!({
                    "id": uuid(&format!("project {name}")), "lead": self.user_ref(&p["lead"]), "name": name,
                    "status": {"name": status, "type": p["status"]}, "targetDate": p["targetDate"],
                    "url": format!("https://linear.app/{}/project/{}", self.org_key(), slug(name)),
                })
            })
            .collect()
    }

    fn cycles(&self) -> Vec<Value> {
        let cycles = self.seed["cycles"].as_array().into_iter().flatten();
        let active = self.current_cycle();
        cycles
            .map(|c| {
                json!({
                    "endsAt": c["endsAt"], "id": uuid(&format!("cycle {}", c["number"])),
                    "isActive": Some(&c["number"]) == active.as_ref(),
                    "isNext": c["number"].as_i64().zip(active.as_ref().and_then(Value::as_i64))
                        .is_some_and(|(n, a)| n == a + 1),
                    "name": null, "number": c["number"], "startsAt": c["startsAt"], "team": self.team_ref(),
                })
            })
            .collect()
    }

    fn current_cycle(&self) -> Option<Value> {
        let now = self.now.to_rfc3339_opts(SecondsFormat::Millis, true);
        let cycles = self.seed["cycles"].as_array()?;
        let c = cycles.iter().find(|c| {
            c["startsAt"].as_str().is_some_and(|s| s <= now.as_str())
                && c["endsAt"].as_str().is_some_and(|e| now.as_str() < e)
        })?;
        Some(c["number"].clone())
    }

    fn row(&self, i: &Value) -> Value {
        let ident = i["identifier"].as_str().unwrap_or_default();
        let state = self.named("states", i["state"].as_str().unwrap_or_default());
        let labels: Vec<Value> = i["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| self.named("labels", n.as_str()?))
            .map(|l| json!({"id": uuid(&format!("label {}", l["name"])), "name": l["name"], "color": l["color"]}))
            .collect();
        let priority = i["priority"].as_i64().unwrap_or(0);
        let project = match i["project"].as_str() {
            Some(p) => json!({"id": uuid(&format!("project {p}")), "name": p}),
            None => Value::Null,
        };
        let cycle = match &i["cycle"] {
            Value::Number(n) => json!({"id": uuid(&format!("cycle {n}")), "number": n}),
            _ => Value::Null,
        };
        json!({
            "id": issue_uuid(i),
            "identifier": ident,
            "title": i["title"],
            "priority": priority,
            "priorityLabel": priority_label(priority),
            "url": format!("https://linear.app/{}/issue/{ident}/{}", self.org_key(), slug(i["title"].as_str().unwrap_or_default())),
            "createdAt": i["createdAt"],
            "updatedAt": i["updatedAt"],
            "state": state.map_or(Value::Null, |s| json!({
                "id": uuid(&format!("state {}", s["name"])), "name": s["name"], "type": s["type"], "color": s["color"],
            })),
            "assignee": self.user_ref(&i["assignee"]),
            "delegate": null,
            "team": {"id": uuid(&format!("team {}", self.team()["key"])), "key": self.team()["key"]},
            "project": project,
            "cycle": cycle,
            "projectMilestone": null,
            "labels": labels,
        })
    }

    fn comment(&self, issue: &str, c: &Value) -> Value {
        let id = comment_uuid(c);
        json!({
            "id": id,
            "body": c["body"],
            "createdAt": c["createdAt"],
            "url": format!("https://linear.app/{}/issue/{issue}#comment-{}", self.org_key(), &id[..8]),
            "user": self.user_ref(&c["user"]),
            "parent": c["parent"].as_str().map_or(Value::Null, |p| json!({"id": uuid(p)})),
        })
    }

    fn detail(&self, i: &Value, comments: usize) -> Value {
        let ident = i["identifier"].as_str().unwrap_or_default();
        let mut d = self.row(i);
        let brief = |x: &Value| json!({"identifier": x["identifier"], "title": x["title"]});
        let children: Vec<Value> = self
            .issues()
            .iter()
            .filter(|c| c["parent"] == ident)
            .map(|c| {
                let s = self.named("states", c["state"].as_str().unwrap_or_default());
                json!({"identifier": c["identifier"], "title": c["title"],
                       "state": s.map_or(Value::Null, |s| json!({"name": s["name"], "type": s["type"]}))})
            })
            .collect();
        let relations: Vec<Value> = i["relations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| {
                let other = self.issue(r["issue"].as_str()?)?;
                Some(json!({"id": uuid(&format!("{ident} {}", r["issue"])), "type": r["type"],
                            "relatedIssue": brief(other)}))
            })
            .collect();
        let all_comments = i["comments"].as_array().map(Vec::as_slice).unwrap_or_default();
        let comments: Vec<Value> =
            all_comments.iter().take(comments).map(|c| self.comment(ident, c)).collect();
        let branch_user = self
            .user(i["assignee"].as_str().unwrap_or_default())
            .and_then(|u| u["displayName"].as_str())
            .unwrap_or("unassigned");
        let m = d.as_object_mut().expect("row is an object");
        let field = |k: &str| i.get(k).cloned().unwrap_or(Value::Null);
        for k in
            ["description", "dueDate", "estimate", "startedAt", "completedAt", "canceledAt", "archivedAt"]
        {
            m.insert(k.into(), field(k));
        }
        let parent = i["parent"].as_str().and_then(|p| self.issue(p)).map_or(Value::Null, brief);
        let branch = format!(
            "{branch_user}/{}-{}",
            ident.to_ascii_lowercase(),
            slug(i["title"].as_str().unwrap_or(""))
        );
        m.insert("branchName".into(), json!(branch));
        m.insert("creator".into(), self.user_ref(&i["creator"]));
        m.insert("parent".into(), parent);
        m.insert("children".into(), json!(children));
        m.insert("relations".into(), json!(relations));
        m.insert("attachments".into(), i.get("attachments").cloned().unwrap_or(json!([])));
        m.insert("comments".into(), json!(comments));
        d
    }

    fn not_found(id: &str) -> Output {
        Output::failed(&format!("not found: issue {id} (or this API key cannot see it)"))
    }

    fn issue_show(&self, id: &str, args: &Args) -> Result<Output, String> {
        args.only(&["comments", "fields", "signed-urls"])?;
        let comments = match args.one("comments") {
            Some(n) => n.parse().map_err(|_| format!("--comments: not a number: {n}"))?,
            None => 20,
        };
        let Some(i) = self.issue(id) else {
            return Ok(Self::not_found(id));
        };
        let mut d = self.detail(i, comments);
        let secs: u64 = match args.one("signed-urls") {
            Some(n) => n.parse().map_err(|_| format!("--signed-urls: not a number: {n}"))?,
            None => 0,
        };
        if secs > 0 {
            sign_all(&mut d, secs);
        }
        let d = args.one("fields").map_or(d.clone(), |f| pick(&d, f));
        Ok(Output::ok(json!({"issue": d})))
    }

    fn comment_list(&self, id: &str, args: &Args) -> Result<Output, String> {
        args.only(&["limit", "cursor", "fields"])?;
        let limit = args.limit(50)?;
        let Some(i) = self.issue(id) else {
            return Ok(Self::not_found(id));
        };
        let ident = i["identifier"].as_str().unwrap_or_default();
        let all = i["comments"].as_array().into_iter().flatten().map(|c| self.comment(ident, c)).collect();
        Ok(Output::page(pick_all(all, args.one("fields")), limit, args.one("cursor")))
    }

    fn issue_list(&self, args: &Args) -> Result<Output, String> {
        args.only(&[
            "team",
            "assignee",
            "state",
            "state-type",
            "label",
            "any-label",
            "without-label",
            "project",
            "cycle",
            "query",
            "updated-since",
            "sort",
            "limit",
            "cursor",
            "fields",
        ])?;
        let limit = args.limit(25)?;
        let since = args.one("updated-since").map(|w| since(w, self.now)).transpose()?;
        let cycle = match args.one("cycle") {
            None => None,
            Some("current") => Some(self.current_cycle().unwrap_or(Value::Null)),
            Some("next") => {
                Some(self.current_cycle().and_then(|c| Some(json!(c.as_i64()? + 1))).unwrap_or(Value::Null))
            }
            Some(n) => Some(json!(n.parse::<i64>().map_err(|_| format!("--cycle: not a cycle: {n}"))?)),
        };
        let viewer = self.seed["viewer"].as_str().unwrap_or_default();
        let query = args.one("query").map(str::to_lowercase);
        let mut issues: Vec<&Value> = self
            .issues()
            .iter()
            // Archived issues drop out of lists, as in Linear.
            .filter(|i| i["archivedAt"].is_null())
            .filter(|i| {
                let labels: Vec<&str> =
                    i["labels"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                let state_type =
                    self.named("states", i["state"].as_str().unwrap_or_default()).map(|s| &s["type"]);
                let text = || {
                    let comments =
                        i["comments"].as_array().into_iter().flatten().map(|c| c["body"].to_string());
                    [i["title"].to_string(), i["description"].to_string()]
                        .into_iter()
                        .chain(comments)
                        .collect::<String>()
                        .to_lowercase()
                };
                args.one("team").is_none_or(|t| self.team()["key"] == t)
                    && args.one("assignee").is_none_or(|a| match a {
                        "me" => i["assignee"] == viewer,
                        "none" => i["assignee"].is_null(),
                        email => i["assignee"] == email,
                    })
                    && (args.all("state").is_empty()
                        || args.all("state").iter().any(|s| i["state"] == s.as_str()))
                    && (args.all("state-type").is_empty()
                        || args.all("state-type").iter().any(|t| state_type.is_some_and(|s| s == t.as_str())))
                    && args.all("label").iter().all(|l| labels.contains(&l.as_str()))
                    && (args.all("any-label").is_empty()
                        || args.all("any-label").iter().any(|l| labels.contains(&l.as_str())))
                    && !args.all("without-label").iter().any(|l| labels.contains(&l.as_str()))
                    && args.one("project").is_none_or(|p| i["project"] == p)
                    && cycle.as_ref().is_none_or(|c| &i["cycle"] == c)
                    && since.as_ref().is_none_or(|s| i["updatedAt"].as_str().is_some_and(|u| u >= s.as_str()))
                    && query.as_ref().is_none_or(|q| text().contains(q.as_str()))
            })
            .collect();
        let updated = |i: &Value| i["updatedAt"].as_str().unwrap_or_default().to_string();
        match args.one("sort").unwrap_or("updated") {
            "updated" => issues.sort_by_key(|i| std::cmp::Reverse(updated(i))),
            "created" => issues
                .sort_by_key(|i| std::cmp::Reverse(i["createdAt"].as_str().unwrap_or_default().to_string())),
            "priority" => issues.sort_by_key(|i| {
                let p = i["priority"].as_i64().unwrap_or(0);
                (if p == 0 { 5 } else { p }, std::cmp::Reverse(updated(i)))
            }),
            s => return Err(format!("--sort: not one of updated, created, priority: {s}")),
        }
        let rows = issues.into_iter().map(|i| self.row(i)).collect();
        Ok(Output::page(pick_all(rows, args.one("fields")), limit, args.one("cursor")))
    }
}

/// Sign every upload URL in the issue's text, as `--signed-urls` does: the
/// demo's signature is a marker, not a real JWT.
fn sign_all(issue: &mut Value, secs: u64) {
    let sign = |v: &mut Value| {
        if let Some(t) = v.as_str() {
            *v = json!(sign_uploads(t, secs));
        }
    };
    sign(&mut issue["description"]);
    if let Some(comments) = issue["comments"].as_array_mut() {
        comments.iter_mut().for_each(|c| sign(&mut c["body"]));
    }
}

fn sign_uploads(text: &str, secs: u64) -> String {
    const UPLOADS: &str = "https://uploads.linear.app/";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(UPLOADS) {
        let url_len = rest[at..]
            .find(|c: char| c == ')' || c == '"' || c == '>' || c.is_whitespace())
            .unwrap_or(rest.len() - at);
        out.push_str(&rest[..at + url_len]);
        out.push_str(&format!("?signature=demo-{secs}"));
        rest = &rest[at + url_len..];
    }
    out.push_str(rest);
    out
}

/// `--updated-since`: a date, an RFC 3339 time, `7d` or `24h`.
fn since(when: &str, now: DateTime<Utc>) -> Result<String, String> {
    let bad = || format!("--updated-since: not a date, a time, Nd or Nh: {when}");
    let t = if let Some(d) = when.strip_suffix('d') {
        now - Duration::days(d.parse().map_err(|_| bad())?)
    } else if let Some(h) = when.strip_suffix('h') {
        now - Duration::hours(h.parse().map_err(|_| bad())?)
    } else if let Ok(t) = DateTime::parse_from_rfc3339(when) {
        t.with_timezone(&Utc)
    } else {
        let d = NaiveDate::parse_from_str(when, "%Y-%m-%d").map_err(|_| bad())?;
        d.and_hms_opt(0, 0, 0).expect("midnight exists").and_utc()
    };
    Ok(t.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// Move every time in the seed by `by`; dates move by whole days.
fn shift(v: &mut Value, by: Duration) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if k == "anchor" {
                    continue;
                }
                if let Value::String(s) = x {
                    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
                        *s = (t.with_timezone(&Utc) + by).to_rfc3339_opts(SecondsFormat::Millis, true);
                    } else if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
                        *s = (d + Duration::days(by.num_days())).format("%Y-%m-%d").to_string();
                    }
                } else {
                    shift(x, by);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| shift(x, by)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The anchor itself, so seed times come out unchanged.
    fn anchor() -> DateTime<Utc> {
        "2026-10-26T12:00:00Z".parse().unwrap()
    }

    fn go(cmd: &str) -> Output {
        let argv: Vec<String> = shlex::split(cmd).unwrap();
        run(&argv, anchor())
    }

    fn json_of(cmd: &str) -> Value {
        let out = go(cmd);
        assert_eq!(out.code, 0, "{cmd}: {}", out.stdout);
        serde_json::from_str(&out.stdout).unwrap()
    }

    fn idents(v: &Value) -> Vec<String> {
        v["items"].as_array().unwrap().iter().map(|i| i["identifier"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn output_is_one_compact_line() {
        let out = go("issue list --json");
        assert_eq!(out.stdout.lines().count(), 1);
    }

    #[test]
    fn version_and_describe() {
        assert!(go("--version").stdout.starts_with("linearctl 0.3.0"));
        let d = crate::contract::Describe::from_json(&go("describe --json").stdout).unwrap();
        assert_eq!(d.version, "0.3.0");
    }

    #[test]
    fn auth_status_is_the_viewer() {
        let v = json_of("auth status --json");
        assert_eq!(v["user"]["name"], "Maya Patel");
        assert_eq!(v["organization"]["urlKey"], "northwind");
        assert_eq!(v["rateLimit"]["requests"]["remaining"], 2500);
    }

    #[test]
    fn lookups() {
        assert_eq!(json_of("team list --json")["items"][0]["key"], "ENG");
        assert_eq!(json_of("user list --active --json")["shown"], 5);
        let states = json_of("state list --team ENG --json");
        assert_eq!(states["items"][3]["name"], "In Progress");
        assert_eq!(states["items"][3]["position"], 3);
        assert_eq!(json_of("label list --json")["items"][0]["color"], "#eb5757");
        let projects = json_of("project list --status started --json");
        assert_eq!(projects["shown"], 2);
        assert_eq!(projects["items"][0]["status"]["name"], "Started");
        let cycles = json_of("cycle list --json");
        let active: Vec<_> =
            cycles["items"].as_array().unwrap().iter().filter(|c| c["isActive"] == true).collect();
        assert_eq!(active[0]["number"], 24);
        assert_eq!(cycles["items"][2]["isNext"], true);
    }

    #[test]
    fn issue_list_default_is_most_recently_updated_first() {
        let v = json_of("issue list --json");
        assert_eq!(idents(&v)[..3], ["ENG-158", "ENG-142", "ENG-143"]);
        assert_eq!(v["shown"], 17);
        assert_eq!(v["truncated"], false);
        assert_eq!(v["total"], Value::Null);
    }

    #[test]
    fn issue_row_shape_matches_linearctl() {
        let v = json_of("issue list --limit 1 --json");
        let row = &v["items"][0];
        let mut keys: Vec<&str> = row.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "assignee",
                "createdAt",
                "cycle",
                "delegate",
                "id",
                "identifier",
                "labels",
                "priority",
                "priorityLabel",
                "project",
                "projectMilestone",
                "state",
                "team",
                "title",
                "updatedAt",
                "url"
            ]
        );
        assert_eq!(row["state"]["color"], "#e2e2e2");
        assert_eq!(row["priorityLabel"], "Urgent");
        assert_eq!(
            row["url"],
            "https://linear.app/northwind/issue/ENG-158/customers-in-brazil-can-t-pay-with-pix"
        );
    }

    #[test]
    fn paging() {
        let first = go("issue list --limit 5 --json");
        assert!(first.stderr.contains("5 shown, more remain; --cursor"));
        let v: Value = serde_json::from_str(&first.stdout).unwrap();
        assert_eq!(v["truncated"], true);
        let next =
            json_of(&format!("issue list --limit 5 --cursor {} --json", v["nextCursor"].as_str().unwrap()));
        assert_eq!(idents(&next)[0], idents(&json_of("issue list --json"))[5]);
        assert_eq!(json_of("issue list --limit 0 --json")["shown"], 17);
        assert_eq!(go("issue list --cursor nope --json").code, 1);
    }

    #[test]
    fn filters() {
        let ids = |cmd: &str| idents(&json_of(cmd));
        assert_eq!(ids("issue list --assignee me --json").len(), 4);
        assert_eq!(ids("issue list --assignee none --json"), ["ENG-157", "ENG-155", "ENG-156"]);
        assert_eq!(ids("issue list --assignee sofia@northwind.example --json").len(), 2);
        assert_eq!(ids("issue list --state-type triage --json").len(), 2);
        assert_eq!(ids("issue list --state Done --state Backlog --json").len(), 3);
        assert_eq!(ids("issue list --label Bug --label 'Customer reported' --json"), ["ENG-158", "ENG-142"]);
        assert_eq!(ids("issue list --any-label Security --any-label Performance --json").len(), 2);
        assert!(!ids("issue list --without-label Bug --json").contains(&"ENG-142".to_string()));
        assert_eq!(ids("issue list --project 'Mobile 3.0' --json").len(), 2);
        assert_eq!(ids("issue list --cycle current --json").len(), 8);
        assert_eq!(ids("issue list --cycle next --json"), ["ENG-151", "ENG-153"]);
        assert_eq!(ids("issue list --cycle 23 --json"), ["ENG-131"]);
        assert_eq!(ids("issue list --team ENG --json").len(), 17);
        assert!(ids("issue list --team WEB --json").is_empty());
        assert_eq!(ids("issue list --query 'n+1' --json"), ["ENG-149"]);
        assert_eq!(ids("issue list --query 'android 3.4.0' --json"), ["ENG-142"], "searches comments");
        assert_eq!(ids("issue list --updated-since 24h --json").len(), 8);
        assert_eq!(ids("issue list --updated-since 2026-10-26 --json").len(), 5);
        assert_eq!(ids("issue list --updated-since 2026-10-26T11:00:00Z --json").len(), 2);
        assert_eq!(ids("issue list --updated-since 30d --json").len(), 17);
    }

    #[test]
    fn sorting() {
        let ids = |cmd: &str| idents(&json_of(cmd));
        let p = ids("issue list --sort priority --json");
        assert_eq!(p[..2], ["ENG-158", "ENG-142"]);
        assert_eq!(p.last().unwrap(), "ENG-120", "no priority goes last");
        assert_eq!(ids("issue list --sort created --json")[0], "ENG-158");
        assert_eq!(go("issue list --sort title --json").code, 2);
    }

    #[test]
    fn fields() {
        let v = json_of("issue list --limit 1 --fields identifier,title --json");
        assert_eq!(v["items"][0].as_object().unwrap().len(), 2);
        let s = json_of("issue show ENG-142 --fields updatedAt --comments=0 --json");
        assert_eq!(s["issue"].as_object().unwrap().keys().collect::<Vec<_>>(), ["updatedAt"]);
    }

    #[test]
    fn issue_show() {
        let v = json_of("issue show ENG-142 --signed-urls 300 --json");
        let i = &v["issue"];
        assert_eq!(i["title"], "Checkout fails when a coupon code has trailing whitespace");
        assert_eq!(i["parent"]["identifier"], "ENG-120");
        assert_eq!(i["children"].as_array().unwrap().len(), 2);
        assert_eq!(i["children"][0]["state"]["type"], "started");
        assert_eq!(i["relations"][0]["relatedIssue"]["identifier"], "ENG-131");
        assert_eq!(i["attachments"][0]["url"], "https://github.com/northwind/shop/pull/2841");
        assert_eq!(i["creator"]["name"], "Jonas Berg");
        assert_eq!(i["branchName"], "maya/eng-142-checkout-fails-when-a-coupon-code-has-trailing-whitespace");
        assert_eq!(i["comments"].as_array().unwrap().len(), 4);
        assert_eq!(i["comments"][1]["parent"]["id"], i["comments"][0]["id"]);
        assert_eq!(i["comments"][0]["parent"], Value::Null);
        assert!(i["description"].as_str().unwrap().contains("## Steps to reproduce"));
        assert_eq!(i["dueDate"], Value::Null);
    }

    #[test]
    fn issue_show_by_alias_uuid_and_comment_count() {
        let id = json_of("issue show ENG-142 --json")["issue"]["id"].as_str().unwrap().to_string();
        assert_eq!(json_of(&format!("issue view {id} --json"))["issue"]["identifier"], "ENG-142");
        assert_eq!(
            json_of("issue show ENG-142 --comments=1 --json")["issue"]["comments"].as_array().unwrap().len(),
            1
        );
        assert_eq!(go("issue show ENG-142 --comments=x --json").code, 2);
        let unassigned = json_of("issue show ENG-156 --json");
        assert!(unassigned["issue"]["branchName"].as_str().unwrap().starts_with("unassigned/"));
    }

    #[test]
    fn not_found() {
        let out = go("issue show ENG-999 --json");
        assert_eq!(out.code, 1);
        let v: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(v["error"], "not found: issue ENG-999 (or this API key cannot see it)");
        assert_eq!(v["usage"], false);
        assert_eq!(go("comment list ENG-999 --json").code, 1);
    }

    #[test]
    fn comment_list() {
        let v = json_of("comment list ENG-142 --limit 2 --json");
        assert_eq!(v["shown"], 2);
        assert_eq!(v["truncated"], true);
        assert_eq!(v["items"][0]["user"]["name"], "Aiko Tanaka");
        assert!(v["items"][0]["url"].as_str().unwrap().contains("ENG-142#comment-"));
    }

    #[test]
    fn writes_are_refused() {
        for cmd in [
            "issue create --team ENG --title x --id 11111111-2222-4333-8444-555555555555 --yes --json",
            "issue update ENG-142 --priority 4 --yes --json",
            "issue archive ENG-142 --yes --json",
            "comment create ENG-142 --body hi --id 11111111-2222-4333-8444-555555555555 --yes --json",
            "file download https://uploads.linear.app/x --yes --json",
            "config set default-team ENG --yes --json",
            "auth login --yes --json",
        ] {
            let out = go(cmd);
            assert_eq!(out.code, 1, "{cmd}");
            assert!(out.stdout.contains("read-only"), "{cmd}");
        }
    }

    #[test]
    fn usage_errors() {
        for cmd in [
            "issue list --bogus x --json",
            "issue list --limit",
            "issue list --limit many",
            "issue list --cycle soon",
            "issue list --updated-since yesterday",
            "issue list --updated-since xd",
            "issue list --updated-since xh",
            "auth status --bogus 1",
            "frobnicate",
        ] {
            let out = go(cmd);
            assert_eq!(out.code, 2, "{cmd}");
            assert!(out.stdout.contains("\"usage\":true"), "{cmd}");
            assert!(out.stderr.starts_with("linearctl: "), "{cmd}");
        }
    }

    #[test]
    fn times_follow_now() {
        let later = anchor() + Duration::days(10);
        let v: Value =
            serde_json::from_str(&run(&["issue".into(), "show".into(), "ENG-140".into()], later).stdout)
                .unwrap();
        assert_eq!(v["issue"]["updatedAt"], "2026-11-05T08:30:00.000Z");
        assert_eq!(v["issue"]["dueDate"], "2026-12-11");
    }

    #[test]
    fn uuids_are_stable_and_shaped() {
        let u = uuid("ENG-142");
        assert_eq!(u, uuid("ENG-142"));
        assert_ne!(u, uuid("ENG-143"));
        let parts: Vec<usize> = u.split('-').map(str::len).collect();
        assert_eq!(parts, [8, 4, 4, 4, 12]);
        assert_eq!(&u[14..15], "4");
    }

    #[test]
    fn signed_urls() {
        let plain = json_of("issue show ENG-142 --json");
        assert!(plain["issue"]["description"].as_str().unwrap().contains("checkout-error.png)"));
        let signed = json_of("issue show ENG-142 --signed-urls 300 --json");
        assert!(
            signed["issue"]["description"]
                .as_str()
                .unwrap()
                .contains("checkout-error.png?signature=demo-300)")
        );
        assert_eq!(go("issue show ENG-142 --signed-urls soon --json").code, 2);
        assert_eq!(
            sign_uploads("a https://uploads.linear.app/x b", 5),
            "a https://uploads.linear.app/x?signature=demo-5 b"
        );
        assert_eq!(
            sign_uploads("https://uploads.linear.app/y", 5),
            "https://uploads.linear.app/y?signature=demo-5"
        );
        assert_eq!(sign_uploads("none", 5), "none");
    }

    #[test]
    fn faults() {
        let argv = vec!["auth".to_string(), "status".to_string()];
        let f = |spec: &str| run_with_fault(&argv, anchor(), Some(spec), None);
        assert_eq!(run_with_fault(&argv, anchor(), None, None).code, 0);
        assert_eq!(run_with_fault(&argv, anchor(), Some(""), None).code, 0);
        assert_eq!(f("exit:5").code, 5);
        assert!(f("exit:2").stdout.contains("\"usage\":true"));
        assert!(f("exit:6").stdout.contains("retryAt"));
        assert_eq!(f("exit:x").code, 2);
        assert_eq!(f("sleep:1").code, 0);
        assert_eq!(f("sleep:x").code, 2);
        assert_eq!(f("garbage").stdout, "this is not json\n");
        assert_eq!(f("crash").code, 101);
        assert_eq!(f("nonsense").code, 2);
        let dir = std::env::temp_dir().join(format!("fake-busy-{}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        let spec = format!("busy-once:{}", dir.display());
        assert_eq!(f(&spec).code, 4);
        assert_eq!(f(&spec).code, 0);
        std::fs::remove_file(&dir).unwrap();
    }

    #[test]
    fn helpers() {
        assert_eq!(slug("Search ignores accents: \"café\""), "search-ignores-accents-caf");
        assert_eq!(priority_label(0), "No priority");
        assert_eq!(priority_label(4), "Low");
        assert_eq!(priority_label(3), "Medium");
        assert_eq!(priority_label(2), "High");
    }
}
