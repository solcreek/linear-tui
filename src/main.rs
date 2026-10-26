//! linear-tui: a terminal UI for Linear on top of linearctl.

use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use linear_tui::app::{App, EditFor, Effect, IssueRef, Msg, default_views, issue_ref, missing};
use linear_tui::contract::Describe;
use linear_tui::ctl::{Exec, Runner, Serve, WatchLine, Watcher};
use linear_tui::images::{self, DiskCache, Loader};
use linear_tui::ui;
use ratatui_image::picker::{Picker, ProtocolType};

const USAGE: &str = "\
linear-tui: a terminal UI for Linear, on top of linearctl

Usage: linear-tui [--demo] [--linearctl PATH] [--images MODE] [--cell-size WxH] [ISSUE]

  ISSUE              open this issue first: ENG-142, or its linear.app URL

  --demo             browse a fictional workspace; needs no Linear account
  --linearctl PATH   the linearctl to run (default: $LINEARCTL, else linearctl on PATH)
  --images MODE      auto (default), kitty, sixel, iterm2, halfblocks or off
  --cell-size WxH    a terminal cell in pixels, when the terminal cannot report it
  -h, --help         this help
  --version          the version

linearctl holds the API key: run `linearctl auth login --yes` first.";

/// linearctl serve processes: calls at a time.
const SERVE_PROCESSES: usize = 3;
/// Image fetches at a time.
const IMAGE_WORKERS: usize = 4;
/// The disk cache for images.
const IMAGE_CACHE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq)]
enum Images {
    Auto,
    Off,
    Force(ProtocolType),
}

struct Options {
    program: PathBuf,
    demo: bool,
    images: Images,
    cell: Option<(u16, u16)>,
    /// The issue to open first.
    issue: Option<IssueRef>,
}

fn parse(args: &[String]) -> Result<Option<Options>, String> {
    let mut demo = false;
    let mut images = Images::Auto;
    let mut cell = None;
    let mut issue = None;
    let mut program = std::env::var_os("LINEARCTL").map(PathBuf::from);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "--version" => {
                println!("linear-tui {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--demo" => demo = true,
            "--linearctl" => program = Some(it.next().ok_or("--linearctl needs a PATH")?.into()),
            "--images" => {
                images = match it.next().map(String::as_str) {
                    Some("auto") => Images::Auto,
                    Some("off") => Images::Off,
                    Some("kitty") => Images::Force(ProtocolType::Kitty),
                    Some("sixel") => Images::Force(ProtocolType::Sixel),
                    Some("iterm2") => Images::Force(ProtocolType::Iterm2),
                    Some("halfblocks") => Images::Force(ProtocolType::Halfblocks),
                    _ => return Err("--images needs auto, kitty, sixel, iterm2, halfblocks or off".into()),
                }
            }
            "--cell-size" => cell = Some(cell_size(it.next())?),
            other if other.starts_with('-') || issue.is_some() => {
                return Err(format!("unknown argument: {other}"));
            }
            other => {
                let id = issue_ref(other)
                    .ok_or_else(|| format!("not an issue identifier or Linear issue URL: {other}"))?;
                issue = Some(id);
            }
        }
    }
    let program = program.unwrap_or_else(|| "linearctl".into());
    Ok(Some(Options { program, demo, images, cell, issue }))
}

/// `--cell-size WxH`, both above zero.
fn cell_size(v: Option<&String>) -> Result<(u16, u16), String> {
    let bad = || "--cell-size needs WxH, e.g. 10x20".to_string();
    let (w, h) = v.ok_or_else(bad)?.split_once('x').ok_or_else(bad)?;
    match (w.parse(), h.parse()) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => Ok((w, h)),
        _ => Err(bad()),
    }
}

