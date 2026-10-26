//! Changing Linear: quick edits through pickers, comments and descriptions
//! through `$EDITOR`, archiving, and the create form.
//!
//! Every write is linearctl's two steps: `--dry-run` first, then the same
//! command with `--yes`. Quick edits (state, priority, assignee, labels,
//! project) apply right after a dry run that changes something; the rest
//! show a confirm dialog with the exact command between the two.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::undo::UndoStep;
use super::{App, Effect, Focus, Request};
use crate::ctl::{self, Failure, Lookup, Mode};
use crate::model::{IssueRow, Named, Page, WriteResult};

/// Long text a write sends in a file (`--description-file`, `--body-file`),
/// never in argv.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    /// `description` or `body`.
    pub flag: &'static str,
    pub text: String,
}

/// What a write is for, so its result lands in the right place.
#[derive(Debug, Clone, PartialEq)]
pub enum Purpose {
    /// A quick edit; `what` is for the notice ("state → Done").
    Quick {
        issue: String,
        what: String,
        /// The flags that put it back; `None` when it cannot be undone.
        undo: Option<Vec<(String, String)>>,
    },
    Comment {
        issue: String,
    },
    Reply {
        issue: String,
        to: String,
    },
    Description {
        issue: String,
        original: String,
    },
    /// Reverting the last write.
    Undo {
        label: String,
        step: super::undo::UndoStep,
    },
    Archive {
        issue: String,
    },
    Create,
}

/// What the editor is open for.
#[derive(Debug, Clone, PartialEq)]
pub enum EditFor {
    Comment {
        issue: String,
    },
    /// A reply in the thread of comment `parent`, written by `to`.
    Reply {
        issue: String,
        parent: String,
        to: String,
    },
    Description {
        issue: String,
        original: String,
        updated_at: String,
    },
    FormDescription,
}

/// A field a picker sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Team,
    State,
    Priority,
    Assignee,
    Labels,
    Project,
}

/// Where a picker's choice goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// A quick edit of this issue; `team` scopes states, labels, projects.
    Issue {
        issue: Box<IssueRow>,
        field: Field,
    },
    Form(Field),
    /// Which thread of this issue to reply in.
    Reply {
        issue: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PickItem {
    pub label: String,
    /// What linearctl is given: a name, an email, `me`, a priority number.
    pub value: String,
    pub color: Option<String>,
    pub hint: String,
    pub chosen: bool,
}

/// A list to choose from, filtered by typing.
#[derive(Debug, Clone, PartialEq)]
pub struct PickList {
    pub title: String,
    pub items: Vec<PickItem>,
    pub multi: bool,
    /// Index into the filtered items.
    pub cursor: usize,
    pub query: String,
    pub target: Target,
    /// The lookup still loading, if any: (what, team).
    pub waiting: Option<(Lookup, String)>,
}

impl PickList {
    /// Items that match the query, with their index in `items`.
    pub fn visible(&self) -> Vec<(usize, &PickItem)> {
        let q = self.query.to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, i)| q.is_empty() || i.label.to_lowercase().contains(&q))
            .collect()
    }
}

/// The step between a dry run and `--yes`.
#[derive(Debug, Clone, PartialEq)]
pub struct Confirm {
    pub title: String,
    pub lines: Vec<String>,
    /// The command that will run, as a shell line.
    pub command: String,
    pub argv: Vec<String>,
    pub text: Option<Text>,
    pub purpose: Purpose,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    Pick(Box<PickList>),
    Confirm(Box<Confirm>),
}

/// The create form's fields, in order.
pub const FORM_FIELDS: [&str; 8] =
    ["Title", "Team", "Status", "Priority", "Assignee", "Labels", "Project", "Description"];

/// A new issue being written.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Form {
    /// Made when the form opens, so a retry can never create twice.
    pub id: String,
    pub team: String,
    pub title: String,
    pub priority: i64,
    pub state: Option<String>,
    /// (value for linearctl, label to show).
    pub assignee: Option<(String, String)>,
    pub labels: Vec<String>,
    pub project: Option<String>,
    pub description: String,
    /// Index into [`FORM_FIELDS`].
    pub field: usize,
}

/// Lookup lists, loaded when a picker first needs them.
#[derive(Debug, Default)]
pub struct Lookups {
    lists: HashMap<(Lookup, String), Vec<Named>>,
    loading: Vec<(Lookup, String)>,
}

impl Lookups {
    pub fn get(&self, what: Lookup, team: &str) -> Option<&Vec<Named>> {
        self.lists.get(&(what, scope(what, team)))
    }
}

/// Lookups that are not per team are kept under "".
fn scope(what: Lookup, team: &str) -> String {
    match what {
        Lookup::Teams | Lookup::Users => String::new(),
        _ => team.to_string(),
    }
}

const PRIORITIES: [(&str, i64); 5] =
    [("Urgent", 1), ("High", 2), ("Medium", 3), ("Low", 4), ("No priority", 0)];

fn priority_name(p: i64) -> &'static str {
    PRIORITIES.iter().find(|(_, n)| *n == p).map_or("No priority", |(name, _)| name)
}

/// The lookup a field's picker lists.
fn lookup_for(field: Field) -> Option<Lookup> {
    match field {
        Field::Team => Some(Lookup::Teams),
        Field::State => Some(Lookup::States),
        Field::Assignee => Some(Lookup::Users),
        Field::Labels => Some(Lookup::Labels),
        Field::Project => Some(Lookup::Projects),
        Field::Priority => None,
    }
}

fn title_of(field: Field) -> &'static str {
    match field {
        Field::Team => "Team",
        Field::State => "Status",
        Field::Priority => "Priority",
        Field::Assignee => "Assignee",
        Field::Labels => "Labels",
        Field::Project => "Project",
    }
}

/// The command as a shell line, for the confirm dialog and copying.
pub fn shell_line(argv: &[String], text: Option<&Text>) -> String {
    let mut words = vec!["linearctl".to_string()];
    words.extend(argv.iter().cloned());
    if let Some(t) = text {
        let at = words.iter().position(|w| w == "--dry-run" || w == "--yes").unwrap_or(words.len());
        words.splice(at..at, [format!("--{}-file", t.flag), "FILE".into()]);
    }
    shlex::try_join(words.iter().map(String::as_str)).unwrap_or_else(|_| words.join(" "))
}

/// The first lines of a text, for previews.
fn preview(text: &str, lines: usize) -> Vec<String> {
    let mut v: Vec<String> = text.lines().take(lines).map(|l| format!("  {l}")).collect();
    if text.lines().count() > lines {
        v.push("  …".into());
    }
    v
}

impl App {
    /// The issue an action applies to: the open one in the detail, else
    /// the selected row.
    fn target_issue(&self) -> Option<IssueRow> {
        match (self.focus, &self.detail) {
            (Focus::Detail, Some(d)) => d.issue.as_ref().map(|i| i.row.clone()),
            _ => self.selected_row().cloned(),
        }
    }

    /// Load a lookup unless loaded or loading.
    fn need(&mut self, what: Lookup, team: &str) -> Vec<Effect> {
        let key = (what, scope(what, team));
        if self.lookups.lists.contains_key(&key) || self.lookups.loading.contains(&key) {
            return Vec::new();
        }
        self.lookups.loading.push(key);
        vec![Effect::Run(Request::Lookup {
            what,
            team: team.to_string(),
            argv: ctl::lookup_argv(what, team),
        })]
    }

    pub(super) fn on_looked(&mut self, what: Lookup, team: &str, result: Result<Page<Named>, Failure>) {
        let key = (what, scope(what, team));
        self.lookups.loading.retain(|k| *k != key);
        match result {
            Ok(page) => {
                let mut items = page.items;
                if what == Lookup::States {
                    items.sort_by(|a, b| a.position.total_cmp(&b.position));
                }
                self.lookups.lists.insert(key.clone(), items);
                if let Some(Overlay::Pick(p)) = &self.overlay
                    && p.waiting.as_ref().is_some_and(|(w, t)| (*w, scope(*w, t)) == key)
                {
                    let (target, title, multi) = (p.target.clone(), p.title.clone(), p.multi);
                    let items = self.pick_items(&target);
                    let cursor = items.iter().position(|i| i.chosen).unwrap_or(0);
                    self.overlay = Some(Overlay::Pick(Box::new(PickList {
                        title,
                        items,
                        multi,
                        cursor,
                        query: String::new(),
                        target,
                        waiting: None,
                    })));
                }
            }
            Err(f) => {
                self.status = Some(f.message());
                if matches!(&self.overlay, Some(Overlay::Pick(p)) if p.waiting.is_some()) {
                    self.overlay = None;
                }
            }
        }
    }

