//! Live updates: a `linearctl watch` for the view on screen, its events
//! merged into the list, and what to do when it stops. Without `watch`
//! (linearctl 0.1.0) the view is reloaded every two minutes instead.

use chrono::{DateTime, Duration, Utc};

use super::{App, Effect, Request};
use crate::ctl::{self, Failure};
use crate::model::WatchEvent;

/// Without watch, how often the view is reloaded.
pub const REFRESH: Duration = Duration::seconds(120);
/// After a watch ends, when to start another.
pub const RETRY: Duration = Duration::seconds(30);
/// No line for this long (heartbeats are every 30 s) means a stuck watch.
pub const QUIET: Duration = Duration::seconds(75);

/// The watch running for a view.
#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    pub generation: u64,
    /// The filters watched: the view's, or the panel's.
    pub args: Vec<String>,
    /// subscription or poll, once linearctl says.
    pub mode: String,
    /// Up to date (after `ready`).
    pub ready: bool,
    /// When the last line came.
    pub heard: DateTime<Utc>,
}

/// Whether this linearctl has `watch` (0.2.0 and later).
pub fn can_watch(d: &crate::contract::Describe) -> bool {
    d.check_argv(&ctl::watch_argv(&[])).is_empty()
}

impl App {
    /// Start a watch for the view if none runs for it.
    pub(super) fn ensure_watch(&mut self) -> Vec<Effect> {
        if !self.can_watch
            || self.search.is_some()
            || self.live.as_ref().is_some_and(|l| l.args == self.watched_args())
        {
            return Vec::new();
        }
        self.start_watch()
    }

