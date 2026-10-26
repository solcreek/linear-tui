//! The TUI's state and how it changes: `update(msg)` takes a key press or a
//! linearctl result and returns the effects to run (linearctl calls, opening
//! a URL). No terminal and no processes here, so all of it is unit-tested.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ctl::{self, Failure};
use crate::model::{AuthStatus, IssueDetail, IssueRow, Page};

/// Rows fetched per page.
pub const PAGE: usize = 50;

/// Server searches allowed a minute. Linear allows 30 per API key; the rest
/// is left for agents using the same key.
pub const SEARCHES_PER_MINUTE: usize = 20;

/// A named set of `issue list` filter flags.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub name: String,
    pub args: Vec<String>,
}

impl View {
    pub fn new(name: &str, args: &[&str]) -> Self {
        Self {
            name: name.into(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
        }
    }
}

/// The views when none are configured.
pub fn default_views() -> Vec<View> {
    vec![
        View::new(
            "My issues",
            &[
                "--assignee",
                "me",
                "--state-type",
                "unstarted",
                "--state-type",
                "started",
                "--sort",
                "priority",
            ],
        ),
        View::new(
            "Active",
            &[
                "--state-type",
                "unstarted",
                "--state-type",
                "started",
                "--sort",
                "priority",
            ],
        ),
        View::new("Triage", &["--state-type", "triage"]),
        View::new("Recent", &["--updated-since", "7d"]),
    ]
}

/// Every command line the app can build for these views, for checking a
/// linearctl's `describe` before starting.
pub fn required_argvs(views: &[View]) -> Vec<Vec<String>> {
    let mut v: Vec<Vec<String>> = views
        .iter()
        .map(|v| ctl::list_argv(&v.args, PAGE, Some("cursor")))
        .collect();
    v.push(ctl::show_argv("ENG-1"));
    v.push(ctl::auth_status_argv());
    v
}

/// What `describe` lacks for these views; empty when linearctl will do.
pub fn missing(d: &crate::contract::Describe, views: &[View]) -> Vec<String> {
    required_argvs(views)
        .iter()
        .flat_map(|a| d.check_argv(a))
        .collect()
}

/// A linearctl call to make; its result comes back as a [`Msg`].
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// A page of the list; `generation` drops answers for a list since replaced.
    List {
        generation: u64,
        argv: Vec<String>,
        append: bool,
    },
    Show {
        id: String,
        argv: Vec<String>,
    },
    Auth {
        argv: Vec<String>,
    },
}

