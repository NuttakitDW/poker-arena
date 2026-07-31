# Working in this repository

poker-arena is a poker bot competition arena: nineteen variants over one
data-driven rules engine, in-process and wire bots, statistically honest
match results. This file is for AI agents (and humans) making changes.

## Navigation

| Where | What |
|---|---|
| `crates/poker-wire/` | The vocabulary: Card, Action, Event, Stakes, messages, JSON-lines framing. Depends on serde only — a bot client needs this crate alone. |
| `crates/poker-core/` | Pure rules, no I/O: deck + deterministic RNG, evaluators, `GameSpec` registry (`game/spec.rs`), pot engine, `HandState`. Re-exports wire's vocabulary at its own paths. |
| `crates/poker-arena/` | Competition machinery: `Bot` trait, builtins, `WireBot` transports, match runner, stats, hand log. |
| `crates/poker-arena-cli/` | The `poker-arena` binary. |
| `WIRE_PROTOCOL.md` | The bot protocol spec. This doc and `poker-wire` must never drift apart. |
| `KEY_DECISIONS.md` | Why things are the way they are. **Maintained**: edit the relevant entry whenever you make or revisit a design decision. |
| `transcripts/` | Curated, byte-reproducible hand histories for every game, with regeneration commands in its README. |

The authoritative rules contract (betting, reopening, fixed-limit model,
stud, draws, showdown, settlement) is the module documentation at the top
of `crates/poker-core/src/game/state.rs`. Read it before touching the
engine; update it in the same change as any rules edit.

## Invariants — do not break

- **Determinism is a forever promise.** Same seed → byte-identical match.
  The RNG is in-crate with a frozen stream-snapshot test; never "fix" that
  test's constants, and never introduce nondeterminism (no HashMap
  iteration order in outputs, no time/random in the engine).
- **Wire bytes are pinned** by exact-JSON tests in `poker-wire` and by the
  committed `transcripts/*.jsonl`. A refactor that changes serde output is
  wrong until proven otherwise; verify suspicious changes by regenerating a
  transcript source match (commands in `transcripts/README.md`) and
  diffing.
- **`HandValue` encodings and evaluator semantics are frozen** (see
  `eval/mod.rs` docs); the C(52,5) frequency sweep and ordering fixtures
  pin them.
- **Chip conservation, legality soundness, termination** are
  property-tested; new engine paths need the same treatment.

## Conventions

- Singular file names: `card.rs`, `event.rs`, `pot.rs` — never plurals.
- No milestone/phase labels in shipped docs or comments; docs describe the
  present. History lives in git; rationale lives in `KEY_DECISIONS.md`.
- Comments state constraints the code can't show — not narration, not
  change history.
- Every checkpoint: `cargo test --workspace --all-features` green,
  `cargo clippy --workspace --all-features --tests` warning-free,
  `cargo fmt --all` applied. Commit only reviewed, compiling states.
  Remote is `origin`, branch `master`.

## Common tasks

- **Run a match**: `cargo run --release -p poker-arena-cli -- run --game
  holdem-nl --bot builtin:caller --bot builtin:random --hands 1000`
  (`--seed N` to reproduce; seed is always printed). `poker-arena games`
  lists the registry.
- **Add a game variant**: constructor + registry entry in
  `poker-core/src/game/spec.rs` (variants are data — if the engine needs
  new code, stop and reconsider the design, then update the `state.rs`
  contract first); scripted-hand tests for its rules edges; runner smoke in
  `poker-arena/tests/game.rs`; curated transcript + README section; counts
  in `README.md`/`DESIGN.md`.
- **Change the protocol**: update `poker-wire`, `WIRE_PROTOCOL.md`, both
  reference bots (`examples/bot.py`, `src/bin/wire-caller.rs`), and the
  pinned-JSON tests together. The protocol is v1 until first release —
  breaking changes stay v1.
- **Test a wire bot end-to-end**: `--bot cmd:"python3 examples/bot.py"`
  runs the reference client over stdio; zero faults is the bar.
