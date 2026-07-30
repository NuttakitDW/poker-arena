# poker-arena — Design

A Rust library and CLI binary where poker bots compete to determine which is
better, across many poker variants, with statistically sound comparison.

## 1. Goals

- **Library crates**: `poker-core` (deterministic poker rules engine covering
  many variants), `poker-wire` (protocol definitions), `poker-arena` (bot
  interface, match orchestration, statistics).
- **Binary** (`poker-arena`): CLI match runner — configure a game, connect
  bots, run N hands, report who is better with confidence intervals.
- Bots connect **in-process** (Rust trait) or **out-of-process** (TCP or
  spawned subprocess over stdio, JSON-lines wire protocol). The wire adapter
  is implemented *on top of* the trait so both modes share one code path.
- Fair comparison: seeded reproducible dealing, optional **duplicate dealing**
  (seat rotation over identical decks), configurable fault policies.

### Non-goals (for now)

- Tournaments (blind escalation, eliminations, table balancing).
- Elo/cross-match leaderboards, persistent server, web UI.
- Real-money anything; GUI play.

## 2. Requirements (agreed)

| Area | Decision |
|---|---|
| Variants | Hold'em, Omaha (+ hi-lo 8ob), Stud family (stud, stud8, razz), Draw family (2-7 TD, A-5 TD, five-card draw, badugi) |
| Betting | Fixed-limit, pot-limit, no-limit; stacks, all-ins, side pots |
| Table size | 2–9 seats |
| Bot interface | Rust trait core; TCP + stdio subprocess wire adapter |
| Variance reduction | Seeded RNG always; duplicate dealing (seat rotation) as a configurable mode |
| Fault handling | Configurable: fold/check policy (default), forfeit |
| Stats | Net winnings ± 95% CI, per-hand action log, behavioral breakdown (VPIP, aggression, showdown%) |
| Binary | CLI match runner |

## 3. Workspace layout

Three crates with a strict dependency direction: `core ← wire ← arena` and
`core ← arena`. The boundary rule: **core answers "what are the rules of
poker"; wire answers "how are messages encoded"; arena answers "how do we run
a competition".** Core (and core + wire) are reusable by other tools — a
solver can drive `HandState` for game-tree traversal; a Rust bot author can
depend on core + wire to build a client with zero arena machinery.

```
poker-arena/                     workspace root
  Cargo.toml
  crates/
    poker-core/                  pure rules, no I/O — reusable by solvers etc.
      src/
        lib.rs
        card.rs                 Card, Rank, Suit, Deck, deterministic shuffling
        eval/
          mod.rs                 HandValue, Evaluator dispatch
          high.rs                standard high hands (5 of N)
          low.rs                 A-5 lowball, 2-7 lowball, 8-or-better qualifier
          badugi.rs
        game/
          spec.rs                GameSpec: data-driven variant descriptor + registry
          state.rs               HandState: the per-hand state machine
                                 (incl. limits, min-raise, pot-limit math)
          action.rs              Action, LegalActions
          pot.rs                 side-pot construction and awarding
          event.rs               Event stream + per-seat visibility filtering
    poker-wire/                  protocol definitions + framing, no networking
      src/
        lib.rs
        message.rs              versioned message types (serde), both directions
        framing.rs               JSON-lines read/write over any Read/Write
    poker-arena/                 competition machinery; lib + `poker-arena` binary
      src/
        lib.rs
        bot/
          mod.rs                 Bot trait, PlayerView / ActionRequest
          builtin.rs             baseline bots: Folder, Caller, Random, …
        remote.rs                WireBot: Bot impl over TCP socket / subprocess stdio
        arena/
          config.rs              MatchConfig, DealingMode, FaultPolicy
          runner.rs              match loop, duplicate rotation, timing, faults
          log.rs                 hand-history writer
        stats/
          winning.rs            per-hand net, Student-t 95% CI, duplicate aggregation
          behavior.rs            VPIP, PFR, aggression factor, showdown/fold rates
        main.rs                  CLI (clap)
```

- `poker-core`: depends only on `thiserror`. The deck RNG (xoshiro256** +
  splitmix64) is implemented in-crate (`rng.rs`) because external RNG crates
  don't guarantee cross-version stream stability and seed → identical deals
  is a forever promise (enforced by a frozen snapshot test). `serde` derives
  live behind an optional `serde` feature so solver-style consumers pay
  nothing for them.
- `poker-wire`: `serde`/`serde_json` + `poker-core` (with `serde` feature).
  Defines and documents the message schema and the framing contract (exactly
  one JSON object per `\n`-terminated line). Transport-agnostic: works over
  any `Read`/`Write`, so it owns no sockets.
- `poker-arena`: everything else — `clap`, networking, subprocess spawning,
  stats. **No async runtime** — the game is strictly turn-based, so blocking
  I/O with deadlines is simpler and sufficient; parallelism (if ever needed)
  is per-match threads.

### Why JSON Lines for the wire format