    /// The team a picker's lookups are for.
    fn team_of(&self, target: &Target) -> String {
        match target {
            Target::Issue { issue, .. } => issue.team.as_ref().map(|t| t.key.clone()).unwrap_or_default(),
            Target::Form(_) => self.form.as_ref().map(|f| f.team.clone()).unwrap_or_default(),
            Target::Reply { .. } => String::new(),
        }
    }

    /// The picker's items, marked with what is set now.
    fn pick_items(&self, target: &Target) -> Vec<PickItem> {
        let field = match target {
            Target::Issue { field, .. } | Target::Form(field) => *field,
            Target::Reply { .. } => return self.thread_items(),
        };
        let team = self.team_of(target);
        let form = self.form.as_ref();
        let issue = match target {
            Target::Issue { issue, .. } => Some(&**issue),
            Target::Form(_) => None,
            Target::Reply { .. } => None,
        };
        let list = |what| self.lookups.get(what, &team).cloned().unwrap_or_default();
        let item = |label: &str, value: &str, color: Option<String>, hint: &str, chosen: bool| PickItem {
            label: label.into(),
            value: value.into(),
            color,
            hint: hint.into(),
            chosen,
        };
        match field {
            Field::Priority => {
                let current = issue.map_or(form.map_or(0, |f| f.priority), |i| i.priority);
                PRIORITIES.iter().map(|(n, p)| item(n, &p.to_string(), None, "", *p == current)).collect()
            }
            Field::Team => list(Lookup::Teams)
                .iter()
                .map(|t| item(&t.name, &t.key, None, &t.key, form.is_some_and(|f| f.team == t.key)))
                .collect(),
            Field::State => {
                let current = issue
                    .and_then(|i| i.state.as_ref())
                    .map(|s| s.name.clone())
                    .or(form.and_then(|f| f.state.clone()));
                list(Lookup::States)
                    .iter()
                    .map(|s| {
                        item(&s.name, &s.name, s.color.clone(), &s.kind, current.as_deref() == Some(&s.name))
                    })
                    .collect()
            }
            Field::Assignee => {
                let current = match issue {
                    Some(i) => i.assignee.as_ref().map(|a| a.email.clone()),
                    None => form.and_then(|f| f.assignee.as_ref().map(|a| a.0.clone())),
                };
                let me = self.auth.as_ref().map(|a| a.user.email.clone());
                let mut v = vec![
                    item("Me", "me", None, me.as_deref().unwrap_or(""), current.is_some() && current == me),
                    item("Unassigned", "none", None, "", current.is_none()),
                ];
                v.extend(
                    list(Lookup::Users).iter().filter(|u| Some(&u.email) != me.as_ref()).map(|u| {
                        item(&u.name, &u.email, None, &u.email, current.as_deref() == Some(&u.email))
                    }),
                );
                v
            }
            Field::Labels => {
                let current: Vec<String> = match issue {
                    Some(i) => i.labels.iter().map(|l| l.name.clone()).collect(),
                    None => form.map(|f| f.labels.clone()).unwrap_or_default(),
                };
                list(Lookup::Labels)
                    .iter()
                    .map(|l| item(&l.name, &l.name, l.color.clone(), "", current.contains(&l.name)))
                    .collect()
            }
            Field::Project => {
                let current = match issue {
                    Some(i) => i.project.as_ref().map(|p| p.name.clone()),
                    None => form.and_then(|f| f.project.clone()),
                };
                list(Lookup::Projects)
                    .iter()
                    .map(|p| item(&p.name, &p.name, None, "", current.as_deref() == Some(&p.name)))
                    .collect()
            }
        }
    }

    fn open_picker(&mut self, target: Target) -> Vec<Effect> {
        let field = match &target {
            Target::Issue { field, .. } | Target::Form(field) => *field,
            Target::Reply { .. } => {
                let items = self.thread_items();
                let title = "Reply in which thread?".to_string();
                let list = PickList {
                    title,
                    items,
                    multi: false,
                    cursor: 0,
                    query: String::new(),
                    target,
                    waiting: None,
                };
                self.overlay = Some(Overlay::Pick(Box::new(list)));
                return Vec::new();
            }
        };
        let team = self.team_of(&target);
        let mut fx = Vec::new();
        let waiting = match lookup_for(field) {
            Some(what) if self.lookups.get(what, &team).is_none() => {
                fx = self.need(what, &team);
                Some((what, team))
            }
            _ => None,
        };
        let items = if waiting.is_some() { Vec::new() } else { self.pick_items(&target) };
        let cursor = items.iter().position(|i| i.chosen).unwrap_or(0);
        let title = title_of(field).to_string();
        let multi = field == Field::Labels;
        self.overlay = Some(Overlay::Pick(Box::new(PickList {
            title,
            items,
            multi,
            cursor,
            query: String::new(),
            target,
            waiting,
        })));
        fx
    }

    /// A dry run of an `issue update`.
    fn quick(
        &mut self,
        issue: &str,
        flags: &[(&str, &str)],
        what: String,
        undo: Option<Vec<(String, String)>>,
    ) -> Vec<Effect> {
        let argv = ctl::update_argv(issue, flags, Mode::DryRun);
        let purpose = Purpose::Quick { issue: issue.into(), what, undo };
        vec![Effect::Run(Request::Write { purpose, argv, text: None, summary: Vec::new() })]
    }

    /// The keys that change an issue, from the list or the detail.
    pub(super) fn issue_action(&mut self, k: KeyEvent) -> Option<Vec<Effect>> {
        let field = match k.code {
            KeyCode::Char('s') => Some(Field::State),
            KeyCode::Char('p') => Some(Field::Priority),
            KeyCode::Char('a') => Some(Field::Assignee),
            KeyCode::Char('L') => Some(Field::Labels),
            KeyCode::Char('P') => Some(Field::Project),
            _ => None,
        };
        if let Some(field) = field {
            let issue = self.target_issue()?;
            return Some(self.open_picker(Target::Issue { issue: Box::new(issue), field }));
        }
        match k.code {
            KeyCode::Char('u') => Some(self.start_undo()),
            KeyCode::Char('A') => {
                let issue = self.target_issue()?;
                let back = issue.assignee.as_ref().map_or("none".to_string(), |a| a.email.clone());
                let undo = Some(vec![("assignee".to_string(), back)]);
                Some(self.quick(&issue.identifier, &[("assignee", "me")], "assigned to you".into(), undo))
            }
            KeyCode::Char('x') => {
                let issue = self.target_issue()?;
                let argv = ctl::archive_argv(&issue.identifier, Mode::DryRun);
                let summary = vec![format!("  {}  {}", issue.identifier, issue.title)];
                let purpose = Purpose::Archive { issue: issue.identifier };
                Some(vec![Effect::Run(Request::Write { purpose, argv, text: None, summary })])
            }
            KeyCode::Char('c') if self.focus == Focus::Detail => {
                let issue = self.target_issue()?.identifier;
                Some(vec![Effect::Edit { purpose: EditFor::Comment { issue }, text: String::new() }])
            }
            KeyCode::Char('c') => Some(self.open_form()),
            KeyCode::Char('R') if self.focus == Focus::Detail => {
                let issue = self.target_issue()?.identifier;
                if !self.can_reply {
                    self.status = Some("Replies in a thread need linearctl 0.2.0 or later.".into());
                    return Some(Vec::new());
                }
                let threads = self.thread_items();
                match threads.as_slice() {
                    [] => {
                        self.notice = Some("No comments to reply to; c writes one.".into());
                        Some(Vec::new())
                    }
                    [only] => {
                        let (parent, to) = (only.value.clone(), only.hint.clone());
                        Some(vec![Effect::Edit {
                            purpose: EditFor::Reply { issue, parent, to },
                            text: String::new(),
                        }])
                    }
                    _ => Some(self.open_picker(Target::Reply { issue })),
                }
            }
            KeyCode::Char('E') if self.focus == Focus::Detail => {
                let d = self.detail.as_ref()?.issue.as_ref()?;
                let issue = d.row.identifier.clone();
                // As stored in Linear: without the signatures the detail was read with.
                let original = crate::images::unsign(d.description.as_deref().unwrap_or_default());
                let text = self.drafts.get(&issue).cloned().unwrap_or_else(|| original.clone());
                if self.drafts.contains_key(&issue) {
                    self.notice = Some("Editing your unsent draft.".into());
                }
                let updated_at = d.row.updated_at.clone();
                Some(vec![Effect::Edit {
                    purpose: EditFor::Description { issue, original, updated_at },
                    text,
                }])
            }
            _ => None,
        }
    }

