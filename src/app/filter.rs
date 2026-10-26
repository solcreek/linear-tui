//! The filter panel (`f`): one row per `issue list` filter, each set
//! through a picker, applied as a temporary view or saved to config.toml.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::write::{Overlay, PickItem, PickList, Target};
use super::{App, Effect, View};
use crate::ctl::{self, Lookup};

/// A row of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Team,
    Assignee,
    StateType,
    State,
    LabelsAll,
    LabelsAny,
    LabelsNone,
    Project,
    Cycle,
    Priority,
    Updated,
    Creator,
    DueAfter,
    DueBefore,
    Sort,
    Reverse,
}

/// The rows, their names, and the `issue list` flag each sets.
pub const SLOTS: [(Slot, &str, &str); 16] = [
    (Slot::Team, "Team", "--team"),
    (Slot::Assignee, "Assignee", "--assignee"),
    (Slot::StateType, "Status type", "--state-type"),
    (Slot::State, "Status", "--state"),
    (Slot::LabelsAll, "All labels", "--label"),
    (Slot::LabelsAny, "Any label", "--any-label"),
    (Slot::LabelsNone, "No label", "--without-label"),
    (Slot::Project, "Project", "--project"),
    (Slot::Cycle, "Cycle", "--cycle"),
    (Slot::Priority, "Priority", "--priority"),
    (Slot::Updated, "Updated", "--updated-since"),
    (Slot::Creator, "Creator", "--creator"),
    (Slot::DueAfter, "Due after", "--due-after"),
    (Slot::DueBefore, "Due before", "--due-before"),
    (Slot::Sort, "Sort", "--sort"),
    (Slot::Reverse, "Reverse", "--reverse"),
];

fn flag_of(slot: Slot) -> &'static str {
    SLOTS.iter().find(|s| s.0 == slot).map_or("", |s| s.2)
}

fn multi(slot: Slot) -> bool {
    matches!(
        slot,
        Slot::StateType | Slot::State | Slot::LabelsAll | Slot::LabelsAny | Slot::LabelsNone | Slot::Priority
    )
}

/// The filters being edited.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FilterPanel {
    /// Values by row, in `SLOTS` order: one for single rows, any for multi.
    pub values: Vec<Vec<String>>,
    /// Flags the panel has no row for, kept as they were.
    pub extra: Vec<String>,
    /// Index into the rows shown.
    pub row: usize,
    /// The name being typed to save it as a view.
    pub naming: Option<String>,
    /// Rows linearctl supports (Priority needs 0.2.0).
    pub slots: Vec<Slot>,
}

impl FilterPanel {
    /// The panel for these `issue list` flags.
    pub fn from_args(args: &[String], slots: Vec<Slot>) -> Self {
        let mut p = FilterPanel { values: vec![Vec::new(); SLOTS.len()], slots, ..Default::default() };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--reverse" && p.slots.contains(&Slot::Reverse) {
                p.values[Self::index(Slot::Reverse)] = vec!["yes".into()];
                continue;
            }
            match SLOTS.iter().position(|s| s.2 == a.as_str()) {
                Some(i) if p.slots.contains(&SLOTS[i].0) => {
                    if let Some(v) = it.next() {
                        if !multi(SLOTS[i].0) {
                            p.values[i].clear();
                        }
                        p.values[i].push(v.clone());
                    }
                }
                _ => p.extra.push(a.clone()),
            }
        }
        p
    }

    fn index(slot: Slot) -> usize {
        SLOTS.iter().position(|s| s.0 == slot).expect("every slot is listed")
    }

    pub fn get(&self, slot: Slot) -> &[String] {
        &self.values[Self::index(slot)]
    }

    fn set(&mut self, slot: Slot, values: Vec<String>) {
        self.values[Self::index(slot)] = values.into_iter().filter(|v| !v.is_empty()).collect();
    }

    /// The `issue list` flags.
    pub fn args(&self) -> Vec<String> {
        let mut a = Vec::new();
        for &slot in &self.slots {
            for v in self.get(slot) {
                if slot == Slot::Reverse {
                    a.push("--reverse".into());
                } else {
                    a.extend([flag_of(slot).to_string(), v.clone()]);
                }
            }
        }
        a.extend(self.extra.iter().cloned());
        a
    }

    pub fn current(&self) -> Slot {
        self.slots[self.row.min(self.slots.len() - 1)]
    }
}

