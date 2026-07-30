# poker-arena

A place for poker bots to compete to see which is better — a Rust library and
CLI supporting multiple poker variants, with statistically sound comparison
(seeded reproducible dealing, duplicate-deal variance reduction, 95%
confidence intervals).

See [DESIGN.md](DESIGN.md) for the full architecture and roadmap.

## Workspace

| Crate | Purpose |
|---|---|
| `poker-core` | Pure rules: cards, hand evaluators (high, A-5 low, 2-7 low, eight-or-better, badugi), data-driven `GameSpec` variants, side-pot engine, and the `HandState` per-hand state machine. No I/O; reusable by solvers and analysis tools. |
| `poker-wire` | Versioned JSON-lines wire-protocol definitions (M2). |
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
`omaha8-fl` (stud and draw families arrive in M3 — the engine's variant
model already covers them).

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
stateful bots informed. Out-of-process bots (any language, JSON lines over
TCP or stdio) arrive with the wire protocol in M2.

## Fairness model

- **Deterministic dealing**: one RNG stream per deck derived from the match
  seed (in-crate xoshiro256**; the stream is frozen by a snapshot test, so a
  seed reproduces its deals forever).
- **Duplicate mode** (default): every deck is replayed once per seat
  rotation, so each bot plays the same cards from every position; a
  rotation-set is one statistical observation, which removes most card luck
  from the comparison.
- **Faults**: illegal actions are never silently patched — they count
  against the bot and are substituted (check/fold) or forfeit the match,
  per configuration.

## Status

- **M1 — done.** Heads-up ↔ 9-max hold'em (NL/FL), builtin bots, duplicate
  dealing, BB/100 ± 95% CI, CLI, hand-history logs.
- **M2 — done.** Wire protocol v1 (TCP + subprocess stdio, any language),
  per-action deadlines and fault handling, Omaha / Omaha hi-lo with
  pot-limit betting.
- **M3 — next.** Stud family (stud, stud8, razz), draw family (triple draw,
  badugi), behavioral stats (VPIP, aggression, showdown%).

All engine rules are covered by scripted-hand fixtures and seeded property
tests (chip conservation, legality soundness, determinism).
