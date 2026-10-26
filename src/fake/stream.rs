//! fake-linearctl's long-running commands: `serve --stdio` and `watch`.
//!
//! `serve` answers one JSON request a line, as linearctl 0.2+ does. `watch`
//! works like linearctl's poll mode: it lists what the filter matches, prints
//! `ready`, then compares the list again every tick (writes made through the
//! state file show up) and prints `upsert`, `remove` and `heartbeat` lines.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{DateTime, Utc, run_in};

/// `serve --stdio`: requests `{"id", "argv"}` in, answers
/// `{"id", "exit", "result", "stdout", "stderr"}` out, one line each, until
/// the input ends or the output closes.
pub fn serve(
    input: impl BufRead,
    mut out: impl Write,
    now: impl Fn() -> DateTime<Utc>,
    state: Option<&Path>,
) {
    for line in input.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let answer = answer(&line, now(), state);
        if writeln!(out, "{answer}").and_then(|()| out.flush()).is_err() {
            return;
        }
    }
}

fn refused(id: Value, msg: &str) -> Value {
    json!({"id": id, "exit": 2, "result": {"error": msg, "usage": true}, "stdout": null, "stderr": format!("linearctl: {msg}\n")})
}

fn answer(line: &str, now: DateTime<Utc>, state: Option<&Path>) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return refused(Value::Null, &format!("not a request: {e}")),
    };
    let id = req["id"].clone();
    let words = req["argv"]
        .as_array()
        .map(|a| a.iter().map(|w| w.as_str().map(str::to_string)).collect::<Option<Vec<_>>>());
    let Some(Some(argv)) = words else {
        return refused(id, "argv must be a list of strings");
    };
    if matches!(argv.first().map(String::as_str), Some("watch" | "serve")) {
        return refused(id, "serve runs one command at a time: run watch on its own");
    }
    if argv.iter().any(|w| w == "-") {
        return refused(id, "serve reads requests on stdin: pass a file, not -");
    }
    let out = run_in(&argv, now, state);
    let parsed: Option<Value> = serde_json::from_str(&out.stdout).ok();
    let stdout = if parsed.is_some() { Value::Null } else { json!(out.stdout) };
    json!({"id": id, "exit": out.code, "result": parsed.unwrap_or(Value::Null), "stdout": stdout, "stderr": out.stderr})
}

/// Flags only `watch` takes, and whether each has a value.
const WATCH_ONLY: &[&str] = &["heartbeat", "since", "mode", "poll-interval", "reconcile"];

/// What the filter matches now: rows by id.
fn snapshot(
    list: &[String],
    now: DateTime<Utc>,
    state: Option<&Path>,
) -> Result<BTreeMap<String, Value>, (i32, String)> {
    let out = run_in(list, now, state);
    let v: Value = serde_json::from_str(&out.stdout).map_err(|_| (out.code, out.stdout.clone()))?;
    if out.code != 0 {
        return Err((out.code, out.stdout));
    }
    let items = v["items"].as_array().cloned().unwrap_or_default();
    Ok(items.into_iter().map(|i| (i["id"].as_str().unwrap_or_default().to_string(), i)).collect())
}

