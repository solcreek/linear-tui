//! fake-linearctl's write commands: `issue update`, `issue create`,
//! `issue archive` and `comment create`, with linearctl's contract:
//! exactly one of `--dry-run` and `--yes`; a dry run prints the input it
//! would send, with names resolved to ids; nothing to do is `changed: false`;
//! creates take `--id` with `--yes` and a rerun returns the existing item.
//!
//! Writes change the seed (in its own, unshifted times) and are kept in the
//! state file; without one, `--yes` is refused, as in linearctl's demo mode.

use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};

use super::{Args, DateTime, Output, SEED, Utc, Workspace, comment_uuid, issue_uuid, uuid};

/// The seed, from the state file when there is one.
pub(super) fn load_raw(state: Option<&Path>) -> Value {
    let text = state.and_then(|p| fs::read_to_string(p).ok());
    text.and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::from_str(SEED).expect("demo/workspace.json is valid JSON"))
}

pub(super) fn write(
    words: &[&str],
    args: &Args,
    now: DateTime<Utc>,
    state: Option<&Path>,
) -> Result<Output, String> {
    let dry = args.has("dry-run");
    if dry == args.has("yes") {
        return Err("exactly one of --dry-run and --yes".into());
    }
    let mut raw = load_raw(state);
    let result = match words {
        ["issue", "update", id] => update(&mut raw, id, args, dry, now)?,
        ["issue", "create"] => create(&mut raw, args, dry, now)?,
        ["issue", "archive", id] => archive(&mut raw, id, args, dry, true)?,
        ["issue", "unarchive", id] => archive(&mut raw, id, args, dry, false)?,
        ["comment", "delete", id] => delete_comment(&mut raw, id, args, dry)?,
        ["comment", "create", id] => comment(&mut raw, id, args, dry)?,
        _ => return Err(format!("unknown command: {}", words.join(" "))),
    };
    let (doc, changed, item) = match result {
        Done::Failed(msg) => return Ok(Output::failed(&msg)),
        Done::Doc { doc, changed, item } => (doc, changed, item),
    };
    let mut doc = doc;
    if !dry && changed {
        let Some(path) = state else {
            return Ok(Output::failed("the demo workspace is read-only"));
        };
        fs::write(path, serde_json::to_string_pretty(&raw).expect("JSON")).map_err(|e| e.to_string())?;
    }
    // The written item, as linearctl returns it after the mutation.
    if !dry {
        let ws = Workspace::from_seed(raw, now);
        match item {
            Item::Issue(ident) => {
                let i = ws.issue(&ident).expect("the issue just written");
                doc["issue"] = ws.row(i);
            }
            Item::Comment(issue, id) => {
                let i = ws.issue(&issue).expect("the issue just commented");
                let c = i["comments"].as_array().into_iter().flatten().find(|c| comment_uuid(c) == id);
                doc["comment"] = ws.comment(&issue, c.expect("the comment just written"));
            }
            Item::None => {}
        }
    }
    Ok(Output::ok(doc))
}

enum Item {
    Issue(String),
    Comment(String, String),
    None,
}

enum Done {
    Failed(String),
    Doc { doc: Value, changed: bool, item: Item },
}

fn not_found(what: &str, name: &str) -> Done {
    Done::Failed(format!("not found: {what} {name:?}"))
}

fn issue_mut<'a>(raw: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    raw["issues"].as_array_mut()?.iter_mut().find(|i| i["identifier"] == id || issue_uuid(i) == id)
}

