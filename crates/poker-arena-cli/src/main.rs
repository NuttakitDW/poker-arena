//! `poker-arena` CLI — list game variants and run matches between bots.
//!
//! One binary over two engines. `--game` is resolved against the betting
//! registry first and the OFC registry second; the family it lands in picks
//! the driver ([`bet`] or [`ofc`]) and, with it, the bot vocabulary, the
//! report tables and which flags are meaningful. The flag set is the union
//! of both families', so the betting-only flags are validated at runtime
//! rather than by clap.

mod bet;
mod ofc;
mod spec;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use poker_arena::config::{DealingMode, FaultPolicy};
use poker_core::game::{BettingKind, GameSpec, Stakes};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Games => {
            print_games();
            ExitCode::SUCCESS
        }
        Command::Run(args) => match dispatch(*args) {
            Ok(code) => code,
            Err(msg) => {
                eprintln!("error: {msg}");
                ExitCode::from(1)
            }
        },
    }
}

#[derive(Parser)]
#[command(name = "poker-arena", version, about = "Poker bot competition arena")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List known game variants, betting and OFC alike.
    Games,
    /// Run a match between bots.
    Run(Box<RunArgs>),
}

#[derive(clap::Args)]
struct RunArgs {
    /// Game variant id, betting or OFC (see `poker-arena games`).
    #[arg(long)]
    game: String,

    /// A competitor: `builtin:NAME` (betting games:
    /// folder|caller|shover|random[:seed]; OFC games:
    /// greedy|filler|random[:seed]) | `tcp:PORT` (wait for a bot to connect
    /// on 127.0.0.1:PORT) | `cmd:COMMAND` (spawn a bot and talk over its
    /// stdio). Prefix any spec with `NAME@` to assign the competition name
    /// (`alice@cmd:python3 bot.py`); bots carry no identity of their own.
    /// Unnamed bots default to the builtin kind, `tcp-PORT`, or `bot-N`.
    /// Repeat once per seat (count must fit the game's seat range).
    #[arg(long = "bot")]
    bots: Vec<String>,

    /// Hands to play (betting games: decks, one hand per deck in seeded
    /// mode or one per seat rotation in duplicate mode). Defaults to 10000
    /// for betting games, 1000 for OFC.
    #[arg(long)]
    hands: Option<u64>,

    /// RNG seed for dealing; the whole match is reproducible from this seed
    /// given deterministic bots. Defaults to a fresh random seed. Always
    /// printed to stderr.
    #[arg(long)]
    seed: Option<u64>,

    /// Per-decision deadline in milliseconds (0 = no deadline). Enforced as
    /// a hard deadline for wire bots; in-process bots are not preemptible.
    /// Defaults to 1000 for betting games, 5000 for OFC.
    #[arg(long)]
    timeout_ms: Option<u64>,

    /// What happens when a bot returns a non-conforming action.
    #[arg(long, value_enum, default_value = "substitute")]
    fault_policy: FaultPolicyArg,

    /// Write the unredacted hand history as JSON lines to this file. By
    /// default every hand is written as it's played. If any of
    /// --log-sample / --log-top / --log-faults is given, selective mode
    /// kicks in instead: only the chosen hands are kept, and the whole file
    /// is written at once when the match ends (nothing appears until then).
    #[arg(long)]
    log: Option<PathBuf>,

    /// Selective log: keep the first N hands (betting games extend this to
    /// whole decks so a duplicate rotation set is never split). N >= 1.
    /// Requires --log.
    #[arg(long)]
    log_sample: Option<u64>,

    /// Selective log: keep the K biggest hands over the whole match (global
    /// top K) — by pot size for betting games, by largest absolute point
    /// swing for OFC. Requires --log.
    #[arg(long)]
    log_top: Option<usize>,

    /// Selective log: cap on hands kept as fault evidence (the first K
    /// hands any bot faulted in); forfeited hands are always kept
    /// regardless of this cap. Only meaningful once selective mode is on
    /// (--log-sample and/or --log-top, or this flag itself); defaults to
    /// 100 once selective mode is on. Requires --log.
    #[arg(long)]
    log_faults: Option<u64>,

