//! What linearctl prints, as Rust types.
//!
//! Every field is optional or defaulted and unknown fields are ignored, so
//! the TUI reads linearctl 0.1.0 and newer builds alike (newer ones add ids
//! and colours to nested objects).

use serde::Deserialize;

/// A list result: `{items, total: null, shown, truncated, nextCursor}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub shown: usize,
    pub truncated: bool,
    pub next_cursor: Option<String>,
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Self { items: Vec::new(), shown: 0, truncated: false, next_cursor: None }
    }
}

/// A workflow state as nested in an issue.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct State {
    pub name: String,
    /// triage, backlog, unstarted, started, completed, canceled, duplicate.
    #[serde(rename = "type")]
    pub kind: String,
    /// `#rrggbb`; absent before linearctl added it.
    pub color: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct User {
    pub name: String,
    pub email: String,
}

impl User {
    /// "Maya Patel" → "MP".
    pub fn initials(&self) -> String {
        self.name
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .take(2)
            .collect::<String>()
            .to_uppercase()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Label {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Team {
    pub key: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Project {
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Cycle {
    pub number: i64,
}

/// One row of `issue list`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IssueRow {
    pub id: String,
    pub identifier: String,
    pub title: String,
    /// 0 none, 1 urgent, 2 high, 3 medium, 4 low.
    pub priority: i64,
    pub priority_label: String,
    pub url: String,
    pub created_at: String,
    pub updated_at: String,
    pub state: Option<State>,
    pub assignee: Option<User>,
    pub team: Option<Team>,
    pub project: Option<Project>,
    pub cycle: Option<Cycle>,
    pub labels: Vec<Label>,
}

/// Another issue, as a parent, child or relation names it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct IssueRef {
    pub identifier: String,
    pub title: String,
    pub state: Option<State>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Relation {
    /// related, blocks, duplicate, similar.
    #[serde(rename = "type")]
    pub kind: String,
    pub related_issue: IssueRef,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Attachment {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct IdRef {
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
    pub user: Option<User>,
    /// The thread's root comment, for a reply.
    pub parent: Option<IdRef>,
}

/// `issue show`'s `issue`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IssueDetail {
    #[serde(flatten)]
    pub row: IssueRow,
    pub description: Option<String>,
    pub due_date: Option<String>,
    pub estimate: Option<f64>,
    pub branch_name: Option<String>,
    pub creator: Option<User>,
    pub parent: Option<IssueRef>,
    pub children: Vec<IssueRef>,
    pub relations: Vec<Relation>,
    pub attachments: Vec<Attachment>,
    pub comments: Vec<Comment>,
}

impl IssueDetail {
    /// Comments as threads: each root, then its replies, oldest first.
    /// A reply whose root is missing (not loaded) is shown as a root.
    pub fn threads(&self) -> Vec<(&Comment, bool)> {
        fn root_of<'a>(c: &'a Comment, ids: &[&str]) -> Option<&'a str> {
            c.parent.as_ref().map(|p| p.id.as_str()).filter(|p| ids.contains(p))
        }
        let ids: Vec<&str> = self.comments.iter().map(|c| c.id.as_str()).collect();
        let mut out = Vec::new();
        for root in self.comments.iter().filter(|c| root_of(c, &ids).is_none()) {
            out.push((root, false));
            let replies = self.comments.iter().filter(|c| root_of(c, &ids) == Some(root.id.as_str()));
            out.extend(replies.map(|c| (c, true)));
        }
        out
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ShowResult {
    pub issue: IssueDetail,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Organization {
    pub name: String,
    pub url_key: String,
}

/// `auth status`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AuthStatus {
    pub organization: Organization,
    pub user: User,
    pub source: String,
}

/// An entry of a lookup list: a team, workflow state, label, user or
/// project. Each kind fills the fields it has.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Named {
    pub id: String,
    pub name: String,
    /// Teams.
    pub key: String,
    /// States: triage, backlog, unstarted, ...
    #[serde(rename = "type")]
    pub kind: String,
    pub color: Option<String>,
    pub position: f64,
    /// Users.
    pub email: String,
    pub display_name: String,
    pub team: Option<Team>,
}

/// What a write prints: `changed`, whether it was a dry run, the input it
/// sends (or would), and the written issue or comment.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WriteResult {
    pub changed: bool,
    pub dry_run: bool,
    pub id: Option<String>,
    pub identifier: Option<String>,
    pub input: serde_json::Value,
    pub issue: Option<IssueRow>,
    pub comment: Option<Comment>,
}

/// One line of `linearctl watch --json`. Unknown events and fields are
/// ignored, as linearctl may add them.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum WatchEvent {
    /// Up to date from `since`.
    Ready {
        #[serde(default)]
        mode: String,
        #[serde(default)]
        since: String,
    },
    /// An issue that matches, in the list's row shape.
    Upsert { issue: Box<IssueRow> },
    /// An issue that stopped matching: filter, archived or deleted.
    Remove {
        id: String,
        #[serde(default)]
        identifier: Option<String>,
        #[serde(default)]
        reason: String,
    },
    Heartbeat {
        #[serde(default)]
        at: String,
        #[serde(default)]
        mode: String,
    },
    /// The stream was down (reconnect) or fell back to polling (fallback);
    /// what changed meanwhile follows.
    Gap {
        #[serde(default)]
        reason: String,
        #[serde(default)]
        mode: String,
    },
    #[serde(other)]
    Other,
}

/// What a failed command prints: `{"error": …, "usage": …}`, plus
/// `retryAt` when rate-limited.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ErrorDoc {
    pub error: String,
    pub usage: bool,
    pub retry_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake;

    fn fake_json(cmd: &str) -> String {
        let argv: Vec<String> = shlex::split(cmd).unwrap();
        fake::run(&argv, "2026-10-26T12:00:00Z".parse().unwrap()).stdout
    }

    #[test]
    fn reads_a_list_page() {
        let p: Page<IssueRow> = serde_json::from_str(&fake_json("issue list --limit 3 --json")).unwrap();
        assert_eq!(p.items.len(), 3);
        assert!(p.truncated);
        assert_eq!(p.next_cursor.as_deref(), Some(p.items[2].id.as_str()));
        let r = &p.items[0];
        assert_eq!(r.identifier, "ENG-158");
        assert_eq!(r.state.as_ref().unwrap().kind, "unstarted");
        assert_eq!(r.labels[0].color.as_deref(), Some("#eb5757"));
        assert_eq!(r.assignee.as_ref().unwrap().initials(), "JB");
        assert_eq!(r.cycle.as_ref().unwrap().number, 24);
    }

    #[test]
    fn reads_linearctl_0_1_0_rows_without_colours() {
        // Captured from linearctl 0.1.0 (names replaced): no ids or colours.
        let json = r#"{"items":[{"assignee":null,"createdAt":"2026-10-26T14:53:16.312Z","cycle":null,
            "id":"c78aa8c5","identifier":"ENG-1","labels":[{"name":"Improvement"}],"priority":0,
            "priorityLabel":"No priority","project":null,"state":{"name":"Triage","type":"triage"},
            "team":{"key":"ENG"},"title":"T","updatedAt":"2026-10-26T14:53:55.757Z","url":"u"}],
            "nextCursor":null,"shown":1,"total":null,"truncated":false}"#;
        let p: Page<IssueRow> = serde_json::from_str(json).unwrap();
        assert_eq!(p.items[0].state.as_ref().unwrap().color, None);
        assert_eq!(p.items[0].labels[0], Label { name: "Improvement".into(), color: None });
        assert!(p.items[0].assignee.is_none());
    }

    #[test]
    fn reads_an_issue_with_threads() {
        let s: ShowResult = serde_json::from_str(&fake_json("issue show ENG-142 --json")).unwrap();
        let i = &s.issue;
        assert_eq!(i.row.identifier, "ENG-142");
        assert!(i.description.as_deref().unwrap().starts_with("Customers who paste"));
        assert_eq!(i.parent.as_ref().unwrap().identifier, "ENG-120");
        assert_eq!(i.children.len(), 2);
        assert_eq!(i.relations[0].related_issue.identifier, "ENG-131");
        assert_eq!(i.attachments.len(), 1);
        assert_eq!(i.estimate, Some(2.0));
        let t: Vec<(&str, bool)> =
            i.threads().iter().map(|(c, reply)| (c.user.as_ref().unwrap().name.as_str(), *reply)).collect();
        assert_eq!(
            t,
            [("Aiko Tanaka", false), ("Maya Patel", true), ("Daniel Okafor", false), ("Maya Patel", false)]
        );
    }

    #[test]
    fn a_reply_to_a_missing_root_is_a_root() {
        let d = IssueDetail {
            comments: vec![Comment {
                id: "b".into(),
                parent: Some(IdRef { id: "gone".into() }),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(d.threads().len(), 1);
        assert!(!d.threads()[0].1);
    }

    #[test]
    fn reads_auth_status_and_errors() {
        let a: AuthStatus = serde_json::from_str(&fake_json("auth status --json")).unwrap();
        assert_eq!(a.organization.name, "Northwind");
        assert_eq!(a.user.name, "Maya Patel");
        let e: ErrorDoc = serde_json::from_str(&fake_json("issue show ENG-1 --json")).unwrap();
        assert!(e.error.starts_with("not found"));
        assert!(!e.usage);
        let r: ErrorDoc = serde_json::from_str(
            r#"{"error":"rate limited","usage":false,"retryAt":"2026-10-26T13:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(r.retry_at.as_deref(), Some("2026-10-26T13:00:00Z"));
    }

    #[test]
    fn reads_watch_events() {
        // Lines as linearctl 0.2.1 printed them (from linear-cli's transcript).
        let lines = [
            r#"{"event":"upsert","issue":{"assignee":{"email":"ada@example.com","name":"Ada"},"createdAt":"2026-10-01T00:00:00.000Z","cycle":null,"id":"id-ENG-3","identifier":"ENG-3","labels":[{"name":"bug"}],"priority":2,"priorityLabel":"High","project":null,"state":{"name":"Todo","type":"unstarted"},"team":{"key":"ENG"},"title":"Replayed since --since","updatedAt":"2026-10-02T00:00:00.000Z","url":"https://linear.app/acme/issue/ENG-3"}}"#,
            r#"{"event":"remove","id":"id-ENG-1","identifier":"ENG-1","reason":"filter"}"#,
            r#"{"event":"ready","everything":false,"known":1,"mode":"subscription","since":"2026-10-26T16:15:55Z"}"#,
            r#"{"at":"2026-10-26T16:16:01Z","event":"heartbeat","mode":"subscription","rateLimit":{}}"#,
            r#"{"downFor":"1s","event":"gap","from":"2026-10-26T16:15:55Z","mode":"subscription","reason":"reconnect","to":"2026-10-26T16:16:02Z"}"#,
            r#"{"event":"remove","id":"x","identifier":null,"reason":"deleted"}"#,
            r#"{"event":"something-new","x":1}"#,
        ];
        let ev: Vec<WatchEvent> = lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert!(
            matches!(&ev[0], WatchEvent::Upsert { issue } if issue.identifier == "ENG-3" && issue.priority == 2)
        );
        assert_eq!(
            ev[1],
            WatchEvent::Remove {
                id: "id-ENG-1".into(),
                identifier: Some("ENG-1".into()),
                reason: "filter".into()
            }
        );
        assert_eq!(
            ev[2],
            WatchEvent::Ready { mode: "subscription".into(), since: "2026-10-26T16:15:55Z".into() }
        );
        assert_eq!(
            ev[3],
            WatchEvent::Heartbeat { at: "2026-10-26T16:16:01Z".into(), mode: "subscription".into() }
        );
        assert_eq!(ev[4], WatchEvent::Gap { reason: "reconnect".into(), mode: "subscription".into() });
        assert_eq!(ev[5], WatchEvent::Remove { id: "x".into(), identifier: None, reason: "deleted".into() });
        assert_eq!(ev[6], WatchEvent::Other);
        // The last line of a failed watch has no event.
        assert!(serde_json::from_str::<WatchEvent>(r#"{"error":"rate limited"}"#).is_err());
    }

    #[test]
    fn initials() {
        let u = |n: &str| User { name: n.into(), email: String::new() };
        assert_eq!(u("maya").initials(), "M");
        assert_eq!(u("Ana María de la Cruz").initials(), "AM");
        assert_eq!(u("").initials(), "");
    }
}
