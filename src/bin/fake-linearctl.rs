//! `fake-linearctl`: answers like linearctl from the demo workspace.
//! `linear-tui --demo` runs it; tests use it as linearctl's double.

use std::io::Write;
use std::time::Duration;

use linear_tui::fake;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let state = std::env::var_os(fake::STATE_ENV).map(std::path::PathBuf::from);
    match argv.first().map(String::as_str) {
        // FAKE_LINEARCTL_NO_SERVE: act like a linearctl without serve (0.1.0).
        Some("serve")
            if argv.iter().any(|a| a == "--stdio")
                && std::env::var_os("FAKE_LINEARCTL_NO_SERVE").is_none() =>
        {
            fake::stream::serve(
                std::io::stdin().lock(),
                std::io::stdout().lock(),
                chrono::Utc::now,
                state.as_deref(),
            );
            return;
        }
        Some("watch") => {
            let tick = Duration::from_millis(200);
            let code = fake::stream::watch(
                &argv[1..],
                std::io::stdout().lock(),
                chrono::Utc::now,
                state.as_deref(),
                || true,
                tick,
            );
            std::process::exit(code);
        }
        _ => {}
    }
    let fault = std::env::var(fake::FAULT_ENV).ok();
    let out = fake::run_with_fault(&argv, chrono::Utc::now(), fault.as_deref(), state.as_deref());
    // A closed pipe is the reader's business, not an error worth a panic.
    let _ = std::io::stdout().write_all(out.stdout.as_bytes());
    let _ = std::io::stderr().write_all(out.stderr.as_bytes());
    std::process::exit(out.code);
}
