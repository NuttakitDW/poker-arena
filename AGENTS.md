# Working in this repository

poker-arena is a poker bot competition arena: twenty betting variants over
one data-driven rules engine plus four Open Face Chinese variants over a
second points-based engine, in-process and wire bots, statistically honest
match results. This file is for AI agents (and humans) making changes.

## Navigation

| Where | What |
|---|---|
| `crates/poker-wire/` | The vocabulary: Card, Action, Event, Stakes, messages, JSON-lines framing. Depends on serde only — a bot client needs this crate alone. `src/ofc/` is the OFC protocol's vocabulary (separate messages/events/reports, versioned independently). |
| `crates/poker-core/` | Pure rules, no I/O: deck + deterministic RNG, evaluators, `GameSpec` registry (`game/spec.rs`), pot engine, `HandState`. Re-exports wire's vocabulary at its own paths. `src/ofc/` is the OFC engine: `OfcSpec` registry (`ofc/spec.rs`), `Board`, `OfcHandState`, points scoring. |
| `crates/poker-arena/` | Competition machinery: `Bot` trait, builtins, `WireBot` transports, match runner, stats, hand log. `src/transport.rs` is the generic JSONL peer both wire adapters share; `src/ofc/` mirrors the whole layer for OFC (incl. the `greedy` foul-avoiding builtin). |
| `crates/poker-arena-cli/` | Two binaries: `poker-arena` (src/main.rs) and `poker-arena-ofc` (src/ofc.rs); shared operator helpers in src/lib.rs. |
| `WIRE_PROTOCOL.md` | The betting bot protocol spec. This doc and `poker-wire` must never drift apart. |
| `WIRE_PROTOCOL_OFC.md` | The OFC bot protocol spec, same never-drift rule against `poker_wire::ofc`. |
| `KEY_DECISIONS.md` | Why things are the way they are. **Maintained**: edit the relevant entry whenever you make or revisit a design decision. |
| `transcripts/` | Curated, byte-reproducible hand histories for every game, with regeneration commands in its README. |

The authoritative rules contract (betting, reopening, fixed-limit model,
stud, draws, showdown, settlement) is the module documentation at the top
of `crates/poker-core/src/game/state.rs`; the OFC rules contract (rows,
placement legality, fouling, royalties, pairwise scoring, fantasyland)
lives the same way on `crates/poker-core/src/ofc/state.rs`. Read the
relevant one before touching its engine; update it in the same change as
any rules edit.

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
- **Run an OFC match**: `cargo run --release -p poker-arena-cli --bin
  poker-arena-ofc -- run --game ofc-pineapple --bot builtin:greedy --bot
  builtin:random --hands 1000`. Same operator model (NAME@spec, seeds,
  JSON output); points instead of chips; no duplicate mode.
- **Add a game variant**: constructor + registry entry in
  `poker-core/src/game/spec.rs` (variants are data — if the engine needs
  new code, stop and reconsider the design, then update the `state.rs`
  contract first); scripted-hand tests for its rules edges; runner smoke in
  `poker-arena/tests/game.rs`; curated transcript + README section; counts
  in `README.md`/`DESIGN.md`.
- **Add an OFC variant**: constructor + registry entry in
  `poker-core/src/ofc/spec.rs` (variants are data; the seat cap must keep
  `max_seats × cards_per_seat ≤ 52` — the deck is never reshuffled);
  update the `ofc/state.rs` contract if rules change; scripted tests in
  `poker-core/tests/ofc.rs`; runner smoke; transcript; doc counts.
- **Change a protocol**: update `poker-wire` (or its `ofc` module), the
  matching spec doc (`WIRE_PROTOCOL.md` / `WIRE_PROTOCOL_OFC.md`), the
  reference bots (`examples/bot.py` + `src/bin/wire-caller.rs`, or
  `examples/ofc_bot.py` + `src/bin/wire-placer.rs`), and the pinned-JSON
  tests together. Both protocols are v1 until first release — breaking
  changes stay v1.
- **Benchmarks**: `cargo bench --workspace` (Criterion; eval + engine
  micro-benches in `poker-core/benches/`, match throughput in
  `poker-arena/benches/`). On-demand only — no CI gating; baselines live
  in each bench file's module doc as review-time drift checks. Re-run and
  update them after touching the engine hot path or the evaluators.
- **Test a wire bot end-to-end**: `--bot cmd:"python3 examples/bot.py"`
  runs the reference client over stdio; zero faults is the bar.