/// The linearctl to run, and its environment. For the demo: linearctl's own
/// demo mode when it has one, else fake-linearctl from beside this binary.
fn linearctl(opts: &Options) -> Result<(Runner, Describe), String> {
    if !opts.demo {
        let d = Describe::from_linearctl(&opts.program.to_string_lossy())?;
        check(&d)?;
        return Ok((Runner::new(&opts.program), d));
    }
    if let Ok(d) = Describe::from_linearctl(&opts.program.to_string_lossy())
        && d.has_global_flag("demo")
        && check(&d).is_ok()
    {
        let mut r = Runner::new(&opts.program);
        r.env.push(("LINEARCTL_DEMO".into(), "1".into()));
        return Ok((r, d));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let fake = exe.with_file_name(format!("fake-linearctl{}", std::env::consts::EXE_SUFFIX));
    let d = Describe::from_linearctl(&fake.to_string_lossy())?;
    check(&d)?;
    Ok((Runner::new(fake), d))
}

/// That this linearctl has every command and flag the TUI uses.
fn check(d: &Describe) -> Result<(), String> {
    let problems = missing(d, &default_views());
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("linearctl {} lacks what linear-tui needs:\n  {}", d.version, problems.join("\n  ")))
    }
}

/// Image fetches, on a few workers so a page of images loads in parallel
/// without a thread each.
fn image_workers(loader: Arc<Loader>, tx: Sender<Msg>) -> Sender<String> {
    let (jobs, rx) = mpsc::channel::<String>();
    let rx: Arc<Mutex<Receiver<String>>> = Arc::new(Mutex::new(rx));
    for _ in 0..IMAGE_WORKERS {
        let (loader, rx, tx) = (loader.clone(), rx.clone(), tx.clone());
        thread::spawn(move || {
            loop {
                let job = rx.lock().map(|r| r.recv());
                let Ok(Ok(url)) = job else { return };
                let result = loader.load(&url).map(Box::new);
                if tx.send(Msg::Image { url, result }).is_err() {
                    return;
                }
            }
        });
    }
    jobs
}

/// Where effects run: linearctl calls (over serve when it can), image
/// fetches, the watch for the view, the browser.
struct Effects {
    exec: Arc<dyn Exec + Send + Sync>,
    runner: Runner,
    tx: Sender<Msg>,
    images: Sender<String>,
    watcher: Option<Watcher>,
}

impl Effects {
    /// Start each effect; edits are returned, since they need the terminal.
    #[must_use]
    fn start(&mut self, fx: Vec<Effect>) -> Vec<(EditFor, String)> {
        let mut edits = Vec::new();
        for f in fx {
            match f {
                Effect::Run(req) => {
                    let (exec, tx) = (self.exec.clone(), self.tx.clone());
                    thread::spawn(move || {
                        let _ = tx.send(req.run(&*exec));
                    });
                }
                Effect::FetchImage(url) => {
                    let _ = self.images.send(url);
                }
                Effect::Edit { purpose, text } => edits.push((purpose, text)),
                Effect::Watch { generation, argv } => {
                    self.watcher = None; // stops the old one
                    let tx = self.tx.clone();
                    let report = move |line| {
                        let _ = tx.send(match line {
                            WatchLine::Event(event) => Msg::Watched { generation, event },
                            WatchLine::End(failure) => Msg::WatchEnded { generation, failure },
                        });
                    };
                    match Watcher::start(&self.runner, &argv, report) {
                        Ok(w) => self.watcher = Some(w),
                        Err(f) => {
                            let _ = self.tx.send(Msg::WatchEnded { generation, failure: Some(f) });
                        }
                    }
                }
                Effect::StopWatch => self.watcher = None,
                Effect::Open(url) => {
                    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
                    let _ = Command::new(opener).arg(url).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                }
            }
        }
        edits
    }
}

