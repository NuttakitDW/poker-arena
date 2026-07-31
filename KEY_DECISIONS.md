# Key decisions

The design decisions that shape poker-arena, with the reasoning that picked
them. This is a maintained record, not a changelog: when a decision here is
revisited, edit the entry (and say why it changed) rather than appending
history. Add an entry whenever a choice would surprise a competent newcomer
or forecloses an alternative someone might reasonably expect.

## Architecture

**Four crates with a strict dependency direction: `wire ← core ← arena ←
cli`.** `poker-wire` is the *vocabulary* — cards, actions, events, stakes,
plus the protocol messages and framing that carry them — and depends on
nothing but serde; `poker-core` is the *machinery* that answers "what are
the rules of poker" (pure, no I/O) in terms of that vocabulary;
`poker-arena` answers "how do we run a competition"; `poker-arena-cli` is
the binary shell.

The direction points this way because of who needs the smaller half: a bot
client needs the words, not the rules engine, so `poker-wire` alone is a
complete dependency for writing a bot in Rust, while solvers and replay
tools take core on top. It also collapses a whole category of bug. The
arrow used to run `core ← wire`, which forced wire to define `WireEvent`, a
deserializable mirror of core's `Serialize`-only `Event`, held in sync by a
`wire_event_fidelity` test asserting the two serialized byte-for-byte
identically. Inverting the dependency let `Event` become one type that the
engine emits and bots deserialize — the mirror, its `From` impls, and the
test that policed them are all gone, because two things that cannot differ
need no test proving they don't.

**Variants are data, not code.** A `GameSpec` is a sequence of streets
(deal + optional betting round), a betting structure, forced bets, and a
showdown rule; one engine interprets all of it. Adding a game means writing
a constructor, not engine code — all nineteen registry games share the same
`HandState`. Family quirks (bring-in, upcard ordering, draw phases) are
enum-encoded hooks the engine understands, not per-variant subclasses.

**No async runtime.** Poker is strictly turn-based — one decision at a
time — so blocking I/O plus a reader thread per wire bot (deadlines via
`recv_timeout` on a channel, identical for sockets and pipes) covers
everything tokio would, with far less machinery.

**The engine owns its RNG.** Deck shuffling and draw reshuffles use an
in-crate xoshiro256\*\* seeded via splitmix64, not an external RNG crate,
because seed → identical deals is a forever promise and external crates
don't guarantee stream stability across versions. A frozen snapshot test
fails if the stream ever changes. Corollary: the whole match — cards,
events, stats — replays byte-identically from one seed.

## Protocol

**JSON Lines, optimized for bot-author friction, not bytes.** Throughput is
irrelevant (~10 messages/hand, localhost); every language reads a line and
calls its JSON parser, so a working bot is ~30 lines with no dependencies
or codegen. Strict framing (one object per `\n` line, 64 KiB cap), a
version field, unknown fields ignored, unknown message types tolerated.
The `hello` is deliberately lean — the game id plus only per-match
parameters (stakes, betting structure and cap, seats, stack, timeout):
bots are expected to know a game's rules from its id, so serializing the
full street/showdown structure was rejected as teaching bots what they
must already know. `act` is decision-tagged (`wager` / `draw` /
`bring-in`) so each turn is self-describing instead of a bag of Options.

**The event stream is the single source of truth.** `act` carries only
`hand_no`, `seat`, the tagged `decision`, and `deadline_ms`; `hand-start`
only `hand_no` and `seat`. Table state (cards, board, stacks, pot, folds)
is reconstructible from events, and shipping it twice invited
snapshot-vs-events drift. Exception, deliberate: the legal-action bounds
ride along inside `decision` because **action legality is
arena-authoritative** — a bot deriving its own legality is how
off-by-one-chip faults happen. The in-process
`ActionRequest` stays rich: borrowed slices cost nothing and can't drift.
Corollary: hand-history logs and transcripts record only the event stream
(actions appear as `acted` events); `act` requests are per-bot derived
state, and anything they carried — legal bounds included — is exactly
recomputable by replaying the transcript through `HandState`.

