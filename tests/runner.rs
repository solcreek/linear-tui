//! ctl::Runner against the fake-linearctl binary, faults included.

use std::time::Duration;

use linear_tui::ctl::{self, Failure, Runner};
use linear_tui::fake::FAULT_ENV;
use linear_tui::model::{AuthStatus, IssueRow, Page, ShowResult};

fn runner(fault: Option<&str>) -> Runner {
    let mut r = Runner::new(env!("CARGO_BIN_EXE_fake-linearctl"));
    if let Some(f) = fault {
        r.env.push((FAULT_ENV.into(), f.into()));
    }
    r.busy_backoff = Duration::from_millis(1);
    r
}

fn auth(r: &Runner) -> Result<AuthStatus, Failure> {
    r.run_as(&ctl::auth_status_argv())
}

#[test]
fn reads_typed_results() {
    let r = runner(None);
    let page: Page<IssueRow> = r.run_as(&ctl::list_argv(&[], 5, None)).unwrap();
    assert_eq!(page.items.len(), 5);
    let next: Page<IssueRow> = r
        .run_as(&ctl::list_argv(&[], 5, page.next_cursor.as_deref()))
        .unwrap();
    assert_ne!(next.items[0].id, page.items[0].id);
    let show: ShowResult = r.run_as(&ctl::show_argv("ENG-142")).unwrap();
    assert_eq!(show.issue.comments.len(), 4);
    assert_eq!(auth(&r).unwrap().organization.name, "Northwind");
}

#[test]
fn maps_every_exit_code() {
    let fail = |code: &str| auth(&runner(Some(&format!("exit:{code}")))).unwrap_err();
    assert_eq!(fail("1"), Failure::Failed("injected failure 1".into()));
    assert_eq!(fail("2"), Failure::Usage("injected failure 2".into()));
    assert_eq!(fail("3"), Failure::Partial("injected failure 3".into()));
    assert_eq!(fail("5"), Failure::Auth("injected failure 5".into()));
    assert_eq!(
        fail("6"),
        Failure::RateLimited {
            message: "rate limited by Linear".into(),
            retry_at: Some("2026-10-26T13:00:00Z".into())
        }
    );
    assert_eq!(fail("7"), Failure::Conflict("injected failure 7".into()));
    assert_eq!(
        fail("8"),
        Failure::Crashed {
            code: Some(8),
            stderr: "injected failure 8".into()
        }
    );
}

#[test]
fn not_found_is_failed() {
    let err = runner(None).run(&ctl::show_argv("ENG-999")).unwrap_err();
    assert_eq!(
        err,
        Failure::Failed("not found: issue ENG-999 (or this API key cannot see it)".into())
    );
}

#[test]
fn busy_is_retried() {
    let marker = std::env::temp_dir().join(format!("linear-tui-busy-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let r = runner(Some(&format!("busy-once:{}", marker.display())));
    assert!(auth(&r).is_ok());
    std::fs::remove_file(&marker).unwrap();
}

#[test]
fn busy_gives_up_after_the_retries() {
    let mut r = runner(Some("exit:4"));
    r.busy_retries = 2;
    assert_eq!(auth(&r).unwrap_err(), Failure::Busy("injected failure 4".into()));
}

#[test]
fn a_slow_linearctl_is_killed() {
    let mut r = runner(Some("sleep:5000"));
    r.timeout = Duration::from_millis(200);
    let start = std::time::Instant::now();
    assert_eq!(
        auth(&r).unwrap_err(),
        Failure::Timeout(Duration::from_millis(200))
    );
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn bad_output() {
    assert!(matches!(
        auth(&runner(Some("garbage"))),
        Err(Failure::BadOutput(_))
    ));
    // Valid JSON of the wrong shape.
    let r = runner(None);
    let wrong: Result<Vec<String>, _> = r.run_as(&ctl::auth_status_argv());
    assert!(matches!(wrong, Err(Failure::BadOutput(_))));
}

#[test]
fn a_crash_keeps_stderr() {
    let err = auth(&runner(Some("crash"))).unwrap_err();
    assert_eq!(
        err,
        Failure::Crashed {
            code: Some(101),
            stderr: "thread 'main' panicked\n".into()
        }
    );
}

#[test]
fn a_missing_binary() {
    let err = auth(&Runner::new("/nonexistent/linearctl")).unwrap_err();
    assert!(matches!(err, Failure::Spawn(m) if m.starts_with("/nonexistent/linearctl")));
}

#[test]
fn requests_become_messages() {
    use linear_tui::app::{App, Effect, Msg, default_views};
    let r = runner(None);
    let mut app = App::new(default_views(), chrono::Utc::now());
    for fx in app.start() {
        let Effect::Run(req) = fx else { panic!() };
        let msg = req.run(&r);
        assert!(
            matches!(&msg, Msg::Listed { result: Ok(_), .. } | Msg::Authed(Ok(_))),
            "{msg:?}"
        );
        app.update(msg);
    }
    let fx = app.update(Msg::Key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Enter,
    )));
    let Effect::Run(req) = fx.into_iter().next().unwrap() else {
        panic!()
    };
    let Msg::Shown { id, result } = req.run(&r) else {
        panic!()
    };
    assert_eq!(id, "ENG-142");
    assert_eq!(result.unwrap().comments.len(), 4);
}
