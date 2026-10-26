//! Every linearctl call named in docs/design.md exists in linearctl.

use linear_tui::contract::Describe;

const SNAPSHOT: &str = include_str!("fixtures/linearctl-describe-0.1.0.json");
const DESIGN: &str = include_str!("../docs/design.md");

fn assert_design_matches(d: &Describe) {
    let (found, problems) = d.check_markdown(DESIGN);
    assert!(found > 0, "no linearctl invocations in docs/design.md");
    assert!(
        problems.is_empty(),
        "docs/design.md vs linearctl {}:\n{}",
        d.version,
        problems.join("\n")
    );
}

#[test]
fn snapshot_is_linearctl_0_1_0() {
    assert_eq!(Describe::from_json(SNAPSHOT).unwrap().version, "0.1.0");
}

#[test]
fn design_matches_the_0_1_0_snapshot() {
    assert_design_matches(&Describe::from_json(SNAPSHOT).unwrap());
}

/// Against the installed linearctl: `LINEARCTL_LIVE=linearctl cargo test`.
#[test]
fn design_matches_the_live_linearctl() {
    let Ok(program) = std::env::var("LINEARCTL_LIVE") else {
        eprintln!("skipped: set LINEARCTL_LIVE to a linearctl binary");
        return;
    };
    assert_design_matches(&Describe::from_linearctl(&program).unwrap());
}
