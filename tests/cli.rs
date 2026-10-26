//! linear-tui's command line, before any terminal is touched.

use std::process::Command;

fn tui(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_linear-tui"))
        .args(args)
        .env_remove("LINEARCTL")
        .output()
        .unwrap()
}

#[test]
fn help_and_version_exit_0() {
    let h = tui(&["--help"]);
    assert!(h.status.success());
    assert!(String::from_utf8_lossy(&h.stdout).contains("Usage: linear-tui [--demo]"));
    let v = tui(&["--version"]);
    assert_eq!(
        String::from_utf8_lossy(&v.stdout),
        format!("linear-tui {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_arguments_are_usage_errors() {
    let out = tui(&["--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("linear-tui: unknown argument: --bogus"));
    assert_eq!(tui(&["--linearctl"]).status.code(), Some(2));
}

#[test]
fn a_missing_linearctl_fails_before_the_terminal() {
    let out = tui(&["--linearctl", "/nonexistent/linearctl"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot run"));
}

#[test]
fn a_failing_describe_is_reported() {
    let out = Command::new(env!("CARGO_BIN_EXE_linear-tui"))
        .args(["--linearctl", env!("CARGO_BIN_EXE_fake-linearctl")])
        .env("FAKE_LINEARCTL_FAULT", "exit:1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("failed"));
}
