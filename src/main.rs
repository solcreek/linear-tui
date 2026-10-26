//! linear-tui: a terminal UI for Linear on top of linearctl.

use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use linear_tui::app::{App, Effect, Msg, default_views, missing};
use linear_tui::contract::Describe;
use linear_tui::ctl::Runner;
use linear_tui::ui;

const USAGE: &str = "\
linear-tui: a terminal UI for Linear, on top of linearctl

Usage: linear-tui [--demo] [--linearctl PATH]

  --demo             browse a fictional workspace; needs no Linear account
  --linearctl PATH   the linearctl to run (default: $LINEARCTL, else linearctl on PATH)
  -h, --help         this help
  --version          the version

linearctl holds the API key: run `linearctl auth login --yes` first.";

struct Options {
    program: PathBuf,
}

fn parse(args: &[String]) -> Result<Option<Options>, String> {
    let mut demo = false;
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
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let program = if demo {
        // fake-linearctl ships beside this binary until linearctl has a demo mode.
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        exe.with_file_name(format!("fake-linearctl{}", std::env::consts::EXE_SUFFIX))
    } else {
        program.unwrap_or_else(|| "linearctl".into())
    };
    Ok(Some(Options { program }))
}

/// Check that this linearctl has every command and flag the TUI uses.
fn check_linearctl(program: &PathBuf) -> Result<(), String> {
    let d = Describe::from_command(Command::new(program).args(["describe", "--json"]))?;
    let problems = missing(&d, &default_views());
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "linearctl {} lacks what linear-tui needs:\n  {}",
            d.version,
            problems.join("\n  ")
        ))
    }
}

fn spawn(runner: &Runner, tx: &Sender<Msg>, fx: Vec<Effect>) {
    for f in fx {
        match f {
            Effect::Run(req) => {
                let (runner, tx) = (runner.clone(), tx.clone());
                thread::spawn(move || {
                    let msg = req.run(&runner);
                    let _ = tx.send(msg);
                });
            }
            Effect::Open(url) => {
                let opener = if cfg!(target_os = "macos") {
                    "open"
                } else {
                    "xdg-open"
                };
                let _ = Command::new(opener)
                    .arg(url)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
        }
    }
}

fn run(runner: &Runner) -> std::io::Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut app = App::new(default_views(), chrono::Utc::now());
    let mut terminal = ratatui::init();
    spawn(runner, &tx, app.start());
    let result = loop {
        app.now = chrono::Utc::now();
        if let Err(e) = terminal.draw(|f| ui::render(&mut app, f)) {
            break Err(e);
        }
        match event::poll(Duration::from_millis(50)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                    spawn(runner, &tx, app.update(Msg::Key(k)))
                }
                Ok(_) => {}
                Err(e) => break Err(e),
            },
            Ok(false) => {}
            Err(e) => break Err(e),
        }
        while let Ok(msg) = rx.try_recv() {
            spawn(runner, &tx, app.update(msg));
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
    if let Err(e) = check_linearctl(&opts.program) {
        eprintln!("linear-tui: {e}");
        return ExitCode::FAILURE;
    }
    match run(&Runner::new(opts.program)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("linear-tui: {e}");
            ExitCode::FAILURE
        }
    }
}
