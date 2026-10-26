//! `fake-linearctl`: answers like linearctl from the demo workspace.
//! `linear-tui --demo` runs it; tests use it as linearctl's double.

use std::io::Write;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let out = linear_tui::fake::run(&argv, chrono::Utc::now());
    // A closed pipe is the reader's business, not an error worth a panic.
    let _ = std::io::stdout().write_all(out.stdout.as_bytes());
    let _ = std::io::stderr().write_all(out.stderr.as_bytes());
    std::process::exit(out.code);
}
