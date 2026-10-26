//! `e`: the issue's fields in the create form, written back as one
//! `issue update` of what changed. Undo restores the fields it changed.

use super::undo::UndoStep;
use super::write::{Form, Purpose, Text};
use super::{App, Effect, Request};
use crate::ctl::{self, Mode};
use crate::model::IssueDetail;

/// What the form edits: the issue, and its fields when the form opened.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Editing {
    pub issue: String,
    /// For `--if-updated-at`.
    pub updated_at: String,
    pub before: Box<Form>,
}

/// The issue's fields as the form shows them.
fn form_of(i: &IssueDetail) -> Form {
    let row = &i.row;
    Form {
        team: row.team.as_ref().map(|t| t.key.clone()).unwrap_or_default(),
        title: row.title.clone(),
        priority: row.priority,
        state: row.state.as_ref().map(|s| s.name.clone()),
        assignee: row.assignee.as_ref().map(|a| (a.email.clone(), a.name.clone())),
        labels: row.labels.iter().map(|l| l.name.clone()).collect(),
        project: row.project.as_ref().map(|p| p.name.clone()),
        // As stored in Linear, not as read with --signed-urls.
        description: crate::images::unsign(i.description.as_deref().unwrap_or_default())
            .trim_end()
            .to_string(),
        ..Form::default()
    }
}

/// The flags an edit sends, the flags that revert it (`None` when it
/// cannot be reverted), and the lines the confirm dialog shows.
struct Changes {
    flags: Vec<(String, String)>,
    undo: Option<Vec<(String, String)>>,
    lines: Vec<String>,
}

impl Changes {
    fn set(&mut self, flag: &str, new: String, old: Option<String>, line: String) {
        self.flags.push((flag.into(), new));
        match (self.undo.as_mut(), old) {
            (Some(u), Some(o)) => u.push((flag.into(), o)),
            _ => self.undo = None,
        }
        self.lines.push(format!("  {line}"));
    }
}

impl App {
    /// Whether `--project none` clears a project: linearctl 0.6.0 and later
    /// (0.4.0 had the flag but took "none" as a project's name).
    pub(super) fn can_clear_project(&self) -> bool {
        self.at_least(0, 6)
    }

    /// `e`: the form for the open issue; from the list, open the issue
    /// first and the form when it has loaded.
    pub(super) fn open_edit(&mut self) -> Vec<Effect> {
        if self.focus == super::Focus::Detail
            && let Some(i) = self.detail.as_ref().and_then(|d| d.issue.as_ref())
        {
            let before = form_of(i);
            let editing = Editing {
                issue: i.row.identifier.clone(),
                updated_at: i.row.updated_at.clone(),
                before: Box::new(before.clone()),
            };
            self.form = Some(Form { editing: Some(Box::new(editing)), ..before });
            return Vec::new();
        }
        let Some(id) = self.selected_row().map(|r| r.identifier.clone()) else { return Vec::new() };
        self.pending_edit = Some(id);
        let mut fx = self.open();
        if self.detail.as_ref().is_some_and(|d| d.issue.is_some()) {
            self.pending_edit = None;
            fx.extend(self.open_edit());
        }
        fx
    }

    /// An assignee value as an email, so `me` compares with the viewer.
    fn email_of(&self, value: &str) -> String {
        match (value, &self.auth) {
            ("me", Some(a)) => a.user.email.clone(),
            _ => value.to_string(),
        }
    }

