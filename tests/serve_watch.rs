//! ctl::Serve and ctl::Watcher against the fake-linearctl binary.

use std::sync::mpsc;
use std::time::Duration;

use linear_tui::app::{Msg, Purpose, Request, Text};
use linear_tui::ctl::{self, Exec, Failure, Mode, Runner, Serve, WatchLine, Watcher};
use linear_tui::fake::STATE_ENV;
use linear_tui::model::{IssueRow, Page, ShowResult, WatchEvent};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-linearctl");

/// A runner on the fake with a state file of its own.
fn runner(name: &str) -> (Runner, std::path::PathBuf) {
    let state = std::env::temp_dir().join(format!("linear-tui-sw-{name}-{}", std::process::id()));
    let _ = std::fs::remove_file(&state);
    let mut r = Runner::new(FAKE);
    r.env.push((STATE_ENV.into(), state.display().to_string()));
    (r, state)
}

#[test]
fn serve_answers_reads_writes_and_failures() {
    let (r, state) = runner("serve");
    let serve = Serve::new(r, 2);
    let page: Page<IssueRow> = ctl::run_as(&serve, &ctl::list_argv(&[], 5, None)).unwrap();
    assert_eq!(page.items.len(), 5);
    let show: ShowResult = ctl::run_as(&serve, &ctl::show_argv("ENG-142")).unwrap();
    assert_eq!(show.issue.comments.len(), 4);
    assert_eq!(
        serve.exec(&ctl::show_argv("ENG-999")),
        Err(Failure::Failed("not found: issue ENG-999 (or this API key cannot see it)".into()))
    );
    // A write with its text in a file, the way the app sends it.
    let req = Request::Write {
        purpose: Purpose::Description { issue: "ENG-155".into() },
        argv: ctl::update_argv("ENG-155", &[], Mode::Yes),
        text: Some(Text { flag: "description", text: "Over serve.".into() }),
        summary: vec![],
    };
    let Msg::Wrote { result, .. } = req.run(&serve) else { panic!() };
    assert!(result.unwrap().changed);
    let show: ShowResult = ctl::run_as(&serve, &ctl::show_argv("ENG-155")).unwrap();
    assert_eq!(show.issue.description.as_deref(), Some("Over serve."));
    std::fs::remove_file(state).unwrap();
}

#[test]
fn serve_runs_calls_side_by_side() {
    let (r, state) = runner("pool");
    let serve = std::sync::Arc::new(Serve::new(r, 2));
    let handles: Vec<_> = (0..6)
        .map(|i| {
            let s = serve.clone();
            std::thread::spawn(move || {
                let page: Page<IssueRow> = ctl::run_as(&*s, &ctl::list_argv(&[], i + 1, None)).unwrap();
                page.items.len()
            })
        })
        .collect();
    let mut sizes: Vec<usize> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    sizes.sort_unstable();
    assert_eq!(sizes, [1, 2, 3, 4, 5, 6]);
    let _ = std::fs::remove_file(state);
}

#[test]
fn without_serve_calls_fall_back_to_a_process_each() {
    let (mut r, state) = runner("noserve");
    r.env.push(("FAKE_LINEARCTL_NO_SERVE".into(), "1".into()));
    let serve = Serve::new(r, 1);
    let page: Page<IssueRow> = ctl::run_as(&serve, &ctl::list_argv(&[], 3, None)).unwrap();
    assert_eq!(page.items.len(), 3);
    let missing = Serve::new(Runner::new("/nonexistent/linearctl"), 1);
    assert!(matches!(missing.exec(&ctl::auth_status_argv()), Err(Failure::Spawn(_))));
    let _ = std::fs::remove_file(state);
}

fn next(rx: &mpsc::Receiver<WatchLine>) -> WatchLine {
    rx.recv_timeout(Duration::from_secs(10)).expect("a watch line in time")
}

#[test]
fn watch_reports_a_write_made_by_another_call() {
    let (r, state) = runner("watch");
    let (tx, rx) = mpsc::channel();
    let argv = ctl::watch_argv(&["--assignee".into(), "me".into()]);
    let w = Watcher::start(&r, &argv, move |l| {
        let _ = tx.send(l);
    })
    .unwrap();
    assert!(matches!(next(&rx), WatchLine::Event(WatchEvent::Ready { mode, .. }) if mode == "poll"));
    r.run(&ctl::update_argv("ENG-142", &[("state", "Done")], Mode::Yes)).unwrap();
    loop {
        match next(&rx) {
            WatchLine::Event(WatchEvent::Upsert { issue }) => {
                assert_eq!(issue.identifier, "ENG-142");
                assert_eq!(issue.state.unwrap().name, "Done");
                break;
            }
            WatchLine::Event(WatchEvent::Heartbeat { .. }) => {}
            other => panic!("{other:?}"),
        }
    }
    drop(w);
    // Stopped on purpose: no End is reported.
    assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
    std::fs::remove_file(state).unwrap();
}

#[test]
fn a_failing_watch_ends_with_its_failure() {
    let (r, _) = runner("watch-fail");
    let (tx, rx) = mpsc::channel();
    let _w = Watcher::start(&r, &["watch".into(), "--sort".into(), "x".into(), "--json".into()], move |l| {
        let _ = tx.send(l);
    })
    .unwrap();
    assert!(matches!(next(&rx), WatchLine::End(Some(Failure::Usage(m))) if m.contains("--sort")));
    assert!(Watcher::start(&Runner::new("/nonexistent/linearctl"), &[], |_| {}).is_err());
}