const STATE_TYPES: [&str; 6] = ["triage", "backlog", "unstarted", "started", "completed", "canceled"];
const PRIORITIES: [(&str, &str); 5] =
    [("Urgent", "1"), ("High", "2"), ("Medium", "3"), ("Low", "4"), ("No priority", "0")];
const UPDATED: [(&str, &str); 4] =
    [("Any time", ""), ("Last 24 hours", "24h"), ("Last 7 days", "7d"), ("Last 30 days", "30d")];
const SORTS: [(&str, &str); 6] = [
    ("Updated", "updated"),
    ("Created", "created"),
    ("Priority", "priority"),
    ("Due date", "due-date"),
    ("Title", "title"),
    ("Status", "state"),
];

fn item(label: &str, value: &str, chosen: bool) -> PickItem {
    PickItem { label: label.into(), value: value.into(), color: None, hint: String::new(), chosen }
}

impl App {
    /// The rows this linearctl can filter by.
    fn filter_slots(&self) -> Vec<Slot> {
        let has = |flag: &str| {
            let mut words = vec![flag.to_string()];
            if flag != "--reverse" {
                words.push("x".into());
            }
            let argv = ctl::list_argv(&words, 1, None);
            self.describe.as_ref().is_none_or(|d| d.check_argv(&argv).is_empty())
        };
        SLOTS.iter().filter(|s| has(s.2)).map(|s| s.0).collect()
    }

    /// `f`: the panel, from what the list shows now.
    pub(super) fn open_filters(&mut self) -> Vec<Effect> {
        let args = match &self.filtered {
            Some(v) => v.args.clone(),
            None => self.views[self.view].args.clone(),
        };
        self.filters = Some(FilterPanel::from_args(&args, self.filter_slots()));
        Vec::new()
    }

    /// The team the panel's states, labels, projects and cycles are for: its
    /// own, else the selected issue's, else the first team.
    pub(super) fn filter_team(&self) -> String {
        let own = self.filters.as_ref().and_then(|f| f.get(Slot::Team).first().cloned());
        own.or_else(|| self.selected_row().and_then(|r| r.team.as_ref().map(|t| t.key.clone())))
            .or_else(|| self.lookups.get(Lookup::Teams, "").and_then(|t| t.first()).map(|t| t.key.clone()))
            .unwrap_or_default()
    }

    pub(super) fn filter_lookup(slot: Slot) -> Option<Lookup> {
        match slot {
            Slot::Team => Some(Lookup::Teams),
            Slot::Assignee => Some(Lookup::Users),
            Slot::State => Some(Lookup::States),
            Slot::LabelsAll | Slot::LabelsAny | Slot::LabelsNone => Some(Lookup::Labels),
            Slot::Project => Some(Lookup::Projects),
            Slot::Cycle => Some(Lookup::Cycles),
            Slot::Creator => Some(Lookup::Users),
            _ => None,
        }
    }

