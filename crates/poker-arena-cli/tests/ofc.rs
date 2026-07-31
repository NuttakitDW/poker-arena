//! CLI smoke tests for the `poker-arena-ofc` binary: a short match in both
//! output modes, checked end-to-end through the real executable (no
//! assert_cmd in this workspace yet, so this drives `std::process::Command`
//! directly against `CARGO_BIN_EXE_poker-arena-ofc`, the same pattern
//! `poker-arena/tests/wire.rs` uses for `CARGO_BIN_EXE_wire-caller`).

use std::process::Command;

use poker_wire::ofc::report::OfcMatchReport;

fn base_args() -> Vec<&'static str> {
    vec![
        "run",
        "--game",
        "ofc",
        "--bot",
        "builtin:greedy",
        "--bot",
        "builtin:random",
        "--hands",
        "20",
        "--seed",
        "7",
    ]
}

#[test]
fn human_output_exits_clean_with_a_report_table_and_a_stderr_seed_line() {
    let output = Command::new(env!("CARGO_BIN_EXE_poker-arena-ofc"))
        .args(base_args())
        .output()
        .expect("run poker-arena-ofc");

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("game: Open Face Chinese"), "{stdout}");
    assert!(stdout.contains("hands: 20"), "{stdout}");
    assert!(stdout.contains("seed: 7"), "{stdout}");
    // Report table header: name + every stat column the design calls for.
    for column in [
        "bot",
        "hands",
        "points",
        "fouls",
        "fls",
        "scoops",
        "royalties",
        "faults",
    ] {
        assert!(
            stdout.contains(column),
            "missing column {column:?}: {stdout}"
        );
    }
    assert!(stdout.contains("greedy"), "{stdout}");
    assert!(stdout.contains("random"), "{stdout}");

    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(
        stderr.lines().any(|line| line.starts_with("seed: 7")),
        "expected a seed line on stderr, got: {stderr}"
    );
}

#[test]
fn json_output_parses_as_a_valid_ofc_match_report() {
    let mut args = base_args();
    args.extend(["--output", "json"]);
    let output = Command::new(env!("CARGO_BIN_EXE_poker-arena-ofc"))
        .args(args)
        .output()
        .expect("run poker-arena-ofc");

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one JSON document on stdout: {stdout}"
    );

    let report: OfcMatchReport = serde_json::from_str(lines[0]).expect("valid OfcMatchReport");
    assert_eq!(report.game_id, "ofc");
    assert_eq!(report.hands, 20);
    assert_eq!(report.seed, 7);
    assert_eq!(report.seat_count, 2);
    assert_eq!(report.fault_policy, "substitute");
    assert!(report.forfeited_by.is_none());
    assert_eq!(report.bots.len(), 2);
    assert_eq!(report.bots[0].name, "greedy");
    assert_eq!(report.bots[0].kind, "builtin:greedy");
    assert_eq!(report.bots[1].kind, "builtin:random");
    let total: i64 = report.bots.iter().map(|b| b.points).sum();
    assert_eq!(total, 0, "points must stay zero-sum");

    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(
        stderr.lines().any(|line| line.starts_with("seed: 7")),
        "expected a seed line on stderr, got: {stderr}"
    );
}

#[test]
fn games_lists_every_ofc_variant() {
    let output = Command::new(env!("CARGO_BIN_EXE_poker-arena-ofc"))
        .arg("games")
        .output()
        .expect("run poker-arena-ofc games");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    for id in ["ofc", "ofc-pineapple", "ofc-progressive", "ofc-27"] {
        assert!(stdout.contains(id), "missing {id:?} in: {stdout}");
    }
}