Message throughput is irrelevant (turn-based, ~10 messages/hand, localhost)
while bot-author friction is everything. JSONL means every language reads a
line and calls its JSON parser — a working Python bot is ~20 lines, no
dependencies, no codegen. Binary formats (protobuf/msgpack/CBOR) optimize
bytes that don't matter and add schema tooling for every bot author;
ACPC-style compact text is fiddly to parse and gets ugly spanning draw
actions, upcards, and hi-lo showdowns. Debuggability is a feature: `nc` into
a match, read logs by eye. Discipline required in exchange: strict one-object-
per-line framing, a `proto` version field, unknown fields ignored, and a
human-written schema document so non-Rust authors never reverse-engineer
serde output.

## 4. Core model

### 4.1 Cards and evaluation

- `Card` = rank × suit, compact `u8` representation, `"As"`-style parsing and
  display.
- `Deck::shuffled(seed, hand_no)` — one RNG stream per hand derived from the
  match seed, so hand K is reproducible in isolation.
- Evaluators return a totally ordered `HandValue` (higher = better for the pot
  being contested — low evaluators invert internally so comparison is uniform):
  - `High` — standard poker high hands, best 5 of N.
  - `AceToFiveLow`, `DeuceToSevenLow` — lowball orderings.
  - `EightOrBetter` — A-5 low with qualifier, returns `Option<HandValue>`.
  - `Badugi` — 1–4 card rainbow-distinct-rank hands.
- Correctness-first implementation: enumerate 5-card combinations (≤ C(7,5)=21
  per hand) over a fast 5-card ranker. This is orders of magnitude faster than
  any bot will act; no lookup-table heroics needed initially.