/// A seed list's entry by name, ignoring case; its canonical name.
fn named(raw: &Value, kind: &str, name: &str) -> Option<String> {
    let found = raw[kind]
        .as_array()?
        .iter()
        .find(|x| x["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)));
    found.and_then(|x| x["name"].as_str()).map(str::to_string)
}

/// Ids as linearctl's demo derives them; must match `Workspace`.
fn state_id(name: &str) -> String {
    uuid(&format!("state {}", Value::from(name)))
}
fn label_id(name: &str) -> String {
    uuid(&format!("label {}", Value::from(name)))
}
fn user_id(email: &str) -> String {
    uuid(&Value::from(email).to_string())
}
fn project_id(name: &str) -> String {
    uuid(&format!("project {name}"))
}

/// `--priority`: 0-4, or none, urgent, high, medium, low.
fn priority(v: &str) -> Result<i64, String> {
    let names = ["none", "urgent", "high", "medium", "low"];
    v.parse::<i64>()
        .ok()
        .filter(|p| (0..=4).contains(p))
        .or_else(|| names.iter().position(|n| n.eq_ignore_ascii_case(v)).map(|p| p as i64))
        .ok_or_else(|| format!("--priority: not 0-4 or none, urgent, high, medium, low: {v}"))
}

/// `--description` or `--description-file` (likewise for `--body`).
fn text(args: &Args, flag: &str) -> Result<Option<String>, String> {
    if let Some(t) = args.one(flag) {
        return Ok(Some(t.to_string()));
    }
    match args.one(&format!("{flag}-file")) {
        Some("-") => Err(format!("--{flag}-file -: the demo reads no stdin; pass a file")),
        Some(path) => fs::read_to_string(path).map(Some).map_err(|e| format!("--{flag}-file {path}: {e}")),
        None => Ok(None),
    }
}

/// `--assignee`: me, none or an email; the email (or null).
fn assignee(raw: &Value, who: &str) -> Result<Value, Done> {
    match who {
        "me" => Ok(raw["viewer"].clone()),
        "none" => Ok(Value::Null),
        email => {
            let users = raw["users"].as_array().into_iter().flatten();
            let found = users.filter_map(|u| u["email"].as_str()).find(|e| e.eq_ignore_ascii_case(email));
            found.map(Value::from).ok_or_else(|| not_found("user", email))
        }
    }
}

/// The fields `update` and `create` share: the seed changes and the
/// mutation input, each flag resolved.
struct Fields {
    seed: Map<String, Value>,
    input: Map<String, Value>,
}

fn fields(raw: &Value, args: &Args, now: DateTime<Utc>) -> Result<Result<Fields, Done>, String> {
    let mut f = Fields { seed: Map::new(), input: Map::new() };
    // linearctl 0.6.0's --cycle, --estimate and --due-date, each with none.
    if let Some(c) = args.one("cycle") {
        if c == "none" {
            f.input.insert("cycleId".into(), Value::Null);
            f.seed.insert("cycle".into(), Value::Null);
        } else {
            let cycles: Vec<i64> =
                raw["cycles"].as_array().into_iter().flatten().filter_map(|c| c["number"].as_i64()).collect();
            let current = Workspace::from_seed(raw.clone(), now).current_cycle().and_then(|v| v.as_i64());
            let n = match c {
                "current" => current,
                "next" => current.map(|x| x + 1),
                "previous" => current.map(|x| x - 1),
                other => other
                    .parse()
                    .ok()
                    .or_else(|| cycles.iter().copied().find(|n| uuid(&format!("cycle {n}")) == other)),
            };
            let Some(n) = n.filter(|n| cycles.contains(n)) else { return Ok(Err(not_found("cycle", c))) };
            f.input.insert("cycleId".into(), json!(uuid(&format!("cycle {n}"))));
            f.seed.insert("cycle".into(), json!(n));
        }
    }
    if let Some(e) = args.one("estimate") {
        let v = if e == "none" {
            Value::Null
        } else {
            json!(e.parse::<i64>().map_err(|_| format!("--estimate: not a number or none: {e}"))?)
        };
        f.input.insert("estimate".into(), v.clone());
        f.seed.insert("estimate".into(), v);
    }
    if let Some(d) = args.one("due-date") {
        let (input, seed) = if d == "none" {
            (Value::Null, Value::Null)
        } else {
            let day = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .map_err(|_| format!("--due-date: not YYYY-MM-DD or none: {d}"))?;
            // The seed's dates move with "now"; store it where it lands on d.
            let anchor = DateTime::parse_from_rfc3339(raw["anchor"].as_str().unwrap_or_default())
                .map_or(now, |a| a.with_timezone(&Utc));
            let back = day - chrono::Duration::days((now - anchor).num_days());
            (json!(d), json!(back.format("%Y-%m-%d").to_string()))
        };
        f.input.insert("dueDate".into(), input);
        f.seed.insert("dueDate".into(), seed);
    }
    if let Some(t) = args.one("title") {
        f.seed.insert("title".into(), json!(t));
        f.input.insert("title".into(), json!(t));
    }
    if let Some(d) = text(args, "description")? {
        f.seed.insert("description".into(), json!(d));
        f.input.insert("description".into(), json!(d));
    }
    if let Some(p) = args.one("priority") {
        let p = priority(p)?;
        f.seed.insert("priority".into(), json!(p));
        f.input.insert("priority".into(), json!(p));
    }
    if let Some(s) = args.one("state") {
        let Some(name) = named(raw, "states", s) else {
            return Ok(Err(not_found("state", s)));
        };
        f.input.insert("stateId".into(), json!(state_id(&name)));
        f.seed.insert("state".into(), json!(name));
    }
    if let Some(a) = args.one("assignee") {
        let email = match assignee(raw, a) {
            Ok(e) => e,
            Err(d) => return Ok(Err(d)),
        };
        f.input.insert("assigneeId".into(), email.as_str().map_or(Value::Null, |e| json!(user_id(e))));
        f.seed.insert("assignee".into(), email);
    }
    if !args.all("label").is_empty() {
        let mut names = Vec::new();
        for l in args.all("label") {
            let Some(name) = named(raw, "labels", l) else {
                return Ok(Err(not_found("label", l)));
            };
            names.push(name);
        }
        f.input.insert("labelIds".into(), json!(names.iter().map(|n| label_id(n)).collect::<Vec<_>>()));
        f.seed.insert("labels".into(), json!(names));
    }
    if let Some(p) = args.one("project") {
        // none clears it, as linearctl 0.6.0 does.
        if p == "none" {
            f.input.insert("projectId".into(), Value::Null);
            f.seed.insert("project".into(), Value::Null);
        } else {
            let Some(name) = named(raw, "projects", p) else {
                return Ok(Err(not_found("project", p)));
            };
            f.input.insert("projectId".into(), json!(project_id(&name)));
            f.seed.insert("project".into(), json!(name));
        }
    }
    if let Some(p) = args.one("parent") {
        let Some(parent) = raw["issues"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|i| i["identifier"] == p || issue_uuid(i) == p)
        else {
            return Ok(Err(not_found("issue", p)));
        };
        f.input.insert("parentId".into(), json!(issue_uuid(parent)));
        f.seed.insert("parent".into(), parent["identifier"].clone());
    }
    Ok(Ok(f))
}

const UPDATE_FLAGS: &[&str] = &[
    "cycle",
    "estimate",
    "due-date",
    "title",
    "description",
    "description-file",
    "priority",
    "state",
    "assignee",
    "label",
    "add-label",
    "remove-label",
    "project",
    "parent",
    "dry-run",
    "yes",
];

fn update(raw: &mut Value, id: &str, args: &Args, dry: bool, now: DateTime<Utc>) -> Result<Done, String> {
    args.only(UPDATE_FLAGS)?;
    let f = match fields(raw, args, now)? {
        Ok(f) => f,
        Err(d) => return Ok(d),
    };
    let anchor = raw["anchor"].clone();
    let labels_known: Vec<(String, String)> = {
        let mut v = Vec::new();
        for (flag, kind) in [("add-label", "add"), ("remove-label", "remove")] {
            for l in args.all(flag) {
                let Some(name) = named(raw, "labels", l) else {
                    return Ok(not_found("label", l));
                };
                v.push((kind.to_string(), name));
            }
        }
        v
    };
    let Some(issue) = issue_mut(raw, id) else {
        return Ok(Done::Failed(format!("not found: issue {id} (or this API key cannot see it)")));
    };
    let ident = issue["identifier"].as_str().unwrap_or_default().to_string();
    let mut input = Map::new();
    let mut seed = Map::new();
    let current_labels: Vec<String> = issue["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l.as_str().map(str::to_string))
        .collect();
    for (k, v) in &f.seed {
        let differs = match k.as_str() {
            "labels" => {
                let mut want: Vec<&str> =
                    v.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                let mut have: Vec<&str> = current_labels.iter().map(String::as_str).collect();
                want.sort_unstable();
                have.sort_unstable();
                want != have
            }
            _ => issue.get(k).unwrap_or(&Value::Null) != v,
        };
        if differs {
            seed.insert(k.clone(), v.clone());
            let input_key = match k.as_str() {
                "state" => "stateId",
                "cycle" => "cycleId",
                "assignee" => "assigneeId",
                "labels" => "labelIds",
                "project" => "projectId",
                "parent" => "parentId",
                other => other,
            };
            input.insert(input_key.into(), f.input[input_key].clone());
        }
    }
    let mut labels = seed
        .get("labels")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|l| l.as_str().map(str::to_string)).collect())
        .unwrap_or(current_labels.clone());
    let (mut added, mut removed) = (Vec::new(), Vec::new());
    for (kind, name) in labels_known {
        let has = labels.contains(&name);
        if kind == "add" && !has {
            added.push(label_id(&name));
            labels.push(name);
        } else if kind == "remove" && has {
            removed.push(label_id(&name));
            labels.retain(|l| *l != name);
        }
    }
    if !added.is_empty() {
        input.insert("addedLabelIds".into(), json!(added));
    }
    if !removed.is_empty() {
        input.insert("removedLabelIds".into(), json!(removed));
    }
    if labels != current_labels {
        seed.insert("labels".into(), json!(labels));
    }
    let changed = !input.is_empty();
    if changed && !dry {
        for (k, v) in seed {
            issue[k.as_str()] = v;
        }
        issue["updatedAt"] = anchor;
    }
    let doc = json!({"changed": changed, "dryRun": dry, "identifier": ident, "input": input});
    // Like linearctl: the issue comes back only when it was changed.
    let item = if changed { Item::Issue(ident) } else { Item::None };
    Ok(Done::Doc { doc, changed, item })
}

const CREATE_FLAGS: &[&str] = &[
    "cycle",
    "estimate",
    "due-date",
    "team",
    "title",
    "description",
    "description-file",
    "priority",
    "state",
    "assignee",
    "label",
    "project",
    "parent",
    "id",
    "dry-run",
    "yes",
];

fn create(raw: &mut Value, args: &Args, dry: bool, now: DateTime<Utc>) -> Result<Done, String> {
    args.only(CREATE_FLAGS)?;
    let team = args.one("team").ok_or("--team is required")?.to_string();
    let title = args.one("title").filter(|t| !t.trim().is_empty()).ok_or("--title is required")?.to_string();
    if !dry && args.one("id").is_none() {
        return Err("--id is required with --yes (take it from --dry-run)".into());
    }
    let id = args.one("id").map_or_else(|| uuid(&format!("new issue {title}")), str::to_string);
    let teams = raw["teams"].as_array().into_iter().flatten();
    let Some(team) = teams.filter_map(|t| t["key"].as_str()).find(|k| k.eq_ignore_ascii_case(&team)) else {
        return Ok(not_found("team", &team));
    };
    let team = team.to_string();
    if let Some(existing) = raw["issues"].as_array().into_iter().flatten().find(|i| issue_uuid(i) == id) {
        let ident = existing["identifier"].as_str().unwrap_or_default().to_string();
        let doc = json!({"changed": false, "dryRun": dry, "id": id});
        return Ok(Done::Doc { doc, changed: false, item: Item::Issue(ident) });
    }
    let f = match fields(raw, args, now)? {
        Ok(f) => f,
        Err(d) => return Ok(d),
    };
    let mut input = f.input;
    input.insert("id".into(), json!(id));
    input.insert("teamId".into(), json!(uuid(&format!("team {}", Value::from(team.as_str())))));
    let doc = json!({"changed": true, "dryRun": dry, "id": id, "input": input});
    if dry {
        return Ok(Done::Doc { doc, changed: true, item: Item::None });
    }
    let next = raw["issues"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i["identifier"].as_str()?.strip_prefix(&format!("{team}-"))?.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let ident = format!("{team}-{next}");
    let backlog = raw["states"].as_array().into_iter().flatten().find(|s| s["type"] == "backlog");
    let mut issue = json!({
        "identifier": ident, "uuid": id, "title": title, "priority": 0,
        "state": backlog.map_or(Value::Null, |s| s["name"].clone()),
        "creator": raw["viewer"], "labels": [], "createdAt": raw["anchor"], "updatedAt": raw["anchor"],
        "description": "", "comments": [],
    });
    for (k, v) in f.seed {
        issue[k.as_str()] = v;
    }
    raw["issues"].as_array_mut().expect("issues").insert(0, issue);
    Ok(Done::Doc { doc, changed: true, item: Item::Issue(ident) })
}

/// `issue archive`, or `issue unarchive` when `archive` is false.
fn archive(raw: &mut Value, id: &str, args: &Args, dry: bool, archive: bool) -> Result<Done, String> {
    args.only(&["dry-run", "yes"])?;
    let anchor = raw["anchor"].clone();
    let key = raw["organization"]["urlKey"].as_str().unwrap_or_default().to_string();
    let Some(issue) = issue_mut(raw, id) else {
        return Ok(Done::Failed(format!("not found: issue {id} (or this API key cannot see it)")));
    };
    let ident = issue["identifier"].as_str().unwrap_or_default().to_string();
    let changed = issue["archivedAt"].is_null() == archive;
    if changed && !dry {
        issue["archivedAt"] = if archive { anchor } else { Value::Null };
    }
    let url = format!("https://linear.app/{key}/issue/{ident}");
    let doc = json!({"changed": changed, "dryRun": dry, "identifier": ident, "url": url});
    Ok(Done::Doc { doc, changed, item: Item::None })
}

/// `comment delete`: one not there is changed: false, found: false.
fn delete_comment(raw: &mut Value, id: &str, args: &Args, dry: bool) -> Result<Done, String> {
    args.only(&["dry-run", "yes"])?;
    let key = raw["organization"]["urlKey"].as_str().unwrap_or_default().to_string();
    let issues = raw["issues"].as_array_mut().into_iter().flatten();
    for issue in issues {
        let ident = issue["identifier"].as_str().unwrap_or_default().to_string();
        let Some(comments) = issue["comments"].as_array_mut() else { continue };
        if let Some(at) = comments.iter().position(|c| comment_uuid(c) == id) {
            if !dry {
                comments.remove(at);
            }
            let url = format!("https://linear.app/{key}/issue/{ident}#comment-{}", &id[..8.min(id.len())]);
            let doc = json!({"changed": true, "dryRun": dry, "found": true, "id": id, "url": url});
            return Ok(Done::Doc { doc, changed: true, item: Item::None });
        }
    }
    let doc = json!({"changed": false, "dryRun": dry, "found": false, "id": id});
    Ok(Done::Doc { doc, changed: false, item: Item::None })
}

fn comment(raw: &mut Value, issue_id: &str, args: &Args, dry: bool) -> Result<Done, String> {
    args.only(&["body", "body-file", "id", "parent", "dry-run", "yes"])?;
    let body =
        text(args, "body")?.filter(|b| !b.trim().is_empty()).ok_or("--body or --body-file is required")?;
    if !dry && args.one("id").is_none() {
        return Err("--id is required with --yes (take it from --dry-run)".into());
    }
    let id = args.one("id").map_or_else(|| uuid(&format!("new comment {body}")), str::to_string);
    let viewer = raw["viewer"].clone();
    let anchor = raw["anchor"].clone();
    let Some(issue) = issue_mut(raw, issue_id) else {
        return Ok(Done::Failed(format!("not found: issue {issue_id} (or this API key cannot see it)")));
    };
    let ident = issue["identifier"].as_str().unwrap_or_default().to_string();
    let issue_uuid = issue_uuid(issue);
    if issue["comments"].as_array().into_iter().flatten().any(|c| comment_uuid(c) == id) {
        let doc = json!({"changed": false, "dryRun": dry, "id": id});
        return Ok(Done::Doc { doc, changed: false, item: Item::Comment(ident, id) });
    }
    // --parent: a comment of this issue; a reply joins its root's thread.
    let parent = match args.one("parent") {
        None => None,
        Some(p) => {
            let comments = issue["comments"].as_array().into_iter().flatten();
            let Some(c) = comments.clone().find(|c| comment_uuid(c) == p) else {
                return Ok(not_found("comment", p));
            };
            let root = match (c["parentUuid"].as_str(), c["parent"].as_str()) {
                (Some(r), _) => r.to_string(),
                (None, Some(seed)) => {
                    comments.clone().find(|x| x["id"] == seed).map_or(p.to_string(), comment_uuid)
                }
                (None, None) => p.to_string(),
            };
            Some(root)
        }
    };
    let mut input = json!({"body": body, "id": id, "issueId": issue_uuid});
    if let Some(p) = &parent {
        input["parentId"] = json!(p);
    }
    let doc = json!({"changed": true, "dryRun": dry, "id": id, "input": input});
    if !dry {
        if !issue["comments"].is_array() {
            issue["comments"] = json!([]);
        }
        let mut c = json!({"id": id, "uuid": id, "user": viewer, "createdAt": anchor, "body": body});
        if let Some(p) = parent {
            c["parentUuid"] = json!(p);
        }
        issue["comments"].as_array_mut().expect("comments").push(c);
        issue["updatedAt"] = anchor;
    }
    Ok(Done::Doc { doc, changed: true, item: if dry { Item::None } else { Item::Comment(ident, id) } })
}

#[cfg(test)]
mod tests {
    use super::super::{STATE_ENV, run_in};
    use super::*;

    fn now() -> DateTime<Utc> {
        "2026-10-26T12:00:00Z".parse().unwrap()
    }

    struct State(std::path::PathBuf);
    impl Drop for State {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn state(name: &str) -> State {
        let p = std::env::temp_dir().join(format!("fake-state-{name}-{}", std::process::id()));
        let _ = fs::remove_file(&p);
        State(p)
    }

    fn go(cmd: &str, st: Option<&State>) -> (i32, Value) {
        let argv: Vec<String> = shlex::split(cmd).unwrap();
        let out = run_in(&argv, now(), st.map(|s| s.0.as_path()));
        (out.code, serde_json::from_str(&out.stdout).unwrap_or(Value::Null))
    }

    #[test]
    fn the_env_name() {
        assert_eq!(STATE_ENV, "FAKE_LINEARCTL_STATE");
    }

    #[test]
    fn update_dry_run_resolves_names_to_ids() {
        let (code, v) =
            go("issue update ENG-142 --state 'in review' --priority high --label bug --dry-run --json", None);
        assert_eq!(code, 0);
        assert_eq!(v["changed"], true);
        assert_eq!(v["dryRun"], true);
        assert_eq!(v["identifier"], "ENG-142");
        let input = v["input"].as_object().unwrap();
        assert_eq!(input["stateId"], json!(state_id("In Review")));
        assert_eq!(input["priority"], 2);
        assert_eq!(input["labelIds"], json!([label_id("Bug")]));
        let mut keys: Vec<&String> = input.keys().collect();
        keys.sort();
        assert_eq!(keys, ["labelIds", "priority", "stateId"]);
        // The ids match what reads report.
        let (_, states) = go("state list --json", None);
        assert!(states["items"].as_array().unwrap().iter().any(|s| s["id"] == input["stateId"]));
    }

    #[test]
    fn nothing_to_change_is_changed_false() {
        let (code, v) = go("issue update ENG-142 --state 'In Progress' --assignee me --dry-run --json", None);
        assert_eq!(code, 0);
        assert_eq!(v["changed"], false);
        assert_eq!(v["input"], json!({}));
        let (_, v) = go("issue update ENG-142 --label 'customer reported' --label bug --yes --json", None);
        assert_eq!(v["changed"], false, "same labels in another order and case");
        assert!(v.get("issue").is_none(), "nothing changed, nothing returned");
    }

    #[test]
    fn yes_without_a_state_file_is_read_only() {
        let (code, v) = go("issue update ENG-142 --priority low --yes --json", None);
        assert_eq!(code, 1);
        assert_eq!(v["error"], "the demo workspace is read-only");
    }

    #[test]
    fn update_with_a_state_file_persists() {
        let st = state("update");
        let (code, v) = go(
            "issue update ENG-142 --state Done --assignee jonas@northwind.example --add-label Security --remove-label bug --yes --json",
            Some(&st),
        );
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["changed"], true);
        assert_eq!(v["issue"]["state"]["name"], "Done");
        assert_eq!(v["issue"]["assignee"]["name"], "Jonas Berg");
        assert_eq!(v["input"]["addedLabelIds"], json!([label_id("Security")]));
        assert_eq!(v["input"]["removedLabelIds"], json!([label_id("Bug")]));
        let names: Vec<&str> =
            v["issue"]["labels"].as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Customer reported", "Security"]);
        assert_eq!(v["issue"]["updatedAt"], "2026-10-26T12:00:00.000Z", "updated now");
        // A later read sees it; adding a label it has is nothing to do.
        let (_, show) = go("issue show ENG-142 --json", Some(&st));
        assert_eq!(show["issue"]["state"]["name"], "Done");
        let (_, again) =
            go("issue update ENG-142 --add-label security --remove-label Bug --yes --json", Some(&st));
        assert_eq!(again["changed"], false);
        let (_, d) = go("issue update ENG-142 --description-file /nonexistent --dry-run --json", Some(&st));
        assert!(d["error"].as_str().unwrap().contains("--description-file /nonexistent"));
    }

    #[test]
    fn update_more_fields() {
        let st = state("fields");
        let desc = std::env::temp_dir().join(format!("fake-desc-{}", std::process::id()));
        fs::write(&desc, "New text").unwrap();
        let cmd = format!(
            "issue update ENG-155 --title 'Dark settings' --description-file {} --project 'mobile 3.0' --parent ENG-120 --assignee none --label feature --yes --json",
            desc.display()
        );
        let (code, v) = go(&cmd, Some(&st));
        assert_eq!(code, 0, "{v}");
        let input = v["input"].as_object().unwrap();
        let mut keys: Vec<&String> = input.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["description", "parentId", "projectId", "title"],
            "assignee none and the same label are no change"
        );
        let (_, show) = go("issue show ENG-155 --json", Some(&st));
        assert_eq!(show["issue"]["description"], "New text");
        assert_eq!(show["issue"]["project"]["name"], "Mobile 3.0");
        assert_eq!(show["issue"]["parent"]["identifier"], "ENG-120");
        fs::remove_file(desc).unwrap();
    }

    #[test]
    fn update_errors() {
        for (cmd, code, needle) in [
            ("issue update ENG-142 --state Nope --dry-run --json", 1, "not found: state \"Nope\""),
            ("issue update ENG-142 --label Nope --dry-run --json", 1, "not found: label \"Nope\""),
            ("issue update ENG-142 --add-label Nope --dry-run --json", 1, "not found: label \"Nope\""),
            (
                "issue update ENG-142 --assignee x@y.example --dry-run --json",
                1,
                "not found: user \"x@y.example\"",
            ),
            ("issue update ENG-142 --project Nope --dry-run --json", 1, "not found: project \"Nope\""),
            ("issue update ENG-142 --parent ENG-999 --dry-run --json", 1, "not found: issue \"ENG-999\""),
            ("issue update ENG-999 --priority 1 --dry-run --json", 1, "not found: issue ENG-999"),
            ("issue update ENG-142 --priority 9 --dry-run --json", 2, "--priority"),
            ("issue update ENG-142 --priority 1 --json", 2, "exactly one of --dry-run and --yes"),
            ("issue update ENG-142 --priority 1 --dry-run --yes --json", 2, "exactly one"),
            ("issue update ENG-142 --description-file - --dry-run --json", 2, "reads no stdin"),
            ("issue update ENG-142 --bogus 1 --dry-run --json", 2, "unknown flag"),
        ] {
            let (c, v) = go(cmd, None);
            assert_eq!(c, code, "{cmd}: {v}");
            assert!(v["error"].as_str().unwrap().contains(needle), "{cmd}: {v}");
        }
    }

    #[test]
    fn create_is_idempotent_by_id() {
        let st = state("create");
        let (_, dry) = go(
            "issue create --team eng --title 'Gift cards' --priority urgent --label feature --assignee me --state todo --dry-run --json",
            Some(&st),
        );
        assert_eq!(dry["changed"], true);
        let id = dry["id"].as_str().unwrap().to_string();
        assert_eq!(dry["input"]["id"], json!(id));
        assert_eq!(dry["input"]["teamId"], json!(uuid("team \"ENG\"")));
        let cmd = format!(
            "issue create --team ENG --title 'Gift cards' --priority urgent --label feature --assignee me --state todo --id {id} --yes --json"
        );
        let (code, v) = go(&cmd, Some(&st));
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["issue"]["identifier"], "ENG-159");
        assert_eq!(v["issue"]["id"], json!(id));
        assert_eq!(v["issue"]["state"]["name"], "Todo");
        assert_eq!(v["issue"]["priority"], 1);
        let (_, again) = go(&cmd, Some(&st));
        assert_eq!(again["changed"], false, "a retry never duplicates");
        assert_eq!(again["issue"]["identifier"], "ENG-159");
        let (_, list) = go("issue list --limit 1 --json", Some(&st));
        assert_eq!(list["items"][0]["identifier"], "ENG-159");
        let (_, plain) =
            go(&format!("issue create --team ENG --title Plain --id {} --yes --json", uuid("x")), Some(&st));
        assert_eq!(plain["issue"]["state"]["name"], "Backlog", "the default state");
    }

    #[test]
    fn create_errors() {
        for (cmd, code) in [
            ("issue create --title x --dry-run --json", 2),
            ("issue create --team ENG --dry-run --json", 2),
            ("issue create --team ENG --title x --yes --json", 2),
            ("issue create --team WEB --title x --dry-run --json", 1),
            ("issue create --team ENG --title x --state nope --dry-run --json", 1),
            ("issue create --team ENG --title x --bogus 1 --dry-run --json", 2),
        ] {
            assert_eq!(go(cmd, None).0, code, "{cmd}");
        }
    }

    #[test]
    fn archive() {
        let st = state("archive");
        let (_, dry) = go("issue archive ENG-155 --dry-run --json", Some(&st));
        assert_eq!(
            dry,
            json!({"changed": true, "dryRun": true, "identifier": "ENG-155", "url": "https://linear.app/northwind/issue/ENG-155"})
        );
        let (code, _) = go("issue archive ENG-155 --yes --json", Some(&st));
        assert_eq!(code, 0);
        let (_, list) = go("issue list --limit 0 --json", Some(&st));
        assert_eq!(list["shown"], 16, "archived issues leave lists");
        let (_, show) = go("issue show ENG-155 --json", Some(&st));
        assert!(show["issue"]["archivedAt"].is_string());
        let (_, again) = go("issue archive ENG-155 --yes --json", Some(&st));
        assert_eq!(again["changed"], false);
        assert_eq!(go("issue archive ENG-999 --yes --json", Some(&st)).0, 1);
    }

    #[test]
    fn comments() {
        let st = state("comment");
        let (_, dry) = go("comment create ENG-158 --body 'On it' --dry-run --json", Some(&st));
        let id = dry["id"].as_str().unwrap().to_string();
        assert_eq!(dry["input"]["body"], "On it");
        let cmd = format!("comment create ENG-158 --body 'On it' --id {id} --yes --json");
        let (code, v) = go(&cmd, Some(&st));
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["comment"]["id"], json!(id));
        assert_eq!(v["comment"]["user"]["name"], "Maya Patel");
        let (_, again) = go(&cmd, Some(&st));
        assert_eq!(again["changed"], false);
        assert_eq!(again["comment"]["body"], "On it");
        let (_, show) = go("issue show ENG-158 --json", Some(&st));
        assert_eq!(show["issue"]["comments"].as_array().unwrap().len(), 2);
        // An issue with no comments yet.
        let (code, _) =
            go(&format!("comment create ENG-155 --body first --id {} --yes --json", uuid("c2")), Some(&st));
        assert_eq!(code, 0);
        assert_eq!(go("comment create ENG-158 --dry-run --json", None).0, 2);
        assert_eq!(go("comment create ENG-158 --body x --yes --json", None).0, 2);
        assert_eq!(go("comment create ENG-999 --body x --dry-run --json", None).0, 1);
        assert_eq!(go("comment create ENG-158 --body x --id abc --yes --json", None).0, 1, "read-only");
    }

    #[test]
    fn cycle_estimate_and_due_date() {
        let st = state("ced");
        let (_, dry) = go(
            "issue update ENG-142 --cycle next --estimate 5 --due-date 2026-12-01 --dry-run --json",
            Some(&st),
        );
        assert_eq!(dry["input"]["cycleId"], json!(uuid("cycle 25")));
        assert_eq!(dry["input"]["estimate"], 5);
        assert_eq!(dry["input"]["dueDate"], "2026-12-01");
        assert_eq!(
            go(
                "issue update ENG-142 --cycle next --estimate 5 --due-date 2026-12-01 --yes --json",
                Some(&st)
            )
            .0,
            0
        );
        let (_, show) = go("issue show ENG-142 --json", Some(&st));
        assert_eq!(show["issue"]["cycle"]["number"], 25);
        assert_eq!(show["issue"]["estimate"], 5);
        assert_eq!(show["issue"]["dueDate"], "2026-12-01", "where it was set, after the shift");
        assert_eq!(
            go("issue update ENG-142 --due-date 2026-12-01 --yes --json", Some(&st)).1["changed"],
            false
        );
        assert_eq!(
            go("issue update ENG-142 --cycle none --estimate none --due-date none --yes --json", Some(&st)).0,
            0
        );
        let (_, show) = go("issue show ENG-142 --json", Some(&st));
        assert!(
            show["issue"]["cycle"].is_null()
                && show["issue"]["estimate"].is_null()
                && show["issue"]["dueDate"].is_null()
        );
        for (cmd, code) in [
            ("issue update ENG-142 --cycle 99 --dry-run --json", 1),
            ("issue update ENG-142 --estimate lots --dry-run --json", 2),
            ("issue update ENG-142 --due-date tomorrow --dry-run --json", 2),
        ] {
            assert_eq!(go(cmd, None).0, code, "{cmd}");
        }
        let (_, prev) = go("issue update ENG-142 --cycle previous --dry-run --json", None);
        assert_eq!(prev["input"]["cycleId"], json!(uuid("cycle 23")));
        let (_, by_id) =
            go(&format!("issue update ENG-142 --cycle {} --dry-run --json", uuid("cycle 23")), None);
        assert_eq!(by_id["input"]["cycleId"], json!(uuid("cycle 23")));
        let (_, current) = go("issue create --team ENG --title x --cycle current --dry-run --json", None);
        assert_eq!(current["input"]["cycleId"], json!(uuid("cycle 24")));
    }

    #[test]
    fn project_none_clears_it() {
        let st = state("project-none");
        let (_, dry) = go("issue update ENG-142 --project none --dry-run --json", Some(&st));
        assert_eq!(dry["input"], json!({"projectId": null}));
        assert_eq!(go("issue update ENG-142 --project none --yes --json", Some(&st)).0, 0);
        assert!(go("issue show ENG-142 --json", Some(&st)).1["issue"]["project"].is_null());
        assert_eq!(go("issue update ENG-142 --project none --yes --json", Some(&st)).1["changed"], false);
    }

    #[test]
    fn unarchive_and_delete_a_comment() {
        let st = state("undo");
        assert_eq!(go("issue archive ENG-155 --yes --json", Some(&st)).0, 0);
        let (_, dry) = go("issue unarchive ENG-155 --dry-run --json", Some(&st));
        assert_eq!(dry["changed"], true);
        let (code, _) = go("issue unarchive ENG-155 --yes --json", Some(&st));
        assert_eq!(code, 0);
        assert!(go("issue show ENG-155 --json", Some(&st)).1["issue"]["archivedAt"].is_null());
        assert_eq!(go("issue unarchive ENG-155 --yes --json", Some(&st)).1["changed"], false);
        let (_, show) = go("issue show ENG-142 --json", Some(&st));
        let c = show["issue"]["comments"][3]["id"].as_str().unwrap().to_string();
        let (_, dry) = go(&format!("comment delete {c} --dry-run --json"), Some(&st));
        assert_eq!((dry["changed"].clone(), dry["found"].clone()), (json!(true), json!(true)));
        assert_eq!(go(&format!("comment delete {c} --yes --json"), Some(&st)).0, 0);
        assert_eq!(
            go("issue show ENG-142 --json", Some(&st)).1["issue"]["comments"].as_array().unwrap().len(),
            3
        );
        let (_, again) = go(&format!("comment delete {c} --yes --json"), Some(&st));
        assert_eq!((again["changed"].clone(), again["found"].clone()), (json!(false), json!(false)));
    }

    #[test]
    fn replies_join_the_root_thread() {
        let st = state("reply");
        let (_, show) = go("issue show ENG-142 --json", Some(&st));
        let root = show["issue"]["comments"][0]["id"].as_str().unwrap().to_string();
        let reply = show["issue"]["comments"][1]["id"].as_str().unwrap().to_string();
        let (_, dry) =
            go(&format!("comment create ENG-142 --body ok --parent {root} --dry-run --json"), Some(&st));
        assert_eq!(dry["input"]["parentId"], json!(root));
        // Replying to a reply goes to its root.
        let cmd =
            format!("comment create ENG-142 --body again --parent {reply} --id {} --yes --json", uuid("r1"));
        let (code, v) = go(&cmd, Some(&st));
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["comment"]["parent"]["id"], json!(root));
        // And to a reply made here.
        let made = v["comment"]["id"].as_str().unwrap().to_string();
        let (_, v2) = go(
            &format!("comment create ENG-142 --body third --parent {made} --id {} --yes --json", uuid("r2")),
            Some(&st),
        );
        assert_eq!(v2["comment"]["parent"]["id"], json!(root));
        // A new root, and a reply to it.
        let (_, top) =
            go(&format!("comment create ENG-142 --body top --id {} --yes --json", uuid("t1")), Some(&st));
        let top_id = top["comment"]["id"].as_str().unwrap().to_string();
        let (_, under) = go(
            &format!(
                "comment create ENG-142 --body under --parent {top_id} --id {} --yes --json",
                uuid("u1")
            ),
            Some(&st),
        );
        assert_eq!(under["comment"]["parent"]["id"], json!(top_id));
        assert_eq!(go("comment create ENG-142 --body x --parent nope --dry-run --json", None).0, 1);
    }

    #[test]
    fn a_corrupt_state_file_falls_back_to_the_seed() {
        let st = state("corrupt");
        fs::write(&st.0, "not json").unwrap();
        assert_eq!(go("issue list --limit 0 --json", Some(&st)).1["shown"], 17);
    }
}
