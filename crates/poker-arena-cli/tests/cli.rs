//! CLI smoke tests for the `poker-arena` binary: short matches of both
//! families in both output modes, the unified games listing, and the runtime
//! validation that keeps the union flag set honest. Checked end-to-end
//! through the real executable (no assert_cmd in this workspace yet, so this
//! drives `std::process::Command` directly against `CARGO_BIN_EXE_*`, the
//! same pattern `poker-arena/tests/wire.rs` uses for `wire-caller`).

use std::path::PathBuf;
use std::process::{Command, Output};

use poker_wire::ofc::report::OfcMatchReport;
use poker_wire::report::MatchReport;

fn arena(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_poker-arena"))
        .args(args)
        .output()
        .expect("run poker-arena")
}

fn ok(args: &[&str]) -> (String, String) {
    let output = arena(args);
    assert!(
        output.status.success(),
        "args: {args:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        String::from_utf8(output.stderr).expect("utf8 stderr"),
    )
}

const BETTING: [&str; 11] = [
    "run",
    "--game",
    "holdem-nl",
    "--bot",
    "builtin:caller",
    "--bot",
    "builtin:random",
    "--hands",
    "20",
    "--seed",
    "7",
];

const OFC: [&str; 11] = [
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
];

fn with(base: &[&str], extra: &[&str]) -> Vec<String> {
    base.iter().chain(extra).map(|s| s.to_string()).collect()
}

fn as_args(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

/// A scratch log path unique to this process and case.
fn log_path(case: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "poker-arena-cli-{}-{case}.jsonl",
        std::process::id()
    ))
}

fn seed_line_on_stderr(stderr: &str) {
    assert!(
        stderr.lines().any(|line| line.starts_with("seed: 7")),
        "expected a seed line on stderr, got: {stderr}"
    );
}

#[test]
fn betting_human_output_exits_clean_with_report_tables_and_a_stderr_seed_line() {
    let (stdout, stderr) = ok(&BETTING);

    assert!(stdout.contains("seed: 7"), "{stdout}");
    // Results table header, then the behavioral profile table's.
    for column in [
        "bot",
        "hands",
        "total chips",
        "bb/100 (±ci95)",
        "faults",
        "vpip",
        "pfr",
        "wtsd",
        "fold",
    ] {
        assert!(
            stdout.contains(column),
            "missing column {column:?}: {stdout}"
        );
    }
    assert!(stdout.contains("caller"), "{stdout}");
    assert!(stdout.contains("random"), "{stdout}");

    seed_line_on_stderr(&stderr);
}

#[test]
fn betting_json_output_parses_as_a_valid_match_report() {
    let args = with(&BETTING, &["--output", "json"]);
    let (stdout, stderr) = ok(&as_args(&args));

    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one JSON document on stdout: {stdout}"
    );

    let report: MatchReport = serde_json::from_str(lines[0]).expect("valid MatchReport");
    assert_eq!(report.family, "betting");
    assert_eq!(report.game_id, "holdem-nl");
    assert_eq!(report.decks, 20);
    assert_eq!(report.seed, 7);
    assert_eq!(report.seat_count, 2);
    assert_eq!(report.fault_policy, "substitute");
    assert!(report.forfeited_by.is_none());
    assert_eq!(report.bots.len(), 2);
    assert_eq!(report.bots[0].name, "caller");
    assert_eq!(report.bots[1].name, "random");
    let total: i64 = report.bots.iter().map(|b| b.total_chips).sum();
    assert_eq!(total, 0, "chips must stay zero-sum");

    seed_line_on_stderr(&stderr);
}

#[test]
fn ofc_human_output_exits_clean_with_a_report_table_and_a_stderr_seed_line() {
    let (stdout, stderr) = ok(&OFC);

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

    seed_line_on_stderr(&stderr);
}

#[test]
fn ofc_json_output_parses_as_a_valid_ofc_match_report() {
    let args = with(&OFC, &["--output", "json"]);
    let (stdout, stderr) = ok(&as_args(&args));

    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one JSON document on stdout: {stdout}"
    );

    let report: OfcMatchReport = serde_json::from_str(lines[0]).expect("valid OfcMatchReport");
    assert_eq!(report.family, "ofc");
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

    seed_line_on_stderr(&stderr);
}

#[test]
fn games_lists_every_variant_of_both_families() {
    let (stdout, _) = ok(&["games"]);
    for id in poker_core::game::GameSpec::known_ids() {
        assert!(stdout.contains(id), "missing {id:?} in: {stdout}");
    }
    for id in ["ofc", "ofc-pineapple", "ofc-progressive", "ofc-27"] {
        assert!(stdout.contains(id), "missing {id:?} in: {stdout}");
    }
    let betting = stdout.lines().filter(|l| l.contains(" betting ")).count();
    let ofc = stdout.lines().filter(|l| l.contains(" ofc ")).count();
    assert_eq!(betting, 20, "{stdout}");
    assert_eq!(ofc, 4, "{stdout}");
}

#[test]
fn betting_only_flags_are_rejected_for_an_ofc_game() {
    for (flag, value) in [
        ("--sb", Some("25")),
        ("--bb", Some("50")),
        ("--stack-bb", Some("200")),
        ("--raise-cap", Some("4")),
        ("--dealing", Some("seeded")),
    ] {
        let mut extra = vec![flag];
        extra.extend(value);
        let args = with(&OFC, &extra);
        let output = arena(&as_args(&args));
        assert!(!output.status.success(), "{flag} should be rejected");
        let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
        assert!(
            stderr.contains(flag) && stderr.contains("ofc"),
            "error should name the flag and the game: {stderr}"
        );
    }
}

#[test]
fn log_top_keeps_the_k_biggest_hands_in_both_families() {
    for (base, case, key) in [
        (&BETTING, "betting", "top_pots"),
        (&OFC, "ofc", "top_swings"),
    ] {
        let path = log_path(case);
        let path_arg = path.display().to_string();
        let args = with(base, &["--log", &path_arg, "--log-top", "2"]);
        ok(&as_args(&args));

        let written = std::fs::read_to_string(&path).expect("log file written");
        let last: serde_json::Value = serde_json::from_str(
            written
                .lines()
                .rfind(|l| !l.trim().is_empty())
                .expect("a summary line"),
        )
        .expect("valid summary JSON");
        let summary = &last["log_summary"];
        assert_eq!(summary["hands_kept"], 2, "{case}: {summary}");
        assert_eq!(summary[key], 2, "{case}: {summary}");
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn an_unknown_builtin_lists_the_chosen_games_family() {
    let args = with(&BETTING, &[]);
    let mut args = as_args(&args);
    args[4] = "builtin:greedy";
    let output = arena(&args);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("folder/caller/shover/random"),
        "betting game must list betting builtins: {stderr}"
    );

    let args = with(&OFC, &[]);
    let mut args = as_args(&args);
    args[4] = "builtin:caller";
    let output = arena(&args);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("greedy/filler/random"),
        "OFC game must list OFC builtins: {stderr}"
    );
}

#[test]
fn an_unknown_game_points_at_the_games_listing() {
    let args = with(&BETTING, &[]);
    let mut args = as_args(&args);
    args[2] = "holdem-xl";
    let output = arena(&args);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("unknown game") && stderr.contains("poker-arena games"),
        "{stderr}"
    );
}