- Hole-card usage constraints live in the showdown spec, not the evaluator:
  `Any` (hold'em/stud), `ExactlyTwo` (omaha), `AllOwn` (draw games).

### 4.2 GameSpec — data-driven variants

Poker variants decompose into orthogonal axes; a variant is *data*, not code:

```rust
pub struct GameSpec {
    pub name: &'static str,
    pub seats: RangeInclusive<u8>,
    pub deck: DeckType,               // Standard52 (room for Short-deck later)
    pub forced_bets: ForcedBets,      // BlindsAndAntes | AntesAndBringIn
    pub betting: BettingKind,         // FixedLimit | PotLimit | NoLimit
    pub streets: Vec<StreetSpec>,
    pub showdown: ShowdownSpec,
}

pub struct StreetSpec {
    pub deal: DealSpec,        // HolePrivate(n) | HoleUp(n) | Community(n) | Draw{max}
    pub betting: BetRound,     // bet tier (small/big for limit), first-to-act rule
}

pub enum FirstToAct { LeftOfButton, ByUpcards }   // stud streets use upcards
pub struct ShowdownSpec {
    pub pots: PotSplit,        // Hi | HiLo { low: Evaluator, qualifier } | LowOnly
    pub hole_usage: HoleUsage, // Any | ExactlyTwo | AllOwn
}
```

Stud quirks (bring-in by lowest upcard, first-to-act by best showing hand,
open-pair double-bet option) are enum-encoded hooks the engine interprets —
still data-driven, no per-variant `impl`. A registry maps CLI identifiers
(`holdem-nl`, `holdem-fl`, `omaha-pl`, `omaha8-fl`, `stud`, `stud8`, `razz`,
`27td`, `a5td`, `5cd-nl`, `badugi`, …) to specs.

### 4.3 HandState — the per-hand state machine

Pure and synchronous; the arena layer owns all I/O and timing.

```rust
impl HandState {
    pub fn new(spec: &GameSpec, stacks: &[Chips], button: Seat, deck: Deck) -> (Self, Vec<Event>);
    pub fn to_act(&self) -> Option<Seat>;                 // None => auto-advance or done
    pub fn legal_actions(&self) -> LegalActions;
    pub fn apply(&mut self, action: Action) -> Result<Vec<Event>, RuleError>;
    pub fn is_over(&self) -> bool;
    pub fn settlement(&self) -> &Settlement;              // per-seat net, pot breakdown
}
```

- `Action`: `Fold | Check | Call | Bet(Chips) | Raise(Chips /*to*/) | Discard(Vec<Card>) | BringIn`.
- `LegalActions` is structured, not a flat list: `{ fold: bool, check_call: Option<Chips>, raise: Option<RaiseBounds> , draw: Option<DrawBounds> }` —
  for fixed-limit `RaiseBounds.min == max`; for NL it encodes min-raise…all-in.
- All-in and side pots are core (not an NL afterthought): `pot.rs` builds
  side pots from contribution levels, awards per `ShowdownSpec` with odd-chip
  rules (first seat left of button).
- `Event` is the single source of truth for everything observable: deals,
  posts, actions, draws, showdowns, pot awards. Each event carries visibility
  (`Public | Private(Seat)`); bots and logs consume the same stream, filtered.
- Stacks reset every hand (configurable depth, default 100 BB for big-bet
  games) — bot comparison measures per-hand EV, not bankroll trajectories.

**Testing spine:** the pure state machine is exercised by (a) unit tests per
rule area, (b) scripted hand fixtures (deck + action script → exact expected
events/settlement), (c) property tests: chip conservation, legal-action
soundness (`apply` accepts exactly what `legal_actions` offered), termination.

## 5. Bot interface

```rust
pub trait Bot: Send {
    fn name(&self) -> &str;
    fn on_hand_start(&mut self, info: &HandStart) {}
    fn on_event(&mut self, event: &VisibleEvent) {}       // filtered to this seat
    fn act(&mut self, req: &ActionRequest) -> Action;
    fn on_hand_end(&mut self, result: &HandResult) {}
}
```

`ActionRequest` contains the full seat-visible view (own cards, board/upcards,
pot(s), stacks, bets this street, legal actions, hand history so far) so bots
can be stateless if they want.

Built-in baselines ship with the library: `Folder`, `Caller` (check/call),
`Random` (uniform over legal actions), and a simple value bot per family —
essential as opponents, smoke tests, and calibration anchors.

## 6. Wire protocol

JSON Lines (one JSON object per `\n`-terminated line), versioned, over TCP
(`--bot tcp:PORT` — arena listens, bot connects) or a spawned subprocess's
stdio (`--bot cmd:"python mybot.py"`). `WireBot` implements `Bot` by
serializing the same `VisibleEvent`/`ActionRequest` types — one semantics,
two transports.

```jsonc
// arena → bot
{"t":"hello","proto":1,"game":"holdem-nl","seats":2,"stack":20000,"blinds":[50,100]}
{"t":"hand","no":17,"seat":1,"button":0}
{"t":"ev", ...}                                  // visible events as they happen
{"t":"act","legal":{...},"deadline_ms":1000}
// bot → arena
{"t":"join","name":"my-bot"}
{"t":"action","kind":"raise","to":300}
```

Deadlines are enforced by the arena with socket read timeouts; the protocol
carries the deadline so well-behaved bots can self-limit.

## 7. Arena

```rust
pub struct MatchConfig {
    pub spec: GameSpec,
    pub hands: u64,               // decks dealt (duplicate multiplies actual hands)
    pub seed: u64,
    pub dealing: DealingMode,     // Seeded | Duplicate
    pub timeout: Duration,
    pub fault_policy: FaultPolicy,   // CheckFold (default) | Forfeit
    pub stack_depth: Chips,
}
```

- **Seeded**: independent hands, button rotates; per-hand nets are the
  observations.
- **Duplicate**: each deck is replayed with seats rotated through all N
  positions (heads-up: mirror pairs). One deck's rotation-set sums to a single
  observation per bot — this is what kills variance. Draw/stud replays reuse
  the same deck order; post-divergence dealing naturally differs (standard
  duplicate-poker behavior).
- **Faults** (illegal action, timeout, disconnect, crash, protocol garbage):
  - `CheckFold`: forced check if free, else fold; match continues; fault
    counts reported at the end.
  - `Forfeit`: match ends, offender loses.
- Timing: wall-clock per action for wire bots (blocking read with deadline).
  In-process bots are trusted; their time is measured and reported but not
  preemptible (documented).

## 8. Statistics & output

- **Winnings**: per-observation net in BB/100 (fixed-limit: big bets/100),
  mean ± two-sided 95% Student-t interval. The headline output answers "is A
  better than B, and is the difference significant".
- **Behavior** per bot: VPIP, PFR (flop games), aggression factor
  (bets+raises)/calls, went-to-showdown %, won-at-showdown %, fold rate.
- **Hand log** (`--log FILE`): full event stream per hand in a line-based
  text format (readable + machine-parseable); exact replay input for debugging.
- Progress line during long matches (`--progress-every N`).

## 9. CLI

```sh
poker-arena run \
  --game holdem-nl --seats 2 --hands 10000 --seed 42 \
  --dealing duplicate --timeout-ms 1000 --fault-policy check-fold \
  --bot cmd:"python3 mybot.py" --bot builtin:caller \
  --log hands.log --progress-every 1000

poker-arena games            # list registered variants + parameters
```

Bot specifiers: `builtin:NAME`, `tcp:PORT`, `cmd:"COMMAND"`. Seat order
follows argument order (seat 0, 1, …).

## 10. Milestones

- **M1 — Foundation (heads-up hold'em, end to end):** cards, RNG, evaluators
  (high, A-5, 2-7, badugi, 8ob — all of them, they're small and testable),
  GameSpec + HandState with full betting engine (limit + NL, all-ins, side
  pots), Bot trait + builtins, runner with seeded + mirror duplicate, winnings
  stats + CI, CLI `run`/`games`, hand log. *Deliverable: two builtin bots play
  10k NLHE hands and the CLI declares a significant winner.*
- **M2 — Breadth:** multiplayer 3–9 with rotation duplicate, Omaha/Omaha8
  (pot-limit, exactly-two, hi-lo split), wire protocol (TCP + stdio) with
  fault policies and timeouts.
- **M3 — Full registry + polish:** stud family (bring-in, upcards, by-upcard
  ordering), draw family (draw streets, badugi), behavioral stats, docs.

Each milestone ends with the full test suite green and a runnable demo.