    /// Send what changed: a dry run, then the confirm dialog.
    pub(super) fn submit_edit(&mut self, f: Form) -> Vec<Effect> {
        let Some(editing) = f.editing.as_deref() else { return Vec::new() };
        let b = &editing.before;
        if f.title.trim().is_empty() {
            self.status = Some("An issue needs a title.".into());
            return Vec::new();
        }
        let mut c = Changes { flags: Vec::new(), undo: Some(Vec::new()), lines: Vec::new() };
        let title = f.title.trim().to_string();
        if title != b.title {
            c.set("title", title.clone(), Some(b.title.clone()), format!("Title: {title}"));
        }
        if f.state != b.state
            && let Some(s) = &f.state
        {
            c.set("state", s.clone(), b.state.clone(), format!("Status: {s}"));
        }
        if f.priority != b.priority {
            let name = ["No priority", "Urgent", "High", "Medium", "Low"][f.priority.clamp(0, 4) as usize];
            c.set(
                "priority",
                f.priority.to_string(),
                Some(b.priority.to_string()),
                format!("Priority: {name}"),
            );
        }
        let new_assignee = f.assignee.as_ref().map(|a| self.email_of(&a.0));
        let old_assignee = b.assignee.as_ref().map(|a| a.0.clone());
        if new_assignee != old_assignee {
            let name = f.assignee.as_ref().map_or("Unassigned".to_string(), |a| a.1.clone());
            let new = new_assignee.unwrap_or_else(|| "none".into());
            let old = Some(old_assignee.unwrap_or_else(|| "none".into()));
            c.set("assignee", new, old, format!("Assignee: {name}"));
        }
        let (mut new_l, mut old_l) = (f.labels.clone(), b.labels.clone());
        new_l.sort();
        old_l.sort();
        if new_l != old_l {
            let shown = if f.labels.is_empty() { "none".to_string() } else { f.labels.join(", ") };
            c.lines.push(format!("  Labels: {shown}"));
            // --label sets them all; with none left, remove the old ones.
            let pairs = |flag: &str, ls: &[String]| {
                ls.iter().map(|l| (flag.to_string(), l.clone())).collect::<Vec<_>>()
            };
            c.flags.extend(if f.labels.is_empty() {
                pairs("remove-label", &b.labels)
            } else {
                pairs("label", &f.labels)
            });
            if let Some(u) = c.undo.as_mut() {
                u.extend(if b.labels.is_empty() {
                    pairs("remove-label", &f.labels)
                } else {
                    pairs("label", &b.labels)
                });
            }
        }
        if f.project != b.project {
            let clear = self.can_clear_project();
            if f.project.is_none() && !clear {
                self.status = Some("Clearing a project needs linearctl 0.6.0 or later.".into());
                return Vec::new();
            }
            let new = f.project.clone().unwrap_or_else(|| "none".into());
            let old = b.project.clone().or_else(|| clear.then(|| "none".into()));
            c.set("project", new.clone(), old, format!("Project: {new}"));
        }
        let Changes { mut flags, undo, mut lines } = c;
        let text = (f.description != b.description)
            .then(|| Text { flag: "description", text: f.description.clone() });
        if text.is_some() {
            lines.push(format!(
                "  Description: {} → {} characters",
                b.description.chars().count(),
                f.description.chars().count()
            ));
        }
        if flags.is_empty() && text.is_none() {
            self.form = None;
            self.notice = Some("Nothing to change.".into());
            return Vec::new();
        }
        if self.can_check_updated {
            flags.push(("if-updated-at".into(), editing.updated_at.clone()));
        }
        let pairs: Vec<(&str, &str)> = flags.iter().map(|(a, v)| (a.as_str(), v.as_str())).collect();
        let argv = ctl::update_argv(&editing.issue, &pairs, Mode::DryRun);
        let undo_text = text.as_ref().map(|_| b.description.clone());
        let purpose = Purpose::Edit { issue: editing.issue.clone(), undo, undo_text };
        vec![Effect::Run(Request::Write { purpose, argv, text, summary: lines })]
    }