    /// The picker's items for a row, marked with what is set.
    pub(super) fn filter_items(&self, slot: Slot) -> Vec<PickItem> {
        let Some(panel) = self.filters.as_ref() else { return Vec::new() };
        let have = panel.get(slot);
        let on = |v: &str| have.iter().any(|h| h == v);
        let team = self.filter_team();
        let list = |what| self.lookups.get(what, &team).cloned().unwrap_or_default();
        match slot {
            Slot::Team => {
                let mut v = vec![item("Any team", "", have.is_empty())];
                v.extend(list(Lookup::Teams).iter().map(|t| item(&t.name, &t.key, on(&t.key))));
                v
            }
            Slot::Assignee => {
                let mut v = vec![
                    item("Anyone", "", have.is_empty()),
                    item("Me", "me", on("me")),
                    item("Unassigned", "none", on("none")),
                ];
                v.extend(list(Lookup::Users).iter().map(|u| item(&u.name, &u.email, on(&u.email))));
                v
            }
            Slot::StateType => STATE_TYPES.iter().map(|t| item(t, t, on(t))).collect(),
            Slot::State | Slot::LabelsAll | Slot::LabelsAny | Slot::LabelsNone => {
                let what = if slot == Slot::State { Lookup::States } else { Lookup::Labels };
                list(what)
                    .iter()
                    .map(|n| PickItem { color: n.color.clone(), ..item(&n.name, &n.name, on(&n.name)) })
                    .collect()
            }
            Slot::Project => {
                let mut v = vec![item("Any project", "", have.is_empty())];
                v.extend(list(Lookup::Projects).iter().map(|p| item(&p.name, &p.name, on(&p.name))));
                v
            }
            Slot::Cycle => {
                let mut v = vec![
                    item("Any cycle", "", have.is_empty()),
                    item("Current", "current", on("current")),
                    item("Next", "next", on("next")),
                ];
                v.extend(list(Lookup::Cycles).iter().filter_map(|c| {
                    let n = c.number?.to_string();
                    Some(item(&format!("Cycle {n}"), &n, on(&n)))
                }));
                v
            }
            Slot::Priority => PRIORITIES.iter().map(|(l, p)| item(l, p, on(p))).collect(),
            Slot::Updated => UPDATED
                .iter()
                .map(|(l, w)| item(l, w, if w.is_empty() { have.is_empty() } else { on(w) }))
                .collect(),
            Slot::Sort => {
                // Due date, title and status need linearctl 0.6.0.
                let n = if self.at_least(0, 6) { SORTS.len() } else { 3 };
                SORTS[..n].iter().map(|(l, s)| item(l, s, on(s))).collect()
            }
            Slot::Creator => {
                let mut v = vec![
                    item("Anyone", "", have.is_empty()),
                    item("Me", "me", on("me")),
                    item("Nobody (integrations)", "none", on("none")),
                ];
                v.extend(list(Lookup::Users).iter().map(|u| item(&u.name, &u.email, on(&u.email))));
                v
            }
            Slot::DueAfter | Slot::DueBefore => {
                let day = |n: i64| (self.now + chrono::Duration::days(n)).format("%Y-%m-%d").to_string();
                let mut v = vec![item("Any date", "", have.is_empty())];
                for (label, n) in [("Today", 0), ("In 7 days", 7), ("In 30 days", 30)] {
                    let d = day(n);
                    v.push(PickItem { hint: d.clone(), ..item(label, &d, on(&d)) });
                }
                // A date typed before: keep it on offer.
                let typed: Vec<PickItem> = have
                    .iter()
                    .filter(|h| !v.iter().any(|i| &i.value == *h))
                    .map(|h| item(h, h, true))
                    .collect();
                v.extend(typed);
                v
            }
            Slot::Reverse => vec![item("No", "", have.is_empty()), item("Yes", "yes", on("yes"))],
        }
    }

    pub(super) fn open_filter_picker(&mut self, slot: Slot) -> Vec<Effect> {
        let team = self.filter_team();
        let mut fx = Vec::new();
        let waiting = match Self::filter_lookup(slot) {
            Some(what) if self.lookups.get(what, &team).is_none() => {
                fx = self.need(what, &team);
                Some((what, team))
            }
            _ => None,
        };
        let items = if waiting.is_some() { Vec::new() } else { self.filter_items(slot) };
        let cursor = items.iter().position(|i| i.chosen).unwrap_or(0);
        let title = SLOTS.iter().find(|s| s.0 == slot).map_or("", |s| s.1).to_string();
        let target = Target::Filter(slot);
        let list =
            PickList { title, items, multi: multi(slot), cursor, query: String::new(), target, waiting };
        self.overlay = Some(Overlay::Pick(Box::new(list)));
        fx
    }