impl Request {
    /// Run it (blocking) and turn the answer into the message for `update`.
    pub fn run(self, runner: &ctl::Runner) -> Msg {
        match self {
            Request::List {
                generation,
                argv,
                append,
            } => Msg::Listed {
                generation,
                append,
                result: runner.run_as(&argv),
            },
            Request::Show { id, argv } => Msg::Shown {
                id,
                result: runner
                    .run_as::<crate::model::ShowResult>(&argv)
                    .map(|s| Box::new(s.issue)),
            },
            Request::Auth { argv } => Msg::Authed(runner.run_as(&argv)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Run(Request),
    /// Open a URL in the browser.
    Open(String),
    /// Fetch and decode an image (an upload URL); answered by `Msg::Image`.
    FetchImage(String),
}

/// An image in the open issue, by upload path.
pub enum ImageSlot {
    Loading,
    Ready(Box<Pic>),
    Failed(String),
}

/// A decoded image, and its terminal encoding once the UI has made one.
pub struct Pic {
    pub image: image::DynamicImage,
    pub proto: Option<ratatui_image::protocol::StatefulProtocol>,
}

impl std::fmt::Debug for ImageSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loading => f.write_str("Loading"),
            Self::Ready(p) => write!(f, "Ready({}x{})", p.image.width(), p.image.height()),
            Self::Failed(e) => write!(f, "Failed({e})"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Key(KeyEvent),
    Listed {
        generation: u64,
        append: bool,
        result: Result<Page<IssueRow>, Failure>,
    },
    Shown {
        id: String,
        result: Result<Box<IssueDetail>, Failure>,
    },
    Authed(Result<AuthStatus, Failure>),
    /// An image fetched for `url`.
    Image {
        url: String,
        result: Result<Box<image::DynamicImage>, String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Detail,
}

/// The issue open in the detail pane.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Detail {
    /// The identifier asked for (ENG-142).
    pub id: String,
    pub issue: Option<IssueDetail>,
    pub error: Option<String>,
    /// First line shown; the UI clamps it to the content.
    pub scroll: u16,
}

#[derive(Debug)]
pub struct App {
    pub views: Vec<View>,
    pub view: usize,
    pub rows: Vec<IssueRow>,
    /// Index into the rows shown (after the local filter).
    pub selected: usize,
    pub next_cursor: Option<String>,
    pub loading: bool,
    generation: u64,
    pub focus: Focus,
    pub detail: Option<Detail>,
    cache: HashMap<String, IssueDetail>,
    pub auth: Option<AuthStatus>,
    /// The last error or notice, for the status bar.
    pub status: Option<String>,
    /// The local filter, and whether it is being typed.
    pub filter: String,
    pub filtering: bool,
    pub help: bool,
    pub quit: bool,
    /// The time relative dates are shown against.
    pub now: DateTime<Utc>,
    /// Images of the issues seen, by upload path.
    pub images: HashMap<String, ImageSlot>,
    /// Whether images are fetched and drawn (`i` toggles).
    pub images_on: bool,
    /// The server search shown instead of the view, if any.
    pub search: Option<String>,
    /// When recent searches ran, for the rate limit.
    searches: Vec<DateTime<Utc>>,
    /// How images are encoded for this terminal; `None` draws none.
    pub picker: Option<ratatui_image::picker::Picker>,
}

impl App {
    pub fn new(views: Vec<View>, now: DateTime<Utc>) -> Self {
        Self {
            views,
            view: 0,
            rows: Vec::new(),
            selected: 0,
            next_cursor: None,
            loading: false,
            generation: 0,
            focus: Focus::List,
            detail: None,
            cache: HashMap::new(),
            auth: None,
            status: None,
            filter: String::new(),
            filtering: false,
            help: false,
            quit: false,
            now,
            images: HashMap::new(),
            images_on: true,
            picker: None,
            search: None,
            searches: Vec::new(),
        }
    }

    /// Fetch the open issue's uploaded images not yet asked for. Images on
    /// other hosts are never fetched: a tracking pixel in someone's text
    /// should not learn who reads it.
    fn image_effects(&mut self) -> Vec<Effect> {
        let Some(issue) = self.detail.as_ref().and_then(|d| d.issue.as_ref()) else {
            return Vec::new();
        };
        if !self.images_on {
            return Vec::new();
        }
        let texts = issue
            .description
            .iter()
            .chain(issue.comments.iter().map(|c| &c.body));
        let mut fx = Vec::new();
        for (url, _) in texts.flat_map(|t| crate::markdown::image_urls(t)) {
            let Some(path) = crate::images::upload_path(&url) else {
                continue;
            };
            if !self.images.contains_key(path) {
                self.images.insert(path.to_string(), ImageSlot::Loading);
                fx.push(Effect::FetchImage(url));
            }
        }
        fx
    }

    /// What to run at start: who we are, and the first view.
    pub fn start(&mut self) -> Vec<Effect> {
        let mut fx = vec![Effect::Run(Request::Auth {
            argv: ctl::auth_status_argv(),
        })];
        fx.extend(self.load_view());
        fx
    }

    /// Rows that pass the local filter.
    pub fn visible(&self) -> Vec<&IssueRow> {
        let needle = self.filter.to_lowercase();
        self.rows
            .iter()
            .filter(|r| needle.is_empty() || matches(r, &needle))
            .collect()
    }

    pub fn selected_row(&self) -> Option<&IssueRow> {
        self.visible().get(self.selected).copied()
    }

    /// The `issue list` filters of what the list shows: the search, or the view.
    fn list_args(&self) -> Vec<String> {
        match &self.search {
            Some(q) => vec!["--query".into(), q.clone()],
            None => self.views[self.view].args.clone(),
        }
    }

    fn load_view(&mut self) -> Vec<Effect> {
        self.generation += 1;
        self.loading = true;
        let argv = ctl::list_argv(&self.list_args(), PAGE, None);
        vec![Effect::Run(Request::List {
            generation: self.generation,
            argv,
            append: false,
        })]
    }

    fn load_more(&mut self) -> Vec<Effect> {
        match (&self.next_cursor, self.loading) {
            (Some(c), false) => {
                self.loading = true;
                let argv = ctl::list_argv(&self.list_args(), PAGE, Some(c));
                vec![Effect::Run(Request::List {
                    generation: self.generation,
                    argv,
                    append: true,
                })]
            }
            _ => Vec::new(),
        }
    }

    fn switch_view(&mut self, view: usize) -> Vec<Effect> {
        let same = view == self.view && !self.rows.is_empty() && self.search.is_none();
        if view >= self.views.len() || same {
            return Vec::new();
        }
        self.view = view;
        self.search = None;
        self.rows.clear();
        self.selected = 0;
        self.next_cursor = None;
        self.load_view()
    }

    /// Open the selected issue: from the cache at once, and fetched fresh.
    fn open(&mut self) -> Vec<Effect> {
        let Some(row) = self.selected_row() else {
            return Vec::new();
        };
        let id = row.identifier.clone();
        self.focus = Focus::Detail;
        if self
            .detail
            .as_ref()
            .is_some_and(|d| d.id == id && d.issue.is_some())
        {
            return Vec::new();
        }
        let issue = self.cache.get(&id).cloned();
        let cached = issue.is_some();
        self.detail = Some(Detail {
            id: id.clone(),
            issue,
            error: None,
            scroll: 0,
        });
        if cached {
            return self.image_effects();
        }
        vec![Effect::Run(Request::Show {
            argv: ctl::show_argv(&id),
            id,
        })]
    }

    fn move_to(&mut self, i: usize) -> Vec<Effect> {
        let n = self.visible().len();
        if n == 0 {
            return Vec::new();
        }
        self.selected = i.min(n - 1);
        // Near the end of what is loaded: fetch the next page.
        if self.selected + 5 >= n && self.filter.is_empty() {
            self.load_more()
        } else {
            Vec::new()
        }
    }

    fn step(&mut self, delta: isize) -> Vec<Effect> {
        self.move_to(self.selected.saturating_add_signed(delta))
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Key(k) => self.key(k),
            Msg::Listed { generation, .. } if generation != self.generation => Vec::new(),
            Msg::Listed { append, result, .. } => {
                self.loading = false;
                match result {
                    Ok(page) => {
                        if !append {
                            self.rows.clear();
                        }
                        self.rows.extend(page.items);
                        self.next_cursor = page.next_cursor.filter(|_| page.truncated);
                        self.selected = self.selected.min(self.visible().len().saturating_sub(1));
                        self.status = None;
                    }
                    Err(f) => self.status = Some(f.message()),
                }
                Vec::new()
            }
            Msg::Shown { id, result } => {
                let current = self.detail.as_mut().filter(|d| d.id == id);
                match result {
                    Ok(issue) => {
                        let issue = *issue;
                        self.cache.insert(id, issue.clone());
                        if let Some(d) = current {
                            d.issue = Some(issue);
                            d.error = None;
                            return self.image_effects();
                        }
                    }
                    Err(f) => {
                        if let Some(d) = current {
                            d.error = Some(f.message());
                        }
                    }
                }
                Vec::new()
            }
            Msg::Image { url, result } => {
                if let Some(path) = crate::images::upload_path(&url) {
                    let slot = match result {
                        Ok(image) => ImageSlot::Ready(Box::new(Pic {
                            image: *image,
                            proto: None,
                        })),
                        Err(e) => ImageSlot::Failed(e),
                    };
                    self.images.insert(path.to_string(), slot);
                }
                Vec::new()
            }
            Msg::Authed(Ok(a)) => {
                self.auth = Some(a);
                Vec::new()
            }
            Msg::Authed(Err(f)) => {
                self.status = Some(f.message());
                Vec::new()
            }
        }
    }

    fn key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            self.quit = true;
            return Vec::new();
        }
        if self.help {
            self.help = false;
            return Vec::new();
        }
        if self.filtering {
            return self.filter_key(k);
        }
        if k.code == KeyCode::Char('?') {
            self.help = true;
            return Vec::new();
        }
        match self.focus {
            Focus::List => self.list_key(k, ctrl),
            Focus::Detail => self.detail_key(k, ctrl),
        }
    }