    /// Print progress to stderr every N decks (betting) or N hands (OFC).
    /// 0 = off.
    #[arg(long, default_value_t = 0)]
    progress_every: u64,

    /// Emit progress at most every S seconds (fractional ok; 0 = off).
    /// Combines with --progress-every: either trigger emits.
    #[arg(long, default_value_t = 0.0)]
    progress_secs: f64,

    /// Emit progress as JSON lines (interim standings incl. per-bot rate
    /// and CI) on stderr instead of the human progress line. Requires a
    /// cadence: --progress-every and/or --progress-secs.
    #[arg(long, default_value_t = false)]
    progress_json: bool,

    /// Result format on stdout: aligned human tables, or a single JSON
    /// document (see poker_arena::report::match_report /
    /// poker_arena::ofc::report::ofc_match_report) for programmatic
    /// consumers.
    #[arg(long, value_enum, default_value = "human")]
    output: OutputArg,

    /// Betting games only. Small blind, in chips. Default 50.
    #[arg(long)]
    sb: Option<u64>,

    /// Betting games only. Big blind, in chips. Default 100.
    #[arg(long)]
    bb: Option<u64>,

    /// Betting games only. Per-player ante, in chips. Any betting game:
    /// blind games default to no ante; stud games default to bb/5 (min 1).
    #[arg(long)]
    ante: Option<u64>,

    /// Betting games only. Forced bring-in, in chips. Stud games only;
    /// defaults to bb/2 (min 1). Passing this for a non-stud game is an
    /// error.
    #[arg(long)]
    bring_in: Option<u64>,

    /// Betting games only. Small bet tier, in chips. Stud games only;
    /// defaults to bb. Passing this for a non-stud game is an error.
    #[arg(long)]
    small_bet: Option<u64>,

    /// Betting games only. Big bet tier, in chips. Stud games only;
    /// defaults to 2 × small bet. Passing this for a non-stud game is an
    /// error.
    #[arg(long)]
    big_bet: Option<u64>,

    /// Betting games only. Raise cap for fixed-limit games (max total
    /// wagers per betting round, including the opening bet). 0 = uncapped.
    /// Passing this for a non-fixed-limit game is an error.
    #[arg(long)]
    raise_cap: Option<u8>,

    /// Betting games only. Starting stack, in big blinds (reset every
    /// hand). Default 100.
    #[arg(long)]
    stack_bb: Option<u64>,

    /// Betting games only. How decks are dealt across seat rotations.
    /// Default duplicate.
    #[arg(long, value_enum)]
    dealing: Option<DealingArg>,
}

impl RunArgs {
    /// True once any selective-log flag is set: the log keeps chosen hands
    /// only, and is written when the match ends.
    fn selective(&self) -> bool {
        self.log_sample.is_some() || self.log_top.is_some() || self.log_faults.is_some()
    }

    fn check_log(&self) -> Result<(), String> {
        if self.selective() && self.log.is_none() {
            return Err("--log-sample/--log-top/--log-faults require --log FILE".to_string());
        }
        if self.log_sample == Some(0) {
            return Err("--log-sample must be >= 1".to_string());
        }
        Ok(())
    }

    /// Validates the progress flags and reports whether a cadence is set at
    /// all (no cadence = no progress callback).
    fn check_progress(&self) -> Result<bool, String> {
        if self.progress_secs < 0.0 || !self.progress_secs.is_finite() {
            return Err("--progress-secs must be a finite number >= 0".to_string());
        }
        let cadence_set = self.progress_every > 0 || self.progress_secs > 0.0;
        if self.progress_json && !cadence_set {
            return Err(
                "--progress-json requires a cadence: --progress-every and/or --progress-secs"
                    .to_string(),
            );
        }
        Ok(cadence_set)
    }