    /// A picker's choice for a row: the chosen values, or the one under the
    /// cursor for single rows.
    pub(super) fn choose_filter(
        &mut self,
        slot: Slot,
        chosen: Vec<String>,
        current: Option<PickItem>,
        query: &str,
    ) {
        // A date typed into the picker, when no item matches it.
        let typed = matches!(slot, Slot::DueAfter | Slot::DueBefore)
            && current.is_none()
            && chrono::NaiveDate::parse_from_str(query.trim(), "%Y-%m-%d").is_ok();
        let current = if typed { Some(item(query.trim(), query.trim(), false)) } else { current };
        let Some(panel) = self.filters.as_mut() else { return };
        if multi(slot) {
            panel.set(slot, chosen);
        } else if let Some(c) = current {
            let team_changed = slot == Slot::Team && panel.get(Slot::Team).first() != Some(&c.value);
            panel.set(slot, vec![c.value]);
            if team_changed {
                // States, labels, projects and cycles are per team.
                for s in [
                    Slot::State,
                    Slot::LabelsAll,
                    Slot::LabelsAny,
                    Slot::LabelsNone,
                    Slot::Project,
                    Slot::Cycle,
                ] {
                    panel.set(s, Vec::new());
                }
            }
        }
    }

    /// Keys while the panel is open (and no picker over it).
    pub(super) fn filters_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let panel = self.filters.as_mut().expect("filters_key needs the panel");
        if let Some(name) = panel.naming.as_mut() {
            match k.code {
                KeyCode::Esc => panel.naming = None,
                KeyCode::Backspace => {
                    name.pop();
                }
                KeyCode::Enter => return self.save_filters(),
                KeyCode::Char(c) if !ctrl => name.push(c),
                _ => {}
            }
            return Vec::new();
        }
        let n = panel.slots.len();
        match k.code {
            KeyCode::Esc => self.filters = None,
            KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => panel.row = (panel.row + 1) % n,
            KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k') => panel.row = (panel.row + n - 1) % n,
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => {
                let slot = panel.current();
                panel.set(slot, Vec::new());
            }
            KeyCode::Char('a') if ctrl => return self.apply_filters(),
            KeyCode::Char('s') if ctrl => panel.naming = Some(String::new()),
            KeyCode::Enter => {
                let slot = panel.current();
                return self.open_filter_picker(slot);
            }
            _ => {}
        }
        Vec::new()
    }

    /// Show the list for the panel's filters, as a temporary view.
    fn apply_filters(&mut self) -> Vec<Effect> {
        let Some(panel) = self.filters.take() else { return Vec::new() };
        self.filtered = Some(View { name: "Filter".into(), args: panel.args() });
        self.search = None;
        self.sort = None;
        self.rows.clear();
        self.selected = 0;
        self.next_cursor = None;
        self.load_view()
    }

    /// Save the panel's filters as a view in config.toml and show it.
    fn save_filters(&mut self) -> Vec<Effect> {
        let Some(panel) = self.filters.as_mut() else { return Vec::new() };
        let name = panel.naming.clone().unwrap_or_default().trim().to_string();
        if name.is_empty() {
            self.status = Some("A view needs a name.".into());
            return Vec::new();
        }
        let view = View { name, args: panel.args() };
        let Some(path) = self.config.clone() else {
            self.status = Some("No place for config.toml (HOME is not set).".into());
            return Vec::new();
        };
        if let Err(e) = crate::config::append_view(&path, &view) {
            self.status = Some(format!("Not saved: {e}"));
            return Vec::new();
        }
        self.notice = Some(format!("Saved view \"{}\" to {}", view.name, path.display()));
        self.views.push(view);
        self.filters = None;
        self.filtered = None;
        self.switch_view(self.views.len() - 1)
    }
}