    fn filter_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        match k.code {
            KeyCode::Esc => {
                self.filter.clear();
                self.filtering = false;
            }
            KeyCode::Enter => {
                self.filtering = false;
                let q = self.filter.trim().to_string();
                if !q.is_empty() {
                    return self.start_search(q);
                }
            }
            // Arrows leave the box and keep the filter.
            KeyCode::Down | KeyCode::Up => {
                self.filtering = false;
                self.selected = 0;
                return Vec::new();
            }
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(c) => self.filter.push(c),
            _ => {}
        }
        self.selected = 0;
        Vec::new()
    }

    /// Search every issue on the server, within the rate limit.
    fn start_search(&mut self, q: String) -> Vec<Effect> {
        let minute_ago = self.now - chrono::Duration::seconds(60);
        self.searches.retain(|t| *t > minute_ago);
        if self.searches.len() >= SEARCHES_PER_MINUTE {
            let wait = (self.searches[0] - minute_ago).num_seconds().max(1);
            self.status = Some(format!(
                "{SEARCHES_PER_MINUTE} searches a minute at most: try again in {wait} s (the filter still works)"
            ));
            return Vec::new();
        }
        self.searches.push(self.now);
        self.search = Some(q);
        self.filter.clear();
        self.rows.clear();
        self.selected = 0;
        self.next_cursor = None;
        self.status = None;
        self.load_view()
    }

    fn list_key(&mut self, k: KeyEvent, ctrl: bool) -> Vec<Effect> {
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.selected = 0;
            }
            KeyCode::Esc if self.search.is_some() => {
                self.search = None;
                self.rows.clear();
                self.selected = 0;
                self.next_cursor = None;
                return self.load_view();
            }
            KeyCode::Esc => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => return self.step(1),
            KeyCode::Char('k') | KeyCode::Up => return self.step(-1),
            KeyCode::Char('d') if ctrl => return self.step(10),
            KeyCode::Char('u') if ctrl => return self.step(-10),
            KeyCode::PageDown => return self.step(10),
            KeyCode::PageUp => return self.step(-10),
            KeyCode::Char('g') | KeyCode::Home => return self.move_to(0),
            KeyCode::Char('G') | KeyCode::End => return self.move_to(usize::MAX),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => return self.open(),
            KeyCode::Tab => return self.switch_view((self.view + 1) % self.views.len()),
            KeyCode::BackTab => {
                return self.switch_view((self.view + self.views.len() - 1) % self.views.len());
            }
            KeyCode::Char(c @ '1'..='9') => return self.switch_view(c as usize - '1' as usize),
            KeyCode::Char('r') => {
                self.selected = 0;
                return self.load_view();
            }
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Char('n') => return self.load_more(),
            KeyCode::Char('O') => {
                if let Some(r) = self.selected_row() {
                    return vec![Effect::Open(r.url.clone())];
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn detail_key(&mut self, k: KeyEvent, ctrl: bool) -> Vec<Effect> {
        let Some(d) = self.detail.as_mut() else {
            self.focus = Focus::List;
            return Vec::new();
        };
        let scroll = |d: &mut Detail, by: i32| {
            d.scroll = (i32::from(d.scroll) + by).clamp(0, i32::from(u16::MAX)) as u16
        };
        match k.code {
            KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Left => self.focus = Focus::List,
            KeyCode::Char('j') | KeyCode::Down => scroll(d, 1),
            KeyCode::Char('k') | KeyCode::Up => scroll(d, -1),
            KeyCode::Char('d') if ctrl => scroll(d, 10),
            KeyCode::Char('u') if ctrl => scroll(d, -10),
            KeyCode::PageDown | KeyCode::Char(' ') => scroll(d, 20),
            KeyCode::PageUp => scroll(d, -20),
            KeyCode::Char('g') | KeyCode::Home => d.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => d.scroll = u16::MAX,
            KeyCode::Char(']') => {
                let mut fx = self.step(1);
                fx.extend(self.open());
                return fx;
            }
            KeyCode::Char('[') => {
                let mut fx = self.step(-1);
                fx.extend(self.open());
                return fx;
            }
            KeyCode::Char('r') => {
                let id = d.id.clone();
                self.cache.remove(&id);
                return vec![Effect::Run(Request::Show {
                    argv: ctl::show_argv(&id),
                    id,
                })];
            }
            KeyCode::Char('i') => {
                self.images_on = !self.images_on;
                return self.image_effects();
            }
            KeyCode::Char('O') => {
                if let Some(i) = &d.issue {
                    return vec![Effect::Open(i.row.url.clone())];
                }
            }
            _ => {}
        }
        Vec::new()
    }
}

fn matches(r: &IssueRow, needle: &str) -> bool {
    let assignee = r.assignee.as_ref().map(|a| a.name.as_str()).unwrap_or("");
    let labels = r.labels.iter().map(|l| l.name.as_str());
    [r.identifier.as_str(), r.title.as_str(), assignee]
        .into_iter()
        .chain(labels)
        .any(|s| s.to_lowercase().contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake;
    use crate::model::ShowResult;

    fn now() -> DateTime<Utc> {
        "2026-10-26T12:00:00Z".parse().unwrap()
    }

    fn key(c: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(c, KeyModifiers::NONE))
    }

    fn ch(c: char) -> Msg {
        key(KeyCode::Char(c))
    }

    fn ctrl(c: char) -> Msg {
        Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    /// Run a request against the fake, as main would.
    fn answer(fx: &Effect) -> Msg {
        let Effect::Run(req) = fx else {
            panic!("not a request: {fx:?}")
        };
        let run = |argv: &[String]| {
            let out = fake::run(argv, now());
            assert_eq!(out.code, 0, "{argv:?}: {}", out.stdout);
            out.stdout
        };
        match req {
            Request::List {
                generation,
                argv,
                append,
            } => Msg::Listed {
                generation: *generation,
                append: *append,
                result: Ok(serde_json::from_str(&run(argv)).unwrap()),
            },
            Request::Show { id, argv } => Msg::Shown {
                id: id.clone(),
                result: Ok(Box::new(
                    serde_json::from_str::<ShowResult>(&run(argv)).unwrap().issue,
                )),
            },
            Request::Auth { argv } => Msg::Authed(Ok(serde_json::from_str(&run(argv)).unwrap())),
        }
    }

    /// Apply a message and answer every request it makes, recursively.
    fn drive(app: &mut App, msg: Msg) -> Vec<Effect> {
        let mut seen = Vec::new();
        let mut queue = app.update(msg);
        while let Some(fx) = queue.pop() {
            seen.push(fx.clone());
            if matches!(fx, Effect::Run(_)) {
                queue.extend(app.update(answer(&fx)));
            }
        }
        seen
    }

    fn started() -> App {
        let mut app = App::new(default_views(), now());
        for fx in app.start() {
            let m = answer(&fx);
            app.update(m);
        }
        app
    }

    fn ids(app: &App) -> Vec<&str> {
        app.visible().iter().map(|r| r.identifier.as_str()).collect()
    }

    #[test]
    fn start_loads_auth_and_my_issues() {
        let app = started();
        assert_eq!(app.auth.as_ref().unwrap().organization.name, "Northwind");
        assert_eq!(ids(&app), ["ENG-142", "ENG-143", "ENG-140", "ENG-120"]);
        assert!(!app.loading);
        assert_eq!(app.next_cursor, None);
    }

    #[test]
    fn moving_and_bounds() {
        let mut app = started();
        drive(&mut app, ch('j'));
        drive(&mut app, key(KeyCode::Down));
        assert_eq!(app.selected, 2);
        drive(&mut app, ch('G'));
        assert_eq!(app.selected, 3);
        drive(&mut app, ch('j'));
        assert_eq!(app.selected, 3);
        drive(&mut app, ch('g'));
        drive(&mut app, ch('k'));
        assert_eq!(app.selected, 0);
        drive(&mut app, ctrl('d'));
        assert_eq!(app.selected, 3);
        drive(&mut app, ctrl('u'));
        drive(&mut app, key(KeyCode::PageDown));
        drive(&mut app, key(KeyCode::PageUp));
        drive(&mut app, key(KeyCode::End));
        drive(&mut app, key(KeyCode::Home));
        drive(&mut app, key(KeyCode::Up));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn views_switch_and_reload() {
        let mut app = started();
        drive(&mut app, key(KeyCode::Tab));
        assert_eq!(app.views[app.view].name, "Active");
        assert_eq!(ids(&app).len(), 12);
        assert_eq!(ids(&app)[0], "ENG-158", "urgent first");
        drive(&mut app, ch('3'));
        assert_eq!(ids(&app), ["ENG-157", "ENG-156"]);
        drive(&mut app, key(KeyCode::BackTab));
        assert_eq!(app.view, 1);
        assert!(drive(&mut app, ch('9')).is_empty(), "no such view");
        assert!(drive(&mut app, ch('2')).is_empty(), "already there");
        let fx = drive(&mut app, ch('r'));
        assert!(matches!(&fx[0], Effect::Run(Request::List { append: false, .. })));
    }

    #[test]
    fn stale_list_answers_are_dropped() {
        let mut app = started();
        let first = app.update(ch('4'));
        let second = app.update(ch('1'));
        app.update(answer(&second[0]));
        app.update(answer(&first[0]));
        assert_eq!(ids(&app).len(), 4, "the Recent answer came late and was ignored");
    }

    #[test]
    fn paging_loads_more_near_the_end() {
        let mut app = App::new(vec![View::new("All", &[])], now());
        // A small page so there is a next one.
        let fx = app.start();
        for f in fx {
            let msg = match answer(&f) {
                Msg::Listed {
                    generation,
                    append,
                    result: Ok(mut p),
                } => {
                    p.items.truncate(10);
                    p.truncated = true;
                    p.next_cursor = Some(p.items[9].id.clone());
                    Msg::Listed {
                        generation,
                        append,
                        result: Ok(p),
                    }
                }
                m => m,
            };
            app.update(msg);
        }
        assert_eq!(app.rows.len(), 10);
        let fx = drive(&mut app, ch('G'));
        assert!(
            matches!(&fx[0], Effect::Run(Request::List { append: true, argv, .. }) if argv.contains(&"--cursor".to_string()))
        );
        assert_eq!(app.rows.len(), 17);
        assert_eq!(app.next_cursor, None);
        assert!(drive(&mut app, ch('n')).is_empty(), "nothing more");
    }

    #[test]
    fn local_filter() {
        let mut app = started();
        drive(&mut app, ch('4'));
        drive(&mut app, ch('/'));
        assert!(app.filtering);
        for c in "coupX".chars() {
            drive(&mut app, ch(c));
        }
        assert!(ids(&app).is_empty());
        drive(&mut app, key(KeyCode::Backspace));
        assert_eq!(ids(&app), ["ENG-142", "ENG-143", "ENG-144"]);
        drive(&mut app, key(KeyCode::Left));
        drive(&mut app, key(KeyCode::Down));
        assert!(!app.filtering);
        assert_eq!(app.filter, "coup");
        drive(&mut app, key(KeyCode::Esc));
        assert!(app.filter.is_empty());
        assert!(!app.quit);
        drive(&mut app, ch('/'));
        drive(&mut app, ch('m'));
        drive(&mut app, ch('a'));
        drive(&mut app, ch('y'));
        assert!(ids(&app).contains(&"ENG-142"), "matches the assignee");
        drive(&mut app, key(KeyCode::Esc));
        assert!(app.filter.is_empty() && !app.filtering);
        drive(&mut app, ch('/'));
        drive(&mut app, ch('s'));
        drive(&mut app, ch('e'));
        drive(&mut app, ch('c'));
        drive(&mut app, key(KeyCode::Down));
        assert_eq!(ids(&app), ["ENG-151"], "matches a label");
    }

    #[test]
    fn opening_an_issue_fetches_it_once() {
        let mut app = started();
        let fx = drive(&mut app, key(KeyCode::Enter));
        assert_eq!(fx.iter().filter(|f| matches!(f, Effect::Run(_))).count(), 1);
        assert!(fx.iter().any(|f| matches!(f, Effect::FetchImage(_))));
        assert_eq!(app.focus, Focus::Detail);
        let d = app.detail.as_ref().unwrap();
        assert_eq!(d.id, "ENG-142");
        assert_eq!(d.issue.as_ref().unwrap().comments.len(), 4);
        drive(&mut app, key(KeyCode::Esc));
        assert_eq!(app.focus, Focus::List);
        assert!(drive(&mut app, ch('l')).is_empty(), "already open");
        drive(&mut app, ch('h'));
        drive(&mut app, ch('j'));
        drive(&mut app, key(KeyCode::Right));
        drive(&mut app, ch('q'));
        drive(&mut app, ch('k'));
        assert!(drive(&mut app, key(KeyCode::Enter)).is_empty(), "cached");
        assert_eq!(app.detail.as_ref().unwrap().id, "ENG-142");
    }

    #[test]
    fn detail_keys() {
        let mut app = started();
        drive(&mut app, key(KeyCode::Enter));
        let scroll = |app: &App| app.detail.as_ref().unwrap().scroll;
        drive(&mut app, ch('j'));
        drive(&mut app, key(KeyCode::Down));
        assert_eq!(scroll(&app), 2);
        drive(&mut app, ch('k'));
        drive(&mut app, key(KeyCode::Up));
        drive(&mut app, key(KeyCode::Up));
        assert_eq!(scroll(&app), 0);
        drive(&mut app, ctrl('d'));
        assert_eq!(scroll(&app), 10);
        drive(&mut app, ctrl('u'));
        drive(&mut app, ch(' '));
        assert_eq!(scroll(&app), 20);
        drive(&mut app, key(KeyCode::PageUp));
        drive(&mut app, key(KeyCode::PageDown));
        drive(&mut app, ch('G'));
        assert_eq!(scroll(&app), u16::MAX);
        drive(&mut app, ch('j'));
        assert_eq!(scroll(&app), u16::MAX);
        drive(&mut app, key(KeyCode::Home));
        drive(&mut app, key(KeyCode::End));
        drive(&mut app, ch('g'));
        assert_eq!(scroll(&app), 0);
        drive(&mut app, ch(']'));
        assert_eq!(app.detail.as_ref().unwrap().id, "ENG-143");
        drive(&mut app, ch('['));
        assert_eq!(app.detail.as_ref().unwrap().id, "ENG-142");
        let fx = drive(&mut app, ch('r'));
        assert!(matches!(&fx[0], Effect::Run(Request::Show { id, .. }) if id == "ENG-142"));
        let fx = drive(&mut app, ch('O'));
        assert!(matches!(&fx[0], Effect::Open(u) if u.contains("/issue/ENG-142/")));
        assert!(drive(&mut app, ch('x')).is_empty());
    }

    #[test]
    fn detail_focus_without_a_detail_returns_to_the_list() {
        let mut app = started();
        app.focus = Focus::Detail;
        drive(&mut app, ch('j'));
        assert_eq!(app.focus, Focus::List);
    }

    #[test]
    fn open_in_browser_from_the_list() {
        let mut app = started();
        assert!(matches!(&drive(&mut app, ch('O'))[0], Effect::Open(u) if u.contains("ENG-142")));
        drive(&mut app, ch('/'));
        drive(&mut app, ch('#'));
        drive(&mut app, key(KeyCode::Up));
        assert!(drive(&mut app, ch('O')).is_empty(), "nothing selected");
        assert!(drive(&mut app, key(KeyCode::Enter)).is_empty());
        assert!(drive(&mut app, ch('j')).is_empty());
        assert!(drive(&mut app, ch('z')).is_empty());
    }

    #[test]
    fn failures_go_to_the_status_bar() {
        let mut app = started();
        let fx = app.update(ch('r'));
        let Effect::Run(Request::List { generation, .. }) = fx[0] else {
            panic!()
        };
        app.update(Msg::Listed {
            generation,
            append: false,
            result: Err(Failure::Auth("no API key".into())),
        });
        assert_eq!(app.status.as_deref(), Some("no API key"));
        assert!(!app.loading);
        app.update(Msg::Authed(Err(Failure::Failed("offline".into()))));
        assert_eq!(app.status.as_deref(), Some("offline"));
        app.update(key(KeyCode::Enter));
        app.update(Msg::Shown {
            id: "ENG-142".into(),
            result: Err(Failure::Failed("gone".into())),
        });
        assert_eq!(app.detail.as_ref().unwrap().error.as_deref(), Some("gone"));
        // An answer for an issue no longer open is cached, not shown.
        app.update(Msg::Shown {
            id: "ENG-999".into(),
            result: Ok(Box::default()),
        });
        app.update(Msg::Shown {
            id: "ENG-998".into(),
            result: Err(Failure::Failed("x".into())),
        });
        assert_eq!(app.detail.as_ref().unwrap().id, "ENG-142");
    }

    #[test]
    fn help_and_quit() {
        let mut app = started();
        drive(&mut app, ch('?'));
        assert!(app.help);
        drive(&mut app, ch('q'));
        assert!(!app.help && !app.quit, "a key closes help");
        drive(&mut app, ch('q'));
        assert!(app.quit);
        let mut app = started();
        drive(&mut app, key(KeyCode::Esc));
        assert!(app.quit);
        let mut app = started();
        drive(&mut app, ch('/'));
        drive(&mut app, ctrl('c'));
        assert!(app.quit, "Ctrl-c quits from anywhere");
    }

    #[test]
    fn linearctl_0_1_0_has_everything_the_views_need() {
        let d = crate::contract::Describe::from_json(include_str!(
            "../tests/fixtures/linearctl-describe-0.1.0.json"
        ))
        .unwrap();
        assert!(missing(&d, &default_views()).is_empty());
        let old = crate::contract::Describe::from_json(
            r#"{"flags":[{"name":"json"}],"commands":[{"name":"issue","aliases":[],"flags":[],"commands":[
                {"name":"list","aliases":[],"flags":[{"name":"limit"}],"commands":[]}]}]}"#,
        )
        .unwrap();
        let m = missing(&old, &default_views());
        assert!(m.iter().any(|p| p.contains("--fields")), "{m:?}");
        assert!(m.iter().any(|p| p.contains("unknown command: auth")), "{m:?}");
    }

    #[test]
    fn images_are_fetched_once_per_upload() {
        let mut app = started();
        let fx = app.update(key(KeyCode::Enter));
        let shown = answer(&fx[0]);
        let fx = app.update(shown);
        assert_eq!(
            fx,
            [Effect::FetchImage(
                "https://uploads.linear.app/demo/checkout-error.png?signature=demo-300".into()
            )]
        );
        assert!(matches!(
            app.images.get("/demo/checkout-error.png"),
            Some(ImageSlot::Loading)
        ));
        // Back and in again: cached, and the image is already asked for.
        drive(&mut app, key(KeyCode::Esc));
        drive(&mut app, ch('j'));
        drive(&mut app, key(KeyCode::Enter));
        drive(&mut app, key(KeyCode::Esc));
        drive(&mut app, ch('k'));
        assert!(drive(&mut app, key(KeyCode::Enter)).is_empty());
        app.update(Msg::Image {
            url: "https://uploads.linear.app/demo/checkout-error.png?x".into(),
            result: Err("boom".into()),
        });
        assert_eq!(
            format!("{:?}", app.images["/demo/checkout-error.png"]),
            "Failed(boom)"
        );
        app.update(Msg::Image {
            url: "https://example.com/not-an-upload".into(),
            result: Err("x".into()),
        });
        assert_eq!(app.images.len(), 1);
        let img = Box::new(image::DynamicImage::new_rgb8(4, 2));
        app.update(Msg::Image {
            url: "https://uploads.linear.app/demo/checkout-error.png".into(),
            result: Ok(img),
        });
        assert_eq!(
            format!("{:?}", app.images["/demo/checkout-error.png"]),
            "Ready(4x2)"
        );
        assert_eq!(format!("{:?}", ImageSlot::Loading), "Loading");
    }

    #[test]
    fn images_can_be_turned_off_and_on() {
        let mut app = started();
        app.update(key(KeyCode::Enter));
        app.update(ch('i'));
        assert!(!app.images_on);
        let fx = app.update(key(KeyCode::Enter));
        assert!(fx.is_empty(), "nothing to do: already open");
        // The issue arrives while images are off: nothing fetched.
        let mut app2 = started();
        let fx = app2.update(key(KeyCode::Enter));
        app2.images_on = false;
        assert!(app2.update(answer(&fx[0])).is_empty());
        let fx = app2.update(ch('i'));
        assert_eq!(fx.len(), 1, "turning images on fetches them");
    }

    fn search_for(app: &mut App, q: &str) -> Vec<Effect> {
        drive(app, ch('/'));
        for c in q.chars() {
            drive(app, ch(c));
        }
        drive(app, key(KeyCode::Enter))
    }

    #[test]
    fn enter_in_the_filter_searches_the_server() {
        let mut app = started();
        let fx = search_for(&mut app, "coupon");
        assert!(matches!(&fx[0], Effect::Run(Request::List { argv, .. })
            if argv.windows(2).any(|w| w == ["--query", "coupon"])));
        assert_eq!(app.search.as_deref(), Some("coupon"));
        assert!(app.filter.is_empty(), "the local filter gives way to the search");
        assert_eq!(ids(&app), ["ENG-142", "ENG-143", "ENG-144", "ENG-131"]);
        // Esc goes back to the view.
        drive(&mut app, key(KeyCode::Esc));
        assert_eq!(app.search, None);
        assert_eq!(ids(&app).len(), 4);
        assert!(!app.quit);
        // A view key also ends the search, even for the view it came from.
        search_for(&mut app, "android");
        let fx = drive(&mut app, ch('1'));
        assert!(!fx.is_empty());
        assert_eq!(app.search, None);
        // An empty filter does not search.
        drive(&mut app, ch('/'));
        drive(&mut app, ch(' '));
        assert!(drive(&mut app, key(KeyCode::Enter)).is_empty());
    }

    #[test]
    fn searches_are_rate_limited() {
        let mut app = started();
        for i in 0..SEARCHES_PER_MINUTE {
            app.now += chrono::Duration::seconds(1);
            assert!(!search_for(&mut app, &format!("q{i}")).is_empty(), "search {i}");
        }
        app.now += chrono::Duration::seconds(1);
        assert!(search_for(&mut app, "more").is_empty());
        assert!(
            app.status.as_deref().unwrap().contains("try again in 40 s"),
            "{:?}",
            app.status
        );
        assert_eq!(app.filter, "more", "the text stays as a local filter");
        app.now += chrono::Duration::seconds(40);
        drive(&mut app, key(KeyCode::Esc));
        assert!(
            !search_for(&mut app, "more").is_empty(),
            "the oldest search has aged out"
        );
    }

    #[test]
    fn an_empty_list_ignores_movement() {
        let mut app = App::new(default_views(), now());
        assert!(app.update(ch('j')).is_empty());
        assert!(app.update(key(KeyCode::Enter)).is_empty());
    }
}