    /// Rejects the betting-only flags for an OFC game. They are `Option` in
    /// the clap surface precisely so "was it passed" is answerable here;
    /// their defaults are applied inside the betting driver.
    fn reject_betting_flags(&self, game: &str) -> Result<(), String> {
        const NO_STAKES: &str = "OFC games have no stakes";
        for (given, flag, why) in [
            (self.sb.is_some(), "--sb", NO_STAKES),
            (self.bb.is_some(), "--bb", NO_STAKES),
            (self.ante.is_some(), "--ante", NO_STAKES),
            (self.bring_in.is_some(), "--bring-in", NO_STAKES),
            (self.small_bet.is_some(), "--small-bet", NO_STAKES),
            (self.big_bet.is_some(), "--big-bet", NO_STAKES),
            (
                self.raise_cap.is_some(),
                "--raise-cap",
                "OFC has no betting",
            ),
            (self.stack_bb.is_some(), "--stack-bb", "OFC has no chips"),
            (
                self.dealing.is_some(),
                "--dealing",
                "OFC deals one hand per deck",
            ),
        ] {
            if given {
                return Err(format!("{flag} is not valid for {game} ({why})"));
            }
        }
        Ok(())
    }
}

#[derive(Copy, Clone, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum DealingArg {
    Seeded,
    Duplicate,
}

impl From<DealingArg> for DealingMode {
    fn from(a: DealingArg) -> Self {
        match a {
            DealingArg::Seeded => DealingMode::Seeded,
            DealingArg::Duplicate => DealingMode::Duplicate,
        }
    }
}

#[derive(Copy, Clone, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum OutputArg {
    Human,
    Json,
}

#[derive(Copy, Clone, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum FaultPolicyArg {
    Substitute,
    Forfeit,
}

impl From<FaultPolicyArg> for FaultPolicy {
    fn from(a: FaultPolicyArg) -> Self {
        match a {
            FaultPolicyArg::Substitute => FaultPolicy::Substitute,
            FaultPolicyArg::Forfeit => FaultPolicy::Forfeit,
        }
    }
}

/// Stakes used only to instantiate a betting spec for lookup and listing;
/// the betting driver rebuilds the spec with the operator's real stakes.
const PROBE_STAKES: Stakes = Stakes::Blinds {
    small_blind: 50,
    big_blind: 100,
    ante: 0,
};

/// Routes `run` to the family the game id belongs to. The two registries
/// have disjoint ids; betting is tried first.
fn dispatch(args: RunArgs) -> Result<ExitCode, String> {
    if GameSpec::by_id(&args.game, PROBE_STAKES).is_some() {
        return bet::run(args);
    }
    if let Some(spec) = poker_core::ofc::find(&args.game) {
        args.reject_betting_flags(spec.id)?;
        return ofc::run(args, *spec);
    }
    Err(format!(
        "unknown game {:?} (see `poker-arena games`)",
        args.game
    ))
}

/// One listing for both registries: the family column is what tells an
/// operator which flags and which `builtin:` names a row accepts.
fn print_games() {
    let row = |id: &str, family: &str, name: &str, seats: &str, structure: &str| {
        println!("{id:<17} {family:<8} {name:<38} {seats:<6} {structure}");
    };
    row("game", "family", "name", "seats", "structure");
    for id in GameSpec::known_ids() {
        let spec =
            GameSpec::by_id(id, PROBE_STAKES).expect("known_ids() entries must resolve via by_id");
        let betting = match spec.betting {
            BettingKind::NoLimit => "no-limit".to_string(),
            BettingKind::PotLimit => "pot-limit".to_string(),
            BettingKind::FixedLimit { raise_cap: Some(n) } => format!("fixed-limit (cap {n})"),
            BettingKind::FixedLimit { raise_cap: None } => "fixed-limit (uncapped)".to_string(),
        };
        row(
            spec.id,
            "betting",
            spec.display_name,
            &format!("{}-{}", spec.seats.start(), spec.seats.end()),
            &format!("{betting}, {} streets", spec.streets.len()),
        );
    }
    for spec in poker_core::ofc::registry() {
        row(
            spec.id,
            "ofc",
            spec.name,
            &format!("{}-{}", spec.min_seats, spec.max_seats),
            &format!(
                "deal {} then {}x(deal {}, place {})",
                spec.initial_deal, spec.rounds, spec.round_deal, spec.round_place
            ),
        );
    }
}