impl App {
    /// `V`: a picker of Linear's saved issue views (linearctl 0.6.0).
    pub(super) fn open_saved_views(&mut self) -> Vec<Effect> {
        let argv = ctl::lookup_argv(Lookup::Views, "");
        if self.describe.as_ref().is_some_and(|d| !d.check_argv(&argv).is_empty()) {
            self.status = Some("Linear's saved views need linearctl 0.6.0 or later.".into());
            return Vec::new();
        }
        let mut fx = Vec::new();
        let waiting = if self.lookups.get(Lookup::Views, "").is_none() {
            fx = self.need(Lookup::Views, "");
            Some((Lookup::Views, String::new()))
        } else {
            None
        };
        let items = if waiting.is_some() { Vec::new() } else { self.saved_view_items() };
        if waiting.is_none() && items.is_empty() {
            self.notice = Some("No saved issue views in this workspace.".into());
            return fx;
        }
        let list = PickList {
            title: "Saved views".into(),
            items,
            multi: false,
            cursor: 0,
            query: String::new(),
            target: Target::SavedView,
            waiting,
        };
        self.overlay = Some(Overlay::Pick(Box::new(list)));
        fx
    }

    /// Issue views (not project views), by name; the id is the value.
    pub(super) fn saved_view_items(&self) -> Vec<PickItem> {
        let views = self.lookups.get(Lookup::Views, "").cloned().unwrap_or_default();
        views
            .iter()
            .filter(|v| v.model_name.is_empty() || v.model_name == "Issue")
            .map(|v| {
                let hint = v.owner.as_ref().map(|o| o.name.clone()).unwrap_or_default();
                let on =
                    self.filtered.as_ref().is_some_and(|f| f.args == ["--view".to_string(), v.id.clone()]);
                PickItem { label: v.name.clone(), value: v.id.clone(), color: None, hint, chosen: on }
            })
            .collect()
    }