    /// The open issue's threads: the author and first line of each root
    /// comment, its id as the value.
    fn thread_items(&self) -> Vec<PickItem> {
        let Some(issue) = self.detail.as_ref().and_then(|d| d.issue.as_ref()) else { return Vec::new() };
        let roots = issue.threads().into_iter().filter(|(_, reply)| !reply).map(|(c, _)| c);
        roots
            .map(|c| {
                let who = c.user.as_ref().map_or("Someone", |u| u.name.as_str()).to_string();
                let first: String = c.body.lines().next().unwrap_or_default().chars().take(50).collect();
                PickItem {
                    label: format!("{who}: {first}"),
                    value: c.id.clone(),
                    color: None,
                    hint: who,
                    chosen: false,
                }
            })
            .collect()
    }

    fn open_form(&mut self) -> Vec<Effect> {
        let team = self
            .selected_row()
            .and_then(|r| r.team.as_ref().map(|t| t.key.clone()))
            .or_else(|| self.lookups.get(Lookup::Teams, "").and_then(|t| t.first()).map(|t| t.key.clone()))
            .unwrap_or_default();
        self.form = Some(Form { id: uuid::Uuid::new_v4().to_string(), team, ..Form::default() });
        self.need(Lookup::Teams, "")
    }

    /// Keys while a picker, a confirm dialog or the form is open; `None`
    /// when none is.
    pub(super) fn write_key(&mut self, k: KeyEvent) -> Option<Vec<Effect>> {
        match self.overlay.take() {
            Some(Overlay::Pick(p)) => Some(self.pick_key(*p, k)),
            Some(Overlay::Confirm(c)) => Some(self.confirm_key(*c, k)),
            None if self.form.is_some() => Some(self.form_key(k)),
            None => None,
        }
    }