    fn start_watch(&mut self) -> Vec<Effect> {
        self.watch_generation += 1;
        self.watch_retry_at = None;
        let generation = self.watch_generation;
        self.live = Some(Live {
            generation,
            args: self.watched_args(),
            mode: String::new(),
            ready: false,
            heard: self.now,
        });
        let mut argv = ctl::watch_argv(&self.watched_args());
        // Replay what changed since the list was loaded, so nothing falls
        // between the load and the watch starting.
        if let Some(t) = self.last_loaded {
            let at = argv.len() - 1; // before --json
            argv.splice(
                at..at,
                ["--since".to_string(), t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)],
            );
        }
        vec![Effect::Watch { generation, argv }]
    }

    pub(super) fn stop_watch(&mut self) -> Vec<Effect> {
        self.watch_retry_at = None;
        if self.live.take().is_some() { vec![Effect::StopWatch] } else { Vec::new() }
    }

    pub(super) fn on_watched(&mut self, generation: u64, event: WatchEvent) -> Vec<Effect> {
        let now = self.now;
        let Some(live) = self.live.as_mut().filter(|l| l.generation == generation) else { return Vec::new() };
        live.heard = now;
        match event {
            WatchEvent::Ready { mode, .. } => {
                live.ready = true;
                live.mode = mode;
                Vec::new()
            }
            WatchEvent::Heartbeat { mode, .. } => {
                live.mode = mode;
                Vec::new()
            }
            WatchEvent::Gap { reason, mode } => {
                live.mode = mode;
                self.notice = Some(if reason == "fallback" {
                    "Live updates: Linear's stream dropped; polling instead.".into()
                } else {
                    "Live updates: reconnected and caught up.".into()
                });
                Vec::new()
            }
            WatchEvent::Upsert { issue } => {
                let row = *issue;
                let id = row.identifier.clone();
                let selected = self.selected_row().map(|r| r.id.clone());
                match self.rows.iter_mut().find(|r| r.id == row.id) {
                    Some(r) => *r = row,
                    None => self.rows.insert(0, row),
                }
                self.reselect(selected);
                self.cache.remove(&id);
                if self.detail.as_ref().is_some_and(|d| d.id == id) {
                    return vec![Effect::Run(Request::Show { argv: ctl::show_argv(&id), id })];
                }
                Vec::new()
            }
            WatchEvent::Remove { id, .. } => {
                let selected = self.selected_row().map(|r| r.id.clone()).filter(|s| *s != id);
                let at = self.selected;
                self.rows.retain(|r| r.id != id);
                match selected {
                    Some(_) => self.reselect(selected),
                    // The selected row went: stay at the same place.
                    None => self.selected = at.min(self.visible().len().saturating_sub(1)),
                }
                Vec::new()
            }
            WatchEvent::Other => Vec::new(),
        }
    }

    /// Keep the selection on the same issue after rows moved.
    pub(super) fn reselect(&mut self, id: Option<String>) {
        if let Some(i) = id.and_then(|id| self.visible().iter().position(|r| r.id == id)) {
            self.selected = i;
        }
        self.selected = self.selected.min(self.visible().len().saturating_sub(1));
    }

    pub(super) fn on_watch_ended(&mut self, generation: u64, failure: Option<Failure>) -> Vec<Effect> {
        if !self.live.as_ref().is_some_and(|l| l.generation == generation) {
            return Vec::new();
        }
        self.live = None;
        self.watch_retry_at = Some(self.now + RETRY);
        let why = failure.map_or_else(|| "it ended".to_string(), |f| f.message());
        self.status = Some(format!("Live updates stopped ({why}); trying again in 30 s."));
        Vec::new()
    }

    /// Once a second: restart a watch that ended or went quiet; without
    /// watch, reload the view now and then.
    pub(super) fn on_tick(&mut self) -> Vec<Effect> {
        if self.search.is_some() || self.loading {
            return Vec::new();
        }
        if self.can_watch {
            if let Some(l) = &self.live {
                if l.ready && self.now - l.heard > QUIET {
                    self.notice = Some("Live updates went quiet; reconnecting.".into());
                    return self.start_watch();
                }
                return Vec::new();
            }
            if self.watch_retry_at.is_some_and(|t| self.now >= t) {
                return self.start_watch();
            }
            return Vec::new();
        }
        let due = self.last_loaded.is_some_and(|t| self.now - t >= REFRESH);
        if due && self.overlay.is_none() && self.form.is_none() {
            return self.load_view();
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Msg, default_views};
    use crate::fake::InProcess;
    use crate::model::IssueRow;
    use crossterm::event::{KeyCode, KeyEvent};

    fn t0() -> DateTime<Utc> {
        "2026-10-26T12:00:00Z".parse().unwrap()
    }

    /// An app on the fake; requests answered, watch effects returned.
    fn run(app: &mut App, fx: Vec<Effect>) -> Vec<Effect> {
        let exec = InProcess { now: t0(), state: None };
        let mut other = Vec::new();
        for f in fx {
            match f {
                Effect::Run(req) => {
                    let more = app.update(req.run(&exec));
                    other.extend(run(app, more));
                }
                f => other.push(f),
            }
        }
        other
    }

    fn app(can_watch: bool) -> (App, Vec<Effect>) {
        let mut app = App::new(default_views(), t0());
        app.can_watch = can_watch;
        let fx = app.start();
        let fx = run(&mut app, fx);
        (app, fx)
    }

    fn key(app: &mut App, c: KeyCode) -> Vec<Effect> {
        let fx = app.update(Msg::Key(KeyEvent::from(c)));
        run(app, fx)
    }

    fn row(id: &str, title: &str) -> Box<IssueRow> {
        Box::new(IssueRow {
            id: id.into(),
            identifier: id.to_uppercase(),
            title: title.into(),
            ..IssueRow::default()
        })
    }

    fn ids(app: &App) -> Vec<&str> {
        app.visible().iter().map(|r| r.identifier.as_str()).collect()
    }

    fn watched(app: &mut App, ev: WatchEvent) -> Vec<Effect> {
        let g = app.live.as_ref().unwrap().generation;
        let fx = app.update(Msg::Watched { generation: g, event: ev });
        run(app, fx)
    }

    #[test]
    fn a_loaded_view_starts_a_watch() {
        let (app, fx) = app(true);
        let mut argv = ctl::watch_argv(&default_views()[0].args);
        let json = argv.pop().unwrap();
        argv.extend(["--since".into(), "2026-10-26T12:00:00Z".into(), json]);
        assert_eq!(fx, [Effect::Watch { generation: 1, argv: argv.clone() }]);
        let d = crate::contract::Describe::from_json(include_str!(
            "../../tests/fixtures/linearctl-describe-0.4.0.json"
        ))
        .unwrap();
        assert!(d.check_argv(&argv).is_empty());
        let live = app.live.as_ref().unwrap();
        assert!(!live.ready && live.args == default_views()[0].args);
    }

    #[test]
    fn no_watch_without_the_command() {
        let (app, fx) = app(false);
        assert!(fx.is_empty());
        assert!(app.live.is_none());
    }

    #[test]
    fn events_update_the_list() {
        let (mut app, _) = app(true);
        watched(&mut app, WatchEvent::Ready { mode: "subscription".into(), since: String::new() });
        assert!(app.live.as_ref().unwrap().ready);
        assert_eq!(ids(&app), ["ENG-142", "ENG-143", "ENG-140", "ENG-120"]);
        key(&mut app, KeyCode::Down); // select ENG-143
        // A new issue comes in on top; the selection stays on ENG-143.
        watched(&mut app, WatchEvent::Upsert { issue: row("new", "Fresh") });
        assert_eq!(ids(&app)[0], "NEW");
        assert_eq!(app.selected_row().unwrap().identifier, "ENG-143");
        // A changed issue is replaced in place.
        let mut changed = app.rows[1].clone();
        changed.title = "Renamed".into();
        watched(&mut app, WatchEvent::Upsert { issue: Box::new(changed) });
        assert_eq!(app.rows[1].title, "Renamed");
        // A removed one goes; removing the selected row keeps the place.
        let gone = app.rows[0].id.clone();
        watched(&mut app, WatchEvent::Remove { id: gone, identifier: None, reason: "filter".into() });
        assert_eq!(app.selected_row().unwrap().identifier, "ENG-143");
        let sel = app.selected_row().unwrap().id.clone();
        watched(&mut app, WatchEvent::Remove { id: sel, identifier: None, reason: "archived".into() });
        assert_eq!(app.selected_row().unwrap().identifier, "ENG-140");
        watched(
            &mut app,
            WatchEvent::Remove { id: "unknown".into(), identifier: None, reason: "filter".into() },
        );
        assert_eq!(ids(&app).len(), 3);
        watched(&mut app, WatchEvent::Heartbeat { at: String::new(), mode: "poll".into() });
        assert_eq!(app.live.as_ref().unwrap().mode, "poll");
        watched(&mut app, WatchEvent::Other);
    }

    #[test]
    fn a_change_to_the_open_issue_fetches_it_again() {
        let (mut app, _) = app(true);
        key(&mut app, KeyCode::Enter);
        let mut r = app.rows[0].clone();
        r.title = "Changed elsewhere".into();
        let g = app.live.as_ref().unwrap().generation;
        let fx = app.update(Msg::Watched { generation: g, event: WatchEvent::Upsert { issue: Box::new(r) } });
        assert!(matches!(&fx[..], [Effect::Run(Request::Show { id, .. })] if id == "ENG-142"));
    }

    #[test]
    fn gaps_say_what_happened() {
        let (mut app, _) = app(true);
        watched(&mut app, WatchEvent::Gap { reason: "reconnect".into(), mode: "subscription".into() });
        assert_eq!(app.notice.as_deref(), Some("Live updates: reconnected and caught up."));
        watched(&mut app, WatchEvent::Gap { reason: "fallback".into(), mode: "poll".into() });
        assert_eq!(app.notice.as_deref(), Some("Live updates: Linear's stream dropped; polling instead."));
    }

    #[test]
    fn stale_events_are_ignored() {
        let (mut app, _) = app(true);
        assert!(
            app.update(Msg::Watched { generation: 99, event: WatchEvent::Upsert { issue: row("x", "x") } })
                .is_empty()
        );
        assert_eq!(ids(&app).len(), 4);
        assert!(app.update(Msg::WatchEnded { generation: 99, failure: None }).is_empty());
        assert!(app.live.is_some());
    }

    #[test]
    fn views_and_searches_move_the_watch() {
        let (mut app, _) = app(true);
        let fx = key(&mut app, KeyCode::Char('2'));
        assert!(
            matches!(&fx[..], [Effect::Watch { generation: 2, argv }] if argv.contains(&"started".to_string()))
        );
        // Reloading the same view keeps its watch.
        assert!(key(&mut app, KeyCode::Char('r')).is_empty());
        key(&mut app, KeyCode::Char('/'));
        for c in "coupon".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        let fx = key(&mut app, KeyCode::Enter);
        assert_eq!(fx, [Effect::StopWatch]);
        assert!(app.live.is_none());
        assert!(app.on_tick().is_empty(), "no watch for a search");
        let fx = key(&mut app, KeyCode::Esc);
        assert!(matches!(&fx[..], [Effect::Watch { generation: 3, .. }]));
    }

    #[test]
    fn an_ended_watch_is_retried() {
        let (mut app, _) = app(true);
        let g = app.live.as_ref().unwrap().generation;
        app.update(Msg::WatchEnded {
            generation: g,
            failure: Some(Failure::RateLimited { message: "rate limited".into(), retry_at: None }),
        });
        assert!(app.live.is_none());
        assert_eq!(app.status.as_deref(), Some("Live updates stopped (rate limited); trying again in 30 s."));
        app.now += Duration::seconds(29);
        assert!(app.update(Msg::Tick).is_empty());
        app.now += Duration::seconds(1);
        assert!(matches!(&app.update(Msg::Tick)[..], [Effect::Watch { .. }]));
        let g = app.live.as_ref().unwrap().generation;
        app.update(Msg::WatchEnded { generation: g, failure: None });
        assert!(app.status.as_deref().unwrap().contains("(it ended)"));
    }

    #[test]
    fn a_quiet_watch_is_restarted() {
        let (mut app, _) = app(true);
        // Not ready yet: it is still starting, so no restart.
        app.now += Duration::seconds(100);
        assert!(app.update(Msg::Tick).is_empty());
        watched(&mut app, WatchEvent::Ready { mode: "subscription".into(), since: String::new() });
        app.now += Duration::seconds(75);
        assert!(app.update(Msg::Tick).is_empty());
        app.now += Duration::seconds(1);
        assert!(matches!(&app.update(Msg::Tick)[..], [Effect::Watch { .. }]));
        assert_eq!(app.notice.as_deref(), Some("Live updates went quiet; reconnecting."));
    }

    #[test]
    fn without_watch_the_view_is_reloaded_every_two_minutes() {
        let (mut app, _) = app(false);
        app.now += Duration::seconds(119);
        assert!(app.update(Msg::Tick).is_empty());
        app.now += Duration::seconds(1);
        let fx = app.update(Msg::Tick);
        assert!(matches!(&fx[..], [Effect::Run(Request::List { append: false, .. })]));
        // Not while loading, nor with a dialog open.
        assert!(app.update(Msg::Tick).is_empty());
        app.loading = false;
        app.overlay = Some(super::super::Overlay::Confirm(Box::new(super::super::Confirm {
            title: String::new(),
            lines: vec![],
            command: String::new(),
            argv: vec![],
            text: None,
            purpose: super::super::Purpose::Create,
        })));
        assert!(app.update(Msg::Tick).is_empty());
    }

    #[test]
    fn the_describe_check() {
        let d = |s| crate::contract::Describe::from_json(s).unwrap();
        assert!(!can_watch(&d(include_str!("../../tests/fixtures/linearctl-describe-0.1.0.json"))));
        assert!(can_watch(&d(include_str!("../../tests/fixtures/linearctl-describe-0.4.0.json"))));
    }
}
