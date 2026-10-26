//! The fake-linearctl binary behaves like linearctl on the process level.

use std::process::Command;

fn fake(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fake-linearctl")).args(args).output().unwrap()
}

#[test]
fn answers_on_stdout_with_exit_0() {
    let out = fake(&["issue", "list", "--limit", "2", "--json"]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["shown"], 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("more remain"));
}

#[test]
fn usage_errors_exit_2_with_json() {
    let out = fake(&["issue", "list", "--bogus", "1", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["usage"], true);
}
