use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root is cli/'s parent")
        .to_path_buf()
}

#[test]
fn route_races_are_bounded_in_gate_and_soaked_nightly() {
    let routes = fs::read_to_string(repo_root().join("server/tests/routes.rs"))
        .expect("read server route tests");
    assert!(
        routes.contains("const PR_FIRM_ANCHOR_RACE_ROUNDS: usize = 1"),
        "the route races need one bounded PR-gate round",
    );
    assert_eq!(
        routes
            .matches("for round in 0..PR_FIRM_ANCHOR_RACE_ROUNDS")
            .count(),
        2,
        "the two fresh-engine route races must use the bounded PR-gate round",
    );
    assert!(
        !routes.contains("for round in 0..12"),
        "the full route-race volume belongs in the scheduled soak",
    );

    let soak = fs::read_to_string(repo_root().join(".github/workflows/firm-anchor-soak.yml"))
        .expect("read firm-anchor soak workflow");
    assert!(
        soak.contains("schedule:") && soak.contains("cron: \"0 8 * * *\""),
        "the route races must remain on the nightly schedule",
    );
    assert!(
        soak.contains("ROUTE_ROUNDS=12"),
        "the soak must carry the original twelve-round route reproduction",
    );
    for test_name in [
        "concurrent_creates_cannot_fork_the_firm_anchor",
        "a_delete_racing_a_rename_into_the_firm_name_never_removes_the_anchor",
        "a_rename_racing_a_rename_into_the_firm_name_never_loses_the_anchor",
    ] {
        assert!(
            soak.contains(test_name),
            "the nightly soak must run route test {test_name}",
        );
    }
    assert!(
        soak.contains("ENG-441")
            && soak.contains("ENG-312")
            && soak.contains("do not re-run to clear it"),
        "the soak must explain the scheduled, non-rerunnable race signal",
    );
}