**The button always sits at seat 0.** The arena rotates *bots* through
seats between hands, never the button, so a seat number is also a position
(seat 0 = button, seat 1 = small blind…). One convention instead of two
moving parts; documented as a protocol invariant. The engine itself keeps a
general `button` parameter — it is a pure rules machine that tests and
solvers may drive with any geometry.

## Fairness & statistics

**Duplicate dealing is the variance killer.** Each deck is replayed once
per cyclic seat rotation, so every bot plays the same cards from every
position; the rotation-set mean is a single statistical observation.
Headline result: mean net per 100 hands with a two-sided 95% Student-t
interval. **Units** (owner ruling): statistics accumulate and serialize in
*chips* — the canonical, ambiguity-free unit (a `BB`-vs-`bb` case
distinction in JSON is a bug generator); the CLI displays the conventional
poker unit, big bets ("BB/100") for fixed-limit games and big blinds
("bb/100") for pot/no-limit, via `GameSpec::rate_unit`.

**Per-deck random arrangement on top of rotation (multiway).** Cyclic
rotation alone preserves the circular order of bots, so "who acts after
whom" never averages out — sitting behind the maniac is a persistent edge.
Each deck therefore draws a seeded-random arrangement of bots and plays its
rotations; positional fairness stays exact per deck, neighbor effects
average out across decks. Exhaustive `n!` seating (ACPC-style for 3-max)
was rejected as intractable beyond 4 seats.

**Duplicate replays fix the deck, not the outcomes.** A rotation replays
the same shuffled card *order*; consumption depends on actions. In draw
games, a player drawing 2 vs 3 shifts every later replacement; in stud,
folds change who receives later cards. "Same draw result" is not even
well-defined across rotations (different bots discard differently), and
per-seat replacement piles were rejected: multiway stubs are too small
(6-max triple draw: 22 stub cards vs up to 15 draws per player), and
isolating streams changes card-removal reasoning — a different game.
Consequence, accepted: duplicate's variance reduction is exact up to the
first behavioral divergence — full strength for community-card games
(boards are action-independent), gracefully degraded for draw/stud.

**Stacks reset every hand.** Bot comparison measures per-hand EV, not
bankroll trajectories; resets keep observations i.i.d. and make duplicate
replays meaningful. Depth is configurable (default 100 BB). Corollary:
because every active seat then always has identical remaining capacity, a
short all-in can only set the price at everyone's ceiling — so the
reopening rules and side pots are *unreachable in arena matches as
configured*. They are implemented and tested anyway: `poker-core`'s
contract is correctness for arbitrary stacks (solvers and uneven-stack
formats hit these paths), and the equal-stack invariant is the runner's
policy, not the engine's promise.

**Seeds are random by default and always printed.** A fixed default seed
silently replays identical hands when users run "more" matches. Unpinned
runs draw entropy, announce the seed up front, and print it with results,
so every run is reproducible after the fact via `--seed`.

## Competition semantics

**Faults are never silently patched.** An illegal action, timeout,
disconnect, or malformed reply costs a fault and is handled by policy:
substitute the decision family's minimal legal action (check, else fold;
stand pat at a draw; bring-in at a bring-in decision) and continue, or
forfeit the match. Fault counts are reported — a bot that "wins" while
faulting is visibly broken.

**One `Bot` trait for everything.** Wire bots implement the same trait the
builtins do (`act` returns `Result<Action, BotFault>` so transports can
report failure); the runner never distinguishes. A timeout must not desync
the connection: stale answers are drained before each new `act` so a late
reply is never mistaken for the next answer.

**All showdown hands are revealed.** No strategic mucking: an arena
optimizes for honest statistics and debuggability, not concealment between
rival bots across hands.

## Rules choices

**Evaluators are correctness-first.** Brute-force best-5-of-N over a
straightforward 5-card classifier, pinned by the exhaustive C(52,5)
frequency test; no lookup-table heroics, because bot thinking time dwarfs
evaluation time. `HandValue` encodings are frozen (low evaluators invert
internally so greater-always-wins holds everywhere).

**Open-folding is disallowed.** Folding is legal only when facing chips to
call. Folding a free check is never correct and almost always a bot bug —
surfacing it as a fault beats letting it pass.