    fn confirm_key(&mut self, c: Confirm, k: KeyEvent) -> Vec<Effect> {
        match k.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                vec![Effect::Run(Request::Write {
                    purpose: c.purpose,
                    argv: c.argv,
                    text: c.text,
                    summary: Vec::new(),
                })]
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                if let (Purpose::Description { issue, .. }, Some(t)) = (&c.purpose, &c.text) {
                    self.drafts.insert(issue.clone(), t.text.clone());
                    self.notice = Some("Not sent. E edits your draft again.".into());
                } else {
                    self.notice = Some("Not sent.".into());
                }
                Vec::new()
            }
            _ => {
                self.overlay = Some(Overlay::Confirm(Box::new(c)));
                Vec::new()
            }
        }
    }

    fn pick_key(&mut self, mut p: PickList, k: KeyEvent) -> Vec<Effect> {
        let n = p.visible().len();
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => return Vec::new(),
            KeyCode::Down => p.cursor = (p.cursor + 1).min(n.saturating_sub(1)),
            KeyCode::Char('n') if ctrl => p.cursor = (p.cursor + 1).min(n.saturating_sub(1)),
            KeyCode::Up => p.cursor = p.cursor.saturating_sub(1),
            KeyCode::Char('p') if ctrl => p.cursor = p.cursor.saturating_sub(1),
            KeyCode::Char(' ') if p.multi => {
                if let Some(&(i, _)) = p.visible().get(p.cursor) {
                    p.items[i].chosen = !p.items[i].chosen;
                }
            }
            KeyCode::Enter => return self.choose(p),
            KeyCode::Backspace => {
                p.query.pop();
                p.cursor = 0;
            }
            KeyCode::Char(c) if !ctrl => {
                p.query.push(c);
                p.cursor = 0;
            }
            _ => {}
        }
        self.overlay = Some(Overlay::Pick(Box::new(p)));
        Vec::new()
    }

    /// Apply a picker's choice.
    fn choose(&mut self, p: PickList) -> Vec<Effect> {
        if p.waiting.is_some() {
            self.overlay = Some(Overlay::Pick(Box::new(p)));
            return Vec::new();
        }
        let current = p.visible().get(p.cursor).map(|(_, i)| (*i).clone());
        let chosen: Vec<String> = p.items.iter().filter(|i| i.chosen).map(|i| i.value.clone()).collect();
        match p.target {
            Target::Reply { issue } => {
                let Some(item) = current else { return Vec::new() };
                let purpose = EditFor::Reply { issue, parent: item.value, to: item.hint };
                vec![Effect::Edit { purpose, text: String::new() }]
            }
            Target::Issue { issue, field: Field::Labels } => {
                let have: Vec<String> = issue.labels.iter().map(|l| l.name.clone()).collect();
                let add: Vec<&String> = chosen.iter().filter(|c| !have.contains(c)).collect();
                let remove: Vec<&String> = have.iter().filter(|h| !chosen.contains(h)).collect();
                if add.is_empty() && remove.is_empty() {
                    self.notice = Some("Nothing to change.".into());
                    return Vec::new();
                }
                let mut flags: Vec<(&str, &str)> = add.iter().map(|l| ("add-label", l.as_str())).collect();
                flags.extend(remove.iter().map(|l| ("remove-label", l.as_str())));
                let what = format!(
                    "labels: {}",
                    if chosen.is_empty() { "none".to_string() } else { chosen.join(", ") }
                );
                let mut undo: Vec<(String, String)> =
                    add.iter().map(|l| ("remove-label".to_string(), (*l).clone())).collect();
                undo.extend(remove.iter().map(|l| ("add-label".to_string(), (*l).clone())));
                self.quick(&issue.identifier, &flags, what, Some(undo))
            }
            Target::Issue { issue, field } => {
                let Some(item) = current else { return Vec::new() };
                if item.chosen {
                    self.notice = Some("Nothing to change.".into());
                    return Vec::new();
                }
                let flag = match field {
                    Field::State => "state",
                    Field::Priority => "priority",
                    Field::Assignee => "assignee",
                    _ => "project",
                };
                let what = format!("{} → {}", title_of(field).to_lowercase(), item.label);
                // The old value; a project cannot be cleared (linearctl
                // 0.4.0 has no --project none), so setting one is final.
                let back = match field {
                    Field::State => issue.state.as_ref().map(|s| s.name.clone()),
                    Field::Priority => Some(issue.priority.to_string()),
                    Field::Assignee => {
                        Some(issue.assignee.as_ref().map_or("none".into(), |a| a.email.clone()))
                    }
                    _ => issue.project.as_ref().map(|p| p.name.clone()),
                };
                let undo = back.map(|b| vec![(flag.to_string(), b)]);
                self.quick(&issue.identifier, &[(flag, &item.value)], what, undo)
            }
            Target::Form(field) => {
                let form = self.form.as_mut().expect("a form picker has a form");
                match field {
                    Field::Labels => form.labels = chosen,
                    _ => {
                        let Some(item) = current else { return Vec::new() };
                        match field {
                            Field::Team => {
                                if form.team != item.value {
                                    // States, labels and projects are per team.
                                    form.state = None;
                                    form.labels.clear();
                                    form.project = None;
                                }
                                form.team = item.value;
                            }
                            Field::State => form.state = Some(item.value),
                            Field::Priority => form.priority = item.value.parse().unwrap_or(0),
                            Field::Assignee if item.value == "none" => form.assignee = None,
                            Field::Assignee => form.assignee = Some((item.value, item.label)),
                            _ => form.project = Some(item.value),
                        }
                    }
                }
                Vec::new()
            }
        }
    }

    fn form_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let form = self.form.as_mut().expect("form_key needs a form");
        let field = FORM_FIELDS[form.field];
        match k.code {
            KeyCode::Esc => {
                self.form = None;
                self.notice = Some("New issue discarded.".into());
            }
            KeyCode::Char('s') if ctrl => return self.submit_form(),
            KeyCode::Tab | KeyCode::Down => form.field = (form.field + 1) % FORM_FIELDS.len(),
            KeyCode::BackTab | KeyCode::Up => {
                form.field = (form.field + FORM_FIELDS.len() - 1) % FORM_FIELDS.len()
            }
            KeyCode::Backspace if field == "Title" => {
                form.title.pop();
            }
            KeyCode::Char(c) if field == "Title" && !ctrl => form.title.push(c),
            KeyCode::Enter if field == "Title" => form.field += 1,
            KeyCode::Enter if field == "Description" => {
                let text = form.description.clone();
                return vec![Effect::Edit { purpose: EditFor::FormDescription, text }];
            }
            KeyCode::Enter => {
                let f = match field {
                    "Team" => Field::Team,
                    "Status" => Field::State,
                    "Priority" => Field::Priority,
                    "Assignee" => Field::Assignee,
                    "Labels" => Field::Labels,
                    _ => Field::Project,
                };
                if f != Field::Team && f != Field::Priority && form.team.is_empty() {
                    self.status = Some("Pick a team first.".into());
                    return Vec::new();
                }
                return self.open_picker(Target::Form(f));
            }
            _ => {}
        }
        Vec::new()
    }

    fn submit_form(&mut self) -> Vec<Effect> {
        let Some(f) = self.form.clone() else {
            return Vec::new();
        };
        if f.title.trim().is_empty() || f.team.is_empty() {
            self.status = Some("A new issue needs a team and a title.".into());
            return Vec::new();
        }
        let priority = f.priority.to_string();
        let mut flags: Vec<(&str, &str)> = vec![("team", &f.team), ("title", f.title.trim())];
        if f.priority != 0 {
            flags.push(("priority", &priority));
        }
        if let Some(s) = &f.state {
            flags.push(("state", s));
        }
        if let Some((a, _)) = &f.assignee {
            flags.push(("assignee", a));
        }
        flags.extend(f.labels.iter().map(|l| ("label", l.as_str())));
        if let Some(p) = &f.project {
            flags.push(("project", p));
        }
        let argv = ctl::create_argv(&flags, &f.id, Mode::DryRun);
        let text = (!f.description.trim().is_empty())
            .then(|| Text { flag: "description", text: f.description.clone() });
        let mut summary = vec![
            format!("  {}", f.title.trim()),
            format!("  Team {}  ·  {}", f.team, priority_name(f.priority)),
        ];
        let mut extra = Vec::new();
        if let Some(s) = &f.state {
            extra.push(s.clone());
        }
        if let Some((_, a)) = &f.assignee {
            extra.push(a.clone());
        }
        if !f.labels.is_empty() {
            extra.push(f.labels.join(", "));
        }
        if let Some(p) = &f.project {
            extra.push(p.clone());
        }
        if !extra.is_empty() {
            summary.push(format!("  {}", extra.join("  ·  ")));
        }
        summary.extend(preview(&f.description, 4));
        vec![Effect::Run(Request::Write { purpose: Purpose::Create, argv, text, summary })]
    }

    /// The editor closed: `None` means it failed or was abandoned.
    pub(super) fn on_edited(&mut self, purpose: EditFor, text: Option<String>) -> Vec<Effect> {
        let Some(text) = text else {
            self.status = Some("The editor did not finish; nothing changed.".into());
            return Vec::new();
        };
        match purpose {
            EditFor::FormDescription => {
                if let Some(f) = self.form.as_mut() {
                    f.description = text.trim_end().to_string();
                }
                Vec::new()
            }
            EditFor::Comment { issue } => {
                let body = text.trim().to_string();
                if body.is_empty() {
                    self.notice = Some("Empty comment discarded.".into());
                    return Vec::new();
                }
                let id = uuid::Uuid::new_v4().to_string();
                let argv = ctl::comment_argv(&issue, &id, Mode::DryRun);
                let summary = preview(&body, 6);
                let text = Some(Text { flag: "body", text: body });
                vec![Effect::Run(Request::Write { purpose: Purpose::Comment { issue }, argv, text, summary })]
            }
            EditFor::Reply { issue, parent, to } => {
                let body = text.trim().to_string();
                if body.is_empty() {
                    self.notice = Some("Empty reply discarded.".into());
                    return Vec::new();
                }
                let id = uuid::Uuid::new_v4().to_string();
                let argv = ctl::reply_argv(&issue, &id, &parent, Mode::DryRun);
                let summary = preview(&body, 6);
                let text = Some(Text { flag: "body", text: body });
                vec![Effect::Run(Request::Write {
                    purpose: Purpose::Reply { issue, to },
                    argv,
                    text,
                    summary,
                })]
            }
            EditFor::Description { issue, original, updated_at } => {
                let new = text.trim_end().to_string();
                if new == original.trim_end() {
                    self.drafts.remove(&issue);
                    self.notice = Some("Description unchanged.".into());
                    return Vec::new();
                }
                // With linearctl 0.2.0, the write fails (exit 7) if someone
                // else changed the issue since it was loaded.
                let guard: Vec<(&str, &str)> = if self.can_check_updated {
                    vec![("if-updated-at", updated_at.as_str())]
                } else {
                    Vec::new()
                };
                let argv = ctl::update_argv(&issue, &guard, Mode::DryRun);
                let mut summary =
                    vec![format!("  {} → {} characters", original.chars().count(), new.chars().count())];
                summary.extend(preview(&new, 6));
                let text = Some(Text { flag: "description", text: new });
                vec![Effect::Run(Request::Write {
                    purpose: Purpose::Description { issue, original },
                    argv,
                    text,
                    summary,
                })]
            }
        }
    }

    /// A write answered: a dry run leads to `--yes` (directly for quick
    /// edits, else through the confirm dialog); `--yes` updates what is shown.
    pub(super) fn on_wrote(
        &mut self,
        purpose: Purpose,
        argv: Vec<String>,
        text: Option<Text>,
        summary: Vec<String>,
        result: Result<Box<WriteResult>, Failure>,
    ) -> Vec<Effect> {
        let r = match result {
            Ok(r) => *r,
            Err(Failure::Conflict(msg)) => {
                if let (Purpose::Description { issue, .. }, Some(t)) = (&purpose, &text) {
                    self.drafts.insert(issue.clone(), t.text.clone());
                    self.status = Some(format!(
                        "{issue} changed on Linear since you opened it ({msg}). Your text is kept: r reloads, E edits it again."
                    ));
                } else {
                    self.status = Some(msg);
                }
                return Vec::new();
            }
            Err(f) => {
                if let (Purpose::Description { issue, .. }, Some(t)) = (&purpose, &text) {
                    self.drafts.insert(issue.clone(), t.text.clone());
                }
                self.status = Some(f.message());
                return Vec::new();
            }
        };
        if r.dry_run {
            if !r.changed {
                self.notice = Some("Nothing to change.".into());
                return Vec::new();
            }
            let yes = ctl::with_mode(&argv, Mode::Yes);
            if matches!(purpose, Purpose::Quick { .. } | Purpose::Undo { .. }) {
                return vec![Effect::Run(Request::Write { purpose, argv: yes, text, summary: Vec::new() })];
            }
            let title = match &purpose {
                Purpose::Comment { issue } => format!("Comment on {issue}"),
                Purpose::Reply { issue, to } => format!("Reply to {to} on {issue}"),
                Purpose::Description { issue, .. } => format!("New description for {issue}"),
                Purpose::Archive { issue } => format!("Archive {issue}?"),
                Purpose::Create => "Create this issue?".into(),
                Purpose::Quick { issue, .. } => issue.clone(),
                Purpose::Undo { label, .. } => format!("Undo {label}"),
            };
            let command = shell_line(&yes, text.as_ref());
            self.overlay = Some(Overlay::Confirm(Box::new(Confirm {
                title,
                lines: summary,
                command,
                argv: yes,
                text,
                purpose,
            })));
            return Vec::new();
        }
        self.status = None;
        match purpose {
            Purpose::Quick { issue, what, undo } => {
                self.apply_row(r.issue);
                self.notice = Some(match (&undo, r.changed) {
                    (_, false) => "Nothing to change.".into(),
                    (Some(_), true) => format!("{issue}: {what} (u undoes)"),
                    (None, true) => format!("{issue}: {what} (linearctl cannot clear a project: no undo)"),
                });
                if r.changed {
                    self.undo = None;
                    if let Some(flags) = undo {
                        let step = UndoStep::Update { issue: issue.clone(), flags, text: None };
                        self.remember_undo(format!("{issue}: {what}"), step);
                    }
                }
                self.refresh_detail(&issue)
            }
            Purpose::Undo { label, step } => self.on_undone(label, step, r),
            Purpose::Description { issue, original } => {
                let text = Some(Text { flag: "description", text: original });
                let step = UndoStep::Update { issue: issue.clone(), flags: Vec::new(), text };
                self.remember_undo(format!("{issue}: description"), step);
                self.drafts.remove(&issue);
                self.apply_row(r.issue);
                self.notice = Some(format!("{issue}: description saved"));
                self.refresh_detail(&issue)
            }
            Purpose::Comment { issue } | Purpose::Reply { issue, .. } => {
                if let Some(c) = r.comment {
                    let step = UndoStep::DeleteComment { issue: issue.clone(), id: c.id.clone() };
                    let open = self.detail.as_mut().filter(|d| d.id == issue).and_then(|d| d.issue.as_mut());
                    if let Some(i) = open
                        && !i.comments.iter().any(|x| x.id == c.id)
                    {
                        i.comments.push(c);
                        self.cache.insert(issue.clone(), i.clone());
                    }
                    self.remember_undo(format!("comment on {issue}"), step);
                }
                self.notice = Some(format!("Comment added to {issue}"));
                Vec::new()
            }
            Purpose::Archive { issue } => {
                self.rows.retain(|row| row.identifier != issue);
                self.selected = self.selected.min(self.visible().len().saturating_sub(1));
                self.cache.remove(&issue);
                if self.detail.as_ref().is_some_and(|d| d.id == issue) {
                    self.detail = None;
                    self.focus = Focus::List;
                }
                self.remember_undo(
                    format!("archiving {issue}"),
                    UndoStep::Unarchive { issue: issue.clone() },
                );
                self.notice = Some(format!("Archived {issue}"));
                Vec::new()
            }
            Purpose::Create => {
                self.form = None;
                let Some(row) = r.issue else { return Vec::new() };
                let id = row.identifier.clone();
                if !self.rows.iter().any(|x| x.id == row.id) {
                    self.rows.insert(0, row);
                }
                self.filter.clear();
                self.selected = 0;
                self.remember_undo(format!("creating {id}"), UndoStep::Archive { issue: id.clone() });
                self.notice = Some(format!("Created {id}"));
                self.focus = Focus::Detail;
                self.detail = Some(super::Detail { id: id.clone(), issue: None, error: None, scroll: 0 });
                vec![Effect::Run(Request::Show { argv: ctl::show_argv(&id), id })]
            }
        }
    }

    /// Replace a row with what a write returned.
    pub(super) fn apply_row(&mut self, row: Option<IssueRow>) {
        let Some(row) = row else { return };
        if let Some(r) = self.rows.iter_mut().find(|r| r.id == row.id) {
            *r = row;
        }
    }

    /// Fetch an issue again if it is open, and forget its cached copy.
    pub(super) fn refresh_detail(&mut self, issue: &str) -> Vec<Effect> {
        self.cache.remove(issue);
        if self.detail.as_ref().is_some_and(|d| d.id == issue) {
            return vec![Effect::Run(Request::Show { argv: ctl::show_argv(issue), id: issue.to_string() })];
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Msg, default_views};
    use crate::fake::InProcess;
    use chrono::{DateTime, Utc};
    use crossterm::event::KeyEvent;
    use std::path::PathBuf;

    fn now() -> DateTime<Utc> {
        "2026-10-26T12:00:00Z".parse().unwrap()
    }

    /// An app on the fake, whose writes persist in a file of their own.
    struct World {
        app: App,
        exec: InProcess,
        /// Editor requests seen.
        edits: Vec<(EditFor, String)>,
    }

    impl Drop for World {
        fn drop(&mut self) {
            if let Some(p) = &self.exec.state {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    fn world(name: &str) -> World {
        let state = std::env::temp_dir().join(format!("linear-tui-write-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&state);
        let mut w = World {
            app: App::new(default_views(), now()),
            exec: InProcess { now: now(), state: Some(state) },
            edits: vec![],
        };
        let fx = w.app.start();
        w.run(fx);
        w
    }

    impl World {
        fn run(&mut self, fx: Vec<Effect>) {
            for f in fx {
                match f {
                    Effect::Run(req) => {
                        let msg = req.run(&self.exec);
                        let more = self.app.update(msg);
                        self.run(more);
                    }
                    Effect::Edit { purpose, text } => self.edits.push((purpose, text)),
                    _ => {}
                }
            }
        }

        fn key(&mut self, code: KeyCode) {
            let fx = self.app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
            self.run(fx);
        }

        fn ctrl(&mut self, c: char) {
            let fx = self.app.update(Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)));
            self.run(fx);
        }

        fn chars(&mut self, s: &str) {
            for c in s.chars() {
                self.key(KeyCode::Char(c));
            }
        }

        fn edited(&mut self, text: Option<&str>) {
            let (purpose, _) = self.edits.pop().expect("an editor was asked for");
            let fx = self.app.update(Msg::Edited { purpose, text: text.map(str::to_string) });
            self.run(fx);
        }

        fn picker(&self) -> &PickList {
            match &self.app.overlay {
                Some(Overlay::Pick(p)) => p,
                other => panic!("no picker: {other:?}"),
            }
        }

        fn confirm(&self) -> &Confirm {
            match &self.app.overlay {
                Some(Overlay::Confirm(c)) => c,
                other => panic!("no confirm dialog: {other:?}"),
            }
        }

        fn row(&self, id: &str) -> &IssueRow {
            self.app.rows.iter().find(|r| r.identifier == id).expect(id)
        }

        fn show(&self, id: &str) -> crate::model::IssueDetail {
            let argv = ctl::show_argv(id);
            ctl::run_as::<crate::model::ShowResult>(&self.exec, &argv).unwrap().issue
        }
    }

    #[test]
    fn quick_state_edit_through_a_picker() {
        let mut w = world("state");
        w.key(KeyCode::Char('s'));
        let p = w.picker();
        assert_eq!(p.title, "Status");
        assert!(p.waiting.is_none(), "the states loaded");
        assert_eq!(
            p.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>()[..3],
            ["Triage", "Backlog", "Todo"]
        );
        assert_eq!(p.items[p.cursor].label, "In Progress", "starts on the current state");
        assert!(p.items[p.cursor].chosen);
        w.chars("rev");
        assert_eq!(w.picker().visible().len(), 1);
        w.key(KeyCode::Enter);
        assert!(w.app.overlay.is_none());
        assert_eq!(w.app.notice.as_deref(), Some("ENG-142: status → In Review (u undoes)"));
        assert_eq!(w.row("ENG-142").state.as_ref().unwrap().name, "In Review");
        assert_eq!(w.show("ENG-142").row.state.unwrap().name, "In Review", "written");
    }

    #[test]
    fn choosing_what_is_already_set_changes_nothing() {
        let mut w = world("same");
        w.key(KeyCode::Char('s'));
        w.key(KeyCode::Enter);
        assert_eq!(w.app.notice.as_deref(), Some("Nothing to change."));
        // Lookups are loaded once.
        w.key(KeyCode::Char('s'));
        assert!(!w.picker().items.is_empty());
        w.key(KeyCode::Esc);
        assert!(w.app.overlay.is_none());
    }

    #[test]
    fn priority_assignee_project() {
        let mut w = world("prio");
        w.key(KeyCode::Char('p'));
        assert_eq!(w.picker().items[w.picker().cursor].label, "Urgent");
        w.key(KeyCode::Down);
        w.key(KeyCode::Down);
        w.ctrl('n');
        w.ctrl('p');
        w.key(KeyCode::Down);
        w.key(KeyCode::Up);
        w.key(KeyCode::Down);
        w.key(KeyCode::Enter);
        assert_eq!(w.app.notice.as_deref(), Some("ENG-142: priority → Low (u undoes)"));
        assert_eq!(w.row("ENG-142").priority, 4);

        w.key(KeyCode::Char('a'));
        let labels: Vec<&str> = w.picker().items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels[..2], ["Me", "Unassigned"]);
        assert!(!labels.contains(&"Maya Patel"), "the viewer is listed as Me");
        assert!(w.picker().items[0].chosen);
        w.chars("jonas");
        w.key(KeyCode::Enter);
        assert_eq!(w.row("ENG-142").assignee.as_ref().unwrap().name, "Jonas Berg");

        w.key(KeyCode::Char('P'));
        w.chars("mobile");
        w.key(KeyCode::Enter);
        assert_eq!(w.row("ENG-142").project.as_ref().unwrap().name, "Mobile 3.0");
    }

    #[test]
    fn labels_add_and_remove_together() {
        let mut w = world("labels");
        w.key(KeyCode::Char('L'));
        assert!(w.picker().multi);
        let chosen: Vec<&str> =
            w.picker().items.iter().filter(|i| i.chosen).map(|i| i.label.as_str()).collect();
        assert_eq!(chosen, ["Bug", "Customer reported"]);
        w.chars("secu");
        w.key(KeyCode::Char(' '));
        w.key(KeyCode::Backspace);
        w.key(KeyCode::Backspace);
        w.key(KeyCode::Backspace);
        w.key(KeyCode::Backspace);
        w.chars("bug");
        w.key(KeyCode::Char(' '));
        w.key(KeyCode::Enter);
        assert_eq!(w.app.notice.as_deref(), Some("ENG-142: labels: Security, Customer reported (u undoes)"));
        let names: Vec<&str> = w.row("ENG-142").labels.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Customer reported", "Security"]);
        w.key(KeyCode::Char('L'));
        w.key(KeyCode::Enter);
        assert_eq!(w.app.notice.as_deref(), Some("Nothing to change."));
        // Removing every label.
        w.key(KeyCode::Char('L'));
        w.chars("cust");
        w.key(KeyCode::Char(' '));
        w.key(KeyCode::Backspace);
        w.key(KeyCode::Backspace);
        w.key(KeyCode::Backspace);
        w.key(KeyCode::Backspace);
        w.chars("secu");
        w.key(KeyCode::Char(' '));
        w.key(KeyCode::Enter);
        assert_eq!(w.app.notice.as_deref(), Some("ENG-142: labels: none (u undoes)"));
    }

    #[test]
    fn assign_to_me_and_archive() {
        let mut w = world("archive");
        w.key(KeyCode::Char('A'));
        assert_eq!(w.app.notice.as_deref(), Some("Nothing to change."), "ENG-142 is already mine");
        w.key(KeyCode::Char('2'));
        assert_eq!(w.app.selected_row().unwrap().identifier, "ENG-158");
        w.key(KeyCode::Char('A'));
        assert_eq!(w.app.notice.as_deref(), Some("ENG-158: assigned to you (u undoes)"));
        w.key(KeyCode::Char('x'));
        let c = w.confirm();
        assert_eq!(c.title, "Archive ENG-158?");
        assert_eq!(c.command, "linearctl issue archive ENG-158 --yes --json");
        w.key(KeyCode::Char('q'));
        assert!(w.app.overlay.is_some(), "other keys keep the dialog");
        w.key(KeyCode::Char('n'));
        assert_eq!(w.app.notice.as_deref(), Some("Not sent."));
        assert!(w.app.rows.iter().any(|r| r.identifier == "ENG-158"));
        w.key(KeyCode::Char('x'));
        w.key(KeyCode::Char('y'));
        assert_eq!(w.app.notice.as_deref(), Some("Archived ENG-158"));
        assert!(!w.app.rows.iter().any(|r| r.identifier == "ENG-158"));
    }

    #[test]
    fn archiving_the_open_issue_closes_it() {
        let mut w = world("archive-open");
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('x'));
        w.key(KeyCode::Enter);
        assert!(w.app.detail.is_none());
        assert_eq!(w.app.focus, Focus::List);
    }

    #[test]
    fn comments_go_through_the_editor_and_a_confirm() {
        let mut w = world("comment");
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('c'));
        assert_eq!(w.edits.last().unwrap(), &(EditFor::Comment { issue: "ENG-142".into() }, String::new()));
        w.edited(Some("Deployed to staging.\n\nChecking the partner API next."));
        let c = w.confirm();
        assert_eq!(c.title, "Comment on ENG-142");
        assert_eq!(c.lines, ["  Deployed to staging.", "  ", "  Checking the partner API next."]);
        assert!(c.command.starts_with("linearctl comment create ENG-142 --id "), "{}", c.command);
        assert!(c.command.ends_with(" --body-file FILE --yes --json"), "{}", c.command);
        w.key(KeyCode::Char('y'));
        assert_eq!(w.app.notice.as_deref(), Some("Comment added to ENG-142"));
        let shown = w.app.detail.as_ref().unwrap().issue.as_ref().unwrap();
        assert_eq!(shown.comments.len(), 5);
        assert!(shown.comments[4].body.starts_with("Deployed to staging."));
        assert_eq!(w.show("ENG-142").comments.len(), 5, "written");
    }

    #[test]
    fn replying_in_a_thread() {
        let mut w = world("reply");
        w.app.can_reply = true;
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('R'));
        let labels: Vec<&str> = w.picker().items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels.len(), 3, "three threads: {labels:?}");
        assert!(labels[1].starts_with("Daniel Okafor: Careful with case too"));
        w.chars("dan");
        w.key(KeyCode::Enter);
        assert!(matches!(&w.edits.last().unwrap().0, EditFor::Reply { to, .. } if to == "Daniel Okafor"));
        w.edited(Some("Good catch, done in #2841."));
        let c = w.confirm();
        assert_eq!(c.title, "Reply to Daniel Okafor on ENG-142");
        assert!(c.command.contains(" --parent "), "{}", c.command);
        w.key(KeyCode::Char('y'));
        assert_eq!(w.app.notice.as_deref(), Some("Comment added to ENG-142"));
        let shown = w.app.detail.as_ref().unwrap().issue.as_ref().unwrap();
        let order: Vec<(&str, bool)> =
            shown.threads().iter().map(|(c, r)| (c.body.lines().next().unwrap_or(""), *r)).collect();
        assert_eq!(order[3], ("Good catch, done in #2841.", true), "under Daniel's comment: {order:?}");
        // An empty reply is dropped.
        w.key(KeyCode::Char('R'));
        w.key(KeyCode::Enter);
        w.edited(Some(""));
        assert_eq!(w.app.notice.as_deref(), Some("Empty reply discarded."));
    }

    #[test]
    fn replying_with_one_thread_or_none() {
        let mut w = world("reply-one");
        w.app.can_reply = true;
        w.key(KeyCode::Char('2')); // Active: ENG-158 first, one comment
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('R'));
        assert!(w.app.overlay.is_none(), "no picker for one thread");
        assert!(matches!(&w.edits.last().unwrap().0, EditFor::Reply { to, .. } if to == "Jonas Berg"));
        w.key(KeyCode::Esc);
        w.key(KeyCode::Char('3')); // Triage: ENG-157, no comments
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('R'));
        assert_eq!(w.app.notice.as_deref(), Some("No comments to reply to; c writes one."));
        w.app.can_reply = false;
        w.key(KeyCode::Char('R'));
        assert_eq!(w.app.status.as_deref(), Some("Replies in a thread need linearctl 0.2.0 or later."));
    }

    #[test]
    fn u_undoes_quick_edits() {
        let mut w = world("undo-quick");
        w.key(KeyCode::Char('u'));
        assert_eq!(w.app.notice.as_deref(), Some("Nothing to undo."));
        w.key(KeyCode::Char('s'));
        w.chars("done");
        w.key(KeyCode::Enter);
        assert_eq!(w.row("ENG-142").state.as_ref().unwrap().name, "Done");
        w.key(KeyCode::Char('u'));
        assert_eq!(w.app.notice.as_deref(), Some("Undone: ENG-142: status → Done"));
        assert_eq!(w.row("ENG-142").state.as_ref().unwrap().name, "In Progress");
        assert_eq!(w.show("ENG-142").row.state.unwrap().name, "In Progress", "written back");
        w.key(KeyCode::Char('u'));
        assert_eq!(w.app.notice.as_deref(), Some("Nothing to undo."), "one level");
        // Labels: added ones come off, removed ones go back.
        w.key(KeyCode::Char('L'));
        w.chars("bug");
        w.key(KeyCode::Char(' '));
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('u'));
        let labels: Vec<String> = w.row("ENG-142").labels.iter().map(|l| l.name.clone()).collect();
        assert!(labels.contains(&"Bug".to_string()), "{labels:?}");
        // Assignee back to someone else.
        w.key(KeyCode::Char('a'));
        w.key(KeyCode::Down);
        w.key(KeyCode::Enter);
        assert!(w.row("ENG-142").assignee.is_none());
        w.key(KeyCode::Char('u'));
        assert_eq!(w.row("ENG-142").assignee.as_ref().unwrap().name, "Maya Patel");
    }

    #[test]
    fn a_project_set_where_there_was_none_cannot_be_undone() {
        let mut w = world("undo-project");
        w.key(KeyCode::Char('2'));
        while w.app.selected_row().unwrap().identifier != "ENG-152" {
            w.key(KeyCode::Down);
        }
        w.key(KeyCode::Char('P'));
        w.chars("mobile");
        w.key(KeyCode::Enter);
        assert_eq!(
            w.app.notice.as_deref(),
            Some("ENG-152: project → Mobile 3.0 (linearctl cannot clear a project: no undo)")
        );
        assert!(w.app.undo.is_none());
        // From one project to another: undoable.
        w.key(KeyCode::Char('P'));
        w.chars("checkout");
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('u'));
        assert_eq!(w.row("ENG-152").project.as_ref().unwrap().name, "Mobile 3.0");
    }

    #[test]
    fn u_undoes_archive_comment_description_and_create() {
        let mut w = world("undo-more");
        w.key(KeyCode::Char('x'));
        w.key(KeyCode::Char('y'));
        assert!(!w.app.rows.iter().any(|r| r.identifier == "ENG-142"));
        w.key(KeyCode::Char('u'));
        assert_eq!(w.app.notice.as_deref(), Some("Undone: archiving ENG-142"));
        assert!(w.app.rows.iter().any(|r| r.identifier == "ENG-142"), "back in the list");
        assert_eq!(w.app.selected_row().unwrap().identifier, "ENG-143", "the selection stayed");
        // A comment.
        w.key(KeyCode::Char('g'));
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('c'));
        w.edited(Some("Oops, wrong issue."));
        w.key(KeyCode::Char('y'));
        assert_eq!(w.app.detail.as_ref().unwrap().issue.as_ref().unwrap().comments.len(), 5);
        w.key(KeyCode::Char('u'));
        assert_eq!(w.app.notice.as_deref(), Some("Undone: comment on ENG-142"));
        assert_eq!(w.app.detail.as_ref().unwrap().issue.as_ref().unwrap().comments.len(), 4);
        assert_eq!(w.show("ENG-142").comments.len(), 4);
        // A description.
        let before = w.show("ENG-142").description.unwrap();
        w.key(KeyCode::Char('E'));
        w.edited(Some("Short."));
        w.key(KeyCode::Char('y'));
        w.key(KeyCode::Char('u'));
        assert_eq!(w.show("ENG-142").description.unwrap(), before.trim_end());
        // A new issue is archived.
        w.key(KeyCode::Esc);
        w.key(KeyCode::Char('c'));
        w.chars("Temporary");
        w.ctrl('s');
        w.key(KeyCode::Char('y'));
        assert_eq!(w.app.notice.as_deref(), Some("Created ENG-159"));
        w.key(KeyCode::Char('u'));
        assert_eq!(w.app.notice.as_deref(), Some("Undone: creating ENG-159"));
        assert!(w.app.detail.is_none());
        assert!(!w.app.rows.iter().any(|r| r.identifier == "ENG-159"));
    }

    #[test]
    fn no_undo_that_linearctl_cannot_run() {
        let mut w = world("undo-old");
        let old = include_str!("../../tests/fixtures/linearctl-describe-0.1.0.json");
        w.app.describe = Some(crate::contract::Describe::from_json(old).unwrap());
        w.key(KeyCode::Char('x'));
        w.key(KeyCode::Char('y'));
        assert!(w.app.undo.is_none(), "0.1.0 has no issue unarchive");
        w.key(KeyCode::Char('s'));
        w.chars("todo");
        w.key(KeyCode::Enter);
        assert!(w.app.undo.is_some(), "an update it can do");
    }

    #[test]
    fn empty_or_abandoned_comments() {
        let mut w = world("comment-empty");
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('c'));
        w.edited(Some("  \n"));
        assert_eq!(w.app.notice.as_deref(), Some("Empty comment discarded."));
        w.key(KeyCode::Char('c'));
        w.edited(None);
        assert_eq!(w.app.status.as_deref(), Some("The editor did not finish; nothing changed."));
    }

    #[test]
    fn editing_the_description() {
        let mut w = world("description");
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('E'));
        let (purpose, text) = w.edits.last().unwrap().clone();
        assert!(text.starts_with("Customers who paste"));
        assert!(
            text.contains("uploads.linear.app/demo/checkout-error.png)"),
            "unsigned, as Linear stores it"
        );
        assert!(!text.contains("signature="));
        assert!(matches!(purpose, EditFor::Description { ref issue, .. } if issue == "ENG-142"));
        w.edited(Some(&format!("{text}\n")));
        assert_eq!(w.app.notice.as_deref(), Some("Description unchanged."));
        w.key(KeyCode::Char('E'));
        w.edited(Some("Short now."));
        let c = w.confirm();
        assert_eq!(c.title, "New description for ENG-142");
        assert!(c.lines[0].contains("→ 10 characters"), "{:?}", c.lines);
        // Cancelling keeps a draft; E opens it.
        w.key(KeyCode::Esc);
        assert_eq!(w.app.notice.as_deref(), Some("Not sent. E edits your draft again."));
        w.key(KeyCode::Char('E'));
        assert_eq!(w.edits.last().unwrap().1, "Short now.");
        assert_eq!(w.app.notice.as_deref(), Some("Editing your unsent draft."));
        w.edited(Some("Short now."));
        w.key(KeyCode::Enter);
        assert_eq!(w.app.notice.as_deref(), Some("ENG-142: description saved"));
        assert!(w.app.drafts.is_empty());
        let shown = w.app.detail.as_ref().unwrap().issue.as_ref().unwrap();
        assert_eq!(shown.description.as_deref(), Some("Short now."), "fetched again");
    }

    #[test]
    fn a_failed_description_write_keeps_the_text() {
        let mut w = world("description-fail");
        // The fake is linearctl 0.1.0, which has no --if-updated-at: the
        // dry run is refused, as a real conflict would be.
        w.app.can_check_updated = true;
        w.key(KeyCode::Enter);
        w.key(KeyCode::Char('E'));
        w.edited(Some("Mine."));
        assert!(w.app.status.as_deref().unwrap().contains("unknown flag: --if-updated-at"));
        assert_eq!(w.app.drafts["ENG-142"], "Mine.");
        // A conflict (exit 7) says what happened.
        let text = Some(Text { flag: "description", text: "Mine again.".into() });
        let fx = w.app.update(Msg::Wrote {
            purpose: Purpose::Description { issue: "ENG-142".into(), original: String::new() },
            argv: vec![],
            text,
            summary: vec![],
            result: Err(Failure::Conflict("updated at 12:01".into())),
        });
        assert!(fx.is_empty());
        assert!(
            w.app
                .status
                .as_deref()
                .unwrap()
                .starts_with("ENG-142 changed on Linear since you opened it (updated at 12:01)")
        );
        assert_eq!(w.app.drafts["ENG-142"], "Mine again.");
        // Any other write's conflict is just its message.
        w.app.update(Msg::Wrote {
            purpose: Purpose::Archive { issue: "ENG-142".into() },
            argv: vec![],
            text: None,
            summary: vec![],
            result: Err(Failure::Conflict("busy".into())),
        });
        assert_eq!(w.app.status.as_deref(), Some("busy"));
    }

    #[test]
    fn writes_refused_by_linearctl_show_why() {
        let mut w = world("refused");
        w.exec.state = None; // read-only: --yes fails
        w.key(KeyCode::Char('p'));
        w.key(KeyCode::Down);
        w.key(KeyCode::Enter);
        assert_eq!(w.app.status.as_deref(), Some("the demo workspace is read-only"));
    }

    #[test]
    fn a_lookup_that_fails_closes_its_picker() {
        let mut w = world("lookup-fail");
        let fx = w.app.update(Msg::Key(KeyEvent::from(KeyCode::Char('s'))));
        assert!(w.picker().waiting.is_some());
        // Enter while loading does nothing.
        w.app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
        assert!(w.picker().waiting.is_some());
        drop(fx);
        w.app.update(Msg::Looked {
            what: Lookup::States,
            team: "ENG".into(),
            result: Err(Failure::Failed("offline".into())),
        });
        assert!(w.app.overlay.is_none());
        assert_eq!(w.app.status.as_deref(), Some("offline"));
        // A later success for a picker no longer open is just kept.
        w.app.update(Msg::Looked { what: Lookup::Users, team: String::new(), result: Ok(Page::default()) });
        assert!(w.app.lookups.get(Lookup::Users, "ENG").is_some());
    }

    #[test]
    fn creating_an_issue() {
        let mut w = world("create");
        w.key(KeyCode::Char('c'));
        let f = w.app.form.as_ref().unwrap();
        assert_eq!(f.team, "ENG", "the selected row's team");
        assert_eq!(f.id.len(), 36);
        w.chars("Gift cards at checkout");
        w.key(KeyCode::Backspace);
        w.chars("t");
        w.key(KeyCode::Enter); // to Team
        w.key(KeyCode::Enter);
        assert_eq!(w.picker().title, "Team");
        w.key(KeyCode::Enter);
        w.key(KeyCode::Tab); // Status
        w.key(KeyCode::Enter);
        w.chars("todo");
        w.key(KeyCode::Enter);
        w.key(KeyCode::Down); // Priority
        w.key(KeyCode::Enter);
        assert_eq!(w.picker().items[w.picker().cursor].label, "No priority");
        w.key(KeyCode::Up);
        w.key(KeyCode::Up);
        w.key(KeyCode::Up);
        w.key(KeyCode::Enter);
        w.key(KeyCode::Tab); // Assignee
        w.key(KeyCode::Enter);
        assert_eq!(w.picker().items[w.picker().cursor].label, "Unassigned");
        w.key(KeyCode::Enter);
        assert_eq!(w.app.form.as_ref().unwrap().assignee, None);
        w.key(KeyCode::Enter);
        w.key(KeyCode::Up);
        w.key(KeyCode::Enter);
        w.key(KeyCode::Tab); // Labels
        w.key(KeyCode::Enter);
        w.chars("feat");
        w.key(KeyCode::Char(' '));
        w.key(KeyCode::Enter);
        w.key(KeyCode::Tab); // Project
        w.key(KeyCode::Enter);
        w.chars("checkout");
        w.key(KeyCode::Enter);
        w.key(KeyCode::Tab); // Description
        w.key(KeyCode::Enter);
        w.edited(Some("Sell and redeem gift cards.\n"));
        let f = w.app.form.as_ref().unwrap();
        assert_eq!(
            (f.title.as_str(), f.state.as_deref(), f.priority, f.labels.clone(), f.project.as_deref()),
            ("Gift cards at checkout", Some("Todo"), 2, vec!["Feature".to_string()], Some("Checkout v2"))
        );
        assert_eq!(f.assignee, Some(("me".into(), "Me".into())));
        assert_eq!(f.description, "Sell and redeem gift cards.");
        w.key(KeyCode::BackTab);
        w.key(KeyCode::Up);
        w.ctrl('s');
        let c = w.confirm();
        assert_eq!(c.title, "Create this issue?");
        assert_eq!(
            c.lines[..3],
            ["  Gift cards at checkout", "  Team ENG  ·  High", "  Todo  ·  Me  ·  Feature  ·  Checkout v2"]
        );
        assert!(c.command.contains("--title 'Gift cards at checkout'"), "{}", c.command);
        let id = w.app.form.as_ref().unwrap().id.clone();
        w.key(KeyCode::Char('y'));
        assert!(w.app.form.is_none());
        assert_eq!(w.app.notice.as_deref(), Some("Created ENG-159"));
        assert_eq!(w.app.focus, Focus::Detail);
        let shown = w.app.detail.as_ref().unwrap().issue.as_ref().unwrap();
        assert_eq!(shown.row.id, id, "the id the form made");
        assert_eq!(shown.description.as_deref(), Some("Sell and redeem gift cards."));
        assert_eq!(w.app.rows[0].identifier, "ENG-159");
    }

    #[test]
    fn the_form_checks_and_discards() {
        let mut w = world("form-checks");
        w.key(KeyCode::Char('c'));
        w.ctrl('s');
        assert_eq!(w.app.status.as_deref(), Some("A new issue needs a team and a title."));
        w.app.form.as_mut().unwrap().team.clear();
        w.key(KeyCode::Tab);
        w.key(KeyCode::Tab);
        w.key(KeyCode::Enter);
        assert_eq!(w.app.status.as_deref(), Some("Pick a team first."));
        // Changing the team clears what was per team.
        w.app.form.as_mut().unwrap().team = "ENG".into();
        w.app.form.as_mut().unwrap().labels = vec!["Bug".into()];
        let teams = Page {
            items: vec![Named { key: "WEB".into(), name: "Web".into(), ..Named::default() }],
            ..Page::default()
        };
        w.app.update(Msg::Looked { what: Lookup::Teams, team: String::new(), result: Ok(teams) });
        w.key(KeyCode::Up);
        w.key(KeyCode::Enter);
        w.key(KeyCode::Enter);
        assert_eq!(w.app.form.as_ref().unwrap().team, "WEB");
        assert!(w.app.form.as_ref().unwrap().labels.is_empty());
        w.key(KeyCode::Esc);
        assert!(w.app.form.is_none());
        assert_eq!(w.app.notice.as_deref(), Some("New issue discarded."));
    }

    #[test]
    fn control_keys_do_not_type_into_a_picker() {
        let mut w = world("ctrl");
        w.key(KeyCode::Char('s'));
        w.ctrl('s');
        assert_eq!(w.picker().query, "");
    }

    #[test]
    fn a_picker_with_no_match() {
        let mut w = world("nomatch");
        w.key(KeyCode::Char('s'));
        w.chars("zzz");
        assert!(w.picker().visible().is_empty());
        w.key(KeyCode::Enter);
        assert!(w.app.overlay.is_none());
        assert!(w.app.notice.is_none());
        w.key(KeyCode::Char('c'));
        w.key(KeyCode::Tab);
        w.key(KeyCode::Tab);
        w.key(KeyCode::Enter);
        w.chars("zzz");
        w.key(KeyCode::Enter);
        assert!(w.app.form.as_ref().unwrap().state.is_none());
    }

    #[test]
    fn actions_need_an_issue() {
        let mut app = App::new(default_views(), now());
        for c in ['s', 'A', 'x'] {
            assert!(app.update(Msg::Key(KeyEvent::from(KeyCode::Char(c)))).is_empty());
        }
        assert!(app.overlay.is_none());
    }

    #[test]
    fn helpers() {
        let argv: Vec<String> = ["issue", "update", "ENG-1", "--title", "It's done", "--yes", "--json"]
            .map(String::from)
            .to_vec();
        let t = Text { flag: "description", text: "x".into() };
        assert_eq!(
            shell_line(&argv, Some(&t)),
            "linearctl issue update ENG-1 --title \"It's done\" --description-file FILE --yes --json"
        );
        assert_eq!(preview("a\nb\nc", 2), ["  a", "  b", "  …"]);
        assert_eq!(priority_name(9), "No priority");
        assert_eq!(scope(Lookup::Users, "ENG"), "");
        assert_eq!(scope(Lookup::Labels, "ENG"), "ENG");
        let _ = PathBuf::new();
    }
}
