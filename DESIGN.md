# poker-arena — Architecture

A Rust workspace where poker bots compete to determine which is better,
across nineteen variants, with statistically sound comparison. This document
describes how the system is put together; [KEY_DECISIONS.md](KEY_DECISIONS.md)
records *why* it is this way, [WIRE_PROTOCOL.md](WIRE_PROTOCOL.md) specifies
the bot protocol, and the rules contract itself lives as module documentation
on `crates/poker-core/src/game/state.rs` — code and contract travel together.

## Workspace

Four crates with a strict dependency direction: `wire ← core ← arena ← cli`.
The boundary rule: **wire owns the vocabulary a match is described in; core
answers "what are the rules of poker"; arena answers "how do we run a
competition".** Each prefix is reusable on its own — a Rust bot author
depends on wire alone (cards, actions, events, framing, zero rules engine);
a solver adds core to drive `HandState` for game-tree traversal.

```
crates/
  poker-wire/                  vocabulary + protocol; serde only, no networking
    src/
      card.rs                  Card, Rank, Suit (serialize as "As", "Td")
      action.rs                Action, LegalActions, BetBounds, DrawBounds
      event.rs                 Event stream + per-seat visibility filtering
      value.rs                 HandValue, HandClass as seen at showdown
      game.rs                  Stakes, BettingKind: the per-match parameters
      message.rs               versioned message types, both directions
      framing.rs               JSON-lines read/write over any Read/Write
  poker-core/                  pure rules, no I/O — reusable by solvers etc.
    src/
      deck.rs                  Deck: deterministic shuffling and scripted deals
      rng.rs                   in-crate xoshiro256** (frozen-stream promise)
      eval/                    high, A-5, 2-7, 8-or-better (+sixes-or-better),
                               badugi (aces low and high); encodings frozen
      game/
        spec.rs                GameSpec: data-driven variant descriptor + registry
        state.rs               HandState: the per-hand state machine
                               (module doc = the authoritative rules contract)
        pot.rs                 side-pot construction and qualifier-aware awards
  poker-arena/                 competition machinery (library)
    src/
      bot.rs                   Bot trait, ActionRequest, BotFault
      builtin.rs               Folder, Caller, Shover, Random baselines
      remote.rs                WireBot: the trait over TCP or subprocess stdio
      runner.rs                match loop, seating, faults, observations
      config.rs                MatchConfig, DealingMode, FaultPolicy
      stat.rs / behavior.rs    winnings ± Student-t CI; VPIP/PFR/AF/WTSD/…
      log.rs                   JSON-lines hand-history sink
      bin/wire-caller.rs       reference wire bot (used by integration tests)
  poker-arena-cli/             the `poker-arena` binary (clap)
```

- `poker-wire`: `serde`/`serde_json`/`thiserror` and nothing else. Serde is
  unconditional (a serialization crate has no business making serialization
  optional). `Event` is defined exactly once and is both `Serialize` and
  `Deserialize`, so the engine's event stream, the hand log, and what a bot
  parses are the same bytes by construction.
- `poker-core`: `poker-wire` + `thiserror`; re-exports wire's vocabulary at
  its own paths. The RNG is in-crate because seed → identical deals is a
  forever promise and external RNG crates don't guarantee stream stability
  (a frozen snapshot test enforces it).
- `poker-arena`: everything with I/O. **No async runtime** — poker is
  strictly turn-based, so blocking I/O plus a reader thread per wire bot
  (deadlines via `recv_timeout`) covers everything an executor would.

## The rules engine

A variant is *data*: `GameSpec` = seats + stakes + forced bets + betting
structure + a street list (each street a deal — hole/community/upcards/draw —
plus an optional betting round) + a showdown of one or two
`ShowdownSide { kind, usage }` halves. One engine interprets all nineteen
registered games; adding a game means writing a constructor in `spec.rs`,
not engine code.

`HandState` runs exactly one hand, pure and synchronous:

```rust
let (mut hand, events) = HandState::new(&spec, &stacks, button, hand_no, deck, rng)?;
while let Some(seat) = hand.to_act() {
    let legal = hand.legal_actions().unwrap();
    let more = hand.apply(choose(seat, &legal))?;   // events out
}
let nets = &hand.settlement().unwrap().nets;         // sums to zero
```

Everything observable flows through the `Event` stream (deals, posts,
actions, draws, showdowns, awards), redacted per observer with
`Event::redacted_for`. The full betting contract — min-raise laddering,
cumulative reopening, the fixed-limit additive model and half-bet rule, stud
bring-in/completion, draw phases and muck reshuffling, run-outs, refunds,
odd chips — is specified normatively in `state.rs`'s module documentation
and pinned by the test suite.

## The arena

`run_match` drives hands between `Bot` implementations (in-process or
`WireBot`-wrapped remote processes — the runner never distinguishes). Per
deck it draws a seeded-random seating arrangement; duplicate mode replays
the deck once per cyclic rotation of it, and the rotation-set mean is one
statistical observation (Student-t 95% CI, normalized in big blinds — small
bets for stud). The button always sits at seat 0; bots rotate. Faults
(illegal action, timeout, disconnect, garbage) are substituted with the
decision family's minimal legal action or forfeit the match, per policy,
and are always reported. Behavioral profiles (VPIP, PFR, AF, WTSD, W$SD,
fold rate) accumulate from the event stream.

## Wire protocol

JSON Lines over TCP or subprocess stdio; `WIRE_PROTOCOL.md` is the
specification and `examples/bot.py` the dependency-free reference client.
Message throughput is irrelevant (turn-based, ~10 messages/hand, localhost)
while bot-author friction is everything — JSONL means every language reads
a line and calls its JSON parser. `hello` carries the game id plus only
per-match parameters; events are the single source of truth for table
state; `act` carries a self-describing tagged decision plus the deadline.

## Testing spine

- Scripted-deck fixtures: exact expected events, awards, and nets per rule
  area, for every game family.
- Seeded property sweeps: termination, chip conservation, legality
  soundness (`apply` accepts exactly what `legal_actions` offered),
  determinism (same seed → identical event streams), no duplicated live
  cards.
- Frozen anchors: the RNG stream snapshot, the evaluator C(52,5) frequency
  sweep, pinned wire JSON lines, and [transcripts/](transcripts/) — curated,
  byte-reproducible hand histories for all nineteen games that double as a
  wire-compatibility oracle for refactors.