    /// The edit was written: close the form, show it, keep its undo.
    pub(super) fn on_edited_issue(
        &mut self,
        issue: String,
        undo: Option<Vec<(String, String)>>,
        undo_text: Option<String>,
        row: Option<crate::model::IssueRow>,
    ) -> Vec<Effect> {
        self.form = None;
        self.apply_row(row);
        self.drafts.remove(&issue);
        self.undo = None;
        match undo {
            Some(flags) => {
                let text = undo_text.map(|t| Text { flag: "description", text: t });
                self.remember_undo(
                    format!("editing {issue}"),
                    UndoStep::Update { issue: issue.clone(), flags, text },
                );
                self.notice = Some(format!("{issue}: saved (u undoes)"));
            }
            None => self.notice = Some(format!("{issue}: saved (linearctl cannot clear a project: no undo)")),
        }
        self.refresh_detail(&issue)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::{Msg, Overlay, default_views};
    use crate::fake::InProcess;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn now() -> chrono::DateTime<chrono::Utc> {
        "2026-10-26T12:00:00Z".parse().unwrap()
    }

    /// An app on the fake, writes kept in a state file; editor requests
    /// are answered with `edited`.
    struct W {
        app: App,
        exec: InProcess,
        edits: Vec<super::super::EditFor>,
    }

    impl Drop for W {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.exec.state.as_ref().unwrap());
        }
    }

    impl W {
        fn new(name: &str) -> Self {
            let state = std::env::temp_dir().join(format!("linear-tui-edit-{name}-{}", std::process::id()));
            let _ = std::fs::remove_file(&state);
            let mut w = W {
                app: App::new(default_views(), now()),
                exec: InProcess { now: now(), state: Some(state) },
                edits: vec![],
            };
            let fx = w.app.start();
            w.run(fx);
            w
        }
        fn run(&mut self, fx: Vec<Effect>) {
            for f in fx {
                match f {
                    Effect::Run(r) => {
                        let more = self.app.update(r.run(&self.exec));
                        self.run(more);
                    }
                    Effect::Edit { purpose, .. } => self.edits.push(purpose),
                    _ => {}
                }
            }
        }
        fn key(&mut self, code: KeyCode) {
            let fx = self.app.update(Msg::Key(KeyEvent::from(code)));
            self.run(fx);
        }
        fn ctrl(&mut self, c: char) {
            let fx = self.app.update(Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)));
            self.run(fx);
        }
        fn chars(&mut self, s: &str) {
            s.chars().for_each(|c| self.key(KeyCode::Char(c)));
        }
        fn edited(&mut self, text: &str) {
            let purpose = self.edits.pop().expect("an editor");
            let fx = self.app.update(Msg::Edited { purpose, text: Some(text.into()) });
            self.run(fx);
        }
        fn to(&mut self, field: &str) {
            for _ in 0..super::super::FORM_FIELDS.len() {
                if super::super::FORM_FIELDS[self.app.form.as_ref().unwrap().field] == field {
                    return;
                }
                self.key(KeyCode::Down);
            }
            panic!("no field {field}");
        }
        fn show(&self, id: &str) -> IssueDetail {
            ctl::run_as::<crate::model::ShowResult>(&self.exec, &ctl::show_argv(id)).unwrap().issue
        }
        fn confirm_lines(&self) -> Vec<String> {
            match &self.app.overlay {
                Some(Overlay::Confirm(c)) => c.lines.clone(),
                other => panic!("no confirm: {other:?}"),
            }
        }
    }

    /// linearctl 0.4.0: the flags of today, before --project none worked.
    pub(crate) fn old_0_4_0() -> crate::contract::Describe {
        let mut v: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/linearctl-describe-0.6.0.json")).unwrap();
        v["version"] = "0.4.0".into();
        crate::contract::Describe::from_json(&v.to_string()).unwrap()
    }

    #[test]
    fn editing_fields_and_undoing_them() {
        let mut w = W::new("fields");
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('e'));
        let f = w.app.form.as_ref().unwrap();
        assert_eq!(f.title, "Checkout fails when a coupon code has trailing whitespace");
        assert_eq!(f.editing.as_ref().unwrap().issue, "ENG-142");
        assert!(!f.description.contains("signature="), "unsigned");
        for _ in 0.."whitespace".len() {
            w.key(KeyCode::Backspace);
        }
        w.chars("spaces");
        w.to("Priority");
        w.key(KeyCode::Enter);
        w.chars("low");
        w.key(KeyCode::Enter);
        w.to("Labels");
        w.key(KeyCode::Enter);
        w.chars("secu");
        w.key(KeyCode::Char(' '));
        w.key(KeyCode::Enter);
        w.to("Description");
        w.key(KeyCode::Enter);
        w.edited("Shorter now.\n");
        w.ctrl('s');
        assert_eq!(
            w.confirm_lines(),
            [
                "  Title: Checkout fails when a coupon code has trailing spaces",
                "  Priority: Low",
                "  Labels: Bug, Security, Customer reported",
                "  Description: 804 → 12 characters",
            ]
        );
        w.key(KeyCode::Char('y'));
        assert!(w.app.form.is_none());
        assert_eq!(w.app.notice.as_deref(), Some("ENG-142: saved (u undoes)"));
        let i = w.show("ENG-142");
        assert!(i.row.title.ends_with("trailing spaces"));
        assert_eq!(i.row.priority, 4);
        assert_eq!(i.row.labels.len(), 3);
        assert_eq!(i.description.as_deref(), Some("Shorter now."));
        w.key(KeyCode::Char('u'));
        assert_eq!(w.app.notice.as_deref(), Some("Undone: editing ENG-142"));
        let i = w.show("ENG-142");
        assert!(i.row.title.ends_with("trailing whitespace"));
        assert_eq!(i.row.priority, 1);
        assert_eq!(i.row.labels.len(), 2);
        assert!(i.description.unwrap().starts_with("Customers who paste"));
    }

    #[test]
    fn e_in_the_list_opens_the_issue_then_the_form() {
        let mut w = W::new("from-list");
        w.key(KeyCode::Down);
        w.key(KeyCode::Char('e'));
        assert_eq!(w.app.form.as_ref().unwrap().editing.as_ref().unwrap().issue, "ENG-143");
        // Nothing changed: nothing sent.
        w.ctrl('s');
        assert!(w.app.form.is_none());
        assert_eq!(w.app.notice.as_deref(), Some("Nothing to change."));
        // Opened again from the cache.
        w.key(KeyCode::Esc);
        w.key(KeyCode::Char('e'));
        assert!(w.app.form.is_some());
        w.key(KeyCode::Esc);
        assert_eq!(w.app.notice.as_deref(), Some("Edit discarded."));
    }

    #[test]
    fn checks_before_sending() {
        let mut w = W::new("checks");
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('e'));
        w.to("Team");
        w.key(KeyCode::Enter);
        assert_eq!(w.app.status.as_deref(), Some("An issue's team is not changed here."));
        w.app.form.as_mut().unwrap().title = "  ".into();
        w.ctrl('s');
        assert_eq!(w.app.status.as_deref(), Some("An issue needs a title."));
        // Me is the viewer: no change. Unassigned is one.
        let f = w.app.form.as_mut().unwrap();
        f.title = f.editing.as_ref().unwrap().before.title.clone();
        f.assignee = Some(("me".into(), "Me".into()));
        w.ctrl('s');
        assert_eq!(w.app.notice.as_deref(), Some("Nothing to change."));
    }

    #[test]
    fn clearing_the_project_needs_linearctl_0_6_0() {
        let mut w = W::new("project");
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('e'));
        w.app.form.as_mut().unwrap().project = None;
        let d = |s: &str| crate::contract::Describe::from_json(s).unwrap();
        w.app.describe = Some(old_0_4_0());
        assert!(!w.app.can_clear_project());
        w.ctrl('s');
        assert_eq!(w.app.status.as_deref(), Some("Clearing a project needs linearctl 0.6.0 or later."));
        w.app.describe = Some(d(include_str!("../../tests/fixtures/linearctl-describe-0.6.0.json")));
        assert!(w.app.can_clear_project());
        w.ctrl('s');
        assert_eq!(w.confirm_lines(), ["  Project: none"]);
        w.key(KeyCode::Enter);
        assert!(w.show("ENG-142").row.project.is_none());
        w.key(KeyCode::Char('u'));
        assert_eq!(w.show("ENG-142").row.project.unwrap().name, "Checkout v2");
        // Setting a project where there was none, with 0.4.0: no undo.
        w.app.describe = Some(old_0_4_0());
        w.key(KeyCode::Esc); // back to the list
        w.key(KeyCode::Char('2'));
        for _ in 0..20 {
            if w.app.selected_row().unwrap().identifier == "ENG-152" {
                break;
            }
            w.key(KeyCode::Down);
        }
        assert_eq!(w.app.selected_row().unwrap().identifier, "ENG-152");
        w.key(KeyCode::Char('e'));
        w.app.form.as_mut().unwrap().project = Some("Mobile 3.0".into());
        w.app.form.as_mut().unwrap().priority = 2;
        w.ctrl('s');
        w.key(KeyCode::Char('y'));
        assert_eq!(
            w.app.notice.as_deref(),
            Some("ENG-152: saved (linearctl cannot clear a project: no undo)")
        );
        assert!(w.app.undo.is_none());
    }

    #[test]
    fn the_guard_goes_with_the_write() {
        let mut w = W::new("guard");
        w.app.can_check_updated = true;
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('e'));
        w.app.form.as_mut().unwrap().priority = 3;
        w.ctrl('s');
        // The fake (0.1.0's flags for update) refuses --if-updated-at: the
        // flag was sent.
        assert!(w.app.status.as_deref().unwrap().contains("--if-updated-at"), "{:?}", w.app.status);
    }

    #[test]
    fn unassigning_and_dropping_every_label() {
        let mut w = W::new("unassign");
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('e'));
        let f = w.app.form.as_mut().unwrap();
        f.assignee = None;
        f.labels.clear();
        f.state = Some("In Review".into());
        w.ctrl('s');
        assert_eq!(w.confirm_lines(), ["  Status: In Review", "  Assignee: Unassigned", "  Labels: none"]);
        w.key(KeyCode::Char('y'));
        let i = w.show("ENG-142");
        assert!(i.row.assignee.is_none() && i.row.labels.is_empty());
        w.key(KeyCode::Char('u'));
        let i = w.show("ENG-142");
        assert_eq!(i.row.assignee.unwrap().name, "Maya Patel");
        assert_eq!(i.row.labels.len(), 2);
        assert_eq!(i.row.state.unwrap().name, "In Progress");
        // From no labels to some, and back.
        w.key(KeyCode::Char('e'));
        w.app.form.as_mut().unwrap().labels.clear();
        w.ctrl('s');
        w.key(KeyCode::Char('y'));
        w.key(KeyCode::Char('e'));
        w.app.form.as_mut().unwrap().labels = vec!["Feature".into()];
        w.ctrl('s');
        w.key(KeyCode::Char('y'));
        w.key(KeyCode::Char('u'));
        assert!(w.show("ENG-142").row.labels.is_empty());
    }
}
