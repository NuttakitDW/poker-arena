# poker-arena

A place for poker bots to compete to see which is better — a Rust library and
CLI supporting multiple poker variants, with statistically sound comparison
(seeded reproducible dealing, duplicate-deal variance reduction, 95%
confidence intervals).

See [DESIGN.md](DESIGN.md) for the architecture, and
[KEY_DECISIONS.md](KEY_DECISIONS.md) for the design decisions and the
reasoning behind them.

## Workspace

| Crate | Purpose |
|---|---|
| `poker-wire` | The shared vocabulary — `Card`, `Action`, `Event`, `Stakes`, `HandValue` — plus the versioned JSON-lines protocol messages and framing that carry them. Depends on nothing but serde, so a Rust bot client can link this crate and nothing else. |
| `poker-core` | Pure rules on top of that vocabulary: hand evaluators (high, A-5 low, 2-7 low, eight-or-better, badugi), data-driven `GameSpec` variants, side-pot engine, deck/shuffling, and the `HandState` per-hand state machine. No I/O; reusable by solvers and analysis tools. |
| `poker-arena` | Competition layer: `Bot` trait, builtin baseline bots, match runner with duplicate dealing, winnings statistics, hand-history log, and the `poker-arena` CLI. |

## Quick start

Run 10,000 duplicate-dealt decks of heads-up no-limit hold'em between two
builtin bots:

```sh
cargo run --release -p poker-arena-cli -- run \
  --game holdem-nl \
  --bot builtin:caller --bot builtin:random \
  --hands 10000 --seed 42 --progress-every 1000
```

List supported games:

```sh
cargo run --release -p poker-arena-cli -- games
```

Currently registered: `holdem-nl`, `holdem-fl`, `omaha-pl`, `omaha8-pl`,
`omaha8-fl`, `bigo-pl`, `stud-fl`, `stud8-fl`, `razz-fl`, `27td-fl`,
`a5td-fl`, `badugi-fl`, `5cd-nl`, `badacey-fl`, `badeucy-fl`, `archie-fl`,
`drawmaha-fl`, `drawmaha-27-fl`, `drawmaha-dugi-fl` — nineteen variants
spanning community-card (including five-card Big O), stud (bring-in,
upcards), draw (including the badacey / badeucy / archie split-pot
games), and drawmaha (board + draw hybrid) families, all expressed as
data over one rules engine. Curated example hand histories for every game
live in [transcripts/](transcripts/).

Match reports include a behavioral profile per bot (VPIP, PFR, aggression
factor, went-to/won-at-showdown, fold rate) alongside the winnings table.
For programmatic consumers — a website ranking bots, a script sweeping
configurations — `--output json` replaces the tables with a single JSON
document (`schema_version`-tagged; see `poker_arena::report::MatchReport`)
carrying the seed, config, and per-bot results; `--progress-json` (cadence via
`--progress-every N` decks and/or `--progress-secs S`) additionally
streams interim standings as JSON lines
on stderr — a live leaderboard with tightening confidence intervals — and
per-hand detail streams separately via `--log` as JSON lines. Selective
logging (`--log-sample`, `--log-top-pots`, `--log-faults`) keeps
rotation-set samples, the biggest pots, and fault evidence, written at
match end.

## Out-of-process bots (any language)

Bots can compete as separate processes speaking a JSON-lines protocol —
see [WIRE_PROTOCOL.md](WIRE_PROTOCOL.md) for the full v1 spec and
[examples/bot.py](examples/bot.py) for a dependency-free Python reference:

```sh
cargo run --release -p poker-arena-cli -- run \
  --game holdem-nl \
  --bot cmd:"python3 examples/bot.py" \
  --bot builtin:random \
  --timeout-ms 1000
```

`cmd:"COMMAND"` spawns the bot and talks over its stdio; `tcp:PORT` listens
on 127.0.0.1 and waits for the bot to connect. Per-action deadlines are
enforced server-side (`--timeout-ms`, 0 = none); timeouts, disconnects, and
malformed replies are faults handled by the configured fault policy.

## Writing a bot (in-process)

Implement the `Bot` trait from the `poker-arena` crate:

```rust
use poker_arena::bot::{ActionRequest, Bot, BotFault};
use poker_core::game::Action;

struct MyBot;

impl Bot for MyBot {
    fn name(&self) -> &str { "my-bot" }

    fn act(&mut self, req: &ActionRequest<'_>) -> Result<Action, BotFault> {
        // req.legal describes exactly what is allowed right now.
        Ok(if req.legal.check { Action::Check } else { Action::Fold })
    }
}
```

Every decision point hands you a self-contained `ActionRequest` (your cards,
the board, stacks, pot, and structured legal actions); an event stream keeps
stateful bots informed. Out-of-process bots connect in any language over
JSON lines (TCP or stdio) — see the section above.

## Fairness model

- **Deterministic dealing**: one RNG stream per deck derived from the match
  seed (in-crate xoshiro256**; the stream is frozen by a snapshot test, so a
  seed reproduces its deals forever).
- **Duplicate mode** (default): each deck draws a seeded-random seating
  arrangement and is replayed once per rotation of it, so every bot plays
  the same cards from every position while neighbor arrangements average
  out across decks; a rotation-set is one statistical observation, which
  removes most card luck from the comparison.
- **Faults**: illegal actions, timeouts, and disconnects are never silently
  patched — they count against the bot and are substituted with the
  decision's minimal legal action (check/fold, stand pat, bring-in) or
  forfeit the match, per configuration.

## Status

Feature-complete for its current scope: nineteen variants across four game
families over one data-driven rules engine, in-process and wire bots with
deadlines and fault policies, duplicate-deal variance reduction with
Student-t confidence intervals, behavioral profiling, deterministic replay
from a seed, and JSON-lines hand histories.

All engine rules are covered by scripted-hand fixtures and seeded property
tests (chip conservation, legality soundness, determinism).