**Short all-ins never lower the price; reopening is cumulative.** A short
all-in blind or bring-in leaves the nominal price intact for everyone
else. A single short all-in below a full raise doesn't reopen betting for
players who already acted and never moves the min-raise base — but
reopening is judged *cumulatively* (TDA-style): a seat that already acted
may re-raise once the price has risen by at least one full raise
(no-limit/pot-limit) or half a bet (fixed limit) since its last action.
Example: bet 500, raise to 1200, all-ins to 1700 and 2000 — the original
raiser faces +800 ≥ 700 and may re-raise (minimum 2700).

**Fixed-limit raises are additive, not ladder-quantized.** A raise is
always to `current price + one bet`: after a 100 bet and a 170 all-in, the
re-raise is to 270 (the TDA ladder/completion model — snapping raises to
tier multiples — was considered and rejected by the project owner). The
half-bet rule classifies a short all-in: at least half a bet above the
price is a raise (consumes a cap slot, reopens action); below half is a
call-plus-extra (no slot, no reopening by itself, cumulative rule above
still applies). The cap counts full wagers — the big blind preflop and
the stud completion are the first — and at the cap it is call or fold, no
exceptions. The bring-in consumes no slot; a raise made while no full
wager exists yet this street is to one bet flat (the completion). The cap
defaults to 4, is configurable per match (`--raise-cap`, 0 = uncapped),
and is announced to bots in the wire `hello`'s betting structure.

**The bring-in gets no option.** If everyone just calls the bring-in, the
round ends — unlike the big blind, the bring-in was that player's own
chosen bet, and nobody may raise their own bet. (The BB option exists
precisely because a blind is not a chosen bet.)

**Split-pot semantics are symmetric-qualifier.** `HiLo` splits award each
half to its best *qualifying* hand; total evaluators (badugi, plain lows,
high) always qualify, so badacey/badeucy split unconditionally while
omaha8/stud8 keep the classic "no low → high scoops". When both sides are
qualifiers (archie: sixes-or-better high, eight-or-better low), one
qualifying side scoops, and if *neither* qualifies the pot splits evenly
among the showdown players — pot carryover was rejected as incompatible
with per-hand scoring and i.i.d. observations. Badeucy plays aces high in
*both* halves (nut badugi 5-4-3-2 rainbow), badacey aces low in both;
that pairing is what distinguishes the two games. Slot convention (the hi
slot takes the odd chip): in badacey/badeucy and all drawmaha variants
the **five-card-hand half is the hi slot** — badugi and the drawmaha
omaha half sit in lo (owner ruling); archie and the omaha8/stud8/Big O
family keep the classic high-hand-in-hi arrangement.

**Stud seats cap at 7; draw games reshuffle the muck.** 7 × 7 = 49 ≤ 52,
so stud never exhausts the deck (the 8-handed shared-community-card
fallback was deliberately dropped). Draw games can exhaust it; the engine
reshuffles the muck — draw discards *and* folded hands, excluding only the
drawing seat's own just-discarded cards — matching real-table practice and
deepening the pile. Determinism is preserved because the reshuffle uses
the hand's seeded RNG stream (match seed ⊕ salt, per hand number): a
reshuffled deck is a *fresh deterministic shuffle of the pile*, not a
reuse of the original deck order, so identical runs replay byte-identically
while duplicate rotations diverge only where actions already diverged.

**Stakes are a two-shape enum.** Blind games post blinds; stud games post
antes, a bring-in, and explicit bet tiers — pretending stud stakes are
"blinds" left fields meaningless and amounts underivable. Constructors
normalize between shapes using the standard conventions (fixed-limit:
small bet = big blind, big bet = 2×; stud defaults: ante = small bet / 5,
bring-in = small bet / 2).

## Conventions

**Singular file names** (`card.rs`, `event.rs`, `pot.rs`). Project-wide,
including delegated work.

**Docs describe the present.** Internal milestone labels don't belong in
shipped docs or comments; `README.md` states features, `DESIGN.md` describes
the architecture, this file keeps the reasoning.
