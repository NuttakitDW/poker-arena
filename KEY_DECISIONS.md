# Key decisions

The design decisions that shape poker-arena, with the reasoning that picked
them. This is a maintained record, not a changelog: when a decision here is
revisited, edit the entry (and say why it changed) rather than appending
history. Add an entry whenever a choice would surprise a competent newcomer
or forecloses an alternative someone might reasonably expect.

## Architecture

**Four crates with a strict dependency direction.** `poker-core` answers
"what are the rules of poker" (pure, no I/O); `poker-wire` answers "how are
messages encoded" (transport-agnostic); `poker-arena` answers "how do we run
a competition"; `poker-arena-cli` is the binary shell. Core (and core +
wire) stay reusable by solvers, replay tools, and Rust bot clients without
dragging in match machinery.

**Variants are data, not code.** A `GameSpec` is a sequence of streets
(deal + optional betting round), a betting structure, forced bets, and a
showdown rule; one engine interprets all of it. Adding a game means writing
a constructor, not engine code — all twelve registry games share the same
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

**The event stream is the single source of truth.** `act` carries only
`hand_no`, `seat`, `legal`, and `deadline_ms`; `hand-start` only `hand_no`
and `seat`. Table state (cards, board, stacks, pot, folds) is
reconstructible from events, and shipping it twice invited
snapshot-vs-events drift. Exception, deliberate: `legal` rides along
because **action legality is arena-authoritative** — a bot deriving its own
legality is how off-by-one-chip faults happen. The in-process
`ActionRequest` stays rich: borrowed slices cost nothing and can't drift.

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
Headline result: mean net in big blinds per hand with a two-sided 95%
Student-t interval. Fixed-limit and stud rates are normalized in small
bets (`Stakes::rate_unit`).

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
replays meaningful. Depth is configurable (default 100 BB).

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

**Short all-ins never lower the price, and don't reopen action.** A short
all-in blind or bring-in leaves the nominal price intact for everyone else;
a short all-in wager below a full raise doesn't reopen betting for players
who already acted (and doesn't move the min-raise base). Fixed limit uses
the half-bet rule; the bring-in doesn't count toward the cap and a
completion counts as the first wager (keyed on "no full wager yet", which
also gives sub-half-bet all-in openings the standard completion behavior).

**The bring-in gets no option.** If everyone just calls the bring-in, the
round ends — unlike the big blind, the bring-in was that player's own
chosen bet, and nobody may raise their own bet. (The BB option exists
precisely because a blind is not a chosen bet.)

**Stud seats cap at 7; draw games reshuffle discards.** 7 × 7 = 49 ≤ 52,
so stud never exhausts the deck (the 8-handed shared-community-card
fallback was deliberately dropped). Draw games can exhaust it; the engine
reshuffles the discard pile — excluding the drawing seat's own
just-discarded cards — with the hand's seeded RNG, preserving determinism.

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
shipped docs or comments; `README.md` states features, `DESIGN.md` keeps
the roadmap, this file keeps the reasoning.