/// `watch`: prints events until `keep_going` says stop (or the output
/// closes); the exit code. Each `tick` the list is compared again.
pub fn watch(
    argv: &[String],
    mut out: impl Write,
    now: impl Fn() -> DateTime<Utc>,
    state: Option<&Path>,
    mut keep_going: impl FnMut() -> bool,
    tick: Duration,
) -> i32 {
    // Split watch's own flags from issue list's.
    let mut list = vec!["issue".to_string(), "list".to_string()];
    let mut heartbeat = Duration::from_secs(30);
    let mut fields = None;
    let mut it = argv.iter();
    while let Some(w) = it.next() {
        let name = w.strip_prefix("--").unwrap_or("");
        if WATCH_ONLY.contains(&name) {
            let v = it.next().cloned().unwrap_or_default();
            if name == "heartbeat" {
                heartbeat = Duration::from_secs(v.parse().unwrap_or(30));
            }
        } else if name == "fields" {
            fields = it.next().cloned();
        } else if name != "json" {
            list.push(w.clone());
        }
    }
    list.extend(["--limit".into(), "0".into(), "--json".into()]);
    // Times in rows follow "now"; hold it still so rows compare equal.
    let start = now();
    let pick = |row: &Value| match &fields {
        Some(f) => super::pick(row, f),
        None => row.clone(),
    };
    let mut emit = |v: Value| writeln!(out, "{v}").and_then(|()| out.flush()).is_ok();
    let mut known = match snapshot(&list, start, state) {
        Ok(k) => k,
        Err((code, doc)) => {
            let _ = emit(serde_json::from_str(&doc).unwrap_or(json!({"error": doc})));
            return code;
        }
    };
    let at = |t: DateTime<Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    if !emit(
        json!({"event": "ready", "mode": "poll", "since": at(start), "known": known.len(), "everything": false}),
    ) {
        return 0;
    }
    let mut beat = Instant::now();
    while keep_going() {
        std::thread::sleep(tick);
        if beat.elapsed() >= heartbeat {
            beat = Instant::now();
            if !emit(json!({"event": "heartbeat", "at": at(now()), "mode": "poll", "rateLimit": {}})) {
                return 0;
            }
        }
        let Ok(current) = snapshot(&list, start, state) else {
            continue;
        };
        for (id, row) in &current {
            if known.get(id) != Some(row) && !emit(json!({"event": "upsert", "issue": pick(row)})) {
                return 0;
            }
        }
        for (id, row) in &known {
            if !current.contains_key(id) {
                let show =
                    run_in(&["issue".into(), "show".into(), id.clone(), "--json".into()], start, state);
                let archived = show.stdout.contains("\"archivedAt\":\"");
                let reason = if archived { "archived" } else { "filter" };
                if !emit(
                    json!({"event": "remove", "id": id, "identifier": row["identifier"], "reason": reason}),
                ) {
                    return 0;
                }
            }
        }
        known = current;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn now() -> DateTime<Utc> {
        "2026-10-26T12:00:00Z".parse().unwrap()
    }

    fn lines(out: &[u8]) -> Vec<Value> {
        String::from_utf8_lossy(out).lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn serve_answers_each_request_in_order() {
        let input = [
            r#"{"id":1,"argv":["issue","show","ENG-142","--fields","title","--comments=0","--json"]}"#,
            "",
            r#"{"id":"two","argv":["issue","show","ENG-999","--json"]}"#,
            r#"{"id":3,"argv":["--version"]}"#,
            r#"{"id":4,"argv":["watch","--json"]}"#,
            r#"{"id":5,"argv":["comment","create","ENG-1","--body-file","-","--dry-run","--json"]}"#,
            r#"{"id":6,"argv":"issue list"}"#,
            r#"not json"#,
        ]
        .join("\n");
        let mut out = Vec::new();
        serve(Cursor::new(input), &mut out, now, None);
        let a = lines(&out);
        assert_eq!(a.len(), 7);
        assert_eq!(a[0]["id"], 1);
        assert_eq!(a[0]["exit"], 0);
        assert_eq!(
            a[0]["result"]["issue"]["title"],
            "Checkout fails when a coupon code has trailing whitespace"
        );
        assert_eq!(a[0]["stdout"], Value::Null);
        assert_eq!((a[1]["id"].clone(), a[1]["exit"].clone()), (json!("two"), json!(1)));
        assert!(a[1]["result"]["error"].as_str().unwrap().starts_with("not found"));
        assert_eq!(a[2]["result"], Value::Null, "text output");
        assert!(a[2]["stdout"].as_str().unwrap().starts_with("linearctl 0.4.0"));
        for (i, needle) in
            [(3, "run watch on its own"), (4, "not -"), (5, "argv must be"), (6, "not a request")]
        {
            assert_eq!(a[i]["exit"], 2, "{i}");
            assert!(a[i]["result"]["error"].as_str().unwrap().contains(needle), "{i}: {}", a[i]);
        }
        assert_eq!(a[6]["id"], Value::Null);
    }

    #[test]
    fn serve_stops_when_the_output_closes() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        serve(
            Cursor::new("{\"id\":1,\"argv\":[\"--version\"]}\n{\"id\":2,\"argv\":[\"--version\"]}"),
            Closed,
            now,
            None,
        );
        // A bad line from the reader also ends it.
        serve(Cursor::new(vec![0xff, 0xfe, b'\n']), Vec::new(), now, None);
    }

    #[test]
    fn watch_reports_writes_made_meanwhile() {
        let state = std::env::temp_dir().join(format!("fake-watch-{}", std::process::id()));
        let _ = std::fs::remove_file(&state);
        let argv: Vec<String> = [
            "--assignee",
            "me",
            "--fields",
            "identifier,state",
            "--heartbeat",
            "0",
            "--mode",
            "poll",
            "--json",
        ]
        .map(String::from)
        .to_vec();
        let mut out = Vec::new();
        let mut n = 0;
        let st = state.clone();
        let code = watch(
            &argv,
            &mut out,
            now,
            Some(&state),
            move || {
                n += 1;
                let go = |cmd: &str| run_in(&shlex::split(cmd).unwrap(), now(), Some(&st));
                match n {
                    2 => assert_eq!(go("issue update ENG-142 --state Done --yes --json").code, 0),
                    3 => assert_eq!(go("issue update ENG-140 --assignee none --yes --json").code, 0),
                    4 => assert_eq!(go("issue archive ENG-120 --yes --json").code, 0),
                    _ => {}
                }
                n < 6
            },
            Duration::from_millis(1),
        );
        assert_eq!(code, 0);
        let ev = lines(&out);
        assert_eq!(ev[0]["event"], "ready");
        assert_eq!(ev[0]["known"], 4);
        assert_eq!(ev[0]["mode"], "poll");
        let upserts: Vec<&Value> = ev.iter().filter(|e| e["event"] == "upsert").collect();
        assert_eq!(upserts.len(), 1);
        assert_eq!(
            upserts[0]["issue"],
            json!({"identifier": "ENG-142", "state": {"id": upserts[0]["issue"]["state"]["id"], "name": "Done", "type": "completed", "color": "#5e6ad2"}})
        );
        let removes: Vec<(&str, &str)> = ev
            .iter()
            .filter(|e| e["event"] == "remove")
            .map(|e| (e["identifier"].as_str().unwrap(), e["reason"].as_str().unwrap()))
            .collect();
        assert_eq!(removes, [("ENG-140", "filter"), ("ENG-120", "archived")]);
        assert!(ev.iter().any(|e| e["event"] == "heartbeat"));
        std::fs::remove_file(&state).unwrap();
    }

    #[test]
    fn watch_with_a_bad_filter_ends_with_the_error() {
        let mut out = Vec::new();
        let code = watch(&["--sort".into(), "x".into()], &mut out, now, None, || true, Duration::ZERO);
        assert_eq!(code, 2);
        assert!(lines(&out)[0]["error"].as_str().unwrap().contains("--sort"));
    }

    #[test]
    fn watch_ends_when_the_output_closes() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(watch(&[], Closed, now, None, || panic!("never polled"), Duration::ZERO), 0);
    }
}