    /// Show a saved view as a temporary view.
    pub(super) fn show_saved_view(&mut self, item: PickItem) -> Vec<Effect> {
        self.filtered = Some(View { name: item.label, args: vec!["--view".into(), item.value] });
        self.search = None;
        self.sort = None;
        self.rows.clear();
        self.selected = 0;
        self.next_cursor = None;
        self.load_view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Slot> {
        SLOTS.iter().map(|s| s.0).collect()
    }

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn args_round_trip() {
        let args = strings(&[
            "--assignee",
            "me",
            "--state-type",
            "unstarted",
            "--state-type",
            "started",
            "--sort",
            "priority",
            "--milestone",
            "Beta",
            "--label",
            "Bug",
        ]);
        let p = FilterPanel::from_args(&args, all());
        assert_eq!(p.get(Slot::Assignee), ["me"]);
        assert_eq!(p.get(Slot::StateType), ["unstarted", "started"]);
        assert_eq!(p.extra, ["--milestone", "Beta"], "kept, not shown");
        assert_eq!(
            p.args(),
            strings(&[
                "--assignee",
                "me",
                "--state-type",
                "unstarted",
                "--state-type",
                "started",
                "--label",
                "Bug",
                "--sort",
                "priority",
                "--milestone",
                "Beta",
            ])
        );
    }

    #[test]
    fn single_rows_keep_the_last_and_missing_values_are_dropped() {
        let p = FilterPanel::from_args(&strings(&["--team", "A", "--team", "B", "--assignee"]), all());
        assert_eq!(p.get(Slot::Team), ["B"]);
        assert!(p.get(Slot::Assignee).is_empty());
    }

    use crate::app::{Msg, default_views};
    use crate::fake::InProcess;

    fn now() -> chrono::DateTime<chrono::Utc> {
        "2026-10-26T12:00:00Z".parse().unwrap()
    }

    fn feed(app: &mut App, fx: Vec<Effect>) -> Vec<Effect> {
        let exec = InProcess { now: now(), state: None };
        let mut rest = Vec::new();
        for f in fx {
            match f {
                Effect::Run(r) => {
                    let more = app.update(r.run(&exec));
                    rest.extend(feed(app, more));
                }
                other => rest.push(other),
            }
        }
        rest
    }

    fn started() -> App {
        let mut app = App::new(default_views(), now());
        let fx = app.start();
        feed(&mut app, fx);
        app
    }

    fn key(app: &mut App, code: KeyCode) -> Vec<Effect> {
        let fx = app.update(Msg::Key(KeyEvent::from(code)));
        feed(app, fx)
    }

    fn ctrl(app: &mut App, c: char) -> Vec<Effect> {
        let fx = app.update(Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)));
        feed(app, fx)
    }

    fn to_row(app: &mut App, slot: Slot) {
        while app.filters.as_ref().unwrap().current() != slot {
            key(app, KeyCode::Down);
        }
    }

    fn ids(app: &App) -> Vec<String> {
        app.visible().iter().map(|r| r.identifier.clone()).collect()
    }

    fn picker(app: &App) -> &PickList {
        match &app.overlay {
            Some(Overlay::Pick(p)) => p,
            other => panic!("no picker: {other:?}"),
        }
    }

    #[test]
    fn the_panel_starts_from_the_view_and_applies() {
        let mut app = started();
        key(&mut app, KeyCode::Char('f'));
        let p = app.filters.as_ref().unwrap();
        assert_eq!(p.get(Slot::Assignee), ["me"]);
        assert_eq!(p.get(Slot::StateType), ["unstarted", "started"]);
        assert_eq!(p.get(Slot::Sort), ["priority"]);
        // Anyone's, with the Security label.
        to_row(&mut app, Slot::Assignee);
        key(&mut app, KeyCode::Char('d'));
        to_row(&mut app, Slot::LabelsAny);
        key(&mut app, KeyCode::Enter);
        assert!(picker(&app).multi);
        for c in "secu".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        key(&mut app, KeyCode::Char(' '));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.filters.as_ref().unwrap().get(Slot::LabelsAny), ["Security"]);
        let fx = ctrl(&mut app, 'a');
        assert!(app.filters.is_none());
        assert_eq!(app.filtered.as_ref().unwrap().name, "Filter");
        assert_eq!(ids(&app), ["ENG-151"]);
        assert!(fx.is_empty(), "no watch without the command");
        // f again starts from the filter; Esc goes back to the view.
        key(&mut app, KeyCode::Char('f'));
        assert_eq!(app.filters.as_ref().unwrap().get(Slot::LabelsAny), ["Security"]);
        key(&mut app, KeyCode::Esc);
        assert!(app.filters.is_none());
        key(&mut app, KeyCode::Esc);
        assert!(app.filtered.is_none());
        assert_eq!(ids(&app).len(), 4);
        assert!(!app.quit);
    }

    #[test]
    fn single_rows_choose_or_clear_and_a_team_change_resets_its_rows() {
        let mut app = started();
        key(&mut app, KeyCode::Char('f'));
        to_row(&mut app, Slot::Cycle);
        key(&mut app, KeyCode::Enter);
        let labels: Vec<String> = picker(&app).items.iter().map(|i| i.label.clone()).collect();
        assert_eq!(labels, ["Any cycle", "Current", "Next", "Cycle 23", "Cycle 24", "Cycle 25"]);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.filters.as_ref().unwrap().get(Slot::Cycle), ["current"]);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Enter);
        assert!(app.filters.as_ref().unwrap().get(Slot::Cycle).is_empty(), "Any cycle clears it");
        // A team set: per-team rows go.
        app.filters.as_mut().unwrap().set(Slot::State, vec!["Done".into()]);
        to_row(&mut app, Slot::Team);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        let p = app.filters.as_ref().unwrap();
        assert_eq!(p.get(Slot::Team), ["ENG"]);
        assert!(p.get(Slot::State).is_empty());
        // Choosing the same team keeps them.
        app.filters.as_mut().unwrap().set(Slot::State, vec!["Done".into()]);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.filters.as_ref().unwrap().get(Slot::State), ["Done"]);
        // Every other picker opens with its items.
        for slot in [
            Slot::Assignee,
            Slot::StateType,
            Slot::State,
            Slot::Project,
            Slot::Priority,
            Slot::Updated,
            Slot::Sort,
        ] {
            to_row(&mut app, slot);
            key(&mut app, KeyCode::Enter);
            assert!(!picker(&app).items.is_empty(), "{slot:?}");
            key(&mut app, KeyCode::Esc);
        }
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::BackTab);
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Char('z'));
    }

    #[test]
    fn saving_a_view_to_config() {
        let mut app = started();
        let dir = std::env::temp_dir().join(format!("linear-tui-filter-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        app.config = Some(dir.join("config.toml"));
        key(&mut app, KeyCode::Char('f'));
        to_row(&mut app, Slot::Assignee);
        key(&mut app, KeyCode::Char('d'));
        ctrl(&mut app, 's');
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.status.as_deref(), Some("A view needs a name."));
        for c in "Everyone's".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        key(&mut app, KeyCode::Backspace);
        key(&mut app, KeyCode::Char('s'));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.views.len(), 5);
        assert_eq!(app.view, 4);
        assert_eq!(app.views[4].name, "Everyone's");
        assert!(app.notice.as_deref().unwrap().starts_with("Saved view \"Everyone's\""));
        let saved = crate::config::load(&dir.join("config.toml")).unwrap();
        assert_eq!(
            saved[0].args,
            ["--state-type", "unstarted", "--state-type", "started", "--sort", "priority"]
        );
        assert_eq!(ids(&app).len(), 12);
        std::fs::remove_dir_all(&dir).unwrap();
        // Naming can be left; with nowhere to save, it says so.
        key(&mut app, KeyCode::Char('f'));
        ctrl(&mut app, 's');
        key(&mut app, KeyCode::Esc);
        assert!(app.filters.as_ref().unwrap().naming.is_none());
        app.config = None;
        ctrl(&mut app, 's');
        key(&mut app, KeyCode::Char('x'));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.status.as_deref(), Some("No place for config.toml (HOME is not set)."));
        app.config = Some(std::path::PathBuf::from("/nonexistent-root/x/config.toml"));
        key(&mut app, KeyCode::Enter);
        assert!(app.status.as_deref().unwrap().starts_with("Not saved: "));
    }

    #[test]
    fn priority_needs_a_linearctl_that_has_it() {
        let mut app = started();
        let old = include_str!("../../tests/fixtures/linearctl-describe-0.1.0.json");
        app.describe = Some(crate::contract::Describe::from_json(old).unwrap());
        key(&mut app, KeyCode::Char('f'));
        assert!(!app.filters.as_ref().unwrap().slots.contains(&Slot::Priority));
        app.describe = None;
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('f'));
        assert!(app.filters.as_ref().unwrap().slots.contains(&Slot::Priority));
    }

    #[test]
    fn a_filter_gets_its_own_watch() {
        let mut app = started();
        app.can_watch = true;
        key(&mut app, KeyCode::Char('f'));
        to_row(&mut app, Slot::Assignee);
        key(&mut app, KeyCode::Char('d'));
        let fx = ctrl(&mut app, 'a');
        let Some(Effect::Watch { argv, .. }) = fx.first() else { panic!("{fx:?}") };
        assert!(!argv.contains(&"me".to_string()), "{argv:?}");
        assert_eq!(app.live.as_ref().unwrap().args, app.filtered.as_ref().unwrap().args);
    }

    #[test]
    fn v_shows_a_saved_linear_view() {
        let mut app = started();
        key(&mut app, KeyCode::Char('V'));
        let p = picker(&app);
        assert_eq!(p.title, "Saved views");
        let names: Vec<&str> = p.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(names, ["Checkout bugs", "Mobile 3.0 open"]);
        assert_eq!(p.items[0].hint, "Maya Patel");
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.filtered.as_ref().unwrap().name, "Checkout bugs");
        assert_eq!(app.filtered.as_ref().unwrap().args[0], "--view");
        assert_eq!(ids(&app), ["ENG-158", "ENG-142", "ENG-143", "ENG-144"]);
        // The one shown is marked; f starts from its flags.
        key(&mut app, KeyCode::Char('V'));
        assert!(picker(&app).items[0].chosen);
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('f'));
        assert_eq!(app.filters.as_ref().unwrap().extra[0], "--view");
    }

    #[test]
    fn a_saved_view_is_not_watched() {
        let mut app = started();
        app.can_watch = true;
        let now = include_str!("../../tests/fixtures/linearctl-describe-0.7.0.json");
        app.describe = Some(crate::contract::Describe::from_json(now).unwrap());
        key(&mut app, KeyCode::Char('V'));
        let fx = key(&mut app, KeyCode::Enter);
        assert!(fx.iter().all(|f| !matches!(f, Effect::Watch { .. })), "watch has no --view: {fx:?}");
    }

    #[test]
    fn saved_views_need_linearctl_0_6_0_and_some_views() {
        let mut app = started();
        let old = include_str!("../../tests/fixtures/linearctl-describe-0.1.0.json");
        app.describe = Some(crate::contract::Describe::from_json(old).unwrap());
        key(&mut app, KeyCode::Char('V'));
        assert_eq!(app.status.as_deref(), Some("Linear's saved views need linearctl 0.6.0 or later."));
        app.describe = None;
        app.update(Msg::Looked {
            what: Lookup::Views,
            team: String::new(),
            result: Ok(crate::model::Page::default()),
        });
        key(&mut app, KeyCode::Char('V'));
        assert!(app.overlay.is_none());
        assert_eq!(app.notice.as_deref(), Some("No saved issue views in this workspace."));
    }

    #[test]
    fn creator_due_dates_and_reverse() {
        let mut app = started();
        key(&mut app, KeyCode::Char('4')); // Recent: everything updated this week
        key(&mut app, KeyCode::Char('f'));
        to_row(&mut app, Slot::Creator);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.filters.as_ref().unwrap().get(Slot::Creator), ["me"]);
        to_row(&mut app, Slot::DueBefore);
        key(&mut app, KeyCode::Enter);
        let labels: Vec<String> = picker(&app).items.iter().map(|i| i.label.clone()).collect();
        assert_eq!(labels, ["Any date", "Today", "In 7 days", "In 30 days"]);
        assert_eq!(picker(&app).items[2].hint, "2026-11-02");
        // A date typed in.
        for c in "2026-12-31".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.filters.as_ref().unwrap().get(Slot::DueBefore), ["2026-12-31"]);
        // It stays on offer; a typed non-date sets nothing.
        key(&mut app, KeyCode::Enter);
        assert!(picker(&app).items.iter().any(|i| i.label == "2026-12-31" && i.chosen));
        for c in "soon".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.filters.as_ref().unwrap().get(Slot::DueBefore), ["2026-12-31"]);
        to_row(&mut app, Slot::Reverse);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        to_row(&mut app, Slot::Sort);
        key(&mut app, KeyCode::Enter);
        assert_eq!(picker(&app).items.len(), 6, "due date, title and status too");
        for c in "due".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        key(&mut app, KeyCode::Enter);
        let args = app.filters.as_ref().unwrap().args();
        assert_eq!(
            args,
            strings(&[
                "--updated-since",
                "7d",
                "--creator",
                "me",
                "--due-before",
                "2026-12-31",
                "--sort",
                "due-date",
                "--reverse"
            ])
        );
        assert_eq!(FilterPanel::from_args(&args, all()).args(), args, "round trip with --reverse");
        ctrl(&mut app, 'a');
        assert!(
            ids(&app).is_empty(),
            "Maya made none with a due date; undated ones are not due before anything"
        );
    }

    #[test]
    fn rows_need_their_flags() {
        let mut app = started();
        let old = include_str!("../../tests/fixtures/linearctl-describe-0.1.0.json");
        app.describe = Some(crate::contract::Describe::from_json(old).unwrap());
        key(&mut app, KeyCode::Char('f'));
        let slots = &app.filters.as_ref().unwrap().slots;
        for s in [Slot::Priority, Slot::Creator, Slot::DueAfter, Slot::DueBefore, Slot::Reverse] {
            assert!(!slots.contains(&s), "{s:?}");
        }
        to_row(&mut app, Slot::Sort);
        key(&mut app, KeyCode::Enter);
        assert_eq!(picker(&app).items.len(), 3);
    }

    #[test]
    fn rows_linearctl_lacks_keep_their_flags_as_extra() {
        let slots: Vec<Slot> = all().into_iter().filter(|s| *s != Slot::Priority).collect();
        let p = FilterPanel::from_args(&strings(&["--priority", "1"]), slots);
        assert!(p.get(Slot::Priority).is_empty());
        assert_eq!(p.args(), ["--priority", "1"]);
    }
}