fn picker(mode: Images, cell: Option<(u16, u16)>) -> Option<Picker> {
    // Ask the terminal for its graphics support and cell size, unless told.
    #[allow(deprecated)] // from_fontsize is the way to skip the query
    let make = || match cell {
        Some(size) => Picker::from_fontsize(size.into()),
        None => Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks()),
    };
    match mode {
        Images::Off => None,
        Images::Auto => Some(make()),
        // Half blocks need no terminal support, so no query.
        Images::Force(ProtocolType::Halfblocks) if cell.is_none() => Some(Picker::halfblocks()),
        Images::Force(p) => {
            let mut picker = make();
            picker.set_protocol_type(p);
            Some(picker)
        }
    }
}

fn run(runner: &Runner, describe: &Describe, opts: &Options) -> std::io::Result<()> {
    let (tx, rx) = mpsc::channel();
    let loader = Loader {
        source: if opts.demo { Box::new(images::Demo) } else { Box::new(images::Http::new()) },
        // One cache for every workspace: keys are upload paths, which hold
        // the organization's id.
        cache: (!opts.demo)
            .then(|| images::cache_dir("uploads"))
            .flatten()
            .map(|d| DiskCache::new(d, IMAGE_CACHE_BYTES)),
        fallback: Some(runner.clone()),
    };
    let images = image_workers(Arc::new(loader), tx.clone());
    // Calls over serve --stdio when linearctl has it (0.2.0): one
    // connection to Linear, kept, instead of a process per call.
    let serve = ["serve".to_string(), "--stdio".to_string()];
    let exec: Arc<dyn Exec + Send + Sync> = if describe.check_argv(&serve).is_empty() {
        Arc::new(Serve::new(runner.clone(), SERVE_PROCESSES))
    } else {
        Arc::new(runner.clone())
    };
    let mut fx = Effects { exec, runner: runner.clone(), tx, images, watcher: None };
    let mut app = App::new(default_views(), chrono::Utc::now());
    let mut terminal = ratatui::init();
    app.picker = picker(opts.images, opts.cell);
    app.images_on = opts.images != Images::Off;
    app.can_check_updated = linear_tui::app::can_check_updated(describe);
    app.can_watch = linear_tui::app::can_watch(describe);
    app.can_reply = linear_tui::app::can_reply(describe);
    app.describe = Some(describe.clone());
    let mut start = app.start();
    if let Some(id) = &opts.issue {
        start.extend(app.open_ref(id.clone()));
    }
    let mut edits = fx.start(start);
    let mut tick = std::time::Instant::now();
    let result = loop {
        app.now = chrono::Utc::now();
        if let Err(e) = terminal.draw(|f| ui::render(&mut app, f)) {
            break Err(e);
        }
        match event::poll(Duration::from_millis(50)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                    edits.extend(fx.start(app.update(Msg::Key(k))));
                }
                Ok(_) => {}
                Err(e) => break Err(e),
            },
            Ok(false) => {}
            Err(e) => break Err(e),
        }
        while let Ok(msg) = rx.try_recv() {
            edits.extend(fx.start(app.update(msg)));
        }
        if tick.elapsed() >= Duration::from_secs(1) {
            tick = std::time::Instant::now();
            edits.extend(fx.start(app.update(Msg::Tick)));
        }
        // The editor gets the terminal; the TUI comes back after it.
        while let Some((purpose, text)) = edits.pop() {
            ratatui::restore();
            let edited = linear_tui::editor::edit(&text);
            terminal = ratatui::init();
            terminal.clear()?;
            edits.extend(fx.start(app.update(Msg::Edited { purpose, text: edited })));
        }
        if app.quit {
            break Ok(());
        }
    };
    ratatui::restore();
    result
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match parse(&args) {
        Ok(Some(o)) => o,
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("linear-tui: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let (runner, describe) = match linearctl(&opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("linear-tui: {e}");
            return ExitCode::FAILURE;
        }
    };
    match run(&runner, &describe, &opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("linear-tui: {e}");
            ExitCode::FAILURE
        }
    }
}
